//! Checks GitHub for a newer release than the one running.

use crate::app::App;
use crate::dialogs::Dialog;
use crate::i18n::{t, tf};
use anyhow::{bail, Result};
use serde_json::Value;
use std::time::Duration;

pub struct Release {
    /// The version, without the tag's leading `v`.
    pub version: String,
    /// The release's page, with its downloads.
    pub url: String,
    pub notes: String,
}

/// The GitHub API address of the latest release, from the repository in Cargo.toml.
fn latest_url() -> String {
    let repo = env!("CARGO_PKG_REPOSITORY").trim_end_matches('/');
    format!("{}/releases/latest", repo.replace("https://github.com/", "https://api.github.com/repos/"))
}

/// Fetches the latest release (runs on a worker thread). Drafts and pre-releases are never
/// the latest.
fn latest() -> Result<Release> {
    let http = reqwest::blocking::Client::builder()
        .user_agent(concat!("StormCloud/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()?;
    let url = latest_url();
    let resp = http.get(&url).header("Accept", "application/vnd.github+json").send()?;
    let status = resp.status();
    log::debug!("GET {url} -> {status}");
    if !status.is_success() {
        bail!(tf("update.github_error", &[("status", &status)]));
    }
    let v: Value = resp.json()?;
    let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Ok(Release {
        version: text("tag_name").trim_start_matches('v').to_string(),
        url: text("html_url"),
        notes: text("body").trim().to_string(),
    })
}

/// "1.2.0" as numbers; anything after a `-` (a pre-release) is ignored.
fn parse(v: &str) -> Option<Vec<u64>> {
    v.split('-').next()?.split('.').map(|p| p.parse().ok()).collect()
}

/// True when `candidate` is a later version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

impl App {
    /// Looks for a newer release in the background. A check the user asked for (`manual`) says
    /// when there is none or it failed, and shows a skipped version again; the check at
    /// startup only speaks up about a newer version that hasn't been skipped.
    pub fn check_for_updates(&mut self, manual: bool) {
        if manual {
            self.status = t("update.checking");
        }
        self.spawn_plain(latest, move |app, r| {
            let current = env!("CARGO_PKG_VERSION");
            match r {
                Ok(release) if is_newer(&release.version, current) => {
                    if manual {
                        app.status = t("status.ready");
                    }
                    log::info!("Version {} is available (running {current})", release.version);
                    if manual || release.version != app.settings.skipped_version {
                        app.push(Dialog::Update(release));
                    }
                }
                Ok(release) => {
                    log::debug!("Up to date: latest release is {}, running {current}", release.version);
                    if manual {
                        app.status = t("status.ready");
                        app.push(Dialog::message(&t("update.check_title"), &tf("update.latest", &[("version", &current)])));
                    }
                }
                Err(e) => {
                    log::warn!("Could not check for updates: {e}");
                    if manual {
                        app.error(&t("update.check_title"), e);
                    }
                }
            }
        });
    }

    /// Stops the startup check showing `version` again.
    pub fn skip_version(&mut self, version: &str) {
        self.settings.skipped_version = version.to_string();
        if let Err(e) = self.settings.save() {
            log::error!("Could not save settings: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("1.2.0", "1.1.0"));
        assert!(is_newer("1.10.0", "1.9.3"));
        assert!(is_newer("2.0", "1.9.9"));
        assert!(!is_newer("1.1.0", "1.1.0"));
        assert!(!is_newer("1.0.9", "1.1.0"));
        assert!(!is_newer("nonsense", "1.1.0"));
        assert!(!is_newer("1.1.0-beta", "1.1.0"));
    }

    #[test]
    fn asks_the_github_api() {
        assert_eq!(latest_url(), "https://api.github.com/repos/mediaswing/stormcloud/releases/latest");
    }
}
