//! Actions on the selected console node or object, and the Graph operations behind them.

use crate::app::App;
use crate::config::Settings;
use crate::dialogs::{Dialog, Field, Form, PickItem, Picker};
use crate::graph::{odata_str, Graph};
use crate::model::{s, sku_name, Node};
use crate::sheets::Sheet;
use eframe::egui::{Key, KeyboardShortcut, Modifiers};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Open,
    Refresh,
    New,
    Properties,
    ToggleEnabled,
    ResetPassword,
    AssignLicence,
    AddToGroup,
    AddMember,
    AssignRole,
    AssignToUser,
    Delete,
    ExportList,
    ExportDatabase,
    ImportUsers,
    ImportMembers,
}

impl Act {
    pub fn shortcuts(self) -> Vec<KeyboardShortcut> {
        let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        match self {
            Act::Open => vec![],
            Act::Refresh => vec![KeyboardShortcut::new(Modifiers::NONE, Key::F5)],
            Act::New => vec![cmd(Key::N)],
            Act::Properties => vec![KeyboardShortcut::new(Modifiers::ALT, Key::Enter)],
            Act::ToggleEnabled => vec![cmd(Key::E)],
            Act::ResetPassword => vec![cmd(Key::P)],
            Act::AssignLicence => vec![cmd(Key::L)],
            Act::AddToGroup => vec![cmd(Key::G)],
            Act::AddMember => vec![cmd(Key::M)],
            Act::AssignRole => vec![cmd(Key::R)],
            Act::AssignToUser => vec![cmd(Key::U)],
            Act::Delete => vec![KeyboardShortcut::new(Modifiers::NONE, Key::Delete), cmd(Key::Backspace)],
            Act::ExportList | Act::ExportDatabase | Act::ImportUsers | Act::ImportMembers => vec![],
        }
    }

    pub fn needs_selection(self) -> bool {
        !matches!(self, Act::Refresh | Act::New | Act::ExportList | Act::ExportDatabase | Act::ImportUsers)
    }
}

/// Actions that apply to the node itself (shown at the top of the Actions pane).
pub fn node_actions(node: Node) -> Vec<Act> {
    match node {
        Node::Users => vec![Act::New, Act::ImportUsers, Act::ExportList, Act::ExportDatabase, Act::Refresh],
        Node::Groups => vec![Act::New, Act::ExportList, Act::ExportDatabase, Act::Refresh],
        Node::Devices | Node::Licences => vec![Act::ExportList, Act::ExportDatabase, Act::Refresh],
        Node::Root => vec![],
    }
}

/// Actions that apply to the selected object.
pub fn item_actions(node: Node) -> Vec<Act> {
    use Act::*;
    match node {
        Node::Root => vec![Open],
        Node::Users => vec![Properties, ToggleEnabled, ResetPassword, AssignLicence, AddToGroup, AssignRole, Delete],
        Node::Groups => vec![Properties, AddMember, ImportMembers, AssignRole, AssignLicence, Delete],
        Node::Devices => vec![Properties, ToggleEnabled, Delete],
        Node::Licences => vec![Properties, AssignToUser],
    }
}

/// Graph collection for objects of a node.
pub fn collection(node: Node) -> &'static str {
    match node {
        Node::Users => "users",
        Node::Groups => "groups",
        Node::Devices => "devices",
        Node::Licences => "subscribedSkus",
        Node::Root => "",
    }
}

impl App {
    pub fn act_label(&self, act: Act) -> String {
        let enabled = self.selected().and_then(|o| o["accountEnabled"].as_bool()).unwrap_or(true);
        match act {
            Act::Open => "Open".into(),
            Act::Refresh => "Refresh".into(),
            Act::New => match self.node {
                Node::Users => "New user…",
                Node::Groups => "New group…",
                _ => "New…",
            }
            .into(),
            Act::Properties => "Properties".into(),
            Act::ToggleEnabled => match (self.node, enabled) {
                (Node::Users, true) => "Disable account",
                (Node::Users, false) => "Enable account",
                (_, true) => "Disable",
                (_, false) => "Enable",
            }
            .into(),
            Act::ResetPassword => "Reset password…".into(),
            Act::AssignLicence => "Assign licence…".into(),
            Act::AddToGroup => "Add to group…".into(),
            Act::AddMember => "Add member…".into(),
            Act::AssignRole => "Assign directory role…".into(),
            Act::AssignToUser => "Assign to user…".into(),
            Act::Delete => "Delete".into(),
            Act::ExportList => "Export list…".into(),
            Act::ExportDatabase => "Export to MariaDB…".into(),
            Act::ImportUsers => "Import users from CSV…".into(),
            Act::ImportMembers => "Add members from CSV…".into(),
        }
    }

    pub fn run_act(&mut self, act: Act) {
        log::debug!("Command {act:?} on {}", self.node.title());
        let node = self.node;
        let obj = self.selected().cloned();
        if act.needs_selection() && obj.is_none() {
            return;
        }
        let obj = obj.unwrap_or(Value::Null);
        let id = s(&obj, "id");
        let name = node.object_name(&obj);
        match act {
            Act::Open => self.open_child(),
            Act::Refresh => self.refresh(),
            Act::Properties => {
                let sheet = Sheet::new(node, obj);
                self.push(Dialog::Sheet(Box::new(sheet)));
            }
            Act::New => match node {
                Node::Users => self.new_user(),
                Node::Groups => self.new_group(),
                _ => {}
            },
            Act::ToggleEnabled => self.toggle_enabled(node, obj),
            Act::ResetPassword => self.reset_password(id, name),
            Act::AssignLicence => {
                let coll = collection(node);
                self.pick_sku(&format!("Assign licence to {name}"), move |app, sku| {
                    app.set_licence(coll, &id, &sku.id, true, move |app| app.load(node));
                });
            }
            Act::AddToGroup => self.pick_group(&format!("Add {name} to group"), move |app, g| {
                app.add_member(&g.id, &id, |_| {});
            }),
            Act::AddMember => self.pick_principal(&format!("Add member to {name}"), move |app, m| {
                app.add_member(&id, &m.id, |_| {});
            }),
            Act::AssignRole => {
                if node == Node::Groups && obj["isAssignableToRole"] != true {
                    self.push(Dialog::message(
                        "Assign directory role",
                        "Directory roles can only be assigned to groups created as role-assignable \
                         (isAssignableToRole). This can only be set when a group is created.",
                    ));
                    return;
                }
                self.pick_role(&format!("Assign directory role to {name}"), move |app, role| {
                    app.assign_role(&id, &role.id, |_| {});
                });
            }
            Act::AssignToUser => {
                let sku = s(&obj, "skuId");
                self.pick_user(&format!("Assign {name} to user"), move |app, u| {
                    app.set_licence("users", &u.id, &sku, true, |app| app.load(Node::Licences));
                });
            }
            Act::Delete => self.delete_obj(node, id, name),
            Act::ExportList => self.export_list(),
            Act::ExportDatabase => self.export_database(),
            Act::ImportUsers => self.import_users(),
            Act::ImportMembers => self.import_members(obj),
        }
    }

    /// Runs a Graph mutation, reporting failure in a dialog and success in the status bar.
    pub fn mutate(
        &mut self,
        label: &str,
        work: impl FnOnce(&Graph) -> anyhow::Result<()> + Send + 'static,
        after: impl FnOnce(&mut App) + Send + 'static,
    ) {
        let title = label.to_string();
        log::info!("Action: {title}");
        self.spawn(format!("{label}…"), work, move |app, r| match r {
            Ok(()) => {
                app.status = format!("{title}: done");
                after(app);
            }
            Err(e) => app.error(&title, e),
        });
    }

    // ----- settings ------------------------------------------------------

    pub fn open_settings(&mut self) {
        if self.dialogs.iter().any(|d| matches!(&d.dialog, Dialog::Form(f) if f.title == "Settings")) {
            return;
        }
        let s = &self.settings;
        let form = Form::new("Settings", "Save")
            .intro(
                "Enter the directory (tenant) ID and the application (client) ID of an app registration \
                 with \"Allow public client flows\" enabled and delegated Microsoft Graph permissions.",
            )
            .field(Field::text("tenant", "Tenant ID", &s.tenant_id).required())
            .field(Field::text("client", "Client ID", &s.client_id).required())
            .field(Field::check("remember", "Remember sign-in on this computer", s.remember_sign_in))
            .field(Field::multiline("scopes", "Scopes", &s.scopes))
            .field(Field::info("Stored in", &crate::config::settings_path().display().to_string()))
            .on_submit(|app, f| {
                let client = f.text("client");
                if !is_guid(&client) {
                    return Err("Client ID must be a GUID, for example 00000000-0000-0000-0000-000000000000.".into());
                }
                let tenant = f.text("tenant");
                if tenant.contains(char::is_whitespace) || tenant.contains('/') {
                    return Err("Tenant ID must be a GUID or a domain such as contoso.onmicrosoft.com.".into());
                }
                let scopes = f.text("scopes").split_whitespace().collect::<Vec<_>>().join(" ");
                let new = Settings {
                    tenant_id: tenant,
                    client_id: client,
                    remember_sign_in: f.checked("remember"),
                    scopes: if scopes.is_empty() { crate::config::DEFAULT_SCOPES.into() } else { scopes },
                    ..app.settings.clone()
                };
                new.save().map_err(|e| e.to_string())?;
                let reconnect = new.tenant_id != app.settings.tenant_id
                    || new.client_id != app.settings.client_id
                    || new.scopes != app.settings.scopes
                    || app.graph.is_none();
                if !new.remember_sign_in {
                    let _ = std::fs::remove_file(crate::config::token_path());
                }
                if reconnect {
                    app.sign_out();
                } else if let Some(g) = &app.graph {
                    // The running sign-in keeps its own copy of the settings.
                    g.auth.set_remember(new.remember_sign_in);
                }
                app.settings = new;
                if reconnect {
                    app.connect();
                }
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    // ----- users ---------------------------------------------------------

    fn new_user(&mut self) {
        let domains: Vec<(String, String)> = self.domains.iter().map(|d| (d.clone(), format!("@{d}"))).collect();
        let mut form = Form::new("New user", "Create")
            .field(Field::text("displayName", "Display name", "").required())
            .field(Field::text("alias", "User name", "").required());
        form = if domains.is_empty() {
            form.field(Field::text("domain", "Domain", ""))
        } else {
            form.field(Field::choice("domain", "Domain", domains, 0))
        };
        let form = form
            .field(Field::text("givenName", "First name", ""))
            .field(Field::text("surname", "Last name", ""))
            .field(Field::text("jobTitle", "Job title", ""))
            .field(Field::text("department", "Department", ""))
            .field(Field::text("usageLocation", "Usage location", ""))
            .field(Field::password("password", "Password", &generate_password()))
            .field(Field::check("force", "Require password change at next sign-in", true))
            .field(Field::check("enabled", "Account enabled", true))
            .intro("Usage location is a two-letter country code (e.g. GB) and is required before licences can be assigned.")
            .on_submit(|app, f| {
                let domain = match f.choice("domain") {
                    d if !d.is_empty() => d,
                    _ => f.text("domain"),
                };
                let alias = f.text("alias");
                if alias.contains('@') || alias.contains(' ') {
                    return Err("User name must not contain spaces or @; choose the domain separately.".into());
                }
                if domain.is_empty() {
                    return Err("Domain is required.".into());
                }
                let password = f.raw("password");
                if password.len() < 8 {
                    return Err("Password must be at least 8 characters.".into());
                }
                let mut body = json!({
                    "accountEnabled": f.checked("enabled"),
                    "displayName": f.text("displayName"),
                    "mailNickname": alias,
                    "userPrincipalName": format!("{alias}@{domain}"),
                    "passwordProfile": {
                        "password": password,
                        "forceChangePasswordNextSignIn": f.checked("force"),
                    },
                });
                for key in ["givenName", "surname", "jobTitle", "department", "usageLocation"] {
                    let v = f.text(key);
                    if !v.is_empty() {
                        body[key] = json!(if key == "usageLocation" { v.to_uppercase() } else { v });
                    }
                }
                app.spawn("Creating user…", move |g| g.post("/users", &body), move |app, r| match r {
                    Ok(user) => {
                        let upn = s(&user, "userPrincipalName");
                        app.upsert(Node::Users, user);
                        app.push(Dialog::Message {
                            title: "User created".into(),
                            text: format!("{upn} was created. Give the user this initial password:"),
                            copy: Some(password),
                        });
                    }
                    Err(e) => app.error("New user", e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    fn reset_password(&mut self, id: String, name: String) {
        let form = Form::new(&format!("Reset password — {name}"), "Reset")
            .field(Field::password("password", "New password", &generate_password()).required())
            .field(Field::check("force", "Require password change at next sign-in", true))
            .on_submit(move |app, f| {
                let password = f.raw("password");
                if password.len() < 8 {
                    return Err("Password must be at least 8 characters.".into());
                }
                let body = json!({ "passwordProfile": {
                    "password": password,
                    "forceChangePasswordNextSignIn": f.checked("force"),
                }});
                let path = format!("/users/{id}");
                let who = name.clone();
                app.spawn("Resetting password…", move |g| g.patch(&path, &body), move |app, r| match r {
                    Ok(()) => {
                        app.push(Dialog::Message {
                            title: "Password reset".into(),
                            text: format!("The password for {who} was reset to:"),
                            copy: Some(password),
                        });
                    }
                    Err(e) => app.error("Reset password", e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    fn toggle_enabled(&mut self, node: Node, obj: Value) {
        let enable = obj["accountEnabled"] == false;
        let id = s(&obj, "id");
        let path = format!("/{}/{id}", collection(node));
        let verb = if enable { "Enable" } else { "Disable" };
        let label = format!("{verb} {}", node.object_name(&obj));
        let run = move |app: &mut App| {
            app.mutate(&label, move |g| g.patch(&path, &json!({ "accountEnabled": enable })), move |app| {
                app.upsert(node, json!({ "id": id, "accountEnabled": enable }));
            });
        };
        if enable {
            run(self);
        } else {
            let what = match node {
                Node::Users => "The user will be unable to sign in.",
                _ => "The device will be unable to authenticate with Microsoft Entra ID.",
            };
            let text = format!("Disable {} '{}'? {what}", node.noun(), node.object_name(&obj));
            self.push(Dialog::confirm(&format!("Disable {}", node.noun()), &text, run));
        }
    }

    fn delete_obj(&mut self, node: Node, id: String, name: String) {
        let note = match node {
            Node::Users => "Deleted users can be restored for 30 days.",
            Node::Groups => "Microsoft 365 groups can be restored for 30 days; security groups are deleted permanently.",
            Node::Devices => "This cannot be undone.",
            _ => "",
        };
        let text = format!("Are you sure you want to delete the {} '{name}'? {note}", node.noun());
        self.push(Dialog::confirm(&format!("Delete {}", node.noun()), &text, move |app| {
            let path = format!("/{}/{id}", collection(node));
            app.mutate(&format!("Delete {name}"), move |g| g.delete(&path), move |app| {
                app.remove_obj(node, &id);
            });
        }));
    }

    // ----- groups --------------------------------------------------------

    fn new_group(&mut self) {
        let types = vec![
            ("security".to_string(), "Security".to_string()),
            ("m365".to_string(), "Microsoft 365".to_string()),
        ];
        let form = Form::new("New group", "Create")
            .field(Field::choice("type", "Group type", types, 0))
            .field(Field::text("displayName", "Name", "").required())
            .field(Field::text("mailNickname", "Mail nickname", ""))
            .field(Field::multiline("description", "Description", ""))
            .field(Field::check("roles", "Microsoft Entra roles can be assigned to this group", false))
            .intro("Role-assignable groups can only be created by a Privileged Role Administrator or Global Administrator, and the setting cannot be changed later.")
            .on_submit(|app, f| {
                let m365 = f.choice("type") == "m365";
                let roles = f.checked("roles");
                let name = f.text("displayName");
                let mut nick = f.text("mailNickname");
                if nick.is_empty() {
                    nick = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                }
                if nick.is_empty() {
                    return Err("Enter a mail nickname using letters and numbers.".into());
                }
                let mut body = json!({
                    "displayName": name,
                    "mailNickname": nick,
                    "mailEnabled": m365,
                    "securityEnabled": !m365 || roles,
                    "groupTypes": if m365 { vec!["Unified"] } else { vec![] },
                });
                let desc = f.text("description");
                if !desc.is_empty() {
                    body["description"] = json!(desc);
                }
                if roles {
                    body["isAssignableToRole"] = json!(true);
                }
                app.spawn("Creating group…", move |g| g.post("/groups", &body), |app, r| match r {
                    Ok(group) => {
                        app.status = format!("Created group {}", s(&group, "displayName"));
                        app.upsert(Node::Groups, group);
                    }
                    Err(e) => app.error("New group", e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn add_member(&mut self, group_id: &str, member_id: &str, after: impl FnOnce(&mut App) + Send + 'static) {
        let path = format!("/groups/{group_id}/members/$ref");
        let body = json!({ "@odata.id": format!("{}/v1.0/directoryObjects/{member_id}", self.settings.graph_endpoint) });
        self.mutate("Add member", move |g| g.post(&path, &body).map(|_| ()), after);
    }

    pub fn add_owner(&mut self, coll: &str, obj_id: &str, user_id: &str, after: impl FnOnce(&mut App) + Send + 'static) {
        let path = format!("/{coll}/{obj_id}/owners/$ref");
        let body = json!({ "@odata.id": format!("{}/v1.0/directoryObjects/{user_id}", self.settings.graph_endpoint) });
        self.mutate("Add owner", move |g| g.post(&path, &body).map(|_| ()), after);
    }

    pub fn assign_role(&mut self, principal_id: &str, role_id: &str, after: impl FnOnce(&mut App) + Send + 'static) {
        let body = json!({ "principalId": principal_id, "roleDefinitionId": role_id, "directoryScopeId": "/" });
        self.mutate(
            "Assign directory role",
            move |g| g.post("/roleManagement/directory/roleAssignments", &body).map(|_| ()),
            after,
        );
    }

    /// Adds (`add = true`) or removes a licence on a user or group (`coll` = "users" / "groups").
    pub fn set_licence(&mut self, coll: &str, id: &str, sku_id: &str, add: bool, after: impl FnOnce(&mut App) + Send + 'static) {
        let path = format!("/{coll}/{id}/assignLicense");
        let body = if add {
            json!({ "addLicenses": [{ "skuId": sku_id, "disabledPlans": [] }], "removeLicenses": [] })
        } else {
            json!({ "addLicenses": [], "removeLicenses": [sku_id] })
        };
        let label = if add { "Assign licence" } else { "Remove licence" };
        self.mutate(label, move |g| g.post(&path, &body).map(|_| ()), after);
    }

    // ----- pickers -------------------------------------------------------

    pub fn pick_user(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        self.push(Dialog::Picker(Picker::new(title, "Name or UPN:", true, search_users, on_pick)));
    }

    pub fn pick_group(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        self.push(Dialog::Picker(Picker::new(title, "Group name:", true, search_groups, on_pick)));
    }

    /// Users, groups and devices (anything that can be a group member).
    pub fn pick_principal(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        let fetch = |g: &Graph, q: &str| {
            let mut items = search_users(g, q)?;
            items.extend(search_groups(g, q)?);
            items.extend(search_devices(g, q)?);
            Ok(items)
        };
        self.push(Dialog::Picker(Picker::new(title, "Search:", true, fetch, on_pick)));
    }

    pub fn pick_role(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        let fetch = |g: &Graph, _: &str| {
            let mut roles: Vec<PickItem> = g
                .list("/roleManagement/directory/roleDefinitions?$select=id,displayName,description")?
                .into_iter()
                .map(|r| PickItem { id: s(&r, "id"), label: s(&r, "displayName"), detail: s(&r, "description") })
                .collect();
            roles.sort_by_key(|r| r.label.to_lowercase());
            Ok(roles)
        };
        self.push(Dialog::Picker(Picker::new(title, "Role:", false, fetch, on_pick)));
    }

    pub fn pick_sku(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        let fetch = |g: &Graph, _: &str| {
            let mut skus: Vec<PickItem> = g
                .list("/subscribedSkus")?
                .into_iter()
                .map(|v| {
                    let total = v["prepaidUnits"]["enabled"].as_i64().unwrap_or(0);
                    let used = v["consumedUnits"].as_i64().unwrap_or(0);
                    let part = s(&v, "skuPartNumber");
                    PickItem {
                        id: s(&v, "skuId"),
                        label: sku_name(&part),
                        detail: format!("{part} · {} of {total} available", total - used),
                    }
                })
                .collect();
            skus.sort_by_key(|s| s.label.to_lowercase());
            Ok(skus)
        };
        self.push(Dialog::Picker(Picker::new(title, "Licence:", false, fetch, on_pick)));
    }
}

fn search_users(g: &Graph, q: &str) -> anyhow::Result<Vec<PickItem>> {
    let select = "$select=id,displayName,userPrincipalName&$top=50";
    let path = if q.is_empty() {
        format!("/users?{select}")
    } else {
        let q = odata_str(q);
        format!("/users?$filter=startswith(displayName,'{q}') or startswith(userPrincipalName,'{q}') or startswith(mail,'{q}')&{select}")
    };
    Ok(g.get(&path)?["value"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|u| PickItem { id: s(&u, "id"), label: format!("👤 {}", s(&u, "displayName")), detail: s(&u, "userPrincipalName") })
        .collect())
}

fn search_groups(g: &Graph, q: &str) -> anyhow::Result<Vec<PickItem>> {
    let select = "$select=id,displayName,mail,isAssignableToRole&$top=50";
    let path = if q.is_empty() {
        format!("/groups?{select}")
    } else {
        let q = odata_str(q);
        format!("/groups?$filter=startswith(displayName,'{q}') or startswith(mail,'{q}')&{select}")
    };
    Ok(g.get(&path)?["value"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|v| PickItem { id: s(&v, "id"), label: format!("👥 {}", s(&v, "displayName")), detail: s(&v, "mail") })
        .collect())
}

fn search_devices(g: &Graph, q: &str) -> anyhow::Result<Vec<PickItem>> {
    let select = "$select=id,displayName,operatingSystem&$top=25";
    let path = if q.is_empty() {
        format!("/devices?{select}")
    } else {
        format!("/devices?$filter=startswith(displayName,'{}')&{select}", odata_str(q))
    };
    Ok(g.get(&path)?["value"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|v| PickItem { id: s(&v, "id"), label: format!("💻 {}", s(&v, "displayName")), detail: s(&v, "operatingSystem") })
        .collect())
}

pub fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// A 16-character password with upper, lower, digit and symbol characters, from the OS RNG.
pub fn generate_password() -> String {
    const SETS: [&[u8]; 4] = [b"ABCDEFGHJKLMNPQRSTUVWXYZ", b"abcdefghijkmnpqrstuvwxyz", b"23456789", b"!#$%*?-+="];
    let mut rnd = [0u8; 64];
    if getrandom::fill(&mut rnd).is_err() {
        // Extremely unlikely; fall back to time-derived bytes so the form still opens.
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        for (i, b) in rnd.iter_mut().enumerate() {
            *b = (t >> (i % 16 * 8)) as u8 ^ (i as u8).wrapping_mul(37);
        }
    }
    let all: Vec<u8> = SETS.concat();
    let mut chars: Vec<u8> = (0..16)
        .map(|i| {
            let set = if i < 4 { SETS[i] } else { &all[..] };
            set[rnd[i] as usize % set.len()]
        })
        .collect();
    // Fisher–Yates shuffle so the required classes aren't always first.
    for i in (1..chars.len()).rev() {
        let j = rnd[16 + i] as usize % (i + 1);
        chars.swap(i, j);
    }
    // A leading - + or = would be saved with an apostrophe in front in a CSV file (see
    // bulk::escape_cell), and that copy of the password would then be wrong.
    if let Some(i) = chars.iter().position(u8::is_ascii_alphanumeric) {
        chars.swap(0, i);
    }
    String::from_utf8(chars).unwrap_or_default()
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn passwords_have_all_classes() {
        for _ in 0..500 {
            let p = generate_password();
            assert_eq!(p.len(), 16);
            assert!(p.chars().any(|c| c.is_ascii_uppercase()));
            assert!(p.chars().any(|c| c.is_ascii_lowercase()));
            assert!(p.chars().any(|c| c.is_ascii_digit()));
            assert!(p.chars().any(|c| !c.is_ascii_alphanumeric()));
            assert!(p.starts_with(|c: char| c.is_ascii_alphanumeric()), "{p}");
        }
    }

    #[test]
    fn guid_validation() {
        assert!(is_guid("00000003-0000-0000-c000-000000000000"));
        assert!(!is_guid("contoso.onmicrosoft.com"));
    }
}
