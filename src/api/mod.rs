pub mod chat;

use crate::metrics;
use axum::response::IntoResponse;

pub async fn health() -> impl IntoResponse {
    "ok"
}

pub async fn metrics() -> impl IntoResponse {
    match metrics::render() {
        Ok(body) => body.into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("metrics error: {err}"),
        )
            .into_response(),
    }
}
