//! Per-turn execution context for [`CodingAgent::send_message`] (feat-041).
//!
//! [`TurnContext`] carries the runtime-facing state every `send_message`
//! call needs: session / workspace identity, working directories, the
//! CLI-native resume id (HTTP runtimes see `None`), the runtime kind,
//! the per-turn [`PermissionSnapshot`](super::permissions::PermissionSnapshot),
//! and the cancellation token.
//!
//! It is the *runtime-facing* complement to
//! [`MessageRequest`](super::MessageRequest), which is the *model-facing*
//! request shape. Keeping the two separate avoids leaking runtime
//! concepts (cwd, cancellation, runtime kind) into the wire format the
//! model sees.
//!
//! ## Populated by
//!
//! `service::sessions::run_prompt_task` constructs the context once per
//! turn, immediately after `ToolContext` is built. The
//! `cwd` / `codebase_root` derivation mirrors the canonical rule at
//! `service/sessions.rs:579-583` so the runtime context and the FS-tool
//! containment boundary agree.
//!
//! `cli_resume_id` is read from `Session::runtime_metadata_json` when
//! present and parseable. A malformed JSON blob is silently swallowed
//! and the field is left `None` — `runtime_metadata_json` is
//! opportunistic metadata, not a required key, and a parse failure must
//! not abort the turn. The full read/write cycle on the metadata column
//! is owned by feat-047.
//!
//! `effective_permissions` is built by calling the per-runtime
//! `PermissionMapper` (feat-046). HTTP runtimes always get an empty
//! snapshot; CLI runtimes get the mapper's output for the session's
//! `ToolProfile`. The runner (feat-043) concatenates the snapshot's
//! `argv_flags` onto the CLI invocation and merges the `env_vars` into
//! the child process env.
//!
//! ## Consumed by
//!
//! `AnthropicAgent::send_message` (native HTTP runtime) ignores every
//! field today. The cancel token is already polled separately in
//! `agent_loop` (`service/sessions.rs:1027`) and does not need to flow
//! through the trait again. Future `CliCodingAgent` implementations
//! (feat-051) will read the full struct to drive subprocess argv / env
//! construction.

use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use super::permissions::PermissionSnapshot;
// `ToolProfile` is only referenced by `test_support` (used in test
// builds). The production binary does not name it directly; silence
// the warning there only — mirrors the re-export pattern in
// `permissions/mod.rs:56` and `claude_code/mod.rs:28`.
#[cfg_attr(not(test), allow(unused_imports))]
use super::permissions::ToolProfile;
use super::RuntimeKind;

/// Per-turn context threaded through `CodingAgent::send_message`.
///
/// Field-by-field rationale:
/// - `session_id` / `workspace_id`: plain `String` (matches the
///   existing `ToolContext` and `Session` shape — no newtype wrappers).
/// - `cwd`: the resolved working directory for this session. The agent
///   uses it for any FS-aware operation; HTTP agents may ignore it.
/// - `codebase_root`: containment boundary. `Some` when the session is
///   bound to a registered codebase (`Session::codebase_id` is
///   `Some`); `None` for HTTP runtimes and for unbound sessions (the
///   spec at `multi-runtime-tasks.md:367` calls for `None for HTTP
///   runtimes`).
/// - `cli_resume_id`: native CLI session id from the previous turn.
///   `None` for HTTP runtimes and for first turns; populated by feat-047
///   once a CLI turn completes.
/// - `runtime_kind`: which runtime this turn is running under. The
///   per-runtime permission shape is in `effective_permissions`.
/// - `effective_permissions`: the per-turn permission snapshot (feat-046)
///   the runner (feat-043) appends to the CLI invocation's argv / env.
///   Always present; HTTP runtimes see an empty snapshot (no flags).
/// - `cancellation_token`: the session-scoped token from
///   `ActiveSessions`. Cancelling the registration cancels the token;
///   the agent loop polls on the same token at
///   `service/sessions.rs:1027`.
#[allow(dead_code)] // Fields are read by future CLI agents (feat-043+).
#[derive(Debug, Clone)]
pub struct TurnContext {
    /// Owning session id.
    pub session_id: String,
    /// Owning workspace id.
    pub workspace_id: String,
    /// Working directory for this session. Defaults to `.` when the
    /// session row has no `cwd`.
    pub cwd: PathBuf,
    /// FS-tool containment boundary. `Some` for sessions bound to a
    /// registered codebase; `None` for HTTP runtimes and unbound
    /// sessions.
    pub codebase_root: Option<PathBuf>,
    /// CLI-native resume id from the previous turn. `None` for HTTP
    /// runtimes and for first turns.
    pub cli_resume_id: Option<String>,
    /// Which runtime this turn is running under. The per-runtime
    /// permission shape is in `effective_permissions`.
    pub runtime_kind: RuntimeKind,
    /// Per-turn permission snapshot the runner (feat-043) appends to
    /// the CLI invocation's argv / env. Built by the per-runtime
    /// `PermissionMapper` (feat-046). HTTP runtimes always see an
    /// empty snapshot.
    pub effective_permissions: PermissionSnapshot,
    /// Cancellation token registered in `ActiveSessions` for this turn.
    pub cancellation_token: CancellationToken,
}

// ---- Test support (shared across test modules and integration tests) ----

pub mod test_support {
    use super::*;

    /// Build a `TurnContext` for tests with sensible defaults: a fresh
    /// cancel token, `.` cwd, no codebase binding, no resume id, an
    /// empty `PermissionSnapshot` (the HTTP default), and the
    /// pre-feat-038 `AnthropicApi` runtime kind. Tests that need
    /// different values build the struct directly.
    pub fn make_test_turn_context() -> TurnContext {
        TurnContext {
            session_id: "test-session".to_string(),
            workspace_id: "test-workspace".to_string(),
            cwd: PathBuf::from("."),
            codebase_root: None,
            cli_resume_id: None,
            runtime_kind: RuntimeKind::default(),
            effective_permissions: PermissionSnapshot::empty(
                RuntimeKind::default(),
                ToolProfile::Full,
            ),
            cancellation_token: CancellationToken::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::make_test_turn_context;

    #[test]
    fn test_turn_context_construction() {
        let ctx = make_test_turn_context();

        assert_eq!(ctx.session_id, "test-session");
        assert_eq!(ctx.workspace_id, "test-workspace");
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert_eq!(ctx.codebase_root, None);
        assert_eq!(ctx.cli_resume_id, None);
        assert_eq!(
            ctx.runtime_kind,
            RuntimeKind::default(),
            "default uses the pre-feat-038 runtime kind"
        );
        assert!(
            ctx.effective_permissions.argv_flags.is_empty(),
            "test default must use the empty HTTP snapshot"
        );
        assert!(
            ctx.effective_permissions.env_vars.is_empty(),
            "test default must use the empty HTTP snapshot"
        );
        assert!(
            !ctx.cancellation_token.is_cancelled(),
            "fresh token is not cancelled"
        );
    }

    #[test]
    fn test_turn_context_cancellation_propagates() {
        // The cancel token in `TurnContext` is a clone of whatever the
        // builder was given; cancelling the original reflects in the
        // context. `CancellationToken` is `Arc`-backed, so the clone
        // shares state.
        let token = CancellationToken::new();
        let mut ctx = make_test_turn_context();
        ctx.cancellation_token = token.clone();

        assert!(!ctx.cancellation_token.is_cancelled());
        token.cancel();
        assert!(ctx.cancellation_token.is_cancelled());
    }
}
