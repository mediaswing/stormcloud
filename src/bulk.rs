//! CSV export of the result list, and bulk imports from CSV: new users, and members for a group.
//!
//! An import reads the whole file first and shows every row in a preview, with the problems that
//! will make it skip a row. Nothing is sent to Microsoft Graph until the preview is confirmed.

use crate::app::{App, Bold};
use crate::dialogs::Dialog;
use crate::graph::{api_error, enc, odata_str, ApiError};
use crate::i18n::{self, t, tf};
use crate::model::{Node, s};
use crate::ops::{generate_password, is_guid};
use anyhow::Context;
use eframe::egui::{self, Align, Layout, RichText};
use egui_extras::{Column, TableBuilder};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Columns a user import understands, by Graph property, with the header names accepted for each.
/// Headers are compared lower-case with everything but letters and digits removed.
const USER_COLUMNS: &[(&str, &[&str])] = &[
    ("displayName", &["displayname", "name", "fullname"]),
    ("userPrincipalName", &["userprincipalname", "upn", "username", "signinname"]),
    ("password", &["password", "initialpassword"]),
    ("givenName", &["givenname", "firstname"]),
    ("surname", &["surname", "lastname", "familyname"]),
    ("jobTitle", &["jobtitle"]),
    ("department", &["department"]),
    ("usageLocation", &["usagelocation"]),
    ("mailNickname", &["mailnickname", "alias"]),
    ("accountEnabled", &["accountenabled", "enabled"]),
    ("forceChange", &["forcechangepassword", "forcechangepasswordnextsignin", "mustchangepassword"]),
];

/// Headers accepted for the column naming the users to add to a group.
const MEMBER_COLUMNS: &[&str] =
    &["userprincipalname", "upn", "username", "mail", "email", "emailaddress", "id", "objectid"];


// ----- reading and writing CSV ------------------------------------------------

pub struct Table {
    pub headers: Vec<String>,
    pub header_line: u64,
    /// Each row with its line number in the file.
    pub rows: Vec<(u64, Vec<String>)>,
}

/// Reads a CSV file as saved by Excel or a text editor: UTF-8 (with or without a byte order
/// mark), UTF-16 ("Unicode Text") or Windows-1252, separated by commas, semicolons or tabs,
/// whichever the header line uses most.
pub fn parse_table(bytes: &[u8]) -> Result<Table, String> {
    let text = decode(bytes);
    let first = text.lines().next().unwrap_or("");
    let mut delimiter = b',';
    let mut most = first.matches(',').count();
    for d in *b";\t" {
        let n = first.bytes().filter(|&b| b == d).count();
        if n > most {
            delimiter = d;
            most = n;
        }
    }
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let mut records = Vec::new();
    let bytes = text.as_bytes();
    // Line breaks counted so far, up to `counted`, so each record only counts its own share.
    let (mut counted, mut breaks) = (0, 0u64);
    for record in reader.records() {
        let record = record.map_err(|e| tf("csv.invalid", &[("error", &e)]))?;
        // From the byte offset, as the reader's own line count is off with Windows line endings
        // and blank lines: its offset can point at the line breaks before the record.
        let mut start = record.position().map_or(0, |p| p.byte() as usize);
        while start < bytes.len() && matches!(bytes[start], b'\r' | b'\n') {
            start += 1;
        }
        breaks += bytes[counted.min(start)..start].iter().filter(|&&b| b == b'\n').count() as u64;
        counted = start;
        let line = breaks + 1;
        let cells: Vec<String> = record.iter().map(unescape_cell).collect();
        if cells.iter().any(|c| !c.is_empty()) {
            records.push((line, cells));
        }
    }
    if records.is_empty() {
        return Err(t("csv.empty"));
    }
    let (header_line, headers) = records.remove(0);
    Ok(Table { headers, header_line, rows: records })
}

fn decode(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(b"\xFF\xFE") {
        let units: Vec<u16> = utf16.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        // Excel's "CSV (Comma delimited)" is saved in the Windows code page.
        Err(_) => bytes.iter().map(|&b| windows_1252(b)).collect(),
    }
}

fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    if (0x80..0xA0).contains(&b) { HIGH[usize::from(b - 0x80)] } else { char::from(b) }
}

/// Spreadsheets run cells starting with these characters as formulas.
const FORMULA_START: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// Stops a value such as a display name of `=HYPERLINK(…)` running as a formula when the
/// exported file is opened in a spreadsheet, the usual way: with a leading apostrophe.
fn escape_cell(cell: &str) -> String {
    if cell.starts_with(FORMULA_START) && cell.parse::<f64>().is_err() { format!("'{cell}") } else { cell.to_string() }
}

/// Reverses [`escape_cell`], so exported files can be imported again.
fn unescape_cell(cell: &str) -> String {
    match cell.strip_prefix('\'') {
        Some(rest) if rest.starts_with(FORMULA_START) => rest.to_string(),
        _ => cell.to_string(),
    }
}

/// CSV for Excel and other spreadsheets: UTF-8 with a byte order mark, so accented names survive.
pub fn csv_bytes(headers: &[&str], rows: &[Vec<String>]) -> anyhow::Result<Vec<u8>> {
    let mut writer = csv::Writer::from_writer(b"\xEF\xBB\xBF".to_vec());
    writer.write_record(headers)?;
    for row in rows {
        writer.write_record(row.iter().map(|c| escape_cell(c)))?;
    }
    writer.into_inner().map_err(|e| anyhow::anyhow!(tf("csv.write_failed", &[("error", e.error())])))
}

/// Lower case with everything but letters and digits removed, so "First name", "firstName" and
/// "first_name" all match.
fn header_key(header: &str) -> String {
    header.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase()
}

fn parse_yes_no(value: &str, default: bool) -> Result<bool, ()> {
    match value.to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "yes" | "y" | "true" | "1" => Ok(true),
        "no" | "n" | "false" | "0" => Ok(false),
        _ => Err(()),
    }
}

// ----- parsing imports --------------------------------------------------------

pub struct UserRow {
    pub line: u64,
    pub display_name: String,
    pub upn: String,
    pub password: String,
    pub generated_password: bool,
    /// The request body for `POST /users`.
    pub body: Value,
    pub error: Option<String>,
}

/// Turns a table into users to create. `domains` are the tenant's verified domains, the default
/// first; a user name without a domain gets the default one. `existing` holds the lower-case
/// UPNs of users already loaded, which are skipped.
pub fn parse_users(table: &Table, domains: &[String], existing: &HashSet<String>) -> Result<(Vec<UserRow>, Vec<String>), String> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut ignored = Vec::new();
    for (i, h) in table.headers.iter().enumerate() {
        let key = header_key(h);
        match USER_COLUMNS.iter().find(|(_, names)| names.contains(&key.as_str())) {
            Some((prop, _)) => {
                index.entry(prop).or_insert(i);
            }
            None if !h.is_empty() => ignored.push(h.clone()),
            None => {}
        }
    }
    for (prop, label) in [("displayName", "Display name"), ("userPrincipalName", "User principal name")] {
        if !index.contains_key(prop) {
            return Err(tf("csv.no_column", &[("column", &label), ("format", &t("csv.user_format"))]));
        }
    }
    let verified: HashSet<String> = domains.iter().map(|d| d.to_lowercase()).collect();
    let mut seen: HashMap<String, u64> = HashMap::new();
    let rows = table
        .rows
        .iter()
        .map(|(line, cells)| {
            let get = |prop: &str| index.get(prop).and_then(|&i| cells.get(i)).cloned().unwrap_or_default();
            let display_name = get("displayName");
            let mut upn = get("userPrincipalName");
            if !upn.is_empty() && !upn.contains('@') && !domains.is_empty() {
                upn = format!("{upn}@{}", domains[0]);
            }
            let (generated_password, password) = match get("password") {
                p if p.is_empty() => (true, generate_password()),
                p => (false, p),
            };
            let mut row = UserRow {
                line: *line,
                display_name: display_name.clone(),
                upn: upn.clone(),
                password: password.clone(),
                generated_password,
                body: Value::Null,
                error: None,
            };
            let (alias, domain) = upn.split_once('@').unwrap_or((upn.as_str(), ""));
            let nickname = match get("mailNickname") {
                n if n.is_empty() => alias.chars().filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c)).collect(),
                n => n,
            };
            let usage = get("usageLocation").to_uppercase();
            let enabled = parse_yes_no(&get("accountEnabled"), true);
            let force = parse_yes_no(&get("forceChange"), true);
            let error = if display_name.is_empty() {
                Some(t("csv.no_display_name"))
            } else if upn.is_empty() {
                Some(t("csv.no_upn"))
            } else if alias.is_empty() || domain.is_empty() || domain.contains('@') || upn.contains(char::is_whitespace) {
                Some(tf("csv.bad_upn", &[("upn", &upn)]))
            } else if !verified.is_empty() && !verified.contains(&domain.to_lowercase()) {
                Some(tf("csv.unverified_domain", &[("domain", &domain)]))
            } else if existing.contains(&upn.to_lowercase()) {
                Some(t("csv.user_exists"))
            } else if let Some(first) = seen.get(&upn.to_lowercase()) {
                Some(tf("csv.same_upn", &[("line", first)]))
            } else if password.chars().count() < 8 {
                Some(t("csv.short_password"))
            } else if !usage.is_empty() && (usage.len() != 2 || !usage.chars().all(|c| c.is_ascii_alphabetic())) {
                Some(tf("csv.bad_usage_location", &[("usage", &usage)]))
            } else if nickname.is_empty() {
                Some(t("csv.no_nickname"))
            } else if enabled.is_err() {
                Some(t("csv.bad_enabled"))
            } else if force.is_err() {
                Some(t("csv.bad_force_change"))
            } else {
                None
            };
            seen.entry(upn.to_lowercase()).or_insert(*line);
            if error.is_none() {
                let mut body = json!({
                    "accountEnabled": enabled.unwrap_or(true),
                    "displayName": display_name,
                    "mailNickname": nickname,
                    "userPrincipalName": upn,
                    "passwordProfile": {
                        "password": password,
                        "forceChangePasswordNextSignIn": force.unwrap_or(true),
                    },
                });
                for key in ["givenName", "surname", "jobTitle", "department"] {
                    let v = get(key);
                    if !v.is_empty() {
                        body[key] = json!(v);
                    }
                }
                if !usage.is_empty() {
                    body["usageLocation"] = json!(usage);
                }
                row.body = body;
            }
            row.error = error;
            row
        })
        .collect();
    Ok((rows, ignored))
}

pub struct MemberRow {
    pub line: u64,
    pub value: String,
    pub error: Option<String>,
}

/// The users named in a member import, by user principal name, email address or object ID.
pub fn parse_members(table: &Table) -> Result<Vec<MemberRow>, String> {
    let column = table.headers.iter().position(|h| MEMBER_COLUMNS.contains(&header_key(h).as_str()));
    let mut values: Vec<(u64, String)> = Vec::new();
    match column {
        Some(c) => {
            values.extend(table.rows.iter().map(|(line, cells)| (*line, cells.get(c).cloned().unwrap_or_default())));
        }
        // A plain list of names, with no header.
        None if table.headers.len() == 1 && (table.headers[0].contains('@') || is_guid(&table.headers[0])) => {
            values.push((table.header_line, table.headers[0].clone()));
            values.extend(table.rows.iter().map(|(line, cells)| (*line, cells.first().cloned().unwrap_or_default())));
        }
        None => return Err(tf("csv.no_user_column", &[("format", &t("csv.member_format"))])),
    }
    let mut seen: HashMap<String, u64> = HashMap::new();
    Ok(values
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(line, value)| {
            let key = value.to_lowercase();
            let error = match seen.get(&key) {
                Some(first) => Some(tf("csv.same_user", &[("line", first)])),
                None if !value.contains('@') && !is_guid(&value) => Some(t("csv.not_a_user")),
                None => None,
            };
            seen.entry(key).or_insert(line);
            MemberRow { line, value, error }
        })
        .collect())
}

// ----- the import dialog ------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowState {
    Ready,
    Skipped,
    Done,
    Failed,
}

struct BulkRow {
    cells: Vec<String>,
    status: String,
    state: RowState,
}

#[derive(PartialEq, Eq)]
enum Stage {
    Preview,
    Running,
    Done,
}

type Run = Box<dyn FnOnce(&mut App, u64)>;

/// An import's preview, progress and results, shown as one dialog.
pub struct Bulk {
    title: String,
    intro: String,
    /// The keys of the column titles in the catalogue.
    headers: Vec<&'static str>,
    rows: Vec<BulkRow>,
    stage: Stage,
    /// The button that starts the import, such as "Create 12 users".
    start: String,
    run: Option<Run>,
    /// What "Save results…" writes once the import is done, under these column titles' keys.
    results_headers: Vec<&'static str>,
    results: Vec<Vec<String>>,
    results_name: String,
    results_note: String,
}

impl Bulk {
    fn ready(&self) -> usize {
        self.rows.iter().filter(|r| r.state == RowState::Ready).count()
    }
}

/// How each row an import sent went, by row index: what to show in its Status column.
type Outcomes = Vec<(usize, Result<String, String>)>;

impl App {
    // ----- export ---------------------------------------------------------

    /// Saves the result list, as filtered and sorted on screen, to a CSV file. It is in English
    /// whatever the language of the interface, as a database export is, so a list of users can be
    /// imported again.
    pub fn export_list(&mut self) {
        let node = self.node;
        let exported = node.export(&self.view_rows());
        // The object ID goes last, after the columns as shown.
        let headers: Vec<&str> = exported.headers.iter().map(String::as_str).chain([exported.id_header]).collect();
        let rows: Vec<Vec<String>> = exported
            .rows
            .into_iter()
            .zip(exported.ids)
            .map(|(mut row, id)| {
                row.push(id);
                row
            })
            .collect();
        let name = i18n::in_english(|| format!("{}.csv", node.list_name()));
        if let Some(path) = self.save_csv(&t("csv.export_title"), &name, &headers, &rows) {
            log::info!("Exported {} {node:?} to {}", rows.len(), path.display());
            let args: [(&str, &dyn std::fmt::Display); 3] = [("count", &rows.len()), ("list", &node.list_name()), ("path", &path.display())];
            self.status = tf("csv.exported", &args);
        }
    }

    /// Asks where to save, then writes the CSV. Returns the path written, or None if the user
    /// cancelled or writing failed (which is reported).
    pub(crate) fn save_csv(&mut self, title: &str, file_name: &str, headers: &[&str], rows: &[Vec<String>]) -> Option<PathBuf> {
        let mut path = rfd::FileDialog::new()
            .set_title(title)
            .add_filter(t("csv.file_type"), &["csv"])
            .set_file_name(file_name)
            .save_file()?;
        if path.extension().is_none() {
            path.set_extension("csv");
        }
        let written = csv_bytes(headers, rows)
            .and_then(|bytes| std::fs::write(&path, bytes).with_context(|| tf("csv.save_failed", &[("path", &path.display())])));
        match written {
            Ok(()) => Some(path),
            Err(e) => {
                self.error(title, format!("{e:#}"));
                None
            }
        }
    }

    /// Asks for a CSV file and reads it. None if the user cancelled or it could not be read
    /// (which is reported).
    fn open_csv(&mut self, title: &str) -> Option<Table> {
        let path = rfd::FileDialog::new().set_title(title).add_filter(t("csv.file_type"), &["csv", "txt"]).pick_file()?;
        log::info!("{title}: reading {}", path.display());
        let parsed = std::fs::read(&path)
            .map_err(|e| tf("csv.read_failed", &[("path", &path.display()), ("error", &e)]))
            .and_then(|bytes| parse_table(&bytes));
        match parsed {
            Ok(table) => Some(table),
            Err(e) => {
                self.error(title, e);
                None
            }
        }
    }

    // ----- user import ----------------------------------------------------

    pub fn import_users(&mut self) {
        let title = &t("bulk.users_title");
        let Some(table) = self.open_csv(title) else { return };
        let existing: HashSet<String> = self
            .data
            .get(&Node::Users)
            .map(|users| users.iter().map(|u| s(u, "userPrincipalName").to_lowercase()).collect())
            .unwrap_or_default();
        let (users, ignored) = match parse_users(&table, &self.domains, &existing) {
            Ok(parsed) => parsed,
            Err(e) => return self.error(title, e),
        };
        if users.is_empty() {
            return self.error(title, tf("csv.no_users", &[("format", &t("csv.user_format"))]));
        }
        let mut intro = t("bulk.users_intro");
        if !ignored.is_empty() {
            intro.push_str("\n\n");
            intro.push_str(&tf("bulk.ignored", &[("columns", &ignored.join(", "))]));
        }
        let rows = users
            .iter()
            .map(|u| {
                let password = t(if u.generated_password { "bulk.generated" } else { "bulk.from_file" });
                let cells = vec![u.line.to_string(), u.display_name.clone(), u.upn.clone(), password];
                match &u.error {
                    Some(e) => BulkRow { cells, status: e.clone(), state: RowState::Skipped },
                    None => BulkRow { cells, status: t("bulk.ready"), state: RowState::Ready },
                }
            })
            .collect();
        let jobs: Vec<(usize, Value)> =
            users.iter().enumerate().filter(|(_, u)| u.error.is_none()).map(|(i, u)| (i, u.body.clone())).collect();
        let creds: Vec<[String; 3]> =
            users.into_iter().map(|u| [u.display_name, u.upn, u.password]).collect();
        let n = jobs.len();
        let run = move |app: &mut App, dialog: u64| {
            log::info!("Importing {n} users from CSV");
            app.spawn(
                tf("bulk.creating_users", &[("count", &n)]),
                move |g| Ok(jobs.into_iter().map(|(i, body)| (i, g.post("/users", &body).map_err(|e| e.to_string()))).collect()),
                move |app, r: anyhow::Result<Vec<(usize, Result<Value, String>)>>| {
                    let mut outcomes = Vec::new();
                    let mut results = Vec::new();
                    let (created, stopped) = match r {
                        Ok(created) => (created, None),
                        Err(e) => (Vec::new(), Some(e)),
                    };
                    for (i, created) in created {
                        let [name, upn, password] = creds[i].clone();
                        match created {
                            Ok(user) => {
                                app.upsert(Node::Users, user);
                                results.push(vec![name, upn, password, t("bulk.created")]);
                                outcomes.push((i, Ok(t("bulk.created"))));
                            }
                            Err(e) => {
                                results.push(vec![name, upn, String::new(), e.clone()]);
                                outcomes.push((i, Err(e)));
                            }
                        }
                    }
                    app.finish_bulk(dialog, outcomes, results, stopped, "bulk.created_users", "bulk.created_users_failed");
                },
            );
        };
        self.push(Dialog::Bulk(Box::new(Bulk {
            title: title.into(),
            intro,
            headers: vec!["col.line", "col.display_name", "col.upn", "col.password", "col.status"],
            rows,
            stage: Stage::Preview,
            start: if n == 1 { t("bulk.create_user") } else { tf("bulk.create_users", &[("count", &n)]) },
            run: Some(Box::new(run)),
            results_headers: vec!["col.display_name", "col.upn", "col.initial_password", "col.result"],
            results: Vec::new(),
            results_name: format!("{}.csv", t("bulk.new_users_file")),
            results_note: t("bulk.passwords_note"),
        })));
    }

    // ----- member import --------------------------------------------------

    pub fn import_members(&mut self, group: Value) {
        let name = s(&group, "displayName");
        let title = tf("bulk.members_title", &[("name", &name)]);
        if !s(&group, "membershipRule").is_empty() {
            return self.error(&title, t("bulk.dynamic_group"));
        }
        let Some(table) = self.open_csv(&title) else { return };
        let members = match parse_members(&table) {
            Ok(m) if m.is_empty() => return self.error(&title, tf("csv.no_members", &[("format", &t("csv.member_format"))])),
            Ok(m) => m,
            Err(e) => return self.error(&title, e),
        };
        let rows = members
            .iter()
            .map(|m| {
                let cells = vec![m.line.to_string(), m.value.clone()];
                match &m.error {
                    Some(e) => BulkRow { cells, status: e.clone(), state: RowState::Skipped },
                    None => BulkRow { cells, status: t("bulk.ready"), state: RowState::Ready },
                }
            })
            .collect();
        let jobs: Vec<(usize, String)> =
            members.iter().enumerate().filter(|(_, m)| m.error.is_none()).map(|(i, m)| (i, m.value.clone())).collect();
        let values: Vec<String> = members.into_iter().map(|m| m.value).collect();
        let n = jobs.len();
        let group_id = s(&group, "id");
        let endpoint = self.settings.graph_endpoint.clone();
        let run = move |app: &mut App, dialog: u64| {
            log::info!("Adding {n} members to {name} from CSV");
            app.spawn(
                tf("bulk.adding_members", &[("count", &n)]),
                move |g| {
                    let path = format!("/groups/{group_id}/members/$ref");
                    Ok(jobs
                        .into_iter()
                        .map(|(i, value)| {
                            let added = find_user(g, &value).and_then(|id| {
                                let body = json!({ "@odata.id": format!("{endpoint}/v1.0/directoryObjects/{id}") });
                                match g.post(&path, &body) {
                                    Ok(_) => Ok(t("bulk.added")),
                                    Err(e) if api_error(&e).is_some_and(ApiError::already_exists) => Ok(t("bulk.already_member")),
                                    Err(e) => Err(e.to_string()),
                                }
                            });
                            (i, added)
                        })
                        .collect())
                },
                move |app, r: anyhow::Result<Outcomes>| {
                    let (outcomes, stopped) = match r {
                        Ok(outcomes) => (outcomes, None),
                        Err(e) => (Vec::new(), Some(e)),
                    };
                    let results = outcomes
                        .iter()
                        .map(|(i, o)| vec![values[*i].clone(), o.clone().unwrap_or_else(|e| e)])
                        .collect();
                    app.finish_bulk(dialog, outcomes, results, stopped, "bulk.added_members", "bulk.added_members_failed");
                },
            );
        };
        self.push(Dialog::Bulk(Box::new(Bulk {
            title: title.clone(),
            intro: t("bulk.members_intro"),
            headers: vec!["col.line", "col.user", "col.status"],
            rows,
            stage: Stage::Preview,
            start: if n == 1 { t("bulk.add_member") } else { tf("bulk.add_members", &[("count", &n)]) },
            run: Some(Box::new(run)),
            results_headers: vec!["col.user", "col.result"],
            results: Vec::new(),
            results_name: format!("{}.csv", t("bulk.added_members_file")),
            results_note: String::new(),
        })));
    }

    /// Shows how an import went, row by row, and sums it up in the status bar with the text for
    /// `done_key`, or for `failed_key` when some failed. `stopped` is the error that ended the
    /// import early, after which what happened to the rows it hadn't reported is not known.
    fn finish_bulk(
        &mut self,
        dialog: u64,
        outcomes: Outcomes,
        results: Vec<Vec<String>>,
        stopped: Option<anyhow::Error>,
        done_key: &str,
        failed_key: &str,
    ) {
        let done = outcomes.iter().filter(|(_, o)| o.is_ok()).count();
        let failed = outcomes.len() - done;
        log::info!("CSV import: {done} done, {failed} failed");
        self.status = if failed == 0 {
            tf(done_key, &[("count", &done)])
        } else {
            tf(failed_key, &[("count", &done), ("failed", &failed)])
        };
        if let Some(e) = &stopped {
            log::error!("CSV import stopped: {e:#}");
            self.status = tf("bulk.stopped_status", &[("count", &done)]);
        }
        let Some(Dialog::Bulk(bulk)) = self.dialog_mut(dialog) else { return };
        for (i, outcome) in outcomes {
            let row = &mut bulk.rows[i];
            (row.state, row.status) = match outcome {
                Ok(status) => (RowState::Done, status),
                Err(e) => (RowState::Failed, e),
            };
        }
        if let Some(e) = stopped {
            let unknown = tf("bulk.stopped", &[("error", &format!("{e:#}"))]);
            for row in bulk.rows.iter_mut().filter(|r| r.state == RowState::Ready) {
                (row.state, row.status) = (RowState::Failed, unknown.clone());
            }
        }
        bulk.results = results;
        bulk.stage = Stage::Done;
    }

    pub(crate) fn show_bulk(&mut self, ctx: &egui::Context, modal: egui::Modal, id: u64, bulk: &mut Bulk, first: bool, escape: bool) -> bool {
        let mut close = escape && bulk.stage != Stage::Running;
        let mut start = false;
        let mut save = false;
        modal.show(ctx, |ui| {
            ui.set_width(760.0);
            ui.heading(bulk.title.as_str());
            ui.add_space(4.0);
            ui.add(egui::Label::new(bulk.intro.as_str()).wrap());
            ui.add_space(6.0);
            let count = |state| bulk.rows.iter().filter(|r| r.state == state).count();
            let summary = match bulk.stage {
                Stage::Preview => tf("bulk.preview_summary", &[("ready", &count(RowState::Ready)), ("skip", &count(RowState::Skipped))]),
                Stage::Running => tf("bulk.running_summary", &[("count", &count(RowState::Ready))]),
                Stage::Done => {
                    let args: [(&str, &dyn std::fmt::Display); 3] =
                        [("done", &count(RowState::Done)), ("failed", &count(RowState::Failed)), ("skipped", &count(RowState::Skipped))];
                    tf("bulk.done_summary", &args)
                }
            };
            ui.label(RichText::new(summary).bold());
            ui.add_space(4.0);
            let text_height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
            let error_color = ui.visuals().error_fg_color;
            let mut table = TableBuilder::new(ui)
                .id_salt(("bulk", id))
                .striped(true)
                .resizable(true)
                .max_scroll_height(340.0)
                .cell_layout(Layout::left_to_right(Align::Center));
            for i in 0..bulk.headers.len() {
                table = table.column(match i {
                    0 => Column::auto().at_least(36.0),
                    _ if i + 1 == bulk.headers.len() => Column::remainder().at_least(160.0).clip(true),
                    _ => Column::initial(180.0).at_least(60.0).clip(true),
                });
            }
            table
                .header(text_height, |mut header| {
                    for h in &bulk.headers {
                        header.col(|ui| {
                            ui.label(RichText::new(t(h)).bold());
                        });
                    }
                })
                .body(|body| {
                    body.rows(text_height, bulk.rows.len(), |mut row| {
                        let r = &bulk.rows[row.index()];
                        for cell in &r.cells {
                            row.col(|ui| {
                                ui.add(egui::Label::new(cell.as_str()).truncate());
                            });
                        }
                        row.col(|ui| {
                            let text = match r.state {
                                RowState::Skipped | RowState::Failed => RichText::new(format!("⚠ {}", r.status)).color(error_color),
                                _ => RichText::new(r.status.as_str()),
                            };
                            ui.add(egui::Label::new(text).truncate()).on_hover_text(r.status.as_str());
                        });
                    });
                });
            ui.add_space(8.0);
            if bulk.stage == Stage::Done && !bulk.results_note.is_empty() {
                ui.add(egui::Label::new(bulk.results_note.as_str()).wrap());
                ui.add_space(6.0);
            }
            ui.horizontal(|ui| match bulk.stage {
                Stage::Preview => {
                    let go = ui.add_enabled(bulk.ready() > 0, egui::Button::new(format!("  {}  ", bulk.start)));
                    let cancel = ui.button(t("dialog.cancel"));
                    if first {
                        cancel.request_focus();
                    }
                    start = go.clicked();
                    close |= cancel.clicked();
                }
                Stage::Running => {
                    ui.spinner();
                    ui.label(t("bulk.working"));
                }
                Stage::Done => {
                    save = ui.button(t("bulk.save_results")).clicked();
                    let ok = ui.button(t("dialog.close"));
                    close |= ok.clicked();
                }
            });
        });
        if start && self.graph.is_some()
            && let Some(run) = bulk.run.take() {
                bulk.stage = Stage::Running;
                run(self, id);
            }
        if save {
            let headers: Vec<String> = bulk.results_headers.iter().map(|h| t(h)).collect();
            let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
            if let Some(path) = self.save_csv(&t("bulk.save_results_title"), &bulk.results_name, &headers, &bulk.results) {
                self.status = tf("bulk.saved_results", &[("path", &path.display())]);
            }
        }
        !close
    }
}

/// The object ID of the user named by `value`: an object ID as it is, otherwise the user with
/// that user principal name or, failing that, that email address.
fn find_user(g: &crate::graph::Graph, value: &str) -> Result<String, String> {
    if is_guid(value) {
        return Ok(value.to_string());
    }
    match g.get(&format!("/users/{}?$select=id", enc(value))) {
        Ok(user) => return Ok(s(&user, "id")),
        Err(e) if !api_error(&e).is_some_and(ApiError::not_found) => return Err(e.to_string()),
        Err(_) => {}
    }
    let found = g
        .get(&format!("/users?$filter=mail eq '{}'&$select=id", odata_str(value)))
        .map_err(|e| e.to_string())?;
    match found["value"].as_array().map(Vec::as_slice) {
        Some([user]) => Ok(s(user, "id")),
        Some([_, _, ..]) => Err(t("bulk.several_users")),
        _ => Err(t("bulk.no_user")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> Table {
        parse_table(text.as_bytes()).unwrap()
    }

    #[test]
    fn reads_excel_variants() {
        let t = parse_table(b"\xEF\xBB\xBFName;UPN\r\nJos\xC3\xA9;jose@contoso.com\r\n").unwrap();
        assert_eq!(t.headers, ["Name", "UPN"]);
        assert_eq!(t.rows, [(2, vec!["José".to_string(), "jose@contoso.com".to_string()])]);
        // Windows-1252, as Excel's "CSV (Comma delimited)" saves it.
        let t = parse_table(b"Name,UPN\nJos\xE9 \x96 IT,jose@contoso.com\n").unwrap();
        assert_eq!(t.rows[0].1[0], "José – IT");
        let t = parse_table(b"Name,UPN\r\n\r\nAnn,ann@contoso.com\r\n\"Bob\r\nSmith\",bob@contoso.com\r\nCy,cy@contoso.com\r\n").unwrap();
        let lines: Vec<u64> = t.rows.iter().map(|r| r.0).collect();
        assert_eq!(lines, [3, 4, 6]);
        let utf16: Vec<u8> = "\u{FEFF}Name\tUPN\nAnn\tann@contoso.com\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(parse_table(&utf16).unwrap().rows[0].1, ["Ann", "ann@contoso.com"]);
        assert!(parse_table(b"\n\n").is_err());
    }

    #[test]
    fn reads_large_files_quickly() {
        // Counting each row's line from the start of the file took minutes for files this size.
        let mut text = String::from("Name,UPN\r\n");
        for i in 0..50_000 {
            text.push_str(&format!("User {i},user{i}@contoso.com\r\n"));
        }
        let started = std::time::Instant::now();
        let t = parse_table(text.as_bytes()).unwrap();
        assert_eq!(t.rows.len(), 50_000);
        assert_eq!(t.rows.last().unwrap().0, 50_001);
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
    }

    #[test]
    fn exports_round_trip_and_defuse_formulas() {
        let rows = vec![vec!["=HYPERLINK(\"x\")".to_string(), "-5".into(), "Smith, Ann".into()]];
        let bytes = csv_bytes(&["A", "B", "C"], &rows).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("'=HYPERLINK") && text.contains(",-5,") && text.contains("\"Smith, Ann\""));
        assert_eq!(parse_table(&bytes).unwrap().rows[0].1, rows[0]);
    }

    #[test]
    fn validates_users() {
        let t = table(
            "Display name,User name,First name,Usage location,Password,Enabled,Licences\n\
             Ann Lee,ann,Ann,gb,,,1\n\
             ,bob@contoso.com,,,,,\n\
             Cy,cy@fabrikam.com,,,,,\n\
             Di,existing@contoso.com,,,,,\n\
             Ann Again,ANN@contoso.com,,,,,\n\
             Ed,ed@contoso.com,,GBR,,,\n\
             Fi,fi@contoso.com,,,short,,\n\
             Gu,gu@contoso.com,,,,maybe,\n",
        );
        let domains = vec!["contoso.com".to_string()];
        let existing: HashSet<String> = ["existing@contoso.com".to_string()].into();
        let (users, ignored) = parse_users(&t, &domains, &existing).unwrap();
        assert_eq!(ignored, ["Licences"]);
        let ann = &users[0];
        assert_eq!(ann.error, None);
        assert_eq!(ann.upn, "ann@contoso.com");
        assert!(ann.generated_password && ann.password.len() == 16);
        assert_eq!(ann.body["usageLocation"], "GB");
        assert_eq!(ann.body["mailNickname"], "ann");
        assert_eq!(ann.body["accountEnabled"], true);
        let errors: Vec<&str> = users[1..].iter().map(|u| u.error.as_deref().unwrap()).collect();
        assert_eq!(
            errors,
            [
                "Display name is missing",
                "fabrikam.com is not a verified domain of this tenant",
                "A user with this name already exists",
                "Same user principal name as line 2",
                "Usage location 'GBR' is not a two-letter country code",
                "Password must be at least 8 characters",
                "Enabled must be Yes or No",
            ]
        );
        assert!(parse_users(&table("Name,Email\nAnn,ann@contoso.com\n"), &domains, &existing).is_err());
    }

    #[test]
    fn reads_member_lists() {
        let m = parse_members(&table("Display name,Email\nAnn,ann@contoso.com\nBob,\nAnn,ANN@contoso.com\nCy,cy\n")).unwrap();
        let values: Vec<(&str, Option<&str>)> = m.iter().map(|r| (r.value.as_str(), r.error.as_deref())).collect();
        assert_eq!(
            values,
            [
                ("ann@contoso.com", None),
                ("ANN@contoso.com", Some("Same user as line 2")),
                ("cy", Some("Not a user principal name, email address or object ID")),
            ]
        );
        // No header: the first line is a user too.
        let m = parse_members(&table("ann@contoso.com\n00000000-0000-0000-0000-000000000001\n")).unwrap();
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|r| r.error.is_none()));
        assert!(parse_members(&table("Name,Department\nAnn,IT\n")).is_err());
    }
}
