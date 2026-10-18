//! Main window: menu bar, toolbar, console tree, result pane, Actions pane and status bar.

use crate::auth::Auth;
use crate::config::Settings;
use crate::dialogs::{Dialog, DialogEntry};
use crate::graph::Graph;
use crate::model::{Node, TREE};
use crate::ops::{item_actions, node_actions, Act};
use eframe::egui::{
    self, Align, Event, EventFilter, Key, KeyboardShortcut, Layout, Modifiers, RichText, Sense,
};
use egui_extras::{Column, TableBuilder};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;

pub type Update = Box<dyn FnOnce(&mut App) + Send>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Tree,
    List,
    Actions,
}

const ARROWS_LOCKED: EventFilter = EventFilter {
    tab: false,
    horizontal_arrows: true,
    vertical_arrows: true,
    escape: false,
};

pub struct App {
    pub settings: Settings,
    pub graph: Option<Arc<Graph>>,
    tx: Sender<Update>,
    rx: Receiver<Update>,
    pub ctx: egui::Context,

    pub signed_in: bool,
    pub signin_gen: u64,
    pub account: String,
    pub tenant_name: String,
    pub domains: Vec<String>,

    pub node: Node,
    expanded: HashSet<Node>,
    pub data: HashMap<Node, Vec<Value>>,
    loading: HashSet<Node>,
    errors: HashMap<Node, String>,
    /// Lists that stopped at [`MAX_ITEMS`] with more objects to come.
    truncated: HashSet<Node>,
    filter: String,
    sort: Option<(usize, bool)>,
    view: Vec<usize>,
    view_dirty: bool,
    sel: Option<usize>,
    /// The selected object's ID, taken before the current list's data changed, as the view's
    /// indices no longer match the data until it is rebuilt.
    sel_before_change: Option<Option<Value>>,

    pane: Pane,
    focus_pane: Option<Pane>,
    scroll_to_sel: bool,
    focus_filter: bool,
    file_menu_id: Option<egui::Id>,
    show_actions: bool,

    pub dialogs: Vec<DialogEntry>,
    pub closed_dialogs: Vec<u64>,
    pub pending_picker_search: Vec<(u64, String)>,
    next_id: u64,
    busy: usize,
    pub status: String,
    quit: bool,
}

/// The font family for strong text, drawn in Google Sans Bold.
fn bold_font() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// Strong text in the bold font. egui's own `strong()` only brightens text.
pub trait Bold {
    fn bold(self) -> RichText;
}

impl Bold for RichText {
    fn bold(self) -> RichText {
        self.strong().family(bold_font())
    }
}

/// Google Sans (SIL Open Font License, see assets/fonts/OFL.txt): Medium for
/// interface text, Bold for strong text. egui's built-in fonts stay behind
/// them as fallbacks for symbols and scripts Google Sans does not cover, and
/// for monospace text.
fn setup_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "GoogleSans-Medium".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/GoogleSans-Medium.ttf"))),
    );
    fonts.font_data.insert(
        "GoogleSans-Bold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/GoogleSans-Bold.ttf"))),
    );
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "GoogleSans-Medium".into());
    let mut bold = vec!["GoogleSans-Bold".to_owned()];
    bold.extend(fallbacks);
    fonts.families.insert(bold_font(), bold);
    ctx.set_fonts(fonts);
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        setup_fonts(&cc.egui_ctx);
        let (tx, rx) = channel();
        let settings = Settings::load().unwrap_or_default();
        let mut app = App {
            settings,
            graph: None,
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            signed_in: false,
            signin_gen: 0,
            account: String::new(),
            tenant_name: String::new(),
            domains: Vec::new(),
            node: Node::Root,
            expanded: [Node::Root].into_iter().collect(),
            data: HashMap::new(),
            loading: HashSet::new(),
            errors: HashMap::new(),
            truncated: HashSet::new(),
            filter: String::new(),
            sort: None,
            view: Vec::new(),
            view_dirty: true,
            sel: None,
            sel_before_change: None,
            pane: Pane::Tree,
            focus_pane: Some(Pane::Tree),
            scroll_to_sel: false,
            focus_filter: false,
            file_menu_id: None,
            show_actions: true,
            dialogs: Vec::new(),
            closed_dialogs: Vec::new(),
            pending_picker_search: Vec::new(),
            next_id: 1,
            busy: 0,
            status: "Ready".into(),
            quit: false,
        };
        app.data.insert(Node::Root, child_rows(Node::Root));
        if app.settings.is_complete() {
            app.connect();
        } else {
            app.open_settings();
        }
        app
    }

    // ----- infrastructure -------------------------------------------------

    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn push(&mut self, dialog: Dialog) -> u64 {
        let id = self.next_id();
        self.dialogs.push(DialogEntry { id, dialog, opened: false });
        id
    }

    pub fn close_dialog(&mut self, id: u64) {
        self.dialogs.retain(|d| d.id != id);
        self.closed_dialogs.push(id);
    }

    pub fn dialog_mut(&mut self, id: u64) -> Option<&mut Dialog> {
        self.dialogs.iter_mut().find(|d| d.id == id).map(|d| &mut d.dialog)
    }

    pub fn error(&mut self, title: &str, err: impl std::fmt::Display) {
        log::error!("{title}: {err}");
        self.status = format!("{title} failed");
        self.push(Dialog::message(title, &err.to_string()));
    }

    /// Runs `work` on a background thread and applies `done` on the UI thread.
    pub fn spawn<T: Send + 'static>(
        &mut self,
        label: impl Into<String>,
        work: impl FnOnce(&Graph) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut App, anyhow::Result<T>) + Send + 'static,
    ) {
        let Some(graph) = self.graph.clone() else {
            self.open_settings();
            return;
        };
        self.busy += 1;
        self.status = label.into();
        log::debug!("Task started: {}", self.status);
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let result = work(&graph);
            let _ = tx.send(Box::new(move |app: &mut App| {
                app.busy -= 1;
                if app.busy == 0 {
                    app.status = "Ready".into();
                }
                done(app, result);
            }));
            ctx.request_repaint();
        });
    }

    /// Runs `work` with only the auth handle (used for sign-in, which has no token yet).
    fn spawn_auth<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&Auth) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut App, anyhow::Result<T>) + Send + 'static,
    ) {
        let Some(graph) = self.graph.clone() else { return };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let result = work(&graph.auth);
            let _ = tx.send(Box::new(move |app: &mut App| done(app, result)));
            ctx.request_repaint();
        });
    }

    // ----- sign-in --------------------------------------------------------

    pub fn connect(&mut self) {
        let http = reqwest::blocking::Client::builder()
            .user_agent(concat!("StormCloud/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client");
        let auth = Arc::new(Auth::new(self.settings.clone(), http.clone()));
        self.graph = Some(Arc::new(Graph::new(auth, http, &self.settings.graph_endpoint)));
        self.signed_in = false;
        self.clear_lists();
        self.sign_in();
    }

    /// Forgets the lists loaded from Graph. Loads still running finish under an older
    /// `signin_gen`, so their results are dropped.
    fn clear_lists(&mut self) {
        self.data.retain(|n, _| n.endpoint().is_none());
        self.loading.clear();
        self.errors.clear();
        self.truncated.clear();
        self.sel_before_change = None;
        self.view_dirty = true;
    }

    pub fn sign_in(&mut self) {
        let Some(graph) = self.graph.clone() else {
            self.open_settings();
            return;
        };
        self.signin_gen += 1;
        let attempt = self.signin_gen;
        if graph.auth.has_refresh_token() {
            self.status = "Signing in…".into();
            self.spawn("Signing in…", |g| g.get("/me?$select=displayName,userPrincipalName"), move |app, r| {
                match r {
                    Ok(me) => app.signed_in_as(me),
                    Err(_) => app.device_code(attempt),
                }
            });
        } else {
            self.device_code(attempt);
        }
    }

    fn device_code(&mut self, attempt: u64) {
        self.status = "Requesting sign-in code…".into();
        self.spawn_auth(|a| a.start_device_code(), move |app, r| {
            if attempt != app.signin_gen {
                return;
            }
            let dc = match r {
                Ok(dc) => dc,
                Err(e) => return app.error("Sign in", e),
            };
            let id = app.push(Dialog::DeviceCode {
                message: dc.message.clone(),
                uri: dc.verification_uri.clone(),
                code: dc.user_code.clone(),
            });
            app.status = "Waiting for sign-in in your browser…".into();
            app.spawn_auth(
                move |a| a.poll_device_code(&dc),
                move |app, r| {
                    app.close_dialog(id);
                    if attempt != app.signin_gen {
                        return;
                    }
                    match r {
                        Ok(()) => app.spawn(
                            "Loading profile…",
                            |g| g.get("/me?$select=displayName,userPrincipalName"),
                            |app, r| match r {
                                Ok(me) => app.signed_in_as(me),
                                Err(e) => app.error("Sign in", e),
                            },
                        ),
                        Err(e) => app.error("Sign in", e),
                    }
                },
            );
        });
    }

    pub fn cancel_sign_in(&mut self) {
        self.signin_gen += 1;
        self.status = "Sign-in cancelled".into();
    }

    fn signed_in_as(&mut self, me: Value) {
        self.signed_in = true;
        self.account = me["userPrincipalName"].as_str().unwrap_or("").to_string();
        log::info!("Signed in as {}", self.account);
        self.status = format!("Signed in as {}", self.account);
        self.spawn(
            "Loading tenant…",
            |g| {
                let org = g.get("/organization?$select=displayName")?;
                let domains = g.list("/domains?$select=id,isVerified,isDefault")?;
                Ok((org, domains))
            },
            |app, r| match r {
                Ok((org, domains)) => {
                    app.tenant_name = org["value"][0]["displayName"].as_str().unwrap_or("").to_string();
                    let mut d: Vec<&Value> = domains.iter().filter(|d| d["isVerified"] == true).collect();
                    d.sort_by_key(|d| d["isDefault"] != true);
                    app.domains = d.iter().filter_map(|d| d["id"].as_str().map(str::to_string)).collect();
                }
                Err(e) => app.error("Load tenant", e),
            },
        );
        if self.node.endpoint().is_some() {
            self.refresh();
        }
    }

    pub fn sign_out(&mut self) {
        if let Some(g) = &self.graph {
            g.auth.sign_out();
        }
        self.signin_gen += 1;
        self.signed_in = false;
        self.account.clear();
        self.tenant_name.clear();
        self.clear_lists();
        self.status = "Signed out".into();
    }

    // ----- data -----------------------------------------------------------

    pub fn select_node(&mut self, node: Node) {
        if self.node == node {
            return;
        }
        log::debug!("Console node selected: {}", node.title());
        self.node = node;
        self.filter.clear();
        self.sort = None;
        self.sel = None;
        self.sel_before_change = None;
        self.view_dirty = true;
        if node.endpoint().is_some() && !self.data.contains_key(&node) {
            self.load(node);
        }
    }

    pub fn refresh(&mut self) {
        if self.node.endpoint().is_some() {
            self.load(self.node);
        }
    }

    /// Reloads `node` in the background (if it has a Graph endpoint).
    pub fn load(&mut self, node: Node) {
        let Some(path) = node.endpoint() else { return };
        if !self.signed_in || self.loading.contains(&node) {
            return;
        }
        self.loading.insert(node);
        let session = self.signin_gen;
        self.spawn(
            format!("Loading {}…", node.title().to_lowercase()),
            move |g| g.list_capped(path),
            move |app, r| {
                if session != app.signin_gen {
                    log::debug!("Dropping {} loaded before signing out", node.title());
                    return;
                }
                app.loading.remove(&node);
                match r {
                    Ok((mut rows, truncated)) => {
                        let first = node.columns()[0].get;
                        rows.sort_by_cached_key(|v| first(v).to_lowercase());
                        let noun = node.title().to_lowercase();
                        app.status = if truncated {
                            format!("{} {noun} loaded; the list stops there, so some are missing", rows.len())
                        } else {
                            format!("{} {noun} loaded", rows.len())
                        };
                        log::info!("{}", app.status);
                        app.errors.remove(&node);
                        if truncated {
                            app.truncated.insert(node);
                        } else {
                            app.truncated.remove(&node);
                        }
                        app.data_changing(node);
                        app.data.insert(node, rows);
                    }
                    Err(e) => {
                        app.status = format!("Could not load {}", node.title().to_lowercase());
                        log::error!("Loading {} failed: {e}", node.title());
                        app.errors.insert(node, e.to_string());
                    }
                }
                if app.node == node {
                    app.view_dirty = true;
                }
            },
        );
    }

    /// Why the list for `node` may not hold every object, if it may not.
    pub fn incomplete_list(&self, node: Node) -> Option<String> {
        let noun = node.title().to_lowercase();
        if self.loading.contains(&node) {
            Some(format!("The {noun} are still loading."))
        } else if self.errors.contains_key(&node) {
            Some(format!("The {noun} did not load. Refresh the list and try again."))
        } else if self.truncated.contains(&node) {
            Some(format!("Only the first {} {noun} were loaded.", crate::graph::MAX_ITEMS))
        } else if !self.data.contains_key(&node) {
            Some(format!("The {noun} have not been loaded."))
        } else {
            None
        }
    }

    /// Remembers the selection before the data for `node` changes, while the view still matches it.
    fn data_changing(&mut self, node: Node) {
        if self.node == node && self.sel_before_change.is_none() {
            self.sel_before_change = Some(self.selected().map(|v| v["id"].clone()));
        }
    }

    /// Updates an object in the cached list for `node`, or adds it. Lists that are not loaded
    /// are left alone, so they still load in full when opened.
    pub fn upsert(&mut self, node: Node, obj: Value) {
        if !self.data.contains_key(&node) {
            return;
        }
        self.data_changing(node);
        let rows = self.data.entry(node).or_default();
        match rows.iter_mut().find(|r| r["id"] == obj["id"]) {
            Some(r) => {
                if let (Some(dst), Some(src)) = (r.as_object_mut(), obj.as_object()) {
                    for (k, v) in src {
                        dst.insert(k.clone(), v.clone());
                    }
                }
            }
            None => rows.push(obj),
        }
        if self.node == node {
            self.view_dirty = true;
        }
    }

    pub fn remove_obj(&mut self, node: Node, id: &str) {
        self.data_changing(node);
        if let Some(rows) = self.data.get_mut(&node) {
            rows.retain(|r| r["id"] != id);
        }
        if self.node == node {
            self.view_dirty = true;
        }
    }

    fn rebuild_view(&mut self) {
        let prev = match self.sel_before_change.take() {
            Some(prev) => prev,
            None => self.selected().map(|v| v["id"].clone()),
        };
        let old_pos = self.sel;
        let rows = self.data.get(&self.node).map(Vec::as_slice).unwrap_or(&[]);
        let cols = self.node.columns();
        let needle = self.filter.trim().to_lowercase();
        let mut view: Vec<usize> = (0..rows.len())
            .filter(|&i| {
                needle.is_empty() || cols.iter().any(|c| (c.get)(&rows[i]).to_lowercase().contains(&needle))
            })
            .collect();
        if let Some((ci, asc)) = self.sort
            && let Some(col) = cols.get(ci) {
                view.sort_by_cached_key(|&i| {
                    let text = (col.get)(&rows[i]);
                    // Sort numeric columns numerically.
                    (text.parse::<f64>().ok().map(|n| (n * 1000.0) as i64), text.to_lowercase())
                });
                if !asc {
                    view.reverse();
                }
            }
        // When the selected object has gone, select the one now in its place.
        self.sel = prev
            .and_then(|id| view.iter().position(|&i| rows[i]["id"] == id))
            .or(if view.is_empty() { None } else { Some(old_pos.unwrap_or(0).min(view.len() - 1)) });
        self.view = view;
        self.view_dirty = false;
    }

    /// The objects in the result list, filtered and sorted as shown.
    pub fn view_rows(&self) -> Vec<&Value> {
        let rows = rows_of(&self.data, self.node);
        self.view.iter().filter_map(|&i| rows.get(i)).collect()
    }

    pub fn selected(&self) -> Option<&Value> {
        let i = *self.view.get(self.sel?)?;
        self.data.get(&self.node)?.get(i)
    }

    fn set_sel(&mut self, pos: usize) {
        if self.view.is_empty() {
            self.sel = None;
        } else {
            self.sel = Some(pos.min(self.view.len() - 1));
            self.scroll_to_sel = true;
            self.focus_pane = Some(Pane::List);
        }
    }

    // ----- keyboard -------------------------------------------------------

    fn global_keys(&mut self, ctx: &egui::Context) {
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        let typing = ctx.egui_wants_keyboard_input();

        if pressed(sc(Modifiers::COMMAND, Key::Q)) {
            self.quit = true;
        }
        if pressed(sc(Modifiers::NONE, Key::F1)) {
            self.push(Dialog::Help);
        }
        if pressed(sc(Modifiers::COMMAND, Key::Comma)) {
            self.open_settings();
        }
        if pressed(sc(Modifiers::COMMAND, Key::F)) {
            self.focus_filter = true;
        }
        if pressed(sc(Modifiers::NONE, Key::F10))
            && let Some(id) = self.file_menu_id {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        if pressed(sc(Modifiers::NONE, Key::F6)) {
            self.cycle_pane(true);
        }
        if pressed(sc(Modifiers::SHIFT, Key::F6)) {
            self.cycle_pane(false);
        }
        if !typing && pressed(sc(Modifiers::SHIFT, Key::F10)) {
            self.push(Dialog::ContextMenu);
        }
        if typing {
            // Shortcuts below would clash with editing text (e.g. Delete).
            if self.filter_has_focus(ctx) && pressed(sc(Modifiers::NONE, Key::ArrowDown)) {
                self.set_sel(self.sel.unwrap_or(0));
            }
            return;
        }
        let mut acts = node_actions(self.node);
        acts.extend(item_actions(self.node));
        for act in acts {
            let mut fired = false;
            for s in act.shortcuts() {
                fired |= pressed(s);
            }
            if fired && self.act_enabled(act) {
                self.run_act(act);
            }
        }
    }

    fn filter_has_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(egui::Id::new("filter")))
    }

    fn cycle_pane(&mut self, forward: bool) {
        let mut order = vec![Pane::Tree, Pane::List];
        if self.show_actions {
            order.push(Pane::Actions);
        }
        let i = order.iter().position(|p| *p == self.pane).unwrap_or(0);
        let n = order.len();
        let next = if forward { (i + 1) % n } else { (i + n - 1) % n };
        self.pane = order[next];
        self.focus_pane = Some(self.pane);
        if self.pane == Pane::List && self.sel.is_none() && !self.view.is_empty() {
            self.sel = Some(0);
        }
        self.scroll_to_sel = true;
    }

    pub fn act_enabled(&self, act: Act) -> bool {
        if act.needs_selection() && self.selected().is_none() {
            return false;
        }
        act == Act::Open || act == Act::Refresh && self.node.endpoint().is_some() || self.signed_in
    }

    // ----- panels ---------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                let file = ui.menu_button("File", |ui| {
                    if ui.button("Settings…").clicked() {
                        self.open_settings();
                    }
                    ui.separator();
                    if self.signed_in {
                        if ui.button("Sign out").clicked() {
                            self.sign_out();
                        }
                    } else if ui.button("Sign in").clicked() {
                        self.sign_in();
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        self.quit = true;
                    }
                });
                self.file_menu_id = Some(file.response.id);
                ui.menu_button("Action", |ui| {
                    let acts = self.all_actions();
                    if acts.is_empty() {
                        ui.label("No actions available");
                    }
                    for act in acts {
                        if self.action_button(ui, act).clicked() {
                            ui.close();
                        }
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.add_enabled(self.signed_in, egui::Button::new("Refresh").shortcut_text("F5")).clicked() {
                        self.refresh();
                    }
                    if ui.add(egui::Button::new("Filter…").shortcut_text(ui.ctx().format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F)))).clicked() {
                        self.focus_filter = true;
                    }
                    ui.separator();
                    ui.checkbox(&mut self.show_actions, "Actions pane");
                    ui.separator();
                    ui.label("Theme");
                    egui::widgets::global_theme_preference_buttons(ui);
                    ui.separator();
                    ui.label("Zoom: Ctrl/⌘ + plus, minus or 0");
                });
                ui.menu_button("Help", |ui| {
                    if ui.add(egui::Button::new("Keyboard shortcuts").shortcut_text("F1")).clicked() {
                        self.push(Dialog::Help);
                    }
                    if ui.button("About Graphical Cloud Management").clicked() {
                        self.push(Dialog::About);
                    }
                });
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let ctx = ui.ctx().clone();
                let tool = |ui: &mut egui::Ui, app: &mut App, icon: &str, act: Act| {
                    let tip = match act.shortcuts().first() {
                        Some(s) => format!("{} ({})", app.act_label(act), ctx.format_shortcut(s)),
                        None => app.act_label(act),
                    };
                    let available = node_actions(app.node).contains(&act) || item_actions(app.node).contains(&act);
                    let r = ui
                        .add_enabled(available && app.act_enabled(act), egui::Button::new(icon))
                        .on_hover_text(&tip)
                        .on_disabled_hover_text(&tip);
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &tip));
                    if r.clicked() {
                        app.run_act(act);
                    }
                };
                tool(ui, self, "⟳", Act::Refresh);
                tool(ui, self, "➕", Act::New);
                tool(ui, self, "🗋", Act::Properties);
                tool(ui, self, "🗑", Act::Delete);
                ui.separator();
                let label = ui.label("Filter:");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .id(egui::Id::new("filter"))
                        .hint_text(format!("Type to filter {}…", self.node.title().to_lowercase()))
                        .desired_width(260.0),
                ).labelled_by(label.id);
                if self.focus_filter {
                    r.request_focus();
                    self.focus_filter = false;
                }
                if r.changed() {
                    self.view_dirty = true;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.set_sel(0);
                }
                if !self.filter.is_empty() && ui.button("✖").on_hover_text("Clear filter").clicked() {
                    self.filter.clear();
                    self.view_dirty = true;
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.busy > 0 {
                    ui.spinner();
                }
                ui.label(&self.status);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.signed_in {
                        ui.label(format!("👤 {}", self.account));
                    } else {
                        ui.label("Not signed in");
                    }
                    ui.separator();
                    if self.node.endpoint().is_some() {
                        let total = self.data.get(&self.node).map_or(0, Vec::len);
                        if self.view.len() == total {
                            ui.label(format!("{total} {}", self.node.title().to_lowercase()));
                        } else {
                            ui.label(format!("{} of {total} shown", self.view.len()));
                        }
                    }
                });
            });
        });
    }

    fn tree_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("tree").resizable(true).default_size(230.0).min_size(160.0).show(ui, |ui| {
            pane_heading(ui, "Console tree", self.pane == Pane::Tree);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let visible: Vec<(Node, usize)> = TREE
                    .iter()
                    .copied()
                    .filter(|(n, _)| {
                        let mut p = n.parent();
                        while let Some(pn) = p {
                            if !self.expanded.contains(&pn) {
                                return false;
                            }
                            p = pn.parent();
                        }
                        true
                    })
                    .collect();
                let mut action: Option<(Node, Key)> = None;
                for (idx, &(node, depth)) in visible.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * 16.0);
                        let marker = if !node.has_children() {
                            "  "
                        } else if self.expanded.contains(&node) {
                            "⏷"
                        } else {
                            "⏵"
                        };
                        let title = if node == Node::Root && !self.tenant_name.is_empty() {
                            format!("{} ({})", node.title(), self.tenant_name)
                        } else {
                            node.title().to_string()
                        };
                        let r = ui.selectable_label(self.node == node, format!("{marker} {} {title}", node.icon()));
                        r.widget_info(|| {
                            let state = match (node.has_children(), self.expanded.contains(&node)) {
                                (false, _) => "",
                                (true, true) => ", expanded",
                                (true, false) => ", collapsed",
                            };
                            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, self.node == node, format!("{title}{state}, level {}", depth + 1))
                        });
                        if r.clicked() {
                            self.select_node(node);
                            self.pane = Pane::Tree;
                        }
                        if r.double_clicked() && node.has_children() {
                            action = Some((node, Key::Space));
                        }
                        if self.node == node && self.focus_pane == Some(Pane::Tree) {
                            r.request_focus();
                            r.scroll_to_me(None);
                            self.focus_pane = None;
                        }
                        if r.has_focus() {
                            self.pane = Pane::Tree;
                            ui.memory_mut(|m| m.set_focus_lock_filter(r.id, ARROWS_LOCKED));
                            if self.node != node {
                                self.select_node(node);
                            }
                            for key in [Key::ArrowUp, Key::ArrowDown, Key::ArrowLeft, Key::ArrowRight, Key::Enter, Key::Home, Key::End] {
                                if ui.input(|i| i.key_pressed(key) && !i.modifiers.any()) {
                                    action = Some((node, key));
                                }
                            }
                        }
                    });
                    let _ = idx;
                }
                if let Some((node, key)) = action {
                    let pos = visible.iter().position(|(n, _)| *n == node).unwrap_or(0);
                    let goto = |app: &mut App, n: Node| {
                        app.select_node(n);
                        app.focus_pane = Some(Pane::Tree);
                    };
                    match key {
                        Key::ArrowUp if pos > 0 => goto(self, visible[pos - 1].0),
                        Key::ArrowDown if pos + 1 < visible.len() => goto(self, visible[pos + 1].0),
                        Key::Home => goto(self, visible[0].0),
                        Key::End => goto(self, visible[visible.len() - 1].0),
                        Key::ArrowRight if node.has_children() => {
                            if !self.expanded.insert(node) {
                                goto(self, visible[pos + 1].0);
                            }
                        }
                        Key::ArrowLeft => {
                            if !(node.has_children() && self.expanded.remove(&node))
                                && let Some(p) = node.parent() {
                                    goto(self, p);
                                }
                        }
                        Key::Space => {
                            if !self.expanded.remove(&node) {
                                self.expanded.insert(node);
                            }
                        }
                        Key::Enter => {
                            self.pane = Pane::List;
                            self.focus_pane = Some(Pane::List);
                            if self.sel.is_none() && !self.view.is_empty() {
                                self.sel = Some(0);
                            }
                        }
                        _ => {}
                    }
                }
            });
        });
    }

    fn all_actions(&self) -> Vec<Act> {
        let mut acts = node_actions(self.node);
        if self.selected().is_some() {
            acts.extend(item_actions(self.node));
        }
        acts
    }

    /// A menu-style button for an action, with its shortcut. Runs the action when clicked.
    pub fn action_button(&mut self, ui: &mut egui::Ui, act: Act) -> egui::Response {
        let shortcut = act
            .shortcuts()
            .first()
            .map(|s| ui.ctx().format_shortcut(s))
            .unwrap_or_default();
        let r = ui.add_enabled(
            self.act_enabled(act),
            egui::Button::new(self.act_label(act)).shortcut_text(shortcut).frame(false),
        );
        if r.clicked() {
            self.run_act(act);
        }
        r
    }

    fn actions_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("actions").resizable(true).default_size(230.0).min_size(170.0).show(ui, |ui| {
            pane_heading(ui, "Actions", self.pane == Pane::Actions);
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                    let mut first = true;
                    let mut any_focus = false;
                    section(ui, self.node.title());
                    for act in node_actions(self.node) {
                        let r = self.action_button(ui, act);
                        any_focus |= r.has_focus();
                        if first && self.focus_pane == Some(Pane::Actions) {
                            r.request_focus();
                            self.focus_pane = None;
                        }
                        first = false;
                    }
                    if let Some(obj) = self.selected() {
                        let name = self.node.object_name(obj);
                        ui.add_space(6.0);
                        section(ui, if name.is_empty() { "Selected item" } else { &name });
                        for act in item_actions(self.node) {
                            let r = self.action_button(ui, act);
                            any_focus |= r.has_focus();
                            if first && self.focus_pane == Some(Pane::Actions) {
                                r.request_focus();
                                self.focus_pane = None;
                            }
                            first = false;
                        }
                    }
                    if any_focus {
                        self.pane = Pane::Actions;
                    }
                });
            });
        });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let heading = if self.node == Node::Root && !self.tenant_name.is_empty() {
                format!("{} — {}", self.node.title(), self.tenant_name)
            } else {
                self.node.title().to_string()
            };
            pane_heading(ui, &heading, self.pane == Pane::List);
            if !self.signed_in && self.node.endpoint().is_some() {
                ui.add_space(12.0);
                ui.label("You are not signed in.");
                if ui.button("Sign in…").clicked() {
                    self.sign_in();
                }
                return;
            }
            if let Some(err) = self.errors.get(&self.node).cloned() {
                ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {err}"));
                if ui.button("Retry").clicked() {
                    self.refresh();
                }
                return;
            }
            if self.loading.contains(&self.node) && !self.data.contains_key(&self.node) {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Loading {}…", self.node.title().to_lowercase()));
                });
                return;
            }
            self.result_table(ui);
        });
    }

    fn result_table(&mut self, ui: &mut egui::Ui) {
        let cols = self.node.columns();
        let empty = Vec::new();
        let rows = self.data.get(&self.node).unwrap_or(&empty);
        if self.view.is_empty() {
            ui.add_space(8.0);
            ui.label(if self.filter.is_empty() { "There are no items to show in this view." } else { "No items match the filter." });
            return;
        }
        let text_height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        let mut table = TableBuilder::new(ui)
            .id_salt(("results", self.node))
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .auto_shrink(false);
        for (i, c) in cols.iter().enumerate() {
            let col = Column::initial(c.width).at_least(40.0).clip(true);
            table = table.column(if i + 1 == cols.len() { Column::remainder().at_least(c.width) } else { col });
        }
        if self.scroll_to_sel {
            if let Some(s) = self.sel {
                table = table.scroll_to_row(s, None);
            }
            self.scroll_to_sel = false;
        }

        let mut sort_click = None;
        let mut clicked = None;
        let mut open = None;
        let mut key_action: Option<Key> = None;
        let mut typed: Option<String> = None;
        let mut menu_act: Option<Act> = None;
        let mut got_focus = false;
        let item_acts: Vec<(Act, String, bool)> = item_actions(self.node)
            .into_iter()
            .map(|a| (a, self.act_label(a), self.act_enabled(a) || a.needs_selection() && self.signed_in))
            .collect();
        let sort = self.sort;
        let sel = self.sel;
        let want_focus = self.focus_pane == Some(Pane::List);
        let mut focus_taken = false;
        let view = &self.view;

        table
            .header(text_height, |mut header| {
                for (i, c) in cols.iter().enumerate() {
                    header.col(|ui| {
                        let arrow = match sort {
                            Some((ci, true)) if ci == i => " ⏶",
                            Some((ci, false)) if ci == i => " ⏷",
                            _ => "",
                        };
                        let r = ui.add(egui::Button::new(RichText::new(format!("{}{arrow}", c.title)).bold()).frame(false));
                        if r.clicked() {
                            sort_click = Some(i);
                        }
                        r.on_hover_text("Sort by this column");
                    });
                }
            })
            .body(|body| {
                body.rows(text_height, view.len(), |mut row| {
                    let pos = row.index();
                    let obj = &rows[view[pos]];
                    let is_sel = sel == Some(pos);
                    row.set_selected(is_sel);
                    for (ci, c) in cols.iter().enumerate() {
                        let text = (c.get)(obj);
                        row.col(|ui| {
                            if ci == 0 {
                                let r = ui.add(egui::Button::selectable(is_sel, &text).frame_when_inactive(false));
                                // Announce the whole row to screen readers.
                                r.widget_info(|| {
                                    let summary: Vec<String> = cols
                                        .iter()
                                        .map(|c| format!("{}: {}", c.title, (c.get)(obj)))
                                        .collect();
                                    egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is_sel, summary.join(", "))
                                });
                                if is_sel && want_focus {
                                    r.request_focus();
                                    focus_taken = true;
                                }
                                if r.gained_focus() && !is_sel {
                                    clicked = Some(pos);
                                }
                                if r.has_focus() {
                                    got_focus = true;
                                    ui.memory_mut(|m| m.set_focus_lock_filter(r.id, ARROWS_LOCKED));
                                    for key in [Key::ArrowUp, Key::ArrowDown, Key::PageUp, Key::PageDown, Key::Home, Key::End, Key::Enter] {
                                        if ui.input(|i| i.key_pressed(key) && !i.modifiers.any()) {
                                            key_action = Some(key);
                                        }
                                    }
                                    ui.input(|i| {
                                        for e in &i.events {
                                            if let Event::Text(t) = e {
                                                typed = Some(t.clone());
                                            }
                                        }
                                    });
                                }
                                if r.clicked() {
                                    clicked = Some(pos);
                                }
                                if r.double_clicked() {
                                    open = Some(pos);
                                }
                            } else {
                                ui.add(egui::Label::new(text).truncate());
                            }
                        });
                    }
                    let resp = row.response();
                    if resp.clicked() {
                        clicked = Some(pos);
                    }
                    if resp.double_clicked() {
                        open = Some(pos);
                    }
                    if resp.secondary_clicked() {
                        clicked = Some(pos);
                    }
                    resp.context_menu(|ui| {
                        for (act, label, enabled) in &item_acts {
                            if ui.add_enabled(*enabled, egui::Button::new(label)).clicked() {
                                menu_act = Some(*act);
                                ui.close();
                            }
                        }
                    });
                });
            });

        if focus_taken {
            self.focus_pane = None;
        }
        if got_focus {
            self.pane = Pane::List;
        }
        if let Some(ci) = sort_click {
            self.sort = match self.sort {
                Some((c, true)) if c == ci => Some((ci, false)),
                _ => Some((ci, true)),
            };
            self.view_dirty = true;
        }
        if let Some(p) = clicked {
            self.sel = Some(p);
            self.pane = Pane::List;
        }
        if let Some(p) = open {
            self.sel = Some(p);
            self.run_act(if self.node.endpoint().is_some() { Act::Properties } else { Act::Open });
        }
        if let Some(act) = menu_act {
            self.run_act(act);
        }
        let cur = self.sel.unwrap_or(0);
        let page = 15;
        match key_action {
            Some(Key::ArrowUp) => self.set_sel(cur.saturating_sub(1)),
            Some(Key::ArrowDown) => self.set_sel(cur + 1),
            Some(Key::PageUp) => self.set_sel(cur.saturating_sub(page)),
            Some(Key::PageDown) => self.set_sel(cur + page),
            Some(Key::Home) => self.set_sel(0),
            Some(Key::End) => self.set_sel(usize::MAX),
            Some(Key::Enter) => {
                self.run_act(if self.node.endpoint().is_some() { Act::Properties } else { Act::Open })
            }
            _ => {}
        }
        // Type-ahead: jump to the next item whose name starts with the typed character.
        if let Some(t) = typed.filter(|t| t.trim().len() == 1) {
            let t = t.to_lowercase();
            let first = cols[0].get;
            let n = self.view.len();
            let found = (1..=n).map(|k| (cur + k) % n).find(|&p| {
                first(&rows_of(&self.data, self.node)[self.view[p]]).to_lowercase().starts_with(&t)
            });
            if let Some(p) = found {
                self.set_sel(p);
            }
        }
    }

    pub fn open_child(&mut self) {
        if let Some(obj) = self.selected() {
            let idx = obj["node"].as_u64().unwrap_or(0) as usize;
            let node = TREE[idx].0;
            if let Some(p) = node.parent() {
                self.expanded.insert(p);
            }
            self.select_node(node);
            self.focus_pane = Some(Pane::Tree);
        }
    }
}

fn rows_of(data: &HashMap<Node, Vec<Value>>, node: Node) -> &[Value] {
    data.get(&node).map(Vec::as_slice).unwrap_or(&[])
}

fn pane_heading(ui: &mut egui::Ui, text: &str, active: bool) {
    let mut rt = RichText::new(text).bold();
    if active {
        rt = rt.color(ui.visuals().selection.stroke.color).underline();
    }
    ui.add(egui::Label::new(rt).selectable(false));
    ui.separator();
}

fn section(ui: &mut egui::Ui, title: &str) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(title).bold()).truncate().selectable(false));
        });
}

/// Synthetic rows describing the children of a container node, as MMC shows them.
fn child_rows(node: Node) -> Vec<Value> {
    let describe = |n: Node| match n {
        Node::Users => "Create, edit, enable/disable and delete user accounts; reset passwords",
        Node::Groups => "Manage groups, membership, owners, licences and directory role assignments",
        Node::Devices => "View, enable/disable and delete registered devices",
        Node::Licences => "View subscriptions and assign or remove licences",
        _ => "",
    };
    TREE.iter()
        .enumerate()
        .filter(|(_, (n, _))| n.parent() == Some(node))
        .map(|(i, (n, _))| json!({ "id": format!("node-{i}"), "node": i, "name": n.title(), "description": describe(*n) }))
        .collect()
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(update) = self.rx.try_recv() {
            update(self);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.view_dirty {
            self.rebuild_view();
        }
        if self.dialogs.is_empty() {
            self.global_keys(&ctx);
        }
        self.menu_bar(ui);
        self.toolbar(ui);
        self.status_bar(ui);
        self.tree_panel(ui);
        if self.show_actions {
            self.actions_panel(ui);
        } else if self.pane == Pane::Actions {
            self.pane = Pane::List;
        }
        self.central(ui);
        self.show_dialogs(&ctx);
        self.run_pending_searches();
        if self.view_dirty {
            ctx.request_repaint();
        }
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
