//! Exchange Online: mailboxes and distribution lists.
//!
//! Exchange Online has no documented REST API for this, so it uses the endpoint the Exchange
//! Online PowerShell module calls (`/adminapi/beta/{tenant}/InvokeCommand`), which runs a
//! cmdlet with named parameters and returns its output as JSON. Everything that depends on that
//! endpoint is in this file.

use crate::app::App;
use crate::auth::Api;
use crate::dialogs::{Dialog, Field, Form, PickItem, Picker};
use crate::graph::{Graph, MAX_ITEMS};
use crate::i18n::{t, tf};
use crate::model::{s, yes_no, Node};
use crate::ops::civil_from_days;
use crate::sheets::{table, SheetRow, TabBody, TabKind, TabOp};
use anyhow::{anyhow, bail, Result};
use reqwest::Method;
use serde_json::{json, Map, Value};

/// The system mailbox Exchange Online PowerShell routes admin requests through.
const ANCHOR: &str = "SystemMailbox{bb558c35-97f1-4cb9-8ff7-d53741dc928c}";

const MAILBOX_TYPES: [&str; 4] = ["UserMailbox", "SharedMailbox", "RoomMailbox", "EquipmentMailbox"];

impl Graph {
    /// Runs an Exchange Online cmdlet with `params` (named as in PowerShell) and returns the
    /// objects it outputs.
    pub fn invoke(&self, cmdlet: &str, params: Value) -> Result<Vec<Value>> {
        self.invoke_capped(cmdlet, params).map(|(items, _)| items)
    }

    /// Like [`Graph::invoke`], also saying whether the output stopped at [`MAX_ITEMS`].
    fn invoke_capped(&self, cmdlet: &str, params: Value) -> Result<(Vec<Value>, bool)> {
        let (tenant, domain) = self.exchange_tenant()?;
        let anchor = format!("UPN:{ANCHOR}@{domain}");
        let headers = [("X-AnchorMailbox", anchor.as_str()), ("X-ResponseFormat", "json")];
        let body = json!({ "CmdletInput": { "CmdletName": cmdlet, "Parameters": params } });
        let mut url = format!("{}/adminapi/beta/{tenant}/InvokeCommand", self.exchange_endpoint());
        let mut out = Vec::new();
        loop {
            let v = self.send_to(Api::Exchange, Method::POST, &url, cmdlet, Some(&body), &headers)?;
            if let Some(items) = v["value"].as_array() {
                out.extend(items.iter().cloned());
            }
            let Some(next) = v["@odata.nextLink"].as_str() else { return Ok((out, false)) };
            if out.len() >= MAX_ITEMS {
                return Ok((out, true));
            }
            url = next.to_string();
        }
    }

    /// The tenant ID and initial (onmicrosoft.com) domain, read once from Graph.
    fn exchange_tenant(&self) -> Result<(String, String)> {
        if let Some(t) = self.exchange_tenant.get() {
            return Ok(t.clone());
        }
        let org = self.get("/organization?$select=id,verifiedDomains")?;
        let org = &org["value"][0];
        let domain = org["verifiedDomains"]
            .as_array()
            .and_then(|d| d.iter().find(|d| d["isInitial"] == true))
            .map(|d| s(d, "name"))
            .ok_or_else(|| anyhow!(t("exchange.no_initial_domain")))?;
        let t = (s(org, "id"), domain);
        let _ = self.exchange_tenant.set(t.clone());
        Ok(t)
    }
}

/// A parameter value that adds to or removes from a multi-valued property, like PowerShell's
/// `@{Add="ann@contoso.com"}`. `op` is "Add" or "Remove".
fn add_remove(op: &str, value: &str) -> Value {
    let mut m = Map::new();
    m.insert("@odata.type".into(), json!("#Exchange.GenericHashTable"));
    m.insert(op.into(), json!(value));
    Value::Object(m)
}

/// What cmdlets are given as `-Identity` for a mailbox or group.
pub fn identity(v: &Value) -> String {
    match s(v, "PrimarySmtpAddress") {
        a if a.is_empty() => s(v, "Guid"),
        a => a,
    }
}

/// Gives an object from Exchange the `id` the app keys objects on: its Entra object ID.
fn with_id(mut v: Value) -> Value {
    let id = match s(&v, "ExternalDirectoryObjectId") {
        x if x.is_empty() => s(&v, "Guid"),
        x => x,
    };
    if v.is_object() {
        v["id"] = json!(id);
    }
    v
}

/// An address without its `smtp:` prefix.
fn bare(a: &str) -> &str {
    a.split_once(':').map_or(a, |(_, x)| x)
}

/// Where a mailbox's email is forwarded: an external address, or a recipient in the organisation.
pub fn forwarding(v: &Value) -> String {
    match s(v, "ForwardingSmtpAddress") {
        f if f.is_empty() => s(v, "ForwardingAddress"),
        f => bare(&f).to_string(),
    }
}

pub fn mailbox_type(v: &Value) -> String {
    match s(v, "RecipientTypeDetails").as_str() {
        "UserMailbox" => t("value.mailbox_user"),
        "SharedMailbox" => t("value.mailbox_shared"),
        "RoomMailbox" => t("value.mailbox_room"),
        "EquipmentMailbox" => t("value.mailbox_equipment"),
        other => other.into(),
    }
}

pub fn group_type(v: &Value) -> String {
    match s(v, "RecipientTypeDetails").as_str() {
        "MailUniversalDistributionGroup" => t("value.distribution_list"),
        "MailUniversalSecurityGroup" => t("value.group_mail_security"),
        "RoomList" => t("value.room_list"),
        other => other.into(),
    }
}

/// The SMTP addresses of a recipient, primary first.
fn addresses(v: &Value) -> String {
    let mut list: Vec<&str> = v["EmailAddresses"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter(|a| a.to_ascii_lowercase().starts_with("smtp:"))
        .collect();
    // The primary address is the one with "SMTP:" in capitals.
    list.sort_by_key(|a| !a.starts_with("SMTP:"));
    list.iter().map(|a| bare(a)).collect::<Vec<_>>().join(", ")
}

fn list_text(v: &Value) -> String {
    match v.as_array() {
        Some(a) => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(", "),
        None => v.as_str().map(str::to_string).unwrap_or_default(),
    }
}

/// A time from Exchange as "YYYY-MM-DD HH:MM". Accepts ISO 8601 and the `/Date(ms)/` form.
fn exo_time(v: &Value) -> String {
    let text = v.as_str().unwrap_or("");
    if let Some(ms) = text.strip_prefix("/Date(").and_then(|t| t.split([')', '+', '-']).next()).and_then(|t| t.parse::<i64>().ok()) {
        let secs = ms.div_euclid(1000);
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let day = secs.rem_euclid(86_400);
        return format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", day / 3600, day % 3600 / 60);
    }
    text.get(..16).unwrap_or(text).replace('T', " ")
}

/// Reads "YYYY-MM-DD HH:MM" as a UTC time for a cmdlet parameter.
pub(crate) fn parse_time(text: &str) -> Option<String> {
    let t = text.trim().replace('T', " ");
    let b = t.as_bytes();
    let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15];
    let ok = b.len() == 16
        && digits.iter().all(|&i| b[i].is_ascii_digit())
        && b[4] == b'-' && b[7] == b'-' && b[10] == b' ' && b[13] == b':';
    ok.then(|| format!("{}T{}:00Z", &t[..10], &t[11..]))
}

/// Plain text from the HTML Exchange keeps automatic replies in.
fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('>') else { break };
        let tag = rest[i + 1..i + j].to_ascii_lowercase();
        if tag.starts_with("br") || tag.starts_with("/p") || tag.starts_with("/div") {
            out.push('\n');
        }
        rest = &rest[i + j + 1..];
        // Outlook on the web puts a <style> block in replies; its text isn't part of the message.
        for hidden in ["style", "head", "script"] {
            if tag.split([' ', '/']).next() == Some(hidden) {
                let end = format!("</{hidden}");
                rest = match rest.to_ascii_lowercase().find(&end) {
                    Some(k) => &rest[k..],
                    None => "",
                };
            }
        }
    }
    if !rest.contains('<') {
        out.push_str(rest);
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.trim().to_string()
}

/// HTML for a reply typed as plain text, keeping its line breaks.
fn text_html(text: &str) -> String {
    let escaped = text.trim().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!("<html><body>{}</body></html>", escaped.replace("\r\n", "\n").replace('\n', "<br>"))
}

fn is_system_account(who: &str) -> bool {
    who.starts_with("NT AUTHORITY\\") || who.starts_with("S-1-5-")
}

/// Loads the list for an Exchange node (runs on a worker thread).
pub fn list(g: &Graph, node: Node) -> Result<(Vec<Value>, bool)> {
    let (rows, more) = match node {
        Node::Mailboxes => g.invoke_capped("Get-Mailbox", json!({ "ResultSize": "Unlimited", "RecipientTypeDetails": MAILBOX_TYPES }))?,
        Node::DistributionLists => g.invoke_capped("Get-DistributionGroup", json!({ "ResultSize": "Unlimited" }))?,
        _ => bail!("{node:?} is not an Exchange list"),
    };
    Ok((rows.into_iter().map(with_id).collect(), more))
}

/// Fetches a property sheet tab for a mailbox or distribution list (runs on a worker thread).
pub fn load_tab(g: &Graph, node: Node, kind: TabKind, obj: &Value) -> Result<(TabBody, Option<Value>)> {
    let id = identity(obj);
    let one = |cmdlet: &str| -> Result<Value> {
        g.invoke(cmdlet, json!({ "Identity": id }))?.into_iter().next().ok_or_else(|| anyhow!(tf("exchange.not_found", &[("name", &id)])))
    };
    let field = |label: &str, value: String| (t(label), value);
    let body = match (node, kind) {
        (Node::Mailboxes, TabKind::General) => {
            let m = with_id(one("Get-Mailbox")?);
            let fields = vec![
                field("prop.display_name", s(&m, "DisplayName")),
                field("prop.email_address", s(&m, "PrimarySmtpAddress")),
                field("prop.type", mailbox_type(&m)),
                field("prop.email_addresses", addresses(&m)),
                field("prop.alias", s(&m, "Alias")),
                field("prop.hidden", yes_no(&m["HiddenFromAddressListsEnabled"])),
                field("prop.forwarding_to", forwarding(&m)),
                field("prop.keep_copy", yes_no(&m["DeliverToMailboxAndForward"])),
                field("prop.archive", s(&m, "ArchiveStatus")),
                field("prop.litigation_hold", yes_no(&m["LitigationHoldEnabled"])),
                field("prop.object_id", s(&m, "ExternalDirectoryObjectId")),
            ];
            return Ok((TabBody::Fields(fields), Some(m)));
        }
        (Node::Mailboxes, TabKind::MailboxAccess) => mailbox_permissions(g, &id)?,
        (Node::Mailboxes, TabKind::AutoReplies) => {
            let c = one("Get-MailboxAutoReplyConfiguration")?;
            let state = s(&c, "AutoReplyState");
            let mut fields = vec![field("tab.auto_replies", match state.as_str() {
                "Enabled" => t("replies.on"),
                "Scheduled" => t("replies.scheduled_below"),
                "Disabled" => t("replies.off"),
                _ => state.clone(),
            })];
            if state == "Scheduled" {
                fields.push(field("replies.start", exo_time(&c["StartTime"])));
                fields.push(field("replies.end", exo_time(&c["EndTime"])));
            }
            fields.push(field("replies.inside", html_text(&s(&c, "InternalMessage"))));
            fields.push(field("replies.outside", match s(&c, "ExternalAudience").as_str() {
                "None" => t("replies.dont_reply"),
                "Known" => tf("replies.known_only", &[("message", &html_text(&s(&c, "ExternalMessage")))]),
                _ => html_text(&s(&c, "ExternalMessage")),
            }));
            TabBody::Fields(fields)
        }
        (Node::DistributionLists, TabKind::General) => {
            let d = with_id(one("Get-DistributionGroup")?);
            let fields = vec![
                field("prop.display_name", s(&d, "DisplayName")),
                field("prop.email_address", s(&d, "PrimarySmtpAddress")),
                field("prop.type", group_type(&d)),
                field("prop.email_addresses", addresses(&d)),
                field("prop.alias", s(&d, "Alias")),
                field("prop.internal_only", yes_no(&d["RequireSenderAuthenticationEnabled"])),
                field("prop.hidden", yes_no(&d["HiddenFromAddressListsEnabled"])),
                field("prop.owners", list_text(&d["ManagedBy"])),
                field("prop.object_id", s(&d, "ExternalDirectoryObjectId")),
            ];
            return Ok((TabBody::Fields(fields), Some(d)));
        }
        (Node::DistributionLists, TabKind::ListMembers) => {
            let items = g.invoke("Get-DistributionGroupMember", json!({ "Identity": id, "ResultSize": "Unlimited" }))?;
            table(vec!["col.name", "col.email_address", "col.type"], items, |v| {
                vec![s(v, "DisplayName"), s(v, "PrimarySmtpAddress"), s(v, "RecipientTypeDetails")]
            })
        }
        _ => TabBody::Fields(Vec::new()),
    };
    Ok((body, None))
}

/// Full Access, Send As and Send on behalf on a mailbox. Each row's data has the `user` and the
/// `kind` of permission ("full", "sendas" or "behalf"), for removing it.
fn mailbox_permissions(g: &Graph, id: &str) -> Result<TabBody> {
    let mut rows = Vec::new();
    let mut add = |who: String, kind: &str, label: &str| {
        let data = json!({ "user": who, "kind": kind });
        rows.push(SheetRow { cells: vec![who, t(label)], data });
    };
    for p in g.invoke("Get-MailboxPermission", json!({ "Identity": id }))? {
        let who = s(&p, "User");
        if p["IsInherited"] != true && p["Deny"] != true && p["AccessRights"].to_string().contains("FullAccess") && !is_system_account(&who) {
            add(who, "full", "exchange.full_access");
        }
    }
    for p in g.invoke("Get-RecipientPermission", json!({ "Identity": id }))? {
        let who = s(&p, "Trustee");
        if p["IsInherited"] != true && s(&p, "AccessControlType") != "Deny" && p["AccessRights"].to_string().contains("SendAs") && !is_system_account(&who) {
            add(who, "sendas", "exchange.send_as");
        }
    }
    let mailbox = g.invoke("Get-Mailbox", json!({ "Identity": id }))?;
    for who in mailbox.first().and_then(|m| m["GrantSendOnBehalfTo"].as_array()).map(Vec::as_slice).unwrap_or_default() {
        if let Some(who) = who.as_str() {
            add(who.to_string(), "behalf", "exchange.send_on_behalf");
        }
    }
    rows.sort_by_cached_key(|r| (r.cells[0].to_lowercase(), r.cells[1].clone()));
    Ok(TabBody::Table { headers: vec!["col.user", "col.permission"], rows })
}

impl App {
    /// Shows the domains of the tenant as a choice, or a text box before they have loaded.
    fn domain_field(&self) -> Field {
        let domains: Vec<(String, String)> = self.domains.iter().map(|d| (d.clone(), format!("@{d}"))).collect();
        if domains.is_empty() {
            Field::text("domain", &t("field.domain"), "")
        } else {
            Field::choice("domain", &t("field.domain"), domains, 0)
        }
    }

    pub fn new_shared_mailbox(&mut self) {
        let form = Form::new(&t("exchange.new_mailbox_title"), &t("dialog.create"))
            .intro(&t("exchange.new_mailbox_intro"))
            .field(Field::text("name", &t("field.display_name"), "").required())
            .field(Field::text("alias", &t("field.email_name"), "").required())
            .field(self.domain_field())
            .on_submit(|app, f| {
                let (name, address) = new_address(f)?;
                let params = json!({
                    "Shared": true,
                    "Name": name,
                    "DisplayName": name,
                    "Alias": f.text("alias"),
                    "PrimarySmtpAddress": address,
                });
                app.spawn(t("exchange.creating_mailbox"), move |g| g.invoke("New-Mailbox", params), move |app, r| match r {
                    Ok(made) => {
                        app.status = tf("exchange.created_mailbox", &[("address", &address)]);
                        if let Some(m) = made.into_iter().next() {
                            app.upsert(Node::Mailboxes, with_id(m));
                        } else {
                            app.load(Node::Mailboxes);
                        }
                    }
                    Err(e) => app.error(&t("exchange.new_mailbox_title"), e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn new_distribution_list(&mut self) {
        let types = vec![
            ("Distribution".to_string(), t("value.distribution_list")),
            ("Security".to_string(), t("value.mail_security_group")),
        ];
        let form = Form::new(&t("exchange.new_list_title"), &t("dialog.create"))
            .field(Field::choice("type", &t("field.type"), types, 0))
            .field(Field::text("name", &t("field.display_name"), "").required())
            .field(Field::text("alias", &t("field.email_name"), "").required())
            .field(self.domain_field())
            .field(Field::check("internal", &t("prop.internal_only"), true))
            .on_submit(|app, f| {
                let (name, address) = new_address(f)?;
                let params = json!({
                    "Type": f.choice("type"),
                    "Name": name,
                    "DisplayName": name,
                    "Alias": f.text("alias"),
                    "PrimarySmtpAddress": address,
                    "RequireSenderAuthenticationEnabled": f.checked("internal"),
                });
                app.spawn(t("exchange.creating_list"), move |g| g.invoke("New-DistributionGroup", params), move |app, r| match r {
                    Ok(made) => {
                        app.status = tf("exchange.created_list", &[("address", &address)]);
                        if let Some(d) = made.into_iter().next() {
                            app.upsert(Node::DistributionLists, with_id(d));
                        } else {
                            app.load(Node::DistributionLists);
                        }
                    }
                    Err(e) => app.error(&t("exchange.new_list_title"), e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    /// Display name, address-list visibility and forwarding of a mailbox.
    pub fn edit_mailbox(&mut self, obj: Value, after: impl Fn(&mut App) + Clone + Send + 'static) {
        let current = forwarding(&obj);
        let form = Form::new(&tf("sheet.edit_title", &[("name", &s(&obj, "DisplayName"))]), &t("dialog.save"))
            .intro(&t("exchange.forward_intro"))
            .field(Field::text("DisplayName", &t("field.display_name"), &s(&obj, "DisplayName")).required())
            .field(Field::check("HiddenFromAddressListsEnabled", &t("field.hide"), obj["HiddenFromAddressListsEnabled"] == true))
            .field(Field::text("forward", &t("field.forward_to"), &current))
            .field(Field::check("DeliverToMailboxAndForward", &t("field.keep_copy"), obj["DeliverToMailboxAndForward"] == true))
            .on_submit(move |app, f| {
                let mut params = json!({ "Identity": identity(&obj) });
                if f.text("DisplayName") != s(&obj, "DisplayName") {
                    params["DisplayName"] = json!(f.text("DisplayName"));
                }
                for key in ["HiddenFromAddressListsEnabled", "DeliverToMailboxAndForward"] {
                    if f.checked(key) != (obj[key] == true) {
                        params[key] = json!(f.checked(key));
                    }
                }
                let forward = f.text("forward");
                if forward != current {
                    if !forward.is_empty() && (!forward.contains('@') || forward.contains(' ')) {
                        return Err(t("exchange.bad_forward"));
                    }
                    // Forwarding is set one way or the other, so clear the other way.
                    params["ForwardingAddress"] = Value::Null;
                    params["ForwardingSmtpAddress"] = if forward.is_empty() { Value::Null } else { json!(format!("smtp:{forward}")) };
                }
                let mut updated = params.clone();
                if let Some(m) = updated.as_object_mut() {
                    m.remove("Identity");
                }
                updated["id"] = obj["id"].clone();
                let after = after.clone();
                app.mutate(&t("task.save_mailbox"), move |g| g.invoke("Set-Mailbox", params).map(|_| ()), move |app| {
                    app.upsert(Node::Mailboxes, updated);
                    after(app);
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn edit_auto_replies(&mut self, obj: Value, after: impl Fn(&mut App) + Clone + Send + 'static) {
        let id = identity(&obj);
        let name = s(&obj, "DisplayName");
        let get = id.clone();
        self.spawn(
            tf("status.loading_tab", &[("tab", &t("tab.auto_replies"))]),
            move |g| g.invoke("Get-MailboxAutoReplyConfiguration", json!({ "Identity": get })),
            move |app, r| match r {
                Ok(c) => app.auto_replies_form(id, &name, c.into_iter().next().unwrap_or_default(), after),
                Err(e) => app.error(&t("tab.auto_replies"), e),
            },
        );
    }

    fn auto_replies_form(&mut self, id: String, name: &str, c: Value, after: impl Fn(&mut App) + Clone + Send + 'static) {
        let opts = |list: &[(&str, &str)]| list.iter().map(|(v, l)| (v.to_string(), t(l))).collect::<Vec<_>>();
        let states = opts(&[("Disabled", "replies.off"), ("Enabled", "replies.on"), ("Scheduled", "replies.scheduled")]);
        let audiences = opts(&[
            ("All", "replies.everyone"),
            ("Known", "replies.known"),
            ("None", "replies.dont_reply"),
        ]);
        let pos = |list: &[(String, String)], v: String| list.iter().position(|o| o.0 == v).unwrap_or(0);
        let state = pos(&states, s(&c, "AutoReplyState"));
        let audience = pos(&audiences, s(&c, "ExternalAudience"));
        let internal = html_text(&s(&c, "InternalMessage"));
        let external = html_text(&s(&c, "ExternalMessage"));
        let form = Form::new(&tf("replies.title", &[("name", &name)]), &t("dialog.save"))
            .intro(&t("replies.intro"))
            .field(Field::choice("state", &t("tab.auto_replies"), states, state))
            .field(Field::text("start", &t("replies.start"), &exo_time(&c["StartTime"])))
            .field(Field::text("end", &t("replies.end"), &exo_time(&c["EndTime"])))
            .field(Field::multiline("internal", &t("replies.inside"), &internal))
            .field(Field::choice("audience", &t("replies.audience"), audiences, audience))
            .field(Field::multiline("external", &t("replies.outside"), &external))
            .on_submit(move |app, f| {
                let mut params = json!({
                    "Identity": id,
                    "AutoReplyState": f.choice("state"),
                    "ExternalAudience": f.choice("audience"),
                });
                // Unchanged messages are left alone, keeping any formatting they have.
                if f.raw("internal").trim() != internal {
                    params["InternalMessage"] = json!(text_html(&f.raw("internal")));
                }
                if f.raw("external").trim() != external {
                    params["ExternalMessage"] = json!(text_html(&f.raw("external")));
                }
                if f.choice("state") == "Scheduled" {
                    let (Some(start), Some(end)) = (parse_time(&f.text("start")), parse_time(&f.text("end"))) else {
                        return Err(t("replies.bad_times"));
                    };
                    if end <= start {
                        return Err(t("replies.end_before_start"));
                    }
                    params["StartTime"] = json!(start);
                    params["EndTime"] = json!(end);
                }
                app.mutate(&t("task.save_replies"), move |g| g.invoke("Set-MailboxAutoReplyConfiguration", params).map(|_| ()), after.clone());
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn convert_mailbox(&mut self, obj: Value) {
        let kind = s(&obj, "RecipientTypeDetails");
        let name = s(&obj, "DisplayName");
        let (to, text) = match kind.as_str() {
            "UserMailbox" => ("Shared", tf("exchange.convert_to_shared", &[("name", &name)])),
            "SharedMailbox" => ("Regular", tf("exchange.convert_to_user", &[("name", &name)])),
            _ => {
                self.push(Dialog::message(&t("exchange.convert_title"), &t("exchange.convert_only")));
                return;
            }
        };
        let id = identity(&obj);
        let oid = obj["id"].clone();
        let new_kind = if to == "Shared" { "SharedMailbox" } else { "UserMailbox" };
        self.push(Dialog::confirm(&t("exchange.convert_title"), &text, move |app| {
            app.mutate(
                &tf("task.convert", &[("name", &name)]),
                move |g| g.invoke("Set-Mailbox", json!({ "Identity": id, "Type": to })).map(|_| ()),
                move |app| app.upsert(Node::Mailboxes, json!({ "id": oid, "RecipientTypeDetails": new_kind })),
            );
        }));
    }

    pub fn delete_exchange(&mut self, node: Node, obj: Value) {
        let name = s(&obj, "DisplayName");
        let (cmdlet, params, text) = match node {
            Node::Mailboxes if s(&obj, "RecipientTypeDetails") == "UserMailbox" => {
                self.push(Dialog::message(&t("exchange.delete_mailbox_title"), &t("exchange.delete_user_mailbox")));
                return;
            }
            Node::Mailboxes => {
                let key = match s(&obj, "RecipientTypeDetails").as_str() {
                    "SharedMailbox" => "exchange.delete_shared_mailbox",
                    "RoomMailbox" => "exchange.delete_room_mailbox",
                    "EquipmentMailbox" => "exchange.delete_equipment_mailbox",
                    _ => "exchange.delete_mailbox",
                };
                ("Remove-Mailbox", json!({ "Identity": identity(&obj), "Confirm": false }), tf(key, &[("name", &name)]))
            }
            Node::DistributionLists => (
                "Remove-DistributionGroup",
                json!({ "Identity": identity(&obj), "BypassSecurityGroupManagerCheck": true, "Confirm": false }),
                tf("exchange.delete_list", &[("name", &name)]),
            ),
            _ => return,
        };
        let id = s(&obj, "id");
        let title = t(if node == Node::Mailboxes { "exchange.delete_mailbox_title" } else { "exchange.delete_list_title" });
        self.push(Dialog::confirm(&title, &text, move |app| {
            app.mutate(&tf("task.delete", &[("name", &name)]), move |g| g.invoke(cmdlet, params).map(|_| ()), move |app| app.remove_obj(node, &id));
        }));
    }

    pub fn edit_distribution_list(&mut self, obj: Value, after: impl Fn(&mut App) + Clone + Send + 'static) {
        let form = Form::new(&tf("sheet.edit_title", &[("name", &s(&obj, "DisplayName"))]), &t("dialog.save"))
            .field(Field::text("DisplayName", &t("field.display_name"), &s(&obj, "DisplayName")).required())
            .field(Field::check(
                "RequireSenderAuthenticationEnabled",
                &t("prop.internal_only"),
                obj["RequireSenderAuthenticationEnabled"] == true,
            ))
            .field(Field::check("HiddenFromAddressListsEnabled", &t("field.hide"), obj["HiddenFromAddressListsEnabled"] == true))
            .on_submit(move |app, f| {
                let mut changes = json!({});
                if f.text("DisplayName") != s(&obj, "DisplayName") {
                    changes["DisplayName"] = json!(f.text("DisplayName"));
                }
                for key in ["RequireSenderAuthenticationEnabled", "HiddenFromAddressListsEnabled"] {
                    if f.checked(key) != (obj[key] == true) {
                        changes[key] = json!(f.checked(key));
                    }
                }
                let mut params = changes.clone();
                params["Identity"] = json!(identity(&obj));
                params["BypassSecurityGroupManagerCheck"] = json!(true);
                changes["id"] = obj["id"].clone();
                let after = after.clone();
                app.mutate(&t("task.save_list"), move |g| g.invoke("Set-DistributionGroup", params).map(|_| ()), move |app| {
                    app.upsert(Node::DistributionLists, changes);
                    after(app);
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn add_list_member(&mut self, obj: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = identity(&obj);
        self.pick_user_or_group(&tf("pick.add_member_to", &[("name", &s(&obj, "DisplayName"))]), move |app, m| {
            // The user principal name or group email address.
            let member = m.detail.clone();
            if member.is_empty() {
                let who = m.label.trim_start_matches(['👤', '👥', ' ']);
                app.push(Dialog::message(&t("task.add_member"), &tf("exchange.no_address", &[("name", &who)])));
                return;
            }
            let params = json!({ "Identity": id, "Member": member, "BypassSecurityGroupManagerCheck": true });
            app.mutate(&t("task.add_member"), move |g| g.invoke("Add-DistributionGroupMember", params).map(|_| ()), after);
        });
    }

    fn add_mailbox_permission(&mut self, obj: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = identity(&obj);
        let name = s(&obj, "DisplayName");
        self.pick_user(&tf("exchange.give_access", &[("name", &name)]), move |app, u| {
            let who = u.detail.clone();
            let item = |id: &str, label: &str, detail: &str| PickItem { id: id.into(), label: t(label), detail: t(detail) };
            let options = vec![
                item("full", "exchange.full_access", "exchange.full_access_about"),
                item("sendas", "exchange.send_as", "exchange.send_as_about"),
                item("behalf", "exchange.send_on_behalf", "exchange.send_on_behalf_about"),
            ];
            let title = tf("exchange.permission_for", &[("name", &u.label.trim_start_matches(['👤', ' ']))]);
            let fetch = move |_: &Graph, _: &str| Ok(options.clone());
            app.push(Dialog::Picker(Picker::new(&title, &t("exchange.permission"), false, fetch, move |app, p| {
                let (cmdlet, params) = match p.id.as_str() {
                    "full" => (
                        "Add-MailboxPermission",
                        json!({ "Identity": id, "User": who, "AccessRights": ["FullAccess"], "InheritanceType": "All", "AutoMapping": true }),
                    ),
                    "sendas" => (
                        "Add-RecipientPermission",
                        json!({ "Identity": id, "Trustee": who, "AccessRights": ["SendAs"], "Confirm": false }),
                    ),
                    _ => ("Set-Mailbox", json!({ "Identity": id, "GrantSendOnBehalfTo": add_remove("Add", &who) })),
                };
                app.mutate(&tf("task.give", &[("permission", &p.label)]), move |g| g.invoke(cmdlet, params).map(|_| ()), after);
            })));
        });
    }

    /// Add, remove and edit on the property sheet tabs of mailboxes and distribution lists.
    pub fn exchange_sheet_op(&mut self, sheet_id: u64, node: Node, kind: TabKind, op: TabOp, obj: Value, row: Option<Value>) {
        let reload = move |app: &mut App| app.reload_tab(sheet_id, kind);
        let id = identity(&obj);
        let name = s(&obj, "DisplayName");
        match (kind, op) {
            (TabKind::General, TabOp::Edit) if node == Node::Mailboxes => self.edit_mailbox(obj, reload),
            (TabKind::General, TabOp::Edit) => self.edit_distribution_list(obj, reload),
            (TabKind::AutoReplies, TabOp::Edit) => self.edit_auto_replies(obj, reload),
            (TabKind::MailboxAccess, TabOp::Add) => self.add_mailbox_permission(obj, reload),
            (TabKind::ListMembers, TabOp::Add) => self.add_list_member(obj, reload),
            (TabKind::MailboxAccess, TabOp::Remove) => {
                let Some(row) = row else { return };
                let who = s(&row, "user");
                let (cmdlet, params, label) = match s(&row, "kind").as_str() {
                    "full" => (
                        "Remove-MailboxPermission",
                        json!({ "Identity": id, "User": who, "AccessRights": ["FullAccess"], "InheritanceType": "All", "Confirm": false }),
                        "exchange.full_access",
                    ),
                    "sendas" => (
                        "Remove-RecipientPermission",
                        json!({ "Identity": id, "Trustee": who, "AccessRights": ["SendAs"], "Confirm": false }),
                        "exchange.send_as",
                    ),
                    _ => ("Set-Mailbox", json!({ "Identity": id, "GrantSendOnBehalfTo": add_remove("Remove", &who) }), "exchange.send_on_behalf"),
                };
                let text = tf("exchange.remove_permission", &[("permission", &t(label)), ("user", &who), ("name", &name)]);
                self.push(Dialog::confirm(&t("sheet.remove_permission"), &text, move |app| {
                    app.mutate(&t("sheet.remove_permission"), move |g| g.invoke(cmdlet, params).map(|_| ()), reload);
                }));
            }
            (TabKind::ListMembers, TabOp::Remove) => {
                let Some(row) = row else { return };
                let params = json!({ "Identity": id, "Member": identity(&row), "BypassSecurityGroupManagerCheck": true, "Confirm": false });
                let text = tf("sheet.remove_assignment_question", &[("member", &s(&row, "DisplayName")), ("name", &name)]);
                self.push(Dialog::confirm(&t("sheet.remove_member"), &text, move |app| {
                    app.mutate(&t("sheet.remove_member"), move |g| g.invoke("Remove-DistributionGroupMember", params).map(|_| ()), reload);
                }));
            }
            _ => {}
        }
    }
}

/// The display name and primary address from a new mailbox or list form.
fn new_address(f: &Form) -> Result<(String, String), String> {
    let alias = f.text("alias");
    if alias.contains('@') || alias.contains(' ') {
        return Err(t("exchange.bad_alias"));
    }
    let domain = match f.choice("domain") {
        d if !d.is_empty() => d,
        _ => f.text("domain"),
    };
    if domain.is_empty() {
        return Err(tf("form.required", &[("field", &t("field.domain"))]));
    }
    Ok((f.text("name"), format!("{alias}@{domain}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exchange_times() {
        assert_eq!(exo_time(&json!("2026-12-24T17:00:00")), "2026-12-24 17:00");
        assert_eq!(exo_time(&json!("/Date(1798131600000)/")), "2026-12-24 17:00");
        assert_eq!(parse_time("2026-12-24 17:00").as_deref(), Some("2026-12-24T17:00:00Z"));
        assert_eq!(parse_time("24/12/2026 17:00"), None);
    }

    #[test]
    fn lists_smtp_addresses_primary_first() {
        let v = json!({ "EmailAddresses": ["smtp:b@contoso.com", "SIP:a@contoso.com", "SMTP:a@contoso.com"] });
        assert_eq!(addresses(&v), "a@contoso.com, b@contoso.com");
    }

    #[test]
    fn strips_reply_html() {
        assert_eq!(html_text("<html><body><p>Away &amp; back</p><p>Monday</p></body></html>"), "Away & back\nMonday");
        assert_eq!(html_text("Plain text"), "Plain text");
        let owa = "<html><head><style type=\"text/css\" style=\"display:none;\"> P {margin-top:0;} </style></head>\
                   <body><div>Away</div><div>Back Monday</div></body></html>";
        assert_eq!(html_text(owa), "Away\nBack Monday");
    }

    #[test]
    fn typed_replies_keep_their_lines() {
        let html = text_html("Away <until>\nBack Monday");
        assert_eq!(html, "<html><body>Away &lt;until&gt;<br>Back Monday</body></html>");
        assert_eq!(html_text(&html), "Away <until>\nBack Monday");
    }

    #[test]
    fn multi_valued_changes() {
        assert_eq!(add_remove("Add", "ann@contoso.com"), json!({ "@odata.type": "#Exchange.GenericHashTable", "Add": "ann@contoso.com" }));
    }
}
