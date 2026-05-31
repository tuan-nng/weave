use axum::Json;
use serde::Serialize;
use std::time::Instant;

/// Shared server start time for computing uptime.
/// Set once during startup and passed into handlers via Extension.
#[derive(Clone, Copy)]
pub struct ServerStartTime(pub Instant);

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_seconds: u64,
}

/// GET /api/health
///
/// Returns server status, version, and uptime.
pub async fn health_check(
    axum::Extension(start_time): axum::Extension<ServerStartTime>,
) -> Json<HealthResponse> {
    let uptime = start_time.0.elapsed().as_secs();
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: uptime,
    })
}
