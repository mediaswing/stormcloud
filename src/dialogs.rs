//! Modal dialogs: messages, confirmations, forms, pickers and the sign-in prompt.

use crate::app::App;
use crate::bulk::Bulk;
use crate::graph::Graph;
use crate::ops::item_actions;
use crate::sheets::Sheet;
use crate::app::Bold;
use eframe::egui::{self, Align, Key, Layout, Modifiers, RichText};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub type OnYes = Box<dyn FnOnce(&mut App)>;
pub type OnPick = Box<dyn FnOnce(&mut App, PickItem)>;

pub struct DialogEntry {
    pub id: u64,
    pub dialog: Dialog,
    /// False until the dialog has been drawn once (used to place initial focus).
    pub opened: bool,
}

pub enum Dialog {
    Message { title: String, text: String, copy: Option<String> },
    Confirm { title: String, text: String, on_yes: Option<OnYes> },
    Form(Form),
    Picker(Picker),
    Sheet(Box<Sheet>),
    Bulk(Box<Bulk>),
    Onboard(Box<crate::onboarding::Onboarding>),
    Offboard(Box<crate::offboarding::Offboarding>),
    /// A device code sign-in; `approval` when it is only to approve more permissions.
    DeviceCode { message: String, uri: String, code: String, approval: bool, cancelled: Arc<AtomicBool> },
    ContextMenu,
    Help,
    About,
    Update(crate::update::Release),
}

impl Dialog {
    pub fn message(title: &str, text: &str) -> Dialog {
        Dialog::Message { title: title.into(), text: text.into(), copy: None }
    }

    pub fn confirm(title: &str, text: &str, on_yes: impl FnOnce(&mut App) + 'static) -> Dialog {
        Dialog::Confirm { title: title.into(), text: text.into(), on_yes: Some(Box::new(on_yes)) }
    }
}

// ----- forms ---------------------------------------------------------------

pub enum FieldKind {
    Text,
    Password,
    Multiline,
    Check,
    Choice(Vec<(String, String)>),
    Info,
}

pub struct Field {
    pub key: &'static str,
    pub label: String,
    pub kind: FieldKind,
    pub value: String,
    pub checked: bool,
    pub choice: usize,
    pub required: bool,
}

impl Field {
    fn new(key: &'static str, label: &str, kind: FieldKind) -> Field {
        Field { key, label: label.into(), kind, value: String::new(), checked: false, choice: 0, required: false }
    }
    pub fn text(key: &'static str, label: &str, value: &str) -> Field {
        Field { value: value.into(), ..Field::new(key, label, FieldKind::Text) }
    }
    pub fn password(key: &'static str, label: &str, value: &str) -> Field {
        Field { value: value.into(), ..Field::new(key, label, FieldKind::Password) }
    }
    pub fn multiline(key: &'static str, label: &str, value: &str) -> Field {
        Field { value: value.into(), ..Field::new(key, label, FieldKind::Multiline) }
    }
    pub fn check(key: &'static str, label: &str, checked: bool) -> Field {
        Field { checked, ..Field::new(key, label, FieldKind::Check) }
    }
    /// Options are `(value, label)` pairs.
    pub fn choice(key: &'static str, label: &str, options: Vec<(String, String)>, selected: usize) -> Field {
        Field { choice: selected, ..Field::new(key, label, FieldKind::Choice(options)) }
    }
    pub fn info(label: &str, text: &str) -> Field {
        Field { value: text.into(), ..Field::new("", label, FieldKind::Info) }
    }
    pub fn required(mut self) -> Field {
        self.required = true;
        self
    }
}

pub type Submit = Box<dyn Fn(&mut App, &Form) -> Result<(), String>>;

pub struct Form {
    pub title: String,
    pub intro: String,
    pub fields: Vec<Field>,
    pub ok: String,
    pub error: Option<String>,
    on_submit: Option<Submit>,
}

impl Form {
    pub fn new(title: &str, ok: &str) -> Form {
        Form { title: title.into(), intro: String::new(), fields: Vec::new(), ok: ok.into(), error: None, on_submit: None }
    }
    pub fn intro(mut self, text: &str) -> Form {
        self.intro = text.into();
        self
    }
    pub fn field(mut self, f: Field) -> Form {
        self.fields.push(f);
        self
    }
    pub fn on_submit(mut self, f: impl Fn(&mut App, &Form) -> Result<(), String> + 'static) -> Form {
        self.on_submit = Some(Box::new(f));
        self
    }
    fn get(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }
    pub fn text(&self, key: &str) -> String {
        self.get(key).map(|f| f.value.trim().to_string()).unwrap_or_default()
    }
    pub fn raw(&self, key: &str) -> String {
        self.get(key).map(|f| f.value.clone()).unwrap_or_default()
    }
    pub fn checked(&self, key: &str) -> bool {
        self.get(key).is_some_and(|f| f.checked)
    }
    pub fn choice(&self, key: &str) -> String {
        match self.get(key) {
            Some(Field { kind: FieldKind::Choice(opts), choice, .. }) => {
                opts.get(*choice).map(|o| o.0.clone()).unwrap_or_default()
            }
            _ => String::new(),
        }
    }
}

// ----- pickers -------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct PickItem {
    pub id: String,
    pub label: String,
    pub detail: String,
}

pub type Fetch = Arc<dyn Fn(&Graph, &str) -> anyhow::Result<Vec<PickItem>> + Send + Sync>;

pub struct Picker {
    pub title: String,
    pub prompt: String,
    pub query: String,
    pub items: Vec<PickItem>,
    all: Option<Vec<PickItem>>,
    pub sel: usize,
    pub loading: bool,
    /// Remote pickers search the server as you type; local pickers load once and filter.
    remote: bool,
    fetch: Fetch,
    on_pick: Option<OnPick>,
    last_edit: Option<Instant>,
    searched: Option<String>,
    error: Option<String>,
}

impl Picker {
    pub fn new(
        title: &str,
        prompt: &str,
        remote: bool,
        fetch: impl Fn(&Graph, &str) -> anyhow::Result<Vec<PickItem>> + Send + Sync + 'static,
        on_pick: impl FnOnce(&mut App, PickItem) + 'static,
    ) -> Picker {
        Picker {
            title: title.into(),
            prompt: prompt.into(),
            query: String::new(),
            items: Vec::new(),
            all: None,
            sel: 0,
            loading: false,
            remote,
            fetch: Arc::new(fetch),
            on_pick: Some(Box::new(on_pick)),
            last_edit: None,
            searched: None,
            error: None,
        }
    }

    fn apply_local_filter(&mut self) {
        if let Some(all) = &self.all {
            let q = self.query.trim().to_lowercase();
            self.items = all
                .iter()
                .filter(|i| q.is_empty() || i.label.to_lowercase().contains(&q) || i.detail.to_lowercase().contains(&q))
                .cloned()
                .collect();
            self.sel = 0;
        }
    }
}

impl App {
    fn picker_search(&mut self, dialog_id: u64, query: String) {
        let Some(Dialog::Picker(p)) = self.dialog_mut(dialog_id) else { return };
        p.loading = true;
        p.searched = Some(query.clone());
        let fetch = p.fetch.clone();
        let q = query.clone();
        self.spawn("Searching…", move |g| fetch(g, &q), move |app, r| {
            let Some(Dialog::Picker(p)) = app.dialog_mut(dialog_id) else { return };
            if p.searched.as_deref() != Some(query.as_str()) {
                return;
            }
            p.loading = false;
            match r {
                Ok(items) => {
                    p.error = None;
                    if p.remote {
                        p.items = items;
                        p.sel = 0;
                    } else {
                        p.all = Some(items);
                        p.apply_local_filter();
                    }
                }
                Err(e) => p.error = Some(e.to_string()),
            }
        });
    }

    // ----- rendering -----------------------------------------------------------

    pub fn show_dialogs(&mut self, ctx: &egui::Context) {
        let mut entries = std::mem::take(&mut self.dialogs);
        let n = entries.len();
        let mut keep = Vec::with_capacity(n);
        for (i, mut entry) in entries.drain(..).enumerate() {
            let top = i + 1 == n;
            if self.show_dialog(ctx, &mut entry, top) {
                entry.opened = true;
                keep.push(entry);
            }
        }
        // Dialogs opened (or closed) while drawing were applied to `self.dialogs`.
        let added = std::mem::take(&mut self.dialogs);
        let closed = std::mem::take(&mut self.closed_dialogs);
        keep.retain(|e| !closed.contains(&e.id));
        keep.extend(added);
        self.dialogs = keep;
    }

    /// Draws one dialog. Returns false when it should close.
    fn show_dialog(&mut self, ctx: &egui::Context, entry: &mut DialogEntry, top: bool) -> bool {
        let id = entry.id;
        let first = !entry.opened;
        let escape = top && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let modal = egui::Modal::new(egui::Id::new(("dialog", id)));
        match &mut entry.dialog {
            Dialog::Message { title, text, copy } => {
                let mut close = escape;
                modal.show(ctx, |ui| {
                    ui.set_max_width(520.0);
                    ui.heading(title.as_str());
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(text.as_str()).wrap());
                    if let Some(secret) = copy {
                        ui.add_space(6.0);
                        let mut s = secret.clone();
                        ui.add(egui::TextEdit::singleline(&mut s).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let ok = ui.button("   OK   ");
                        if first {
                            ok.request_focus();
                        }
                        close |= ok.clicked();
                        if let Some(secret) = copy
                            && ui.button("Copy to clipboard").clicked() {
                                ui.ctx().copy_text(secret.clone());
                                self.status = "Copied to clipboard".into();
                            }
                    });
                });
                !close
            }
            Dialog::Confirm { title, text, on_yes } => {
                let mut answer = if escape { Some(false) } else { None };
                modal.show(ctx, |ui| {
                    ui.set_max_width(480.0);
                    ui.heading(title.as_str());
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(text.as_str()).wrap());
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let yes = ui.button("  Yes  ");
                        let no = ui.button("  No  ");
                        if first {
                            no.request_focus();
                        }
                        if yes.clicked() {
                            answer = Some(true);
                        }
                        if no.clicked() {
                            answer = Some(false);
                        }
                    });
                });
                match answer {
                    Some(true) => {
                        if let Some(f) = on_yes.take() {
                            f(self);
                        }
                        false
                    }
                    Some(false) => false,
                    None => true,
                }
            }
            Dialog::Form(form) => self.show_form(ctx, modal, form, first, top, escape),
            Dialog::Picker(p) => self.show_picker(ctx, modal, id, p, first, top, escape),
            Dialog::Sheet(sheet) => self.show_sheet(ctx, modal, id, sheet, first, top, escape),
            Dialog::Bulk(bulk) => self.show_bulk(ctx, modal, id, bulk, first, escape),
            Dialog::Onboard(w) => self.show_onboarding(ctx, modal, id, w, escape),
            Dialog::Offboard(w) => self.show_offboarding(ctx, modal, id, w, escape),
            Dialog::DeviceCode { message, uri, code, approval, cancelled } => {
                let approval = *approval;
                let mut close = escape;
                modal.show(ctx, |ui| {
                    ui.set_max_width(520.0);
                    ui.heading(if approval { "Approve permissions" } else { "Sign in to Microsoft Entra ID" });
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(message.as_str()).wrap());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("Code:");
                        ui.add(egui::Label::new(RichText::new(code.as_str()).monospace().size(24.0).strong()).selectable(true));
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let open = ui.button("Copy code and open browser");
                        if first {
                            open.request_focus();
                        }
                        if open.clicked() {
                            ui.ctx().copy_text(code.clone());
                            ui.ctx().open_url(egui::OpenUrl::new_tab(uri.as_str()));
                        }
                        if ui.button("Copy code").clicked() {
                            ui.ctx().copy_text(code.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        ui.spinner();
                    });
                });
                if close {
                    // Stops the poll, so a sign-in finished in the browser later isn't used.
                    cancelled.store(true, Ordering::SeqCst);
                }
                if close && approval {
                    self.cancel_approval();
                } else if close {
                    self.cancel_sign_in();
                }
                !close
            }
            Dialog::ContextMenu => {
                let mut close = escape;
                modal.show(ctx, |ui| {
                    ui.set_min_width(260.0);
                    let name = self.selected().map(|o| self.node.object_name(o)).unwrap_or_default();
                    ui.label(RichText::new(if name.is_empty() { "Actions".to_string() } else { name }).bold());
                    ui.separator();
                    ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                        let acts = if self.selected().is_some() { item_actions(self.node) } else { Vec::new() };
                        if acts.is_empty() {
                            ui.label("No actions available");
                        }
                        for (i, act) in acts.into_iter().enumerate() {
                            let r = self.action_button(ui, act);
                            if i == 0 && first {
                                r.request_focus();
                            }
                            close |= r.clicked();
                        }
                    });
                });
                !close
            }
            Dialog::Help => {
                let mut close = escape;
                modal.show(ctx, |ui| {
                    ui.set_max_width(560.0);
                    ui.heading("Keyboard shortcuts");
                    ui.add_space(6.0);
                    let cmd = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
                    let rows: Vec<(String, &str)> = vec![
                        ("F6 / Shift+F6".into(), "Move between the console tree, results and Actions pane"),
                        ("Tab / Shift+Tab".into(), "Move between controls"),
                        ("↑ ↓ PgUp PgDn Home End".into(), "Move the selection"),
                        ("← →".into(), "Collapse / expand in the console tree"),
                        ("Enter".into(), "Open the selected item's properties"),
                        ("Shift+F10".into(), "Actions for the selected item (context menu)"),
                        ("F10".into(), "Move to the menu bar"),
                        ("F5".into(), "Refresh"),
                        (format!("{cmd}F"), "Filter the results list"),
                        (format!("{cmd}N"), "New user, group, shared mailbox or distribution list"),
                        ("Delete".into(), "Delete the selected item"),
                        (format!("{cmd}E"), "Enable or disable the selected item"),
                        (format!("{cmd}P"), "Reset a user's password"),
                        (format!("{cmd}L"), "Assign a licence"),
                        (format!("{cmd}G / {cmd}M"), "Add to group / add member"),
                        (format!("{cmd}R"), "Assign a directory role"),
                        (format!("{cmd}U"), "Assign the selected licence to a user, or a user or group to the selected application"),
                        ("Ctrl+Tab / Ctrl+Shift+Tab".into(), "Next / previous tab in a properties sheet"),
                        ("Insert, Delete, F2".into(), "Add, remove, edit on a properties tab"),
                        (format!("{cmd}, (comma)"), "Settings"),
                        (format!("{cmd}+ / {cmd}- / {cmd}0"), "Zoom in / out / reset"),
                        ("Escape".into(), "Close the dialog"),
                        (format!("{cmd}Q"), "Exit"),
                    ];
                    egui::Grid::new("help").num_columns(2).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                        for (k, d) in rows {
                            ui.label(RichText::new(k).monospace());
                            ui.label(d);
                            ui.end_row();
                        }
                    });
                    ui.add_space(8.0);
                    let ok = ui.button("Close");
                    if first {
                        ok.request_focus();
                    }
                    close |= ok.clicked();
                });
                !close
            }
            Dialog::Update(release) => {
                let mut close = escape;
                modal.show(ctx, |ui| {
                    ui.set_max_width(560.0);
                    ui.heading("Update available");
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(format!(
                            "Graphical Cloud Management {} is available. You have version {}.",
                            release.version,
                            env!("CARGO_PKG_VERSION")
                        ))
                        .wrap(),
                    );
                    if !release.notes.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new("What's new").bold());
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            ui.add(egui::Label::new(release.notes.as_str()).wrap().selectable(true));
                        });
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let download = ui.button("Download…").on_hover_text("Open the release page in your browser");
                        if first {
                            download.request_focus();
                        }
                        if download.clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(release.url.as_str()));
                            close = true;
                        }
                        if ui.button("Skip this version").clicked() {
                            let version = release.version.clone();
                            self.skip_version(&version);
                            close = true;
                        }
                        close |= ui.button("Later").clicked();
                    });
                });
                !close
            }
            Dialog::About => {
                let mut close = escape;
                modal.show(ctx, |ui| {
                    ui.heading("Graphical Cloud Management");
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.label("A console for managing Microsoft Entra ID through Microsoft Graph.");
                    ui.label("Free software under the GNU General Public License, version 3 or later.");
                    ui.add_space(4.0);
                    ui.label(format!("Settings: {}", crate::config::settings_path().display()));
                    ui.add_space(8.0);
                    let ok = ui.button("   OK   ");
                    if first {
                        ok.request_focus();
                    }
                    close |= ok.clicked();
                });
                !close
            }
        }
    }

    fn show_form(&mut self, ctx: &egui::Context, modal: egui::Modal, form: &mut Form, first: bool, top: bool, escape: bool) -> bool {
        let mut submit = false;
        let mut cancel = escape;
        modal.show(ctx, |ui| {
            ui.set_width(500.0);
            ui.heading(form.title.as_str());
            if !form.intro.is_empty() {
                ui.add_space(4.0);
                ui.add(egui::Label::new(form.intro.as_str()).wrap());
            }
            ui.add_space(8.0);
            let mut focused_first = !first;
            egui::Grid::new("form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                for (fi, f) in form.fields.iter_mut().enumerate() {
                    let label_text = if f.required { format!("{}*", f.label) } else { f.label.clone() };
                    let resp = match &f.kind {
                        FieldKind::Check => {
                            ui.label("");
                            Some(ui.checkbox(&mut f.checked, f.label.as_str()))
                        }
                        FieldKind::Info => {
                            ui.label(label_text);
                            ui.add(egui::Label::new(f.value.as_str()).wrap());
                            None
                        }
                        kind => {
                            let label = ui.label(label_text);
                            let r = match kind {
                                FieldKind::Text => ui.add(egui::TextEdit::singleline(&mut f.value).desired_width(320.0)),
                                FieldKind::Password => ui.add(egui::TextEdit::singleline(&mut f.value).password(true).desired_width(320.0)),
                                FieldKind::Multiline => ui.add(egui::TextEdit::multiline(&mut f.value).desired_rows(3).desired_width(320.0)),
                                FieldKind::Choice(opts) => {
                                    let current = opts.get(f.choice).map(|o| o.1.clone()).unwrap_or_default();
                                    egui::ComboBox::from_id_salt(("choice", fi))
                                        .selected_text(current)
                                        .width(320.0)
                                        .show_ui(ui, |ui| {
                                            for (oi, o) in opts.iter().enumerate() {
                                                ui.selectable_value(&mut f.choice, oi, o.1.as_str());
                                            }
                                        })
                                        .response
                                }
                                _ => unreachable!(),
                            };
                            let r = r.labelled_by(label.id);
                            if matches!(f.kind, FieldKind::Text | FieldKind::Password)
                                && r.lost_focus()
                                && ui.input(|i| i.key_pressed(Key::Enter))
                            {
                                submit = true;
                            }
                            Some(r)
                        }
                    };
                    if let Some(r) = resp
                        && !focused_first {
                            r.request_focus();
                            focused_first = true;
                        }
                    ui.end_row();
                }
            });
            if let Some(err) = &form.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {err}"));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                submit |= ui.button(format!("  {}  ", form.ok)).clicked();
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if cancel {
            return false;
        }
        if submit && top {
            if let Some(missing) = form.fields.iter().find(|f| f.required && f.value.trim().is_empty()) {
                form.error = Some(format!("{} is required.", missing.label));
                return true;
            }
            let cb = form.on_submit.take();
            if let Some(cb) = cb {
                let r = cb(self, form);
                form.on_submit = Some(cb);
                match r {
                    Ok(()) => return false,
                    Err(e) => form.error = Some(e),
                }
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn show_picker(&mut self, ctx: &egui::Context, modal: egui::Modal, id: u64, p: &mut Picker, first: bool, top: bool, escape: bool) -> bool {
        if first {
            self.pending_picker_search.push((id, String::new()));
        }
        let query_id = egui::Id::new(("picker-query", id));
        let query_focused = ctx.memory(|m| m.has_focus(query_id));
        let mut pick: Option<usize> = None;
        let mut cancel = escape;
        if top && query_focused {
            ctx.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    p.sel = (p.sel + 1).min(p.items.len().saturating_sub(1));
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    p.sel = p.sel.saturating_sub(1);
                }
                if i.consume_key(Modifiers::NONE, Key::Enter) && !p.items.is_empty() {
                    pick = Some(p.sel);
                }
            });
        }
        modal.show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(p.title.as_str());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = ui.label(p.prompt.as_str());
                let r = ui
                    .add(egui::TextEdit::singleline(&mut p.query).id(query_id).hint_text("Type to search").desired_width(f32::INFINITY))
                    .labelled_by(label.id);
                if first {
                    r.request_focus();
                }
                if r.changed() {
                    if p.remote {
                        p.last_edit = Some(Instant::now());
                    } else {
                        p.apply_local_filter();
                    }
                }
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                    if p.loading && p.items.is_empty() {
                        ui.spinner();
                    } else if let Some(err) = &p.error {
                        ui.colored_label(ui.visuals().error_fg_color, err.as_str());
                    } else if p.items.is_empty() {
                        ui.label("No matches.");
                    }
                    for (i, item) in p.items.iter().enumerate() {
                        let text = if item.detail.is_empty() { item.label.clone() } else { format!("{}  —  {}", item.label, item.detail) };
                        let r = ui.selectable_label(i == p.sel, text);
                        if i == p.sel && query_focused {
                            r.scroll_to_me(None);
                        }
                        if r.clicked() {
                            p.sel = i;
                        }
                        if r.double_clicked() || r.has_focus() && ui.input(|inp| inp.key_pressed(Key::Enter)) {
                            pick = Some(i);
                        }
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(RichText::new("↑/↓ to choose, Enter to select, Esc to cancel").small().weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!p.items.is_empty(), egui::Button::new("  Select  ")).clicked() {
                    pick = Some(p.sel);
                }
                cancel |= ui.button("Cancel").clicked();
            });
        });
        // Debounced server search.
        if let Some(t) = p.last_edit {
            if t.elapsed() >= Duration::from_millis(350) {
                p.last_edit = None;
                if p.searched.as_deref() != Some(p.query.trim()) {
                    self.pending_picker_search.push((id, p.query.trim().to_string()));
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if cancel {
            return false;
        }
        if let Some(i) = pick.filter(|_| top)
            && let (Some(item), Some(cb)) = (p.items.get(i).cloned(), p.on_pick.take()) {
                cb(self, item);
                return false;
            }
        true
    }

    /// Starts searches requested while drawing (dialogs are detached from `self` then).
    pub fn run_pending_searches(&mut self) {
        for (id, q) in std::mem::take(&mut self.pending_picker_search) {
            self.picker_search(id, q);
        }
    }
}
