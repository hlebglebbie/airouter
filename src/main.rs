mod admin;
mod benchmark;
mod intel;
mod models;
mod proxy;
mod state;
mod static_files;
mod upstream;

use std::sync::Arc;
use std::time::Duration;

use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::models::Status;
use crate::state::AppState;

const BIND_ADDR: &str = "127.0.0.1:5959";

#[tokio::main]
async fn main() {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("failed to build http client");
    let state = Arc::new(AppState::new(client));

    // Restore persisted providers (API keys survive restarts) and re-check them.
    let persisted = AppState::load_persisted();
    let persisted_count = persisted.providers.len();
    *state.fast_override.write().await = persisted.fast_override;
    *state.think_override.write().await = persisted.think_override;
    *state.tg_token.write().await = persisted.tg_token;
    let ids: Vec<String> = persisted.providers.iter().map(|p| p.id.clone()).collect();
    for p in persisted.providers {
        let provider = crate::state::Provider::new(
            p.id.clone(),
            p.name,
            p.base_url,
            p.api_key,
            p.model_override,
        );
        state.providers.write().await.insert(p.id.clone(), provider);
        let s = state.clone();
        let i = p.id;
        tokio::spawn(async move { benchmark::check_provider(s, i, false).await });
    }
    state.bump_id_counter_after(&ids);
    if persisted_count > 0 {
        println!("airouter: загружено {persisted_count} сохранённых API");
    }

    let recovery_state = state.clone();
    tokio::spawn(async move { recovery_loop(recovery_state).await });

    let app = Router::new()
        .route("/", get(static_files::index))
        .route("/style.css", get(static_files::style_css))
        .route("/app.js", get(static_files::app_js))
        .route("/healthz", get(admin::healthz))
        .route("/v1/models", get(proxy::models_handler))
        .route("/v1/chat/completions", post(proxy::chat_handler))
        .route("/api/state", get(admin::state_handler))
        .route("/api/providers", post(admin::add_provider))
        .route(
            "/api/providers/{id}",
            delete(admin::delete_provider).put(admin::update_provider),
        )
        .route("/api/providers/{id}/check", post(admin::check_provider_handler))
        .route("/api/check_all", post(admin::check_all_handler))
        .route("/api/benchmark", post(admin::benchmark_handler))
        .route("/api/routing", put(admin::update_routing))
        .route("/api/telegram", put(admin::update_telegram))
        .route("/api/telegram/token", get(admin::get_telegram_token))
        .fallback(static_files::not_found)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(BIND_ADDR)
        .await
        .unwrap_or_else(|e| panic!("не удалось занять {BIND_ADDR}: {e}"));

    println!("airouter: веб-интерфейс и OpenAI endpoint на http://{BIND_ADDR}");
    axum::serve(listener, app)
        .await
        .expect("server error");
}

/// Every 60s, re-check `down` providers with a full generation verification
/// and restore the healthy ones back into rotation.
async fn recovery_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.tick().await; // skip the immediate first tick

    loop {
        interval.tick().await;
        recover_unhealthy(&state).await;
    }
}

async fn recover_unhealthy(state: &Arc<AppState>) {
    let down: Vec<(String, String)> = {
        let providers = state.providers.read().await;
        providers
            .values()
            .filter(|p| p.status == Status::Down)
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect()
    };

    for (id, name) in down {
        benchmark::check_provider(state.clone(), id.clone(), true).await;

        let ok = {
            let providers = state.providers.read().await;
            providers.get(&id).map(|p| p.status == Status::Ok).unwrap_or(false)
        };

        if ok {
            state
                .push_event(
                    "auto_recover",
                    format!("«{name}» снова доступен — генерация проверена"),
                )
                .await;
        }
    }
}