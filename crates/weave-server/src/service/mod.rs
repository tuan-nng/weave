pub mod kanban;
pub mod sessions;
pub mod startup;

#[cfg(test)]
mod sessions_wrapped_tests;

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

/// Thread-safe map of active CLI subprocess pids, keyed by session id
/// (feat-049).
///
/// Sibling of [`ActiveSessions`], but tracks the child *process* (the pid)
/// rather than the streaming task's cancel token. Three consumers:
///
///   * `CliRunner::run` registers the pid after spawn and unregisters on
///     exit (success, cancel, or spawn-failure).
///   * `service::startup::reap_cli_processes` reads /proc on cold start
///     to find surviving CLI children — but does NOT consult this
///     in-memory table (it's empty after a crash by construction).
///   * The `POST /api/sessions/{sid}/cancel` handler calls
///     [`ActiveChildProcesses::terminate`] as a defense-in-depth path:
///     the primary cancel signal is the `CancellationToken` in
///     `ActiveSessions`, but a tracked pid lets us SIGTERM the
///     process group directly if the token path is somehow unavailable.
///
/// The struct itself is unconditional; the kill methods are `#[cfg(unix)]`
/// because `libc::killpg` is the right primitive on Unix and the
/// runners are Unix-only (see `process_group(0)` at
/// `agent/cli_runner.rs:242-245`).
pub struct ActiveChildProcesses {
    inner: Mutex<HashMap<String, u32>>,
}

impl ActiveChildProcesses {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Register a session's child pid. Returns the prior pid if any
    /// (a re-register on retry). Per-turn records — never outlive the
    /// turn. The runner calls this once after `cmd.spawn()` and once
    /// with `unregister` on the matching exit path.
    pub fn register(&self, session_id: String, pid: u32) -> Option<u32> {
        let mut map = self
            .inner
            .lock()
            .expect("active_child_processes lock poisoned");
        map.insert(session_id, pid)
    }

    /// Unregister a session's child. Returns the prior pid if any. The
    /// runner calls this on every exit path (success, cancel, error) so
    /// the table never holds phantom entries.
    pub fn unregister(&self, session_id: &str) -> Option<u32> {
        self.inner
            .lock()
            .expect("active_child_processes lock poisoned")
            .remove(session_id)
    }

    /// Look up the pid for a session, if any.
    pub fn get(&self, session_id: &str) -> Option<u32> {
        self.inner
            .lock()
            .expect("active_child_processes lock poisoned")
            .get(session_id)
            .copied()
    }

    /// Number of currently-tracked children.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("active_child_processes lock poisoned")
            .len()
    }

    /// Snapshot of all tracked pids. For tests / observability.
    pub fn pids(&self) -> Vec<u32> {
        self.inner
            .lock()
            .expect("active_child_processes lock poisoned")
            .values()
            .copied()
            .collect()
    }

    /// Send SIGTERM to the process group of the session's tracked
    /// child, then remove the entry. Returns `true` if a pid was
    /// found and signaled.
    ///
    /// `killpg` matches the runner's `wait_or_cancel` primitive
    /// (`agent/cli_runner.rs:457`); the runner spawns with
    /// `process_group(0)` so the entire tree is signaled. On
    /// non-Unix, this is a no-op that still clears the entry.
    #[cfg(unix)]
    pub fn terminate(&self, session_id: &str) -> bool {
        let pid = match self.unregister(session_id) {
            Some(p) => p,
            None => return false,
        };
        // Safety: killpg is libc; pid came from a child we registered
        // after spawn with process_group(0). ESRCH and other errors
        // are intentionally ignored — the caller has done its best;
        // the token path remains as the primary cancel signal.
        let _ = unsafe { libc::killpg(pid as i32, libc::SIGTERM) };
        true
    }

    #[cfg(not(unix))]
    pub fn terminate(&self, session_id: &str) -> bool {
        self.unregister(session_id).is_some()
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

    // -----------------------------------------------------------------
    // ActiveChildProcesses (feat-049)
    // -----------------------------------------------------------------

    #[test]
    fn test_active_child_processes_lifecycle() {
        let procs = ActiveChildProcesses::new();
        assert_eq!(procs.len(), 0);
        assert!(procs.pids().is_empty());

        // Register a pid.
        let prior = procs.register("session-a".into(), 1234);
        assert_eq!(prior, None, "first register returns no prior pid");
        assert_eq!(procs.len(), 1);
        assert_eq!(procs.get("session-a"), Some(1234));
        assert_eq!(procs.pids(), vec![1234]);

        // Re-register the same session: returns the prior pid.
        let prior = procs.register("session-a".into(), 5678);
        assert_eq!(prior, Some(1234), "re-register returns the prior pid");
        assert_eq!(procs.get("session-a"), Some(5678));
        assert_eq!(procs.len(), 1, "still one entry after re-register");

        // Unregister returns the current pid and clears the entry.
        let pid = procs.unregister("session-a");
        assert_eq!(pid, Some(5678));
        assert_eq!(procs.len(), 0);
        assert_eq!(procs.get("session-a"), None);

        // Unregistering a missing key is a no-op.
        assert_eq!(procs.unregister("never-registered"), None);
    }

    #[cfg(unix)]
    #[test]
    fn test_active_child_processes_terminate_signals_pid() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        let procs = ActiveChildProcesses::new();

        // Spawn a sleeper via /bin/sh so the test doesn't depend on
        // /usr/bin/sleep being on PATH (the runner uses the same
        // /bin/sh -c form for the same reason — see cli_runner tests).
        // Use `exec sleep 60` so the shell IS the sleep process —
        // `child.id()` is then the sleep pid directly, and
        // `killpg(pid, SIGTERM)` signals the whole tree.
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec sleep 60"])
            .process_group(0)
            .spawn()
            .expect("spawn sleeper");
        let pid = child.id();

        // Register the pid under a session id, then terminate it.
        procs.register("session-kill".into(), pid);
        let signaled = procs.terminate("session-kill");
        assert!(signaled, "terminate should report the pid was found");

        // The entry is gone (terminate removes the entry as it
        // signals, so a subsequent terminate is a no-op).
        assert_eq!(procs.len(), 0);
        assert!(
            !procs.terminate("session-kill"),
            "double-terminate is no-op"
        );

        // Reap the child so it transitions from zombie to gone.
        // `kill(pid, 0)` returns 0 for zombies (the pid is still in
        // the process table until the parent reaps), so a bare
        // `kill 0` check would falsely report the child as alive.
        let status = child.wait().expect("wait on child");
        assert!(
            !status.success(),
            "sleeper should be killed by SIGTERM, not exit cleanly"
        );

        // The pid is now truly gone.
        // SAFETY: kill(pid, 0) is the standard "is process alive" check.
        let alive = unsafe { libc::kill(pid as i32, 0) };
        assert_ne!(
            alive, 0,
            "child pid {pid} should no longer be alive after terminate + reap"
        );
    }

    #[test]
    fn test_active_child_processes_terminate_missing_returns_false() {
        let procs = ActiveChildProcesses::new();
        assert!(!procs.terminate("never-registered"));
    }
}
