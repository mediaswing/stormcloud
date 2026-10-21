//! Actions on the selected console node or object, and the Graph operations behind them.

use crate::app::App;
use crate::config::Settings;
use crate::dialogs::{Dialog, Field, Form, PickItem, Picker};
use crate::graph::{odata_str, Graph};
use crate::i18n::{t, tf};
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
    Forwarding,
    AutoReplies,
    ConvertMailbox,
    Onboard,
    Offboard,
    Security,
    RevokeSessions,
    TemporaryAccessPass,
    CheckServer,
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
            Act::ExportList
            | Act::ExportDatabase
            | Act::ImportUsers
            | Act::ImportMembers
            | Act::Forwarding
            | Act::AutoReplies
            | Act::ConvertMailbox
            | Act::Onboard
            | Act::Offboard
            | Act::Security
            | Act::RevokeSessions
            | Act::TemporaryAccessPass
            | Act::CheckServer => vec![],
        }
    }

    pub fn needs_selection(self) -> bool {
        !matches!(self, Act::Refresh | Act::New | Act::Onboard | Act::ExportList | Act::ExportDatabase | Act::ImportUsers)
    }
}

/// Actions that apply to the node itself (shown at the top of the Actions pane).
pub fn node_actions(node: Node) -> Vec<Act> {
    match node {
        Node::Users => vec![Act::New, Act::Onboard, Act::ImportUsers, Act::ExportList, Act::ExportDatabase, Act::Refresh],
        Node::Groups | Node::Mailboxes | Node::DistributionLists => vec![Act::New, Act::ExportList, Act::ExportDatabase, Act::Refresh],
        Node::Devices | Node::Applications | Node::Licences => vec![Act::ExportList, Act::ExportDatabase, Act::Refresh],
        Node::RemoteServers => vec![Act::New, Act::ExportList, Act::Refresh],
        Node::Root | Node::Exchange => vec![],
    }
}

/// Actions that apply to the selected object.
pub fn item_actions(node: Node) -> Vec<Act> {
    use Act::*;
    match node {
        Node::Root | Node::Exchange => vec![Open],
        Node::Mailboxes => vec![Properties, Forwarding, AutoReplies, ConvertMailbox, Delete],
        Node::DistributionLists => vec![Properties, AddMember, Delete],
        Node::Users => vec![
            Properties,
            Security,
            ToggleEnabled,
            ResetPassword,
            RevokeSessions,
            TemporaryAccessPass,
            AssignLicence,
            AddToGroup,
            AssignRole,
            Offboard,
            Delete,
        ],
        Node::Groups => vec![Properties, AddMember, ImportMembers, AssignRole, AssignLicence, Delete],
        Node::Devices => vec![Properties, ToggleEnabled, Delete],
        Node::Applications => vec![Properties, ToggleEnabled, AssignToUser, Delete],
        Node::Licences => vec![Properties, AssignToUser],
        Node::RemoteServers => vec![Properties, CheckServer, Delete],
    }
}

/// Graph collection for objects of a node.
pub fn collection(node: Node) -> &'static str {
    match node {
        Node::Users => "users",
        Node::Groups => "groups",
        Node::Devices => "devices",
        Node::Applications => "servicePrincipals",
        Node::Licences => "subscribedSkus",
        Node::Root | Node::Exchange | Node::Mailboxes | Node::DistributionLists | Node::RemoteServers => "",
    }
}

impl App {
    pub fn act_label(&self, act: Act) -> String {
        let enabled = self.selected().and_then(|o| o["accountEnabled"].as_bool()).unwrap_or(true);
        t(match act {
            Act::Open => "act.open",
            Act::Refresh => "act.refresh",
            Act::New => match self.node {
                Node::Users => "act.new_user",
                Node::Groups => "act.new_group",
                Node::Mailboxes => "act.new_shared_mailbox",
                Node::DistributionLists => "act.new_distribution_list",
                Node::RemoteServers => "act.add_server",
                _ => "act.new",
            },
            Act::Properties => "act.properties",
            Act::ToggleEnabled => match (self.node, enabled) {
                (Node::Users, true) => "act.disable_account",
                (Node::Users, false) => "act.enable_account",
                (Node::Applications, true) => "act.disable_sign_in",
                (Node::Applications, false) => "act.enable_sign_in",
                (_, true) => "act.disable",
                (_, false) => "act.enable",
            },
            Act::ResetPassword => "act.reset_password",
            Act::AssignLicence => "act.assign_licence",
            Act::AddToGroup => "act.add_to_group",
            Act::AddMember => "act.add_member",
            Act::AssignRole => "act.assign_role",
            Act::AssignToUser if self.node == Node::Applications => "act.assign_users_groups",
            Act::AssignToUser => "act.assign_to_user",
            Act::Delete if self.node == Node::RemoteServers => "act.remove_server",
            Act::Delete => "act.delete",
            Act::ExportList => "act.export_list",
            Act::ExportDatabase => "act.export_database",
            Act::ImportUsers => "act.import_users",
            Act::ImportMembers => "act.import_members",
            Act::Forwarding => "act.forwarding",
            Act::AutoReplies => "act.auto_replies",
            Act::ConvertMailbox => match self.selected().map(|o| s(o, "RecipientTypeDetails")).as_deref() {
                Some("SharedMailbox") => "act.convert_to_user",
                _ => "act.convert_to_shared",
            },
            Act::Onboard => "act.onboard",
            Act::Offboard => "act.offboard",
            Act::Security => "act.security",
            Act::RevokeSessions => "act.revoke_sessions",
            Act::TemporaryAccessPass => "act.temporary_access_pass",
            Act::CheckServer => "act.check_server",
        })
    }

    pub fn run_act(&mut self, act: Act) {
        log::debug!("Command {act:?} on {:?}", self.node);
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
                Node::Mailboxes => self.new_shared_mailbox(),
                Node::DistributionLists => self.new_distribution_list(),
                Node::RemoteServers => self.server_form(None),
                _ => {}
            },
            Act::ToggleEnabled => self.toggle_enabled(node, obj),
            Act::ResetPassword => self.reset_password(id, name),
            Act::AssignLicence => {
                let coll = collection(node);
                self.pick_sku(&tf("pick.assign_licence_to", &[("name", &name)]), move |app, sku| {
                    app.set_licence(coll, &id, &sku.id, true, move |app| app.load(node));
                });
            }
            Act::AddToGroup => self.pick_group(&tf("pick.add_to_group", &[("name", &name)]), move |app, g| {
                app.add_member(&g.id, &id, |_| {});
            }),
            Act::AddMember if node == Node::DistributionLists => self.add_list_member(obj, |_| {}),
            Act::AddMember => self.pick_principal(&tf("pick.add_member_to", &[("name", &name)]), move |app, m| {
                app.add_member(&id, &m.id, |_| {});
            }),
            Act::AssignRole => {
                if node == Node::Groups && obj["isAssignableToRole"] != true {
                    self.push(Dialog::message(&t("task.assign_role"), &t("ops.role_assignable_only")));
                    return;
                }
                self.pick_role(&tf("pick.assign_role_to", &[("name", &name)]), move |app, role| {
                    app.assign_role(&id, &role.id, |_| {});
                });
            }
            Act::AssignToUser if node == Node::Applications => self.assign_app(obj, |_| {}),
            Act::AssignToUser => {
                let sku = s(&obj, "skuId");
                self.pick_user(&tf("pick.assign_to_user", &[("name", &name)]), move |app, u| {
                    app.set_licence("users", &u.id, &sku, true, |app| app.load(Node::Licences));
                });
            }
            Act::Delete if node == Node::RemoteServers => self.remove_server(obj),
            Act::Delete if matches!(node, Node::Mailboxes | Node::DistributionLists) => self.delete_exchange(node, obj),
            Act::Delete => self.delete_obj(node, id, name),
            Act::ExportList => self.export_list(),
            Act::ExportDatabase => self.export_database(),
            Act::ImportUsers => self.import_users(),
            Act::ImportMembers => self.import_members(obj),
            Act::Forwarding => self.edit_mailbox(obj, |_| {}),
            Act::AutoReplies => self.edit_auto_replies(obj, |_| {}),
            Act::ConvertMailbox => self.convert_mailbox(obj),
            Act::Onboard => self.onboard_user(),
            Act::Offboard => self.offboard_user(obj),
            Act::Security => {
                let sheet = Sheet::new(node, obj).on_tab(crate::sheets::TabKind::Security);
                self.push(Dialog::Sheet(Box::new(sheet)));
            }
            Act::RevokeSessions => self.revoke_sessions(obj, |_| {}),
            Act::TemporaryAccessPass => self.temporary_access_pass(obj, |_| {}),
            Act::CheckServer => self.check_server(&id),
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
        self.spawn(tf("status.working", &[("task", &label)]), work, move |app, r| match r {
            Ok(()) => {
                app.status = tf("status.done", &[("task", &title)]);
                after(app);
            }
            Err(e) => app.error(&title, e),
        });
    }

    // ----- settings ------------------------------------------------------

    pub fn open_settings(&mut self) {
        let title = t("settings.title");
        if self.dialogs.iter().any(|d| matches!(&d.dialog, Dialog::Form(f) if f.title == title)) {
            return;
        }
        let s = &self.settings;
        let form = Form::new(&title, &t("dialog.save"))
            .intro(&t("settings.intro"))
            .field(Field::text("tenant", &t("settings.tenant"), &s.tenant_id).required())
            .field(Field::text("client", &t("settings.client"), &s.client_id).required())
            .field(Field::check("remember", &t("settings.remember"), s.remember_sign_in))
            .field(Field::multiline("scopes", &t("settings.scopes"), &s.scopes))
            .field(Field::check("updates", &t("settings.updates"), s.update_check))
            .field(Field::info(&t("settings.stored_in"), &crate::config::settings_path().display().to_string()))
            .on_submit(|app, f| {
                let client = f.text("client");
                if !is_guid(&client) {
                    return Err(t("settings.bad_client"));
                }
                let tenant = f.text("tenant");
                if tenant.contains(char::is_whitespace) || tenant.contains('/') {
                    return Err(t("settings.bad_tenant"));
                }
                let scopes = f.text("scopes").split_whitespace().collect::<Vec<_>>().join(" ");
                let new = Settings {
                    tenant_id: tenant,
                    client_id: client,
                    remember_sign_in: f.checked("remember"),
                    update_check: f.checked("updates"),
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
        let mut form = Form::new(&t("user.new_title"), &t("dialog.create"))
            .field(Field::text("displayName", &t("field.display_name"), "").required())
            .field(Field::text("alias", &t("field.user_name"), "").required());
        form = if domains.is_empty() {
            form.field(Field::text("domain", &t("field.domain"), ""))
        } else {
            form.field(Field::choice("domain", &t("field.domain"), domains, 0))
        };
        let form = form
            .field(Field::text("givenName", &t("field.first_name"), ""))
            .field(Field::text("surname", &t("field.last_name"), ""))
            .field(Field::text("jobTitle", &t("field.job_title"), ""))
            .field(Field::text("department", &t("field.department"), ""))
            .field(Field::text("usageLocation", &t("field.usage_location"), ""))
            .field(Field::password("password", &t("field.password"), &generate_password()))
            .field(Field::check("force", &t("field.force_change"), true))
            .field(Field::check("enabled", &t("field.account_enabled"), true))
            .intro(&t("user.new_intro"))
            .on_submit(|app, f| {
                let domain = match f.choice("domain") {
                    d if !d.is_empty() => d,
                    _ => f.text("domain"),
                };
                let alias = f.text("alias");
                if alias.contains('@') || alias.contains(' ') {
                    return Err(t("user.bad_alias"));
                }
                if domain.is_empty() {
                    return Err(tf("form.required", &[("field", &t("field.domain"))]));
                }
                let password = f.raw("password");
                if password.len() < 8 {
                    return Err(t("user.short_password"));
                }
                if !is_usage_location(&f.text("usageLocation")) {
                    return Err(t("user.bad_usage_location"));
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
                app.spawn(t("user.creating"), move |g| g.post("/users", &body), move |app, r| match r {
                    Ok(user) => {
                        let upn = s(&user, "userPrincipalName");
                        app.upsert(Node::Users, user);
                        app.push(Dialog::Message {
                            title: t("user.created_title"),
                            text: tf("user.created", &[("upn", &upn)]),
                            copy: Some(password),
                        });
                    }
                    Err(e) => app.error(&t("user.new_title"), e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    fn reset_password(&mut self, id: String, name: String) {
        let form = Form::new(&tf("password.title", &[("name", &name)]), &t("password.reset"))
            .field(Field::password("password", &t("field.new_password"), &generate_password()).required())
            .field(Field::check("force", &t("field.force_change"), true))
            .on_submit(move |app, f| {
                let password = f.raw("password");
                if password.len() < 8 {
                    return Err(t("user.short_password"));
                }
                let body = json!({ "passwordProfile": {
                    "password": password,
                    "forceChangePasswordNextSignIn": f.checked("force"),
                }});
                let path = format!("/users/{id}");
                let who = name.clone();
                app.spawn(t("password.resetting"), move |g| g.patch(&path, &body), move |app, r| match r {
                    Ok(()) => {
                        app.push(Dialog::Message {
                            title: t("password.done_title"),
                            text: tf("password.done", &[("name", &who)]),
                            copy: Some(password),
                        });
                    }
                    Err(e) => app.error(&t("password.reset_title"), e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    fn toggle_enabled(&mut self, node: Node, obj: Value) {
        let enable = obj["accountEnabled"] == false;
        let id = s(&obj, "id");
        let path = format!("/{}/{id}", collection(node));
        let name = node.object_name(&obj);
        let label = tf(if enable { "task.enable" } else { "task.disable" }, &[("name", &name)]);
        let run = move |app: &mut App| {
            app.mutate(&label, move |g| g.patch(&path, &json!({ "accountEnabled": enable })), move |app| {
                app.upsert(node, json!({ "id": id, "accountEnabled": enable }));
            });
        };
        if enable {
            run(self);
        } else {
            let (title, text) = match node {
                Node::Users => ("ops.disable_user_title", "ops.disable_user"),
                Node::Applications => ("ops.disable_application_title", "ops.disable_application"),
                _ => ("ops.disable_device_title", "ops.disable_device"),
            };
            self.push(Dialog::confirm(&t(title), &tf(text, &[("name", &name)]), run));
        }
    }

    fn delete_obj(&mut self, node: Node, id: String, name: String) {
        let (title, text) = match node {
            Node::Users => ("ops.delete_user_title", "ops.delete_user"),
            Node::Groups => ("ops.delete_group_title", "ops.delete_group"),
            Node::Devices => ("ops.delete_device_title", "ops.delete_device"),
            Node::Applications => ("ops.delete_application_title", "ops.delete_application"),
            _ => return,
        };
        let text = tf(text, &[("name", &name)]);
        self.push(Dialog::confirm(&t(title), &text, move |app| {
            let path = format!("/{}/{id}", collection(node));
            app.mutate(&tf("task.delete", &[("name", &name)]), move |g| g.delete(&path), move |app| {
                app.remove_obj(node, &id);
            });
        }));
    }

    // ----- groups --------------------------------------------------------

    fn new_group(&mut self) {
        let types = vec![
            ("security".to_string(), t("value.group_security")),
            ("m365".to_string(), t("value.group_m365")),
        ];
        let form = Form::new(&t("group.new_title"), &t("dialog.create"))
            .field(Field::choice("type", &t("field.group_type"), types, 0))
            .field(Field::text("displayName", &t("field.name"), "").required())
            .field(Field::text("mailNickname", &t("field.mail_nickname"), ""))
            .field(Field::multiline("description", &t("field.description"), ""))
            .field(Field::check("roles", &t("field.role_assignable"), false))
            .intro(&t("group.new_intro"))
            .on_submit(|app, f| {
                let m365 = f.choice("type") == "m365";
                let roles = f.checked("roles");
                let name = f.text("displayName");
                let mut nick = f.text("mailNickname");
                if nick.is_empty() {
                    nick = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                }
                if nick.is_empty() {
                    return Err(t("group.bad_nickname"));
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
                app.spawn(t("group.creating"), move |g| g.post("/groups", &body), |app, r| match r {
                    Ok(group) => {
                        app.status = tf("group.created", &[("name", &s(&group, "displayName"))]);
                        app.upsert(Node::Groups, group);
                    }
                    Err(e) => app.error(&t("group.new_title"), e),
                });
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    pub fn add_member(&mut self, group_id: &str, member_id: &str, after: impl FnOnce(&mut App) + Send + 'static) {
        let path = format!("/groups/{group_id}/members/$ref");
        let body = json!({ "@odata.id": format!("{}/v1.0/directoryObjects/{member_id}", self.settings.graph_endpoint) });
        self.mutate(&t("task.add_member"), move |g| g.post(&path, &body).map(|_| ()), after);
    }

    pub fn add_owner(&mut self, coll: &str, obj_id: &str, user_id: &str, after: impl FnOnce(&mut App) + Send + 'static) {
        let path = format!("/{coll}/{obj_id}/owners/$ref");
        let body = json!({ "@odata.id": format!("{}/v1.0/directoryObjects/{user_id}", self.settings.graph_endpoint) });
        self.mutate(&t("task.add_owner"), move |g| g.post(&path, &body).map(|_| ()), after);
    }

    pub fn assign_role(&mut self, principal_id: &str, role_id: &str, after: impl FnOnce(&mut App) + Send + 'static) {
        let body = json!({ "principalId": principal_id, "roleDefinitionId": role_id, "directoryScopeId": "/" });
        self.mutate(
            &t("task.assign_role"),
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
        let label = t(if add { "task.assign_licence" } else { "task.remove_licence" });
        self.mutate(&label, move |g| g.post(&path, &body).map(|_| ()), after);
    }

    /// Assigns a user or group to an application, asking which app role to give them if the
    /// application has more than one.
    pub fn assign_app(&mut self, sp: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let sp_id = s(&sp, "id");
        let name = s(&sp, "displayName");
        let roles = app_roles(&sp);
        // Default access is only for applications that define no app roles of their own.
        let own_roles = sp["appRoles"].as_array().is_some_and(|r| r.iter().any(|r| r["isEnabled"] != false));
        if roles.is_empty() && own_roles {
            self.push(Dialog::message(&t("ops.assign_app_title"), &tf("ops.no_user_roles", &[("name", &name)])));
            return;
        }
        self.pick_user_or_group(&tf("pick.assign_user_or_group_to", &[("name", &name)]), move |app, who| {
            let assign = move |app: &mut App, role_id: String| {
                let path = format!("/servicePrincipals/{sp_id}/appRoleAssignedTo");
                let body = json!({ "principalId": who.id, "resourceId": sp_id, "appRoleId": role_id });
                app.mutate(&t("task.assign_app"), move |g| g.post(&path, &body).map(|_| ()), after);
            };
            match roles.len() {
                0 => assign(app, DEFAULT_ACCESS_ROLE.into()),
                1 => assign(app, roles[0].id.clone()),
                _ => {
                    let title = tf("pick.role_for", &[("name", &who.label.trim_start_matches(['👤', '👥', ' ']))]);
                    let fetch = move |_: &Graph, _: &str| Ok(roles.clone());
                    app.push(Dialog::Picker(Picker::new(&title, &t("pick.role"), false, fetch, move |app, role| assign(app, role.id))));
                }
            }
        });
    }

    // ----- pickers -------------------------------------------------------

    pub fn pick_user(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        self.push(Dialog::Picker(Picker::new(title, &t("pick.name_or_upn"), true, search_users, on_pick)));
    }

    pub fn pick_group(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        self.push(Dialog::Picker(Picker::new(title, &t("pick.group_name"), true, search_groups, on_pick)));
    }

    pub fn pick_user_or_group(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        let fetch = |g: &Graph, q: &str| {
            let mut items = search_users(g, q)?;
            items.extend(search_groups(g, q)?);
            Ok(items)
        };
        self.push(Dialog::Picker(Picker::new(title, &t("pick.search"), true, fetch, on_pick)));
    }

    /// Users, groups and devices (anything that can be a group member).
    pub fn pick_principal(&mut self, title: &str, on_pick: impl FnOnce(&mut App, PickItem) + 'static) {
        let fetch = |g: &Graph, q: &str| {
            let mut items = search_users(g, q)?;
            items.extend(search_groups(g, q)?);
            items.extend(search_devices(g, q)?);
            Ok(items)
        };
        self.push(Dialog::Picker(Picker::new(title, &t("pick.search"), true, fetch, on_pick)));
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
        self.push(Dialog::Picker(Picker::new(title, &t("pick.role"), false, fetch, on_pick)));
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
                        detail: tf("pick.sku_detail", &[("sku", &part), ("available", &(total - used)), ("total", &total)]),
                    }
                })
                .collect();
            skus.sort_by_key(|s| s.label.to_lowercase());
            Ok(skus)
        };
        self.push(Dialog::Picker(Picker::new(title, &t("pick.licence"), false, fetch, on_pick)));
    }
}

/// The app role an assignment gets when the application defines none, giving plain access.
pub const DEFAULT_ACCESS_ROLE: &str = "00000000-0000-0000-0000-000000000000";

/// The enabled app roles of a service principal that users and groups can be assigned.
pub fn app_roles(sp: &Value) -> Vec<PickItem> {
    let mut roles: Vec<PickItem> = sp["appRoles"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|r| r["isEnabled"] != false && r["allowedMemberTypes"].as_array().is_some_and(|t| t.iter().any(|t| t == "User")))
        .map(|r| PickItem { id: s(r, "id"), label: s(r, "displayName"), detail: s(r, "description") })
        .collect();
    roles.sort_by_key(|r| r.label.to_lowercase());
    roles
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

/// Blank, or a two-letter country code such as GB.
pub fn is_usage_location(s: &str) -> bool {
    s.is_empty() || s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic())
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
    fn app_roles_for_users_only() {
        let sp = json!({ "appRoles": [
            { "id": "1", "displayName": "Writer", "isEnabled": true, "allowedMemberTypes": ["User"] },
            { "id": "2", "displayName": "Daemon", "isEnabled": true, "allowedMemberTypes": ["Application"] },
            { "id": "3", "displayName": "Old", "isEnabled": false, "allowedMemberTypes": ["User"] },
            { "id": "4", "displayName": "Admin", "isEnabled": true, "allowedMemberTypes": ["User", "Application"] },
        ]});
        let names: Vec<String> = app_roles(&sp).into_iter().map(|r| r.label).collect();
        assert_eq!(names, ["Admin", "Writer"]);
        assert!(app_roles(&json!({})).is_empty());
    }

    #[test]
    fn applications_leave_out_microsoft_and_managed_identities() {
        let sp = |kind: &str, owner: &str| json!({ "servicePrincipalType": kind, "appOwnerOrganizationId": owner });
        assert!(Node::Applications.keep(&sp("Application", "11111111-2222-3333-4444-555555555555")));
        assert!(!Node::Applications.keep(&sp("Application", "f8cdef31-a31e-4b4a-93e4-5f571e91255a")));
        assert!(!Node::Applications.keep(&sp("ManagedIdentity", "11111111-2222-3333-4444-555555555555")));
        assert!(Node::Users.keep(&json!({})));
    }

    #[test]
    fn admin_tools() {
        let app = |id: &str| json!({ "appId": id });
        let own = "AAAAAAAA-1111-2222-3333-444444444444";
        assert!(crate::model::is_admin_tool(&app("aaaaaaaa-1111-2222-3333-444444444444"), own));
        assert!(crate::model::is_admin_tool(&app("14d82eec-204b-4c2f-b7e8-296a70dab67e"), ""));
        assert!(!crate::model::is_admin_tool(&app("11111111-2222-3333-4444-555555555555"), own));
        assert!(!crate::model::is_admin_tool(&json!({}), ""));
    }

    #[test]
    fn usage_locations() {
        assert!(is_usage_location("") && is_usage_location("gb"));
        assert!(!is_usage_location("GBR") && !is_usage_location("G1"));
    }

    #[test]
    fn guid_validation() {
        assert!(is_guid("00000003-0000-0000-c000-000000000000"));
        assert!(!is_guid("contoso.onmicrosoft.com"));
    }
}
