use std::time::Duration;

use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};

pub const CHECK_TIMEOUT_SECS: u64 = 20;
pub const BENCH_TIMEOUT_SECS: u64 = 30;
pub const CHAT_TIMEOUT_SECS: u64 = 300;

#[derive(Debug)]
pub enum ChatResult {
    /// Full (non-streaming) response body already read.
    Json { status: u16, body: Value },
    /// Streaming response committed to — body must be relayed as-is.
    Stream { resp: reqwest::Response },
}

pub fn normalize_base(base: &str) -> String {
    base.trim().trim_end_matches('/').to_string()
}

/// Which `/models` path candidates to probe for a given base URL.
pub fn candidate_prefixes(base: &str) -> Vec<&'static str> {
    if base.ends_with("/v1") {
        vec![""]
    } else {
        vec!["", "/v1"]
    }
}

pub fn models_url(base: &str, prefix: &str) -> String {
    format!("{base}{prefix}/models")
}

pub fn chat_url(base: &str, prefix: &str) -> String {
    format!("{base}{prefix}/chat/completions")
}

/// Fetches and parses the model list. Returns `(v1_prefix, model_ids)` where
/// `v1_prefix` is "" or "/v1" — whichever path answered.
pub async fn fetch_models(
    client: &reqwest::Client,
    base: &str,
    key: &str,
) -> Result<(String, Vec<String>), String> {
    let mut last_error: Option<String> = None;

    for prefix in candidate_prefixes(base) {
        let url = models_url(base, prefix);
        let mut req = client
            .get(&url)
            .timeout(Duration::from_secs(CHECK_TIMEOUT_SECS));
        if !key.is_empty() {
            req = req.bearer_auth(key);
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                last_error = Some(format!("ошибка соединения: {e}"));
                continue;
            }
        };
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);

        match status {
            200 => {
                let ids = parse_model_ids(&body);
                return Ok((prefix.to_string(), ids));
            }
            401 | 403 => {
                return Err(format!(
                    "ошибка авторизации ({status}): {}",
                    extract_error(&body)
                ));
            }
            404 => {
                last_error = Some("endpoint /models не найден".to_string());
                continue;
            }
            other => {
                last_error = Some(format!("http {other}: {}", extract_error(&body)));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "не удалось получить /models".to_string()))
}

/// Sends a `/chat/completions` request. `stream=true` returns the live
/// response for relay; otherwise the full body is read.
pub async fn chat_request(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    body: &Value,
    stream: bool,
    timeout_secs: u64,
) -> Result<ChatResult, String> {
    let mut builder = client.post(url).header(CONTENT_TYPE, "application/json").json(body);
    if !key.is_empty() {
        builder = builder.bearer_auth(key);
    }

    let resp = if stream {
        // Bound only time-to-headers; the body keeps streaming as long as needed.
        match tokio::time::timeout(Duration::from_secs(timeout_secs), builder.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(format!("ошибка соединения: {e}")),
            Err(_) => return Err("таймаут ожидания ответа upstream".to_string()),
        }
    } else {
        builder
            .timeout(Duration::from_secs(timeout_secs))
            .send()
            .await
            .map_err(|e| format!("ошибка соединения: {e}"))?
    };

    let status = resp.status().as_u16();
    if stream && status < 400 {
        return Ok(ChatResult::Stream { resp });
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("ошибка чтения ответа: {e}"))?;
    let body_val: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        json!({ "message": String::from_utf8_lossy(&bytes).into_owned() })
    });
    Ok(ChatResult::Json {
        status,
        body: body_val,
    })
}

/// Defensively parses OpenAI-compatible `/models` responses.
fn parse_model_ids(body: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    let arr = body
        .get("data")
        .or_else(|| body.get("models"))
        .unwrap_or(body);
    let items = match arr {
        Value::Array(a) => a,
        _ => return ids,
    };
    for item in items {
        match item {
            Value::String(s) => ids.push(s.clone()),
            Value::Object(o) => {
                if let Some(Value::String(s)) = o
                    .get("id")
                    .or_else(|| o.get("model"))
                    .or_else(|| o.get("name"))
                {
                    ids.push(s.clone());
                }
            }
            _ => {}
        }
    }
    ids
}

/// Extracts the most useful error message from an upstream JSON error body.
pub fn extract_error(body: &Value) -> String {
    if let Some(msg) = body.pointer("/error/message").and_then(|v| v.as_str()) {
        return msg.to_string();
    }
    if let Some(msg) = body.get("message").and_then(|v| v.as_str()) {
        return msg.to_string();
    }
    "без деталей от upstream".to_string()
}