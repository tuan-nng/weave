pub mod health;

use axum::{routing::get, Router};
use health::ServerStartTime;

/// Build the API router with all routes.
pub fn router(start_time: ServerStartTime) -> Router {
    Router::new()
        .route("/api/health", get(health::health_check))
        .layer(axum::Extension(start_time))
}
