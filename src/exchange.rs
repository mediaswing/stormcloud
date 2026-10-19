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
            .ok_or_else(|| anyhow!("Could not find the tenant's onmicrosoft.com domain."))?;
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
        "UserMailbox" => "User".into(),
        "SharedMailbox" => "Shared".into(),
        "RoomMailbox" => "Room".into(),
        "EquipmentMailbox" => "Equipment".into(),
        other => other.into(),
    }
}

pub fn group_type(v: &Value) -> String {
    match s(v, "RecipientTypeDetails").as_str() {
        "MailUniversalDistributionGroup" => "Distribution list".into(),
        "MailUniversalSecurityGroup" => "Mail-enabled security".into(),
        "RoomList" => "Room list".into(),
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
fn parse_time(text: &str) -> Option<String> {
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

fn is_system_account(who: &str) -> bool {
    who.starts_with("NT AUTHORITY\\") || who.starts_with("S-1-5-")
}

/// Loads the list for an Exchange node (runs on a worker thread).
pub fn list(g: &Graph, node: Node) -> Result<(Vec<Value>, bool)> {
    let (rows, more) = match node {
        Node::Mailboxes => g.invoke_capped("Get-Mailbox", json!({ "ResultSize": "Unlimited", "RecipientTypeDetails": MAILBOX_TYPES }))?,
        Node::DistributionLists => g.invoke_capped("Get-DistributionGroup", json!({ "ResultSize": "Unlimited" }))?,
        _ => bail!("{} is not an Exchange list", node.title()),
    };
    Ok((rows.into_iter().map(with_id).collect(), more))
}

/// Fetches a property sheet tab for a mailbox or distribution list (runs on a worker thread).
pub fn load_tab(g: &Graph, node: Node, kind: TabKind, obj: &Value) -> Result<(TabBody, Option<Value>)> {
    let id = identity(obj);
    let one = |cmdlet: &str| -> Result<Value> {
        g.invoke(cmdlet, json!({ "Identity": id }))?.into_iter().next().ok_or_else(|| anyhow!("{id} was not found."))
    };
    let field = |label: &str, value: String| (label.to_string(), value);
    let body = match (node, kind) {
        (Node::Mailboxes, TabKind::General) => {
            let m = with_id(one("Get-Mailbox")?);
            let fields = vec![
                field("Display name", s(&m, "DisplayName")),
                field("Email address", s(&m, "PrimarySmtpAddress")),
                field("Type", mailbox_type(&m)),
                field("Email addresses", addresses(&m)),
                field("Alias", s(&m, "Alias")),
                field("Hidden from address lists", yes_no(&m["HiddenFromAddressListsEnabled"])),
                field("Forwarding to", forwarding(&m)),
                field("Keep a copy when forwarding", yes_no(&m["DeliverToMailboxAndForward"])),
                field("Archive", s(&m, "ArchiveStatus")),
                field("Litigation hold", yes_no(&m["LitigationHoldEnabled"])),
                field("Object ID", s(&m, "ExternalDirectoryObjectId")),
            ];
            return Ok((TabBody::Fields(fields), Some(m)));
        }
        (Node::Mailboxes, TabKind::MailboxAccess) => mailbox_permissions(g, &id)?,
        (Node::Mailboxes, TabKind::AutoReplies) => {
            let c = one("Get-MailboxAutoReplyConfiguration")?;
            let state = s(&c, "AutoReplyState");
            let mut fields = vec![field("Automatic replies", match state.as_str() {
                "Enabled" => "On".into(),
                "Scheduled" => "On, between the times below".into(),
                "Disabled" => "Off".into(),
                _ => state.clone(),
            })];
            if state == "Scheduled" {
                fields.push(field("Start", exo_time(&c["StartTime"])));
                fields.push(field("End", exo_time(&c["EndTime"])));
            }
            fields.push(field("Reply inside the organisation", html_text(&s(&c, "InternalMessage"))));
            fields.push(field("Reply outside the organisation", match s(&c, "ExternalAudience").as_str() {
                "None" => "Don't reply".into(),
                "Known" => format!("Only to senders in the mailbox's contacts: {}", html_text(&s(&c, "ExternalMessage"))),
                _ => html_text(&s(&c, "ExternalMessage")),
            }));
            TabBody::Fields(fields)
        }
        (Node::DistributionLists, TabKind::General) => {
            let d = with_id(one("Get-DistributionGroup")?);
            let fields = vec![
                field("Display name", s(&d, "DisplayName")),
                field("Email address", s(&d, "PrimarySmtpAddress")),
                field("Type", group_type(&d)),
                field("Email addresses", addresses(&d)),
                field("Alias", s(&d, "Alias")),
                field("Only people in the organisation can send to it", yes_no(&d["RequireSenderAuthenticationEnabled"])),
                field("Hidden from address lists", yes_no(&d["HiddenFromAddressListsEnabled"])),
                field("Owners", list_text(&d["ManagedBy"])),
                field("Object ID", s(&d, "ExternalDirectoryObjectId")),
            ];
            return Ok((TabBody::Fields(fields), Some(d)));
        }
        (Node::DistributionLists, TabKind::ListMembers) => {
            let items = g.invoke("Get-DistributionGroupMember", json!({ "Identity": id, "ResultSize": "Unlimited" }))?;
            table(vec!["Name", "Email address", "Type"], items, |v| {
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
        rows.push(SheetRow { cells: vec![who, label.into()], data });
    };
    for p in g.invoke("Get-MailboxPermission", json!({ "Identity": id }))? {
        let who = s(&p, "User");
        if p["IsInherited"] != true && p["Deny"] != true && p["AccessRights"].to_string().contains("FullAccess") && !is_system_account(&who) {
            add(who, "full", "Full Access");
        }
    }
    for p in g.invoke("Get-RecipientPermission", json!({ "Identity": id }))? {
        let who = s(&p, "Trustee");
        if p["IsInherited"] != true && s(&p, "AccessControlType") != "Deny" && p["AccessRights"].to_string().contains("SendAs") && !is_system_account(&who) {
            add(who, "sendas", "Send As");
        }
    }
    let mailbox = g.invoke("Get-Mailbox", json!({ "Identity": id }))?;
    for who in mailbox.first().and_then(|m| m["GrantSendOnBehalfTo"].as_array()).map(Vec::as_slice).unwrap_or_default() {
        if let Some(who) = who.as_str() {
            add(who.to_string(), "behalf", "Send on behalf");
        }
    }
    rows.sort_by_cached_key(|r| (r.cells[0].to_lowercase(), r.cells[1].clone()));
    Ok(TabBody::Table { headers: vec!["User", "Permission"], rows })
}

impl App {
    /// Shows the domains of the tenant as a choice, or a text box before they have loaded.
    fn domain_field(&self) -> Field {
        let domains: Vec<(String, String)> = self.domains.iter().map(|d| (d.clone(), format!("@{d}"))).collect();
        if domains.is_empty() {
            Field::text("domain", "Domain", "")
        } else {
            Field::choice("domain", "Domain", domains, 0)
        }
    }

    pub fn new_shared_mailbox(&mut self) {
        let form = Form::new("New shared mailbox", "Create")
            .intro("A shared mailbox doesn't need a licence while it is under 50 GB. Give people access to it on its Mailbox permissions tab.")
            .field(Field::text("name", "Display name", "").required())
            .field(Field::text("alias", "Email name", "").required())
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
                app.spawn("Creating shared mailbox…", move |g| g.invoke("New-Mailbox", params), move |app, r| match r {
                    Ok(made) => {
                        app.status = format!("Created shared mailbox {address}");
                        if let Some(m) = made.into_iter().next() {
                            app.upsert(Node::Mailboxes, with_id(m));
                        } else {
                            app.load(Node::Mailboxes);
                        }
                    }
                    Err(e) => app.error("New shared mailbox", e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn new_distribution_list(&mut self) {
        let types = vec![
            ("Distribution".to_string(), "Distribution list".to_string()),
            ("Security".to_string(), "Mail-enabled security group".to_string()),
        ];
        let form = Form::new("New distribution list", "Create")
            .field(Field::choice("type", "Type", types, 0))
            .field(Field::text("name", "Display name", "").required())
            .field(Field::text("alias", "Email name", "").required())
            .field(self.domain_field())
            .field(Field::check("internal", "Only people in the organisation can send to it", true))
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
                app.spawn("Creating distribution list…", move |g| g.invoke("New-DistributionGroup", params), move |app, r| match r {
                    Ok(made) => {
                        app.status = format!("Created distribution list {address}");
                        if let Some(d) = made.into_iter().next() {
                            app.upsert(Node::DistributionLists, with_id(d));
                        } else {
                            app.load(Node::DistributionLists);
                        }
                    }
                    Err(e) => app.error("New distribution list", e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    /// Display name, address-list visibility and forwarding of a mailbox.
    pub fn edit_mailbox(&mut self, obj: Value, after: impl Fn(&mut App) + Clone + Send + 'static) {
        let current = forwarding(&obj);
        let form = Form::new(&format!("Edit {}", s(&obj, "DisplayName")), "Save")
            .intro(
                "Leave \"Forward email to\" blank to stop forwarding. Forwarding to an address outside the \
                 organisation only works if the outbound spam policy allows it.",
            )
            .field(Field::text("DisplayName", "Display name", &s(&obj, "DisplayName")).required())
            .field(Field::check("HiddenFromAddressListsEnabled", "Hide from address lists", obj["HiddenFromAddressListsEnabled"] == true))
            .field(Field::text("forward", "Forward email to", &current))
            .field(Field::check("DeliverToMailboxAndForward", "Keep a copy in the mailbox when forwarding", obj["DeliverToMailboxAndForward"] == true))
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
                        return Err("Enter the email address to forward to, such as ann@contoso.com.".into());
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
                app.mutate("Save mailbox", move |g| g.invoke("Set-Mailbox", params).map(|_| ()), move |app| {
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
            "Loading automatic replies…",
            move |g| g.invoke("Get-MailboxAutoReplyConfiguration", json!({ "Identity": get })),
            move |app, r| match r {
                Ok(c) => app.auto_replies_form(id, &name, c.into_iter().next().unwrap_or_default(), after),
                Err(e) => app.error("Automatic replies", e),
            },
        );
    }

    fn auto_replies_form(&mut self, id: String, name: &str, c: Value, after: impl Fn(&mut App) + Clone + Send + 'static) {
        let opts = |list: &[(&str, &str)]| list.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>();
        let states = opts(&[("Disabled", "Off"), ("Enabled", "On"), ("Scheduled", "On, between these times")]);
        let audiences = opts(&[
            ("All", "Reply to everyone"),
            ("Known", "Reply only to senders in the mailbox's contacts"),
            ("None", "Don't reply"),
        ]);
        let pos = |list: &[(String, String)], v: String| list.iter().position(|o| o.0 == v).unwrap_or(0);
        let state = pos(&states, s(&c, "AutoReplyState"));
        let audience = pos(&audiences, s(&c, "ExternalAudience"));
        let internal = html_text(&s(&c, "InternalMessage"));
        let external = html_text(&s(&c, "ExternalMessage"));
        let form = Form::new(&format!("Automatic replies — {name}"), "Save")
            .intro("Times are in UTC, written as 2026-12-24 17:00.")
            .field(Field::choice("state", "Automatic replies", states, state))
            .field(Field::text("start", "Start", &exo_time(&c["StartTime"])))
            .field(Field::text("end", "End", &exo_time(&c["EndTime"])))
            .field(Field::multiline("internal", "Reply inside the organisation", &internal))
            .field(Field::choice("audience", "Outside the organisation", audiences, audience))
            .field(Field::multiline("external", "Reply outside the organisation", &external))
            .on_submit(move |app, f| {
                let mut params = json!({
                    "Identity": id,
                    "AutoReplyState": f.choice("state"),
                    "ExternalAudience": f.choice("audience"),
                });
                // Unchanged messages are left alone, keeping any formatting they have.
                if f.raw("internal").trim() != internal {
                    params["InternalMessage"] = json!(f.raw("internal"));
                }
                if f.raw("external").trim() != external {
                    params["ExternalMessage"] = json!(f.raw("external"));
                }
                if f.choice("state") == "Scheduled" {
                    let (Some(start), Some(end)) = (parse_time(&f.text("start")), parse_time(&f.text("end"))) else {
                        return Err("Enter the start and end as a date and time, such as 2026-12-24 17:00.".into());
                    };
                    if end <= start {
                        return Err("The end must be after the start.".into());
                    }
                    params["StartTime"] = json!(start);
                    params["EndTime"] = json!(end);
                }
                app.mutate("Save automatic replies", move |g| g.invoke("Set-MailboxAutoReplyConfiguration", params).map(|_| ()), after.clone());
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn convert_mailbox(&mut self, obj: Value) {
        let kind = s(&obj, "RecipientTypeDetails");
        let name = s(&obj, "DisplayName");
        let (to, text) = match kind.as_str() {
            "UserMailbox" => (
                "Shared",
                format!(
                    "Convert {name} to a shared mailbox? Its account can no longer sign in, and people with \
                     Full Access open it from their own mailbox. Under 50 GB it doesn't need a licence, so \
                     you can remove the user's licence afterwards."
                ),
            ),
            "SharedMailbox" => (
                "Regular",
                format!(
                    "Convert {name} to a user mailbox? Its user needs a licence that includes Exchange Online \
                     within 30 days, and a password and an enabled account to sign in."
                ),
            ),
            _ => {
                self.push(Dialog::message("Convert mailbox", "Only user and shared mailboxes can be converted."));
                return;
            }
        };
        let id = identity(&obj);
        let oid = obj["id"].clone();
        let new_kind = if to == "Shared" { "SharedMailbox" } else { "UserMailbox" };
        self.push(Dialog::confirm("Convert mailbox", &text, move |app| {
            app.mutate(
                &format!("Convert {name}"),
                move |g| g.invoke("Set-Mailbox", json!({ "Identity": id, "Type": to })).map(|_| ()),
                move |app| app.upsert(Node::Mailboxes, json!({ "id": oid, "RecipientTypeDetails": new_kind })),
            );
        }));
    }

    pub fn delete_exchange(&mut self, node: Node, obj: Value) {
        let name = s(&obj, "DisplayName");
        let (cmdlet, params, text) = match node {
            Node::Mailboxes if s(&obj, "RecipientTypeDetails") == "UserMailbox" => {
                self.push(Dialog::message(
                    "Delete mailbox",
                    "A user's mailbox goes with the user: delete the user under Users, or remove their \
                     Exchange Online licence. To keep the email, convert it to a shared mailbox first.",
                ));
                return;
            }
            Node::Mailboxes => (
                "Remove-Mailbox",
                json!({ "Identity": identity(&obj), "Confirm": false }),
                format!("Delete the {} mailbox '{name}' and its email? It can be restored for 30 days.", mailbox_type(&obj).to_lowercase()),
            ),
            Node::DistributionLists => (
                "Remove-DistributionGroup",
                json!({ "Identity": identity(&obj), "BypassSecurityGroupManagerCheck": true, "Confirm": false }),
                format!("Delete the distribution list '{name}'? This cannot be undone."),
            ),
            _ => return,
        };
        let id = s(&obj, "id");
        self.push(Dialog::confirm(&format!("Delete {}", node.noun()), &text, move |app| {
            app.mutate(&format!("Delete {name}"), move |g| g.invoke(cmdlet, params).map(|_| ()), move |app| app.remove_obj(node, &id));
        }));
    }

    pub fn edit_distribution_list(&mut self, obj: Value, after: impl Fn(&mut App) + Clone + Send + 'static) {
        let form = Form::new(&format!("Edit {}", s(&obj, "DisplayName")), "Save")
            .field(Field::text("DisplayName", "Display name", &s(&obj, "DisplayName")).required())
            .field(Field::check(
                "RequireSenderAuthenticationEnabled",
                "Only people in the organisation can send to it",
                obj["RequireSenderAuthenticationEnabled"] == true,
            ))
            .field(Field::check("HiddenFromAddressListsEnabled", "Hide from address lists", obj["HiddenFromAddressListsEnabled"] == true))
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
                app.mutate("Save distribution list", move |g| g.invoke("Set-DistributionGroup", params).map(|_| ()), move |app| {
                    app.upsert(Node::DistributionLists, changes);
                    after(app);
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn add_list_member(&mut self, obj: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = identity(&obj);
        self.pick_user_or_group(&format!("Add member to {}", s(&obj, "DisplayName")), move |app, m| {
            // The user principal name or group email address.
            let member = m.detail.clone();
            if member.is_empty() {
                let who = m.label.trim_start_matches(['👤', '👥', ' ']);
                app.push(Dialog::message("Add member", &format!("{who} has no email address, so it can't be added to a distribution list.")));
                return;
            }
            let params = json!({ "Identity": id, "Member": member, "BypassSecurityGroupManagerCheck": true });
            app.mutate("Add member", move |g| g.invoke("Add-DistributionGroupMember", params).map(|_| ()), after);
        });
    }

    fn add_mailbox_permission(&mut self, obj: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = identity(&obj);
        let name = s(&obj, "DisplayName");
        self.pick_user(&format!("Give a user access to {name}"), move |app, u| {
            let who = u.detail.clone();
            let item = |id: &str, label: &str, detail: &str| PickItem { id: id.into(), label: label.into(), detail: detail.into() };
            let options = vec![
                item("full", "Full Access", "Open the mailbox and read, send and delete its email"),
                item("sendas", "Send As", "Send email that appears to come from the mailbox"),
                item("behalf", "Send on behalf", "Send email shown as sent on behalf of the mailbox"),
            ];
            let title = format!("Permission for {}", u.label.trim_start_matches(['👤', ' ']));
            let fetch = move |_: &Graph, _: &str| Ok(options.clone());
            app.push(Dialog::Picker(Picker::new(&title, "Permission:", false, fetch, move |app, p| {
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
                app.mutate(&format!("Give {}", p.label), move |g| g.invoke(cmdlet, params).map(|_| ()), after);
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
                        "Full Access",
                    ),
                    "sendas" => (
                        "Remove-RecipientPermission",
                        json!({ "Identity": id, "Trustee": who, "AccessRights": ["SendAs"], "Confirm": false }),
                        "Send As",
                    ),
                    _ => ("Set-Mailbox", json!({ "Identity": id, "GrantSendOnBehalfTo": add_remove("Remove", &who) }), "Send on behalf"),
                };
                let text = format!("Remove {who}'s {label} permission on {name}?");
                self.push(Dialog::confirm("Remove permission", &text, move |app| {
                    app.mutate("Remove permission", move |g| g.invoke(cmdlet, params).map(|_| ()), reload);
                }));
            }
            (TabKind::ListMembers, TabOp::Remove) => {
                let Some(row) = row else { return };
                let params = json!({ "Identity": id, "Member": identity(&row), "BypassSecurityGroupManagerCheck": true, "Confirm": false });
                let text = format!("Remove '{}' from {name}?", s(&row, "DisplayName"));
                self.push(Dialog::confirm("Remove member", &text, move |app| {
                    app.mutate("Remove member", move |g| g.invoke("Remove-DistributionGroupMember", params).map(|_| ()), reload);
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
        return Err("Email name must not contain spaces or @; choose the domain separately.".into());
    }
    let domain = match f.choice("domain") {
        d if !d.is_empty() => d,
        _ => f.text("domain"),
    };
    if domain.is_empty() {
        return Err("Domain is required.".into());
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
    }

    #[test]
    fn multi_valued_changes() {
        assert_eq!(add_remove("Add", "ann@contoso.com"), json!({ "@odata.type": "#Exchange.GenericHashTable", "Add": "ann@contoso.com" }));
    }
}
