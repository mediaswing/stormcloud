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
                bail!(graph_error(api, status, &text));
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

fn graph_error(api: Api, status: StatusCode, text: &str) -> String {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    match v["error"]["message"].as_str() {
        // Exchange puts the exception type and a code in front of the message, between bars.
        Some(msg) if api == Api::Exchange => msg.rsplit('|').next().unwrap_or(msg).trim().to_string(),
        Some(msg) => format!("{} ({}): {}", v["error"]["code"].as_str().unwrap_or("Error"), status.as_u16(), msg),
        None => format!("HTTP {}: {}", status, text.chars().take(300).collect::<String>()),
    }
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
    fn encodes_filter_literals() {
        assert_eq!(odata_str("O'Neil & co"), "O%27%27Neil%20%26%20co");
    }
}
