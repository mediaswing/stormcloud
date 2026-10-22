//! The words the app shows, in English and in any language the user has had translated.
//!
//! Every piece of interface text has a key, such as `menu.file`, and an English version in
//! `ENGLISH`. A translation is a JSON file in the languages folder that gives the text for some
//! or all of the keys in another language. Anything a translation leaves out, or gets wrong, is
//! shown in English instead, so a damaged or out-of-date file never leaves a control without a
//! label.
//!
//! Translations are made by a local AI model in Ollama (see `translate_batch`), so the interface
//! text never leaves the computer. The files are plain JSON, so anyone can correct them, or share
//! them with others.
//!
//! What the app sends to Microsoft, writes to the log or exports to a database stays in English
//! (see `in_english`), so it reads the same whichever language the interface is in.

mod english;

use english::ENGLISH;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

/// The language code for English, which needs no translation file.
pub const ENGLISH_CODE: &str = "en";

/// How many pieces of text are sent to the AI model at once. Small enough for a small model to
/// keep track of, large enough not to be slow.
pub const BATCH_SIZE: usize = 20;

/// Where Ollama listens.
const OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Translation files can come from other people, so they are checked before use. The whole
/// catalogue in English is about 60 KB.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Longest language name accepted from a translation file.
const MAX_NAME_CHARS: usize = 60;

/// Languages offered: code, name in the language itself, and name in English. Only languages
/// written in the Latin, Greek and Cyrillic alphabets are listed, because the window cannot yet
/// draw other scripts, or text that runs from right to left.
const LANGUAGES: &[(&str, &str, &str)] = &[
    ("en", "English", "English"),
    ("cy", "Cymraeg", "Welsh"),
    ("ga", "Gaeilge", "Irish"),
    ("gd", "Gàidhlig", "Scottish Gaelic"),
    ("cs", "Čeština", "Czech"),
    ("da", "Dansk", "Danish"),
    ("de", "Deutsch", "German"),
    ("el", "Ελληνικά", "Greek"),
    ("es", "Español", "Spanish"),
    ("fr", "Français", "French"),
    ("hr", "Hrvatski", "Croatian"),
    ("it", "Italiano", "Italian"),
    ("lt", "Lietuvių", "Lithuanian"),
    ("lv", "Latviešu", "Latvian"),
    ("hu", "Magyar", "Hungarian"),
    ("nl", "Nederlands", "Dutch"),
    ("nb", "Norsk bokmål", "Norwegian"),
    ("pl", "Polski", "Polish"),
    ("pt", "Português", "Portuguese"),
    ("ro", "Română", "Romanian"),
    ("sk", "Slovenčina", "Slovak"),
    ("sl", "Slovenščina", "Slovenian"),
    ("sq", "Shqip", "Albanian"),
    ("fi", "Suomi", "Finnish"),
    ("sv", "Svenska", "Swedish"),
    ("tr", "Türkçe", "Turkish"),
    ("uk", "Українська", "Ukrainian"),
    ("bg", "Български", "Bulgarian"),
    ("ru", "Русский", "Russian"),
    ("sr", "Српски", "Serbian"),
];

/// The translation in use, or `None` for English.
static ACTIVE: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

thread_local! {
    /// Set while `in_english` runs on this thread.
    static ENGLISH_ONLY: Cell<bool> = const { Cell::new(false) };
}

fn english_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| ENGLISH.iter().copied().collect())
}

/// The English text for `key`.
pub fn english(key: &str) -> Option<&'static str> {
    english_map().get(key).copied()
}

/// The text for `key` in the language in use.
pub fn t(key: &str) -> String {
    tf(key, &[])
}

/// The text for `key` in the language in use, with each `{name}` replaced by its value from
/// `args`.
pub fn tf(key: &str, args: &[(&str, &dyn Display)]) -> String {
    if ENGLISH_ONLY.get() {
        return fill(template(None, key), args);
    }
    // The lock is let go before the values are filled in: a value can be an error whose text
    // comes from here too, and reading the lock again while the window switches language would
    // wait for ever.
    let text = template(ACTIVE.read().unwrap_or_else(|e| e.into_inner()).as_ref(), key).to_owned();
    fill(&text, args)
}

/// Whether a translation is in use, rather than English.
pub fn translated() -> bool {
    ACTIVE.read().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Runs `f` with all interface text in English on this thread, for what has to read the same
/// whatever the interface language, such as the rows and column names of a database export.
pub fn in_english<T>(f: impl FnOnce() -> T) -> T {
    /// Puts the setting back however `f` ends, so a panic can't leave the window in English.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            ENGLISH_ONLY.set(self.0);
        }
    }
    let _restore = Restore(ENGLISH_ONLY.replace(true));
    f()
}

/// The text for `key` before values are filled in.
fn template<'a>(active: Option<&'a HashMap<String, String>>, key: &'a str) -> &'a str {
    match active.and_then(|a| a.get(key)) {
        Some(text) => text.as_str(),
        None => english(key).unwrap_or_else(|| {
            log::warn!("no English text for {key}");
            key
        }),
    }
}

/// Replaces each `{name}` in `template` with its value. Braces that are not a known name are left
/// as they are.
fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let value = after
            .find('}')
            .and_then(|end| args.iter().find(|(name, _)| *name == &after[..end]).map(|(_, v)| (end, v)));
        match value {
            Some((end, v)) => {
                out.push_str(&v.to_string());
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The names in braces in `text`, such as `name` in "Opening {name}."
fn placeholders(text: &str) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if end > 0 && after[..end].chars().all(|c| c.is_ascii_lowercase() || c == '_') => {
                names.insert(&after[..end]);
                rest = &after[end + 1..];
            }
            _ => rest = after,
        }
    }
    names
}

/// Characters that could hide or disguise text: control characters, and the marks that reorder
/// text or make it invisible. They could be used to make a button or a warning read as something
/// it isn't.
fn is_hidden_char(c: char) -> bool {
    (c.is_control() && c != '\n')
        || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
}

/// Whether a translation of `key` can be used: it is not empty or unreasonably long, has no
/// hidden characters, only has line breaks where the English does, and has the same `{name}`
/// values as the English.
fn fits(key: &str, text: &str) -> bool {
    let Some(en) = english(key) else { return false };
    let limit = en.chars().count() * 4 + 40;
    !text.trim().is_empty()
        && text.chars().count() <= limit
        && !text.chars().any(is_hidden_char)
        && (en.contains('\n') || !text.contains('\n'))
        && placeholders(en) == placeholders(text)
}

/// A language name from a translation file, if it is safe to show.
fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    let ok = !name.is_empty() && name.chars().count() <= MAX_NAME_CHARS && !name.chars().any(|c| is_hidden_char(c) || c == '\n');
    ok.then(|| name.to_owned())
}

// ----- languages and translation files -------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Language {
    pub code: String,
    /// The language's name in that language, such as "Français".
    pub name: String,
    /// The language's name in English, such as "French".
    pub english_name: String,
}

impl Language {
    /// How the language is shown in the list, such as "Français (French)".
    pub fn label(&self) -> String {
        if self.name == self.english_name || self.english_name.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.english_name)
        }
    }
}

/// A translation file: the language's names, which model made it, and the text for each key.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Translation {
    pub language: String,
    pub english_name: String,
    /// The AI model that made the translation, if any.
    pub made_with: String,
    pub strings: BTreeMap<String, String>,
}

impl Translation {
    pub fn new(language: &Language) -> Self {
        Self { language: language.name.clone(), english_name: language.english_name.clone(), ..Self::default() }
    }

    /// Keys with no usable translation, in catalogue order.
    pub fn missing(&self) -> Vec<&'static str> {
        ENGLISH
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| !self.strings.get(*key).is_some_and(|text| fits(key, text)))
            .collect()
    }

    /// The translations that can be used: known keys whose `{name}` values match the English.
    /// The rest are left out, so they show in English.
    fn usable(&self) -> HashMap<String, String> {
        self.strings.iter().filter(|(key, text)| fits(key, text)).map(|(key, text)| (key.clone(), text.clone())).collect()
    }
}

/// Translations of the interface, one JSON file per language, next to the settings.
pub fn languages_dir() -> PathBuf {
    crate::config::config_dir().join("languages")
}

pub fn file_for(dir: &Path, code: &str) -> PathBuf {
    dir.join(format!("{code}.json"))
}

/// A language code that is safe to use as a file name.
fn valid_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 16 && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn load(dir: &Path, code: &str) -> anyhow::Result<Translation> {
    if !valid_code(code) {
        bail!("\"{code}\" is not a language code");
    }
    let path = file_for(dir, code);
    let size = std::fs::metadata(&path).context("the file could not be read")?.len();
    if size > MAX_FILE_BYTES {
        bail!("the file is too large (the limit is 1 MB)");
    }
    let text = std::fs::read_to_string(&path).context("the file could not be read")?;
    serde_json::from_str(&text).context("the file is not a valid translation")
}

pub fn save(dir: &Path, code: &str, translation: &Translation) -> anyhow::Result<()> {
    if !valid_code(code) {
        bail!("\"{code}\" is not a language code");
    }
    let json = serde_json::to_vec_pretty(translation)?;
    std::fs::create_dir_all(dir).context("the languages folder could not be created")?;
    crate::config::write_file(&file_for(dir, code), &json, false).context("the translation could not be saved")
}

/// The languages to offer: English, the built-in list, and any other translation files someone
/// has put in the languages folder.
pub fn available(dir: &Path) -> Vec<Language> {
    let mut languages: Vec<Language> = LANGUAGES
        .iter()
        .map(|(code, name, english_name)| Language {
            code: (*code).into(),
            name: (*name).into(),
            english_name: (*english_name).into(),
        })
        .collect();
    let Ok(entries) = std::fs::read_dir(dir) else { return languages };
    let mut extra = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(code) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if languages.iter().any(|l| l.code == code) {
            continue;
        }
        match load(dir, code) {
            Ok(t) => match clean_name(&t.language) {
                Some(name) => extra.push(Language {
                    code: code.to_owned(),
                    name,
                    english_name: clean_name(&t.english_name).unwrap_or_default(),
                }),
                None => log::warn!("Translation {code} has no usable language name"),
            },
            Err(e) => log::warn!("Could not read translation {code}: {e:#}"),
        }
    }
    extra.sort_by_key(|l| l.name.to_lowercase());
    languages.extend(extra);
    languages
}

/// Uses `translation` for all interface text, or English if `None`.
pub fn activate(translation: Option<&Translation>) {
    let usable = translation.map(Translation::usable);
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = usable;
}

/// Switches to the saved language at startup. Falls back to English, with a note in the log, if
/// its translation can't be read.
pub fn activate_saved(dir: &Path, code: &str) {
    if code == ENGLISH_CODE || code.is_empty() {
        return;
    }
    match load(dir, code) {
        Ok(translation) => activate(Some(&translation)),
        Err(e) => log::warn!("Could not load translation {code}, using English: {e:#}"),
    }
}

// ----- translating with a local AI model ------------------------------------

fn ollama() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            // A large model can take minutes over a batch on a laptop.
            .timeout(Duration::from_secs(900))
            .build()
            .expect("HTTP client")
    })
}

/// The models installed in Ollama, by name.
pub fn models() -> anyhow::Result<Vec<String>> {
    let body: Value = ollama()
        .get(format!("{OLLAMA_URL}/api/tags"))
        .send()
        .context("Ollama did not answer. Check that it is running.")?
        .json()
        .context("Ollama sent an unexpected reply")?;
    let mut names: Vec<String> = body["models"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m["name"].as_str().map(str::to_owned))
        // Embedding models can't write text.
        .filter(|n| !n.contains("embed"))
        .collect();
    names.sort();
    Ok(names)
}

/// Asks the Ollama model `model` for a JSON answer to `prompt`.
fn generate(model: &str, prompt: &str) -> anyhow::Result<String> {
    let request = json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "format": "json",
        // Keep answers steady, so the same text gets the same translation. Ollama's default
        // context can be as small as 2,048 tokens, which a batch of text and its translation can
        // overflow, cutting the answer short.
        "options": { "temperature": 0.1, "num_ctx": 8192 },
    });
    // Ollama explains a failure, such as a model it can't load, in the reply.
    let body: Value = ollama()
        .post(format!("{OLLAMA_URL}/api/generate"))
        .json(&request)
        .send()
        .context("Ollama did not answer. Check that it is running.")?
        .json()
        .context("Ollama sent an unexpected reply")?;
    if let Some(error) = body["error"].as_str() {
        bail!("Ollama could not use the model {model}: {error}");
    }
    match body["response"].as_str() {
        Some(text) if !text.trim().is_empty() => Ok(text.to_owned()),
        _ => bail!("the AI model gave an empty answer"),
    }
}

fn prompt(language: &Language, batch: &BTreeMap<&str, &str>) -> String {
    let target = if language.english_name.is_empty() {
        language.name.clone()
    } else {
        format!("{} ({})", language.english_name, language.name)
    };
    let input = serde_json::to_string_pretty(batch).unwrap_or_default();
    format!(
        "You are translating the interface of a console that IT administrators use to manage Microsoft Entra ID \
         (users, groups, devices, licences and Exchange Online). Translate each value in the JSON object below from \
         British English into {target}.\n\n\
         Rules:\n\
         - Reply with only a JSON object that has exactly the same keys, with each value translated.\n\
         - Keep every name in curly braces, such as {{name}} or {{count}}, exactly as it is. Do not translate it.\n\
         - Keep these unchanged: key names such as Ctrl, Shift, Tab, Enter, Escape, Delete, Insert, F1, F2, F5, F6 and \
           F10; product and service names such as Microsoft Entra ID, Microsoft Graph, Microsoft 365, Exchange Online, \
           Office 365, Teams, My Apps, Temporary Access Pass, MariaDB, Ollama and GitHub; permission names such as \
           User.ReadWrite.All or Exchange.Manage; property names such as isAssignableToRole; and abbreviations such \
           as CSV, TLS, MFA, UPN, SKU, ID, GUID, URL and OS.\n\
         - Keep line breaks, quotation marks and the … character where the English has them.\n\
         - Use the words Microsoft uses in that language for Microsoft Entra ID and Microsoft 365, and the form of \
           address usual for software in that language.\n\
         - Translate every value, even short ones.\n\n\
         For example, into French, {{\"menu.file\": \"File\", \"status.loading_list\": \"Loading {{list}}…\"}} \
         becomes {{\"menu.file\": \"Fichier\", \"status.loading_list\": \"Chargement des {{list}}…\"}}\n\n\
         {input}"
    )
}

/// Translates one batch of keys with the Ollama model `model`. Returns the translations that can
/// be used; any the model got wrong are left out.
pub fn translate_batch(model: &str, language: &Language, keys: &[&'static str]) -> anyhow::Result<BTreeMap<String, String>> {
    let batch: BTreeMap<&str, &str> = keys.iter().filter_map(|k| Some((*k, english(k)?))).collect();
    let reply = generate(model, &prompt(language, &batch))?;
    let reply: Value = serde_json::from_str(reply.trim()).context("the AI model did not reply with a translation")?;
    let Some(reply) = reply.as_object() else { bail!("the AI model did not reply with a translation") };
    let mut out = BTreeMap::new();
    for key in keys {
        match reply.get(*key).and_then(Value::as_str) {
            // Small models sometimes hand the English back. Leaving it out shows the same text,
            // and lets the next run try it again.
            Some(text) if english(key) == Some(text.trim()) => log::debug!("{key} came back untranslated"),
            Some(text) if fits(key, text) => {
                out.insert((*key).to_owned(), text.trim().to_owned());
            }
            Some(_) => log::debug!("Translation of {key} dropped: placeholders changed"),
            None => log::debug!("Translation of {key} missing from the reply"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    fn render(active: Option<&HashMap<String, String>>, key: &str, args: &[(&str, &dyn Display)]) -> String {
        fill(template(active, key), args)
    }

    #[test]
    fn keys_are_unique() {
        assert_eq!(english_map().len(), ENGLISH.len(), "a key appears twice in ENGLISH");
    }

    const SOURCES: [&str; 20] = [
        include_str!("app.rs"),
        include_str!("auth.rs"),
        include_str!("bulk.rs"),
        include_str!("db.rs"),
        include_str!("dialogs.rs"),
        include_str!("exchange.rs"),
        include_str!("graph.rs"),
        include_str!("language.rs"),
        include_str!("listview.rs"),
        include_str!("model.rs"),
        include_str!("offboarding.rs"),
        include_str!("onboarding.rs"),
        include_str!("ops.rs"),
        include_str!("security.rs"),
        include_str!("servers.rs"),
        include_str!("sheets.rs"),
        include_str!("update.rs"),
        include_str!("validate.rs"),
        include_str!("workflow.rs"),
        include_str!("main.rs"),
    ];

    /// The sections keys are grouped in, such as `menu` in `menu.file`.
    fn sections() -> BTreeSet<&'static str> {
        ENGLISH.iter().filter_map(|(key, _)| key.split('.').next()).collect()
    }

    #[test]
    fn every_key_in_the_code_has_english_text() {
        // Keys are passed to t() and tf(), and also kept in tables (such as column titles) and
        // looked up when shown, so look for any quoted string that starts with a section name.
        let quoted = keys_in();
        let mut count = 0;
        for source in SOURCES {
            for key in quoted(source) {
                count += 1;
                assert!(english(key).is_some(), "no English text for {key}");
            }
        }
        assert!(count > 1000, "only found {count} keys in the code");
    }

    /// The string literals in Rust source, skipping escaped quotes and the `'"'` character.
    fn literals(source: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let bytes = source.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"'\"'") {
                i += 3;
            } else if bytes[i..].starts_with(b"'\\\"'") {
                i += 4;
            } else if bytes[i] == b'"' {
                let start = i + 1;
                i = start;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                out.push(&source[start..i.min(bytes.len())]);
                i += 1;
            } else {
                i += 1;
            }
        }
        out
    }

    /// Finds quoted strings such as "menu.file" whose first part is a section of `ENGLISH`.
    fn keys_in() -> impl Fn(&str) -> Vec<&str> {
        let sections = sections();
        move |source: &str| {
            literals(source)
                .into_iter()
                .filter(|piece| {
                    piece.contains('.')
                        && piece.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
                        && piece.split('.').next().is_some_and(|s| sections.contains(s))
                })
                .collect()
        }
    }

    #[test]
    fn finds_literals_past_escaped_quotes() {
        let source = r#"a("x \"y\" z"); b('"'); c("menu.file");"#;
        assert_eq!(literals(source), ["x \\\"y\\\" z", "menu.file"]);
    }

    #[test]
    fn every_english_text_is_used() {
        for (key, _) in ENGLISH {
            let quoted = format!("\"{key}\"");
            assert!(SOURCES.iter().any(|s| s.contains(&quoted)), "{key} is not used anywhere");
        }
    }

    #[test]
    fn english_has_no_hidden_characters() {
        for (key, text) in ENGLISH {
            assert!(!text.chars().any(is_hidden_char), "{key} has a hidden character");
            assert!(fits(key, text), "{key} does not fit its own checks");
        }
    }

    #[test]
    fn fills_in_values() {
        assert_eq!(fill("Opening {name}.", &[("name", &"notes")]), "Opening notes.");
        // Unknown names and stray braces are left alone.
        assert_eq!(fill("{a} {b} {", &[("a", &1)]), "1 {b} {");
    }

    #[test]
    fn uses_the_translation_and_falls_back_to_english() {
        let fr = map(&[("menu.file", "Fichier")]);
        assert_eq!(render(Some(&fr), "menu.file", &[]), "Fichier");
        assert_eq!(render(Some(&fr), "menu.help", &[]), "Help");
    }

    #[test]
    fn english_scope_ignores_the_translation() {
        let mut fr = Translation::default();
        fr.strings.insert("menu.file".into(), "Fichier".into());
        // Only this test's thread sees the scope, but other tests share ACTIVE, so check the
        // scope against a key no other test translates.
        fr.strings.insert("menu.view".into(), "Affichage".into());
        activate(Some(&fr));
        assert_eq!(in_english(|| t("menu.view")), "View");
        assert_eq!(t("menu.view"), "Affichage");
        activate(None);
    }

    #[test]
    fn finds_placeholders() {
        assert_eq!(placeholders("{a} and {b_c}, not {Up} or {}"), BTreeSet::from(["a", "b_c"]));
    }

    #[test]
    fn drops_translations_that_lose_or_change_values() {
        let mut t = Translation::default();
        t.strings.insert("status.loading_list".into(), "Chargement des {liste}…".into());
        t.strings.insert("menu.file".into(), "Fichier".into());
        t.strings.insert("menu.help".into(), "   ".into());
        t.strings.insert("no.such.key".into(), "Rien".into());
        let usable = t.usable();
        assert_eq!(usable.len(), 1);
        assert_eq!(usable["menu.file"], "Fichier");
        let missing = t.missing();
        assert!(missing.contains(&"status.loading_list"));
        assert!(missing.contains(&"menu.help"));
        assert!(!missing.contains(&"menu.file"));
        assert_eq!(missing.len(), ENGLISH.len() - 1);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stormcloud-i18n-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saves_loads_and_lists_translations() {
        let dir = temp_dir("list");
        let mut fr = Translation::new(&Language { code: "fr".into(), name: "Français".into(), english_name: "French".into() });
        fr.strings.insert("menu.file".into(), "Fichier".into());
        save(&dir, "fr", &fr).unwrap();
        assert_eq!(load(&dir, "fr").unwrap().strings["menu.file"], "Fichier");

        // A file for a language not in the built-in list is offered too.
        let eo = Translation { language: "Esperanto".into(), ..Translation::default() };
        save(&dir, "eo", &eo).unwrap();
        let languages = available(&dir);
        assert_eq!(languages[0].code, ENGLISH_CODE);
        assert!(languages.iter().any(|l| l.code == "eo" && l.label() == "Esperanto"));
        assert_eq!(languages.iter().filter(|l| l.code == "fr").count(), 1);

        assert!(load(&dir, "../settings").is_err());
        assert!(save(&dir, "", &eo).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_hidden_characters_and_overlong_text() {
        assert!(fits("menu.file", "Fichier"));
        // Right-to-left override, which can make text read backwards.
        assert!(!fits("menu.file", "Fich\u{202e}ier"));
        assert!(!fits("menu.file", "Fich\u{200b}ier"));
        assert!(!fits("menu.file", "Fich\u{7}ier"));
        assert!(!fits("menu.file", "Fich\nier"));
        assert!(!fits("menu.file", &"x".repeat(100)));
        assert_eq!(clean_name("  Esperanto "), Some("Esperanto".into()));
        assert_eq!(clean_name("Esper\u{202e}anto"), None);
        assert_eq!(clean_name(&"x".repeat(61)), None);
    }

    #[test]
    fn refuses_files_that_are_too_large() {
        let dir = temp_dir("large");
        let big = format!("{{\"language\": \"Big\", \"made_with\": \"{}\"}}", "x".repeat(MAX_FILE_BYTES as usize));
        std::fs::write(file_for(&dir, "xx"), big).unwrap();
        assert!(load(&dir, "xx").is_err());
        assert!(available(&dir).iter().all(|l| l.code != "xx"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn labels_languages() {
        let fr = Language { code: "fr".into(), name: "Français".into(), english_name: "French".into() };
        assert_eq!(fr.label(), "Français (French)");
        let en = Language { code: "en".into(), name: "English".into(), english_name: "English".into() };
        assert_eq!(en.label(), "English");
    }

    #[test]
    fn the_prompt_keeps_braces() {
        let fr = Language { code: "fr".into(), name: "Français".into(), english_name: "French".into() };
        let batch = BTreeMap::from([("status.loading_list", "Loading {list}…")]);
        let p = prompt(&fr, &batch);
        assert!(p.contains("such as {name} or {count}"));
        assert!(p.contains("French (Français)"));
        assert!(p.contains("\"status.loading_list\": \"Loading {list}…\""));
    }

    /// Needs Ollama running. Uses the model named in `STORMCLOUD_OLLAMA_MODEL`, or gemma3:4b.
    #[test]
    #[ignore]
    fn translates_with_a_local_model() {
        let model = std::env::var("STORMCLOUD_OLLAMA_MODEL").unwrap_or_else(|_| "gemma3:4b".into());
        let fr = Language { code: "fr".into(), name: "Français".into(), english_name: "French".into() };
        let keys: Vec<&str> = ENGLISH.iter().map(|(k, _)| *k).take(BATCH_SIZE).collect();
        let done = translate_batch(&model, &fr, &keys).unwrap();
        for (key, text) in &done {
            println!("{key}: {text}");
        }
        assert!(done.len() >= BATCH_SIZE * 3 / 4, "only {} of {BATCH_SIZE} came back usable", done.len());
    }
}
