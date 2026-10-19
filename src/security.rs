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
        return anyhow!(
            "Sign-in logs, sign-in activity and MFA registration details need a Microsoft Entra ID P1 or P2 \
             licence in the tenant.\n\nMicrosoft Graph said: {text}"
        );
    }
    if text.contains("(403)") {
        let roles = match need {
            Need::Reports => {
                "Reading sign-in logs and registration details needs one of these roles: Reports Reader, \
                 Security Reader, Security Operator, Security Administrator or Global Reader."
            }
            Need::Methods => {
                "Managing another user's authentication methods needs the Authentication Administrator role, \
                 or Privileged Authentication Administrator when the user is an administrator. When you \
                 change your own methods, Microsoft also wants an MFA sign-in in the last 10 minutes: sign \
                 out and in again, then retry."
            }
            Need::Users => {
                "This needs a role that can manage the user, such as User Administrator, Helpdesk \
                 Administrator (for non-administrators) or Privileged Authentication Administrator."
            }
        };
        return anyhow!("Your account isn't allowed to do this. {roles}\n\nMicrosoft Graph said: {text}");
    }
    e
}

/// A short reason for a part of the overview that couldn't be read.
fn unavailable(e: &anyhow::Error) -> String {
    if let Some(NeedsScopes(missing)) = e.downcast_ref::<NeedsScopes>() {
        return format!("Not available: needs the {} permission", missing.join(", "));
    }
    let text = e.to_string();
    if text.contains("RequestFromNonPremiumTenant") || text.contains("premium license") {
        "Not available: needs Microsoft Entra ID P1 or P2".into()
    } else if text.contains("(403)") {
        "Not available: your role can't read this (for example Reports Reader or Global Reader can)".into()
    } else {
        format!("Not available: {text}")
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
    let opt = |label: &str, key: &str| {
        let value = s(v, key);
        if value.is_empty() { None } else { Some(format!("{label}{value}")) }
    };
    let join = |parts: Vec<Option<String>>| parts.into_iter().flatten().collect::<Vec<_>>().join(" · ");
    match method_type(v) {
        "passwordAuthenticationMethod" => ("Password".into(), "Can't be shown or removed; reset it instead".into()),
        "microsoftAuthenticatorAuthenticationMethod" => (
            "Microsoft Authenticator".into(),
            join(vec![opt("", "displayName"), opt("", "deviceTag"), opt("App version ", "phoneAppVersion")]),
        ),
        "phoneAuthenticationMethod" => {
            let kind = match s(v, "phoneType").as_str() {
                "mobile" => "Mobile phone",
                "alternateMobile" => "Alternate mobile phone",
                "office" => "Office phone",
                _ => "Phone",
            };
            let sms = if s(v, "smsSignInState") == "ready" { " · SMS sign-in enabled" } else { "" };
            (kind.into(), format!("Ending {}{sms}", mask_phone(&s(v, "phoneNumber"))))
        }
        "fido2AuthenticationMethod" => {
            ("Passkey (FIDO2)".into(), join(vec![opt("", "displayName"), opt("Model ", "model")]))
        }
        "windowsHelloForBusinessAuthenticationMethod" => (
            "Windows Hello for Business".into(),
            join(vec![opt("", "displayName"), opt("Key strength ", "keyStrength")]),
        ),
        "emailAuthenticationMethod" => ("Email (password reset)".into(), mask_email(&s(v, "emailAddress"))),
        "softwareOathAuthenticationMethod" => ("Authenticator app code (OATH)".into(), String::new()),
        "temporaryAccessPassAuthenticationMethod" => {
            let mut parts = vec![format!("{} minutes", v["lifetimeInMinutes"].as_i64().unwrap_or(0))];
            if v["isUsableOnce"] == true {
                parts.push("one use".into());
            }
            parts.push(if v["isUsable"] == true { "usable now".into() } else { format!("not usable: {}", s(v, "methodUsabilityReason")) });
            ("Temporary Access Pass".into(), parts.join(" · "))
        }
        "platformCredentialAuthenticationMethod" => {
            ("Platform credential".into(), join(vec![opt("", "displayName"), opt("", "platform")]))
        }
        "externalAuthenticationMethod" => ("External MFA".into(), opt("", "displayName").unwrap_or_default()),
        other => (other.trim_end_matches("AuthenticationMethod").to_string(), String::new()),
    }
}

/// What deleting a method does, for its confirmation.
fn delete_effect(v: &Value) -> &'static str {
    match method_type(v) {
        "phoneAuthenticationMethod" => {
            "They can no longer use this number for MFA or password reset. A number that is their default \
             MFA method can't be deleted until they choose another default; a mobile number can't be \
             deleted while an alternate mobile number remains."
        }
        "microsoftAuthenticatorAuthenticationMethod" => {
            "Notifications and codes from the app on that device stop working. They can register the app again."
        }
        "fido2AuthenticationMethod" => "They can no longer sign in with this security key or passkey.",
        "windowsHelloForBusinessAuthenticationMethod" => {
            "Windows Hello sign-in on that device stops working until it is set up again."
        }
        "emailAuthenticationMethod" => "They can no longer use this address to reset their password.",
        "temporaryAccessPassAuthenticationMethod" => "The pass can no longer be used to sign in.",
        _ => "They can no longer sign in with it.",
    }
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
    TabBody::Table { headers: vec!["Method", "Details", "Added (UTC)"], rows }
}

// ----- sign-ins ----------------------------------------------------------------

/// Error codes that interrupt a sign-in to ask for something, which is usually completed straight
/// after, rather than failing it.
const INTERRUPTS: [i64; 8] = [50058, 50072, 50074, 50076, 50079, 50125, 50140, 50199];

/// What a sign-in error code usually means, and what to do about it.
pub fn advice(code: i64) -> Option<&'static str> {
    Some(match code {
        50126 => "Wrong user name or password. If the user is sure of their password, reset it.",
        50053 => {
            "The account is locked after too many failed attempts (smart lockout), or sign-ins from that IP \
             address are blocked. It unlocks by itself after the lockout period. Many attempts the user \
             didn't make can mean someone is guessing the password."
        }
        50057 => "The account is disabled. Enable it if the user should be able to sign in.",
        50055 => "The password has expired. Reset it, or have the user change it.",
        50144 => "The user's Active Directory password has expired. Change it on-premises.",
        50056 => {
            "No password is stored for this user in Microsoft Entra ID. Set one, or check password hash \
             synchronisation for a synced user."
        }
        50034 => "No account with that user name is in this directory: check the name the user types.",
        50074 | 50076 => "MFA was required for this sign-in. This is normal, and is usually followed by a successful sign-in.",
        500121 => {
            "MFA failed: the user denied or didn't answer the prompt, or entered a wrong code. Prompts the \
             user didn't expect can mean someone else knows their password."
        }
        50072 | 50079 => {
            "The user has to register for MFA before they can continue. A Temporary Access Pass lets them \
             sign in to register."
        }
        53003 => {
            "Blocked by Conditional Access. The sign-in's details list the policies that applied, if this \
             app can read Conditional Access data."
        }
        53000 => "Conditional Access requires a compliant device, and this device isn't marked compliant.",
        53001 => "Conditional Access requires a Microsoft Entra hybrid joined device.",
        50097 => "Device authentication is required for this sign-in.",
        50105 => "The user isn't assigned to this application, which requires assignment. Assign them under Applications.",
        50133 | 50173 => {
            "The session is no longer valid because the password changed or sessions were revoked. Signing in again fixes it."
        }
        70044 => {
            "The session expired, or a Conditional Access sign-in frequency setting asked for a new sign-in. \
             Signing in again fixes it."
        }
        50140 => "The \"Stay signed in?\" question interrupted the sign-in. This is normal.",
        50158 => "An external security challenge, such as a third-party MFA provider, wasn't completed.",
        50199 => "The user was asked to confirm the sign-in for security reasons. This is normal.",
        50058 => "Silent sign-in found no session to use. The app then usually asks the user to sign in, which is normal.",
        65001 => "The user, or an administrator, hasn't consented to the permissions the application asks for.",
        90094 => "The application needs an administrator to grant consent.",
        50020 => {
            "The account's identity provider isn't known to this tenant: often a guest who wasn't invited or \
             has been removed."
        }
        _ => return None,
    })
}

/// "Success", "Interrupted (50140)" or "Failed (50126)".
pub fn sign_in_result(v: &Value) -> String {
    let code = v["status"]["errorCode"].as_i64().unwrap_or(0);
    if code == 0 {
        "Success".into()
    } else if INTERRUPTS.contains(&code) {
        format!("Interrupted ({code})")
    } else {
        format!("Failed ({code})")
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

fn conditional_access(status: &str) -> &str {
    match status {
        "success" => "Passed",
        "failure" => "Failed",
        "notApplied" => "Not applied",
        "" => "",
        other => other,
    }
}

/// The full details of a sign-in, for its Details dialog.
pub fn sign_in_details(v: &Value) -> String {
    let code = v["status"]["errorCode"].as_i64().unwrap_or(0);
    let mut lines = vec![
        format!("Date (UTC): {}", datetime(&v["createdDateTime"])),
        format!("User: {}", s(v, "userPrincipalName")),
        format!("Application: {}", s(v, "appDisplayName")),
        format!("Resource: {}", s(v, "resourceDisplayName")),
        format!("Result: {}", sign_in_result(v)),
    ];
    for (label, key) in [("Reason", "failureReason"), ("Additional details", "additionalDetails")] {
        let text = s(&v["status"], key);
        if !text.is_empty() && code != 0 {
            lines.push(format!("{label}: {text}"));
        }
    }
    if let Some(a) = advice(code) {
        lines.push(format!("What it means: {a}"));
    }
    let d = &v["deviceDetail"];
    lines.extend([
        format!("Interactive: {}", yes_no(&v["isInteractive"])),
        format!("Client app: {}", s(v, "clientAppUsed")),
        format!("IP address: {}", s(v, "ipAddress")),
        format!("Location: {}", location(v)),
        format!(
            "Device: {}",
            [s(d, "displayName"), s(d, "operatingSystem"), s(d, "browser")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", ")
        ),
        format!("Device compliant: {} · managed: {}", yes_no(&d["isCompliant"]), yes_no(&d["isManaged"])),
        format!("Conditional Access: {}", conditional_access(&s(v, "conditionalAccessStatus"))),
    ]);
    match v["appliedConditionalAccessPolicies"].as_array() {
        Some(policies) => {
            // Policies that weren't enabled or didn't apply say nothing about this sign-in.
            let relevant: Vec<String> = policies
                .iter()
                .filter(|p| !matches!(s(p, "result").as_str(), "notEnabled" | "notApplied"))
                .map(|p| {
                    let grants = p["enforcedGrantControls"].as_array().map(|g| g.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    let grants = if grants.is_empty() { String::new() } else { format!(" (requires {grants})") };
                    format!("  • {}: {}{grants}", s(p, "displayName"), s(p, "result"))
                })
                .collect();
            if relevant.is_empty() {
                lines.push("Policies: none applied".into());
            } else {
                lines.push("Policies:".into());
                lines.extend(relevant);
            }
        }
        None => lines.push(
            "Policies: not shown. Microsoft Graph only lists them when this app has Policy.Read.ConditionalAccess \
             and your role can read Conditional Access (Global Reader, Security Reader, Security Administrator or \
             Conditional Access Administrator)."
                .into(),
        ),
    }
    let risk = s(v, "riskLevelDuringSignIn");
    if !risk.is_empty() && risk != "hidden" && risk != "none" {
        lines.push(format!("Risk during sign-in: {risk} ({})", s(v, "riskState")));
    }
    lines.push(format!("Correlation ID: {}", s(v, "correlationId")));
    lines.push(format!("Request ID: {}", s(v, "id")));
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
        vec!["Date (UTC)", "Application", "Result", "IP address", "Location", "Conditional Access"],
        items,
        |v| {
            vec![
                datetime(&v["createdDateTime"]),
                s(v, "appDisplayName"),
                sign_in_result(v),
                s(v, "ipAddress"),
                location(v),
                conditional_access(&s(v, "conditionalAccessStatus")).to_string(),
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
        out.push(if f.synced {
            "The account is disabled, so the user can't sign in. It is synced from Active Directory: enable it there.".to_string()
        } else {
            "The account is disabled, so the user can't sign in.".to_string()
        });
    }
    if f.synced {
        out.push(
            "The account is synced from on-premises Active Directory, so its password and whether it is \
             enabled are set there (unless password writeback is set up)."
                .into(),
        );
    }
    if f.guest {
        out.push(
            "This is a guest: they sign in with their home organisation's account, whose password and MFA \
             methods are managed there. Revoking sessions here doesn't sign them out of their home tenant."
                .into(),
        );
    }
    if let Some(methods) = &f.methods {
        let tap = methods.iter().find(|m| method_type(m) == "temporaryAccessPassAuthenticationMethod");
        if !methods.iter().any(is_mfa_method) && !f.guest {
            out.push(
                "No MFA method is registered. Where MFA is required, the user is asked to register at their next \
                 sign-in, or blocked if your policies don't allow registering from where they are. A Temporary \
                 Access Pass lets them sign in to register."
                    .into(),
            );
        }
        if let Some(t) = tap {
            let usable = if t["isUsable"] == true { "usable now" } else { "not usable yet or any more" };
            out.push(format!("The user has a Temporary Access Pass ({usable}), created {}.", datetime(&t["createdDateTime"])));
        }
    }
    if let Some(sign_ins) = &f.sign_ins {
        let code = |v: &Value| v["status"]["errorCode"].as_i64().unwrap_or(0);
        match sign_ins.first() {
            None => out.push(
                "There are no interactive sign-ins in the sign-in log for this user (it keeps 30 days).".into(),
            ),
            Some(latest) if code(latest) == 0 => {
                out.push(format!("The latest sign-in, at {}, succeeded.", datetime(&latest["createdDateTime"])));
            }
            Some(_) => {}
        }
        if let Some(failed) = sign_ins.iter().find(|v| code(v) != 0 && !INTERRUPTS.contains(&code(v))) {
            let c = code(failed);
            let mut text = format!(
                "The last failed sign-in was at {} to {}: error {c}, {}.",
                datetime(&failed["createdDateTime"]),
                s(failed, "appDisplayName"),
                s(&failed["status"], "failureReason").trim_end_matches('.')
            );
            if let Some(a) = advice(c) {
                text.push(' ');
                text.push_str(a);
            }
            out.push(text);
        }
        let bad_passwords = sign_ins.iter().filter(|v| code(v) == 50126).count();
        if bad_passwords >= 5 {
            out.push(format!(
                "{bad_passwords} of the recent sign-ins failed with a wrong password: the user may have forgotten \
                 it, or someone may be guessing it."
            ));
        }
    }
    if sessions_revoked(f) {
        out.push(format!(
            "Sessions from before {} UTC are no longer valid (sessions were revoked then).",
            datetime(&Value::String(f.sessions_valid_from.clone()))
        ));
    }
    if out.is_empty() {
        out.push("No problems found.".into());
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
    let mut field = |label: &str, value: String| fields.push((label.to_string(), value));
    field("Account", match user["accountEnabled"].as_bool() {
        Some(true) => "Enabled".into(),
        Some(false) => "Disabled: the user can't sign in".into(),
        None => String::new(),
    });
    field("User type", s(&user, "userType"));
    field("Synced from on-premises", yes_no(&user["onPremisesSyncEnabled"]));
    match &activity {
        Ok(a) => {
            let a = &a["signInActivity"];
            field("Last interactive sign-in", datetime(&a["lastSignInDateTime"]));
            field("Last non-interactive sign-in", datetime(&a["lastNonInteractiveSignInDateTime"]));
            field("Last successful sign-in", datetime(&a["lastSuccessfulSignInDateTime"]));
        }
        Err(e) => field("Last sign-in", unavailable(e)),
    }
    field("Password last changed", datetime(&user["lastPasswordChangeDateTime"]));
    if s(&user, "passwordPolicies").contains("DisablePasswordExpiration") {
        field("Password expiry", "Never expires".into());
    }
    field("Sessions valid from", datetime(&user["signInSessionsValidFromDateTime"]));
    match &registration {
        Ok(r) => {
            field("Registered for MFA", yes_no(&r["isMfaRegistered"]));
            field("Can use MFA", yes_no(&r["isMfaCapable"]));
            field("Default MFA method", s(r, "defaultMfaMethod"));
            field("Passwordless capable", yes_no(&r["isPasswordlessCapable"]));
            field("Registered for password reset", yes_no(&r["isSsprRegistered"]));
            field("Registration details updated", datetime(&r["lastUpdatedDateTime"]));
        }
        Err(e) => field("Registered for MFA", unavailable(e)),
    }
    match &methods {
        Ok(m) => {
            let mut names: Vec<String> = m.iter().map(|v| describe_method(v).0).collect();
            names.sort();
            names.dedup();
            field("Authentication methods", names.join(", "));
        }
        Err(e) => field("Authentication methods", unavailable(e)),
    }
    if let Err(e) = &sign_ins {
        field("Recent sign-ins", unavailable(e));
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
        field(if i == 0 { "Troubleshooting" } else { "" }, finding);
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

/// Buttons a security tab has beyond add, remove and edit, in order.
pub fn extra_ops(kind: TabKind) -> &'static [&'static str] {
    match kind {
        TabKind::Security => &["Revoke sessions…", "Temporary Access Pass…", "Require MFA re-registration…"],
        TabKind::AuthMethods => &["Require MFA re-registration…"],
        _ => &[],
    }
}

/// Add, remove and edit labels of the security tabs.
pub fn op_labels(kind: TabKind) -> [Option<&'static str>; 3] {
    match kind {
        TabKind::AuthMethods => [Some("Temporary Access Pass…"), Some("Delete method"), None],
        TabKind::SignIns => [None, None, Some("Details…")],
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
                    self.push(Dialog::Message { title: "Sign-in details".into(), text: sign_in_details(&row), copy: None });
                }
            }
            (_, TabOp::Extra(i)) => match extra(i) {
                Some("Revoke sessions…") => self.revoke_sessions(user, reload),
                Some("Temporary Access Pass…") => self.temporary_access_pass(user, reload),
                Some("Require MFA re-registration…") => self.require_reregistration(user, reload),
                _ => {}
            },
            _ => {}
        }
    }

    pub fn revoke_sessions(&mut self, user: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = s(&user, "id");
        let name = s(&user, "displayName");
        let guest = if s(&user, "userType") == "Guest" {
            "\n\nThis is a guest, who signs in through their home organisation, so this doesn't revoke their sessions there."
        } else {
            ""
        };
        let text = format!(
            "Revoke all sign-in sessions for {name}?\n\n\
             This invalidates the refresh tokens and browser session cookies issued to them, so their apps and \
             browsers have to sign in again the next time they need a new token. It can take a few minutes to \
             take effect.\n\n\
             It does not end access straight away: access tokens already issued stay valid until they expire, \
             usually within an hour (apps that support Continuous Access Evaluation may stop sooner). It also \
             doesn't stop them signing in again: disable the account or reset the password for that.{guest}"
        );
        self.push(Dialog::confirm("Revoke sessions", &text, move |app| {
            let path = format!("/users/{id}/revokeSignInSessions");
            app.spawn(
                "Revoking sessions…",
                move |g| g.post(&path, &json!({})).map_err(|e| explain(e, Need::Users)),
                move |app, r| match r {
                    Ok(_) => {
                        app.status = format!("Sessions revoked for {name}");
                        log::info!("{}", app.status);
                        after(app);
                    }
                    Err(e) => app.fail("Revoke sessions", e),
                },
            );
        }));
    }

    pub fn temporary_access_pass(&mut self, user: Value, after: impl FnOnce(&mut App) + Send + 'static) {
        let id = s(&user, "id");
        let name = s(&user, "displayName");
        let after = std::cell::RefCell::new(Some(after));
        let form = Form::new(&format!("Temporary Access Pass — {name}"), "Create")
            .intro(
                "A Temporary Access Pass is a time-limited passcode the user can sign in with, for example to set \
                 up Microsoft Authenticator or a passkey, or after losing their phone. Creating one replaces any pass \
                 they already have. It must be enabled in the tenant's authentication methods policy, which also \
                 limits its lifetime and whether it can be used more than once. The pass is shown only once.",
            )
            .field(Field::text("lifetime", "Lifetime in minutes", "60").required())
            .field(Field::check("once", "Can be used only once", true))
            .field(Field::text("start", "Start (UTC, optional)", ""))
            .field(Field::info("", "Leave Start blank for now, or enter a time such as 2026-12-24 09:00."))
            .on_submit(move |app, f| {
                let lifetime: i64 = f.text("lifetime").parse().map_err(|_| "Enter the lifetime as a number of minutes.".to_string())?;
                if !(10..=43_200).contains(&lifetime) {
                    return Err("The lifetime must be between 10 minutes and 43,200 minutes (30 days).".into());
                }
                let mut body = json!({ "lifetimeInMinutes": lifetime, "isUsableOnce": f.checked("once") });
                let start = f.text("start");
                if !start.is_empty() {
                    let at = crate::exchange::parse_time(&start).ok_or("Enter the start as a date and time, such as 2026-12-24 09:00.")?;
                    body["startDateTime"] = json!(at);
                }
                let path = format!("/users/{id}/authentication/temporaryAccessPassMethods");
                let name = name.clone();
                let after = after.borrow_mut().take();
                app.spawn(
                    "Creating Temporary Access Pass…",
                    move |g| {
                        require(g, AUTH_METHODS_SCOPE)?;
                        g.post(&path, &body).map_err(|e| explain(e, Need::Methods))
                    },
                    move |app, r| match r {
                        Ok(tap) => {
                            log::info!("Created a Temporary Access Pass for {name}");
                            app.status = format!("Temporary Access Pass created for {name}");
                            let when = match s(&tap, "startDateTime") {
                                st if st.is_empty() => "from now".to_string(),
                                st => format!("from {} UTC", datetime(&Value::String(st))),
                            };
                            let once = if tap["isUsableOnce"] == true { ", for one sign-in" } else { "" };
                            let usable = match s(&tap, "methodUsabilityReason").as_str() {
                                "" | "EnabledByPolicy" => String::new(),
                                reason if tap["isUsable"] != true => format!("\n\nIt can't be used yet: {reason}."),
                                _ => String::new(),
                            };
                            app.push(Dialog::Message {
                                title: "Temporary Access Pass".into(),
                                text: format!(
                                    "This pass for {name} is valid for {} minutes {when}{once}. It won't be shown again: \
                                     give it to the user in a secure way.{usable}",
                                    tap["lifetimeInMinutes"].as_i64().unwrap_or(0)
                                ),
                                copy: Some(s(&tap, "temporaryAccessPass")),
                            });
                            if let Some(after) = after {
                                after(app);
                            }
                        }
                        Err(e) => app.fail("Temporary Access Pass", e),
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
                "The password can't be deleted. Use Reset password to change it.".to_string()
            } else {
                format!("{kind} can't be deleted with Microsoft Graph. Remove it in the Microsoft Entra admin centre.")
            };
            self.push(Dialog::message("Delete method", &text));
            return;
        };
        let what = if details.is_empty() { kind.clone() } else { format!("{kind} ({details})") };
        let text = format!("Delete {what} from {name}?\n\n{}", delete_effect(&method));
        self.push(Dialog::confirm("Delete authentication method", &text, move |app| {
            app.spawn(
                "Deleting authentication method…",
                move |g| {
                    require(g, AUTH_METHODS_SCOPE)?;
                    g.delete(&path).map_err(|e| explain(e, Need::Methods))
                },
                move |app, r| match r {
                    Ok(()) => {
                        app.status = format!("Deleted {kind} from {name}");
                        log::info!("{}", app.status);
                        after(app);
                    }
                    Err(e) => app.fail("Delete authentication method", e),
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
        self.spawn("Loading authentication methods…", move |g| load_methods(g, &get), move |app, r| match r {
            Ok(methods) => app.reregistration_form(id, name, methods, after),
            Err(e) => app.fail("Require MFA re-registration", e),
        });
    }

    fn reregistration_form(&mut self, id: String, name: String, methods: Vec<Value>, after: impl FnOnce(&mut App) + Send + 'static) {
        let mut targets: Vec<Value> = methods.into_iter().filter(is_mfa_method).collect();
        if targets.is_empty() {
            self.push(Dialog::message(
                "Require MFA re-registration",
                &format!("{name} has no MFA methods registered, so they will already be asked to register where MFA is required."),
            ));
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
        let intro = format!(
            "Microsoft Graph has no single switch for this. As Microsoft documents, it is done by deleting the \
             user's MFA methods; for {name} these {} will be deleted:\n\n{}\n\n\
             The next time they sign in where MFA is required, they have to register a method again (a Temporary \
             Access Pass can help). Until then, sign-ins that need MFA prompt for registration, or are blocked if \
             your policies don't allow registering from where they are.\n\n\
             Kept: the password, Windows Hello for Business and platform credentials on their devices, the email \
             address used for password reset, and any Temporary Access Pass. Delete those on the Authentication \
             methods tab too if a device may be compromised. A phone number that is their default MFA method may \
             refuse to be deleted; each result is shown afterwards.",
            targets.len(),
            list.join("\n")
        );
        let after = std::cell::RefCell::new(Some(after));
        let form = Form::new(&format!("Require MFA re-registration — {name}"), "Delete methods")
            .intro(&intro)
            .field(Field::check("revoke", "Also revoke sign-in sessions, so apps have to sign in again", false))
            .on_submit(move |app, f| {
                let revoke = f.checked("revoke");
                let jobs: Vec<(String, Option<String>)> =
                    targets.iter().map(|m| (describe_method(m).0, delete_path(&id, m))).collect();
                let revoke_path = format!("/users/{id}/revokeSignInSessions");
                let name = name.clone();
                let after = after.borrow_mut().take();
                app.spawn(
                    "Deleting MFA methods…",
                    move |g| {
                        require(g, AUTH_METHODS_SCOPE)?;
                        let mut results: Vec<(String, Result<(), String>)> = jobs
                            .into_iter()
                            .map(|(label, path)| {
                                let r = match path {
                                    Some(p) => g.delete(&p).map_err(|e| format!("{:#}", explain(e, Need::Methods))),
                                    None => Err("can't be deleted with Microsoft Graph".into()),
                                };
                                (format!("Delete {label}"), r)
                            })
                            .collect();
                        if revoke {
                            let r = g.post(&revoke_path, &json!({})).map(|_| ()).map_err(|e| format!("{:#}", explain(e, Need::Users)));
                            results.push(("Revoke sign-in sessions".into(), r));
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
                            let (title, head) = if failed == 0 {
                                ("MFA re-registration required", format!("All {} steps succeeded for {name}.", results.len()))
                            } else {
                                (
                                    "MFA re-registration incomplete",
                                    format!(
                                        "{failed} of {} steps failed for {name}, so they may still be able to use a method \
                                         that wasn't deleted.",
                                        results.len()
                                    ),
                                )
                            };
                            log::info!("{title}: {head}");
                            app.status = title.into();
                            app.push(Dialog::message(title, &format!("{head}\n\n{}", lines.join("\n"))));
                            if let Some(after) = after {
                                after(app);
                            }
                        }
                        Err(e) => app.fail("Require MFA re-registration", e),
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
