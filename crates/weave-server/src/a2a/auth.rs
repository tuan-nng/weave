//! A2A token verification.
//!
//! The A2A protocol requires authentication. Weave uses a simple
//! shared token approach: the `WEAVE_A2A_TOKEN` environment variable
//! is read at startup. All A2A endpoints check the `Authorization:
//! Bearer <token>` header against this value.
//!
//! If no token is configured (None), auth is disabled — useful for
//! localhost development where TLS isn't used.

use crate::error::AppError;
use axum::http::HeaderMap;

/// Verify the `Authorization: Bearer <token>` header against the
/// configured A2A token.
///
/// Returns `Ok(())` if auth passes, `Err(AppError::Unauthorized)`
/// otherwise.
pub fn verify_a2a_token(
    configured_token: &Option<String>,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    let Some(expected) = configured_token else {
        // Auth disabled — no token configured
        return Ok(());
    };

    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match auth {
        Some(t) if t == expected => Ok(()),
        _ => Err(AppError::Unauthorized(
            "invalid or missing A2A token; provide a valid Bearer token in the Authorization header"
                .into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn test_a2a_auth_disabled_when_no_token_configured() {
        let headers = HeaderMap::new();
        assert!(verify_a2a_token(&None, &headers).is_ok());
    }

    #[test]
    fn test_a2a_auth_rejects_missing_header() {
        let headers = HeaderMap::new();
        let result = verify_a2a_token(&Some("secret".into()), &headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Unauthorized(_) => {}
            other => panic!("expected Unauthorized, got {:?}", other),
        }
    }

    #[test]
    fn test_a2a_auth_rejects_wrong_token() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer wrong".parse().unwrap());
        let result = verify_a2a_token(&Some("secret".into()), &headers);
        assert!(result.is_err());
    }

    #[test]
    fn test_a2a_auth_accepts_correct_token() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer secret".parse().unwrap());
        assert!(verify_a2a_token(&Some("secret".into()), &headers).is_ok());
    }

    #[test]
    fn test_a2a_auth_rejects_missing_bearer_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "secret".parse().unwrap());
        let result = verify_a2a_token(&Some("secret".into()), &headers);
        assert!(result.is_err());
    }
}
