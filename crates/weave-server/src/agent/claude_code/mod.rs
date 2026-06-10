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
//! - feat-046: the `PermissionMapper` impl — `permissions.rs`.
//! - feat-047: the resume-id persistence glue — `resume.rs`.
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
