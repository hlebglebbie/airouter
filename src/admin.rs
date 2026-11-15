use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::benchmark;
use crate::models::{AddProviderRequest, RoutingUpdateRequest, StateDto, UpdateProviderRequest};
use crate::state::{AppState, Provider};
use crate::models::Status;

pub async fn healthz() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

pub async fn state_handler(State(state): State<Arc<AppState>>) -> Json<StateDto> {
    Json(state.snapshot().await)
}

pub async fn add_provider(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddProviderRequest>,
) -> Response {
    let name = req.name.trim();
    let base_url = req.base_url.trim();
    if name.is_empty() || base_url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "name и base_url обязательны" })),
        )
            .into_response();
    }

    let id = state.new_provider_id();
    let model_override = req
        .model_override
        .as_deref()
        .map(str::trim)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let provider = Provider::new(
        id.clone(),
        name.to_string(),
        crate::upstream::normalize_base(base_url),
        req.api_key.trim().to_string(),
        model_override,
    );

    state
        .providers
        .write()
        .await
        .insert(id.clone(), provider);
    state.save().await;
    state
        .push_event("add", format!("добавлен API «{name}»"))
        .await;

    let s = state.clone();
    let i = id.clone();
    tokio::spawn(async move { benchmark::check_provider(s, i, false).await });

    (StatusCode::CREATED, Json(json!({ "id": id }))).into_response()
}

pub async fn delete_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match state.providers.write().await.remove(&id) {
        Some(p) => {
            state.save().await;
            state
                .push_event("delete", format!("удалён API «{}»", p.name))
                .await;
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
    }
}

pub async fn update_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProviderRequest>,
) -> Response {
    let name = req.name.trim();
    let base_url = req.base_url.trim();
    if name.is_empty() || base_url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "name и base_url обязательны" })),
        )
            .into_response();
    }

    let model_override = req
        .model_override
        .as_deref()
        .map(str::trim)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let new_key = req.api_key.trim();

    {
        let mut providers = state.providers.write().await;
        let Some(p) = providers.get_mut(&id) else {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
        };
        p.name = name.to_string();
        p.base_url = crate::upstream::normalize_base(base_url);
        if !new_key.is_empty() {
            p.api_key = new_key.to_string();
        }
        p.model_override = model_override;
        // Runtime fields are stale after an edit — reset and re-check.
        p.v1_prefix = String::new();
        p.status = Status::Checking;
        p.last_error = None;
        p.last_latency_ms = None;
        p.models = Vec::new();
        p.benchmark = None;
        p.in_flight = false;
    }

    state.save().await;
    state
        .push_event("edit", format!("обновлён API «{name}»"))
        .await;

    let s = state.clone();
    let i = id.clone();
    tokio::spawn(async move { benchmark::check_provider(s, i, false).await });

    (StatusCode::OK, Json(json!({ "id": id }))).into_response()
}

pub async fn check_provider_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if !state.providers.read().await.contains_key(&id) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    let s = state.clone();
    tokio::spawn(async move { benchmark::check_provider(s, id, false).await });
    (StatusCode::ACCEPTED, Json(json!({ "status": "started" }))).into_response()
}

pub async fn check_all_handler(State(state): State<Arc<AppState>>) -> Response {
    let s = state.clone();
    tokio::spawn(async move { benchmark::check_all(s).await });
    (StatusCode::ACCEPTED, Json(json!({ "status": "started" }))).into_response()
}

pub async fn update_routing(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RoutingUpdateRequest>,
) -> Response {
    *state.fast_override.write().await = clean_override(req.fast);
    *state.think_override.write().await = clean_override(req.think);
    state.save().await;
    state
        .push_event("routing", "изменён выбор моделей (fast/think)".to_string())
        .await;
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

fn clean_override(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub async fn benchmark_handler(State(state): State<Arc<AppState>>) -> Response {
    let s = state.clone();
    tokio::spawn(async move { benchmark::benchmark_all(s).await });
    (StatusCode::ACCEPTED, Json(json!({ "status": "started" }))).into_response()
}