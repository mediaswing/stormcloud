//! Minimal blocking Microsoft Graph client.

use crate::auth::{Api, Auth};
use crate::i18n::tf;
use anyhow::{bail, Result};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// Upper bound on objects fetched for a single list, to keep huge tenants responsive.
pub const MAX_ITEMS: usize = 20_000;

pub struct Graph {
    pub auth: Arc<Auth>,
    http: Client,
    base: String,
    exchange: String,
    /// The tenant's ID and initial domain, which Exchange Online requests are addressed with.
    pub(crate) exchange_tenant: OnceLock<(String, String)>,
}

impl Graph {
    pub fn new(auth: Arc<Auth>, http: Client, endpoint: &str, exchange: &str) -> Graph {
        Graph {
            auth,
            http,
            base: format!("{}/v1.0", endpoint.trim_end_matches('/')),
            exchange: exchange.trim_end_matches('/').to_string(),
            exchange_tenant: OnceLock::new(),
        }
    }

    pub fn exchange_endpoint(&self) -> &str {
        &self.exchange
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.base, path)
        }
    }

    fn send(&self, method: Method, path: &str, body: Option<&Value>, eventual: bool) -> Result<Value> {
        let headers: &[(&str, &str)] = if eventual { &[("ConsistencyLevel", "eventual")] } else { &[] };
        self.send_to(Api::Graph, method, &self.url(path), path, body, headers)
    }

    /// Sends a request to `url` on `api`, retrying when throttled. `path` names it in the log.
    pub(crate) fn send_to(
        &self,
        api: Api,
        method: Method,
        url: &str,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Result<Value> {
        for attempt in 0..4 {
            let mut req: RequestBuilder = self
                .http
                .request(method.clone(), url)
                .bearer_auth(self.auth.token_for(api)?);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            if let Some(b) = body {
                req = req.json(b);
                log::debug!("{method} {path} body: {}", redact(b));
            }
            let started = Instant::now();
            let resp = req.send().inspect_err(|e| log::error!("{method} {path}: request failed: {e}"))?;
            let status = resp.status();
            // Microsoft support asks for the request-id when investigating a failure.
            let request_id = resp
                .headers()
                .get("request-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-")
                .to_string();
            let ms = started.elapsed().as_millis();
            if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
                let wait = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(2 << attempt);
                log::warn!("{method} {path} -> {status} in {ms} ms (request-id {request_id}); retrying in {wait}s");
                thread::sleep(Duration::from_secs(wait.min(30)));
                continue;
            }
            let text = resp.text()?;
            if !status.is_success() {
                log::error!("{method} {path} -> {status} in {ms} ms (request-id {request_id}): {}", text.chars().take(2000).collect::<String>());
                return Err(ApiError::new(api, status, &text).into());
            }
            log::debug!("{method} {path} -> {status} in {ms} ms ({} bytes, request-id {request_id})", text.len());
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return Ok(serde_json::from_str(&text)?);
        }
        let service = if api == Api::Graph { "Microsoft Graph" } else { "Exchange Online" };
        bail!(tf("graph.throttled", &[("service", &service)]))
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.send(Method::GET, path, None, false)
    }

    /// GET a collection, following `@odata.nextLink`.
    pub fn list(&self, path: &str) -> Result<Vec<Value>> {
        self.list_inner(path, false).map(|(items, _)| items)
    }

    /// Like [`Graph::list`], also saying whether the list stopped at [`MAX_ITEMS`] with more to come.
    pub fn list_capped(&self, path: &str) -> Result<(Vec<Value>, bool)> {
        self.list_inner(path, false)
    }

    /// GET a collection using advanced query capabilities (`ConsistencyLevel: eventual`).
    pub fn list_advanced(&self, path: &str) -> Result<Vec<Value>> {
        self.list_inner(path, true).map(|(items, _)| items)
    }

    fn list_inner(&self, path: &str, eventual: bool) -> Result<(Vec<Value>, bool)> {
        let mut out = Vec::new();
        let mut next = Some(path.to_string());
        while let Some(p) = next.take() {
            let v = self.send(Method::GET, &p, None, eventual)?;
            if let Some(items) = v["value"].as_array() {
                out.extend(items.iter().cloned());
            }
            next = v["@odata.nextLink"].as_str().map(str::to_string);
            if next.is_some() && out.len() >= MAX_ITEMS {
                return Ok((out, true));
            }
        }
        Ok((out, false))
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(Method::POST, path, Some(body), false)
    }

    pub fn patch(&self, path: &str, body: &Value) -> Result<()> {
        self.send(Method::PATCH, path, Some(body), false).map(|_| ())
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        self.send(Method::DELETE, path, None, false).map(|_| ())
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<()> {
        self.send(Method::PUT, path, Some(body), false).map(|_| ())
    }

    /// The full URL of an object in `collection`, for an `@odata.id` reference.
    pub fn object_url(&self, collection: &str, id: &str) -> String {
        format!("{}/{collection}/{id}", self.base)
    }
}

/// A request that Microsoft Graph or Exchange Online refused, and what it said. Callers tell
/// one failure from another with its methods, never by reading the text shown to the user.
#[derive(Debug)]
pub struct ApiError {
    pub api: Api,
    pub status: StatusCode,
    /// Graph's error code, such as `Request_ResourceNotFound`, or the type of the exception an
    /// Exchange cmdlet threw, such as `…ManagementObjectNotFoundException`. Empty if none was given.
    pub code: String,
    /// The service's own message, in English.
    message: String,
    /// What the user is shown.
    text: String,
}

impl ApiError {
    pub fn new(api: Api, status: StatusCode, body: &str) -> ApiError {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let (code, message, text) = match v["error"]["message"].as_str() {
            // Exchange puts a code and the exception type in front of the message, between bars.
            Some(msg) if api == Api::Exchange => {
                let parts: Vec<&str> = msg.split('|').map(str::trim).collect();
                let exception = parts.iter().find(|p| p.ends_with("Exception")).copied().unwrap_or("");
                let message = parts.last().copied().unwrap_or(msg);
                (exception.to_string(), message.to_string(), message.to_string())
            }
            Some(msg) => {
                let code = v["error"]["code"].as_str().unwrap_or("");
                let shown = if code.is_empty() { "Error" } else { code };
                (code.to_string(), msg.to_string(), format!("{shown} ({}): {msg}", status.as_u16()))
            }
            None => (String::new(), String::new(), format!("HTTP {}: {}", status, body.chars().take(300).collect::<String>())),
        };
        ApiError { api, status, code, message, text }
    }

    /// The object asked about doesn't exist (or, just after it was created, isn't everywhere yet).
    pub fn not_found(&self) -> bool {
        match self.api {
            Api::Graph => self.status == StatusCode::NOT_FOUND || self.code == "Request_ResourceNotFound",
            // Exchange gives no reliable status, so go by the exception, or by the message where
            // there is none.
            Api::Exchange if self.code.is_empty() => self.message.contains("couldn't be found"),
            Api::Exchange => self.code.ends_with("NotFoundException"),
        }
    }

    /// The person signed in isn't allowed to do it.
    pub fn forbidden(&self) -> bool {
        self.status == StatusCode::FORBIDDEN
    }

    /// What was being added is already there, such as a member already in a group. Graph only
    /// says so in the message, under the general `Request_BadRequest` code.
    pub fn already_exists(&self) -> bool {
        self.status == StatusCode::BAD_REQUEST && self.message.contains("already exist")
    }

    /// The tenant needs a Microsoft Entra ID P1 or P2 licence for this.
    pub fn needs_premium(&self) -> bool {
        // Such as `Authentication_RequestFromNonPremiumTenantOrB2CTenant`.
        self.code.contains("RequestFromNonPremiumTenant") || self.message.contains("premium license")
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for ApiError {}

/// The service's refusal behind `e`, if that is what `e` is.
pub fn api_error(e: &anyhow::Error) -> Option<&ApiError> {
    e.downcast_ref::<ApiError>()
}

/// Copy of a request body with passwords and secrets masked, for logging.
pub fn redact(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let key = k.to_ascii_lowercase();
                    let masked = (key.contains("password") && !v.is_object())
                        || key.contains("secret")
                        || key == "temporaryaccesspass";
                    (k.clone(), if masked { Value::String("***".into()) } else { redact(v) })
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(redact).collect()),
        other => other.clone(),
    }
}

/// Percent-encodes a value for use inside an OData query string.
pub fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Escapes and encodes a string literal for an OData `$filter`.
pub fn odata_str(s: &str) -> String {
    enc(&s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_passwords_and_secrets() {
        let body = serde_json::json!({
            "displayName": "Ann",
            "passwordProfile": { "password": "hunter2", "forceChangePasswordNextSignIn": true },
            "passwordCredential": { "displayName": "ci", "secretText": "abc" },
            "tap": { "temporaryAccessPass": "+drkzqAD", "lifetimeInMinutes": 60 },
        });
        let out = redact(&body).to_string();
        assert!(!out.contains("hunter2") && !out.contains("abc") && !out.contains("drkzqAD"));
        assert!(out.contains("Ann") && out.contains("forceChangePasswordNextSignIn"));
    }

    #[test]
    fn reads_graph_errors() {
        let body = r#"{"error":{"code":"Request_ResourceNotFound","message":"Resource 'x' does not exist."}}"#;
        let e = ApiError::new(Api::Graph, StatusCode::NOT_FOUND, body);
        // The same text as before errors were structured.
        assert_eq!(e.to_string(), "Request_ResourceNotFound (404): Resource 'x' does not exist.");
        assert!(e.not_found() && !e.forbidden() && !e.already_exists());

        let body = r#"{"error":{"code":"Request_BadRequest","message":"One or more added object references already exist for the following modified properties: 'members'."}}"#;
        assert!(ApiError::new(Api::Graph, StatusCode::BAD_REQUEST, body).already_exists());

        let body = r#"{"error":{"code":"Authorization_RequestDenied","message":"Insufficient privileges."}}"#;
        assert!(ApiError::new(Api::Graph, StatusCode::FORBIDDEN, body).forbidden());

        let body = r#"{"error":{"code":"Authentication_RequestFromNonPremiumTenantOrB2CTenant","message":"Neither tenant is B2C or tenant doesn't have premium license"}}"#;
        assert!(ApiError::new(Api::Graph, StatusCode::FORBIDDEN, body).needs_premium());
        let body = r#"{"error":{"code":"Authentication_RequestFromNonPremiumTenantOrB2CTenant","message":"Reworded."}}"#;
        assert!(ApiError::new(Api::Graph, StatusCode::FORBIDDEN, body).needs_premium());

        let e = ApiError::new(Api::Graph, StatusCode::BAD_GATEWAY, "<html>bad gateway</html>");
        assert_eq!(e.to_string(), "HTTP 502 Bad Gateway: <html>bad gateway</html>");
        assert!(!e.not_found());

        // An error carried through context is still found.
        let wrapped = anyhow::Error::from(ApiError::new(Api::Graph, StatusCode::NOT_FOUND, "")).context("loading the user");
        assert!(api_error(&wrapped).is_some_and(ApiError::not_found));
    }

    #[test]
    fn reads_exchange_errors() {
        let body = r#"{"error":{"code":"InternalServerError","message":"|Microsoft.Exchange.Configuration.Tasks.ManagementObjectNotFoundException|The operation couldn't be performed because object 'ann@contoso.com' couldn't be found on 'XYZ.PROD.OUTLOOK.COM'."}}"#;
        let e = ApiError::new(Api::Exchange, StatusCode::INTERNAL_SERVER_ERROR, body);
        assert_eq!(e.to_string(), "The operation couldn't be performed because object 'ann@contoso.com' couldn't be found on 'XYZ.PROD.OUTLOOK.COM'.");
        assert!(e.code.ends_with("ManagementObjectNotFoundException"));
        assert!(e.not_found());

        let body = r#"{"error":{"message":"Ex1|Microsoft.Exchange.Data.StoragePermanentException|Something else broke."}}"#;
        assert!(!ApiError::new(Api::Exchange, StatusCode::INTERNAL_SERVER_ERROR, body).not_found());
    }

    #[test]
    fn encodes_filter_literals() {
        assert_eq!(odata_str("O'Neil & co"), "O%27%27Neil%20%26%20co");
    }
}
