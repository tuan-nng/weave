//! Claude Code CLI adapter scaffolding (Phase 8).
//!
//! The Claude Code CLI's `stream-json` mode emits one JSON object per
//! line on stdout. This subdir hosts the line-stream parser that
//! converts those lines into the universal [`StreamEvent`] contract
//! used by [`crate::agent::CodingAgent`].
//!
//! ## Phase 8 plan
//!
//! - **feat-045** (this feature): the parser — `parser.rs` + `parser_test.rs`.
//! - **feat-046** (done): the `PermissionMapper` impl — `permissions/`.
//! - **feat-047** (this feature): the resume-id persistence glue —
//!   the `detect_resume_rejection` free function in this file. The
//!   original plan reserved `resume.rs` for feat-047; the Pragmatic
//!   approach keeps the helper here (one function, one caller) and
//!   hoists to a sibling module if a second consumer lands.
//! - feat-048: the journey translator — `journey.rs`.
//! - feat-051: the `ClaudeCodeCodingAgent` `CodingAgent` impl — `agent.rs`.
//!
//! Each feature adds a sibling module under `claude_code/`. This file
//! is the public re-export point for the whole subdir.

mod parser;

#[cfg(test)]
mod parser_test;

// Public surface for feat-051's `ClaudeCodeCodingAgent` impl and any
// future caller. `parser_test` imports via this re-export (so the
// re-export IS used in test builds); the production build has no
// caller yet, so silence the warning there only.
#[cfg_attr(not(test), allow(unused_imports))]
pub use parser::ClaudeCodeStreamParser;

use serde_json::Value;

/// Decide whether the CLI rejected a `--resume <id>` invocation (feat-047).
///
/// Returns `true` if the CLI was asked to resume a known session id and
/// refused. Two independent signals:
///
/// 1. The structured JSON event
///    `{"type":"error","code":"resume_unknown_session"}`
///    (preferred — matches the wire format Claude Code emits on the
///    `--resume` path). The `parsed_error_event` is the most recent
///    `error`-shaped line the runner parsed; pass `None` when no
///    structured error was seen.
/// 2. A substring match on `stderr` for `resume_unknown_session`,
///    `no such session`, or `session not found` — defensive carve for
///    older Claude Code versions and CLI vendors that route the
///    error to stderr instead of stdout. The substring match is
///    biased toward false positives (a recoverable UX issue — the
///    next turn replays) over false negatives (a persistent
///    user-facing error).
///
/// Co-located with the parser that produces the values it inspects.
/// feat-051's `ClaudeCodeCodingAgent` will call this on the captured
/// `CliRunResult` (the most-recent parsed error event + stderr).
/// feat-047 only threads the data path; the runner is feat-051.
#[allow(dead_code)] // Used by tests + the feat-051 runner; production call site lands later.
pub(crate) fn detect_resume_rejection(parsed_error_event: Option<&Value>, stderr: &str) -> bool {
    if let Some(v) = parsed_error_event {
        let ty = v.get("type").and_then(|t| t.as_str());
        let code = v.get("code").and_then(|c| c.as_str());
        if ty == Some("error") && code == Some("resume_unknown_session") {
            return true;
        }
    }
    stderr.contains("resume_unknown_session")
        || stderr.contains("no such session")
        || stderr.contains("session not found")
}

#[cfg(test)]
mod resume_rejection_tests {
    use super::detect_resume_rejection;
    use serde_json::json;

    #[test]
    fn detects_structured_error_event() {
        let v = json!({"type": "error", "code": "resume_unknown_session", "message": "x"});
        assert!(detect_resume_rejection(Some(&v), ""));
    }

    #[test]
    fn detects_stderr_substring() {
        assert!(detect_resume_rejection(
            None,
            "fake_cli: resume_unknown_session: bad id"
        ));
        assert!(detect_resume_rejection(None, "claude: no such session"));
        assert!(detect_resume_rejection(None, "claude: session not found"));
    }

    #[test]
    fn no_rejection_on_clean_turn() {
        let clean = json!({"type": "text_delta", "text": "hello"});
        assert!(!detect_resume_rejection(Some(&clean), "all good"));
        assert!(!detect_resume_rejection(None, ""));
    }

    #[test]
    fn unrelated_error_code_does_not_match() {
        let v = json!({"type": "error", "code": "permission_denied"});
        assert!(!detect_resume_rejection(Some(&v), ""));
    }
}
