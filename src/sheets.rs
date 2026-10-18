//! Tabbed property sheets for users, groups, devices and licences.

use crate::app::App;
use crate::dialogs::{Dialog, Field, Form};
use crate::graph::Graph;
use crate::model::{self, general_fields, s, sku_name, Node};
use crate::ops::collection;
use crate::app::Bold;
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
}

impl TabKind {
    fn title(self) -> &'static str {
        match self {
            TabKind::General => "General",
            TabKind::MemberOf => "Member of",
            TabKind::UserLicences | TabKind::GroupLicences => "Licences",
            TabKind::Roles => "Directory roles",
            TabKind::Members => "Members",
            TabKind::Owners => "Owners",
            TabKind::RegisteredOwners => "Registered owners",
            TabKind::RegisteredUsers => "Registered users",
            TabKind::ServicePlans => "Service plans",
            TabKind::LicensedUsers => "Licensed users",
        }
    }
}

fn tabs_for(node: Node) -> Vec<TabKind> {
    use TabKind::*;
    match node {
        Node::Users => vec![General, MemberOf, UserLicences, Roles],
        Node::Groups => vec![General, Members, Owners, Roles, GroupLicences],
        Node::Devices => vec![General, RegisteredOwners, RegisteredUsers, MemberOf],
        Node::Licences => vec![General, ServicePlans, LicensedUsers],
        Node::Root => vec![General],
    }
}

type Work = Box<dyn FnOnce(&Graph) -> anyhow::Result<()> + Send>;

pub enum TabBody {
    Loading,
    Error(String),
    Fields(Vec<(String, String)>),
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
enum TabOp {
    Add,
    Remove,
    Edit,
    Reload,
}

impl Sheet {
    pub fn new(node: Node, obj: Value) -> Sheet {
        let tabs = tabs_for(node)
            .into_iter()
            .map(|kind| Tab { kind, body: TabBody::Loading, requested: false, sel: None })
            .collect();
        Sheet { node, obj, tabs, active: 0 }
    }

    fn name(&self) -> String {
        self.node.object_name(&self.obj)
    }

    /// Labels for the add / remove / edit operations available on a tab.
    fn op_labels(&self, kind: TabKind) -> [Option<&'static str>; 3] {
        use TabKind::*;
        let editable = matches!(self.node, Node::Users | Node::Groups);
        match (self.node, kind) {
            (_, General) if editable => [None, None, Some("Edit…")],
            (Node::Users, MemberOf) => [Some("Add to group…"), Some("Remove from group"), None],
            (Node::Users, UserLicences) | (Node::Groups, GroupLicences) => [Some("Assign licence…"), Some("Remove licence"), None],
            (_, Roles) => [Some("Assign role…"), Some("Remove assignment"), None],
            (_, Members) => [Some("Add member…"), Some("Remove member"), None],
            (Node::Groups, Owners) => [Some("Add owner…"), Some("Remove owner"), None],
            (_, LicensedUsers) => [Some("Assign to user…"), Some("Remove from user"), None],
            _ => [None, None, None],
        }
    }
}

fn table(headers: Vec<&'static str>, items: Vec<Value>, cells: impl Fn(&Value) -> Vec<String>) -> TabBody {
    let rows = items.into_iter().map(|v| SheetRow { cells: cells(&v), data: v }).collect();
    TabBody::Table { headers, rows }
}

fn odata_kind(v: &Value) -> String {
    let t = v["@odata.type"].as_str().unwrap_or("").trim_start_matches("#microsoft.graph.");
    match t {
        "user" => "User",
        "group" => "Group",
        "device" => "Device",
        "directoryRole" => "Directory role",
        "orgContact" => "Contact",
        other => other,
    }
    .to_string()
}

fn sku_names(g: &Graph) -> anyhow::Result<std::collections::HashMap<String, String>> {
    Ok(g.list("/subscribedSkus")?
        .iter()
        .map(|v| (s(v, "skuId"), sku_name(&s(v, "skuPartNumber"))))
        .collect())
}

/// Fetches the content of a tab (runs on a worker thread). Returns the body and,
/// for General tabs, the freshly read object.
fn load_tab(g: &Graph, node: Node, kind: TabKind, obj: &Value) -> anyhow::Result<(TabBody, Option<Value>)> {
    use TabKind::*;
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
                ("Product".into(), sku_name(&s(obj, "skuPartNumber"))),
                ("SKU part number".into(), s(obj, "skuPartNumber")),
                ("SKU ID".into(), s(obj, "skuId")),
                ("Status".into(), s(obj, "capabilityStatus")),
                ("Applies to".into(), s(obj, "appliesTo")),
                ("Assigned".into(), used.to_string()),
                ("Total (enabled)".into(), total.to_string()),
                ("Available".into(), (total - used).to_string()),
                ("Suspended".into(), obj["prepaidUnits"]["suspended"].to_string()),
                ("In warning".into(), obj["prepaidUnits"]["warning"].to_string()),
            ])
        }
        (Node::Root, _) => TabBody::Fields(vec![("Name".into(), s(obj, "name"))]),
        (_, General) => {
            let (select, keys): (&str, &[(&str, &str)]) = match node {
                Node::Users => (
                    "id,displayName,givenName,surname,userPrincipalName,mail,jobTitle,department,companyName,officeLocation,mobilePhone,businessPhones,usageLocation,accountEnabled,userType,createdDateTime,lastPasswordChangeDateTime,onPremisesSyncEnabled,employeeId",
                    &[("Display name", "displayName"), ("First name", "givenName"), ("Last name", "surname"), ("User principal name", "userPrincipalName"), ("Email", "mail"), ("Job title", "jobTitle"), ("Department", "department"), ("Company", "companyName"), ("Office", "officeLocation"), ("Mobile phone", "mobilePhone"), ("Business phones", "businessPhones"), ("Usage location", "usageLocation"), ("Employee ID", "employeeId"), ("Account enabled", "accountEnabled"), ("User type", "userType"), ("Synced from on-premises", "onPremisesSyncEnabled"), ("Created", "createdDateTime"), ("Password last changed", "lastPasswordChangeDateTime"), ("Object ID", "id")],
                ),
                Node::Groups => (
                    "id,displayName,description,mail,mailNickname,mailEnabled,securityEnabled,groupTypes,isAssignableToRole,membershipRule,membershipRuleProcessingState,visibility,createdDateTime,onPremisesSyncEnabled,assignedLicenses",
                    &[("Name", "displayName"), ("Description", "description"), ("Email", "mail"), ("Mail nickname", "mailNickname"), ("Security enabled", "securityEnabled"), ("Mail enabled", "mailEnabled"), ("Group types", "groupTypes"), ("Role-assignable", "isAssignableToRole"), ("Membership rule", "membershipRule"), ("Rule processing", "membershipRuleProcessingState"), ("Visibility", "visibility"), ("Synced from on-premises", "onPremisesSyncEnabled"), ("Created", "createdDateTime"), ("Object ID", "id")],
                ),
                _ => (
                    "id,deviceId,displayName,operatingSystem,operatingSystemVersion,trustType,accountEnabled,isCompliant,isManaged,manufacturer,model,approximateLastSignInDateTime,registrationDateTime,profileType,onPremisesSyncEnabled",
                    &[("Name", "displayName"), ("Operating system", "operatingSystem"), ("OS version", "operatingSystemVersion"), ("Join type", "trustType"), ("Manufacturer", "manufacturer"), ("Model", "model"), ("Enabled", "accountEnabled"), ("Compliant", "isCompliant"), ("Managed", "isManaged"), ("Profile type", "profileType"), ("Synced from on-premises", "onPremisesSyncEnabled"), ("Registered", "registrationDateTime"), ("Last sign-in", "approximateLastSignInDateTime"), ("Device ID", "deviceId"), ("Object ID", "id")],
                ),
            };
            let fresh = g.get(&format!("/{coll}/{id}?$select={select}"))?;
            let mut fields = general_fields(&fresh, keys);
            if node == Node::Devices {
                fields[3].1 = model::join_type(&fields[3].1).to_string();
            }
            return Ok((TabBody::Fields(fields), Some(fresh)));
        }
        (_, MemberOf) => {
            let items = g.list(&format!("/{coll}/{id}/memberOf?$select=id,displayName,mail,description&$top=999"))?;
            table(vec!["Name", "Type", "Email"], items, |v| vec![s(v, "displayName"), odata_kind(v), s(v, "mail")])
        }
        (Node::Users, UserLicences) => {
            let items = g.list(&format!("/users/{id}/licenseDetails"))?;
            table(vec!["Product", "SKU", "Service plans"], items, |v| {
                vec![sku_name(&s(v, "skuPartNumber")), s(v, "skuPartNumber"), model::count(&v["servicePlans"])]
            })
        }
        (_, GroupLicences) => {
            let fresh = g.get(&format!("/groups/{id}?$select=assignedLicenses"))?;
            let names = sku_names(g)?;
            let items = fresh["assignedLicenses"].as_array().cloned().unwrap_or_default();
            table(vec!["Product", "SKU ID", "Disabled plans"], items, |v| {
                let sku = s(v, "skuId");
                vec![names.get(&sku).cloned().unwrap_or_else(|| sku.clone()), sku, model::count(&v["disabledPlans"])]
            })
        }
        (_, Roles) => {
            let items = g.list(&format!(
                "/roleManagement/directory/roleAssignments?$filter=principalId eq '{id}'&$expand=roleDefinition($select=displayName,description)"
            ))?;
            table(vec!["Role", "Scope", "Description"], items, |v| {
                vec![s(&v["roleDefinition"], "displayName"), s(v, "directoryScopeId"), s(&v["roleDefinition"], "description")]
            })
        }
        (_, Members) => {
            let items = g.list(&format!("/groups/{id}/members?$select=id,displayName,userPrincipalName,mail&$top=999"))?;
            table(vec!["Name", "Type", "UPN / email"], items, principal_cols)
        }
        (_, Owners) => {
            let items = g.list(&format!("/{coll}/{id}/owners?$select=id,displayName,userPrincipalName,mail"))?;
            table(vec!["Name", "Type", "UPN / email"], items, principal_cols)
        }
        (_, RegisteredOwners) | (_, RegisteredUsers) => {
            let rel = if kind == RegisteredOwners { "registeredOwners" } else { "registeredUsers" };
            let items = g.list(&format!("/devices/{id}/{rel}?$select=id,displayName,userPrincipalName,mail"))?;
            table(vec!["Name", "Type", "UPN / email"], items, principal_cols)
        }
        (_, ServicePlans) => {
            let items = obj["servicePlans"].as_array().cloned().unwrap_or_default();
            table(vec!["Service plan", "Applies to", "Status"], items, |v| {
                vec![s(v, "servicePlanName"), s(v, "appliesTo"), s(v, "provisioningStatus")]
            })
        }
        (_, LicensedUsers) => {
            let sku = s(obj, "skuId");
            let items = g.list_advanced(&format!(
                "/users?$filter=assignedLicenses/any(x:x/skuId eq {sku})&$select=id,displayName,userPrincipalName,accountEnabled&$count=true&$top=999"
            ))?;
            table(vec!["Name", "User principal name", "Enabled"], items, |v| {
                vec![s(v, "displayName"), s(v, "userPrincipalName"), model::yes_no(&v["accountEnabled"])]
            })
        }
        _ => TabBody::Fields(Vec::new()),
    };
    Ok((body, None))
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
            format!("Loading {}…", kind.title().to_lowercase()),
            move |g| load_tab(g, node, kind, &obj),
            move |app, r| {
                let mut fresh_obj = None;
                if let Some(sheet) = app.sheet_mut(sheet_id)
                    && let Some(tab) = sheet.tabs.iter_mut().find(|t| t.kind == kind) {
                        match r {
                            Ok((body, fresh)) => {
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
                            Err(e) => tab.body = TabBody::Error(e.to_string()),
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
            if !matches!(sheet.tabs[sheet.active].body, TabBody::Table { .. } | TabBody::Fields(_)) {
                sheet.tabs[sheet.active].body = TabBody::Loading;
            }
            self.start_tab_load(sheet_id, sheet.node, kind, sheet.obj.clone());
        }

        let labels = sheet.op_labels(kind);
        let name = sheet.name();
        modal.show(ctx, |ui| {
            ui.set_width(760.0);
            ui.set_min_height(440.0);
            ui.heading(format!("{} {} Properties", sheet.node.icon(), name));
            ui.add_space(6.0);
            // Tab strip.
            ui.horizontal_wrapped(|ui| {
                for (i, tab) in sheet.tabs.iter().enumerate() {
                    let r = ui.selectable_label(sheet.active == i, tab.kind.title());
                    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sheet.active == i, format!("{} tab", tab.kind.title())));
                    if r.clicked() {
                        sheet.active = i;
                    }
                }
            });
            ui.label(RichText::new("Ctrl+Tab switches tabs").small().weak());
            ui.separator();
            let tab = &mut sheet.tabs[sheet.active];
            let body_height = 330.0;
            match &tab.body {
                TabBody::Loading => {
                    ui.allocate_ui(egui::vec2(ui.available_width(), body_height), |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading…");
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
                                ui.label(RichText::new(k).bold());
                                ui.add(egui::Label::new(if v.is_empty() { "—" } else { v.as_str() }).wrap().selectable(true));
                                ui.end_row();
                            }
                        });
                    });
                }
                TabBody::Table { headers, rows } => {
                    if rows.is_empty() {
                        ui.allocate_ui(egui::vec2(ui.available_width(), body_height), |ui| {
                            ui.label("There are no items to show.");
                        });
                    } else {
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
                            for t in headers {
                                h.col(|ui| {
                                    ui.label(RichText::new(*t).bold());
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
                                                if b.clicked() || b.gained_focus() {
                                                    clicked = Some(ri);
                                                }
                                            } else {
                                                ui.add(egui::Label::new(cell.as_str()).truncate());
                                            }
                                        });
                                    }
                                    if r.response().clicked() {
                                        clicked = Some(ri);
                                    }
                                });
                            }
                        });
                        if let Some(c) = clicked {
                            tab.sel = Some(c);
                        }
                    }
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                let has_row = tab.sel.is_some() && matches!(tab.body, TabBody::Table { .. });
                if let Some(l) = labels[0]
                    && ui.button(l).on_hover_text("Insert").clicked() {
                        op = Some(TabOp::Add);
                    }
                if let Some(l) = labels[1]
                    && ui.add_enabled(has_row, egui::Button::new(l)).on_hover_text("Delete").clicked() {
                        op = Some(TabOp::Remove);
                    }
                if let Some(l) = labels[2]
                    && ui.button(l).on_hover_text("F2").clicked() {
                        op = Some(TabOp::Edit);
                    }
                if ui.button("Refresh").on_hover_text("F5").clicked() {
                    op = Some(TabOp::Reload);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let c = ui.button("  Close  ");
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
        let row: Option<Value> = match (&tab.body, tab.sel) {
            (TabBody::Table { rows, .. }, Some(i)) => rows.get(i).map(|r| r.data.clone()),
            _ => None,
        };
        if op == TabOp::Reload {
            tab.requested = false;
            return;
        }
        let obj = sheet.obj.clone();
        let id = s(&obj, "id");
        let name = node.object_name(&obj);
        let reload = move |app: &mut App| app.reload_tab(sheet_id, kind);

        match op {
            TabOp::Add if labels[0].is_some() => match kind {
                MemberOf => self.pick_group(&format!("Add {name} to group"), move |app, g| app.add_member(&g.id, &id, reload)),
                Members => self.pick_principal(&format!("Add member to {name}"), move |app, m| app.add_member(&id, &m.id, reload)),
                Owners => {
                    let coll = collection(node);
                    self.pick_user(&format!("Add owner to {name}"), move |app, u| app.add_owner(coll, &id, &u.id, reload));
                }
                Roles => {
                    if node == Node::Groups && obj["isAssignableToRole"] != true {
                        self.push(Dialog::message("Assign directory role", "Only role-assignable groups can be assigned directory roles."));
                        return;
                    }
                    self.pick_role(&format!("Assign directory role to {name}"), move |app, r| app.assign_role(&id, &r.id, reload));
                }
                UserLicences | GroupLicences => {
                    let coll = collection(node);
                    self.pick_sku(&format!("Assign licence to {name}"), move |app, sku| {
                        app.set_licence(coll, &id, &sku.id, true, move |app| {
                            reload(app);
                            app.load(node);
                        })
                    });
                }
                LicensedUsers => {
                    let sku = s(&obj, "skuId");
                    self.pick_user(&format!("Assign {name} to user"), move |app, u| {
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
                        (format!("Remove {name} from '{}'?", s(&row, "displayName")), Box::new(move |g| g.delete(&path)))
                    }
                    Members | Owners => {
                        let rel = if kind == Members { "members" } else { "owners" };
                        let path = format!("/{}/{id}/{rel}/{rid}/$ref", collection(node));
                        (format!("Remove '{}' from {rel} of {name}?", s(&row, "displayName")), Box::new(move |g| g.delete(&path)))
                    }
                    Roles => {
                        let path = format!("/roleManagement/directory/roleAssignments/{rid}");
                        (format!("Remove the role '{}' from {name}?", s(&row["roleDefinition"], "displayName")), Box::new(move |g| g.delete(&path)))
                    }
                    UserLicences | GroupLicences => {
                        let sku = s(&row, "skuId");
                        let path = format!("/{}/{id}/assignLicense", collection(node));
                        let body = json!({ "addLicenses": [], "removeLicenses": [sku] });
                        (format!("Remove this licence from {name}?"), Box::new(move |g| g.post(&path, &body).map(|_| ())))
                    }
                    LicensedUsers => {
                        let path = format!("/users/{rid}/assignLicense");
                        let body = json!({ "addLicenses": [], "removeLicenses": [s(&obj, "skuId")] });
                        (format!("Remove {name} from '{}'?", s(&row, "displayName")), Box::new(move |g| g.post(&path, &body).map(|_| ())))
                    }
                    _ => return,
                };
                let label = labels[1].unwrap_or("Remove");
                self.push(Dialog::confirm(label, &what, move |app| {
                    app.mutate(label, path_or_body, move |app| {
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
                let mut f = Form::new(&format!("Edit {}", s(&obj, "displayName")), "Save");
                for (key, label) in [
                    ("displayName", "Display name"),
                    ("givenName", "First name"),
                    ("surname", "Last name"),
                    ("jobTitle", "Job title"),
                    ("department", "Department"),
                    ("companyName", "Company"),
                    ("officeLocation", "Office"),
                    ("mobilePhone", "Mobile phone"),
                    ("employeeId", "Employee ID"),
                    ("usageLocation", "Usage location"),
                ] {
                    let field = Field::text(key, label, &s(&obj, key));
                    f = f.field(if key == "displayName" { field.required() } else { field });
                }
                f
            }
            Node::Groups => Form::new(&format!("Edit {}", s(&obj, "displayName")), "Save")
                .field(Field::text("displayName", "Name", &s(&obj, "displayName")).required())
                .field(Field::multiline("description", "Description", &s(&obj, "description"))),
            _ => return,
        };
        let form = form.on_submit(move |app, f| {
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
            app.mutate("Save changes", move |g| g.patch(&path, &patch), move |app| {
                let mut updated = body;
                updated["id"] = json!(oid);
                app.upsert(node, updated);
                app.reload_tab(sheet_id, TabKind::General);
            });
            Ok(())
        });
        self.push(Dialog::Form(form));
    }
}
