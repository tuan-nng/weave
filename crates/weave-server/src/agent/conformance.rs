//! Shared CLI adapter conformance contract (feat-057).
//!
//! Every [`CliCodingAgent`] implementation must satisfy the
//! [`ConformanceAdapter`] trait before shipping. The integration test
//! suite at `tests/cli_conformance.rs` exercises these methods against
//! the fake CLI harness.
//!
//! ## Adding a new adapter
//!
//! 1. Implement [`CliStreamParser`] for your parser type.
//! 2. Implement [`ConformanceAdapter`] for your adapter, delegating
//!    to the concrete components.
//! 3. Add a `_your_runtime` variant of each test in
//!    `tests/cli_conformance.rs`.
//!
//! The conformance suite is the forcing function for the adapter
//! contract — it is NOT a unit test. It runs as an integration test
//! (`tests/`) so it can only access `pub` items from this crate.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::agent::permissions::{PermissionMapper, PermissionSnapshot, ToolProfile};
use crate::agent::turn_context::TurnContext;
use crate::agent::{ProviderError, RuntimeKind, StreamEvent};

// ---------------------------------------------------------------------------
// CliStreamParser trait
// ---------------------------------------------------------------------------

/// Common interface for all CLI stream parsers.
///
/// Mirrors the public API of [`crate::agent::claude_code::parser::ClaudeCodeStreamParser`]
/// so the conformance suite can test any parser with the same scenarios.
pub trait CliStreamParser: Send {
    /// Feed one line of newline-delimited JSON from the CLI's stdout.
    fn feed_line(&mut self, line: &str) -> Result<Option<Vec<StreamEvent>>, ProviderError>;

    /// Passive getter for the captured session id (if any).
    fn session_id(&self) -> Option<String>;

    /// Consuming getter — returns the session id and clears it.
    fn take_session_id(&mut self) -> Option<String>;

    /// Drain the first pending deferred `ToolUseStart` event.
    fn flush(&mut self) -> Vec<StreamEvent>;
}

// ---------------------------------------------------------------------------
// ConformanceAdapter trait
// ---------------------------------------------------------------------------

/// Bundles the adapter-specific components the conformance suite
/// exercises. Each CLI adapter (Claude Code, Codex, OpenCode)
/// implements this trait.
///
/// The trait is intentionally narrow: it covers what's truly shared
/// across adapters (argv construction, env construction, parser
/// creation, permission mapping). The journey translator is
/// adapter-specific and tested directly through the concrete type.
pub trait ConformanceAdapter: Send + Sync {
    /// The runtime kind this adapter serves.
    fn runtime_kind(&self) -> RuntimeKind;

    /// Path to the fake CLI binary used for conformance testing.
    fn fake_cli_path(&self) -> PathBuf;

    /// Build a [`CliInvocation`](crate::agent::cli_runner::CliInvocation)
    /// from the given args, env, and cwd. The binary path comes from
    /// [`fake_cli_path`](Self::fake_cli_path).
    fn build_invocation(
        &self,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: PathBuf,
    ) -> crate::agent::cli_runner::CliInvocation;

    /// Create a fresh stream parser for this adapter.
    fn new_parser(&self) -> Box<dyn CliStreamParser>;

    /// Return the adapter's permission mapper.
    fn permission_mapper(&self) -> &dyn PermissionMapper;

    /// Build a [`TurnContext`] with sensible test defaults for the
    /// given session id, cwd, and optional codebase root.
    fn make_turn_context(
        &self,
        session_id: &str,
        cwd: PathBuf,
        codebase_root: Option<PathBuf>,
    ) -> TurnContext;
}

// ---------------------------------------------------------------------------
// Claude Code adapter implementation
// ---------------------------------------------------------------------------

/// Claude Code adapter for conformance testing.
///
/// Delegates to [`crate::agent::claude_code::parser::ClaudeCodeStreamParser`]
/// and [`crate::agent::permissions::claude_code::ClaudeCodePermissionMapper`].
pub struct ClaudeCodeConformanceAdapter;

impl ConformanceAdapter for ClaudeCodeConformanceAdapter {
    fn runtime_kind(&self) -> RuntimeKind {
        RuntimeKind::ClaudeCode
    }

    fn fake_cli_path(&self) -> PathBuf {
        // `CARGO_BIN_EXE_fake_cli` is set by Cargo for integration tests.
        // Fallback walks up from the test binary to target/debug/.
        std::env::var("CARGO_BIN_EXE_fake_cli")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let exe = std::env::current_exe().expect("current_exe is set");
                exe.parent()
                    .and_then(|p| p.parent())
                    .expect("target/debug/")
                    .join("fake_cli")
            })
    }

    fn build_invocation(
        &self,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: PathBuf,
    ) -> crate::agent::cli_runner::CliInvocation {
        crate::agent::cli_runner::CliInvocation {
            binary: self.fake_cli_path(),
            args,
            env,
            cwd,
            stdin_payload: None,
        }
    }

    fn new_parser(&self) -> Box<dyn CliStreamParser> {
        Box::new(ClaudeCodeParserAdapter(
            crate::agent::claude_code::ClaudeCodeStreamParser::new(),
        ))
    }

    fn permission_mapper(&self) -> &dyn PermissionMapper {
        // Leak the mapper for the trait-object return. This is test-only
        // code; the leak is bounded and harmless.
        Box::leak(Box::new(
            crate::agent::permissions::ClaudeCodePermissionMapper::new(),
        ))
    }

    fn make_turn_context(
        &self,
        session_id: &str,
        cwd: PathBuf,
        codebase_root: Option<PathBuf>,
    ) -> TurnContext {
        use tokio_util::sync::CancellationToken;
        TurnContext {
            session_id: session_id.to_string(),
            workspace_id: "conformance-test-workspace".to_string(),
            cwd,
            codebase_root,
            cli_resume_id: None,
            runtime_kind: RuntimeKind::ClaudeCode,
            effective_permissions: PermissionSnapshot::empty(
                RuntimeKind::ClaudeCode,
                ToolProfile::Full,
            ),
            cancellation_token: CancellationToken::new(),
        }
    }
}

/// Wrapper that adapts [`ClaudeCodeStreamParser`] to the
/// [`CliStreamParser`] trait.
struct ClaudeCodeParserAdapter(crate::agent::claude_code::ClaudeCodeStreamParser);

impl CliStreamParser for ClaudeCodeParserAdapter {
    fn feed_line(&mut self, line: &str) -> Result<Option<Vec<StreamEvent>>, ProviderError> {
        self.0.feed_line(line)
    }

    fn session_id(&self) -> Option<String> {
        self.0.session_id().map(String::from)
    }

    fn take_session_id(&mut self) -> Option<String> {
        self.0.take_session_id()
    }

    fn flush(&mut self) -> Vec<StreamEvent> {
        // The parser's `flush()` returns `Option<StreamEvent>`;
        // the trait returns `Vec<StreamEvent>` for consistency.
        self.0.flush().into_iter().collect()
    }
}
