//! The offboarding wizard: takes away a leaver's access step by step, in an order that keeps
//! their mailbox safe, after showing every change it will make.
//!
//! The order is: block sign-in, revoke sessions, remove directory roles, convert the mailbox and
//! set forwarding, remove groups, remove licences, and last delete the account. Removing a licence
//! (directly, or by leaving a group that assigns one) waits for the mailbox steps to succeed, as
//! a mailbox without a licence is deleted after 30 days. Deleting the account waits for
//! everything else, and can't be combined with keeping the mailbox, which belongs to the account.

use crate::app::{App, Bold};
use crate::auth::NeedsConsent;
use crate::dialogs::Dialog;
use crate::graph::{api_error, ApiError, Graph};
use crate::i18n::{t, tf};
use crate::model::{group_type, s, sku_name, Node};
use crate::onboarding::{assigns_licences, managed, Managed, GROUP_SELECT};
use crate::security::{explain, join, Need};
use crate::workflow::{self, Loadable, Shared, Step};
use eframe::egui::{self, RichText};
use serde_json::{json, Value};
use std::collections::HashMap;

const PAGES: [&str; 3] = ["offboard.page_choose", "offboard.page_preview", "wizard.page_results"];
const PREVIEW: usize = 1;
const RESULTS: usize = 2;

/// The user's mailbox, as far as could be found out.
pub enum Mailbox {
    None,
    /// A mailbox, with its `RecipientTypeDetails` and identity for cmdlets.
    Found { kind: String, identity: String },
    Unavailable(String),
}

pub struct RoleItem {
    pub name: String,
    /// The active assignment's ID, or None for an eligible (PIM) assignment.
    pub assignment_id: Option<String>,
    pub role_id: String,
    pub scope: String,
    /// For an eligible assignment, whether the role is activated now.
    pub active: bool,
    pub selected: bool,
}

pub struct GroupItem {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Mail-enabled security groups and distribution lists are changed in Exchange Online, by
    /// this identity.
    pub exchange: Option<String>,
    pub removable: bool,
    pub assigns_licences: bool,
    pub note: String,
    pub selected: bool,
}

pub struct LicenceItem {
    pub sku_id: String,
    pub name: String,
    /// Names of the groups it is inherited from; empty when it is assigned directly.
    pub from_groups: Vec<String>,
    pub direct: bool,
    pub selected: bool,
}

/// What was read about the user before choosing.
pub struct Found {
    pub user: Value,
    pub mailbox: Mailbox,
    pub roles: Vec<RoleItem>,
    pub groups: Vec<GroupItem>,
    pub licences: Vec<LicenceItem>,
    /// Parts that couldn't be read, to point out on the preview.
    pub problems: Vec<String>,
}

pub struct Offboarding {
    page: usize,
    name: String,
    upn: String,
    found: Loadable<Found>,
    disable: bool,
    revoke: bool,
    convert: bool,
    forward: bool,
    forward_to: String,
    keep_copy: bool,
    delete: bool,
    confirmed: bool,
    error: Option<String>,
    progress: Option<Shared>,
}

/// One thing offboarding will do.
#[derive(Clone, Debug, PartialEq)]
pub enum Planned {
    Disable,
    Revoke,
    RemoveRole { assignment_id: String, name: String },
    RemoveEligibleRole { role_id: String, scope: String, name: String, active: bool },
    Convert { identity: String },
    Forward { identity: String, to: String, keep_copy: bool },
    RemoveGroup { id: String, name: String, exchange: Option<String>, assigns_licences: bool },
    RemoveLicence { sku_id: String, name: String },
    Delete,
}

impl Planned {
    pub fn label(&self) -> String {
        match self {
            Planned::Disable => t("offboard.disable"),
            Planned::Revoke => t("reregister.revoke_step"),
            Planned::RemoveRole { name, .. } => tf("offboard.step_role", &[("name", name)]),
            Planned::RemoveEligibleRole { name, active: false, .. } => tf("offboard.step_eligible", &[("name", name)]),
            Planned::RemoveEligibleRole { name, active: true, .. } => tf("offboard.step_eligible_active", &[("name", name)]),
            Planned::Convert { .. } => t("offboard.step_convert"),
            Planned::Forward { to, keep_copy, .. } => {
                tf(if *keep_copy { "offboard.step_forward_copy" } else { "offboard.step_forward" }, &[("address", to)])
            }
            Planned::RemoveGroup { name, .. } => tf("offboard.step_group", &[("name", name)]),
            Planned::RemoveLicence { name, .. } => tf("offboard.step_licence", &[("name", name)]),
            Planned::Delete => t("offboard.step_delete"),
        }
    }

    fn is_mailbox_step(&self) -> bool {
        matches!(self, Planned::Convert { .. } | Planned::Forward { .. })
    }

    /// Whether the step takes a licence away, which must wait for the mailbox steps.
    fn removes_licence(&self) -> bool {
        matches!(self, Planned::RemoveLicence { .. } | Planned::RemoveGroup { assigns_licences: true, .. })
    }
}

/// Orders `chosen` safely and works out what each step needs (by index) to have succeeded first.
pub fn order(mut chosen: Vec<Planned>) -> Vec<(Planned, Vec<usize>)> {
    let rank = |p: &Planned| match p {
        Planned::Disable => 0,
        Planned::Revoke => 1,
        Planned::RemoveRole { .. } | Planned::RemoveEligibleRole { .. } => 2,
        Planned::Convert { .. } => 3,
        Planned::Forward { .. } => 4,
        Planned::RemoveGroup { .. } => 5,
        Planned::RemoveLicence { .. } => 6,
        Planned::Delete => 7,
    };
    chosen.sort_by_key(rank);
    let mailbox: Vec<usize> = chosen.iter().enumerate().filter(|(_, p)| p.is_mailbox_step()).map(|(i, _)| i).collect();
    chosen
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let needs = if p == Planned::Delete {
                (0..i).collect()
            } else if p.removes_licence() {
                mailbox.clone()
            } else {
                Vec::new()
            };
            (p, needs)
        })
        .collect()
}

impl Offboarding {
    fn found(&self) -> Option<&Found> {
        match &self.found {
            Loadable::Ready(f) => Some(f),
            _ => None,
        }
    }

    fn mailbox_kind(&self) -> Option<&str> {
        match self.found().map(|f| &f.mailbox) {
            Some(Mailbox::Found { kind, .. }) => Some(kind),
            _ => None,
        }
    }

    fn identity(&self) -> String {
        match self.found().map(|f| &f.mailbox) {
            Some(Mailbox::Found { identity, .. }) => identity.clone(),
            _ => self.upn.clone(),
        }
    }

    /// Everything chosen, in the order it will run.
    pub fn plan(&self) -> Vec<(Planned, Vec<usize>)> {
        let Some(f) = self.found() else { return Vec::new() };
        let mut chosen = Vec::new();
        if self.disable {
            chosen.push(Planned::Disable);
        }
        if self.revoke {
            chosen.push(Planned::Revoke);
        }
        for r in f.roles.iter().filter(|r| r.selected) {
            chosen.push(match &r.assignment_id {
                Some(id) => Planned::RemoveRole { assignment_id: id.clone(), name: r.name.clone() },
                None => Planned::RemoveEligibleRole {
                    role_id: r.role_id.clone(),
                    scope: r.scope.clone(),
                    name: r.name.clone(),
                    active: r.active,
                },
            });
        }
        let user_mailbox = self.mailbox_kind() == Some("UserMailbox");
        if self.convert && user_mailbox {
            chosen.push(Planned::Convert { identity: self.identity() });
        }
        if self.forward && self.mailbox_kind().is_some() {
            chosen.push(Planned::Forward { identity: self.identity(), to: self.forward_to.trim().to_string(), keep_copy: self.keep_copy });
        }
        for g in f.groups.iter().filter(|g| g.removable && g.selected) {
            chosen.push(Planned::RemoveGroup {
                id: g.id.clone(),
                name: g.name.clone(),
                exchange: g.exchange.clone(),
                assigns_licences: g.assigns_licences,
            });
        }
        for l in f.licences.iter().filter(|l| l.direct && l.selected) {
            chosen.push(Planned::RemoveLicence { sku_id: l.sku_id.clone(), name: l.name.clone() });
        }
        if self.delete {
            chosen.push(Planned::Delete);
        }
        order(chosen)
    }

    /// Why the choices can't be run, if they can't.
    fn problem(&self) -> Option<String> {
        if self.found().is_none() {
            return Some(t("offboard.wait"));
        }
        if self.delete && (self.convert || self.forward) {
            return Some(t("offboard.delete_keeps_mailbox"));
        }
        if self.forward {
            let to = self.forward_to.trim();
            if to.is_empty() || !to.contains('@') || to.contains(char::is_whitespace) {
                return Some(t("offboard.bad_forward"));
            }
            if to.eq_ignore_ascii_case(&self.upn) {
                return Some(t("offboard.forward_to_self"));
            }
        }
        if self.plan().is_empty() {
            return Some(t("offboard.nothing_chosen"));
        }
        None
    }

    /// Things to point out on the preview.
    fn warnings(&self) -> Vec<String> {
        let Some(f) = self.found() else { return Vec::new() };
        let plan: Vec<Planned> = self.plan().into_iter().map(|(p, _)| p).collect();
        let mut out = f.problems.clone();
        if f.user["onPremisesSyncEnabled"] == true {
            out.push(t("offboard.warn_synced"));
        }
        let removes_licence = plan.iter().any(Planned::removes_licence);
        if self.mailbox_kind() == Some("UserMailbox") && removes_licence && !self.convert {
            out.push(t("offboard.warn_not_converted"));
        }
        if matches!(f.mailbox, Mailbox::Unavailable(_)) && removes_licence {
            out.push(t("offboard.warn_mailbox_unchecked"));
        }
        if plan.iter().any(|p| matches!(p, Planned::Convert { .. })) {
            out.push(t("offboard.warn_large_mailbox"));
            if removes_licence {
                out.push(t("offboard.warn_licences_wait"));
            }
        }
        if self.revoke {
            out.push(t("offboard.warn_revoke"));
        }
        for g in f.groups.iter().filter(|g| !g.removable) {
            out.push(tf("offboard.warn_stays_in_group", &[("group", &g.name), ("note", &g.note)]));
        }
        let leaving: Vec<&str> = f.groups.iter().filter(|g| g.removable && g.selected).map(|g| g.name.as_str()).collect();
        for l in f.licences.iter().filter(|l| !l.direct) {
            if !l.from_groups.iter().all(|g| leaving.contains(&g.as_str())) {
                out.push(tf("offboard.warn_licence_stays", &[("licence", &l.name), ("groups", &l.from_groups.join(", "))]));
            }
        }
        if self.delete {
            out.push(t("offboard.warn_delete"));
        }
        if f.user["userType"] == "Guest" && self.revoke {
            out.push(t("offboard.warn_guest"));
        }
        out
    }

    fn running(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| !p.lock().unwrap().finished)
    }
}

/// What the steps of a running offboarding did, to update the lists afterwards.
#[derive(Default)]
struct Ctx {
    disabled: bool,
    converted: bool,
    deleted: bool,
}

fn step(planned: Planned, needs: Vec<usize>, user_id: String, upn: String) -> Step<Graph, Ctx> {
    Step::new(planned.label(), needs, move |g: &Graph, ctx: &mut Ctx| match planned {
        Planned::Disable => {
            g.patch(&format!("/users/{user_id}"), &json!({ "accountEnabled": false })).map_err(|e| explain(e, Need::Users))?;
            ctx.disabled = true;
            Ok(t("offboard.blocked"))
        }
        Planned::Revoke => {
            g.post(&format!("/users/{user_id}/revokeSignInSessions"), &json!({})).map_err(|e| explain(e, Need::Users))?;
            Ok(t("offboard.revoked"))
        }
        Planned::RemoveRole { assignment_id, .. } => {
            g.delete(&format!("/roleManagement/directory/roleAssignments/{assignment_id}"))?;
            Ok(t("offboard.removed"))
        }
        Planned::RemoveEligibleRole { role_id, scope, active, .. } => {
            let body = json!({
                "action": "adminRemove",
                "principalId": user_id,
                "roleDefinitionId": role_id,
                "directoryScopeId": scope,
                "justification": "Offboarding",
            });
            let r = g.post("/roleManagement/directory/roleEligibilityScheduleRequests", &body)?;
            let mut done = tf("offboard.request", &[("status", &s(&r, "status"))]);
            // An activation can't be deleted as a plain role assignment; PIM removes it, unless
            // removing the eligibility has ended it already.
            if active && role_assigned(g, &user_id, &role_id, &scope)? {
                let r = g.post("/roleManagement/directory/roleAssignmentScheduleRequests", &body)?;
                done.push_str("; ");
                done.push_str(&tf("offboard.activation_removal", &[("status", &s(&r, "status"))]));
            }
            Ok(done)
        }
        Planned::Convert { identity } => {
            g.invoke("Set-Mailbox", json!({ "Identity": identity, "Type": "Shared" }))?;
            ctx.converted = true;
            Ok(t("offboard.converted"))
        }
        Planned::Forward { identity, to, keep_copy } => {
            let params = json!({
                "Identity": identity,
                "ForwardingAddress": Value::Null,
                "ForwardingSmtpAddress": format!("smtp:{to}"),
                "DeliverToMailboxAndForward": keep_copy,
            });
            g.invoke("Set-Mailbox", params)?;
            Ok(tf("offboard.forwarding", &[("address", &to)]))
        }
        Planned::RemoveGroup { id, exchange, .. } => {
            match exchange {
                Some(list) => {
                    let params = json!({ "Identity": list, "Member": upn, "BypassSecurityGroupManagerCheck": true, "Confirm": false });
                    g.invoke("Remove-DistributionGroupMember", params)?;
                }
                None => g.delete(&format!("/groups/{id}/members/{user_id}/$ref"))?,
            }
            Ok(t("offboard.removed"))
        }
        Planned::RemoveLicence { sku_id, .. } => {
            g.post(&format!("/users/{user_id}/assignLicense"), &json!({ "addLicenses": [], "removeLicenses": [sku_id] }))?;
            Ok(t("offboard.removed"))
        }
        Planned::Delete => {
            g.delete(&format!("/users/{user_id}"))?;
            ctx.deleted = true;
            Ok(t("offboard.deleted"))
        }
    })
}

/// Reads everything offboarding can change for the user (runs on a worker thread). Parts that
/// can't be read are noted rather than stopping it.
fn load(g: &Graph, user_id: &str, upn: &str) -> anyhow::Result<(Found, bool)> {
    // The reads don't depend on each other, so they run at the same time.
    let (user, (roles, mut problems), groups, names, mailbox) = std::thread::scope(|t| {
        let user = t.spawn(|| {
            g.get(&format!(
                "/users/{user_id}?$select=id,displayName,userPrincipalName,mail,userType,accountEnabled,onPremisesSyncEnabled,assignedLicenses,licenseAssignmentStates"
            ))
        });
        let roles = t.spawn(|| load_roles(g, user_id));
        let groups = t.spawn(|| g.list(&format!("/users/{user_id}/memberOf/microsoft.graph.group?$select={GROUP_SELECT}&$top=999")));
        let names = t.spawn(|| g.list("/subscribedSkus").unwrap_or_default());
        let mailbox = g.invoke("Get-Mailbox", json!({ "Identity": upn }));
        (join(user), join(roles), join(groups), join(names), mailbox)
    });
    let user = user?;

    let groups = match groups {
        Ok(items) => {
            let mut groups: Vec<GroupItem> = items.iter().map(group_item).collect();
            groups.sort_by_key(|g| (!g.removable, g.name.to_lowercase()));
            groups
        }
        Err(e) => {
            problems.push(tf("offboard.groups_unread", &[("error", &e)]));
            Vec::new()
        }
    };

    let names: HashMap<String, String> = names.iter().map(|v| (s(v, "skuId"), sku_name(&s(v, "skuPartNumber")))).collect();
    let group_names: HashMap<&str, &str> = groups.iter().map(|g| (g.id.as_str(), g.name.as_str())).collect();
    let licences = licence_items(&user, &names, &group_names);

    let (mailbox, needs_consent) = match mailbox {
        Ok(found) => match found.first() {
            Some(m) => (Mailbox::Found { kind: s(m, "RecipientTypeDetails"), identity: crate::exchange::identity(m) }, false),
            None => (Mailbox::None, false),
        },
        Err(e) if api_error(&e).is_some_and(ApiError::not_found) => (Mailbox::None, false),
        Err(e) => {
            let consent = e.downcast_ref::<NeedsConsent>().is_some();
            problems.push(tf("offboard.mailbox_unread", &[("error", &e)]));
            (Mailbox::Unavailable(e.to_string()), consent)
        }
    };
    Ok((Found { user, mailbox, roles, groups, licences, problems }, needs_consent))
}

/// The user's directory roles, active and eligible, and what couldn't be read about them.
fn load_roles(g: &Graph, user_id: &str) -> (Vec<RoleItem>, Vec<String>) {
    let mut problems = Vec::new();
    let mut roles = Vec::new();
    match g.list(&format!(
        "/roleManagement/directory/roleAssignments?$filter=principalId eq '{user_id}'&$expand=roleDefinition($select=displayName)"
    )) {
        Ok(items) => roles.extend(items.iter().map(|a| RoleItem {
            name: scoped(&s(&a["roleDefinition"], "displayName"), &s(a, "directoryScopeId")),
            assignment_id: Some(s(a, "id")),
            role_id: s(a, "roleDefinitionId"),
            scope: s(a, "directoryScopeId"),
            active: false,
            selected: true,
        })),
        Err(e) => problems.push(tf("offboard.roles_unread", &[("error", &e)])),
    }
    // Eligible assignments need PIM, so a tenant without Entra ID P2 can't list them.
    match g.list(&format!(
        "/roleManagement/directory/roleEligibilitySchedules?$filter=principalId eq '{user_id}'&$expand=roleDefinition($select=displayName)"
    )) {
        Ok(items) => roles.extend(items.iter().map(|a| RoleItem {
            name: scoped(&s(&a["roleDefinition"], "displayName"), &s(a, "directoryScopeId")),
            assignment_id: None,
            role_id: s(a, "roleDefinitionId"),
            scope: s(a, "directoryScopeId"),
            active: false,
            selected: true,
        })),
        Err(e) => log::info!("Eligible role assignments not read: {e}"),
    }
    if roles.iter().any(|r| r.assignment_id.is_none()) {
        match g.list(&format!("/roleManagement/directory/roleAssignmentScheduleInstances?$filter=principalId eq '{user_id}'")) {
            Ok(items) => {
                let activated: Vec<(String, String)> = items
                    .iter()
                    .filter(|a| a["assignmentType"] == "Activated")
                    .map(|a| (s(a, "roleDefinitionId"), s(a, "directoryScopeId")))
                    .collect();
                merge_activated(&mut roles, &activated);
            }
            Err(e) => problems.push(tf("offboard.activations_unread", &[("error", &e)])),
        }
    }
    (roles, problems)
}

/// Whether the user has an active assignment of the role at the scope.
fn role_assigned(g: &Graph, user_id: &str, role_id: &str, scope: &str) -> anyhow::Result<bool> {
    let items = g.list(&format!(
        "/roleManagement/directory/roleAssignments?$filter=principalId eq '{user_id}' and roleDefinitionId eq '{role_id}'"
    ))?;
    Ok(items.iter().any(|a| s(a, "directoryScopeId") == scope))
}

/// Folds each role activated through PIM, which is listed both as an active and as an eligible
/// assignment, into its eligible one: the active one can't be deleted directly, so removing it
/// would always fail (and stop the account being deleted).
fn merge_activated(roles: &mut Vec<RoleItem>, activated: &[(String, String)]) {
    let is_activated = |r: &RoleItem| activated.iter().any(|(role, scope)| *role == r.role_id && *scope == r.scope);
    let eligible: Vec<(String, String)> =
        roles.iter().filter(|r| r.assignment_id.is_none()).map(|r| (r.role_id.clone(), r.scope.clone())).collect();
    roles.retain(|r| r.assignment_id.is_none() || !(is_activated(r) && eligible.contains(&(r.role_id.clone(), r.scope.clone()))));
    for r in roles.iter_mut().filter(|r| r.assignment_id.is_none()) {
        r.active = is_activated(r);
        if r.active {
            r.name = tf("offboard.active_now", &[("role", &r.name)]);
        }
    }
}

/// A role's name, with its scope when it isn't the whole directory.
fn scoped(name: &str, scope: &str) -> String {
    if scope.is_empty() || scope == "/" { name.to_string() } else { tf("offboard.scoped", &[("role", &name), ("scope", &scope)]) }
}

/// Whether the user can be taken out of a group here, and how.
pub fn group_item(v: &Value) -> GroupItem {
    let managed = managed(v);
    let assigns_licences = assigns_licences(v);
    let (removable, mut note) = match managed {
        Managed::Dynamic => (false, t("offboard.group_dynamic")),
        Managed::Synced => (false, t("offboard.group_synced")),
        Managed::Exchange => (true, t("offboard.group_exchange")),
        Managed::Graph => (true, String::new()),
    };
    if assigns_licences {
        note = if note.is_empty() { t("onboard.group_licences") } else { format!("{note}. {}", t("onboard.group_licences")) };
    }
    let identity = match s(v, "mail") {
        m if m.is_empty() => s(v, "id"),
        m => m,
    };
    GroupItem {
        id: s(v, "id"),
        name: s(v, "displayName"),
        kind: group_type(v),
        exchange: (managed == Managed::Exchange).then_some(identity),
        removable,
        assigns_licences,
        note,
        selected: removable,
    }
}

/// The user's licences, saying which are assigned directly (and can be removed) and which come
/// from groups.
pub fn licence_items(user: &Value, names: &HashMap<String, String>, groups: &HashMap<&str, &str>) -> Vec<LicenceItem> {
    let mut out: Vec<LicenceItem> = Vec::new();
    let states = user["licenseAssignmentStates"].as_array().cloned().unwrap_or_default();
    let assigned = user["assignedLicenses"].as_array().cloned().unwrap_or_default();
    for sku in assigned.iter().map(|a| s(a, "skuId")) {
        let mine: Vec<&Value> = states.iter().filter(|st| s(st, "skuId") == sku).collect();
        // Without assignment states, assume it is direct, which is what removing needs.
        let direct = mine.is_empty() || mine.iter().any(|st| st["assignedByGroup"].is_null());
        let from_groups = mine
            .iter()
            .filter_map(|st| st["assignedByGroup"].as_str())
            .map(|g| groups.get(g).map(|n| n.to_string()).unwrap_or_else(|| g.to_string()))
            .collect();
        let name = names.get(&sku).cloned().unwrap_or_else(|| sku.clone());
        out.push(LicenceItem { sku_id: sku, name, from_groups, direct, selected: direct });
    }
    out.sort_by_key(|l| l.name.to_lowercase());
    out
}

impl App {
    pub fn offboard_user(&mut self, user: Value) {
        let name = s(&user, "displayName");
        let upn = s(&user, "userPrincipalName");
        if !self.account.is_empty() && upn.eq_ignore_ascii_case(&self.account) {
            self.push(Dialog::message(&t("offboard.title"), &t("offboard.not_yourself")));
            return;
        }
        let wizard = Offboarding {
            page: 0,
            name: name.clone(),
            upn,
            found: Loadable::Loading,
            disable: true,
            revoke: true,
            convert: true,
            forward: false,
            forward_to: String::new(),
            keep_copy: true,
            delete: false,
            confirmed: false,
            error: None,
            progress: None,
        };
        let id = self.push(Dialog::Offboard(Box::new(wizard)));
        let user_id = s(&user, "id");
        let upn = s(&user, "userPrincipalName");
        self.spawn(tf("offboard.reading", &[("name", &name)]), move |g| load(g, &user_id, &upn), move |app, r| {
            let mut consent = false;
            if let Some(Dialog::Offboard(w)) = app.dialog_mut(id) {
                w.found = match r {
                    Ok((found, needs_consent)) => {
                        consent = needs_consent;
                        // Nothing to convert unless it is a user mailbox.
                        w.convert = matches!(&found.mailbox, Mailbox::Found { kind, .. } if kind == "UserMailbox");
                        Loadable::Ready(found)
                    }
                    Err(e) => Loadable::Failed(e.to_string()),
                };
            }
            if consent {
                app.offer_exchange_consent();
            }
        });
    }

    fn start_offboarding(&mut self, w: &mut Offboarding) {
        let Some(user_id) = w.found().map(|f| s(&f.user, "id")) else { return };
        let upn = w.upn.clone();
        let steps: Vec<Step<Graph, Ctx>> =
            w.plan().into_iter().map(|(p, needs)| step(p, needs, user_id.clone(), upn.clone())).collect();
        let id = user_id.clone();
        let progress = self.run_workflow(&tf("offboard.task", &[("name", &upn)]), Ctx::default(), steps, move |app, ctx, _| {
            if ctx.deleted {
                app.remove_obj(Node::Users, &id);
                app.remove_obj(Node::Mailboxes, &id);
            } else if ctx.disabled {
                app.upsert(Node::Users, json!({ "id": id, "accountEnabled": false }));
            }
            if ctx.converted && !ctx.deleted {
                app.upsert(Node::Mailboxes, json!({ "id": id, "RecipientTypeDetails": "SharedMailbox" }));
            }
        });
        w.progress = Some(progress);
        w.page = RESULTS;
    }

    pub(crate) fn show_offboarding(&mut self, ctx: &egui::Context, modal: egui::Modal, id: u64, w: &mut Offboarding, escape: bool) -> bool {
        let mut close = escape && !w.running();
        let mut next = false;
        let mut back = false;
        modal.show(ctx, |ui| {
            ui.set_width(760.0);
            workflow::page_heading(ui, &tf("offboard.task", &[("name", &w.name)]), &PAGES, w.page);
            egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| match w.page {
                0 => offboarding_choices(ui, w),
                PREVIEW => {
                    ui.label(RichText::new(tf("offboard.preview_intro", &[("upn", &w.upn)])).bold());
                    ui.add_space(4.0);
                    for (i, (p, needs)) in w.plan().iter().enumerate() {
                        let steps = needs.iter().map(|n| (n + 1).to_string()).collect::<Vec<_>>().join(", ");
                        let after = match needs.len() {
                            0 => String::new(),
                            n if *p == Planned::Delete => format!("  ({})", tf("offboard.only_if_all", &[("count", &n)])),
                            1 => format!("  ({})", tf("offboard.only_if_step", &[("step", &steps)])),
                            _ => format!("  ({})", tf("offboard.only_if_steps", &[("steps", &steps)])),
                        };
                        ui.label(format!("{}. {}{after}", i + 1, p.label()));
                    }
                    let warnings = w.warnings();
                    if !warnings.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new(t("offboard.before_you_go_on")).bold());
                        for warning in warnings {
                            ui.colored_label(ui.visuals().warn_fg_color, format!("⚠ {warning}"));
                        }
                    }
                    ui.add_space(8.0);
                    let check = t(if w.delete { "offboard.checked_delete" } else { "offboard.checked" });
                    ui.checkbox(&mut w.confirmed, check);
                }
                _ => {
                    if let Some(p) = &w.progress {
                        workflow::progress_ui(ui, id, &p.lock().unwrap());
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
                    let file = format!("{}.csv", t("offboard.report_file"));
                    close |= workflow::report_buttons(self, ui, &tf("offboard.report_title", &[("upn", &w.upn)]), &file, &p);
                }
                return;
            }
            ui.horizontal(|ui| {
                if w.page > 0 && ui.button(format!("  {}  ", t("wizard.back"))).clicked() {
                    back = true;
                }
                let (label, enabled) = if w.page == PREVIEW {
                    (tf("offboard.run", &[("count", &w.plan().len())]), w.confirmed)
                } else {
                    (t("wizard.next"), w.found().is_some())
                };
                if ui.add_enabled(enabled, egui::Button::new(format!("  {label}  "))).clicked() {
                    next = true;
                }
                close |= ui.button(t("dialog.cancel")).clicked();
            });
        });
        if back {
            w.page -= 1;
            w.confirmed = false;
            w.error = None;
        } else if next {
            w.error = w.problem();
            if w.error.is_none() {
                if w.page == PREVIEW && w.confirmed {
                    self.start_offboarding(w);
                } else if w.page == 0 {
                    w.page = PREVIEW;
                }
            }
        }
        !close
    }
}

/// The first page: what to do, from what was found.
fn offboarding_choices(ui: &mut egui::Ui, w: &mut Offboarding) {
    let found = match &mut w.found {
        Loadable::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tf("offboard.reading_all", &[("name", &w.name)]));
            });
            return;
        }
        Loadable::Failed(e) => {
            ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {}", tf("offboard.read_failed", &[("error", e)])));
            return;
        }
        Loadable::Ready(f) => f,
    };
    let weak = |text: &str| RichText::new(text).small().weak();
    let section = |ui: &mut egui::Ui, key: &str| {
        ui.add_space(6.0);
        ui.label(RichText::new(t(key)).bold());
    };
    let state = if found.user["accountEnabled"] == false { "offboard.account_disabled" } else { "offboard.account_enabled" };
    ui.label(format!("{} · {}", w.upn, t(state)));

    section(ui, "offboard.section_sign_in");
    ui.checkbox(&mut w.disable, t("offboard.disable"));
    ui.checkbox(&mut w.revoke, t("reregister.revoke_step"));

    section(ui, "tab.roles");
    if found.roles.is_empty() {
        ui.label(weak(&t("offboard.no_roles")));
    }
    for r in &mut found.roles {
        let key = if r.assignment_id.is_some() { "offboard.remove_active_role" } else { "offboard.remove_eligible_role" };
        ui.checkbox(&mut r.selected, tf(key, &[("role", &r.name)]));
    }

    section(ui, "offboard.section_mailbox");
    let delete = w.delete;
    match &found.mailbox {
        Mailbox::None => {
            ui.label(weak(&t("offboard.no_mailbox")));
        }
        Mailbox::Unavailable(e) => {
            ui.label(weak(&tf("offboard.mailbox_unchecked", &[("error", e)])));
        }
        Mailbox::Found { kind, .. } => {
            let user_mailbox = kind == "UserMailbox";
            if user_mailbox {
                ui.add_enabled(!delete, egui::Checkbox::new(&mut w.convert, t("offboard.convert")));
            } else {
                let kind = crate::exchange::mailbox_type(&json!({ "RecipientTypeDetails": kind }));
                ui.label(weak(&tf("offboard.already_kind", &[("kind", &kind)])));
            }
            ui.horizontal(|ui| {
                ui.add_enabled(!delete, egui::Checkbox::new(&mut w.forward, t("field.forward_to")));
                ui.add_enabled(w.forward && !delete, egui::TextEdit::singleline(&mut w.forward_to).hint_text("manager@contoso.com").desired_width(260.0));
            });
            ui.add_enabled(w.forward && !delete, egui::Checkbox::new(&mut w.keep_copy, t("field.keep_copy")));
            ui.label(weak(&t("offboard.full_access_note")));
        }
    }

    section(ui, "node.groups");
    if found.groups.is_empty() {
        ui.label(weak(&t("offboard.no_groups")));
    } else {
        ui.horizontal(|ui| {
            if ui.small_button(t("offboard.select_all")).clicked() {
                found.groups.iter_mut().for_each(|g| g.selected = g.removable);
            }
            if ui.small_button(t("offboard.select_none")).clicked() {
                found.groups.iter_mut().for_each(|g| g.selected = false);
            }
        });
    }
    for g in &mut found.groups {
        ui.horizontal(|ui| {
            ui.add_enabled(g.removable, egui::Checkbox::new(&mut g.selected, format!("{}  ({})", g.name, g.kind)));
            if !g.note.is_empty() {
                ui.label(weak(&g.note));
            }
        });
    }

    section(ui, "tab.licences");
    if found.licences.is_empty() {
        ui.label(weak(&t("offboard.no_licences")));
    }
    for l in &mut found.licences {
        if l.direct {
            ui.checkbox(&mut l.selected, tf("offboard.remove_licence", &[("licence", &l.name)]));
        } else {
            ui.label(weak(&tf("offboard.inherited_licence", &[("licence", &l.name), ("groups", &l.from_groups.join(", "))])));
        }
    }

    section(ui, "overview.account");
    let keeps_mailbox = w.convert || w.forward;
    ui.add_enabled(!keeps_mailbox, egui::Checkbox::new(&mut w.delete, t("offboard.delete_account")));
    if keeps_mailbox {
        ui.label(weak(&t("offboard.delete_unavailable")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str, licences: bool) -> Planned {
        Planned::RemoveGroup { id: name.into(), name: name.into(), exchange: None, assigns_licences: licences }
    }

    #[test]
    fn orders_steps_so_licences_wait_for_the_mailbox() {
        let licence = Planned::RemoveLicence { sku_id: "s".into(), name: "E3".into() };
        let convert = Planned::Convert { identity: "ann@contoso.com".into() };
        let forward = Planned::Forward { identity: "ann@contoso.com".into(), to: "bo@contoso.com".into(), keep_copy: true };
        // Chosen in a careless order.
        let plan = order(vec![licence.clone(), group("Sales", true), group("Staff", false), forward.clone(), convert.clone(), Planned::Revoke, Planned::Disable]);
        let steps: Vec<&Planned> = plan.iter().map(|(p, _)| p).collect();
        assert_eq!(steps, [&Planned::Disable, &Planned::Revoke, &convert, &forward, &group("Sales", true), &group("Staff", false), &licence]);
        let needs: Vec<&Vec<usize>> = plan.iter().map(|(_, n)| n).collect();
        // The licence and the group that assigns one wait for converting (2) and forwarding (3).
        assert_eq!(needs, [&vec![], &vec![], &vec![], &vec![], &vec![2, 3], &vec![], &vec![2, 3]]);
    }

    #[test]
    fn deletes_last_and_only_if_everything_else_worked() {
        let plan = order(vec![Planned::Delete, Planned::Disable, Planned::RemoveLicence { sku_id: "s".into(), name: "E3".into() }]);
        assert_eq!(plan.last().unwrap(), &(Planned::Delete, vec![0, 1]));
    }

    #[test]
    fn folds_activated_roles_into_their_eligibility() {
        let role = |id: Option<&str>, role: &str| RoleItem {
            name: role.into(),
            assignment_id: id.map(String::from),
            role_id: role.into(),
            scope: "/".into(),
            active: false,
            selected: true,
        };
        // Activated through PIM, eligible but not activated, and assigned permanently.
        let mut roles = vec![role(Some("a1"), "admin"), role(None, "admin"), role(None, "reader"), role(Some("a2"), "billing")];
        merge_activated(&mut roles, &[("admin".into(), "/".into()), ("billing".into(), "/".into())]);
        let left: Vec<(&str, bool, bool)> = roles.iter().map(|r| (r.name.as_str(), r.assignment_id.is_some(), r.active)).collect();
        assert_eq!(left, [("admin, active now", false, true), ("reader", false, false), ("billing", true, false)]);
    }

    #[test]
    fn groups_are_removed_where_they_are_managed() {
        let list = group_item(&json!({ "id": "1", "displayName": "All staff", "groupTypes": [], "mailEnabled": true, "mail": "staff@contoso.com" }));
        assert_eq!(list.exchange.as_deref(), Some("staff@contoso.com"));
        let dynamic = group_item(&json!({ "id": "2", "groupTypes": ["DynamicMembership"], "membershipRule": "x" }));
        assert!(!dynamic.removable && !dynamic.selected);
        let m365 = group_item(&json!({ "id": "3", "groupTypes": ["Unified"], "mailEnabled": true, "assignedLicenses": [{ "skuId": "x" }] }));
        assert!(m365.removable && m365.exchange.is_none() && m365.assigns_licences);
    }

    #[test]
    fn tells_direct_licences_from_inherited_ones() {
        let user = json!({
            "assignedLicenses": [{ "skuId": "e3" }, { "skuId": "e5" }, { "skuId": "both" }],
            "licenseAssignmentStates": [
                { "skuId": "e3", "assignedByGroup": null },
                { "skuId": "e5", "assignedByGroup": "g1" },
                { "skuId": "both", "assignedByGroup": null },
                { "skuId": "both", "assignedByGroup": "g1" },
            ],
        });
        let names: HashMap<String, String> = [("e3".to_string(), "Microsoft 365 E3".to_string())].into();
        let groups: HashMap<&str, &str> = [("g1", "Licensing")].into();
        let items = licence_items(&user, &names, &groups);
        let by_id = |id: &str| items.iter().find(|l| l.sku_id == id).unwrap();
        assert!(by_id("e3").direct && by_id("e3").name == "Microsoft 365 E3");
        assert!(!by_id("e5").direct && by_id("e5").from_groups == ["Licensing"]);
        assert!(by_id("both").direct, "a licence assigned both ways can still have its direct assignment removed");
    }

    fn wizard(mailbox: &str) -> Offboarding {
        let found = Found {
            user: json!({ "id": "u1", "userPrincipalName": "ann@contoso.com" }),
            mailbox: Mailbox::Found { kind: mailbox.into(), identity: "ann@contoso.com".into() },
            roles: vec![],
            groups: vec![],
            licences: vec![LicenceItem { sku_id: "e3".into(), name: "E3".into(), from_groups: vec![], direct: true, selected: true }],
            problems: vec![],
        };
        Offboarding {
            page: 0,
            name: "Ann".into(),
            upn: "ann@contoso.com".into(),
            found: Loadable::Ready(found),
            disable: true,
            revoke: true,
            convert: false,
            forward: false,
            forward_to: String::new(),
            keep_copy: true,
            delete: false,
            confirmed: false,
            error: None,
            progress: None,
        }
    }

    #[test]
    fn refuses_to_delete_an_account_whose_mailbox_is_kept() {
        let mut w = wizard("UserMailbox");
        w.convert = true;
        w.delete = true;
        assert!(w.problem().unwrap().contains("deleting the account deletes the mailbox"));
        w.convert = false;
        w.forward = true;
        w.forward_to = "not an address".into();
        w.delete = false;
        assert!(w.problem().unwrap().contains("address to forward"));
    }

    #[test]
    fn warns_when_a_mailbox_would_lose_its_licence() {
        let w = wizard("UserMailbox");
        assert!(w.warnings().iter().any(|x| x.contains("deletes it after 30 days")));
        let shared = wizard("SharedMailbox");
        assert!(!shared.warnings().iter().any(|x| x.contains("deletes it after 30 days")));
        // Nothing to convert when it is already shared.
        let mut shared = shared;
        shared.convert = true;
        assert!(!shared.plan().iter().any(|(p, _)| matches!(p, Planned::Convert { .. })));
    }
}
