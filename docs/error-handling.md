# Error Handling

## Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Not found: {resource} {id}")]
    NotFound { resource: String, id: String },

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
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
```

## Error-to-HTTP Mapping

```rust
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::NotFound { .. } => (404, "not_found", self.to_string()),
            AppError::Validation(_) => (400, "validation_error", self.to_string()),
            AppError::Provider(ProviderError::AuthFailed) => (401, "auth_failed", self.to_string()),
            AppError::Provider(ProviderError::RateLimited { .. }) => (429, "rate_limited", self.to_string()),
            AppError::Provider(_) => (502, "provider_error", self.to_string()),
            AppError::Database(_) => (500, "internal_error", "Internal server error".into()),
            AppError::Internal(_) => (500, "internal_error", "Internal server error".into()),
        };
        (status, Json(json!({"error": {"code": code, "message": message}}))).into_response()
    }
}
```

## Retry Strategy for Provider Calls

```rust
async fn with_retry<F, T, E>(f: F, max_retries: u32) -> Result<T, E>
where
    F: Fn() -> Pin<Box<dyn Future<Output = Result<T, E>>>>,
    E: IsRetryable,
{
    let mut attempts = 0;
    loop {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if e.is_retryable() && attempts < max_retries => {
                attempts += 1;
                let delay = Duration::from_millis(1000 * 2u64.pow(attempts));
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}
```

**Retryable errors**: rate limits (429), server errors (500), overloaded (529), network timeouts.
**Non-retryable**: auth failures (401/403), bad requests (400), model not found (404).

## Provider HTTP Status Handling

| HTTP Status | Meaning | Retry? |
|-------------|---------|--------|
| 400 | Bad request (invalid message format) | No |
| 401 | Invalid API key | No |
| 403 | Permission denied | No |
| 404 | Model not found | No |
| 429 | Rate limited | Yes (with backoff) |
| 500 | Server error | Yes (once) |
| 529 | Overloaded | Yes (with backoff) |
