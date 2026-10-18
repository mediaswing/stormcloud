//! OAuth 2.0 device authorisation grant against the Microsoft identity platform.

use crate::config::{token_path, Settings};
use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::fs;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: Option<u64>,
    pub message: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
    error_description: Option<String>,
}

#[derive(Default)]
struct Tokens {
    access_token: Option<String>,
    expires_at: Option<Instant>,
    refresh_token: Option<String>,
}

pub struct Auth {
    settings: Settings,
    http: reqwest::blocking::Client,
    tokens: Mutex<Tokens>,
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
        Auth {
            settings,
            http,
            tokens: Mutex::new(Tokens { refresh_token, ..Default::default() }),
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

    fn scopes(&self) -> String {
        let mut scopes: Vec<String> = Vec::new();
        for s in self.settings.scopes.split_whitespace() {
            let full = if s.contains("://") || s == "offline_access" {
                s.to_string()
            } else {
                format!("{}/{}", self.settings.graph_endpoint, s)
            };
            scopes.push(full);
        }
        scopes.push("offline_access".into());
        scopes.join(" ")
    }

    pub fn has_refresh_token(&self) -> bool {
        self.tokens.lock().unwrap().refresh_token.is_some()
    }

    pub fn start_device_code(&self) -> Result<DeviceCode> {
        log::info!(
            "Requesting device code: tenant={} client_id={} scopes=[{}]",
            self.settings.tenant_id.trim(),
            self.settings.client_id.trim(),
            self.scopes()
        );
        let resp = self
            .http
            .post(self.endpoint("devicecode"))
            .form(&[("client_id", self.settings.client_id.trim()), ("scope", &self.scopes())])
            .send()
            .inspect_err(|e| log::error!("Device code request failed: {e}"))?;
        if !resp.status().is_success() {
            let text = resp.text().unwrap_or_default();
            // Full AADSTS response, including trace and correlation IDs.
            log::error!("Device code request rejected: {text}");
            bail!(describe_error(text));
        }
        let dc: DeviceCode = resp.json()?;
        log::info!("Device code issued; expires in {}s, waiting for user", dc.expires_in);
        Ok(dc)
    }

    /// Blocks until the user completes (or abandons) the device code sign-in.
    pub fn poll_device_code(&self, dc: &DeviceCode) -> Result<()> {
        let mut interval = Duration::from_secs(dc.interval.unwrap_or(5).max(1));
        let deadline = Instant::now() + Duration::from_secs(dc.expires_in);
        loop {
            thread::sleep(interval);
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
                self.store(resp.json()?);
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

    fn refresh(&self, refresh_token: &str) -> Result<()> {
        log::debug!("Refreshing access token");
        let scopes = self.scopes();
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
            let text = resp.text().unwrap_or_default();
            log::warn!("Token refresh rejected; discarding cached sign-in: {text}");
            let mut t = self.tokens.lock().unwrap();
            *t = Tokens::default();
            let _ = fs::remove_file(token_path());
            bail!(describe_error(text));
        }
        self.store(resp.json()?);
        log::debug!("Access token refreshed");
        Ok(())
    }

    fn store(&self, tr: TokenResponse) {
        let mut t = self.tokens.lock().unwrap();
        t.access_token = Some(tr.access_token);
        t.expires_at = Some(Instant::now() + Duration::from_secs(tr.expires_in));
        if let Some(rt) = tr.refresh_token {
            if self.settings.remember_sign_in {
                save_refresh_token(&rt);
            }
            t.refresh_token = Some(rt);
        }
    }

    /// Returns a valid access token, refreshing it if it is close to expiry.
    pub fn token(&self) -> Result<String> {
        let refresh_token = {
            let t = self.tokens.lock().unwrap();
            if let (Some(at), Some(exp)) = (&t.access_token, t.expires_at)
                && exp > Instant::now() + Duration::from_secs(120) {
                    return Ok(at.clone());
                }
            t.refresh_token.clone()
        };
        let rt = refresh_token.ok_or_else(|| anyhow!("Not signed in. Choose File > Sign in."))?;
        self.refresh(&rt)?;
        self.tokens
            .lock()
            .unwrap()
            .access_token
            .clone()
            .ok_or_else(|| anyhow!("No access token"))
    }

    pub fn sign_out(&self) {
        log::info!("Signing out; clearing cached tokens");
        *self.tokens.lock().unwrap() = Tokens::default();
        let _ = fs::remove_file(token_path());
    }
}

fn save_refresh_token(rt: &str) {
    let path = token_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let body = serde_json::json!({ "refresh_token": rt }).to_string();
    if fs::write(&path, body).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }
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
