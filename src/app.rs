//! Main window: menu bar, toolbar, console tree, result pane, Actions pane and status bar.

use crate::auth::{Api, Auth, NeedsConsent, NeedsScopes};
use crate::config::Settings;
use crate::dialogs::{Dialog, DialogEntry};
use crate::graph::Graph;
use crate::i18n::{t, tf};
use crate::listview::ListView;
use crate::model::{Node, Source, TREE};
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

/// Who is signed in, as far as the app needs to know.
const ME: &str = "/me?$select=displayName,userPrincipalName";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Tree,
    List,
    Actions,
}

/// What a context menu is about, which decides the actions it offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuFor {
    /// A node in the console tree.
    Node,
    /// The results list, away from any object in it.
    List,
    /// The selected object in the results list.
    Item,
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
    /// Bumped to drop the result of an approval of more permissions that was cancelled.
    approval_gen: u64,
    /// Approving the Exchange Online permission has been offered this session.
    exchange_offered: bool,
    /// Graph scopes an administrator has been asked to approve this session.
    scopes_offered: HashSet<String>,
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
    /// The current node's objects as the results list shows them.
    list: ListView,

    pane: Pane,
    focus_pane: Option<Pane>,
    scroll_to_sel: bool,
    focus_filter: bool,
    file_menu_id: Option<egui::Id>,
    show_actions: bool,
    /// Show this app and Microsoft's command-line tools in Applications (see `model::is_admin_tool`).
    show_admin_tools: bool,

    pub dialogs: Vec<DialogEntry>,
    pub closed_dialogs: Vec<u64>,
    pub pending_picker_search: Vec<(u64, String)>,
    next_id: u64,
    busy: usize,
    pub status: String,
    /// Translating the interface (see `language`).
    pub translation: Option<Arc<crate::language::Job>>,
    /// Remote Servers' passwords and key passphrases, by server ID, kept only while the app runs.
    pub ssh_secrets: HashMap<String, String>,
    /// The open password prompt of each Remote Server that has one, by server ID.
    pub ssh_prompts: HashMap<String, u64>,
    quit: bool,
}

/// Runs `work`, turning a panic into an error, so whatever waits for the result still gets one
/// and the app isn't left busy for ever. The panic itself is logged by the hook in `logging`.
pub fn caught<T>(work: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|payload| {
        let what = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        Err(anyhow::anyhow!(tf("task.crashed", &[("error", &what)])))
    })
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
            approval_gen: 0,
            exchange_offered: false,
            scopes_offered: HashSet::new(),
            account: String::new(),
            tenant_name: String::new(),
            domains: Vec::new(),
            node: Node::Root,
            expanded: [Node::Root].into_iter().collect(),
            data: HashMap::new(),
            loading: HashSet::new(),
            errors: HashMap::new(),
            truncated: HashSet::new(),
            list: ListView::new(),
            pane: Pane::Tree,
            focus_pane: Some(Pane::Tree),
            scroll_to_sel: false,
            focus_filter: false,
            file_menu_id: None,
            show_actions: true,
            show_admin_tools: false,
            dialogs: Vec::new(),
            closed_dialogs: Vec::new(),
            pending_picker_search: Vec::new(),
            next_id: 1,
            busy: 0,
            status: t("status.ready"),
            translation: None,
            ssh_secrets: HashMap::new(),
            ssh_prompts: HashMap::new(),
            quit: false,
        };
        let (servers, problem) = crate::servers::load_rows();
        app.data.insert(Node::RemoteServers, servers);
        if let Some(problem) = problem {
            app.error(&t("server.load_title"), problem);
        }
        app.language_changed();
        if app.settings.is_complete() {
            app.connect();
        } else {
            app.open_settings();
        }
        if app.settings.update_check {
            app.check_for_updates(false);
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

    /// Brings text the app keeps, rather than draws afresh each frame, into the language now in
    /// use.
    pub fn language_changed(&mut self) {
        self.data.insert(Node::Root, child_rows(Node::Root));
        self.data.insert(Node::Exchange, child_rows(Node::Exchange));
        self.list.dirty = true;
    }

    pub fn error(&mut self, title: &str, err: impl std::fmt::Display) {
        log::error!("{title}: {err}");
        self.status = tf("status.failed", &[("title", &title)]);
        self.push(Dialog::message(title, &err.to_string()));
    }

    /// Reports a failed operation. When it failed for want of Graph permissions, offers (once a
    /// session) to have an administrator approve them instead.
    pub fn fail(&mut self, title: &str, err: anyhow::Error) {
        if let Some(NeedsScopes(missing)) = err.downcast_ref::<NeedsScopes>() {
            let missing = missing.clone();
            if self.offer_scopes(title, missing) {
                log::warn!("{title}: {err}");
                self.status = tf("status.needs_permissions", &[("title", &title)]);
                return;
            }
        }
        self.error(title, format!("{err:#}"));
    }

    /// Asks whether an administrator wants to approve `missing` scopes for `feature`, unless
    /// each of them was offered already this session. Returns whether it asked.
    pub fn offer_scopes(&mut self, feature: &str, missing: Vec<String>) -> bool {
        let mut new = false;
        for scope in &missing {
            new |= self.scopes_offered.insert(scope.to_ascii_lowercase());
        }
        if !new {
            return false;
        }
        let text = tf("scopes.offer_text", &[("feature", &feature), ("scopes", &missing.join(", "))]);
        let label = feature.to_string();
        self.push(Dialog::confirm(&t("scopes.approve_title"), &text, move |app| app.approve_scopes(Api::Graph, missing, &label)));
        true
    }

    /// Saves scopes the sign-in was granted without anyone signing in again (see
    /// [`Auth::ensure_scopes`]) to the Scopes setting, so they are asked for from now on.
    pub fn sync_scopes(&mut self) {
        let Some(g) = &self.graph else { return };
        let current = g.auth.scopes();
        let wanted: Vec<&str> = current.split_whitespace().collect();
        let added = crate::config::missing_scopes(&self.settings.scopes, &wanted);
        if !added.is_empty() {
            self.save_scopes(&added);
        }
    }

    /// Runs `f` at the start of the next frame, when every dialog is back on the stack, so it can
    /// change a dialog other than the one being drawn (dialogs are detached while drawn).
    pub fn defer(&self, f: impl FnOnce(&mut App) + Send + 'static) {
        let _ = self.tx.send(Box::new(f));
        self.ctx.request_repaint();
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
            let result = caught(|| work(&graph));
            let _ = tx.send(Box::new(move |app: &mut App| {
                app.busy -= 1;
                if app.busy == 0 {
                    app.status = t("status.ready");
                }
                // `work` may have been granted more scopes (see `Auth::ensure_scopes`).
                app.sync_scopes();
                done(app, result);
            }));
            ctx.request_repaint();
        });
    }

    /// Runs `work` on a background thread without Graph, showing it in the status bar as
    /// [`App::spawn`] does, and applies `done` on the UI thread.
    pub fn spawn_offline<T: Send + 'static>(
        &mut self,
        label: impl Into<String>,
        work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut App, anyhow::Result<T>) + Send + 'static,
    ) {
        self.busy += 1;
        self.status = label.into();
        log::debug!("Task started: {}", self.status);
        self.spawn_plain(work, move |app, result| {
            app.busy -= 1;
            if app.busy == 0 {
                app.status = t("status.ready");
            }
            done(app, result);
        });
    }

    /// Runs `work` on a background thread without Graph, applying `done` on the UI thread.
    pub fn spawn_plain<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut App, anyhow::Result<T>) + Send + 'static,
    ) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let result = caught(work);
            let _ = tx.send(Box::new(move |app: &mut App| done(app, result)));
            ctx.request_repaint();
        });
    }

    fn spawn_auth<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&Auth) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut App, anyhow::Result<T>) + Send + 'static,
    ) {
        let Some(graph) = self.graph.clone() else { return };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let result = caught(|| work(&graph.auth));
            let _ = tx.send(Box::new(move |app: &mut App| done(app, result)));
            ctx.request_repaint();
        });
    }

    // ----- sign-in --------------------------------------------------------

    pub fn connect(&mut self) {
        let http = match reqwest::blocking::Client::builder()
            .user_agent(concat!("StormCloud/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(http) => http,
            // Such as when the system's TLS set-up can't be loaded: the rest of the app still works.
            Err(e) => return self.error(&t("signin.title"), format!("{e:#}")),
        };
        let auth = Arc::new(Auth::new(self.settings.clone(), http.clone()));
        self.graph = Some(Arc::new(Graph::new(auth, http, &self.settings.graph_endpoint, &self.settings.exchange_endpoint)));
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
        self.list.forget_objects();
    }

    pub fn sign_in(&mut self) {
        let Some(graph) = self.graph.clone() else {
            self.open_settings();
            return;
        };
        self.signin_gen += 1;
        let attempt = self.signin_gen;
        if graph.auth.has_refresh_token() {
            self.status = t("status.signing_in");
            self.spawn(t("status.signing_in"), |g| g.get(ME), move |app, r| {
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
        let current = move |app: &App| attempt == app.signin_gen;
        self.run_device_code(Api::Graph, Vec::new(), false, current, |app, r| match r {
            Ok(()) => app.load_profile(),
            Err(e) => app.error(&t("signin.title"), e),
        });
    }

    /// A device code sign-in asking for the scopes on `api` plus `extra`: shows the code, waits
    /// for it to be entered, then gives `done` the outcome. `approval` when it is only to approve
    /// more permissions. Nothing more is done once `current` says the attempt was overtaken, by
    /// being cancelled or by signing out.
    fn run_device_code(
        &mut self,
        api: Api,
        extra: Vec<String>,
        approval: bool,
        current: impl Fn(&App) -> bool + Copy + Send + 'static,
        done: impl FnOnce(&mut App, anyhow::Result<()>) + Send + 'static,
    ) {
        let title = t(if approval { "scopes.approve_title" } else { "signin.title" });
        self.status = t("status.requesting_code");
        self.spawn_auth(move |a| a.start_device_code(api, &extra), move |app, r| {
            if !current(app) {
                return;
            }
            let dc = match r {
                Ok(dc) => dc,
                Err(e) => return app.error(&title, e),
            };
            let id = app.push(Dialog::DeviceCode {
                message: dc.message.clone(),
                uri: dc.verification_uri.clone(),
                code: dc.user_code.clone(),
                approval,
                cancelled: dc.cancelled.clone(),
            });
            app.status = t(if approval { "status.waiting_approval" } else { "status.waiting_signin" });
            app.spawn_auth(
                move |a| a.poll_device_code(&dc),
                move |app, r| {
                    app.close_dialog(id);
                    if current(app) {
                        done(app, r);
                    }
                },
            );
        });
    }

    /// Reads who is signed in, and carries on as them.
    fn load_profile(&mut self) {
        self.spawn(t("status.loading_profile"), |g| g.get(ME), |app, r| match r {
            Ok(me) => app.signed_in_as(me),
            Err(e) => app.error(&t("signin.title"), e),
        });
    }

    pub fn cancel_sign_in(&mut self) {
        self.signin_gen += 1;
        self.status = t("status.signin_cancelled");
    }

    /// Adds the scopes the Applications node needs to a Scopes setting saved without them:
    /// quietly when they have already been granted, or else by asking, once, for an
    /// administrator to approve them.
    fn add_app_scopes(&mut self) {
        let missing = crate::config::missing_scopes(&self.settings.scopes, &crate::config::APP_SCOPES);
        if missing.is_empty() {
            return;
        }
        let session = self.signin_gen;
        let ask = missing.clone();
        self.spawn_auth(move |a| a.try_add_scopes(&ask), move |app, r| {
            if session != app.signin_gen {
                return;
            }
            match r {
                Ok(true) => app.save_scopes(&missing),
                Ok(false) if !app.settings.app_scopes_offered => app.offer_app_scopes(missing),
                Ok(false) => log::info!("The permissions for Applications have not been granted; not asking again"),
                Err(e) => log::warn!("Could not add the permissions for Applications: {e}"),
            }
        });
    }

    /// Saves `added` to the Scopes setting, once the sign-in has them.
    fn save_scopes(&mut self, added: &[String]) {
        self.settings.scopes = format!("{} {}", self.settings.scopes.trim(), added.join(" "));
        match self.settings.save() {
            Ok(_) => log::info!("Added to the saved scopes: {}", added.join(" ")),
            Err(e) => log::error!("Could not save the added scopes: {e}"),
        }
        self.status = t("status.permissions_added");
    }

    fn offer_app_scopes(&mut self, missing: Vec<String>) {
        self.settings.app_scopes_offered = true;
        if let Err(e) = self.settings.save() {
            log::error!("Could not save settings: {e}");
        }
        let text = tf("scopes.apps_text", &[("scopes", &missing.join(", "))]);
        self.push(Dialog::confirm(&t("scopes.apps_title"), &text, move |app| {
            app.approve_scopes(Api::Graph, missing, &t("node.applications"))
        }));
    }

    /// Offers, once a session, to approve the Exchange Online permission the Exchange node needs.
    pub(crate) fn offer_exchange_consent(&mut self) {
        if self.exchange_offered {
            return;
        }
        self.exchange_offered = true;
        self.push(Dialog::confirm(&t("scopes.exchange_title"), &t("scopes.exchange_text"), |app| {
            app.approve_scopes(Api::Exchange, Vec::new(), &t("node.exchange"))
        }));
    }

    /// A device code sign-in asking for the scopes on `api` plus `extra`, for an administrator to
    /// approve them. The sign-in so far is kept if it fails or is cancelled.
    fn approve_scopes(&mut self, api: Api, extra: Vec<String>, feature: &str) {
        self.approval_gen += 1;
        let (attempt, session) = (self.approval_gen, self.signin_gen);
        let current = move |app: &App| attempt == app.approval_gen && session == app.signin_gen;
        let feature = feature.to_string();
        let added = extra.clone();
        self.run_device_code(api, extra, true, current, move |app, r| match r {
            Ok(()) => {
                if !added.is_empty() {
                    if let Some(g) = &app.graph {
                        g.auth.add_scopes(&added);
                    }
                    app.save_scopes(&added);
                }
                // An administrator other than the person signed in may have approved them.
                app.load_profile();
            }
            Err(e) => app.error(&t("scopes.approve_title"), tf("scopes.not_added", &[("feature", &feature), ("error", &e)])),
        });
    }

    pub fn cancel_approval(&mut self) {
        self.approval_gen += 1;
        self.status = t("status.permissions_not_added");
    }

    fn signed_in_as(&mut self, me: Value) {
        self.signed_in = true;
        self.account = me["userPrincipalName"].as_str().unwrap_or("").to_string();
        log::info!("Signed in as {}", self.account);
        self.status = tf("status.signed_in_as", &[("account", &self.account)]);
        self.spawn(
            t("status.loading_tenant"),
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
                Err(e) => app.error(&t("tenant.load_title"), e),
            },
        );
        if self.node.endpoint().is_some() {
            self.refresh();
        }
        self.add_app_scopes();
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
        self.status = t("status.signed_out");
    }

    // ----- data -----------------------------------------------------------

    pub fn select_node(&mut self, node: Node) {
        if self.node == node {
            return;
        }
        log::debug!("Console node selected: {node:?}");
        self.node = node;
        self.list.reset();
        if node.endpoint().is_some() && !self.data.contains_key(&node) {
            self.load(node);
        }
    }

    pub fn refresh(&mut self) {
        if self.node == Node::RemoteServers {
            self.check_all_servers();
        } else if self.node.endpoint().is_some() {
            self.load(self.node);
        }
    }

    /// Reloads `node` in the background (if it has a Graph endpoint).
    pub fn load(&mut self, node: Node) {
        let Some(source) = node.endpoint() else { return };
        if !self.signed_in || self.loading.contains(&node) {
            return;
        }
        self.loading.insert(node);
        let session = self.signin_gen;
        self.spawn(
            tf("status.loading_list", &[("list", &node.list_name())]),
            move |g| match source {
                Source::Graph(path) => g.list_capped(path),
                Source::Exchange => crate::exchange::list(g, node),
            },
            move |app, r| {
                if session != app.signin_gen {
                    log::debug!("Dropping {node:?} loaded before signing out");
                    return;
                }
                app.loading.remove(&node);
                match r {
                    Ok((mut rows, truncated)) => {
                        rows.retain(|v| node.keep(v));
                        let first = node.columns()[0].get;
                        rows.sort_by_cached_key(|v| first(v).to_lowercase());
                        let list = node.list_name();
                        app.status = if truncated {
                            tf("status.loaded_truncated", &[("count", &rows.len()), ("list", &list)])
                        } else {
                            tf("status.loaded", &[("count", &rows.len()), ("list", &list)])
                        };
                        log::info!("{node:?}: {} loaded{}", rows.len(), if truncated { ", truncated" } else { "" });
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
                        if e.downcast_ref::<NeedsConsent>().is_some() {
                            app.offer_exchange_consent();
                        }
                        app.status = tf("status.load_failed", &[("list", &node.list_name())]);
                        log::error!("Loading {node:?} failed: {e}");
                        app.errors.insert(node, e.to_string());
                    }
                }
                if app.node == node {
                    app.list.dirty = true;
                }
            },
        );
    }

    /// Why the list for `node` may not hold every object, if it may not.
    pub fn incomplete_list(&self, node: Node) -> Option<String> {
        let list = node.list_name();
        if self.loading.contains(&node) {
            Some(tf("list.still_loading", &[("list", &list)]))
        } else if self.errors.contains_key(&node) {
            Some(tf("list.load_failed", &[("list", &list)]))
        } else if self.truncated.contains(&node) {
            Some(tf("list.truncated", &[("count", &crate::graph::MAX_ITEMS), ("list", &list)]))
        } else if !self.data.contains_key(&node) {
            Some(tf("list.not_loaded", &[("list", &list)]))
        } else {
            None
        }
    }

    /// Remembers the selection before the data for `node` changes, while the view still matches it.
    fn data_changing(&mut self, node: Node) {
        if self.node == node {
            self.list.objects_changing(rows_of(&self.data, node));
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
            self.list.dirty = true;
        }
    }

    pub fn remove_obj(&mut self, node: Node, id: &str) {
        self.data_changing(node);
        if let Some(rows) = self.data.get_mut(&node) {
            rows.retain(|r| r["id"] != id);
        }
        if self.node == node {
            self.list.dirty = true;
        }
    }

    /// Whether a row is left out of the list whatever the filter, as admin tools are by default.
    fn hidden(&self, v: &Value) -> bool {
        hidden(self.node, self.show_admin_tools, &self.settings.client_id, v)
    }

    fn rebuild_view(&mut self) {
        let (node, show_admin_tools, client_id) = (self.node, self.show_admin_tools, &self.settings.client_id);
        self.list.rebuild(rows_of(&self.data, node), &node.columns(), |v| hidden(node, show_admin_tools, client_id, v));
    }

    /// The objects in the result list, filtered and sorted as shown.
    pub fn view_rows(&self) -> Vec<&Value> {
        self.list.shown(rows_of(&self.data, self.node))
    }

    pub fn selected(&self) -> Option<&Value> {
        self.list.selected(rows_of(&self.data, self.node))
    }

    fn set_sel(&mut self, pos: usize) {
        if self.list.select(pos) {
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
            let what = match self.pane {
                Pane::Tree => MenuFor::Node,
                _ if self.selected().is_some() => MenuFor::Item,
                _ => MenuFor::List,
            };
            self.push(Dialog::ContextMenu(what));
        }
        if typing {
            // Shortcuts below would clash with editing text (e.g. Delete).
            if self.filter_has_focus(ctx) && pressed(sc(Modifiers::NONE, Key::ArrowDown)) {
                self.set_sel(self.list.sel.unwrap_or(0));
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
        if self.pane == Pane::List {
            self.list.select_first_if_none();
        }
        self.scroll_to_sel = true;
    }

    pub fn act_enabled(&self, act: Act) -> bool {
        if act.needs_selection() && self.selected().is_none() {
            return false;
        }
        // Remote Servers have nothing to do with the Microsoft sign-in.
        act == Act::Open || act == Act::Refresh && self.node.endpoint().is_some() || self.signed_in || self.node == Node::RemoteServers
    }

    // ----- panels ---------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                let file = ui.menu_button(t("menu.file"), |ui| {
                    if ui.button(t("menu.settings")).clicked() {
                        self.open_settings();
                    }
                    if ui.button(self.language_menu_label()).clicked() {
                        self.open_language();
                    }
                    ui.separator();
                    if self.signed_in {
                        if ui.button(t("menu.sign_out")).clicked() {
                            self.sign_out();
                        }
                    } else if ui.button(t("menu.sign_in")).clicked() {
                        self.sign_in();
                    }
                    ui.separator();
                    if ui.button(t("menu.exit")).clicked() {
                        self.quit = true;
                    }
                });
                self.file_menu_id = Some(file.response.id);
                ui.menu_button(t("menu.action"), |ui| {
                    let acts = self.all_actions();
                    if acts.is_empty() {
                        ui.label(t("menu.no_actions"));
                    }
                    for act in acts {
                        if self.action_button(ui, act).clicked() {
                            ui.close();
                        }
                    }
                });
                ui.menu_button(t("menu.view"), |ui| {
                    let can_refresh = self.signed_in || self.node == Node::RemoteServers;
                    if ui.add_enabled(can_refresh, egui::Button::new(t("act.refresh")).shortcut_text("F5")).clicked() {
                        self.refresh();
                    }
                    if ui.add(egui::Button::new(t("menu.filter")).shortcut_text(ui.ctx().format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F)))).clicked() {
                        self.focus_filter = true;
                    }
                    ui.separator();
                    ui.checkbox(&mut self.show_actions, t("menu.actions_pane"));
                    if ui.checkbox(&mut self.show_admin_tools, t("menu.show_admin_tools")).changed() {
                        self.list.dirty = true;
                    }
                    ui.separator();
                    ui.label(t("menu.theme"));
                    egui::widgets::global_theme_preference_buttons(ui);
                    ui.separator();
                    ui.label(t("menu.zoom"));
                });
                ui.menu_button(t("menu.help"), |ui| {
                    if ui.add(egui::Button::new(t("menu.shortcuts")).shortcut_text("F1")).clicked() {
                        self.push(Dialog::Help);
                    }
                    if ui.button(t("menu.check_updates")).clicked() {
                        self.check_for_updates(true);
                    }
                    if ui.button(t("menu.about")).clicked() {
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
                let label = ui.label(t("toolbar.filter"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.list.filter)
                        .id(egui::Id::new("filter"))
                        .hint_text(tf("toolbar.filter_hint", &[("list", &self.node.list_name())]))
                        .desired_width(260.0),
                ).labelled_by(label.id);
                if self.focus_filter {
                    r.request_focus();
                    self.focus_filter = false;
                }
                if r.changed() {
                    self.list.dirty = true;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.set_sel(0);
                }
                if !self.list.filter.is_empty() && ui.button("✖").on_hover_text(t("toolbar.clear_filter")).clicked() {
                    self.list.filter.clear();
                    self.list.dirty = true;
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.busy > 0 || self.translation.is_some() {
                    ui.spinner();
                }
                match &self.translation {
                    Some(job) if self.busy == 0 => {
                        let percent = (job.fraction() * 100.0).round() as u32;
                        ui.label(tf("language.progress", &[("percent", &percent)]));
                    }
                    _ => {
                        ui.label(&self.status);
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.signed_in {
                        ui.label(format!("👤 {}", self.account));
                    } else {
                        ui.label(t("status.not_signed_in"));
                    }
                    ui.separator();
                    if self.node.is_list() {
                        let total = self.data.get(&self.node).map_or(0, |rows| rows.iter().filter(|v| !self.hidden(v)).count());
                        if self.list.len() == total {
                            ui.label(tf("status.count", &[("count", &total), ("list", &self.node.list_name())]));
                        } else {
                            ui.label(tf("status.shown", &[("shown", &self.list.len()), ("total", &total)]));
                        }
                    }
                });
            });
        });
    }

    fn tree_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("tree").resizable(true).default_size(230.0).min_size(160.0).show(ui, |ui| {
            pane_heading(ui, &t("pane.tree"), self.pane == Pane::Tree);
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
                            node.title()
                        };
                        let r = ui.selectable_label(self.node == node, format!("{marker} {} {title}", node.icon()));
                        r.widget_info(|| {
                            let key = match (node.has_children(), self.expanded.contains(&node)) {
                                (false, _) => "pane.tree_item",
                                (true, true) => "pane.tree_item_expanded",
                                (true, false) => "pane.tree_item_collapsed",
                            };
                            let text = tf(key, &[("title", &title), ("level", &(depth + 1))]);
                            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, self.node == node, text)
                        });
                        if r.clicked() || r.secondary_clicked() {
                            self.select_node(node);
                            self.pane = Pane::Tree;
                        }
                        self.context_menus(std::slice::from_ref(&r), MenuFor::Node);
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
                            self.list.select_first_if_none();
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

    /// The contents of a right-click menu, or the Shift+F10 menu: everything the keyboard can do
    /// to `what`, with its shortcut. Runs whichever is clicked, and returns the buttons.
    pub fn context_menu_ui(&mut self, ui: &mut egui::Ui, what: MenuFor) -> Vec<egui::Response> {
        let mut buttons = Vec::new();
        if what == MenuFor::Node && self.node.has_children() {
            let expanded = self.expanded.contains(&self.node);
            let (label, key) = if expanded { ("menu.collapse", "←") } else { ("menu.expand", "→") };
            let r = ui.add(egui::Button::new(t(label)).shortcut_text(key).frame(false));
            if r.clicked() && !self.expanded.remove(&self.node) {
                self.expanded.insert(self.node);
            }
            buttons.push(r);
        }
        let acts = match what {
            MenuFor::Node | MenuFor::List => node_actions(self.node),
            MenuFor::Item => item_actions(self.node),
        };
        if !acts.is_empty() && !buttons.is_empty() {
            ui.separator();
        }
        for act in acts {
            buttons.push(self.action_button(ui, act));
        }
        if what == MenuFor::List && self.node.is_list() {
            if !buttons.is_empty() {
                ui.separator();
            }
            let shortcut = ui.ctx().format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F));
            let r = ui.add(egui::Button::new(t("menu.filter")).shortcut_text(shortcut).frame(false));
            if r.clicked() {
                self.focus_filter = true;
            }
            buttons.push(r);
        }
        if buttons.is_empty() {
            ui.label(t("menu.no_actions"));
        }
        buttons
    }

    /// Shows the right-click menu for `what` on each of `targets`.
    fn context_menus(&mut self, targets: &[egui::Response], what: MenuFor) {
        for target in targets {
            target.context_menu(|ui| {
                if self.context_menu_ui(ui, what).iter().any(|b| b.clicked()) {
                    ui.close();
                }
            });
        }
    }

    fn actions_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("actions").resizable(true).default_size(230.0).min_size(170.0).show(ui, |ui| {
            pane_heading(ui, &t("pane.actions"), self.pane == Pane::Actions);
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                    let mut first = true;
                    let mut any_focus = false;
                    section(ui, &self.node.title());
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
                        section(ui, &if name.is_empty() { t("pane.selected_item") } else { name });
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
            // Behind everything else in the pane, so it gets the right-clicks that miss the rows.
            // Not focusable, as the menu is also on Shift+F10.
            let background = ui.interact(ui.max_rect(), ui.id().with("list-menu"), Sense::CLICK);
            self.results(ui);
            self.context_menus(&[background], MenuFor::List);
        });
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let heading = if self.node == Node::Root && !self.tenant_name.is_empty() {
            format!("{} — {}", self.node.title(), self.tenant_name)
        } else {
            self.node.title()
        };
        pane_heading(ui, &heading, self.pane == Pane::List);
        if !self.signed_in && self.node.endpoint().is_some() {
            ui.add_space(12.0);
            ui.label(t("central.not_signed_in"));
            if ui.button(t("central.sign_in")).clicked() {
                self.sign_in();
            }
            return;
        }
        if let Some(err) = self.errors.get(&self.node).cloned() {
            ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {err}"));
            if ui.button(t("central.retry")).clicked() {
                self.refresh();
            }
            return;
        }
        if self.loading.contains(&self.node) && !self.data.contains_key(&self.node) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tf("status.loading_list", &[("list", &self.node.list_name())]));
            });
            return;
        }
        self.result_table(ui);
    }

    fn result_table(&mut self, ui: &mut egui::Ui) {
        // The tree or a menu may have changed node earlier this frame, after the view was last built.
        if self.list.dirty {
            self.rebuild_view();
        }
        let cols = self.node.columns();
        let empty = Vec::new();
        let rows = self.data.get(&self.node).unwrap_or(&empty);
        if self.list.is_empty() {
            ui.add_space(8.0);
            ui.label(t(if self.list.filter.is_empty() { "central.empty" } else { "central.no_match" }));
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
            if let Some(s) = self.list.sel {
                table = table.scroll_to_row(s, None);
            }
            self.scroll_to_sel = false;
        }

        let mut sort_click = None;
        let mut clicked = None;
        let mut open = None;
        let mut key_action: Option<Key> = None;
        let mut typed: Option<String> = None;
        // Everything in the rows that a right-click can land on.
        let mut menu_targets: Vec<egui::Response> = Vec::new();
        let mut got_focus = false;
        let sort = self.list.sort;
        let sel = self.list.sel;
        let want_focus = self.focus_pane == Some(Pane::List);
        let mut focus_taken = false;
        let view = self.list.rows();
        let titles: Vec<String> = cols.iter().map(|c| t(c.title)).collect();

        table
            .header(text_height, |mut header| {
                for (i, title) in titles.iter().enumerate() {
                    header.col(|ui| {
                        let arrow = match sort {
                            Some((ci, true)) if ci == i => " ⏶",
                            Some((ci, false)) if ci == i => " ⏷",
                            _ => "",
                        };
                        let r = ui.add(egui::Button::new(RichText::new(format!("{title}{arrow}")).bold()).frame(false));
                        if r.clicked() {
                            sort_click = Some(i);
                        }
                        r.on_hover_text(t("central.sort"));
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
                                        .zip(&titles)
                                        .map(|(c, title)| format!("{title}: {}", (c.get)(obj)))
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
                                if r.clicked() || r.secondary_clicked() {
                                    clicked = Some(pos);
                                }
                                if r.double_clicked() {
                                    open = Some(pos);
                                }
                                menu_targets.push(r);
                            } else {
                                // Not selectable, so clicks on the text reach the row.
                                ui.add(egui::Label::new(text).truncate().selectable(false));
                            }
                        });
                    }
                    let resp = row.response();
                    if resp.clicked() || resp.secondary_clicked() {
                        clicked = Some(pos);
                    }
                    if resp.double_clicked() {
                        open = Some(pos);
                    }
                    menu_targets.push(resp);
                });
            });

        if focus_taken {
            self.focus_pane = None;
        }
        if got_focus {
            self.pane = Pane::List;
        }
        if let Some(ci) = sort_click {
            self.list.toggle_sort(ci);
        }
        if let Some(p) = clicked {
            self.list.sel = Some(p);
            self.pane = Pane::List;
        }
        if let Some(p) = open {
            self.list.sel = Some(p);
            self.run_act(if self.node.is_list() { Act::Properties } else { Act::Open });
        }
        // After the selection, so a right-clicked row's menu is about that row from the start.
        self.context_menus(&menu_targets, MenuFor::Item);
        let cur = self.list.sel.unwrap_or(0);
        let page = 15;
        match key_action {
            Some(Key::ArrowUp) => self.set_sel(cur.saturating_sub(1)),
            Some(Key::ArrowDown) => self.set_sel(cur + 1),
            Some(Key::PageUp) => self.set_sel(cur.saturating_sub(page)),
            Some(Key::PageDown) => self.set_sel(cur + page),
            Some(Key::Home) => self.set_sel(0),
            Some(Key::End) => self.set_sel(usize::MAX),
            Some(Key::Enter) => {
                self.run_act(if self.node.is_list() { Act::Properties } else { Act::Open })
            }
            _ => {}
        }
        // Type-ahead: jump to the next item whose name starts with the typed character.
        if let Some(t) = typed.filter(|t| t.trim().len() == 1)
            && let Some(p) = self.list.next_starting_with(rows_of(&self.data, self.node), cols[0].get, &t) {
                self.set_sel(p);
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

/// Whether a row of `node` is left out of the list whatever the filter, as admin tools are unless
/// `show_admin_tools`.
fn hidden(node: Node, show_admin_tools: bool, client_id: &str, v: &Value) -> bool {
    node == Node::Applications && !show_admin_tools && crate::model::is_admin_tool(v, client_id)
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
        Node::Users => t("node.users_about"),
        Node::Groups => t("node.groups_about"),
        Node::Devices => t("node.devices_about"),
        Node::Applications => t("node.applications_about"),
        Node::RemoteServers => t("node.remote_servers_about"),
        Node::Exchange => t("node.exchange_about"),
        Node::Mailboxes => t("node.mailboxes_about"),
        Node::DistributionLists => t("node.distribution_lists_about"),
        Node::Licences => t("node.licences_about"),
        Node::Root => String::new(),
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
        if self.list.dirty {
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
        if self.list.dirty {
            ctx.request_repaint();
        }
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
