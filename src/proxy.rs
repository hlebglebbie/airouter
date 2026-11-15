use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::models::{Status, VirtualModel};
use crate::state::AppState;
use crate::upstream::{self, ChatResult};

pub async fn models_handler() -> impl IntoResponse {
    json_response(
        StatusCode::OK,
        &json!({
            "object": "list",
            "data": [
                {"id": "fast", "object": "model", "created": 0, "owned_by": "ai-router"},
                {"id": "think", "object": "model", "created": 0, "owned_by": "ai-router"}
            ]
        }),
    )
}

pub async fn chat_handler(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return json_error(StatusCode::BAD_REQUEST, "invalid JSON", "invalid_request_error")
        }
    };

    let model = match value.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "missing 'model' field",
                "invalid_request_error",
            )
        }
    };
    let virtual_model = match VirtualModel::from_marker(&model) {
        Some(v) => v,
        None => {
            return json_error(
                StatusCode::NOT_FOUND,
                "model not found",
                "invalid_request_error",
            )
        }
    };
    let stream = value.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let marker = virtual_model.marker();

    let candidates = state.candidates_for(virtual_model).await;
    if candidates.is_empty() {
        return json_error(
            StatusCode::BAD_GATEWAY,
            "no healthy providers",
            "router_error",
        );
    }

    let mut last_error: Option<(StatusCode, String)> = None;

    for cand in candidates {
        let mut upstream_body = value.clone();
        upstream_body["model"] = json!(cand.model);
        let url = upstream::chat_url(&cand.base_url, &cand.v1_prefix);

        match upstream::chat_request(
            &state.http,
            &url,
            &cand.api_key,
            &upstream_body,
            stream,
            upstream::CHAT_TIMEOUT_SECS,
        )
        .await
        {
            Ok(ChatResult::Json { status, body }) if status < 400 => {
                let mut response = body.clone();
                response["model"] = json!(marker);
                return json_response(StatusCode::OK, &response);
            }
            Ok(ChatResult::Json { status, body }) => {
                let msg = upstream::extract_error(&body);
                mark_down(&state, &cand.provider_id, &format!("http {status}: {msg}")).await;
                last_error = Some((status_from_u16(status), msg));
            }
            Ok(ChatResult::Stream { resp }) => {
                // Committed: relay the SSE stream as-is, no more failover.
                return stream_response(resp);
            }
            Err(e) => {
                mark_down(&state, &cand.provider_id, &e).await;
                last_error = Some((StatusCode::BAD_GATEWAY, e));
            }
        }
    }

    match last_error {
        Some((status, msg)) => {
            json_response(status, &json!({"error": {"message": msg, "type": "upstream_error"}}))
        }
        None => json_error(StatusCode::BAD_GATEWAY, "no healthy providers", "router_error"),
    }
}

async fn mark_down(state: &Arc<AppState>, provider_id: &str, msg: &str) {
    {
        let mut providers = state.providers.write().await;
        if let Some(p) = providers.get_mut(provider_id) {
            p.status = Status::Down;
            p.last_error = Some(msg.to_string());
        }
    }
    let name = state
        .providers
        .read()
        .await
        .get(provider_id)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| provider_id.to_string());
    state
        .push_event("failover", format!("{name} недоступен ({msg}) — переключение"))
        .await;
}

fn stream_response(resp: reqwest::Response) -> Response {
    let stream = resp.bytes_stream().map(|item| {
        item.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });
    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    *response.status_mut() = StatusCode::OK;
    response
}

fn json_error(status: StatusCode, message: &str, kind: &str) -> Response {
    json_response(status, &json!({"error": {"message": message, "type": kind}}))
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    (status, [(CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

fn status_from_u16(n: u16) -> StatusCode {
    StatusCode::from_u16(n).unwrap_or(StatusCode::BAD_GATEWAY)
}