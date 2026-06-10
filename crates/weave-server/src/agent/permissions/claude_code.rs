//! Claude Code `PermissionMapper` implementation (feat-046).
//!
//! Maps a Weave `(RuntimeKind, ToolProfile)` pair to a Claude Code
//! `--permission-mode` value + a Weave-side tool allowlist. The
//! allowlist is emitted to the CLI as a JSON env var
//! (`WEAVE_TOOL_ALLOWLIST`) so the journey translator (feat-048) can
//! correlate which Weave tools the CLI was authorized to invoke —
//! but the allowlist is NOT mirrored into Claude Code's own tool
//! list (Claude Code's tool surface is the CLI's responsibility per
//! the feat-046 spec).
//!
//! ## Mapping table
//!
//! | Weave `ToolProfile` | Claude Code `--permission-mode` | Allowlist                    |
//! |---------------------|----------------------------------|------------------------------|
//! | `Full`              | `bypassPermissions`              | fs_read, fs_write, shell_exec|
//! | `Implementation`    | `acceptEdits`                    | fs_read, fs_write, shell_exec|
//! | `Review`            | `plan`                           | fs_read                      |
//! | `Planning`          | `plan`                           | fs_read                      |
//! | `Reporting`         | `default`                        | (empty — no shell)           |
//!
//! The Claude Code wire form uses camelCase (`acceptEdits`,
//! `bypassPermissions`) for some values and lowercase (`plan`,
//! `default`) for others. The mapping below matches the CLI's
//! documented vocabulary at the time of feat-046.
//!
//! ## Non-Claude-Code runtimes
//!
//! The mapper returns an **empty** snapshot for any runtime that is
//! not `RuntimeKind::ClaudeCode`. The trait is the seam for the
//! future `PermissionMapperRegistry` (feat-051) which will dispatch
//! by `RuntimeKind`; today there is no other mapper, so non-matching
//! runtimes see no flags. HTTP runtimes are unaffected — they never
//! spawn a subprocess.

// Public surface for `ClaudeCodeCodingAgent` (feat-051) — see the
// cli_runner.rs:58 precedent for the rationale.
#![allow(dead_code)]

use std::collections::BTreeMap;

use serde_json::json;

use crate::agent::RuntimeKind;

use super::{PermissionMapper, PermissionSnapshot, ToolProfile};

/// Env var name under which the allowlist is communicated to the
/// child process. The journey translator (feat-048) reads this
/// variable from the child's environment to attribute tool calls.
const TOOL_ALLOWLIST_ENV: &str = "WEAVE_TOOL_ALLOWLIST";

/// Maps Weave profiles to Claude Code permission-mode values.
struct ClaudeCodeModeMap;

impl ClaudeCodeModeMap {
    fn mode_for(profile: ToolProfile) -> &'static str {
        match profile {
            // Most permissive: skip all permission checks. Matches
            // the "I trust the model completely" intent of the
            // `full` profile.
            ToolProfile::Full => "bypassPermissions",
            // Auto-accept file edits; other tools still prompt.
            // Matches the "I want to ship code, not click
            // Approve" intent of the `implementation` profile.
            ToolProfile::Implementation => "acceptEdits",
            // Read-only / planning mode: tools that modify state
            // are denied at the CLI layer. Matches the
            // "review before commit" intent of `review` and the
            // "design first" intent of `planning`.
            ToolProfile::Review | ToolProfile::Planning => "plan",
            // Standard permission mode: tools prompt unless the
            // user has pre-approved. The allowlist is empty
            // (no shell) so the model can read/write files but
            // cannot run shell commands.
            ToolProfile::Reporting => "default",
        }
    }

    fn allowlist_for(profile: ToolProfile) -> &'static [&'static str] {
        match profile {
            // Full: every Weave tool is fair game.
            ToolProfile::Full => &["fs_read", "fs_write", "shell_exec"],
            // Implementation: same tool surface as `full`; the
            // permission-mode flag (acceptEdits) changes only how
            // the CLI prompts for edits.
            ToolProfile::Implementation => &["fs_read", "fs_write", "shell_exec"],
            // Review / Planning: read-only. The Weave server
            // enforces this independently via the FS-tool
            // containment boundary (feat-062) and the
            // permission-mode flag mirrors it at the CLI layer.
            ToolProfile::Review | ToolProfile::Planning => &["fs_read"],
            // Reporting: read + write but no shell.
            ToolProfile::Reporting => &[],
        }
    }
}

/// The Claude Code `PermissionMapper`. Stateless and `Clone`-able;
/// construct once per process and share across turns (the
/// `PermissionMapperRegistry` in feat-051 will hand out `Arc<Self>`).
#[derive(Debug, Clone, Default)]
pub struct ClaudeCodePermissionMapper;

impl ClaudeCodePermissionMapper {
    /// Build a new mapper. Trivial today but kept as a constructor
    /// (rather than exposing `Self` directly) so future config —
    /// e.g. a per-workspace allowlist extension loaded from the DB —
    /// can be threaded in without breaking callers.
    pub fn new() -> Self {
        Self
    }
}

impl PermissionMapper for ClaudeCodePermissionMapper {
    fn effective_permissions(
        &self,
        runtime_kind: RuntimeKind,
        tool_profile: ToolProfile,
    ) -> PermissionSnapshot {
        // The trait is the seam for a future registry of mappers.
        // Today there is only the Claude Code mapper; for any
        // other runtime, return an empty snapshot so the runner
        // gets a well-defined "no flags" answer instead of an
        // error. The HTTP runtimes never reach this method (the
        // runner is only invoked for `mode=wrapped` sessions), so
        // the empty path is the Codex / OpenCode placeholder.
        if runtime_kind != RuntimeKind::ClaudeCode {
            return PermissionSnapshot::empty(runtime_kind, tool_profile);
        }

        let mode = ClaudeCodeModeMap::mode_for(tool_profile);
        let allowlist = ClaudeCodeModeMap::allowlist_for(tool_profile);

        // Serialize the allowlist to a JSON env var so the
        // journey translator (feat-048) can read it back. We use
        // a JSON array of strings — the simplest shape that
        // survives a round-trip through the env without quoting
        // headaches. The allowlist is `&'static [&'static str]`,
        // so the JSON serialization is allocation-free modulo
        // the String the env var will eventually own.
        let allowlist_json = json!(allowlist).to_string();

        let mut env_vars = BTreeMap::new();
        env_vars.insert(TOOL_ALLOWLIST_ENV.to_string(), allowlist_json);

        PermissionSnapshot {
            runtime_kind,
            tool_profile,
            // `--permission-mode` is a two-token argv pair
            // (`--flag value`). The runner concatenates these
            // onto `CliInvocation::args` in order, so the
            // existing `--flag value` parsing on the Claude Code
            // side picks them up unchanged.
            argv_flags: vec!["--permission-mode".to_string(), mode.to_string()],
            env_vars,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Full profile ----

    #[test]
    fn test_permission_mapper_claude_code_full() {
        let mapper = ClaudeCodePermissionMapper::new();
        let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, ToolProfile::Full);

        assert_eq!(snap.runtime_kind, RuntimeKind::ClaudeCode);
        assert_eq!(snap.tool_profile, ToolProfile::Full);
        assert_eq!(
            snap.argv_flags,
            vec![
                "--permission-mode".to_string(),
                "bypassPermissions".to_string()
            ],
        );
        // Allowlist must contain the three Weave tools.
        let allowlist: Vec<String> =
            serde_json::from_str(&snap.env_vars["WEAVE_TOOL_ALLOWLIST"]).unwrap();
        assert_eq!(
            allowlist,
            vec![
                "fs_read".to_string(),
                "fs_write".to_string(),
                "shell_exec".to_string()
            ],
        );
    }

    // ---- Implementation profile ----

    #[test]
    fn test_permission_mapper_claude_code_implementation() {
        let mapper = ClaudeCodePermissionMapper::new();
        let snap =
            mapper.effective_permissions(RuntimeKind::ClaudeCode, ToolProfile::Implementation);

        assert_eq!(snap.tool_profile, ToolProfile::Implementation);
        assert_eq!(
            snap.argv_flags,
            vec!["--permission-mode".to_string(), "acceptEdits".to_string()],
        );
        let allowlist: Vec<String> =
            serde_json::from_str(&snap.env_vars["WEAVE_TOOL_ALLOWLIST"]).unwrap();
        assert_eq!(
            allowlist,
            vec![
                "fs_read".to_string(),
                "fs_write".to_string(),
                "shell_exec".to_string()
            ],
        );
    }

    // ---- Review profile ----

    #[test]
    fn test_permission_mapper_claude_code_review() {
        let mapper = ClaudeCodePermissionMapper::new();
        let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, ToolProfile::Review);

        assert_eq!(snap.tool_profile, ToolProfile::Review);
        assert_eq!(
            snap.argv_flags,
            vec!["--permission-mode".to_string(), "plan".to_string()],
        );
        // Read-only allowlist — no shell, no writes.
        let allowlist: Vec<String> =
            serde_json::from_str(&snap.env_vars["WEAVE_TOOL_ALLOWLIST"]).unwrap();
        assert_eq!(allowlist, vec!["fs_read".to_string()]);
    }

    // ---- Planning profile ----

    #[test]
    fn test_permission_mapper_claude_code_planning() {
        let mapper = ClaudeCodePermissionMapper::new();
        let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, ToolProfile::Planning);

        assert_eq!(snap.tool_profile, ToolProfile::Planning);
        // Planning shares `plan` with Review (same intent: no
        // state mutation), but is its own profile for the UI to
        // label the session.
        assert_eq!(
            snap.argv_flags,
            vec!["--permission-mode".to_string(), "plan".to_string()],
        );
        let allowlist: Vec<String> =
            serde_json::from_str(&snap.env_vars["WEAVE_TOOL_ALLOWLIST"]).unwrap();
        assert_eq!(allowlist, vec!["fs_read".to_string()]);
    }

    // ---- Reporting profile ----

    #[test]
    fn test_permission_mapper_claude_code_reporting() {
        let mapper = ClaudeCodePermissionMapper::new();
        let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, ToolProfile::Reporting);

        assert_eq!(snap.tool_profile, ToolProfile::Reporting);
        assert_eq!(
            snap.argv_flags,
            vec!["--permission-mode".to_string(), "default".to_string()],
        );
        // No shell: empty allowlist per the feat-046 spec.
        let allowlist: Vec<String> =
            serde_json::from_str(&snap.env_vars["WEAVE_TOOL_ALLOWLIST"]).unwrap();
        assert!(
            allowlist.is_empty(),
            "reporting must have an empty allowlist (no shell), got: {allowlist:?}",
        );
    }

    // ---- Non-Claude-Code runtimes ----

    #[test]
    fn test_permission_mapper_non_claude_code_runtime_is_empty() {
        let mapper = ClaudeCodePermissionMapper::new();
        // HTTP runtimes never reach the runner; this is the
        // documented "empty snapshot" placeholder for the
        // future registry.
        for runtime in [
            RuntimeKind::AnthropicApi,
            RuntimeKind::OpenaiApi,
            RuntimeKind::OpenaiCompatible,
            RuntimeKind::Codex,
            RuntimeKind::Opencode,
        ] {
            let snap = mapper.effective_permissions(runtime, ToolProfile::Full);
            assert_eq!(snap.runtime_kind, runtime);
            assert_eq!(snap.tool_profile, ToolProfile::Full);
            assert!(
                snap.argv_flags.is_empty(),
                "non-claude-code runtime {runtime} must produce empty argv_flags, got: {:?}",
                snap.argv_flags,
            );
            assert!(
                snap.env_vars.is_empty(),
                "non-claude-code runtime {runtime} must produce empty env_vars, got: {:?}",
                snap.env_vars,
            );
        }
    }

    // ---- Argv / env shape stability ----

    #[test]
    fn test_permission_mapper_argv_always_two_tokens() {
        // `--permission-mode` is documented as a two-token argv
        // pair in Claude Code's CLI. Pin that shape so a future
        // refactor that accidentally inlines the value (e.g.
        // `--permission-mode=plan`) fails loudly.
        let mapper = ClaudeCodePermissionMapper::new();
        for profile in [
            ToolProfile::Full,
            ToolProfile::Implementation,
            ToolProfile::Review,
            ToolProfile::Planning,
            ToolProfile::Reporting,
        ] {
            let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, profile);
            assert_eq!(
                snap.argv_flags.len(),
                2,
                "{profile} must produce a 2-token argv pair, got: {:?}",
                snap.argv_flags,
            );
            assert_eq!(snap.argv_flags[0], "--permission-mode");
            assert!(
                !snap.argv_flags[1].is_empty(),
                "{profile} must have a non-empty mode value",
            );
        }
    }

    #[test]
    fn test_permission_mapper_allowlist_env_is_valid_json() {
        // The env var is consumed by the journey translator
        // (feat-048) and possibly by tests. Pin that it's valid
        // JSON for every profile, including the empty-array case.
        let mapper = ClaudeCodePermissionMapper::new();
        for profile in [
            ToolProfile::Full,
            ToolProfile::Implementation,
            ToolProfile::Review,
            ToolProfile::Planning,
            ToolProfile::Reporting,
        ] {
            let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, profile);
            let raw = snap
                .env_vars
                .get(TOOL_ALLOWLIST_ENV)
                .expect("env var must be set for Claude Code");
            let parsed: serde_json::Value =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("{profile}: {e}"));
            assert!(
                parsed.is_array(),
                "{profile} env var must be a JSON array, got: {parsed}",
            );
        }
    }

    #[test]
    fn test_permission_mapper_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ClaudeCodePermissionMapper>();
    }
}
