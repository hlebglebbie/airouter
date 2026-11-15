use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::intel;
use crate::models::Status;
use crate::state::AppState;
use crate::upstream::{self, ChatResult};

const CANDIDATE_CAP: usize = 6;
const CONCURRENCY: usize = 4;

/// One-word latency test; returns elapsed milliseconds on success.
async fn time_one_word(
    client: &reqwest::Client,
    base: &str,
    prefix: &str,
    key: &str,
    model: &str,
) -> Result<u64, String> {
    let url = upstream::chat_url(base, prefix);
    let mkbody = || {
        json!({
            "model": model,
            "messages": [{"role": "user", "content": "Reply with exactly one word: ready"}],
            "temperature": 0,
            "stream": false,
            "max_tokens": 32
        })
    };

    let t0 = Instant::now();
    match upstream::chat_request(client, &url, key, &mkbody(), false, upstream::BENCH_TIMEOUT_SECS).await
    {
        Ok(ChatResult::Json { status: 200, .. }) => Ok(t0.elapsed().as_millis() as u64),
        Ok(ChatResult::Json { status: 400, body }) if mentions_max_tokens(&body) => {
            // o-series / gpt-5 accept only max_completion_tokens.
            let mut body2 = mkbody();
            if let Some(obj) = body2.as_object_mut() {
                obj.remove("max_tokens");
                obj.insert("max_completion_tokens".to_string(), json!(32));
            }
            let t1 = Instant::now();
            match upstream::chat_request(client, &url, key, &body2, false, upstream::BENCH_TIMEOUT_SECS).await
            {
                Ok(ChatResult::Json { status, body: _ }) if status < 400 => {
                    Ok(t1.elapsed().as_millis() as u64)
                }
                Ok(ChatResult::Json { status, body }) => {
                    Err(format!("http {status}: {}", upstream::extract_error(&body)))
                }
                Ok(ChatResult::Stream { .. }) => Err("неожиданный стрим при замере".to_string()),
                Err(e) => Err(e),
            }
        }
        Ok(ChatResult::Json { status, body }) => {
            Err(format!("http {status}: {}", upstream::extract_error(&body)))
        }
        Ok(ChatResult::Stream { .. }) => Err("неожиданный стрим при замере".to_string()),
        Err(e) => Err(e),
    }
}

fn mentions_max_tokens(body: &Value) -> bool {
    upstream::extract_error(body)
        .to_ascii_lowercase()
        .contains("max_tokens")
        || body.to_string().contains("max_tokens")
}

enum CheckOutcome {
    Healthy {
        prefix: String,
        models: Vec<crate::models::ModelInfo>,
        target: String,
        latency: u64,
        models_listed: bool,
    },
    Down(String),
}

fn guess_prefix(base: &str) -> String {
    if base.ends_with("/v1") {
        String::new()
    } else {
        "/v1".to_string()
    }
}

/// Runs and times the one-word generation test for a chosen model.
/// `models_listed` records whether the provider's /models endpoint responded.
async fn check_generation(
    client: &reqwest::Client,
    snap: &crate::state::Provider,
    prefix: &str,
    models: Vec<crate::models::ModelInfo>,
    models_listed: bool,
) -> CheckOutcome {
    let target = match intel::smallest_chat_model(&models)
        .or_else(|| snap.benchmark.as_ref().map(|b| b.model.clone()))
    {
        Some(t) => t,
        None => return CheckOutcome::Down("нет чат-моделей для проверки".to_string()),
    };

    match time_one_word(client, &snap.base_url, prefix, &snap.api_key, &target).await {
        Ok(latency) => CheckOutcome::Healthy {
            prefix: prefix.to_string(),
            models,
            target,
            latency,
            models_listed,
        },
        Err(e) => CheckOutcome::Down(format!("генерация не работает: {e}")),
    }
}

/// Applies the check result to the provider and emits the event.
async fn apply_check_result(
    state: &Arc<AppState>,
    id: &str,
    outcome: CheckOutcome,
    quiet: bool,
) {
    let models_listed = match &outcome {
        CheckOutcome::Healthy { models_listed, .. } => *models_listed,
        CheckOutcome::Down(_) => false,
    };

    {
        let mut providers = state.providers.write().await;
        let Some(p) = providers.get_mut(id) else { return };
        p.in_flight = false;
        match outcome {
            CheckOutcome::Healthy {
                prefix,
                models,
                target,
                latency,
                ..
            } => {
                p.v1_prefix = prefix;
                p.models = models;
                p.last_error = None;
                p.status = Status::Ok;
                p.last_latency_ms = Some(latency);
                p.benchmark = Some(crate::models::BenchResult {
                    model: target,
                    latency_ms: latency,
                });
            }
            CheckOutcome::Down(e) => {
                p.status = Status::Down;
                p.last_error = Some(e.clone());
            }
        }
    }

    if quiet {
        return;
    }

    let (name, status, model_count, err, latency) = {
        let providers = state.providers.read().await;
        let p = &providers[id];
        (
            p.name.clone(),
            p.status,
            p.models.len(),
            p.last_error.clone(),
            p.last_latency_ms,
        )
    };
    match status {
        Status::Ok => {
            let models_note = if models_listed {
                format!("моделей {model_count}")
            } else {
                "модели неизвестны (/models недоступен)".to_string()
            };
            state
                .push_event(
                    "check",
                    format!("{name}: ok, {models_note}, генерация {} мс", latency.unwrap_or(0)),
                )
                .await
        }
        _ => {
            state
                .push_event("check", format!("{name}: down — {}", err.unwrap_or_default()))
                .await
        }
    }
}

/// Quick check of one provider: generation is always tested and timed,
/// even when /models is unavailable. /models only enriches the known model
/// list when it responds.
pub async fn check_provider(state: Arc<AppState>, id: String, quiet: bool) {
    {
        let mut providers = state.providers.write().await;
        let Some(p) = providers.get_mut(&id) else { return };
        if p.in_flight {
            return;
        }
        p.in_flight = true;
        p.status = Status::Checking;
    }

    let snap = match state.providers.read().await.get(&id) {
        Some(p) => p.clone(),
        None => return,
    };

    let outcome: CheckOutcome =
        match upstream::fetch_models(&state.http, &snap.base_url, &snap.api_key).await {
            Ok((prefix, ids)) => {
                let models = intel::score_models(ids);
                check_generation(&state.http, &snap, &prefix, models, true).await
            }
            Err(_) => {
                // /models недоступен — не парсим модели, всё равно меряем генерацию.
                let prefix = if snap.v1_prefix.is_empty() {
                    guess_prefix(&snap.base_url)
                } else {
                    snap.v1_prefix.clone()
                };
                let models = if !snap.models.is_empty() {
                    snap.models.clone()
                } else if let Some(ov) = snap.model_override.as_deref().filter(|s| !s.is_empty()) {
                    intel::score_models(vec![ov.to_string()])
                } else {
                    return apply_check_result(
                        &state,
                        &id,
                        CheckOutcome::Down(
                            "недоступен /models и нет известной модели — укажите «Модель» в карточке API"
                                .to_string(),
                        ),
                        quiet,
                    )
                    .await;
                };
                check_generation(&state.http, &snap, &prefix, models, false).await
            }
        };

    apply_check_result(&state, &id, outcome, quiet).await;
}

pub async fn check_all(state: Arc<AppState>) {
    let ids: Vec<String> = state.providers.read().await.keys().cloned().collect();
    futures_util::future::join_all(ids.into_iter().map(|id| {
        let s = state.clone();
        async move { check_provider(s, id, false).await }
    }))
    .await;
}

/// Full speed race across every healthy provider's candidate shortlist.
pub async fn benchmark_all(state: Arc<AppState>) {
    if state
        .benchmark_running
        .swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }
    state.push_event("benchmark", "запущен забег скорости".into()).await;

    let mut tasks: Vec<(String, String, String, String, String)> = Vec::new();
    {
        let providers = state.providers.read().await;
        for p in providers.values().filter(|p| p.status == Status::Ok) {
            for m in intel::benchmark_shortlist(&p.models, CANDIDATE_CAP) {
                tasks.push((
                    p.id.clone(),
                    p.base_url.clone(),
                    p.v1_prefix.clone(),
                    p.api_key.clone(),
                    m,
                ));
            }
        }
    }

    let client = state.http.clone();
    let results: Vec<(String, String, Result<u64, String>)> = futures_util::stream::iter(
        tasks
            .into_iter()
            .map(|(pid, base, prefix, key, model)| {
                let c = client.clone();
                async move {
                    let r = time_one_word(&c, &base, &prefix, &key, &model).await;
                    (pid, model, r)
                }
            }),
    )
    .buffer_unordered(CONCURRENCY)
    .collect()
    .await;

    let mut per_provider: HashMap<String, crate::models::BenchResult> = HashMap::new();
    for (pid, model, res) in results {
        if let Ok(ms) = res {
            match per_provider.get_mut(&pid) {
                Some(entry) if ms < entry.latency_ms => {
                    entry.model = model;
                    entry.latency_ms = ms;
                }
                None => {
                    per_provider.insert(
                        pid,
                        crate::models::BenchResult {
                            model,
                            latency_ms: ms,
                        },
                    );
                }
                _ => {}
            }
        }
    }

    {
        let mut providers = state.providers.write().await;
        for (pid, bench) in per_provider {
            if let Some(p) = providers.get_mut(&pid) {
                p.benchmark = Some(bench);
            }
        }
    }

    state
        .benchmark_running
        .store(false, std::sync::atomic::Ordering::Relaxed);

    let snap = state.snapshot().await;
    match &snap.routing_fast {
        Some(f) => {
            state
                .push_event(
                    "benchmark",
                    format!(
                        "fast → {} @ {} ({} мс)",
                        f.model,
                        f.provider_name,
                        f.latency_ms.unwrap_or(0)
                    ),
                )
                .await
        }
        None => {
            state
                .push_event("benchmark", "забег завершён, но нет ни одного замера".into())
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Json, Router};

    use crate::state::Provider;

    async fn spawn_mock(chat_status: u16) -> String {
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { Json(json!({ "data": [{ "id": "tiny-1b" }] })) }),
            )
            .route(
                "/v1/chat/completions",
                post(move |Json(_): Json<Value>| async move {
                    if chat_status < 400 {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "id": "c", "object": "chat.completion", "model": "tiny-1b",
                                "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ready" }, "finish_reason": "stop" }]
                            })),
                        )
                            .into_response()
                    } else {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": { "message": "boom" } })),
                        )
                            .into_response()
                    }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/v1")
    }

    /// Mock upstream WITHOUT a /models endpoint (always 404), but with a
    /// working /chat/completions.
    async fn spawn_chat_only_mock(chat_status: u16) -> String {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(_): Json<Value>| async move {
                if chat_status < 400 {
                    (
                        StatusCode::OK,
                        Json(json!({
                            "id": "c", "object": "chat.completion", "model": "gpt-4o-mini",
                            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ready" }, "finish_reason": "stop" }]
                        })),
                    )
                        .into_response()
                } else {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": { "message": "boom" } })),
                    )
                        .into_response()
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/v1")
    }

    fn provider(base_url: String) -> Provider {
        Provider {
            id: "p1".to_string(),
            name: "mock".to_string(),
            base_url,
            api_key: String::new(),
            v1_prefix: String::new(),
            status: Status::Checking,
            last_error: None,
            last_latency_ms: None,
            models: Vec::new(),
            benchmark: None,
            model_override: None,
            in_flight: false,
        }
    }

    #[tokio::test]
    async fn check_verifies_generation_and_measures_speed() {
        let base = spawn_mock(200).await;
        let state = Arc::new(AppState::new(reqwest::Client::new()));
        state.providers.write().await.insert("p1".into(), provider(base));

        check_provider(state.clone(), "p1".into(), true).await;

        let p = state.providers.read().await.get("p1").unwrap().clone();
        assert_eq!(p.status, Status::Ok);
        assert_eq!(p.models.len(), 1);
        assert!(p.last_latency_ms.is_some(), "должна быть измерена скорость");
        assert_eq!(p.benchmark.as_ref().map(|b| b.model.as_str()), Some("tiny-1b"));
    }

    #[tokio::test]
    async fn check_marks_down_when_generation_fails() {
        let base = spawn_mock(500).await;
        let state = Arc::new(AppState::new(reqwest::Client::new()));
        state.providers.write().await.insert("p1".into(), provider(base));

        check_provider(state.clone(), "p1".into(), true).await;

        let p = state.providers.read().await.get("p1").unwrap().clone();
        assert_eq!(p.status, Status::Down);
        assert!(
            p.last_error
                .as_deref()
                .is_some_and(|e| e.contains("генерация не работает")),
            "last_error должен указывать на сбой генерации, а было: {:?}",
            p.last_error
        );
    }

    #[tokio::test]
    async fn check_measures_speed_when_models_unavailable() {
        // /models returns 404, but generation works — speed must still be
        // measured using the override model, not fail on missing /models.
        let base = spawn_chat_only_mock(200).await;
        let state = Arc::new(AppState::new(reqwest::Client::new()));
        let mut p = provider(base);
        p.model_override = Some("gpt-4o-mini".to_string());
        state.providers.write().await.insert("p1".into(), p);

        check_provider(state.clone(), "p1".into(), true).await;

        let p = state.providers.read().await.get("p1").unwrap().clone();
        assert_eq!(p.status, Status::Ok);
        assert_eq!(
            p.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["gpt-4o-mini"]
        );
        assert!(p.last_latency_ms.is_some(), "скорость должна быть измерена");
    }
}