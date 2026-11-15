use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

const INDEX_HTML: &str = include_str!("../static/index.html");
const STYLE_CSS: &str = include_str!("../static/style.css");
const APP_JS: &str = include_str!("../static/app.js");

pub async fn index() -> Response {
    ([(CONTENT_TYPE, "text/html; charset=utf-8")], INDEX_HTML).into_response()
}

pub async fn style_css() -> Response {
    ([(CONTENT_TYPE, "text/css; charset=utf-8")], STYLE_CSS).into_response()
}

pub async fn app_js() -> Response {
    (
        [(CONTENT_TYPE, "application/javascript; charset=utf-8")],
        APP_JS,
    )
        .into_response()
}

pub async fn not_found(uri: axum::http::Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") || path.starts_with("/v1/") {
        (
            StatusCode::NOT_FOUND,
            [(CONTENT_TYPE, "application/json")],
            "{\"error\":\"not found\"}",
        )
            .into_response()
    } else {
        index().await
    }
}