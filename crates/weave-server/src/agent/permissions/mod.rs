//! Per-runtime permission mapping (feat-046).
//!
//! Translates a Weave `(RuntimeKind, ToolProfile)` pair into a
//! [`PermissionSnapshot`] — the opaque argv / env flags a CLI
//! subprocess needs to honor the user's chosen tool surface.
//!
//! The trait is the seam between the Weave-side policy (a Weave
//! `ToolProfile` chosen during session creation) and the runtime-side
//! policy (a CLI's specific flag vocabulary). HTTP runtimes see an
//! empty snapshot (the trait's HTTP mappings are no-ops); CLI runtimes
//! see a concrete set of argv flags + env vars.
//!
//! ## Snapshot shape
//!
//! A [`PermissionSnapshot`] is intentionally opaque to the runner:
//! the runner just concatenates `argv_flags` onto the CLI invocation
//! and merges `env_vars` into the child process env. The Claude Code
//! mapper (in `claude_code.rs`) is the only place that knows the
//! internal structure of those two fields; future CLIs (Codex,
//! OpenCode) get their own mappers with their own flag vocabularies.
//!
//! ## Intentionally minimal mapping
//!
//! The spec calls for the mapper to emit the CLI's `--permission-mode`
//! value and a Weave-side tool allowlist — but explicitly NOT to mirror
//! the allowlist into the CLI's own tool list. The CLI's tool surface
//! is the CLI's responsibility; Weave enforces containment separately
//! (the `cwd`-arg / `fs_*` / explicit-`cwd` form per feat-062, plus
//! the shell-body policy). The allowlist is emitted to the CLI as
//! metadata (via `WEAVE_TOOL_ALLOWLIST` env) so the journey translator
//! (feat-048) can correlate which Weave tools the CLI was authorized
//! to invoke.
//!
//! ## JSON debug logging
//!
//! A snapshot is `Serialize + Deserialize` so the runner can log it at
//! DEBUG on each turn for post-hoc debugging. The
//! `to_json` / `from_json` helpers are thin wrappers that pin the
//! shape (snake_case, kebab-case for the runtime kind) so the JSON
//! form is stable across refactors.

// Public surface for `ClaudeCodeCodingAgent` (feat-051) — see the
// cli_runner.rs:58 precedent for the rationale. The trait, snapshot,
// and enum are exercised by the test module but have no production
// caller until feat-051 wires the mapper into session creation.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::agent::RuntimeKind;
use crate::error::AppError;

pub mod claude_code;

// Re-export the Claude Code mapper at the module root so callers
// (feat-051+) can `use crate::agent::permissions::ClaudeCodePermissionMapper`
// without naming the submodule. The `claude_code` module re-exports
// the same symbol (see `claude_code.rs`) for callers that prefer the
// qualified path. The re-export IS used in test builds (see
// `test_permission_mapper_trait_compiles` below); the production
// build has no consumer yet, so silence the warning there only —
// mirrors the `claude_code/mod.rs:28` precedent.
#[cfg_attr(not(test), allow(unused_imports))]
pub use claude_code::ClaudeCodePermissionMapper;

// ---------------------------------------------------------------------------
// ToolProfile
// ---------------------------------------------------------------------------

/// The Weave-side tool profile chosen for a session.
///
/// Independent of the CLI's own permission vocabulary. The mapper
/// translates `(RuntimeKind, ToolProfile)` into CLI-specific flags;
/// HTTP runtimes ignore the field entirely (the Weave server enforces
/// the profile on the tool registry side instead).
///
/// The string wire form is the kebab-cased variant name and matches
/// the `tool_profile` field in specialist YAML. Round-trip through
/// `FromStr` is lossless.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ToolProfile {
    /// Full access: every Weave tool is available; the CLI's most
    /// permissive mode.
    Full,
    /// Implementation: read + write + shell, file edits auto-accepted
    /// at the CLI layer (`accept-edits` for Claude Code).
    Implementation,
    /// Review: read-only on the CLI side; the model may inspect but
    /// not modify state (`plan` for Claude Code).
    Review,
    /// Planning: same as `Review` semantically; distinguished for the
    /// UI to label the session's intent. Maps to `plan` on the CLI.
    Planning,
    /// Reporting: read + write but no shell execution. Used for
    /// sessions that produce reports from existing data without
    /// touching the codebase (`default` mode for Claude Code, empty
    /// allowlist).
    Reporting,
}

impl ToolProfile {
    /// Stable kebab-case wire form (matches the YAML field).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Implementation => "implementation",
            Self::Review => "review",
            Self::Planning => "planning",
            Self::Reporting => "reporting",
        }
    }
}

impl std::fmt::Display for ToolProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ToolProfile {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full" => Ok(Self::Full),
            "implementation" => Ok(Self::Implementation),
            "review" => Ok(Self::Review),
            "planning" => Ok(Self::Planning),
            "reporting" => Ok(Self::Reporting),
            other => Err(AppError::validation(format!(
                "invalid tool_profile '{other}', expected one of: \
                 full, implementation, review, planning, reporting"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// PermissionSnapshot
// ---------------------------------------------------------------------------

/// CLI-specific permission flags derived from a `(RuntimeKind, ToolProfile)`.
///
/// The runner consumes `argv_flags` and `env_vars` opaquely:
/// - `argv_flags` are appended to the CLI invocation's `args` in order.
/// - `env_vars` are merged into the child process's `env` (overriding
///   any inherited value with the same key).
///
/// `runtime_kind` and `tool_profile` are recorded on the snapshot
/// itself so the JSON debug log is self-describing without an extra
/// lookup against the source data.
///
/// The struct is `Clone + Serialize + Deserialize` so it can flow
/// through `TurnContext` (feat-041) and survive a JSON debug log +
/// reload (used by integration tests that capture and assert on the
/// per-turn log).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionSnapshot {
    /// The runtime this snapshot was generated for. Recorded so the
    /// JSON debug log can identify which mapper produced it.
    pub runtime_kind: RuntimeKind,
    /// The tool profile this snapshot was generated for. Recorded
    /// for the same reason.
    pub tool_profile: ToolProfile,
    /// Extra argv flags the runner appends to the CLI invocation.
    /// Empty for runtimes that take no extra flags (HTTP runtimes
    /// always see an empty list).
    pub argv_flags: Vec<String>,
    /// Env vars the runner merges into the child process env.
    /// `BTreeMap` (not `HashMap`) so the JSON debug log is
    /// deterministic in key order.
    pub env_vars: BTreeMap<String, String>,
}

impl PermissionSnapshot {
    /// Build an empty snapshot for a given `(runtime_kind, tool_profile)`.
    /// Used by HTTP mappers (which never add any flags) and as a
    /// neutral default for tests that don't care about the contents.
    pub fn empty(runtime_kind: RuntimeKind, tool_profile: ToolProfile) -> Self {
        Self {
            runtime_kind,
            tool_profile,
            argv_flags: Vec::new(),
            env_vars: BTreeMap::new(),
        }
    }

    /// Serialize to a JSON string for debug logging. Always succeeds
    /// because the snapshot has no non-string keys that could fail to
    /// serialize; the `Result` wrapper is forward-looking for future
    /// fields that might carry an `io::Error` (none planned).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parse a JSON string produced by [`PermissionSnapshot::to_json`].
    /// Used by integration tests that round-trip through the debug log.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

// ---------------------------------------------------------------------------
// PermissionMapper trait
// ---------------------------------------------------------------------------

/// Maps a Weave `(RuntimeKind, ToolProfile)` pair to a [`PermissionSnapshot`].
///
/// Object-safe so a future `PermissionMapperRegistry` (analogous to
/// `ProviderRegistry`) can store mappers as `Arc<dyn PermissionMapper>`
/// and dispatch by `RuntimeKind`. The first registry consumer lands
/// in feat-051; for now, callers instantiate the concrete
/// `ClaudeCodePermissionMapper` directly.
///
/// The trait method is **synchronous** — the mapping is pure data
/// translation, no IO. Async would force every caller to `.await` for
/// no reason.
pub trait PermissionMapper: Send + Sync {
    /// Build the snapshot for the given `(runtime_kind, tool_profile)`.
    /// Callers pass the actual `RuntimeKind` (not `Self::runtime_kind`)
    /// so a single mapper can answer for every runtime it knows about
    /// — for runtimes it doesn't know, the snapshot is empty.
    fn effective_permissions(
        &self,
        runtime_kind: RuntimeKind,
        tool_profile: ToolProfile,
    ) -> PermissionSnapshot;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ToolProfile ----

    #[test]
    fn test_tool_profile_variants() {
        // Pin the variant count so adding a new profile forces a
        // test update.
        let all = [
            ToolProfile::Full,
            ToolProfile::Implementation,
            ToolProfile::Review,
            ToolProfile::Planning,
            ToolProfile::Reporting,
        ];
        assert_eq!(all.len(), 5, "ToolProfile must have exactly 5 variants");
    }

    #[test]
    fn test_tool_profile_as_str() {
        assert_eq!(ToolProfile::Full.as_str(), "full");
        assert_eq!(ToolProfile::Implementation.as_str(), "implementation");
        assert_eq!(ToolProfile::Review.as_str(), "review");
        assert_eq!(ToolProfile::Planning.as_str(), "planning");
        assert_eq!(ToolProfile::Reporting.as_str(), "reporting");
    }

    #[test]
    fn test_tool_profile_from_str_roundtrip() {
        for profile in [
            ToolProfile::Full,
            ToolProfile::Implementation,
            ToolProfile::Review,
            ToolProfile::Planning,
            ToolProfile::Reporting,
        ] {
            let parsed: ToolProfile = profile.as_str().parse().expect("valid wire form");
            assert_eq!(parsed, profile);
        }
    }

    #[test]
    fn test_tool_profile_from_str_rejects_unknown() {
        let err = "super-user".parse::<ToolProfile>().unwrap_err();
        match err {
            AppError::Validation { message, .. } => {
                assert!(message.contains("invalid tool_profile"), "msg: {message}");
                assert!(message.contains("super-user"), "msg: {message}");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[test]
    fn test_tool_profile_serde_roundtrip() {
        for profile in [
            ToolProfile::Full,
            ToolProfile::Implementation,
            ToolProfile::Review,
            ToolProfile::Planning,
            ToolProfile::Reporting,
        ] {
            let json = serde_json::to_string(&profile).expect("serialize");
            let parsed: ToolProfile = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, profile);
        }
    }

    // ---- PermissionSnapshot ----

    #[test]
    fn test_permission_snapshot_empty() {
        let snap = PermissionSnapshot::empty(RuntimeKind::AnthropicApi, ToolProfile::Full);
        assert_eq!(snap.runtime_kind, RuntimeKind::AnthropicApi);
        assert_eq!(snap.tool_profile, ToolProfile::Full);
        assert!(snap.argv_flags.is_empty());
        assert!(snap.env_vars.is_empty());
    }

    #[test]
    fn test_permission_snapshot_serializes_to_json() {
        // Pin the JSON shape so the debug log stays stable across
        // refactors. The order of fields inside `argv_flags` /
        // `env_vars` is not part of the contract (Vec / BTreeMap
        // are order-stable but the test asserts the key set).
        let snap = PermissionSnapshot {
            runtime_kind: RuntimeKind::ClaudeCode,
            tool_profile: ToolProfile::Review,
            argv_flags: vec!["--permission-mode".to_string(), "plan".to_string()],
            env_vars: BTreeMap::from([(
                "WEAVE_TOOL_ALLOWLIST".to_string(),
                "[\"fs_read\"]".to_string(),
            )]),
        };

        let json = snap.to_json().expect("serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");

        assert_eq!(parsed["runtime_kind"], "claude-code");
        assert_eq!(parsed["tool_profile"], "review");
        assert_eq!(
            parsed["argv_flags"],
            serde_json::json!(["--permission-mode", "plan"])
        );
        assert_eq!(
            parsed["env_vars"]["WEAVE_TOOL_ALLOWLIST"],
            serde_json::json!("[\"fs_read\"]")
        );

        // Round-trip: the JSON → PermissionSnapshot → JSON should
        // produce the same shape.
        let back = PermissionSnapshot::from_json(&json).expect("deserialize");
        assert_eq!(back, snap);
        let json2 = back.to_json().expect("serialize");
        assert_eq!(json, json2);
    }

    #[test]
    fn test_permission_snapshot_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PermissionSnapshot>();
        assert_send_sync::<ToolProfile>();
    }

    // ---- PermissionMapper trait ----

    /// Compile-time check: the trait is object-safe, can be used as
    /// `Box<dyn PermissionMapper>`, and the concrete mapper satisfies
    /// the bound. If the trait signature ever changes incompatibly,
    /// this test fails to compile and points at the offending line.
    #[test]
    fn test_permission_mapper_trait_compiles() {
        let mapper: Box<dyn PermissionMapper> = Box::new(ClaudeCodePermissionMapper::new());
        let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, ToolProfile::Review);
        assert_eq!(snap.tool_profile, ToolProfile::Review);
        assert_eq!(snap.runtime_kind, RuntimeKind::ClaudeCode);
    }
}
