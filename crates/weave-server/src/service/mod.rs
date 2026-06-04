pub mod kanban;
pub mod sessions;
pub mod startup;

use std::collections::HashMap;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

/// Thread-safe map of active session IDs to their cancellation tokens.
///
/// Used by `SessionService` to track in-flight streaming tasks and support
/// cancellation via `POST /api/sessions/:sid/cancel`.
pub struct ActiveSessions {
    inner: Mutex<HashMap<String, CancellationToken>>,
}

impl ActiveSessions {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Atomically check-and-insert. Returns `true` if the session was newly
    /// inserted, `false` if it was already present. This prevents TOCTOU races
    /// between concurrent `send_prompt` calls for the same session.
    pub fn try_insert(&self, session_id: String, token: CancellationToken) -> bool {
        let mut map = self.inner.lock().expect("active_sessions lock poisoned");
        if map.contains_key(&session_id) {
            return false;
        }
        map.insert(session_id, token);
        true
    }

    /// Remove a session from the active map (called when streaming completes).
    pub fn remove(&self, session_id: &str) {
        self.inner
            .lock()
            .expect("active_sessions lock poisoned")
            .remove(session_id);
    }

    /// Get the cancellation token for an active session, if any.
    pub fn get(&self, session_id: &str) -> Option<CancellationToken> {
        self.inner
            .lock()
            .expect("active_sessions lock poisoned")
            .get(session_id)
            .cloned()
    }

    /// Check whether a session is currently active.
    pub fn contains(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .expect("active_sessions lock poisoned")
            .contains_key(session_id)
    }

    /// Number of currently-active sessions.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("active_sessions lock poisoned")
            .len()
    }

    /// Cancel every active session's token.
    ///
    /// Iterates the map, calls `.cancel()` on each `CancellationToken`, and
    /// returns the number of tokens cancelled. The map itself is NOT
    /// cleared here — `SessionGuard::drop` is responsible for removing the
    /// entry once the streaming task actually returns, which prevents
    /// `cancel_all` from racing with a session that finished between the
    /// snapshot and the cancel.
    ///
    /// Used by the graceful-shutdown sequence (feat-034) to abort every
    /// in-flight agent stream in response to SIGTERM / SIGINT.
    pub fn cancel_all(&self) -> usize {
        let map = self.inner.lock().expect("active_sessions lock poisoned");
        let count = map.len();
        for token in map.values() {
            token.cancel();
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_active_sessions_cancel_all_cancels_every_token() {
        let sessions = ActiveSessions::new();
        let token_a = CancellationToken::new();
        let token_b = CancellationToken::new();
        let token_c = CancellationToken::new();

        sessions.try_insert("a".into(), token_a.clone());
        sessions.try_insert("b".into(), token_b.clone());
        sessions.try_insert("c".into(), token_c.clone());

        let cancelled = sessions.cancel_all();
        assert_eq!(
            cancelled, 3,
            "all three tokens should be reported cancelled"
        );
        assert!(token_a.is_cancelled());
        assert!(token_b.is_cancelled());
        assert!(token_c.is_cancelled());
    }

    #[tokio::test]
    async fn test_active_sessions_cancel_all_empty_is_zero() {
        let sessions = ActiveSessions::new();
        assert_eq!(sessions.cancel_all(), 0);
    }

    #[tokio::test]
    async fn test_active_sessions_len_tracks_inserts_and_removes() {
        let sessions = ActiveSessions::new();
        assert_eq!(sessions.len(), 0);

        sessions.try_insert("x".into(), CancellationToken::new());
        assert_eq!(sessions.len(), 1);

        sessions.try_insert("y".into(), CancellationToken::new());
        assert_eq!(sessions.len(), 2);

        sessions.remove("x");
        assert_eq!(sessions.len(), 1);
    }
}
