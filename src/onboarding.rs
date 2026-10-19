//! The onboarding wizard: creates a user with their manager, licences and groups, then reports
//! how each step went.

use crate::app::{App, Bold};
use crate::dialogs::{Dialog, PickItem};
use crate::graph::{enc, Graph};
use crate::model::{group_type, s, sku_name, Node};
use crate::ops::generate_password;
use crate::workflow::{self, Loadable, Shared, State, Step};
use eframe::egui::{self, RichText};
use serde_json::{json, Value};
use std::time::Duration;

const PAGES: [&str; 5] = ["User", "Licences", "Groups", "Review", "Results"];
const REVIEW: usize = 3;
const RESULTS: usize = 4;

/// The group properties needed to decide whether a new user can and should be added.
pub const GROUP_SELECT: &str =
    "id,displayName,groupTypes,membershipRule,mailEnabled,securityEnabled,isAssignableToRole,onPremisesSyncEnabled,mail,assignedLicenses";

pub struct SkuChoice {
    pub id: String,
    pub name: String,
    pub part: String,
    pub available: i64,
    pub selected: bool,
}

pub struct GroupChoice {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Whether membership can be set here at all.
    pub allowed: bool,
    pub selected: bool,
    pub note: String,
}

/// Whether a new user can be added to a group here, and whether copying memberships should tick
/// it. Groups whose membership is set elsewhere can't; role-assignable groups grant directory
/// roles, so they are only added when ticked by hand.
pub fn classify_group(v: &Value) -> GroupChoice {
    let types = v["groupTypes"].as_array().cloned().unwrap_or_default();
    let unified = types.iter().any(|t| t == "Unified");
    let (allowed, preselect, mut note) = if types.iter().any(|t| t == "DynamicMembership") || !s(v, "membershipRule").is_empty() {
        (false, false, "Dynamic membership: set by its rule".to_string())
    } else if v["onPremisesSyncEnabled"] == true {
        (false, false, "Synced from Active Directory: change it there".into())
    } else if v["mailEnabled"] == true && !unified {
        (false, false, "Distribution list or mail-enabled security group: add the user in Exchange Online once they have a mailbox".into())
    } else if v["isAssignableToRole"] == true {
        (true, false, "Role-assignable: its members get directory roles, so it isn't ticked for you".into())
    } else {
        (true, true, String::new())
    };
    if v["assignedLicenses"].as_array().is_some_and(|a| !a.is_empty()) {
        note = if note.is_empty() { "Assigns licences".into() } else { format!("{note}. Assigns licences") };
    }
    GroupChoice { id: s(v, "id"), name: s(v, "displayName"), kind: group_type(v), allowed, selected: preselect, note }
}

/// Why a user name can't be used, if it can't. Entra allows letters, digits and `' . - _ ! # ^ ~`,
/// not starting or ending with a full stop.
pub fn alias_problem(alias: &str) -> Option<String> {
    if alias.is_empty() {
        return Some("Enter a user name.".into());
    }
    if let Some(c) = alias.chars().find(|c| !(c.is_ascii_alphanumeric() || "'.-_!#^~".contains(*c))) {
        return Some(format!("The user name can't contain '{c}'. Use letters, digits and . - _ only; choose the domain separately."));
    }
    if alias.starts_with('.') || alias.ends_with('.') || alias.contains("..") {
        return Some("The user name can't start or end with a full stop, or have two in a row.".into());
    }
    None
}

/// Why Microsoft Entra ID would refuse a password, if it would: it must be 8 to 256 characters
/// with three of upper case, lower case, digits and symbols.
pub fn password_problem(p: &str) -> Option<String> {
    let n = p.chars().count();
    let classes = [
        p.chars().any(|c| c.is_ascii_uppercase()),
        p.chars().any(|c| c.is_ascii_lowercase()),
        p.chars().any(|c| c.is_ascii_digit()),
        p.chars().any(|c| !c.is_ascii_alphanumeric()),
    ];
    if !(8..=256).contains(&n) {
        Some("The password must be 8 to 256 characters.".into())
    } else if classes.iter().filter(|c| **c).count() < 3 {
        Some("The password needs three of: upper case letters, lower case letters, digits and symbols.".into())
    } else {
        None
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum UpnCheck {
    Checking,
    Free,
    Taken,
    Unknown(String),
}

/// One thing onboarding will do.
#[derive(Debug, PartialEq)]
pub enum Planned {
    Create(Value),
    Manager { id: String, name: String },
    Licence { id: String, name: String },
    Group { id: String, name: String },
}

impl Planned {
    pub fn label(&self, upn: &str) -> String {
        match self {
            Planned::Create(_) => format!("Create the user {upn}"),
            Planned::Manager { name, .. } => format!("Set their manager to {name}"),
            Planned::Licence { name, .. } => format!("Assign the licence {name}"),
            Planned::Group { name, .. } => format!("Add them to the group {name}"),
        }
    }
}

/// What a running onboarding has made so far.
#[derive(Default)]
struct Ctx {
    user: Value,
}

pub struct Onboarding {
    page: usize,
    /// Set when a page is shown for the first time, to put the focus in it.
    page_new: bool,
    display_name: String,
    /// The display name is still the one made from the first and last names.
    auto_name: bool,
    given_name: String,
    surname: String,
    alias: String,
    domains: Vec<String>,
    domain: usize,
    domain_text: String,
    job_title: String,
    department: String,
    usage_location: String,
    manager: Option<PickItem>,
    password: String,
    show_password: bool,
    force_change: bool,
    skus: Loadable<Vec<SkuChoice>>,
    groups: Vec<GroupChoice>,
    busy: Option<String>,
    upn_check: Option<UpnCheck>,
    error: Option<String>,
    progress: Option<Shared>,
}

impl Onboarding {
    fn domain(&self) -> String {
        match self.domains.get(self.domain) {
            Some(d) => d.clone(),
            None => self.domain_text.trim().trim_start_matches('@').to_string(),
        }
    }

    fn upn(&self) -> String {
        format!("{}@{}", self.alias.trim(), self.domain())
    }

    fn selected_skus(&self) -> Vec<&SkuChoice> {
        match &self.skus {
            Loadable::Ready(skus) => skus.iter().filter(|s| s.selected).collect(),
            _ => Vec::new(),
        }
    }

    /// Why the User page can't be left yet, if it can't.
    fn user_problem(&self) -> Option<String> {
        let usage = self.usage_location.trim();
        if self.display_name.trim().is_empty() {
            Some("Enter a display name.".into())
        } else if let Some(p) = alias_problem(self.alias.trim()) {
            Some(p)
        } else if self.domain().is_empty() || self.domain().contains(['@', ' ']) {
            Some("Choose or enter the domain.".into())
        } else if !usage.is_empty() && (usage.len() != 2 || !usage.chars().all(|c| c.is_ascii_alphabetic())) {
            Some("The usage location is a two-letter country code, such as GB or US.".into())
        } else {
            password_problem(&self.password)
        }
    }

    fn body(&self) -> Value {
        let alias = self.alias.trim();
        let mut body = json!({
            "accountEnabled": true,
            "displayName": self.display_name.trim(),
            "mailNickname": alias.chars().filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c)).collect::<String>(),
            "userPrincipalName": self.upn(),
            "passwordProfile": { "password": self.password, "forceChangePasswordNextSignIn": self.force_change },
        });
        for (key, value) in [
            ("givenName", &self.given_name),
            ("surname", &self.surname),
            ("jobTitle", &self.job_title),
            ("department", &self.department),
        ] {
            if !value.trim().is_empty() {
                body[key] = json!(value.trim());
            }
        }
        if !self.usage_location.trim().is_empty() {
            body["usageLocation"] = json!(self.usage_location.trim().to_uppercase());
        }
        body
    }

    /// Everything onboarding will do, in order: the user first, as every other step needs it.
    pub fn plan(&self) -> Vec<Planned> {
        let mut plan = vec![Planned::Create(self.body())];
        if let Some(m) = &self.manager {
            plan.push(Planned::Manager { id: m.id.clone(), name: m.label.trim_start_matches(['👤', ' ']).to_string() });
        }
        plan.extend(self.selected_skus().into_iter().map(|s| Planned::Licence { id: s.id.clone(), name: s.name.clone() }));
        plan.extend(
            self.groups.iter().filter(|g| g.allowed && g.selected).map(|g| Planned::Group { id: g.id.clone(), name: g.name.clone() }),
        );
        plan
    }

    /// Things to point out before the user is created.
    fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        match &self.upn_check {
            Some(UpnCheck::Taken) => out.push(format!("A user or object named {} already exists. Go back and choose another user name.", self.upn())),
            Some(UpnCheck::Unknown(e)) => out.push(format!("Could not check whether {} is free: {e}", self.upn())),
            _ => {}
        }
        if self.selected_skus().is_empty() {
            out.push("No licence is assigned, so the user won't get a mailbox or Microsoft 365 apps until one is.".into());
        }
        if self.groups.iter().any(|g| g.selected && g.note.starts_with("Role-assignable")) {
            out.push("A role-assignable group is ticked: the user will get the directory roles assigned to it.".into());
        }
        if !self.force_change {
            out.push("The user won't be asked to change the initial password when they first sign in.".into());
        }
        out
    }

    fn running(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| !p.lock().unwrap().finished)
    }
}

/// Retries `f` while it fails because an object it uses can't be found yet: a user just created
/// can take a few seconds to reach every Microsoft Entra ID replica.
fn retry_while_new<T>(mut f: impl FnMut() -> anyhow::Result<T>) -> anyhow::Result<T> {
    for wait in [2, 4, 8] {
        match f() {
            Err(e) if e.to_string().contains("(404)") || e.to_string().contains("Request_ResourceNotFound") => {
                log::info!("Not found yet, retrying in {wait}s: {e}");
                std::thread::sleep(Duration::from_secs(wait));
            }
            r => return r,
        }
    }
    f()
}

fn step(planned: Planned, upn: &str) -> Step<Graph, Ctx> {
    let label = planned.label(upn);
    let needs = if matches!(planned, Planned::Create(_)) { vec![] } else { vec![0] };
    Step::new(label, needs, move |g: &Graph, ctx: &mut Ctx| {
        let user_id = s(&ctx.user, "id");
        match planned {
            Planned::Create(body) => {
                ctx.user = g.post("/users", &body)?;
                Ok(format!("Created with object ID {}", s(&ctx.user, "id")))
            }
            Planned::Manager { id, .. } => {
                let body = json!({ "@odata.id": g.object_url("users", &id) });
                retry_while_new(|| g.put(&format!("/users/{user_id}/manager/$ref"), &body))?;
                Ok("Manager set".into())
            }
            Planned::Licence { id, .. } => {
                let body = json!({ "addLicenses": [{ "skuId": id, "disabledPlans": [] }], "removeLicenses": [] });
                retry_while_new(|| g.post(&format!("/users/{user_id}/assignLicense"), &body))?;
                Ok("Licence assigned".into())
            }
            Planned::Group { id, .. } => {
                let body = json!({ "@odata.id": g.object_url("directoryObjects", &user_id) });
                match retry_while_new(|| g.post(&format!("/groups/{id}/members/$ref"), &body)) {
                    Ok(_) => Ok("Added".into()),
                    Err(e) if e.to_string().contains("already exist") => Ok("Already a member".into()),
                    Err(e) => Err(e),
                }
            }
        }
    })
}

impl App {
    pub fn onboard_user(&mut self) {
        let wizard = Onboarding {
            page: 0,
            page_new: true,
            display_name: String::new(),
            auto_name: true,
            given_name: String::new(),
            surname: String::new(),
            alias: String::new(),
            domains: self.domains.clone(),
            domain: 0,
            domain_text: String::new(),
            job_title: String::new(),
            department: String::new(),
            usage_location: String::new(),
            manager: None,
            password: generate_password(),
            show_password: false,
            force_change: true,
            skus: Loadable::Loading,
            groups: Vec::new(),
            busy: None,
            upn_check: None,
            error: None,
            progress: None,
        };
        let id = self.push(Dialog::Onboard(Box::new(wizard)));
        self.spawn(
            "Loading licences…",
            |g| g.list("/subscribedSkus"),
            move |app, r| {
                let Some(Dialog::Onboard(w)) = app.dialog_mut(id) else { return };
                w.skus = match r {
                    Ok(list) => {
                        let mut skus: Vec<SkuChoice> = list
                            .iter()
                            .filter(|v| v["capabilityStatus"] == "Enabled" && v["appliesTo"] == "User")
                            .map(|v| {
                                let part = s(v, "skuPartNumber");
                                let total = v["prepaidUnits"]["enabled"].as_i64().unwrap_or(0);
                                SkuChoice {
                                    id: s(v, "skuId"),
                                    name: sku_name(&part),
                                    part,
                                    available: total - v["consumedUnits"].as_i64().unwrap_or(0),
                                    selected: false,
                                }
                            })
                            .collect();
                        skus.sort_by_key(|s| s.name.to_lowercase());
                        Loadable::Ready(skus)
                    }
                    Err(e) => Loadable::Failed(e.to_string()),
                };
            },
        );
    }

    /// Adds groups (as read with [`GROUP_SELECT`]) to the wizard, leaving ones already there alone.
    fn add_onboarding_groups(&mut self, id: u64, found: Vec<Value>) {
        let Some(Dialog::Onboard(w)) = self.dialog_mut(id) else { return };
        for v in found {
            let choice = classify_group(&v);
            if !w.groups.iter().any(|g| g.id == choice.id) {
                w.groups.push(choice);
            }
        }
        w.groups.sort_by_key(|g| (!g.allowed, g.name.to_lowercase()));
    }

    fn onboarding_pick_group(&mut self, id: u64) {
        self.pick_group("Add the new user to a group", move |app, g| {
            let path = format!("/groups/{}?$select={GROUP_SELECT}", g.id);
            app.spawn("Reading group…", move |gr| gr.get(&path), move |app, r| match r {
                Ok(v) => {
                    app.add_onboarding_groups(id, vec![v.clone()]);
                    // Ticked by hand, so tick it even if copying wouldn't, as long as it can be set.
                    if let Some(Dialog::Onboard(w)) = app.dialog_mut(id)
                        && let Some(c) = w.groups.iter_mut().find(|c| c.id == s(&v, "id"))
                    {
                        c.selected = c.allowed;
                    }
                }
                Err(e) => app.error("Add group", e),
            });
        });
    }

    fn onboarding_copy_groups(&mut self, id: u64) {
        self.pick_user("Copy group memberships from", move |app, u| {
            let name = u.label.trim_start_matches(['👤', ' ']).to_string();
            let path = format!("/users/{}/memberOf/microsoft.graph.group?$select={GROUP_SELECT}&$top=999", u.id);
            app.defer(move |app| {
                if let Some(Dialog::Onboard(w)) = app.dialog_mut(id) {
                    w.busy = Some(format!("Reading {name}'s groups…"));
                }
                app.spawn("Reading groups…", move |g| g.list(&path), move |app, r| {
                    if let Some(Dialog::Onboard(w)) = app.dialog_mut(id) {
                        w.busy = None;
                    }
                    match r {
                        Ok(found) => {
                            let n = found.len();
                            app.add_onboarding_groups(id, found);
                            app.status = format!("Copied {n} group memberships from {name}");
                        }
                        Err(e) => app.error("Copy group memberships", e),
                    }
                });
            });
        });
    }

    fn onboarding_check_upn(&mut self, id: u64, upn: String) {
        let path = format!("/users/{}?$select=id", enc(&upn));
        self.spawn("Checking the user name…", move |g| Ok(g.get(&path)), move |app, r: anyhow::Result<anyhow::Result<Value>>| {
            let Some(Dialog::Onboard(w)) = app.dialog_mut(id) else { return };
            if w.upn() != upn {
                return;
            }
            w.upn_check = Some(match r.and_then(|x| x) {
                Ok(_) => UpnCheck::Taken,
                Err(e) if e.to_string().contains("(404)") => UpnCheck::Free,
                Err(e) => UpnCheck::Unknown(e.to_string()),
            });
        });
    }

    fn start_onboarding(&mut self, w: &mut Onboarding) {
        let upn = w.upn();
        let steps: Vec<Step<Graph, Ctx>> = w.plan().into_iter().map(|p| step(p, &upn)).collect();
        let progress = self.run_workflow(&format!("Onboard {upn}"), Ctx::default(), steps, |app, ctx, _| {
            if ctx.user.is_object() {
                app.upsert(Node::Users, ctx.user);
            }
        });
        w.progress = Some(progress);
        w.page = RESULTS;
        w.page_new = true;
    }

    pub(crate) fn show_onboarding(&mut self, ctx: &egui::Context, modal: egui::Modal, id: u64, w: &mut Onboarding, escape: bool) -> bool {
        let mut close = escape && !w.running();
        let mut next = false;
        let mut back = false;
        let first = std::mem::take(&mut w.page_new);
        let mut pick_manager = false;
        let mut pick_group = false;
        let mut copy_groups = false;
        modal.show(ctx, |ui| {
            ui.set_width(720.0);
            workflow::page_heading(ui, "Onboard a new user", &PAGES, w.page);
            egui::ScrollArea::vertical().max_height(400.0).auto_shrink([false, true]).show(ui, |ui| match w.page {
                0 => {
                    egui::Grid::new(("onboard-user", id)).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        let text = |ui: &mut egui::Ui, label: &str, value: &mut String, focus: bool| {
                            let l = ui.label(label);
                            let r = ui.add(egui::TextEdit::singleline(value).desired_width(360.0)).labelled_by(l.id);
                            if focus {
                                r.request_focus();
                            }
                            ui.end_row();
                            r.changed()
                        };
                        let names = text(ui, "First name", &mut w.given_name, first) | text(ui, "Last name", &mut w.surname, false);
                        if names && w.auto_name {
                            w.display_name = format!("{} {}", w.given_name.trim(), w.surname.trim()).trim().to_string();
                        }
                        if text(ui, "Display name*", &mut w.display_name, false) {
                            w.auto_name = false;
                        }
                        let l = ui.label("User name*");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut w.alias).desired_width(170.0)).labelled_by(l.id);
                            if w.domains.is_empty() {
                                ui.label("@");
                                ui.add(egui::TextEdit::singleline(&mut w.domain_text).desired_width(170.0).hint_text("contoso.com"));
                            } else {
                                egui::ComboBox::from_id_salt(("onboard-domain", id))
                                    .selected_text(format!("@{}", w.domain()))
                                    .width(170.0)
                                    .show_ui(ui, |ui| {
                                        for (i, d) in w.domains.iter().enumerate() {
                                            ui.selectable_value(&mut w.domain, i, format!("@{d}"));
                                        }
                                    });
                            }
                        });
                        ui.end_row();
                        text(ui, "Job title", &mut w.job_title, false);
                        text(ui, "Department", &mut w.department, false);
                        text(ui, "Usage location", &mut w.usage_location, false);
                        ui.label("");
                        ui.label(RichText::new("A two-letter country code such as GB, needed before licences can be assigned.").small().weak());
                        ui.end_row();
                        ui.label("Manager");
                        ui.horizontal(|ui| {
                            match &w.manager {
                                Some(m) => ui.label(format!("{}  ({})", m.label.trim_start_matches(['👤', ' ']), m.detail)),
                                None => ui.label(RichText::new("None").weak()),
                            };
                            pick_manager = ui.button("Choose…").clicked();
                            if w.manager.is_some() && ui.button("Clear").clicked() {
                                w.manager = None;
                            }
                        });
                        ui.end_row();
                        let l = ui.label("Initial password*");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut w.password).password(!w.show_password).desired_width(200.0)).labelled_by(l.id);
                            ui.checkbox(&mut w.show_password, "Show");
                            if ui.button("Generate").on_hover_text("Make a new random password").clicked() {
                                w.password = generate_password();
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut w.force_change, "Require a password change at first sign-in");
                        ui.end_row();
                    });
                }
                1 => match &mut w.skus {
                    Loadable::Loading => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading licences…");
                        });
                    }
                    Loadable::Failed(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, format!("⚠ Could not load the licences: {e}"));
                        ui.label("You can go on without assigning any, and assign them later.");
                    }
                    Loadable::Ready(skus) if skus.is_empty() => {
                        ui.label("The tenant has no licences for users.");
                    }
                    Loadable::Ready(skus) => {
                        ui.label("Tick the licences to assign. Each is assigned with all its services.");
                        ui.add_space(4.0);
                        for (i, sku) in skus.iter_mut().enumerate() {
                            let label = format!("{}  —  {} · {} available", sku.name, sku.part, sku.available.max(0));
                            let r = ui.add_enabled(sku.available > 0 || sku.selected, egui::Checkbox::new(&mut sku.selected, label));
                            if first && i == 0 {
                                r.request_focus();
                            }
                        }
                    }
                },
                2 => {
                    ui.label(
                        "Choose the groups to add the user to, or copy them from a user in a similar role. Groups whose \
                         membership is set elsewhere are listed but can't be ticked.",
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let add = ui.button("Add group…");
                        if first {
                            add.request_focus();
                        }
                        pick_group = add.clicked();
                        copy_groups = ui.button("Copy from user…").clicked();
                        if let Some(b) = &w.busy {
                            ui.spinner();
                            ui.label(b.as_str());
                        }
                    });
                    ui.add_space(4.0);
                    if w.groups.is_empty() {
                        ui.label(RichText::new("No groups chosen.").weak());
                    }
                    for g in &mut w.groups {
                        ui.horizontal(|ui| {
                            ui.add_enabled(g.allowed, egui::Checkbox::new(&mut g.selected, format!("{}  ({})", g.name, g.kind)));
                            if !g.note.is_empty() {
                                ui.label(RichText::new(g.note.as_str()).small().weak());
                            }
                        });
                    }
                }
                REVIEW => {
                    egui::Grid::new(("onboard-review", id)).num_columns(2).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                        let usage = w.usage_location.trim().to_uppercase();
                        let manager = w.manager.as_ref().map(|m| m.label.trim_start_matches(['👤', ' ']).to_string()).unwrap_or_default();
                        let rows = [
                            ("Display name", w.display_name.trim().to_string()),
                            ("User principal name", w.upn()),
                            ("Job title", w.job_title.trim().to_string()),
                            ("Department", w.department.trim().to_string()),
                            ("Usage location", usage),
                            ("Manager", manager),
                            ("Password change at first sign-in", if w.force_change { "Required".into() } else { "Not required".into() }),
                        ];
                        for (k, v) in rows {
                            ui.label(RichText::new(k).bold());
                            ui.label(if v.is_empty() { "—".to_string() } else { v });
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("These steps will run, in this order:").bold());
                    let upn = w.upn();
                    for (i, p) in w.plan().iter().enumerate() {
                        ui.label(format!("{}. {}", i + 1, p.label(&upn)));
                    }
                    ui.add_space(6.0);
                    match &w.upn_check {
                        Some(UpnCheck::Checking) | None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Checking the user name is free…");
                            });
                        }
                        Some(UpnCheck::Free) => {
                            ui.label(format!("✔ {upn} is free."));
                        }
                        _ => {}
                    }
                    for warning in w.warnings() {
                        ui.colored_label(ui.visuals().warn_fg_color, format!("⚠ {warning}"));
                    }
                }
                _ => {
                    if let Some(p) = &w.progress {
                        let p = p.lock().unwrap();
                        workflow::progress_ui(ui, id, &p);
                        if p.finished && matches!(p.steps.first().map(|s| &s.state), Some(State::Done(_))) {
                            ui.add_space(6.0);
                            ui.label(format!("Give {} this initial password in a secure way. It isn't saved in the report.", w.upn()));
                            ui.horizontal(|ui| {
                                let mut shown = w.password.clone();
                                ui.add(egui::TextEdit::singleline(&mut shown).font(egui::TextStyle::Monospace).desired_width(240.0));
                                if ui.button("Copy password").clicked() {
                                    ui.ctx().copy_text(w.password.clone());
                                }
                            });
                        }
                    }
                }
            });
            if let Some(e) = &w.error {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {e}"));
            }
            ui.separator();
            if w.page == RESULTS {
                if let Some(p) = w.progress.clone() {
                    close |= workflow::report_buttons(self, ui, &format!("Onboarding {}", w.upn()), "onboarding report.csv", &p);
                }
                return;
            }
            ui.horizontal(|ui| {
                if w.page > 0 && ui.button("  Back  ").clicked() {
                    back = true;
                }
                let blocked = w.page == REVIEW && !matches!(w.upn_check, Some(UpnCheck::Free | UpnCheck::Unknown(_)));
                let label = if w.page == REVIEW { "  Create user  " } else { "  Next  " };
                if ui.add_enabled(!blocked, egui::Button::new(label)).clicked() {
                    next = true;
                }
                close |= ui.button("Cancel").clicked();
            });
        });
        if back {
            w.page -= 1;
            w.page_new = true;
            w.error = None;
        } else if next && w.page < RESULTS {
            w.error = match w.page {
                0 => w.user_problem(),
                1 if !w.selected_skus().is_empty() && w.usage_location.trim().is_empty() => {
                    Some("Licences can only be assigned to a user with a usage location. Go back and enter one.".into())
                }
                _ => None,
            };
            if w.error.is_none() {
                if w.page == REVIEW {
                    if !matches!(w.upn_check, Some(UpnCheck::Taken | UpnCheck::Checking) | None) {
                        self.start_onboarding(w);
                    }
                } else {
                    w.page += 1;
                    w.page_new = true;
                    if w.page == REVIEW {
                        w.upn_check = Some(UpnCheck::Checking);
                        self.onboarding_check_upn(id, w.upn());
                    }
                }
            }
        }
        if pick_manager {
            self.pick_user("Manager of the new user", move |app, u| {
                app.defer(move |app| {
                    if let Some(Dialog::Onboard(w)) = app.dialog_mut(id) {
                        w.manager = Some(u);
                    }
                });
            });
        }
        if pick_group {
            self.onboarding_pick_group(id);
        }
        if copy_groups {
            self.onboarding_copy_groups(id);
        }
        !close
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_set_elsewhere_cannot_be_copied() {
        let dynamic = classify_group(&json!({ "id": "1", "groupTypes": ["DynamicMembership"], "membershipRule": "x" }));
        let synced = classify_group(&json!({ "id": "2", "groupTypes": [], "onPremisesSyncEnabled": true }));
        let list = classify_group(&json!({ "id": "3", "groupTypes": [], "mailEnabled": true, "securityEnabled": false }));
        let roles = classify_group(&json!({ "id": "4", "groupTypes": [], "securityEnabled": true, "isAssignableToRole": true }));
        let m365 = classify_group(&json!({ "id": "5", "groupTypes": ["Unified"], "mailEnabled": true, "assignedLicenses": [{ "skuId": "x" }] }));
        assert!(!dynamic.allowed && !synced.allowed && !list.allowed);
        assert!(roles.allowed && !roles.selected, "role-assignable groups must not be ticked automatically");
        assert!(m365.allowed && m365.selected);
        assert_eq!(m365.note, "Assigns licences");
    }

    #[test]
    fn checks_names_and_passwords() {
        assert_eq!(alias_problem("ann.lee"), None);
        assert!(alias_problem("ann lee").is_some());
        assert!(alias_problem("ann@contoso.com").is_some());
        assert!(alias_problem(".ann").is_some());
        assert!(password_problem("short1A").is_some());
        assert!(password_problem("alllowercase").is_some());
        assert_eq!(password_problem("Lower-and-1"), None);
        for _ in 0..50 {
            assert_eq!(password_problem(&generate_password()), None);
        }
    }

    fn wizard() -> Onboarding {
        Onboarding {
            page: 0,
            page_new: false,
            display_name: "Ann Lee".into(),
            auto_name: false,
            given_name: "Ann".into(),
            surname: "Lee".into(),
            alias: "ann".into(),
            domains: vec!["contoso.com".into()],
            domain: 0,
            domain_text: String::new(),
            job_title: String::new(),
            department: "Sales".into(),
            usage_location: "gb".into(),
            manager: Some(PickItem { id: "m1".into(), label: "👤 Bo Chan".into(), detail: "bo@contoso.com".into() }),
            password: "Lower-and-1".into(),
            show_password: false,
            force_change: true,
            skus: Loadable::Ready(vec![
                SkuChoice { id: "s1".into(), name: "E3".into(), part: "SPE_E3".into(), available: 2, selected: true },
                SkuChoice { id: "s2".into(), name: "E5".into(), part: "SPE_E5".into(), available: 2, selected: false },
            ]),
            groups: vec![
                classify_group(&json!({ "id": "g1", "displayName": "Sales", "groupTypes": [] })),
                classify_group(&json!({ "id": "g2", "displayName": "All", "groupTypes": ["DynamicMembership"], "membershipRule": "x" })),
            ],
            busy: None,
            upn_check: None,
            error: None,
            progress: None,
        }
    }

    #[test]
    fn plans_the_user_first_then_what_was_chosen() {
        let w = wizard();
        assert_eq!(w.user_problem(), None);
        let plan = w.plan();
        let Planned::Create(body) = &plan[0] else { panic!("the user must be created first") };
        assert_eq!(body["userPrincipalName"], "ann@contoso.com");
        assert_eq!(body["usageLocation"], "GB");
        assert_eq!(body["passwordProfile"]["forceChangePasswordNextSignIn"], true);
        assert_eq!(
            plan[1..],
            [
                Planned::Manager { id: "m1".into(), name: "Bo Chan".into() },
                Planned::Licence { id: "s1".into(), name: "E3".into() },
                Planned::Group { id: "g1".into(), name: "Sales".into() },
            ]
        );
        let steps: Vec<Step<Graph, Ctx>> = plan.into_iter().map(|p| step(p, "ann@contoso.com")).collect();
        assert!(steps[0].needs.is_empty() && steps[1..].iter().all(|s| s.needs == [0]));
    }

    #[test]
    fn warns_before_creating() {
        let mut w = wizard();
        w.upn_check = Some(UpnCheck::Taken);
        w.force_change = false;
        let warnings = w.warnings();
        assert!(warnings[0].contains("already exists"));
        assert!(warnings.iter().any(|x| x.contains("won't be asked to change")));
        w.usage_location = "GBR".into();
        assert!(w.user_problem().unwrap().contains("two-letter"));
    }
}
