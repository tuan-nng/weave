use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Application-level errors that map to HTTP responses.
///
/// Each variant carries enough context for a useful error message.
/// The `IntoResponse` impl maps variants to status codes and a
/// consistent JSON envelope: `{"error": {"code": "...", "message": "..."}}`.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // Variants will be used as handlers are added (feat-004+)
pub enum AppError {
    #[error("Not found: {resource} {id}")]
    NotFound { resource: String, id: String },

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

/// Errors specific to AI provider interactions.
///
/// Variants distinguish recoverable conditions (rate limiting) from
/// permanent failures (auth, model not found) so callers can decide
/// whether to retry.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // Variants will be used when provider integration lands (feat-006)
pub enum ProviderError {
    #[error("Authentication failed")]
    AuthFailed,
    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("Model not found: {model}")]
    ModelNotFound { model: String },
    #[error("Provider unreachable: {0}")]
    Unreachable(String),
    #[error("Stream interrupted: {0}")]
    StreamInterrupted(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::NotFound { .. } => (StatusCode::NOT_FOUND, "not_found", self.to_string()),
            AppError::Validation(_) => (
                StatusCode::BAD_REQUEST,
                "validation_error",
                self.to_string(),
            ),
            AppError::Provider(ProviderError::AuthFailed) => {
                (StatusCode::UNAUTHORIZED, "auth_failed", self.to_string())
            }
            AppError::Provider(ProviderError::RateLimited { .. }) => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                self.to_string(),
            ),
            AppError::Provider(_) => (StatusCode::BAD_GATEWAY, "provider_error", self.to_string()),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict", self.to_string()),
            AppError::Database(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Internal server error".to_string(),
                )
            }
            AppError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Internal server error".to_string(),
                )
            }
        };

        (
            status,
            Json(json!({"error": {"code": code, "message": message}})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    /// Extract status code and parsed JSON body from a response.
    async fn error_to_parts(error: AppError) -> (StatusCode, serde_json::Value) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    /// Verify every AppError variant produces the correct JSON envelope shape.
    #[tokio::test]
    async fn test_error_response_format() {
        let cases: Vec<(AppError, &str, &str)> = vec![
            (
                AppError::NotFound {
                    resource: "workspace".into(),
                    id: "abc123".into(),
                },
                "not_found",
                "workspace",
            ),
            (
                AppError::Validation("name too long".into()),
                "validation_error",
                "name too long",
            ),
            (
                AppError::Provider(ProviderError::AuthFailed),
                "auth_failed",
                "Authentication failed",
            ),
            (
                AppError::Provider(ProviderError::RateLimited {
                    retry_after_ms: 5000,
                }),
                "rate_limited",
                "5000",
            ),
            (
                AppError::Provider(ProviderError::ModelNotFound {
                    model: "gpt-5".into(),
                }),
                "provider_error",
                "gpt-5",
            ),
            (
                AppError::Provider(ProviderError::Unreachable("connection refused".into())),
                "provider_error",
                "connection refused",
            ),
            (
                AppError::Provider(ProviderError::StreamInterrupted("timeout".into())),
                "provider_error",
                "timeout",
            ),
        ];

        for (error, expected_code, expected_message_sub) in cases {
            let (_status, json) = error_to_parts(error).await;

            // Verify envelope shape
            assert!(json.get("error").is_some(), "missing 'error' key");
            let err_obj = &json["error"];
            assert!(err_obj.get("code").is_some(), "missing 'code' key");
            assert!(err_obj.get("message").is_some(), "missing 'message' key");

            // Verify code and message content
            assert_eq!(err_obj["code"], expected_code, "wrong error code");
            let msg = err_obj["message"].as_str().unwrap();
            assert!(
                msg.contains(expected_message_sub),
                "message '{}' should contain '{}'",
                msg,
                expected_message_sub
            );
        }

        // Database and Internal errors must return sanitized message
        for error in [
            AppError::Database(rusqlite::Error::ExecuteReturnedResults),
            AppError::Internal(anyhow::anyhow!("secret internal detail")),
        ] {
            let (_status, json) = error_to_parts(error).await;
            let err_obj = &json["error"];
            assert_eq!(err_obj["code"], "internal_error");
            assert_eq!(
                err_obj["message"], "Internal server error",
                "must not leak internal details"
            );
        }
    }

    /// Verify every AppError variant maps to the correct HTTP status code.
    #[tokio::test]
    async fn test_error_status_codes() {
        let cases: Vec<(AppError, StatusCode)> = vec![
            (
                AppError::NotFound {
                    resource: "session".into(),
                    id: "x".into(),
                },
                StatusCode::NOT_FOUND,
            ),
            (AppError::Validation("bad".into()), StatusCode::BAD_REQUEST),
            (
                AppError::Provider(ProviderError::AuthFailed),
                StatusCode::UNAUTHORIZED,
            ),
            (
                AppError::Provider(ProviderError::RateLimited {
                    retry_after_ms: 1000,
                }),
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                AppError::Provider(ProviderError::ModelNotFound { model: "x".into() }),
                StatusCode::BAD_GATEWAY,
            ),
            (
                AppError::Provider(ProviderError::Unreachable("x".into())),
                StatusCode::BAD_GATEWAY,
            ),
            (
                AppError::Provider(ProviderError::StreamInterrupted("x".into())),
                StatusCode::BAD_GATEWAY,
            ),
            (
                AppError::Database(rusqlite::Error::ExecuteReturnedResults),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AppError::Internal(anyhow::anyhow!("x")),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (error, expected_status) in cases {
            let (status, _json) = error_to_parts(error).await;
            assert_eq!(
                status, expected_status,
                "wrong status code for error variant"
            );
        }
    }

    /// Verify #[from] derives produce the correct variant.
    #[test]
    fn test_from_conversions() {
        let err: AppError = rusqlite::Error::ExecuteReturnedResults.into();
        assert!(
            matches!(err, AppError::Database(_)),
            "rusqlite::Error should convert to AppError::Database"
        );

        let err: AppError = ProviderError::AuthFailed.into();
        assert!(
            matches!(err, AppError::Provider(ProviderError::AuthFailed)),
            "ProviderError should convert to AppError::Provider"
        );

        let err: AppError = anyhow::anyhow!("test error").into();
        assert!(
            matches!(err, AppError::Internal(_)),
            "anyhow::Error should convert to AppError::Internal"
        );
    }
}
