//! A user's authentication and security: the Authentication & security, Authentication methods
//! and Sign-ins tabs of a user's properties, and the actions on them (revoking sessions,
//! Temporary Access Passes, deleting methods and requiring MFA re-registration).
//!
//! Everything uses documented Microsoft Graph v1.0 APIs. The permissions it needs beyond the
//! default scopes, [`AUTH_METHODS_SCOPE`] and [`AUDIT_LOG_SCOPE`], are asked for the first time
//! they are needed (see `Auth::ensure_scopes`). Phone numbers and email addresses used for
//! authentication are shown masked.

use crate::app::App;
use crate::auth::NeedsScopes;
use crate::config::{AUDIT_LOG_SCOPE, AUTH_METHODS_SCOPE};
use crate::dialogs::{Dialog, Field, Form};
use crate::graph::Graph;
use crate::i18n::{t, tf};
use crate::model::{datetime, s, yes_no};
use crate::sheets::{SheetRow, TabBody, TabKind, TabOp};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// What an operation needed, to explain a refusal from Graph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Need {
    /// Reading sign-in logs, sign-in activity or MFA registration details.
    Reports,
    /// Reading or changing another user's authentication methods.
    Methods,
    /// Revoking sessions or changing the account.
    Users,
}

/// Fails with [`NeedsScopes`] unless the sign-in has `scope`, asking for it quietly first.
fn require(g: &Graph, scope: &str) -> Result<()> {
    g.auth.ensure_scopes(&[scope])
}

/// The error Graph gave, with what it means when the cause is a missing licence or role.
pub fn explain(e: anyhow::Error, need: Need) -> anyhow::Error {
    if e.downcast_ref::<NeedsScopes>().is_some() {
        return e;
    }
    let text = e.to_string();
    if text.contains("RequestFromNonPremiumTenant") || text.contains("premium license") {
        return anyhow!(tf("security.needs_premium", &[("error", &text)]));
    }
    if text.contains("(403)") {
        let roles = t(match need {
            Need::Reports => "security.roles_reports",
            Need::Methods => "security.roles_methods",
            Need::Users => "security.roles_users",
        });
        return anyhow!(tf("security.not_allowed", &[("roles", &roles), ("error", &text)]));
    }
    e
}

/// A short reason for a part of the overview that couldn't be read.
fn unavailable(e: &anyhow::Error) -> String {
    if let Some(NeedsScopes(missing)) = e.downcast_ref::<NeedsScopes>() {
        return tf("security.unavailable_scope", &[("scopes", &missing.join(", "))]);
    }
    let text = e.to_string();
    if text.contains("RequestFromNonPremiumTenant") || text.contains("premium license") {
        t("security.unavailable_premium")
    } else if text.contains("(403)") {
        t("security.unavailable_role")
    } else {
        tf("security.unavailable", &[("error", &text)])
    }
}

// ----- authentication methods ---------------------------------------------------

/// The method type from `@odata.type`, such as `phoneAuthenticationMethod`.
fn method_type(v: &Value) -> &str {
    v["@odata.type"].as_str().unwrap_or("").trim_start_matches("#microsoft.graph.")
}

/// The collection under `/users/{id}/authentication/` each deletable method type lives in.
fn method_collection(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "microsoftAuthenticatorAuthenticationMethod" => "microsoftAuthenticatorMethods",
        "phoneAuthenticationMethod" => "phoneMethods",
        "fido2AuthenticationMethod" => "fido2Methods",
        "windowsHelloForBusinessAuthenticationMethod" => "windowsHelloForBusinessMethods",
        "emailAuthenticationMethod" => "emailMethods",
        "softwareOathAuthenticationMethod" => "softwareOathMethods",
        "temporaryAccessPassAuthenticationMethod" => "temporaryAccessPassMethods",
        "platformCredentialAuthenticationMethod" => "platformCredentialMethods",
        "externalAuthenticationMethod" => "externalAuthenticationMethods",
        _ => return None,
    })
}

/// The path to DELETE to remove a method, or None for the password and methods Graph can't delete.
pub fn delete_path(user_id: &str, v: &Value) -> Option<String> {
    method_collection(method_type(v)).map(|c| format!("/users/{user_id}/authentication/{c}/{}", s(v, "id")))
}

/// Methods deleted to require MFA re-registration: those used as a second factor. Device-bound
/// credentials (Windows Hello for Business, platform credentials), the SSPR email address and
/// any Temporary Access Pass are kept, as is the password.
pub fn is_mfa_method(v: &Value) -> bool {
    matches!(
        method_type(v),
        "microsoftAuthenticatorAuthenticationMethod"
            | "phoneAuthenticationMethod"
            | "softwareOathAuthenticationMethod"
            | "fido2AuthenticationMethod"
    )
}

/// Only the country code and last three digits of a phone number.
pub fn mask_phone(number: &str) -> String {
    let (cc, rest) = match number.trim().split_once(' ') {
        Some((cc, rest)) if cc.starts_with('+') => (format!("{cc} "), rest),
        _ => (String::new(), number),
    };
    let digits: Vec<char> = rest.split(['x', 'X']).next().unwrap_or("").chars().filter(char::is_ascii_digit).collect();
    let last: String = digits[digits.len().saturating_sub(3)..].iter().collect();
    format!("{cc}•••{last}")
}

/// The first letter of the mailbox part and the domain of an email address.
pub fn mask_email(address: &str) -> String {
    match address.split_once('@') {
        Some((user, domain)) => format!("{}•••@{domain}", user.chars().next().unwrap_or('•')),
        None => "•••".into(),
    }
}

/// A method's name and details for the table, with phone numbers and addresses masked.
pub fn describe_method(v: &Value) -> (String, String) {
    // A property's value, or with `label_key` the text for that key with the value in it.
    let opt = |label_key: Option<&str>, key: &str| {
        let value = s(v, key);
        match label_key {
            _ if value.is_empty() => None,
            Some(label) => Some(tf(label, &[("value", &value)])),
            None => Some(value),
        }
    };
    let join = |parts: Vec<Option<String>>| parts.into_iter().flatten().collect::<Vec<_>>().join(" · ");
    match method_type(v) {
        "passwordAuthenticationMethod" => (t("method.password"), t("method.password_details")),
        "microsoftAuthenticatorAuthenticationMethod" => (
            t("method.authenticator"),
            join(vec![opt(None, "displayName"), opt(None, "deviceTag"), opt(Some("method.app_version"), "phoneAppVersion")]),
        ),
        "phoneAuthenticationMethod" => {
            let kind = t(match s(v, "phoneType").as_str() {
                "mobile" => "method.mobile_phone",
                "alternateMobile" => "method.alternate_mobile_phone",
                "office" => "method.office_phone",
                _ => "method.phone",
            });
            let mut details = tf("method.ending", &[("number", &mask_phone(&s(v, "phoneNumber")))]);
            if s(v, "smsSignInState") == "ready" {
                details = format!("{details} · {}", t("method.sms_sign_in"));
            }
            (kind, details)
        }
        "fido2AuthenticationMethod" => {
            (t("method.passkey"), join(vec![opt(None, "displayName"), opt(Some("method.model"), "model")]))
        }
        "windowsHelloForBusinessAuthenticationMethod" => (
            t("method.windows_hello"),
            join(vec![opt(None, "displayName"), opt(Some("method.key_strength"), "keyStrength")]),
        ),
        "emailAuthenticationMethod" => (t("method.email"), mask_email(&s(v, "emailAddress"))),
        "softwareOathAuthenticationMethod" => (t("method.oath"), String::new()),
        "temporaryAccessPassAuthenticationMethod" => {
            let mut parts = vec![tf("method.minutes", &[("count", &v["lifetimeInMinutes"].as_i64().unwrap_or(0))])];
            if v["isUsableOnce"] == true {
                parts.push(t("method.one_use"));
            }
            parts.push(if v["isUsable"] == true {
                t("method.usable_now")
            } else {
                tf("method.not_usable", &[("reason", &s(v, "methodUsabilityReason"))])
            });
            (t("method.tap"), parts.join(" · "))
        }
        "platformCredentialAuthenticationMethod" => {
            (t("method.platform_credential"), join(vec![opt(None, "displayName"), opt(None, "platform")]))
        }
        "externalAuthenticationMethod" => (t("method.external"), opt(None, "displayName").unwrap_or_default()),
        other => (other.trim_end_matches("AuthenticationMethod").to_string(), String::new()),
    }
}

/// What deleting a method does, for its confirmation.
fn delete_effect(v: &Value) -> String {
    t(match method_type(v) {
        "phoneAuthenticationMethod" => "method.delete_phone",
        "microsoftAuthenticatorAuthenticationMethod" => "method.delete_authenticator",
        "fido2AuthenticationMethod" => "method.delete_passkey",
        "windowsHelloForBusinessAuthenticationMethod" => "method.delete_windows_hello",
        "emailAuthenticationMethod" => "method.delete_email",
        "temporaryAccessPassAuthenticationMethod" => "method.delete_tap",
        _ => "method.delete_other",
    })
}

/// The order MFA methods are deleted in: phone numbers last, as one that is the default method
/// can't be deleted while other methods remain, and the mobile number after the alternate
/// mobile and office numbers, as it can't be deleted while an alternate mobile remains.
fn deletion_order(m: &Value) -> u8 {
    if method_type(m) != "phoneAuthenticationMethod" {
        0
    } else if m["phoneType"] == "mobile" {
        2
    } else {
        1
    }
}

fn load_methods(g: &Graph, user_id: &str) -> Result<Vec<Value>> {
    require(g, AUTH_METHODS_SCOPE)?;
    g.list(&format!("/users/{user_id}/authentication/methods")).map_err(|e| explain(e, Need::Methods))
}

fn methods_table(methods: Vec<Value>) -> TabBody {
    let mut rows: Vec<SheetRow> = methods
        .into_iter()
        .map(|v| {
            let (name, details) = describe_method(&v);
            SheetRow { cells: vec![name, details, datetime(&v["createdDateTime"])], data: v }
        })
        .collect();
    // The password first, then the rest by name.
    rows.sort_by_key(|r| (method_type(&r.data) != "passwordAuthenticationMethod", r.cells[0].clone()));
    TabBody::Table { headers: vec!["col.method", "col.details", "col.added_utc"], rows }
}

// ----- sign-ins ----------------------------------------------------------------

/// Error codes that interrupt a sign-in to ask for something, which is usually completed straight
/// after, rather than failing it.
const INTERRUPTS: [i64; 8] = [50058, 50072, 50074, 50076, 50079, 50125, 50140, 50199];

/// What a sign-in error code usually means, and what to do about it.
pub fn advice(code: i64) -> Option<String> {
    Some(t(match code {
        50126 => "advice.50126",
        50053 => "advice.50053",
        50057 => "advice.50057",
        50055 => "advice.50055",
        50144 => "advice.50144",
        50056 => "advice.50056",
        50034 => "advice.50034",
        50074 | 50076 => "advice.50074",
        500121 => "advice.500121",
        50072 | 50079 => "advice.50072",
        53003 => "advice.53003",
        53000 => "advice.53000",
        53001 => "advice.53001",
        50097 => "advice.50097",
        50105 => "advice.50105",
        50133 | 50173 => "advice.50133",
        70044 => "advice.70044",
        50140 => "advice.50140",
        50158 => "advice.50158",
        50199 => "advice.50199",
        50058 => "advice.50058",
        65001 => "advice.65001",
        90094 => "advice.90094",
        50020 => "advice.50020",
        _ => return None,
    }))
}

/// "Success", "Interrupted (50140)" or "Failed (50126)".
pub fn sign_in_result(v: &Value) -> String {
    let code = v["status"]["errorCode"].as_i64().unwrap_or(0);
    if code == 0 {
        t("signins.success")
    } else if INTERRUPTS.contains(&code) {
        tf("signins.interrupted", &[("code", &code)])
    } else {
        tf("signins.failed", &[("code", &code)])
    }
}

fn location(v: &Value) -> String {
    let l = &v["location"];
    [s(l, "city"), s(l, "state"), s(l, "countryOrRegion")]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn conditional_access(status: &str) -> String {
    match status {
        "success" => t("signins.ca_passed"),
        "failure" => t("signins.ca_failed"),
        "notApplied" => t("signins.ca_not_applied"),
        other => other.to_string(),
    }
}

/// The full details of a sign-in, for its Details dialog.
pub fn sign_in_details(v: &Value) -> String {
    let code = v["status"]["errorCode"].as_i64().unwrap_or(0);
    // A line of the details: the text for `key`, such as "User: {value}", with `value` in it.
    let line = |key: &str, value: &dyn std::fmt::Display| tf(key, &[("value", value)]);
    let mut lines = vec![
        line("signins.date", &datetime(&v["createdDateTime"])),
        line("signins.user", &s(v, "userPrincipalName")),
        line("signins.application", &s(v, "appDisplayName")),
        line("signins.resource", &s(v, "resourceDisplayName")),
        line("signins.result", &sign_in_result(v)),
    ];
    for (label, key) in [("signins.reason", "failureReason"), ("signins.additional", "additionalDetails")] {
        let text = s(&v["status"], key);
        if !text.is_empty() && code != 0 {
            lines.push(line(label, &text));
        }
    }
    if let Some(a) = advice(code) {
        lines.push(line("signins.meaning", &a));
    }
    let d = &v["deviceDetail"];
    let device = [s(d, "displayName"), s(d, "operatingSystem"), s(d, "browser")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", ");
    lines.extend([
        line("signins.interactive", &yes_no(&v["isInteractive"])),
        line("signins.client_app", &s(v, "clientAppUsed")),
        line("signins.ip", &s(v, "ipAddress")),
        line("signins.location", &location(v)),
        line("signins.device", &device),
        tf("signins.device_state", &[("compliant", &yes_no(&d["isCompliant"])), ("managed", &yes_no(&d["isManaged"]))]),
        line("signins.conditional_access", &conditional_access(&s(v, "conditionalAccessStatus"))),
    ]);
    match v["appliedConditionalAccessPolicies"].as_array() {
        Some(policies) => {
            // Policies that weren't enabled or didn't apply say nothing about this sign-in.
            let relevant: Vec<String> = policies
                .iter()
                .filter(|p| !matches!(s(p, "result").as_str(), "notEnabled" | "notApplied"))
                .map(|p| {
                    let grants = p["enforcedGrantControls"].as_array().map(|g| g.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    let grants = if grants.is_empty() { String::new() } else { format!(" ({})", tf("signins.requires", &[("grants", &grants)])) };
                    format!("  • {}: {}{grants}", s(p, "displayName"), s(p, "result"))
                })
                .collect();
            if relevant.is_empty() {
                lines.push(t("signins.no_policies"));
            } else {
                lines.push(t("signins.policies"));
                lines.extend(relevant);
            }
        }
        None => lines.push(t("signins.policies_hidden")),
    }
    let risk = s(v, "riskLevelDuringSignIn");
    if !risk.is_empty() && risk != "hidden" && risk != "none" {
        lines.push(tf("signins.risk", &[("risk", &risk), ("state", &s(v, "riskState"))]));
    }
    lines.push(line("signins.correlation_id", &s(v, "correlationId")));
    lines.push(line("signins.request_id", &s(v, "id")));
    lines.join("\n")
}

/// The most recent interactive sign-ins (one page; the newest come first).
fn recent_sign_ins(g: &Graph, user_id: &str, top: u32) -> Result<Vec<Value>> {
    require(g, AUDIT_LOG_SCOPE)?;
    let v = g
        .get(&format!("/auditLogs/signIns?$filter=userId eq '{user_id}'&$top={top}"))
        .map_err(|e| explain(e, Need::Reports))?;
    Ok(v["value"].as_array().cloned().unwrap_or_default())
}

fn sign_ins_table(items: Vec<Value>) -> TabBody {
    crate::sheets::table(
        vec!["col.date_utc", "col.application", "col.result", "col.ip_address", "col.location", "col.conditional_access"],
        items,
        |v| {
            vec![
                datetime(&v["createdDateTime"]),
                s(v, "appDisplayName"),
                sign_in_result(v),
                s(v, "ipAddress"),
                location(v),
                conditional_access(&s(v, "conditionalAccessStatus")),
            ]
        },
    )
}

// ----- the overview --------------------------------------------------------------

/// What is known about a user's sign-in, for [`diagnose`]. `None` means it couldn't be read.
#[derive(Default)]
pub struct Facts {
    pub enabled: Option<bool>,
    pub synced: bool,
    pub guest: bool,
    pub methods: Option<Vec<Value>>,
    /// Newest first.
    pub sign_ins: Option<Vec<Value>>,
    pub sessions_valid_from: String,
    pub created: String,
    pub password_changed: String,
}

/// Seconds since 1970 of a Graph date and time such as `2026-10-19T08:00:00Z` (read as UTC).
fn seconds(t: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| t.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

/// Whether sessions were revoked: Graph also sets the time sessions are valid from when the
/// account is created and when its password changes, so only a later time means a revocation.
fn sessions_revoked(f: &Facts) -> bool {
    let Some(from) = seconds(&f.sessions_valid_from) else { return false };
    let since = [&f.created, &f.password_changed].iter().filter_map(|t| seconds(t)).max().unwrap_or(i64::MIN);
    from > since.saturating_add(120)
}

/// Plain findings about why a user might be unable to sign in, most important first.
pub fn diagnose(f: &Facts) -> Vec<String> {
    let mut out = Vec::new();
    if f.enabled == Some(false) {
        out.push(t(if f.synced { "diagnose.disabled_synced" } else { "diagnose.disabled" }));
    }
    if f.synced {
        out.push(t("diagnose.synced"));
    }
    if f.guest {
        out.push(t("diagnose.guest"));
    }
    if let Some(methods) = &f.methods {
        let tap = methods.iter().find(|m| method_type(m) == "temporaryAccessPassAuthenticationMethod");
        if !methods.iter().any(is_mfa_method) && !f.guest {
            out.push(t("diagnose.no_mfa"));
        }
        if let Some(pass) = tap {
            let key = if pass["isUsable"] == true { "diagnose.tap_usable" } else { "diagnose.tap_unusable" };
            out.push(tf(key, &[("created", &datetime(&pass["createdDateTime"]))]));
        }
    }
    if let Some(sign_ins) = &f.sign_ins {
        let code = |v: &Value| v["status"]["errorCode"].as_i64().unwrap_or(0);
        match sign_ins.first() {
            None => out.push(t("diagnose.no_sign_ins")),
            Some(latest) if code(latest) == 0 => {
                out.push(tf("diagnose.latest_succeeded", &[("time", &datetime(&latest["createdDateTime"]))]));
            }
            Some(_) => {}
        }
        if let Some(failed) = sign_ins.iter().find(|v| code(v) != 0 && !INTERRUPTS.contains(&code(v))) {
            let c = code(failed);
            let args: [(&str, &dyn std::fmt::Display); 4] = [
                ("time", &datetime(&failed["createdDateTime"])),
                ("application", &s(failed, "appDisplayName")),
                ("code", &c),
                ("reason", &s(&failed["status"], "failureReason").trim_end_matches('.').to_string()),
            ];
            let mut text = tf("diagnose.last_failed", &args);
            if let Some(a) = advice(c) {
                text.push(' ');
                text.push_str(&a);
            }
            out.push(text);
        }
        let bad_passwords = sign_ins.iter().filter(|v| code(v) == 50126).count();
        if bad_passwords >= 5 {
            out.push(tf("diagnose.bad_passwords", &[("count", &bad_passwords)]));
        }
    }
    if sessions_revoked(f) {
        out.push(tf("diagnose.revoked", &[("time", &datetime(&Value::String(f.sessions_valid_from.clone())))]));
    }
    if out.is_empty() {
        out.push(t("diagnose.no_problems"));
    }
    out
}

/// The result of a scoped thread, passing on its panic.
pub fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))
}

/// What the overview reads, each part on its own so one that fails doesn't hide the others.
fn load_overview(g: &Graph, user_id: &str, missing: &mut Vec<String>) -> Result<TabBody> {
    // Each scope is checked once; what can't be read without it says so.
    let mut granted = |scope: &str| match require(g, scope) {
        Ok(()) => Ok(()),
        Err(e) => {
            if let Some(NeedsScopes(m)) = e.downcast_ref::<NeedsScopes>() {
                missing.extend(m.iter().cloned());
            }
            Err(e)
        }
    };
    let methods_ok = granted(AUTH_METHODS_SCOPE);
    let audit_ok = granted(AUDIT_LOG_SCOPE);
    let same = |r: &Result<()>| match r {
        Ok(()) => Ok(()),
        Err(e) => Err(match e.downcast_ref::<NeedsScopes>() {
            Some(NeedsScopes(m)) => anyhow::Error::new(NeedsScopes(m.clone())),
            None => anyhow!("{e}"),
        }),
    };
    let reports = |path: String| -> Result<Value> {
        same(&audit_ok)?;
        g.get(&path).map_err(|e| explain(e, Need::Reports))
    };
    // The reads don't depend on each other, so they run at the same time.
    let (user, methods, sign_ins, activity, registration) = std::thread::scope(|t| {
        let user = t.spawn(|| {
            g.get(&format!(
                "/users/{user_id}?$select=id,accountEnabled,userType,onPremisesSyncEnabled,createdDateTime,lastPasswordChangeDateTime,signInSessionsValidFromDateTime,passwordPolicies"
            ))
        });
        let methods = t.spawn(|| {
            same(&methods_ok).and_then(|()| g.list(&format!("/users/{user_id}/authentication/methods")).map_err(|e| explain(e, Need::Methods)))
        });
        let sign_ins = t.spawn(|| {
            reports(format!("/auditLogs/signIns?$filter=userId eq '{user_id}'&$top=25"))
                .map(|v| v["value"].as_array().cloned().unwrap_or_default())
        });
        let activity = t.spawn(|| reports(format!("/users/{user_id}?$select=signInActivity")));
        let registration = reports(format!("/reports/authenticationMethods/userRegistrationDetails/{user_id}")).map(|v| {
            // The documentation's example wraps the object in "value".
            if v["value"].is_object() { v["value"].clone() } else { v }
        });
        (join(user), join(methods), join(sign_ins), join(activity), registration)
    });
    let user = user?;

    let mut fields: Vec<(String, String)> = Vec::new();
    // An empty label continues the field above.
    let mut field = |label: &str, value: String| fields.push((if label.is_empty() { String::new() } else { t(label) }, value));
    field("overview.account", match user["accountEnabled"].as_bool() {
        Some(true) => t("overview.enabled"),
        Some(false) => t("overview.disabled"),
        None => String::new(),
    });
    field("prop.user_type", s(&user, "userType"));
    field("prop.synced", yes_no(&user["onPremisesSyncEnabled"]));
    match &activity {
        Ok(a) => {
            let a = &a["signInActivity"];
            field("overview.last_interactive", datetime(&a["lastSignInDateTime"]));
            field("overview.last_non_interactive", datetime(&a["lastNonInteractiveSignInDateTime"]));
            field("overview.last_successful", datetime(&a["lastSuccessfulSignInDateTime"]));
        }
        Err(e) => field("prop.last_sign_in", unavailable(e)),
    }
    field("prop.password_changed", datetime(&user["lastPasswordChangeDateTime"]));
    if s(&user, "passwordPolicies").contains("DisablePasswordExpiration") {
        field("overview.password_expiry", t("overview.never_expires"));
    }
    field("overview.sessions_valid_from", datetime(&user["signInSessionsValidFromDateTime"]));
    match &registration {
        Ok(r) => {
            field("overview.mfa_registered", yes_no(&r["isMfaRegistered"]));
            field("overview.mfa_capable", yes_no(&r["isMfaCapable"]));
            field("overview.default_mfa", s(r, "defaultMfaMethod"));
            field("overview.passwordless", yes_no(&r["isPasswordlessCapable"]));
            field("overview.sspr_registered", yes_no(&r["isSsprRegistered"]));
            field("overview.registration_updated", datetime(&r["lastUpdatedDateTime"]));
        }
        Err(e) => field("overview.mfa_registered", unavailable(e)),
    }
    match &methods {
        Ok(m) => {
            let mut names: Vec<String> = m.iter().map(|v| describe_method(v).0).collect();
            names.sort();
            names.dedup();
            field("tab.auth_methods", names.join(", "));
        }
        Err(e) => field("tab.auth_methods", unavailable(e)),
    }
    if let Err(e) = &sign_ins {
        field("overview.recent_sign_ins", unavailable(e));
    }
    let facts = Facts {
        enabled: user["accountEnabled"].as_bool(),
        synced: user["onPremisesSyncEnabled"] == true,
        guest: s(&user, "userType") == "Guest",
        methods: methods.ok(),
        sign_ins: sign_ins.ok(),
        sessions_valid_from: s(&user, "signInSessionsValidFromDateTime"),
        created: s(&user, "createdDateTime"),
        password_changed: s(&user, "lastPasswordChangeDateTime"),
    };
    for (i, finding) in diagnose(&facts).into_iter().enumerate() {
        field(if i == 0 { "overview.troubleshooting" } else { "" }, finding);
    }
    missing.sort();
    missing.dedup();
    Ok(TabBody::Fields(fields))
}

/// Fetches one of the security tabs of a user (runs on a worker thread). Also returns the scopes
/// that a part of it couldn't be read without.
pub fn load_tab(g: &Graph, kind: TabKind, user: &Value) -> Result<(TabBody, Vec<String>)> {
    let id = s(user, "id");
    let mut missing = Vec::new();
    let body = match kind {
        TabKind::Security => load_overview(g, &id, &mut missing)?,
        TabKind::AuthMethods => methods_table(load_methods(g, &id)?),
        TabKind::SignIns => sign_ins_table(recent_sign_ins(g, &id, 50)?),
        _ => TabBody::Fields(Vec::new()),
    };
    Ok((body, missing))
}

/// The keys of the labels of the buttons a security tab has beyond add, remove and edit, in order.
pub fn extra_ops(kind: TabKind) -> &'static [&'static str] {
    match kind {
        TabKind::Security => &["act.revoke_sessions", "act.temporary_access_pass", "security.reregister"],
        TabKind::AuthMethods => &["security.reregister"],
        _ => &[],
    }
}

/// The keys of the add, remove and edit labels of the security tabs.
pub fn op_labels(kind: TabKind) -> [Option<&'static str>; 3] {
    match kind {
        TabKind::AuthMethods => [Some("act.temporary_access_pass"), Some("security.delete_method"), None],
        TabKind::SignIns => [None, None, Some("security.details")],
        _ => [None, None, None],
    }
}

impl App {
    /// Add, remove, edit and the extra buttons on a user's security tabs.
    pub fn security_sheet_op(&mut self, sheet_id: u64, kind: TabKind, op: TabOp, user: Value, row: Option<Value>) {
        let reload = move |app: &mut App| {
            app.reload_tab(sheet_id, TabKind::Security);
            app.reload_tab(sheet_id, TabKind::AuthMethods);
        };
        let extra = |i: usize| extra_ops(kind).get(i).copied();
        match (kind, op) {
            (TabKind::AuthMethods, TabOp::Add) => self.temporary_access_pass(user, reload),
            (TabKind::AuthMethods, TabOp::Remove) => {
                if let Some(row) = row {
                    self.delete_method(user, row, reload);
                }
            }
            (TabKind::SignIns, TabOp::Edit) => {
                if let Some(row) = row {
                    self.push(Dialog::Message { title: t("signins.details_title"), text: sign_in_details(&row), copy: None });
                }
            }
            (_, TabOp::Extra(i)) => match extra(i) {
                Some("act.revoke_sessions") => self.revoke_sessions(user, reload),
                Some("act.temporary_access_pass") => self.temporary_access_pass(user, reload),
                Some("security.reregister") => self.require_reregistration(user, reload),
                _ => {}
            },
            _ => {}
        }
    }

    pub fn revoke_sessions(&mut self, user: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = s(&user, "id");
        let name = s(&user, "displayName");
        let mut text = tf("revoke.question", &[("name", &name)]);
        if s(&user, "userType") == "Guest" {
            text.push_str("\n\n");
            text.push_str(&t("revoke.guest"));
        }
        self.push(Dialog::confirm(&t("revoke.title"), &text, move |app| {
            let path = format!("/users/{id}/revokeSignInSessions");
            app.spawn(
                t("revoke.working"),
                move |g| g.post(&path, &json!({})).map_err(|e| explain(e, Need::Users)),
                move |app, r| match r {
                    Ok(_) => {
                        log::info!("Sessions revoked for {name}");
                        app.status = tf("revoke.done", &[("name", &name)]);
                        after(app);
                    }
                    Err(e) => app.fail(&t("revoke.title"), e),
                },
            );
        }));
    }

    pub fn temporary_access_pass(&mut self, user: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = s(&user, "id");
        let name = s(&user, "displayName");
        let after = std::cell::RefCell::new(Some(after));
        let form = Form::new(&tf("tap.title", &[("name", &name)]), &t("dialog.create"))
            .intro(&t("tap.intro"))
            .field(Field::text("lifetime", &t("tap.lifetime"), "60").required())
            .field(Field::check("once", &t("tap.once"), true))
            .field(Field::text("start", &t("tap.start"), ""))
            .field(Field::info("", &t("tap.start_note")))
            .on_submit(move |app, f| {
                let lifetime: i64 = f.text("lifetime").parse().map_err(|_| t("tap.bad_lifetime"))?;
                if !(10..=43_200).contains(&lifetime) {
                    return Err(t("tap.lifetime_range"));
                }
                let mut body = json!({ "lifetimeInMinutes": lifetime, "isUsableOnce": f.checked("once") });
                let start = f.text("start");
                if !start.is_empty() {
                    let at = crate::exchange::parse_time(&start).ok_or_else(|| t("tap.bad_start"))?;
                    body["startDateTime"] = json!(at);
                }
                let path = format!("/users/{id}/authentication/temporaryAccessPassMethods");
                let name = name.clone();
                let after = after.borrow_mut().take();
                app.spawn(
                    t("tap.creating"),
                    move |g| {
                        require(g, AUTH_METHODS_SCOPE)?;
                        g.post(&path, &body).map_err(|e| explain(e, Need::Methods))
                    },
                    move |app, r| match r {
                        Ok(tap) => {
                            log::info!("Created a Temporary Access Pass for {name}");
                            app.status = tf("tap.done", &[("name", &name)]);
                            let minutes = tap["lifetimeInMinutes"].as_i64().unwrap_or(0);
                            let once = tap["isUsableOnce"] == true;
                            let mut text = match s(&tap, "startDateTime") {
                                st if st.is_empty() => {
                                    tf(if once { "tap.valid_now_once" } else { "tap.valid_now" }, &[("name", &name), ("minutes", &minutes)])
                                }
                                st => {
                                    let args: [(&str, &dyn std::fmt::Display); 3] =
                                        [("name", &name), ("minutes", &minutes), ("start", &datetime(&Value::String(st)))];
                                    tf(if once { "tap.valid_from_once" } else { "tap.valid_from" }, &args)
                                }
                            };
                            match s(&tap, "methodUsabilityReason").as_str() {
                                "" | "EnabledByPolicy" => {}
                                reason if tap["isUsable"] != true => {
                                    text.push_str("\n\n");
                                    text.push_str(&tf("tap.not_usable", &[("reason", &reason)]));
                                }
                                _ => {}
                            }
                            app.push(Dialog::Message { title: t("method.tap"), text, copy: Some(s(&tap, "temporaryAccessPass")) });
                            if let Some(after) = after {
                                after(app);
                            }
                        }
                        Err(e) => app.fail(&t("method.tap"), e),
                    },
                );
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    fn delete_method(&mut self, user: Value, method: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = s(&user, "id");
        let name = s(&user, "displayName");
        let (kind, details) = describe_method(&method);
        let Some(path) = delete_path(&id, &method) else {
            let text = if method_type(&method) == "passwordAuthenticationMethod" {
                t("method.password_not_deletable")
            } else {
                tf("method.not_deletable", &[("method", &kind)])
            };
            self.push(Dialog::message(&t("security.delete_method"), &text));
            return;
        };
        let what = if details.is_empty() { kind.clone() } else { format!("{kind} ({details})") };
        let text = format!("{}\n\n{}", tf("method.delete_question", &[("method", &what), ("name", &name)]), delete_effect(&method));
        self.push(Dialog::confirm(&t("method.delete_title"), &text, move |app| {
            app.spawn(
                t("method.deleting"),
                move |g| {
                    require(g, AUTH_METHODS_SCOPE)?;
                    g.delete(&path).map_err(|e| explain(e, Need::Methods))
                },
                move |app, r| match r {
                    Ok(()) => {
                        log::info!("Deleted an authentication method from {name}");
                        app.status = tf("method.deleted", &[("method", &kind), ("name", &name)]);
                        after(app);
                    }
                    Err(e) => app.fail(&t("method.delete_title"), e),
                },
            );
        }));
    }

    /// Requires the user to register for MFA again by deleting their MFA methods, which is how
    /// Microsoft documents doing it with Graph.
    pub fn require_reregistration(&mut self, user: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = s(&user, "id");
        let name = s(&user, "displayName");
        let get = id.clone();
        self.spawn(tf("status.loading_tab", &[("tab", &t("tab.auth_methods"))]), move |g| load_methods(g, &get), move |app, r| match r {
            Ok(methods) => app.reregistration_form(id, name, methods, after),
            Err(e) => app.fail(&t("reregister.title"), e),
        });
    }

    fn reregistration_form(&mut self, id: String, name: String, methods: Vec<Value>, after: impl FnOnce(&mut App) + Send + 'static) {
        let mut targets: Vec<Value> = methods.into_iter().filter(is_mfa_method).collect();
        if targets.is_empty() {
            self.push(Dialog::message(&t("reregister.title"), &tf("reregister.none", &[("name", &name)])));
            return;
        }
        targets.sort_by_key(deletion_order);
        let list: Vec<String> = targets
            .iter()
            .map(|m| {
                let (kind, details) = describe_method(m);
                if details.is_empty() { format!("  • {kind}") } else { format!("  • {kind}: {details}") }
            })
            .collect();
        let intro = tf("reregister.intro", &[("name", &name), ("count", &targets.len()), ("methods", &list.join("\n"))]);
        let after = std::cell::RefCell::new(Some(after));
        let form = Form::new(&tf("reregister.form_title", &[("name", &name)]), &t("reregister.delete"))
            .intro(&intro)
            .field(Field::check("revoke", &t("reregister.revoke"), false))
            .on_submit(move |app, f| {
                let revoke = f.checked("revoke");
                let jobs: Vec<(String, Option<String>)> =
                    targets.iter().map(|m| (describe_method(m).0, delete_path(&id, m))).collect();
                let revoke_path = format!("/users/{id}/revokeSignInSessions");
                let name = name.clone();
                let after = after.borrow_mut().take();
                app.spawn(
                    t("reregister.working"),
                    move |g| {
                        require(g, AUTH_METHODS_SCOPE)?;
                        let mut results: Vec<(String, Result<(), String>)> = jobs
                            .into_iter()
                            .map(|(label, path)| {
                                let r = match path {
                                    Some(p) => g.delete(&p).map_err(|e| format!("{:#}", explain(e, Need::Methods))),
                                    None => Err(t("reregister.not_deletable")),
                                };
                                (tf("task.delete", &[("name", &label)]), r)
                            })
                            .collect();
                        if revoke {
                            let r = g.post(&revoke_path, &json!({})).map(|_| ()).map_err(|e| format!("{:#}", explain(e, Need::Users)));
                            results.push((t("reregister.revoke_step"), r));
                        }
                        Ok(results)
                    },
                    move |app, r| match r {
                        Ok(results) => {
                            let failed = results.iter().filter(|(_, r)| r.is_err()).count();
                            let lines: Vec<String> = results
                                .iter()
                                .map(|(label, r)| match r {
                                    Ok(()) => format!("✔ {label}"),
                                    Err(e) => format!("✖ {label}: {e}"),
                                })
                                .collect();
                            log::info!("MFA re-registration for {name}: {failed} of {} steps failed", results.len());
                            let (title, head) = if failed == 0 {
                                (t("reregister.done_title"), tf("reregister.done", &[("count", &results.len()), ("name", &name)]))
                            } else {
                                let args: [(&str, &dyn std::fmt::Display); 3] = [("failed", &failed), ("count", &results.len()), ("name", &name)];
                                (t("reregister.incomplete_title"), tf("reregister.incomplete", &args))
                            };
                            app.status = title.clone();
                            app.push(Dialog::message(&title, &format!("{head}\n\n{}", lines.join("\n"))));
                            if let Some(after) = after {
                                after(app);
                            }
                        }
                        Err(e) => app.fail(&t("reregister.title"), e),
                    },
                );
                Ok(())
            });
        self.push(Dialog::Form(form));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method(kind: &str, extra: Value) -> Value {
        let mut v = json!({ "@odata.type": format!("#microsoft.graph.{kind}"), "id": "m1" });
        if let (Some(dst), Some(src)) = (v.as_object_mut(), extra.as_object()) {
            dst.extend(src.clone());
        }
        v
    }

    #[test]
    fn masks_phone_numbers_and_addresses() {
        assert_eq!(mask_phone("+44 7700900123"), "+44 •••123");
        assert_eq!(mask_phone("+1 2065555555x1234"), "+1 •••555");
        assert_eq!(mask_phone("12"), "•••12");
        assert_eq!(mask_email("ann.lee@contoso.com"), "a•••@contoso.com");
        let (kind, details) = describe_method(&method("phoneAuthenticationMethod", json!({ "phoneNumber": "+44 7700900123", "phoneType": "mobile" })));
        assert_eq!(kind, "Mobile phone");
        assert!(!details.contains("7700900"), "{details}");
    }

    #[test]
    fn deletes_through_the_right_collection() {
        let fido = method("fido2AuthenticationMethod", json!({}));
        assert_eq!(delete_path("u1", &fido).as_deref(), Some("/users/u1/authentication/fido2Methods/m1"));
        assert_eq!(delete_path("u1", &method("passwordAuthenticationMethod", json!({}))), None);
    }

    #[test]
    fn reregistration_keeps_device_credentials_and_recovery() {
        let kept = ["passwordAuthenticationMethod", "windowsHelloForBusinessAuthenticationMethod", "emailAuthenticationMethod", "temporaryAccessPassAuthenticationMethod", "platformCredentialAuthenticationMethod"];
        let deleted = ["microsoftAuthenticatorAuthenticationMethod", "phoneAuthenticationMethod", "softwareOathAuthenticationMethod", "fido2AuthenticationMethod"];
        assert!(kept.iter().all(|k| !is_mfa_method(&method(k, json!({})))));
        assert!(deleted.iter().all(|k| is_mfa_method(&method(k, json!({})))));

        let phone = |kind: &str| method("phoneAuthenticationMethod", json!({ "phoneType": kind }));
        let mut targets = [phone("mobile"), phone("alternateMobile"), method("fido2AuthenticationMethod", json!({})), phone("office")];
        targets.sort_by_key(deletion_order);
        let order: Vec<String> = targets.iter().map(|m| s(m, "phoneType")).collect();
        assert_eq!(order, ["", "alternateMobile", "office", "mobile"]);
    }

    fn sign_in(code: i64, reason: &str) -> Value {
        json!({ "createdDateTime": "2026-10-19T08:00:00Z", "appDisplayName": "Outlook", "status": { "errorCode": code, "failureReason": reason } })
    }

    #[test]
    fn classifies_sign_ins() {
        assert_eq!(sign_in_result(&sign_in(0, "")), "Success");
        assert_eq!(sign_in_result(&sign_in(50140, "")), "Interrupted (50140)");
        assert_eq!(sign_in_result(&sign_in(50126, "")), "Failed (50126)");
        let details = sign_in_details(&sign_in(53003, "Blocked by Conditional Access"));
        assert!(details.contains("Policy.Read.ConditionalAccess"), "policies missing from the response must be explained");
        let mut v = sign_in(53003, "Blocked");
        v["appliedConditionalAccessPolicies"] = json!([
            { "displayName": "Block legacy", "result": "failure", "enforcedGrantControls": ["Block"] },
            { "displayName": "Off", "result": "notEnabled" },
        ]);
        let details = sign_in_details(&v);
        assert!(details.contains("Block legacy: failure (requires Block)") && !details.contains("Off:"));
    }

    #[test]
    fn diagnoses_common_problems() {
        let f = Facts {
            enabled: Some(false),
            methods: Some(vec![method("passwordAuthenticationMethod", json!({}))]),
            sign_ins: Some(vec![sign_in(50140, "Interrupted"), sign_in(50057, "User account is disabled.")]),
            ..Default::default()
        };
        let found = diagnose(&f);
        assert!(found[0].starts_with("The account is disabled"));
        assert!(found.iter().any(|x| x.starts_with("No MFA method is registered")));
        // The interrupt is passed over for the real failure.
        assert!(found.iter().any(|x| x.contains("error 50057, User account is disabled.") && x.contains("Enable it")));

        let healthy = Facts {
            enabled: Some(true),
            methods: Some(vec![method("microsoftAuthenticatorAuthenticationMethod", json!({}))]),
            sign_ins: Some(vec![sign_in(0, "")]),
            ..Default::default()
        };
        assert_eq!(diagnose(&healthy), ["The latest sign-in, at 2026-10-19 08:00, succeeded."]);
        assert_eq!(diagnose(&Facts::default()), ["No problems found."]);

        // Graph sets the time sessions are valid from on creating the account and changing its password too.
        let created = Facts {
            sessions_valid_from: "2026-03-01T09:00:05Z".into(),
            created: "2026-03-01T09:00:04Z".into(),
            password_changed: "2026-03-01T09:00:04Z".into(),
            ..Default::default()
        };
        assert_eq!(diagnose(&created), ["No problems found."]);
        let revoked = Facts { sessions_valid_from: "2026-10-18T17:30:00Z".into(), ..created };
        assert!(diagnose(&revoked)[0].starts_with("Sessions from before 2026-10-18 17:30 UTC"));
        assert_eq!(seconds("1970-01-02T00:00:01Z"), Some(86401));
        assert_eq!(seconds("2000-03-01T00:00:00Z"), Some(951868800));

        let guessing = Facts { sign_ins: Some(vec![sign_in(50126, "Invalid password"); 6]), ..Default::default() };
        assert!(diagnose(&guessing).iter().any(|x| x.starts_with("6 of the recent sign-ins failed")));
    }

    #[test]
    fn explains_licence_and_role_refusals() {
        let premium = explain(anyhow!("Authentication_RequestFromNonPremiumTenantOrB2CTenant (403): Neither tenant is B2C or tenant doesn't have premium license"), Need::Reports);
        assert!(premium.to_string().starts_with("Sign-in logs, sign-in activity and MFA registration details need a Microsoft Entra ID P1 or P2"));
        let role = explain(anyhow!("Authorization_RequestDenied (403): Insufficient privileges"), Need::Methods);
        assert!(role.to_string().contains("Authentication Administrator"));
        let other = explain(anyhow!("Request_ResourceNotFound (404): gone"), Need::Methods);
        assert_eq!(other.to_string(), "Request_ResourceNotFound (404): gone");
    }
}
