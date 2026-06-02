pub mod kanban;
pub mod sessions;

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
}
