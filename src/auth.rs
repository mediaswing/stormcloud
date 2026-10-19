//! OAuth 2.0 device authorisation grant against the Microsoft identity platform.

use crate::config::{token_path, Settings};
use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The services the app gets access tokens for. One sign-in (refresh token) covers both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Api {
    #[default]
    Graph,
    Exchange,
}

/// The sign-in has not been granted the permissions an API needs, so someone has to consent.
#[derive(Debug)]
pub struct NeedsConsent(pub Api);

impl std::fmt::Display for NeedsConsent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Api::Exchange => f.write_str(
                "This app has not been granted the Exchange.Manage permission on Office 365 Exchange Online, \
                 which managing Exchange needs. An administrator can add it to the app registration's API \
                 permissions and grant admin consent.",
            ),
            Api::Graph => f.write_str("This app has not been granted the Microsoft Graph permissions it asks for."),
        }
    }
}

impl std::error::Error for NeedsConsent {}

/// The sign-in hasn't been granted these Microsoft Graph scopes, and asking for them without
/// signing in again was refused, so an administrator has to approve them.
#[derive(Debug)]
pub struct NeedsScopes(pub Vec<String>);

impl std::fmt::Display for NeedsScopes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "This needs Microsoft Graph permissions that haven't been granted to this app: {}. An administrator \
             can approve them by signing in when asked, or by adding them to the app registration's API \
             permissions and granting admin consent.",
            self.0.join(", ")
        )
    }
}

impl std::error::Error for NeedsScopes {}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: Option<u64>,
    pub message: String,
    /// The API the sign-in asks for scopes on.
    #[serde(skip)]
    pub api: Api,
    /// Set when the sign-in is cancelled, to stop polling and keep its tokens if it completes.
    #[serde(skip)]
    pub cancelled: Arc<AtomicBool>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
    /// The scopes the access token was granted, space-separated.
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
    error_description: Option<String>,
}

#[derive(Default)]
struct Tokens {
    /// Access tokens and when they expire, for each API.
    access: HashMap<Api, (String, Instant)>,
    /// The scopes the latest access token for each API was granted.
    granted: HashMap<Api, String>,
    refresh_token: Option<String>,
    /// Keep the refresh token on disk. Held with the tokens, so a refresh finishing on another
    /// thread can't write the file back after it was turned off.
    remember: bool,
}

pub struct Auth {
    settings: Settings,
    http: reqwest::blocking::Client,
    tokens: Mutex<Tokens>,
    /// The scopes asked for, which start as `settings.scopes` and grow when more are granted.
    scopes: Mutex<String>,
}

impl Auth {
    pub fn new(settings: Settings, http: reqwest::blocking::Client) -> Auth {
        let refresh_token = if settings.remember_sign_in {
            fs::read_to_string(token_path())
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v["refresh_token"].as_str().map(str::to_string))
        } else {
            None
        };
        let remember = settings.remember_sign_in;
        let scopes = Mutex::new(settings.scopes.clone());
        Auth {
            settings,
            http,
            tokens: Mutex::new(Tokens { refresh_token, remember, ..Default::default() }),
            scopes,
        }
    }

    /// Turns keeping the sign-in on disk on or off for this session, saving or deleting it now.
    pub fn set_remember(&self, remember: bool) {
        let mut t = self.tokens.lock().unwrap();
        t.remember = remember;
        match (&t.refresh_token, remember) {
            (Some(rt), true) => save_refresh_token(rt),
            (_, false) => {
                let _ = fs::remove_file(token_path());
            }
            (None, true) => {}
        }
    }

    fn endpoint(&self, leaf: &str) -> String {
        format!(
            "{}/{}/oauth2/v2.0/{}",
            self.settings.authority_host,
            self.settings.tenant_id.trim(),
            leaf
        )
    }

    /// The `scope` parameter for the scopes asked for on `api`, plus `extra`. A token request can
    /// only name scopes on one API.
    fn scopes_with(&self, api: Api, extra: &[String]) -> String {
        let mut scopes: Vec<String> = Vec::new();
        let (current, base) = match api {
            Api::Graph => (self.scopes.lock().unwrap().clone(), &self.settings.graph_endpoint),
            Api::Exchange => (self.settings.exchange_scopes.clone(), &self.settings.exchange_endpoint),
        };
        for s in current.split_whitespace().chain(extra.iter().map(String::as_str)) {
            let full = if s.contains("://") || s == "offline_access" {
                s.to_string()
            } else {
                format!("{base}/{s}")
            };
            scopes.push(full);
        }
        scopes.push("offline_access".into());
        scopes.join(" ")
    }

    pub fn has_refresh_token(&self) -> bool {
        self.tokens.lock().unwrap().refresh_token.is_some()
    }

    /// Starts a device code sign-in asking for the usual scopes on `api` plus `extra`, which are
    /// not added to the scopes asked for later until [`Auth::add_scopes`] is called.
    pub fn start_device_code(&self, api: Api, extra: &[String]) -> Result<DeviceCode> {
        let scopes = self.scopes_with(api, extra);
        log::info!(
            "Requesting device code: tenant={} client_id={} scopes=[{}]",
            self.settings.tenant_id.trim(),
            self.settings.client_id.trim(),
            scopes
        );
        let resp = self
            .http
            .post(self.endpoint("devicecode"))
            .form(&[("client_id", self.settings.client_id.trim()), ("scope", &scopes)])
            .send()
            .inspect_err(|e| log::error!("Device code request failed: {e}"))?;
        if !resp.status().is_success() {
            let text = resp.text().unwrap_or_default();
            // Full AADSTS response, including trace and correlation IDs.
            log::error!("Device code request rejected: {text}");
            bail!(describe_error(text));
        }
        let mut dc: DeviceCode = resp.json()?;
        dc.api = api;
        log::info!("Device code issued; expires in {}s, waiting for user", dc.expires_in);
        Ok(dc)
    }

    /// Blocks until the user completes (or abandons) the device code sign-in.
    pub fn poll_device_code(&self, dc: &DeviceCode) -> Result<()> {
        let mut interval = Duration::from_secs(dc.interval.unwrap_or(5).max(1));
        let deadline = Instant::now() + Duration::from_secs(dc.expires_in);
        loop {
            thread::sleep(interval);
            if dc.cancelled.load(Ordering::SeqCst) {
                log::info!("Device code sign-in cancelled");
                bail!("The sign-in was cancelled.");
            }
            if Instant::now() > deadline {
                log::warn!("Device code expired before sign-in completed");
                bail!("The sign-in code expired. Choose File > Sign in to try again.");
            }
            let resp = self
                .http
                .post(self.endpoint("token"))
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", self.settings.client_id.trim()),
                    ("device_code", dc.device_code.as_str()),
                ])
                .send()?;
            if resp.status().is_success() {
                if dc.cancelled.load(Ordering::SeqCst) {
                    log::info!("Device code sign-in completed after it was cancelled; not using it");
                    bail!("The sign-in was cancelled.");
                }
                let tr = resp.json()?;
                // Whoever signed in may not be who was signed in before, so access tokens for
                // other APIs, from the old sign-in, must not be used with the new one.
                let mut t = self.tokens.lock().unwrap();
                t.access.clear();
                t.granted.clear();
                drop(t);
                self.store(dc.api, tr);
                log::info!("Device code sign-in completed");
                return Ok(());
            }
            let text = resp.text().unwrap_or_default();
            match serde_json::from_str::<ErrorResponse>(&text) {
                Ok(e) if e.error == "authorization_pending" => {}
                Ok(e) if e.error == "slow_down" => {
                    log::debug!("Token endpoint asked us to slow down");
                    interval += Duration::from_secs(5)
                }
                _ => {
                    log::error!("Device code sign-in failed: {text}");
                    bail!(describe_error(text))
                }
            }
        }
    }

    fn refresh(&self, api: Api, refresh_token: &str) -> Result<()> {
        log::debug!("Refreshing {api:?} access token");
        let scopes = self.scopes_with(api, &[]);
        let resp = self
            .http
            .post(self.endpoint("token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.settings.client_id.trim()),
                ("refresh_token", refresh_token),
                ("scope", scopes.as_str()),
            ])
            .send()
            .inspect_err(|e| log::error!("Token refresh request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            if !sign_in_rejected(&text) {
                // An outage or throttling: the refresh token may still be good, so keep it.
                log::warn!("Token refresh failed with {status}; keeping the cached sign-in: {text}");
                bail!("Could not refresh the sign-in ({status}): {}", describe_error(text));
            }
            if api != Api::Graph {
                // The sign-in itself is fine; it just hasn't been granted this API.
                log::warn!("No {api:?} token for this sign-in: {text}");
                if text.contains("consent_required") || text.contains("AADSTS65001") {
                    return Err(NeedsConsent(api).into());
                }
                bail!(describe_error(text));
            }
            log::warn!("Token refresh rejected; discarding cached sign-in: {text}");
            let mut t = self.tokens.lock().unwrap();
            *t = Tokens { remember: t.remember, ..Default::default() };
            let _ = fs::remove_file(token_path());
            bail!(describe_error(text));
        }
        self.store(api, resp.json()?);
        log::debug!("{api:?} access token refreshed");
        Ok(())
    }

    /// Adds `extra` to the scopes asked for from now on.
    pub fn add_scopes(&self, extra: &[String]) {
        let mut scopes = self.scopes.lock().unwrap();
        *scopes = scopes.split_whitespace().map(str::to_string).chain(extra.iter().cloned()).collect::<Vec<_>>().join(" ");
    }

    /// Asks for `extra` scopes as well, using the refresh token, so that no one has to sign in.
    /// Returns true when they were granted (and adds them), or false when they need consent,
    /// leaving the sign-in as it was.
    pub fn try_add_scopes(&self, extra: &[String]) -> Result<bool> {
        let rt = self.tokens.lock().unwrap().refresh_token.clone().ok_or_else(|| anyhow!("Not signed in."))?;
        let scopes = self.scopes_with(Api::Graph, extra);
        log::debug!("Asking for more scopes: [{}]", extra.join(" "));
        let resp = self
            .http
            .post(self.endpoint("token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.settings.client_id.trim()),
                ("refresh_token", rt.as_str()),
                ("scope", scopes.as_str()),
            ])
            .send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            if text.contains("consent_required") || text.contains("AADSTS65001") {
                log::info!("More scopes need consent: {text}");
                return Ok(false);
            }
            log::warn!("Asking for more scopes failed with {status}: {text}");
            bail!("Could not ask for more permissions ({status}): {}", describe_error(text));
        }
        self.store(Api::Graph, resp.json()?);
        self.add_scopes(extra);
        log::info!("Granted more scopes: [{}]", extra.join(" "));
        Ok(true)
    }

    /// The scopes asked for on Microsoft Graph, including any added since signing in.
    pub fn scopes(&self) -> String {
        self.scopes.lock().unwrap().clone()
    }

    /// Makes sure the Graph sign-in has `wanted` scopes, asking for any it lacks without signing
    /// in again. Fails with [`NeedsScopes`] when they need an administrator's approval.
    pub fn ensure_scopes(&self, wanted: &[&str]) -> Result<()> {
        self.token_for(Api::Graph)?;
        let granted = self.tokens.lock().unwrap().granted.get(&Api::Graph).cloned().unwrap_or_default();
        // A token response without `scope` says nothing either way, so go by what was asked for.
        let have = if granted.trim().is_empty() { self.scopes() } else { granted };
        let missing = crate::config::missing_scopes(&have, wanted);
        if missing.is_empty() || self.try_add_scopes(&missing)? {
            Ok(())
        } else {
            Err(NeedsScopes(missing).into())
        }
    }

    fn store(&self, api: Api, tr: TokenResponse) {
        let mut t = self.tokens.lock().unwrap();
        t.granted.insert(api, tr.scope);
        t.access.insert(api, (tr.access_token, Instant::now() + Duration::from_secs(tr.expires_in)));
        if let Some(rt) = tr.refresh_token {
            if t.remember {
                save_refresh_token(&rt);
            }
            t.refresh_token = Some(rt);
        }
    }

    /// Returns a valid access token for `api`, refreshing it if it is close to expiry. Fails
    /// with [`NeedsConsent`] when the sign-in hasn't been granted that API.
    pub fn token_for(&self, api: Api) -> Result<String> {
        let refresh_token = {
            let t = self.tokens.lock().unwrap();
            if let Some((at, exp)) = t.access.get(&api)
                && *exp > Instant::now() + Duration::from_secs(120) {
                    return Ok(at.clone());
                }
            t.refresh_token.clone()
        };
        let rt = refresh_token.ok_or_else(|| anyhow!("Not signed in. Choose File > Sign in."))?;
        self.refresh(api, &rt)?;
        self.tokens
            .lock()
            .unwrap()
            .access
            .get(&api)
            .map(|(at, _)| at.clone())
            .ok_or_else(|| anyhow!("No access token"))
    }

    pub fn sign_out(&self) {
        log::info!("Signing out; clearing cached tokens");
        let mut t = self.tokens.lock().unwrap();
        *t = Tokens { remember: t.remember, ..Default::default() };
        let _ = fs::remove_file(token_path());
    }
}

fn save_refresh_token(rt: &str) {
    let path = token_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let body = serde_json::json!({ "refresh_token": rt }).to_string();
    if let Err(e) = write_private(&path, body.as_bytes()) {
        log::error!("Could not save the sign-in to {}: {e}", path.display());
    }
}

/// Writes a file only its owner can read. On Unix the file is created owner-only, and an existing
/// file is made owner-only before anything is written to it, so the token is never readable by
/// other users, even briefly.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    options.open(path)?.write_all(bytes)
}

/// True when the token endpoint refused the refresh token itself (expired, revoked, or needing
/// the user to sign in again), as opposed to failing for a while.
fn sign_in_rejected(text: &str) -> bool {
    serde_json::from_str::<ErrorResponse>(text).is_ok_and(|e| {
        matches!(e.error.as_str(), "invalid_grant" | "interaction_required" | "invalid_client" | "unauthorized_client")
    })
}

fn describe_error(text: String) -> String {
    match serde_json::from_str::<ErrorResponse>(&text) {
        Ok(e) => {
            let desc = e.error_description.unwrap_or_default();
            // AADSTS descriptions carry trace/correlation IDs after the first line.
            let first = desc.lines().next().unwrap_or("").to_string();
            format!("{}: {}", e.error, first)
        }
        Err(_) => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_granted_scopes() {
        let tr: TokenResponse =
            serde_json::from_str(r#"{"access_token":"a","expires_in":3600,"scope":"User.Read AuditLog.Read.All"}"#).unwrap();
        assert_eq!(tr.scope, "User.Read AuditLog.Read.All");
        let tr: TokenResponse = serde_json::from_str(r#"{"access_token":"a","expires_in":3600}"#).unwrap();
        assert!(tr.scope.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("stormcloud-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token.json");
        // An older version left the file readable by everyone.
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"new").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        fs::remove_dir_all(&dir).unwrap();
    }
}
