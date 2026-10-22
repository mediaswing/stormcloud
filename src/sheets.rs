//! Tabbed property sheets for users, groups, devices, applications, Exchange recipients, licences
//! and remote servers.

use crate::app::App;
use crate::dialogs::{Dialog, Field, Form};
use crate::graph::Graph;
use crate::i18n::{t, tf};
use crate::model::{self, general_fields, s, sku_name, Node};
use crate::ops::{collection, DEFAULT_ACCESS_ROLE};
use crate::app::Bold;
use crate::auth::NeedsScopes;
use eframe::egui::{self, Align, Key, Layout, Modifiers, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    General,
    MemberOf,
    UserLicences,
    Roles,
    Members,
    Owners,
    GroupLicences,
    RegisteredOwners,
    RegisteredUsers,
    ServicePlans,
    LicensedUsers,
    AppUsers,
    Permissions,
    MailboxAccess,
    AutoReplies,
    ListMembers,
    Security,
    AuthMethods,
    SignIns,
    Software,
    Services,
    Updates,
}

impl TabKind {
    pub fn title(self) -> String {
        t(match self {
            TabKind::General => "tab.general",
            TabKind::MemberOf => "tab.member_of",
            TabKind::UserLicences | TabKind::GroupLicences => "tab.licences",
            TabKind::Roles => "tab.roles",
            TabKind::Members | TabKind::ListMembers => "tab.members",
            TabKind::Owners => "tab.owners",
            TabKind::RegisteredOwners => "tab.registered_owners",
            TabKind::RegisteredUsers => "tab.registered_users",
            TabKind::ServicePlans => "tab.service_plans",
            TabKind::LicensedUsers => "tab.licensed_users",
            TabKind::AppUsers => "tab.app_users",
            TabKind::Permissions => "tab.permissions",
            TabKind::MailboxAccess => "tab.mailbox_access",
            TabKind::AutoReplies => "tab.auto_replies",
            TabKind::Security => "tab.security",
            TabKind::AuthMethods => "tab.auth_methods",
            TabKind::SignIns => "tab.sign_ins",
            TabKind::Software => "tab.installed_apps",
            TabKind::Services => "tab.running_services",
            TabKind::Updates => "tab.updates",
        })
    }
}

fn tabs_for(node: Node) -> Vec<TabKind> {
    use TabKind::*;
    match node {
        Node::Users => vec![General, MemberOf, UserLicences, Roles, Security, AuthMethods, SignIns],
        Node::Groups => vec![General, Members, Owners, Roles, GroupLicences],
        Node::Devices => vec![General, RegisteredOwners, RegisteredUsers, MemberOf],
        Node::Applications => vec![General, AppUsers, Owners, Permissions],
        Node::Licences => vec![General, ServicePlans, LicensedUsers],
        Node::Mailboxes => vec![General, MailboxAccess, AutoReplies],
        Node::DistributionLists => vec![General, ListMembers],
        Node::RemoteServers => vec![General, Software, Services, Updates],
        Node::Root | Node::Exchange => vec![General],
    }
}

type Work = Box<dyn FnOnce(&Graph) -> anyhow::Result<()> + Send>;

pub enum TabBody {
    Loading,
    Error(String),
    Fields(Vec<(String, String)>),
    /// The keys of the column titles in the catalogue, and the rows.
    Table { headers: Vec<&'static str>, rows: Vec<SheetRow> },
}

pub struct SheetRow {
    pub cells: Vec<String>,
    pub data: Value,
}

pub struct Tab {
    pub kind: TabKind,
    pub body: TabBody,
    requested: bool,
    sel: Option<usize>,
}

pub struct Sheet {
    pub node: Node,
    pub obj: Value,
    pub tabs: Vec<Tab>,
    pub active: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TabOp {
    Add,
    Remove,
    Edit,
    Reload,
    /// One of a tab's further buttons, by position (see `security::extra_ops`).
    Extra(usize),
}

impl Sheet {
    pub fn new(node: Node, obj: Value) -> Sheet {
        let tabs = tabs_for(node)
            .into_iter()
            .map(|kind| Tab { kind, body: TabBody::Loading, requested: false, sel: None })
            .collect();
        Sheet { node, obj, tabs, active: 0 }
    }

    /// Shows `obj` in place of the object the sheet was opened on, reloading every tab.
    pub fn replace_obj(&mut self, obj: Value) {
        self.obj = obj;
        for tab in &mut self.tabs {
            tab.requested = false;
        }
    }

    /// Opens the sheet on the tab of `kind`, if it has one.
    pub fn on_tab(mut self, kind: TabKind) -> Sheet {
        if let Some(i) = self.tabs.iter().position(|t| t.kind == kind) {
            self.active = i;
        }
        self
    }

    fn name(&self) -> String {
        self.node.object_name(&self.obj)
    }

    /// The keys of the labels for the add / remove / edit operations available on a tab.
    fn op_labels(&self, kind: TabKind) -> [Option<&'static str>; 3] {
        use TabKind::*;
        let editable = matches!(self.node, Node::Users | Node::Groups | Node::Applications | Node::Mailboxes | Node::DistributionLists | Node::RemoteServers);
        match (self.node, kind) {
            (Node::Users, Security | AuthMethods | SignIns) => crate::security::op_labels(kind),
            (_, General) if editable => [None, None, Some("sheet.edit")],
            (Node::Users, MemberOf) => [Some("act.add_to_group"), Some("sheet.remove_from_group"), None],
            (Node::Users, UserLicences) | (Node::Groups, GroupLicences) => [Some("act.assign_licence"), Some("task.remove_licence"), None],
            (_, Roles) => [Some("sheet.assign_role"), Some("sheet.remove_assignment"), None],
            (_, Members) => [Some("act.add_member"), Some("sheet.remove_member"), None],
            (Node::Groups | Node::Applications, Owners) => [Some("sheet.add_owner"), Some("sheet.remove_owner"), None],
            (_, AppUsers) => [Some("sheet.assign_user_or_group"), Some("sheet.remove_assignment"), None],
            (_, Permissions) => [None, Some("sheet.revoke_permission"), None],
            (_, MailboxAccess) => [Some("sheet.add_permission"), Some("sheet.remove_permission"), None],
            (_, AutoReplies) => [None, None, Some("sheet.edit")],
            (_, ListMembers) => [Some("act.add_member"), Some("sheet.remove_member"), None],
            (_, LicensedUsers) => [Some("act.assign_to_user"), Some("sheet.remove_from_user"), None],
            _ => [None, None, None],
        }
    }
}

pub fn table(headers: Vec<&'static str>, items: Vec<Value>, cells: impl Fn(&Value) -> Vec<String>) -> TabBody {
    let rows = items.into_iter().map(|v| SheetRow { cells: cells(&v), data: v }).collect();
    TabBody::Table { headers, rows }
}

fn odata_kind(v: &Value) -> String {
    let kind = v["@odata.type"].as_str().unwrap_or("").trim_start_matches("#microsoft.graph.");
    match kind {
        "user" => t("value.user"),
        "group" => t("value.group"),
        "device" => t("value.device"),
        "directoryRole" => t("value.directory_role"),
        "orgContact" => t("value.contact"),
        "servicePrincipal" => t("value.application"),
        other => other.to_string(),
    }
}

fn sku_names(g: &Graph) -> anyhow::Result<std::collections::HashMap<String, String>> {
    Ok(g.list("/subscribedSkus")?
        .iter()
        .map(|v| (s(v, "skuId"), sku_name(&s(v, "skuPartNumber"))))
        .collect())
}

/// What loading a tab found: its body, for General tabs the freshly read object, and any
/// Graph scopes part of it couldn't be read without.
type Loaded = (TabBody, Option<Value>, Vec<String>);

/// Fetches the content of a tab (runs on a worker thread).
fn load_tab(g: &Graph, node: Node, kind: TabKind, obj: &Value) -> anyhow::Result<Loaded> {
    use TabKind::*;
    if matches!(node, Node::Mailboxes | Node::DistributionLists) {
        return crate::exchange::load_tab(g, node, kind, obj).map(|(body, fresh)| (body, fresh, Vec::new()));
    }
    if matches!(kind, Security | AuthMethods | SignIns) {
        return crate::security::load_tab(g, kind, obj).map(|(body, missing)| (body, None, missing));
    }
    let id = s(obj, "id");
    let coll = collection(node);
    let principal_cols = |v: &Value| vec![s(v, "displayName"), odata_kind(v), {
        let upn = s(v, "userPrincipalName");
        if upn.is_empty() { s(v, "mail") } else { upn }
    }];
    let body = match (node, kind) {
        (Node::Licences, General) => {
            let total = obj["prepaidUnits"]["enabled"].as_i64().unwrap_or(0);
            let used = obj["consumedUnits"].as_i64().unwrap_or(0);
            TabBody::Fields(vec![
                (t("prop.product"), sku_name(&s(obj, "skuPartNumber"))),
                (t("prop.sku_part"), s(obj, "skuPartNumber")),
                (t("prop.sku_id"), s(obj, "skuId")),
                (t("prop.status"), s(obj, "capabilityStatus")),
                (t("prop.applies_to"), s(obj, "appliesTo")),
                (t("prop.assigned"), used.to_string()),
                (t("prop.total_enabled"), total.to_string()),
                (t("prop.available"), (total - used).to_string()),
                (t("prop.suspended"), obj["prepaidUnits"]["suspended"].to_string()),
                (t("prop.in_warning"), obj["prepaidUnits"]["warning"].to_string()),
            ])
        }
        (Node::Root | Node::Exchange, _) => TabBody::Fields(vec![(t("prop.name"), s(obj, "name"))]),
        (_, General) => {
            let (select, keys): (&str, &[(&str, &str)]) = match node {
                Node::Users => (
                    "id,displayName,givenName,surname,userPrincipalName,mail,jobTitle,department,companyName,officeLocation,mobilePhone,businessPhones,usageLocation,accountEnabled,userType,createdDateTime,lastPasswordChangeDateTime,onPremisesSyncEnabled,employeeId",
                    &[("prop.display_name", "displayName"), ("prop.first_name", "givenName"), ("prop.last_name", "surname"), ("prop.upn", "userPrincipalName"), ("prop.email", "mail"), ("prop.job_title", "jobTitle"), ("prop.department", "department"), ("prop.company", "companyName"), ("prop.office", "officeLocation"), ("prop.mobile_phone", "mobilePhone"), ("prop.business_phones", "businessPhones"), ("prop.usage_location", "usageLocation"), ("prop.employee_id", "employeeId"), ("prop.account_enabled", "accountEnabled"), ("prop.user_type", "userType"), ("prop.synced", "onPremisesSyncEnabled"), ("prop.created", "createdDateTime"), ("prop.password_changed", "lastPasswordChangeDateTime"), ("prop.object_id", "id")],
                ),
                Node::Applications => (
                    "id,appId,displayName,publisherName,accountEnabled,appRoleAssignmentRequired,tags,appRoles,homepage,loginUrl,replyUrls,notes,signInAudience,preferredSingleSignOnMode",
                    &[("prop.name", "displayName"), ("prop.publisher", "publisherName"), ("prop.sign_in_enabled", "accountEnabled"), ("prop.assignment_required", "appRoleAssignmentRequired"), ("prop.homepage", "homepage"), ("prop.sign_on_url", "loginUrl"), ("prop.reply_urls", "replyUrls"), ("prop.sso_mode", "preferredSingleSignOnMode"), ("prop.audience", "signInAudience"), ("prop.notes", "notes"), ("prop.application_id", "appId"), ("prop.object_id", "id")],
                ),
                Node::Groups => (
                    "id,displayName,description,mail,mailNickname,mailEnabled,securityEnabled,groupTypes,isAssignableToRole,membershipRule,membershipRuleProcessingState,visibility,createdDateTime,onPremisesSyncEnabled,assignedLicenses",
                    &[("prop.name", "displayName"), ("prop.description", "description"), ("prop.email", "mail"), ("prop.mail_nickname", "mailNickname"), ("prop.security_enabled", "securityEnabled"), ("prop.mail_enabled", "mailEnabled"), ("prop.group_types", "groupTypes"), ("prop.role_assignable", "isAssignableToRole"), ("prop.membership_rule", "membershipRule"), ("prop.rule_processing", "membershipRuleProcessingState"), ("prop.visibility", "visibility"), ("prop.synced", "onPremisesSyncEnabled"), ("prop.created", "createdDateTime"), ("prop.object_id", "id")],
                ),
                _ => (
                    "id,deviceId,displayName,operatingSystem,operatingSystemVersion,trustType,accountEnabled,isCompliant,isManaged,manufacturer,model,approximateLastSignInDateTime,registrationDateTime,profileType,onPremisesSyncEnabled",
                    &[("prop.name", "displayName"), ("prop.os", "operatingSystem"), ("prop.os_version", "operatingSystemVersion"), ("prop.join_type", "trustType"), ("prop.manufacturer", "manufacturer"), ("prop.model", "model"), ("prop.enabled", "accountEnabled"), ("prop.compliant", "isCompliant"), ("prop.managed", "isManaged"), ("prop.profile_type", "profileType"), ("prop.synced", "onPremisesSyncEnabled"), ("prop.registered", "registrationDateTime"), ("prop.last_sign_in", "approximateLastSignInDateTime"), ("prop.device_id", "deviceId"), ("prop.object_id", "id")],
                ),
            };
            let fresh = g.get(&format!("/{coll}/{id}?$select={select}"))?;
            let mut fields = general_fields(&fresh, keys);
            if node == Node::Devices {
                fields[3].1 = model::join_type(&fields[3].1);
            }
            if node == Node::Applications {
                fields.insert(4, (t("prop.visible_to_users"), model::yes_no(&model::visible_to_users(&fresh))));
            }
            return Ok((TabBody::Fields(fields), Some(fresh), Vec::new()));
        }
        (_, MemberOf) => {
            let items = g.list(&format!("/{coll}/{id}/memberOf?$select=id,displayName,mail,description&$top=999"))?;
            table(vec!["col.name", "col.type", "col.email"], items, |v| vec![s(v, "displayName"), odata_kind(v), s(v, "mail")])
        }
        (Node::Users, UserLicences) => {
            let items = g.list(&format!("/users/{id}/licenseDetails"))?;
            table(vec!["col.product", "col.sku", "col.service_plans"], items, |v| {
                vec![sku_name(&s(v, "skuPartNumber")), s(v, "skuPartNumber"), model::count(&v["servicePlans"])]
            })
        }
        (_, GroupLicences) => {
            let fresh = g.get(&format!("/groups/{id}?$select=assignedLicenses"))?;
            let names = sku_names(g)?;
            let items = fresh["assignedLicenses"].as_array().cloned().unwrap_or_default();
            table(vec!["col.product", "col.sku_id", "col.disabled_plans"], items, |v| {
                let sku = s(v, "skuId");
                vec![names.get(&sku).cloned().unwrap_or_else(|| sku.clone()), sku, model::count(&v["disabledPlans"])]
            })
        }
        (_, Roles) => {
            let items = g.list(&format!(
                "/roleManagement/directory/roleAssignments?$filter=principalId eq '{id}'&$expand=roleDefinition($select=displayName,description)"
            ))?;
            table(vec!["col.role", "col.scope", "col.description"], items, |v| {
                vec![s(&v["roleDefinition"], "displayName"), s(v, "directoryScopeId"), s(&v["roleDefinition"], "description")]
            })
        }
        (_, Members) => {
            let items = g.list(&format!("/groups/{id}/members?$select=id,displayName,userPrincipalName,mail&$top=999"))?;
            table(vec!["col.name", "col.type", "col.upn_or_email"], items, principal_cols)
        }
        (_, Owners) => {
            let items = g.list(&format!("/{coll}/{id}/owners?$select=id,displayName,userPrincipalName,mail"))?;
            table(vec!["col.name", "col.type", "col.upn_or_email"], items, principal_cols)
        }
        (_, RegisteredOwners) | (_, RegisteredUsers) => {
            let rel = if kind == RegisteredOwners { "registeredOwners" } else { "registeredUsers" };
            let items = g.list(&format!("/devices/{id}/{rel}?$select=id,displayName,userPrincipalName,mail"))?;
            table(vec!["col.name", "col.type", "col.upn_or_email"], items, principal_cols)
        }
        (_, ServicePlans) => {
            let items = obj["servicePlans"].as_array().cloned().unwrap_or_default();
            table(vec!["col.service_plan", "col.applies_to", "col.status"], items, |v| {
                vec![s(v, "servicePlanName"), s(v, "appliesTo"), s(v, "provisioningStatus")]
            })
        }
        (_, LicensedUsers) => {
            let sku = s(obj, "skuId");
            let items = g.list_advanced(&format!(
                "/users?$filter=assignedLicenses/any(x:x/skuId eq {sku})&$select=id,displayName,userPrincipalName,accountEnabled&$count=true&$top=999"
            ))?;
            table(vec!["col.name", "col.upn", "col.enabled"], items, |v| {
                vec![s(v, "displayName"), s(v, "userPrincipalName"), model::yes_no(&v["accountEnabled"])]
            })
        }
        (_, AppUsers) => {
            let roles: std::collections::HashMap<String, String> = obj["appRoles"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|r| (s(r, "id"), s(r, "displayName")))
                .collect();
            let items = g.list(&format!("/servicePrincipals/{id}/appRoleAssignedTo?$top=999"))?;
            table(vec!["col.name", "col.type", "col.role"], items, |v| {
                let role = s(v, "appRoleId");
                let role = match roles.get(&role) {
                    Some(name) => name.clone(),
                    None if role == DEFAULT_ACCESS_ROLE => t("value.default_access"),
                    None => role,
                };
                vec![s(v, "principalDisplayName"), s(v, "principalType"), role]
            })
        }
        (_, Permissions) => load_permissions(g, &id)?,
        _ => TabBody::Fields(Vec::new()),
    };
    Ok((body, None, Vec::new()))
}

/// The application and delegated permissions granted to a service principal. Each row's data
/// gets `kind` ("application" or "delegated") and the shown cells, for revoking it.
fn load_permissions(g: &Graph, id: &str) -> anyhow::Result<TabBody> {
    let mut cache: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    // A lookup that fails (for example, an API since deleted) shows the ID instead.
    let mut lookup = |path: String| cache.entry(path.clone()).or_insert_with(|| g.get(&path).unwrap_or(Value::Null)).clone();
    let mut rows = Vec::new();
    for mut a in g.list(&format!("/servicePrincipals/{id}/appRoleAssignments"))? {
        let api = lookup(format!("/servicePrincipals/{}?$select=displayName,appRoles", s(&a, "resourceId")));
        let role = s(&a, "appRoleId");
        let perm = api["appRoles"]
            .as_array()
            .and_then(|roles| roles.iter().find(|r| r["id"] == role.as_str()))
            .map(|r| s(r, "value"))
            .unwrap_or(role);
        let cells = vec![s(&a, "resourceDisplayName"), perm, t("value.application"), t("value.admin_consent")];
        a["kind"] = json!("application");
        rows.push(SheetRow { cells, data: a });
    }
    let grants = g.list(&format!("/oauth2PermissionGrants?$filter=clientId eq '{id}'"))?;
    // Names of the users who consented for themselves, fetched together rather than one by one.
    let mut user_ids: Vec<String> = grants.iter().filter(|g| g["consentType"] != "AllPrincipals").map(|g| s(g, "principalId")).collect();
    user_ids.sort();
    user_ids.dedup();
    let mut user_names = std::collections::HashMap::new();
    for chunk in user_ids.chunks(1000) {
        let found = g.post("/directoryObjects/getByIds", &json!({ "ids": chunk, "types": ["user"] }))?;
        for u in found["value"].as_array().map(Vec::as_slice).unwrap_or_default() {
            user_names.insert(s(u, "id"), s(u, "displayName"));
        }
    }
    for mut grant in grants {
        let resource = s(&grant, "resourceId");
        let api = lookup(format!("/servicePrincipals/{resource}?$select=displayName,appRoles"));
        let api = if api.is_null() { resource } else { s(&api, "displayName") };
        let scopes = s(&grant, "scope").split_whitespace().collect::<Vec<_>>().join(", ");
        let granted_for = if grant["consentType"] == "AllPrincipals" {
            t("value.all_users_admin_consent")
        } else {
            // A user since deleted shows as their ID.
            let user = s(&grant, "principalId");
            user_names.get(&user).cloned().unwrap_or(user)
        };
        let cells = vec![api, scopes, t("value.delegated"), granted_for];
        grant["kind"] = json!("delegated");
        rows.push(SheetRow { cells, data: grant });
    }
    rows.sort_by_cached_key(|r| (r.cells[0].to_lowercase(), r.cells[2].clone(), r.cells[1].to_lowercase()));
    Ok(TabBody::Table { headers: vec!["col.api", "col.permissions", "col.type", "col.granted_for"], rows })
}

/// The right-click menu of a properties tab: the same operations as its buttons, with their keys.
fn tab_menu(ui: &mut egui::Ui, labels: [Option<&'static str>; 3], extras: &[&str], has_row: bool) -> Option<TabOp> {
    let item = |ui: &mut egui::Ui, label: &str, key: &str, enabled: bool| {
        ui.add_enabled(enabled, egui::Button::new(t(label)).shortcut_text(key).frame(false)).clicked()
    };
    let mut op = None;
    if let Some(l) = labels[0]
        && item(ui, l, "Insert", true) {
            op = Some(TabOp::Add);
        }
    if let Some(l) = labels[1]
        && item(ui, l, "Delete", has_row) {
            op = Some(TabOp::Remove);
        }
    if let Some(l) = labels[2]
        && item(ui, l, "F2", true) {
            op = Some(TabOp::Edit);
        }
    for (i, l) in extras.iter().enumerate() {
        if item(ui, l, "", true) {
            op = Some(TabOp::Extra(i));
        }
    }
    if labels.iter().any(Option::is_some) || !extras.is_empty() {
        ui.separator();
    }
    if item(ui, "act.refresh", "F5", true) {
        op = Some(TabOp::Reload);
    }
    op
}

impl App {
    fn sheet_mut(&mut self, id: u64) -> Option<&mut Sheet> {
        match self.dialog_mut(id) {
            Some(Dialog::Sheet(s)) => Some(s),
            _ => None,
        }
    }

    /// Starts loading a tab; the result is applied when the sheet is back on the dialog stack.
    fn start_tab_load(&mut self, sheet_id: u64, node: Node, kind: TabKind, obj: Value) {
        self.spawn(
            tf("status.loading_tab", &[("tab", &kind.title())]),
            move |g| load_tab(g, node, kind, &obj),
            move |app, r| {
                let mut fresh_obj = None;
                let mut missing = Vec::new();
                if let Some(NeedsScopes(m)) = r.as_ref().err().and_then(|e| e.downcast_ref::<NeedsScopes>()) {
                    missing = m.clone();
                }
                if let Some(sheet) = app.sheet_mut(sheet_id)
                    && let Some(tab) = sheet.tabs.iter_mut().find(|t| t.kind == kind) {
                        match r {
                            Ok((body, fresh, partly)) => {
                                missing = partly;
                                tab.body = body;
                                if let Some(f) = fresh {
                                    if let (Some(dst), Some(src)) = (sheet.obj.as_object_mut(), f.as_object()) {
                                        for (k, v) in src {
                                            dst.insert(k.clone(), v.clone());
                                        }
                                    }
                                    fresh_obj = Some(f);
                                }
                            }
                            Err(e) => tab.body = TabBody::Error(format!("{e:#}")),
                        }
                        let len = match &tab.body {
                            TabBody::Table { rows, .. } => rows.len(),
                            _ => 0,
                        };
                        tab.sel = if len == 0 { None } else { Some(tab.sel.unwrap_or(0).min(len - 1)) };
                    }
                if let Some(f) = fresh_obj {
                    app.upsert(node, f);
                }
                if !missing.is_empty() {
                    app.offer_scopes(&kind.title(), missing);
                }
            },
        );
    }

    /// Marks a tab for reloading (applied on the next frame).
    pub fn reload_tab(&mut self, sheet_id: u64, kind: TabKind) {
        if let Some(sheet) = self.sheet_mut(sheet_id)
            && let Some(tab) = sheet.tabs.iter_mut().find(|t| t.kind == kind) {
                tab.requested = false;
            }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show_sheet(&mut self, ctx: &egui::Context, modal: egui::Modal, sheet_id: u64, sheet: &mut Sheet, first: bool, top: bool, escape: bool) -> bool {
        let mut close = escape;
        let mut op: Option<TabOp> = None;
        let typing = ctx.egui_wants_keyboard_input();
        if top && !typing {
            ctx.input_mut(|i| {
                let n = sheet.tabs.len();
                if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) || i.consume_key(Modifiers::CTRL, Key::PageUp) {
                    sheet.active = (sheet.active + n - 1) % n;
                }
                if i.consume_key(Modifiers::CTRL, Key::Tab) || i.consume_key(Modifiers::CTRL, Key::PageDown) {
                    sheet.active = (sheet.active + 1) % n;
                }
                if i.consume_key(Modifiers::NONE, Key::Insert) || i.consume_key(Modifiers::COMMAND, Key::N) {
                    op = Some(TabOp::Add);
                }
                if i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::COMMAND, Key::Backspace) {
                    op = Some(TabOp::Remove);
                }
                if i.consume_key(Modifiers::NONE, Key::F2) {
                    op = Some(TabOp::Edit);
                }
                if i.consume_key(Modifiers::NONE, Key::F5) {
                    op = Some(TabOp::Reload);
                }
            });
        }

        // Lazily load the active tab.
        let kind = sheet.tabs[sheet.active].kind;
        if !sheet.tabs[sheet.active].requested {
            sheet.tabs[sheet.active].requested = true;
            if sheet.node == Node::RemoteServers {
                // A server's tabs come from its last check, already in hand.
                let tab = &mut sheet.tabs[sheet.active];
                tab.body = crate::servers::tab_body(kind, &sheet.obj);
                tab.sel = match &tab.body {
                    TabBody::Table { rows, .. } if !rows.is_empty() => Some(tab.sel.unwrap_or(0).min(rows.len() - 1)),
                    _ => None,
                };
            } else {
                if !matches!(sheet.tabs[sheet.active].body, TabBody::Table { .. } | TabBody::Fields(_)) {
                    sheet.tabs[sheet.active].body = TabBody::Loading;
                }
                self.start_tab_load(sheet_id, sheet.node, kind, sheet.obj.clone());
            }
        }

        let labels = sheet.op_labels(kind);
        let extras = if sheet.node == Node::Users { crate::security::extra_ops(kind) } else { &[] };
        let name = sheet.name();
        modal.show(ctx, |ui| {
            ui.set_width(760.0);
            ui.set_min_height(440.0);
            ui.heading(format!("{} {}", sheet.node.icon(), tf("sheet.title", &[("name", &name)])));
            ui.add_space(6.0);
            // Tab strip.
            ui.horizontal_wrapped(|ui| {
                for (i, tab) in sheet.tabs.iter().enumerate() {
                    let title = tab.kind.title();
                    let r = ui.selectable_label(sheet.active == i, &title);
                    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sheet.active == i, tf("sheet.tab", &[("tab", &title)])));
                    if r.clicked() {
                        sheet.active = i;
                    }
                }
            });
            ui.label(RichText::new(t("sheet.ctrl_tab")).small().weak());
            ui.separator();
            let tab = &mut sheet.tabs[sheet.active];
            let body_height = 330.0;
            // Behind the tab's contents, so it gets the right-clicks that miss everything else.
            let body_rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), body_height));
            let mut menu_targets = vec![ui.interact(body_rect, ui.id().with(("tab-menu", sheet_id)), Sense::CLICK)];
            match &tab.body {
                TabBody::Loading => {
                    ui.allocate_ui(egui::vec2(ui.available_width(), body_height), |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t("sheet.loading"));
                        });
                    });
                }
                TabBody::Error(e) => {
                    ui.allocate_ui(egui::vec2(ui.available_width(), body_height), |ui| {
                        ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {e}"));
                    });
                }
                TabBody::Fields(fields) => {
                    egui::ScrollArea::vertical().max_height(body_height).auto_shrink([false, false]).show(ui, |ui| {
                        egui::Grid::new(("fields", sheet_id, kind as u8)).num_columns(2).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                            for (k, v) in fields {
                                menu_targets.push(ui.label(RichText::new(k).bold()));
                                menu_targets.push(ui.add(egui::Label::new(if v.is_empty() { "—" } else { v.as_str() }).wrap().selectable(true)));
                                ui.end_row();
                            }
                        });
                    });
                }
                TabBody::Table { headers, rows } => {
                    if rows.is_empty() {
                        ui.allocate_ui(egui::vec2(ui.available_width(), body_height), |ui| {
                            ui.label(t("sheet.empty"));
                        });
                    } else {
                        let headers: Vec<String> = headers.iter().map(|h| t(h)).collect();
                        let mut clicked = None;
                        let mut first_focus = first;
                        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
                        let mut tb = TableBuilder::new(ui)
                            .id_salt(("sheet-table", sheet_id, kind as u8))
                            .striped(true)
                            .resizable(true)
                            .sense(Sense::click())
                            .max_scroll_height(body_height)
                            .min_scrolled_height(body_height)
                            .auto_shrink([false, false])
                            .cell_layout(Layout::left_to_right(Align::Center));
                        for i in 0..headers.len() {
                            tb = tb.column(if i + 1 == headers.len() { Column::remainder() } else { Column::initial(200.0).clip(true) });
                        }
                        tb.header(row_h, |mut h| {
                            for title in &headers {
                                h.col(|ui| {
                                    ui.label(RichText::new(title).bold());
                                });
                            }
                        })
                        .body(|mut body| {
                            for (ri, row) in rows.iter().enumerate() {
                                body.row(row_h, |mut r| {
                                    let is_sel = tab.sel == Some(ri);
                                    r.set_selected(is_sel);
                                    for (ci, cell) in row.cells.iter().enumerate() {
                                        r.col(|ui| {
                                            if ci == 0 {
                                                let b = ui.add(egui::Button::selectable(is_sel, cell.as_str()).frame_when_inactive(false));
                                                b.widget_info(|| {
                                                    let all: Vec<String> = headers.iter().zip(&row.cells).map(|(h, c)| format!("{h}: {c}")).collect();
                                                    egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is_sel, all.join(", "))
                                                });
                                                if first_focus && is_sel {
                                                    b.request_focus();
                                                    first_focus = false;
                                                }
                                                if b.clicked() || b.secondary_clicked() || b.gained_focus() {
                                                    clicked = Some(ri);
                                                }
                                                menu_targets.push(b);
                                            } else {
                                                // Not selectable, so clicks on the text reach the row.
                                                ui.add(egui::Label::new(cell.as_str()).truncate().selectable(false));
                                            }
                                        });
                                    }
                                    let resp = r.response();
                                    if resp.clicked() || resp.secondary_clicked() {
                                        clicked = Some(ri);
                                    }
                                    menu_targets.push(resp);
                                });
                            }
                        });
                        if let Some(c) = clicked {
                            tab.sel = Some(c);
                        }
                    }
                }
            }
            let has_row = tab.sel.is_some() && matches!(tab.body, TabBody::Table { .. });
            for target in &menu_targets {
                target.context_menu(|ui| {
                    if let Some(o) = tab_menu(ui, labels, extras, has_row) {
                        op = Some(o);
                        ui.close();
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if let Some(l) = labels[0]
                    && ui.button(t(l)).on_hover_text("Insert").clicked() {
                        op = Some(TabOp::Add);
                    }
                if let Some(l) = labels[1]
                    && ui.add_enabled(has_row, egui::Button::new(t(l))).on_hover_text("Delete").clicked() {
                        op = Some(TabOp::Remove);
                    }
                if let Some(l) = labels[2]
                    && ui.button(t(l)).on_hover_text("F2").clicked() {
                        op = Some(TabOp::Edit);
                    }
                for (i, l) in extras.iter().enumerate() {
                    if ui.button(t(l)).clicked() {
                        op = Some(TabOp::Extra(i));
                    }
                }
                if ui.button(t("act.refresh")).on_hover_text("F5").clicked() {
                    op = Some(TabOp::Reload);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let c = ui.button(format!("  {}  ", t("dialog.close")));
                    if first && !matches!(tab.body, TabBody::Table { .. }) {
                        c.request_focus();
                    }
                    close |= c.clicked();
                });
            });
        });

        if let Some(op) = op.filter(|_| top) {
            self.sheet_op(sheet_id, sheet, op);
        }
        !close
    }

    fn sheet_op(&mut self, sheet_id: u64, sheet: &mut Sheet, op: TabOp) {
        use TabKind::*;
        let kind = sheet.tabs[sheet.active].kind;
        let node = sheet.node;
        let labels = sheet.op_labels(kind);
        let tab = &mut sheet.tabs[sheet.active];
        let (row, cells): (Option<Value>, Vec<String>) = match (&tab.body, tab.sel) {
            (TabBody::Table { rows, .. }, Some(i)) => rows.get(i).map(|r| (Some(r.data.clone()), r.cells.clone())).unwrap_or_default(),
            _ => (None, Vec::new()),
        };
        if node == Node::RemoteServers {
            // Refresh checks the server again; Edit changes how it is reached.
            match op {
                TabOp::Reload => self.check_server(&s(&sheet.obj, "id")),
                TabOp::Edit if kind == General => self.server_form(Some(sheet.obj.clone())),
                _ => {}
            }
            return;
        }
        if op == TabOp::Reload {
            tab.requested = false;
            return;
        }
        let obj = sheet.obj.clone();
        if node == Node::Users && matches!(kind, Security | AuthMethods | SignIns) {
            return self.security_sheet_op(sheet_id, kind, op, obj, row);
        }
        if matches!(node, Node::Mailboxes | Node::DistributionLists) {
            let has_op = match op {
                TabOp::Add => labels[0].is_some(),
                TabOp::Remove => labels[1].is_some(),
                TabOp::Edit => labels[2].is_some(),
                TabOp::Reload | TabOp::Extra(_) => false,
            };
            if has_op {
                self.exchange_sheet_op(sheet_id, node, kind, op, obj, row);
            }
            return;
        }
        let id = s(&obj, "id");
        let name = node.object_name(&obj);
        let reload = move |app: &mut App| app.reload_tab(sheet_id, kind);

        match op {
            TabOp::Add if labels[0].is_some() => match kind {
                MemberOf => self.pick_group(&tf("pick.add_to_group", &[("name", &name)]), move |app, g| app.add_member(&g.id, &id, reload)),
                Members => self.pick_principal(&tf("pick.add_member_to", &[("name", &name)]), move |app, m| app.add_member(&id, &m.id, reload)),
                AppUsers => self.assign_app(obj, reload),
                Owners => {
                    let coll = collection(node);
                    self.pick_user(&tf("pick.add_owner_to", &[("name", &name)]), move |app, u| app.add_owner(coll, &id, &u.id, reload));
                }
                Roles => {
                    if node == Node::Groups && obj["isAssignableToRole"] != true {
                        self.push(Dialog::message(&t("task.assign_role"), &t("sheet.role_assignable_only")));
                        return;
                    }
                    self.pick_role(&tf("pick.assign_role_to", &[("name", &name)]), move |app, r| app.assign_role(&id, &r.id, reload));
                }
                UserLicences | GroupLicences => {
                    let coll = collection(node);
                    self.pick_sku(&tf("pick.assign_licence_to", &[("name", &name)]), move |app, sku| {
                        app.set_licence(coll, &id, &sku.id, true, move |app| {
                            reload(app);
                            app.load(node);
                        })
                    });
                }
                LicensedUsers => {
                    let sku = s(&obj, "skuId");
                    self.pick_user(&tf("pick.assign_to_user", &[("name", &name)]), move |app, u| {
                        app.set_licence("users", &u.id, &sku, true, move |app| {
                            reload(app);
                            app.load(Node::Licences);
                        })
                    });
                }
                _ => {}
            },
            TabOp::Remove if labels[1].is_some() => {
                let Some(row) = row else { return };
                let rid = s(&row, "id");
                let (what, path_or_body): (String, Work) = match kind {
                    MemberOf => {
                        let path = format!("/groups/{rid}/members/{id}/$ref");
                        let question = tf("sheet.remove_from_question", &[("name", &name), ("from", &s(&row, "displayName"))]);
                        (question, Box::new(move |g| g.delete(&path)))
                    }
                    Members | Owners => {
                        let (rel, key) = if kind == Members { ("members", "sheet.remove_member_question") } else { ("owners", "sheet.remove_owner_question") };
                        let path = format!("/{}/{id}/{rel}/{rid}/$ref", collection(node));
                        (tf(key, &[("member", &s(&row, "displayName")), ("name", &name)]), Box::new(move |g| g.delete(&path)))
                    }
                    Roles => {
                        let path = format!("/roleManagement/directory/roleAssignments/{rid}");
                        let question = tf("sheet.remove_role_question", &[("role", &s(&row["roleDefinition"], "displayName")), ("name", &name)]);
                        (question, Box::new(move |g| g.delete(&path)))
                    }
                    UserLicences | GroupLicences => {
                        let sku = s(&row, "skuId");
                        let path = format!("/{}/{id}/assignLicense", collection(node));
                        let body = json!({ "addLicenses": [], "removeLicenses": [sku] });
                        (tf("sheet.remove_licence_question", &[("name", &name)]), Box::new(move |g| g.post(&path, &body).map(|_| ())))
                    }
                    AppUsers => {
                        let path = format!("/servicePrincipals/{id}/appRoleAssignedTo/{rid}");
                        let question = tf("sheet.remove_assignment_question", &[("member", &s(&row, "principalDisplayName")), ("name", &name)]);
                        (question, Box::new(move |g| g.delete(&path)))
                    }
                    Permissions => {
                        let (what, path) = if row["kind"] == "application" {
                            let args: [(&str, &dyn std::fmt::Display); 3] = [("permission", &cells[1]), ("api", &cells[0]), ("name", &name)];
                            (tf("sheet.revoke_application_question", &args), format!("/servicePrincipals/{id}/appRoleAssignments/{rid}"))
                        } else {
                            let key = if row["consentType"] == "AllPrincipals" {
                                "sheet.revoke_delegated_all_question"
                            } else {
                                "sheet.revoke_delegated_question"
                            };
                            let args: [(&str, &dyn std::fmt::Display); 4] =
                                [("api", &cells[0]), ("name", &name), ("user", &cells[3]), ("permissions", &cells[1])];
                            (tf(key, &args), format!("/oauth2PermissionGrants/{rid}"))
                        };
                        (what, Box::new(move |g| g.delete(&path)))
                    }
                    LicensedUsers => {
                        let path = format!("/users/{rid}/assignLicense");
                        let body = json!({ "addLicenses": [], "removeLicenses": [s(&obj, "skuId")] });
                        let question = tf("sheet.remove_from_question", &[("name", &name), ("from", &s(&row, "displayName"))]);
                        (question, Box::new(move |g| g.post(&path, &body).map(|_| ())))
                    }
                    _ => return,
                };
                let label = t(labels[1].unwrap_or("sheet.remove"));
                let title = label.clone();
                self.push(Dialog::confirm(&title, &what, move |app| {
                    app.mutate(&label, path_or_body, move |app| {
                        reload(app);
                        if matches!(kind, UserLicences | GroupLicences | LicensedUsers) {
                            app.load(node);
                            app.load(Node::Licences);
                        }
                    });
                }));
            }
            TabOp::Edit if labels[2].is_some() => self.edit_general(sheet_id, node, obj),
            _ => {}
        }
    }

    fn edit_general(&mut self, sheet_id: u64, node: Node, obj: Value) {
        let id = s(&obj, "id");
        let form = match node {
            Node::Users => {
                let mut f = Form::new(&tf("sheet.edit_title", &[("name", &s(&obj, "displayName"))]), &t("dialog.save"));
                for (key, label) in [
                    ("displayName", "field.display_name"),
                    ("givenName", "field.first_name"),
                    ("surname", "field.last_name"),
                    ("jobTitle", "field.job_title"),
                    ("department", "field.department"),
                    ("companyName", "field.company"),
                    ("officeLocation", "field.office"),
                    ("mobilePhone", "field.mobile_phone"),
                    ("employeeId", "field.employee_id"),
                    ("usageLocation", "field.usage_location"),
                ] {
                    let field = Field::text(key, &t(label), &s(&obj, key));
                    f = f.field(if key == "displayName" { field.required() } else { field });
                }
                f
            }
            Node::Groups => Form::new(&tf("sheet.edit_title", &[("name", &s(&obj, "displayName"))]), &t("dialog.save"))
                .field(Field::text("displayName", &t("field.name"), &s(&obj, "displayName")).required())
                .field(Field::multiline("description", &t("field.description"), &s(&obj, "description"))),
            Node::Applications => return self.edit_application(sheet_id, obj),
            _ => return,
        };
        let form = form.on_submit(move |app, f| {
            if !crate::ops::is_usage_location(&f.text("usageLocation")) {
                return Err(t("user.bad_usage_location"));
            }
            let mut body = json!({});
            for field in &f.fields {
                if field.key.is_empty() {
                    continue;
                }
                let key = field.key;
                let new = f.text(key);
                match key {
                    _ if new != s(&obj, key) => {
                        body[key] = if new.is_empty() {
                            Value::Null
                        } else if key == "usageLocation" {
                            json!(new.to_uppercase())
                        } else {
                            json!(new)
                        };
                    }
                    _ => {}
                }
            }
            let path = format!("/{}/{id}", collection(node));
            let patch = body.clone();
            let oid = id.clone();
            app.mutate(&t("task.save_changes"), move |g| g.patch(&path, &patch), move |app| {
                let mut updated = body;
                updated["id"] = json!(oid);
                app.upsert(node, updated);
                app.reload_tab(sheet_id, TabKind::General);
            });
            Ok(())
        });
        self.push(Dialog::Form(form));
    }

    fn edit_application(&mut self, sheet_id: u64, obj: Value) {
        let id = s(&obj, "id");
        let visible = model::visible_to_users(&obj) != false;
        // A missing value shows as the default, and only counts as changed if the box is.
        let was = [("accountEnabled", obj["accountEnabled"] != false), ("appRoleAssignmentRequired", obj["appRoleAssignmentRequired"] == true)];
        let form = Form::new(&tf("sheet.edit_title", &[("name", &s(&obj, "displayName"))]), &t("dialog.save"))
            .field(Field::check("accountEnabled", &t("prop.sign_in_enabled"), was[0].1))
            .field(Field::check("appRoleAssignmentRequired", &t("prop.assignment_required"), was[1].1))
            .field(Field::check("visible", &t("prop.visible_to_users"), visible))
            .field(Field::multiline("notes", &t("prop.notes"), &s(&obj, "notes")))
            .intro(&t("sheet.app_intro"))
            .on_submit(move |app, f| {
                let mut body = json!({});
                for (key, before) in was {
                    if f.checked(key) != before {
                        body[key] = json!(f.checked(key));
                    }
                }
                if f.checked("visible") != visible {
                    let mut tags: Vec<Value> = obj["tags"].as_array().cloned().unwrap_or_default();
                    tags.retain(|t| t != model::HIDE_APP_TAG);
                    if !f.checked("visible") {
                        tags.push(json!(model::HIDE_APP_TAG));
                    }
                    body["tags"] = json!(tags);
                }
                let notes = f.text("notes");
                if notes != s(&obj, "notes") {
                    body["notes"] = if notes.is_empty() { Value::Null } else { json!(notes) };
                }
                let path = format!("/servicePrincipals/{id}");
                let patch = body.clone();
                let oid = id.clone();
                app.mutate(&t("task.save_changes"), move |g| g.patch(&path, &patch), move |app| {
                    let mut updated = body;
                    updated["id"] = json!(oid);
                    app.upsert(Node::Applications, updated);
                    app.reload_tab(sheet_id, TabKind::General);
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }
}
