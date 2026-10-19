//! Settings persisted to `settings.ini` in the per-user configuration folder:
//! `%APPDATA%\StormCloud` on Windows, `~/Library/Application Support/StormCloud`
//! on macOS and `$XDG_CONFIG_HOME/stormcloud` (usually `~/.config`) on Linux.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_SCOPES: &str = "User.ReadWrite.All Group.ReadWrite.All Device.ReadWrite.All \
Directory.Read.All RoleManagement.ReadWrite.Directory Organization.Read.All \
LicenseAssignment.ReadWrite.All User-PasswordProfile.ReadWrite.All Application.ReadWrite.All \
AppRoleAssignment.ReadWrite.All DelegatedPermissionGrant.ReadWrite.All";

/// Scopes added in 1.2.0 for the Applications node. A saved `scopes` setting without them gets
/// them after signing in, once they are granted (see `App::add_app_scopes`).
pub const APP_SCOPES: [&str; 3] =
    ["Application.ReadWrite.All", "AppRoleAssignment.ReadWrite.All", "DelegatedPermissionGrant.ReadWrite.All"];

/// The scopes in `wanted` missing from the space-separated `scopes`, which may name them in full
/// (`https://graph.microsoft.com/User.Read`) or not, in any case.
pub fn missing_scopes(scopes: &str, wanted: &[&str]) -> Vec<String> {
    let have: Vec<String> = scopes
        .split_whitespace()
        .map(|s| s.rsplit('/').next().unwrap_or(s).to_ascii_lowercase())
        .collect();
    wanted.iter().filter(|w| !have.contains(&w.to_ascii_lowercase())).map(|w| w.to_string()).collect()
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub tenant_id: String,
    pub client_id: String,
    /// Keep the refresh token on disk between runs.
    pub remember_sign_in: bool,
    pub scopes: String,
    /// The administrator has been asked to approve [`APP_SCOPES`], so they are not asked again.
    pub app_scopes_offered: bool,
    pub authority_host: String,
    pub graph_endpoint: String,
    /// off, error, warn, info, debug or trace.
    pub log_level: String,
    /// The MariaDB server last exported to. The password is never saved.
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_name: String,
    pub db_tls: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            tenant_id: String::new(),
            client_id: String::new(),
            remember_sign_in: false,
            scopes: DEFAULT_SCOPES.to_string(),
            app_scopes_offered: false,
            authority_host: "https://login.microsoftonline.com".to_string(),
            graph_endpoint: "https://graph.microsoft.com".to_string(),
            log_level: "debug".to_string(),
            db_host: String::new(),
            db_port: crate::db::DEFAULT_PORT,
            db_user: String::new(),
            db_name: String::new(),
            db_tls: false,
        }
    }
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "linux") {
        base.join("stormcloud")
    } else {
        base.join("StormCloud")
    }
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.ini")
}

pub fn token_path() -> PathBuf {
    config_dir().join("token.json")
}

impl Settings {
    pub fn is_complete(&self) -> bool {
        !self.tenant_id.trim().is_empty() && !self.client_id.trim().is_empty()
    }

    pub fn load() -> Result<Settings> {
        let path = settings_path();
        let mut s = Settings::default();
        let Ok(text) = fs::read_to_string(&path) else {
            return Ok(s);
        };
        let ini = parse_ini(&text);
        let get = |sec: &str, key: &str| ini.get(&(sec.to_string(), key.to_string())).cloned();
        if let Some(v) = get("auth", "tenant_id") {
            s.tenant_id = v;
        }
        if let Some(v) = get("auth", "client_id") {
            s.client_id = v;
        }
        if let Some(v) = get("auth", "remember_sign_in") {
            s.remember_sign_in = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
        }
        if let Some(v) = get("auth", "scopes").filter(|v| !v.is_empty()) {
            s.scopes = v;
        }
        if let Some(v) = get("auth", "app_scopes_offered") {
            s.app_scopes_offered = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
        }
        if let Some(v) = get("cloud", "authority_host").filter(|v| !v.is_empty()) {
            s.authority_host = v.trim_end_matches('/').to_string();
        }
        if let Some(v) = get("cloud", "graph_endpoint").filter(|v| !v.is_empty()) {
            s.graph_endpoint = v.trim_end_matches('/').to_string();
        }
        if let Some(v) = get("logging", "level").filter(|v| !v.is_empty()) {
            s.log_level = v;
        }
        if let Some(v) = get("mariadb", "host") {
            s.db_host = v;
        }
        if let Some(v) = get("mariadb", "port").and_then(|v| v.parse().ok()) {
            s.db_port = v;
        }
        if let Some(v) = get("mariadb", "user") {
            s.db_user = v;
        }
        if let Some(v) = get("mariadb", "database") {
            s.db_name = v;
        }
        if let Some(v) = get("mariadb", "tls") {
            s.db_tls = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
        }
        Ok(s)
    }

    pub fn save(&self) -> Result<PathBuf> {
        let dir = config_dir();
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = settings_path();
        let text = format!(
            "; Graphical Cloud Management settings\n\
             ; The app registration must allow public client flows (device code).\n\
             [auth]\n\
             tenant_id = {}\n\
             client_id = {}\n\
             remember_sign_in = {}\n\
             ; Space-separated delegated Microsoft Graph permissions to request.\n\
             scopes = {}\n\
             ; Set once you have been asked to approve the permissions for Applications.\n\
             app_scopes_offered = {}\n\
             \n\
             [cloud]\n\
             authority_host = {}\n\
             graph_endpoint = {}\n\
             \n\
             [logging]\n\
             ; Written to output.log in this folder: off, error, warn, info, debug or trace.\n\
             level = {}\n\
             \n\
             [mariadb]\n\
             ; The server for Export to MariaDB. The password is asked for each time.\n\
             host = {}\n\
             port = {}\n\
             user = {}\n\
             database = {}\n\
             tls = {}\n",
            self.tenant_id.trim(),
            self.client_id.trim(),
            self.remember_sign_in,
            self.scopes.trim(),
            self.app_scopes_offered,
            self.authority_host,
            self.graph_endpoint,
            self.log_level.trim(),
            self.db_host.trim(),
            self.db_port,
            self.db_user.trim(),
            self.db_name.trim(),
            self.db_tls,
        );
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }
}

fn parse_ini(text: &str) -> BTreeMap<(String, String), String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
        } else if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().trim_matches('"').to_string();
            out.insert((section.clone(), k.trim().to_ascii_lowercase()), v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_comments() {
        let ini = parse_ini("; c\n[Auth]\nTenant_ID = abc\nclient_id=\"def\"\n[cloud]\nx = 1\n");
        assert_eq!(ini[&("auth".into(), "tenant_id".into())], "abc");
        assert_eq!(ini[&("auth".into(), "client_id".into())], "def");
        assert_eq!(ini[&("cloud".into(), "x".into())], "1");
    }

    #[test]
    fn finds_missing_scopes() {
        assert!(missing_scopes(DEFAULT_SCOPES, &APP_SCOPES).is_empty());
        let old = "User.ReadWrite.All https://graph.microsoft.com/application.readwrite.all";
        assert_eq!(missing_scopes(old, &APP_SCOPES), ["AppRoleAssignment.ReadWrite.All", "DelegatedPermissionGrant.ReadWrite.All"]);
    }
}
