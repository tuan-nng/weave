//! Pure formatting for the rich kanban auto-spawn prompt (feat-063).
//!
//! See [`super::kanban_prompt_ctx`] for the store-IO that assembles
//! the input [`KanbanPromptContext`]. This module owns the prompt
//! structure: section headers, formatting, and the pure-function
//! tests that exercise them with hand-built fixtures (no DB needed).
//!
//! ## Section layout (12 slots)
//!
//! | #  | Header                  | When emitted                |
//! |----|-------------------------|-----------------------------|
//! |  1 | `## Assignment`          | always                      |
//! |  2 | `## Context`             | always                      |
//! |  3 | `## Task Details`        | always                      |
//! |  4 | `## Objective`           | always                      |
//! |  5 | `## Story Readiness`     | only when column = Backlog  |
//! |  6 | `## Artifact Gates`      | always                      |
//! |  7 | `## Delivery Gates`      | OMITTED in v1 (feat-066)    |
//! |  8 | `## Contract`            | only when column = Backlog  |
//! |  9 | `## Lane History`        | always                      |
//! | 10 | `## Lane Handoff Context`| always                      |
//! | 11 | `## Available Tools`     | always                      |
//! | 12 | `## Instructions`        | always                      |
//!
//! The spec's "10 sections" count is a header + 9 others. With
//! Delivery omitted in v1 the actual rendered surface is 11 sections
//! in the backlog case and 9 sections elsewhere (sections 5 and 8
//! are backlog-only).
//!
//! ## Lifetime note
//!
//! The spec's "borrowed from the caller" wording for the context
//! fields forced a `'static` return from the async assembler, which
//! is an anti-pattern (it requires `Box::leak` or stack-local
//! references). The fields are owned here so the assembler can
//! produce a fully-owned value and the formatter can consume it
//! by value. `Task`, `Column`, `Board` are all `#[derive(Clone)]`,
//! so the clone cost is one allocation per kanban auto-spawn.

use std::collections::HashSet;
use std::fmt::Write as _;

use crate::store::boards::Board;
use crate::store::columns::{Column, ColumnStage};
use crate::store::tasks::Task;

/// One row of Section 9 (Lane History): a peer task in the same
/// column that already has a session bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneSession {
    pub task_title: String,
    pub session_id: String,
    pub session_role: String,
    pub session_status: String,
    pub started_at: String,
}

/// All inputs [`build_kanban_prompt`] needs to render the 12-slot
/// prompt. See module doc for why the fields are owned.
#[derive(Debug)]
pub struct KanbanPromptContext {
    pub task: Task,
    pub column: Column,
    pub board: Board,
    /// Up to 5 peer tasks in the same column whose `session_id` is
    /// set. Each row pre-resolves the `Session` row so the formatter
    /// does not re-query. Empty when the lane is solo.
    pub lane_sessions: Vec<LaneSession>,
    /// Set of artifact types already attached to the task (computed
    /// by the assembler via `ArtifactStore::list_types_for_task`).
    pub present_artifact_types: HashSet<String>,
    /// `true` when no other task in this column has a `session_id`
    /// (i.e., this card is the first to start a session in its lane).
    /// Affects Section 4 (Objective) language ("you are the first
    /// card in this lane to start a session"). The name is
    /// `is_first_active_run` rather than `is_first_lane_run` because
    /// there may be other cards in the column without sessions —
    /// the card is first *to fire*, not first *to exist*.
    pub is_first_active_run: bool,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Render the 12-slot prompt for an auto-spawned session.
///
/// Pure function over `ctx`. No IO, no logging, no `AppState`
/// reference. Sections are concatenated in fixed order; each section
/// is preceded by a blank line and a `## Header` line. Sections that
/// are conditional (5, 8) or omitted (7) skip their header line so
/// the prompt does not contain empty stubs.
pub fn build_kanban_prompt(ctx: KanbanPromptContext) -> String {
    let mut out = String::with_capacity(2048);
    // First section's header has no leading blank line (the prompt
    // starts with `## Assignment`, not `\n## Assignment`).
    render_section_1_assignment(&ctx, &mut out);
    render_section_2_context(&ctx, &mut out);
    render_section_3_task_details(&ctx, &mut out);
    render_section_4_objective(&ctx, &mut out);
    render_section_5_story_readiness(&ctx, &mut out);
    render_section_6_artifact_gates(&ctx, &mut out);
    render_section_7_delivery(&ctx, &mut out);
    render_section_8_contract(&ctx, &mut out);
    render_section_9_lane_history(&ctx, &mut out);
    render_section_10_lane_handoff(&ctx, &mut out);
    render_section_11_available_tools(&ctx, &mut out);
    render_section_12_instructions(&ctx, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Section helpers (private)
// ---------------------------------------------------------------------------

/// Maximum number of `task.description` characters carried into
/// Section 4 (Objective). `task.description` is unbounded at the API
/// layer (`api/kanban.rs:422-430` validates only `title`), so the
/// prompt-builder must cap to keep the agent's context window safe.
/// 8 KB is enough for a multi-paragraph story; longer descriptions
/// are truncated with a marker so the agent knows to fetch the full
/// body via the card tools.
const MAX_DESCRIPTION_CHARS_IN_PROMPT: usize = 8_192;

fn write_section_header(out: &mut String, header: &str) {
    // NB: use `push_str` not `writeln!` — `writeln!` appends an
    // extra `\n`, so two newlines would follow the header and
    // section bodies would render with a leading blank line.
    out.push_str("\n## ");
    out.push_str(header);
    out.push('\n');
}

/// Escape a string for safe use inside a Markdown table cell: replace
/// `|` with `\|` and newlines with spaces. Used by Section 9 (Lane
/// History) where a peer task's title could otherwise corrupt the
/// table layout.
fn escape_table_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn is_backlog_column(column: &Column) -> bool {
    column.stage == ColumnStage::Backlog
}

fn render_section_1_assignment(ctx: &KanbanPromptContext, out: &mut String) {
    // First section — no leading blank line so the prompt starts at
    // `## Assignment` (Section 1 is the first non-blank line).
    out.push_str("## Assignment\n");
    let _ = writeln!(out, "You are assigned to Kanban task: {}", ctx.task.title);
}

fn render_section_2_context(ctx: &KanbanPromptContext, out: &mut String) {
    write_section_header(out, "Context");
    let _ = writeln!(
        out,
        "Lane: {} on board {} (id {}).\nUse tools to manage this card.",
        ctx.column.name, ctx.board.name, ctx.board.id
    );
}

fn render_section_3_task_details(ctx: &KanbanPromptContext, out: &mut String) {
    write_section_header(out, "Task Details");
    let _ = writeln!(out, "- Card ID: {}", ctx.task.id);
    let _ = writeln!(out, "- Board: {} (id {})", ctx.board.name, ctx.board.id);
    let _ = writeln!(
        out,
        "- Current Column: {} (id {})",
        ctx.column.name, ctx.column.id
    );
    let _ = writeln!(out, "- Status: {}", ctx.task.status);
    let _ = writeln!(
        out,
        "- Session: {}",
        ctx.task.session_id.as_deref().unwrap_or("(unbound)")
    );
    // `priority`, `labels`, `github_issue` are not yet on `Task`
    // (`store/tasks.rs:35-49`). Document the deferral inline so the
    // agent does not silently assume they exist.
    let _ = writeln!(out, "- (priority/labels/issue fields not yet in schema)");
}

fn render_section_4_objective(ctx: &KanbanPromptContext, out: &mut String) {
    write_section_header(out, "Objective");
    if ctx.is_first_active_run {
        out.push_str("You are the first card in this lane to start a session.\n\n");
    }
    match ctx.task.description.as_deref() {
        Some(desc) if desc.chars().count() <= MAX_DESCRIPTION_CHARS_IN_PROMPT => {
            out.push_str(desc);
        }
        Some(desc) => {
            // Truncate by char count (not byte count) to avoid
            // splitting a multi-byte UTF-8 sequence. The agent can
            // call `update_card` or `provide_artifact` to see the
            // full description.
            let truncated: String = desc.chars().take(MAX_DESCRIPTION_CHARS_IN_PROMPT).collect();
            out.push_str(&truncated);
            out.push_str("\n... [description truncated at ");
            let _ = write!(out, "{MAX_DESCRIPTION_CHARS_IN_PROMPT}");
            out.push_str(" chars; fetch full body via the card tools]");
        }
        None => out.push_str("(no description set)"),
    }
    out.push('\n');
}

fn render_section_5_story_readiness(ctx: &KanbanPromptContext, out: &mut String) {
    // Backlog-only in v1. feat-065 will add `Column.stage` and the
    // trigger becomes `stage == Backlog` directly.
    if !is_backlog_column(&ctx.column) {
        return;
    }
    write_section_header(out, "Story Readiness");
    // The three optional string fields on `Task` are the only signals
    // the v1 schema exposes; a future feature will add dedicated
    // columns for `verification_commands` and `test_cases`.
    let ac = presence(&ctx.task.acceptance_criteria);
    let vp = presence(&ctx.task.completion_summary);
    let tc = presence(&ctx.task.verification_report);
    let _ = writeln!(out, "- Acceptance Criteria: {ac}");
    let _ = writeln!(out, "- Verification Plan: {vp}");
    let _ = writeln!(out, "- Acceptance Test Cases: {tc}");
}

/// `"present"` when the optional string is non-blank, otherwise
/// `"MISSING — required to leave this lane"`.
fn presence(s: &Option<String>) -> &'static str {
    match s.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(_) => "present",
        None => "MISSING — required to leave this lane",
    }
}

fn render_section_6_artifact_gates(ctx: &KanbanPromptContext, out: &mut String) {
    write_section_header(out, "Artifact Gates");
    if ctx.column.required_artifact_types.is_empty() {
        let _ = writeln!(out, "No artifact gates on this lane.");
        return;
    }
    for required in &ctx.column.required_artifact_types {
        let status = if ctx.present_artifact_types.contains(required) {
            "present"
        } else {
            "MISSING — call provide_artifact to attach"
        };
        let _ = writeln!(out, "- {required}: {status}");
    }
}

fn render_section_7_delivery(_ctx: &KanbanPromptContext, _out: &mut String) {
    // OMITTED in v1. feat-066 adds `Column.automation.delivery_rules`
    // (committed-changes check, clean-worktree check, etc.) and this
    // section will render the configured checks. The slot exists so
    // the per-section numbering matches the spec; the agent never
    // sees a `## Delivery Gates` header today.
}

fn render_section_8_contract(ctx: &KanbanPromptContext, out: &mut String) {
    // Contract is a Backlog-exit concern. v1 free-text scans
    // `task.description` for a ```yaml code block; feat-066's
    // `contract_rules` automation will switch this to a parse.
    if !is_backlog_column(&ctx.column) {
        return;
    }
    write_section_header(out, "Contract");
    let has_block = ctx
        .task
        .description
        .as_deref()
        .is_some_and(|d| d.contains("```yaml"));
    if has_block {
        let _ = writeln!(
            out,
            "Canonical story block: present (yaml code block found in description)"
        );
    } else {
        let _ = writeln!(
            out,
            "Canonical story block: MISSING — task.description must contain a ```yaml code block before leaving Backlog"
        );
    }
}

fn render_section_9_lane_history(ctx: &KanbanPromptContext, out: &mut String) {
    write_section_header(out, "Lane History");
    if ctx.lane_sessions.is_empty() {
        let _ = writeln!(out, "No other cards in this lane yet.");
        return;
    }
    let _ = writeln!(out, "| Task | Role | Status | Started |");
    let _ = writeln!(out, "| --- | --- | --- | --- |");
    for s in &ctx.lane_sessions {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            escape_table_cell(&s.task_title),
            escape_table_cell(&s.session_role),
            escape_table_cell(&s.session_status),
            escape_table_cell(&s.started_at),
        );
    }
}

fn render_section_10_lane_handoff(ctx: &KanbanPromptContext, out: &mut String) {
    write_section_header(out, "Lane Handoff Context");
    match ctx.task.session_id.as_deref() {
        Some(sid) => {
            let _ = writeln!(out, "Previous session on this card: {sid}");
            let _ = writeln!(
                out,
                "(status / handoff payload: see Lane History above; feat-066 will land a typed handoff blob.)"
            );
        }
        None => {
            let _ = writeln!(out, "No prior session on this card.");
        }
    }
}

fn render_section_11_available_tools(ctx: &KanbanPromptContext, out: &mut String) {
    write_section_header(out, "Available Tools");
    match ctx.column.stage {
        ColumnStage::Backlog => {
            let _ = writeln!(
                out,
                "- `update_card` — change title, description, status, fields"
            );
            let _ = writeln!(out, "- `update_task` — change the task body");
            let _ = writeln!(
                out,
                "- `move_card` — advance the card to the next column (Todo or Dev)"
            );
            let _ = writeln!(out, "- `create_note` — leave context for the next lane");
        }
        ColumnStage::Todo => {
            let _ = writeln!(
                out,
                "- `update_card` — change title, description, status, fields"
            );
            let _ = writeln!(out, "- `update_task` — change the task body");
            let _ = writeln!(
                out,
                "- `move_card` — advance the card to the next column (Dev)"
            );
            let _ = writeln!(out, "- `create_note` — leave context for the next lane");
        }
        ColumnStage::Dev => {
            let _ = writeln!(
                out,
                "- `update_card` — change title, description, status, fields"
            );
            let _ = writeln!(out, "- `update_task` — change the task body");
            let _ = writeln!(
                out,
                "- `move_card` — advance the card to the next column (Review)"
            );
            let _ = writeln!(out, "- `list_artifacts` — see what evidence is attached");
            let _ = writeln!(out, "- `provide_artifact` — attach new evidence");
            let _ = writeln!(out, "- `create_note` — leave context for the next lane");
        }
        ColumnStage::Review => {
            let _ = writeln!(
                out,
                "- `update_card` — change title, description, status, fields"
            );
            let _ = writeln!(out, "- `update_task` — change the task body");
            let _ = writeln!(
                out,
                "- `move_card` — advance the card to the next column (Done)"
            );
            let _ = writeln!(out, "- `list_artifacts` — see what evidence is attached");
            let _ = writeln!(out, "- `provide_artifact` — attach new evidence");
            let _ = writeln!(out, "- `create_note` — leave context for the next lane");
        }
        ColumnStage::Done => {
            let _ = writeln!(out, "- `list_artifacts` — see what evidence is attached");
            let _ = writeln!(out, "- `create_note` — leave context for the next lane");
        }
    }
}

fn render_section_12_instructions(ctx: &KanbanPromptContext, out: &mut String) {
    write_section_header(out, "Instructions");
    match ctx.column.stage {
        ColumnStage::Backlog => {
            let _ = writeln!(out, "1. Read your Objective carefully.");
            let _ = writeln!(
                out,
                "2. Check Story Readiness (Section 5) — if any item is MISSING, fix it before proceeding."
            );
            let _ = writeln!(
                out,
                "3. Check Artifact Gates (Section 6) — for each MISSING type, call provide_artifact to attach the required evidence."
            );
            let _ = writeln!(
                out,
                "4. When your work is complete, call move_card to advance to the next column (Todo or Dev)."
            );
            let _ = writeln!(
                out,
                "5. If you are blocked, call update_card with a comment explaining the blocker — do not advance the card without resolution."
            );
            let _ = writeln!(
                out,
                "6. Refer to your specialist system prompt for role-specific behavior — sections 1-11 above provide task-specific context only."
            );
        }
        ColumnStage::Todo => {
            let _ = writeln!(out, "1. Read your Objective carefully.");
            let _ = writeln!(
                out,
                "2. Break the task into actionable sub-steps if it is complex."
            );
            let _ = writeln!(
                out,
                "3. Check Artifact Gates (Section 6) — for each MISSING type, call provide_artifact to attach the required evidence."
            );
            let _ = writeln!(
                out,
                "4. When planning is complete, call move_card to advance to Dev."
            );
            let _ = writeln!(
                out,
                "5. If you are blocked, call update_card with a comment explaining the blocker."
            );
            let _ = writeln!(
                out,
                "6. Refer to your specialist system prompt for role-specific behavior."
            );
        }
        ColumnStage::Dev => {
            let _ = writeln!(out, "1. Read your Objective carefully.");
            let _ = writeln!(out, "2. Implement the changes described in the Objective.");
            let _ = writeln!(
                out,
                "3. Check Artifact Gates (Section 6) — for each MISSING type, call provide_artifact to attach the required evidence."
            );
            let _ = writeln!(
                out,
                "4. When your work is complete, call move_card to advance to Review."
            );
            let _ = writeln!(
                out,
                "5. If you are blocked, call update_card with a comment explaining the blocker — do not advance the card without resolution."
            );
            let _ = writeln!(
                out,
                "6. Refer to your specialist system prompt for role-specific behavior."
            );
        }
        ColumnStage::Review => {
            let _ = writeln!(
                out,
                "1. Read the Objective and verify the implementation matches."
            );
            let _ = writeln!(
                out,
                "2. Check Artifact Gates (Section 6) — review all attached evidence."
            );
            let _ = writeln!(
                out,
                "3. If changes are needed, call update_task with verification_report noting the issues."
            );
            let _ = writeln!(
                out,
                "4. When review is complete, call move_card to advance to Done."
            );
            let _ = writeln!(
                out,
                "5. If the work is not ready, call update_card with a comment explaining what needs fixing."
            );
            let _ = writeln!(
                out,
                "6. Refer to your specialist system prompt for role-specific behavior."
            );
        }
        ColumnStage::Done => {
            let _ = writeln!(
                out,
                "1. This card is complete. No further action is needed."
            );
            let _ = writeln!(
                out,
                "2. Optionally call create_note to leave a summary for future reference."
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — pure, no DB.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn task_with(title: &str, description: Option<&str>) -> Task {
        Task {
            id: "task-1".into(),
            board_id: "board-1".into(),
            column_id: "col-1".into(),
            title: title.into(),
            description: description.map(String::from),
            position: 0,
            status: "active".into(),
            session_id: None,
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
            priority: None,
            labels: None,
            scope: None,
            verification_commands: None,
            test_cases: None,
            created_at: "2026-06-12T00:00:00Z".into(),
            updated_at: "2026-06-12T00:00:00Z".into(),
        }
    }

    fn column_with(name: &str) -> Column {
        Column {
            id: "col-1".into(),
            board_id: "board-1".into(),
            name: name.into(),
            position: 0,
            specialist_id: Some("dev".into()),
            auto_trigger: true,
            freeze_description: false,
            required_fields: vec![],
            required_artifact_types: vec![],
            runtime_kind: None,
            stage: ColumnStage::Dev,
            created_at: "2026-06-12T00:00:00Z".into(),
        }
    }

    fn board_fixture() -> Board {
        Board {
            id: "board-1".into(),
            workspace_id: "ws-1".into(),
            name: "Platform".into(),
            created_at: "2026-06-12T00:00:00Z".into(),
        }
    }

    fn ctx_with(
        title: &str,
        description: Option<&str>,
        column_name: &str,
        required_artifact_types: Vec<&str>,
        present: Vec<&str>,
    ) -> KanbanPromptContext {
        let mut column = column_with(column_name);
        // Set stage based on column name for tests.
        column.stage = match column_name.to_ascii_lowercase().as_str() {
            "backlog" => ColumnStage::Backlog,
            "to do" | "todo" => ColumnStage::Todo,
            "in progress" | "dev" => ColumnStage::Dev,
            "review" => ColumnStage::Review,
            "done" => ColumnStage::Done,
            _ => ColumnStage::Dev,
        };
        column.required_artifact_types = required_artifact_types
            .into_iter()
            .map(String::from)
            .collect();
        let mut task = task_with(title, description);
        if column.stage == ColumnStage::Backlog {
            task.acceptance_criteria = Some("- a\n- b".into());
        }
        KanbanPromptContext {
            task,
            column,
            board: board_fixture(),
            lane_sessions: vec![],
            present_artifact_types: present.into_iter().map(String::from).collect(),
            is_first_active_run: true,
        }
    }

    #[test]
    fn test_section_1_assignment_includes_title() {
        let ctx = ctx_with("Refine auth story", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        assert!(
            out.contains("## Assignment"),
            "missing Assignment header: {out}"
        );
        assert!(
            out.contains("You are assigned to Kanban task: Refine auth story"),
            "missing title line: {out}"
        );
        // Section 1 is the first non-blank line.
        let first = out.trim_start().lines().next().unwrap();
        assert!(
            first.starts_with("## Assignment"),
            "first line was: {first}"
        );
    }

    #[test]
    fn test_section_2_context_includes_lane_and_board() {
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        assert!(out.contains("## Context"));
        assert!(out.contains("Lane: Backlog"));
        assert!(out.contains("on board Platform"));
        assert!(out.contains("(id board-1)"));
    }

    #[test]
    fn test_section_3_task_details_emits_required_fields() {
        let ctx = ctx_with("T", Some("body"), "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        assert!(out.contains("## Task Details"));
        assert!(out.contains("Card ID: task-1"));
        assert!(out.contains("Board: Platform (id board-1)"));
        assert!(out.contains("Current Column: Backlog (id col-1)"));
        assert!(out.contains("Status: active"));
        assert!(out.contains("Session: (unbound)"));
        // Deferral marker.
        assert!(out.contains("priority/labels/issue fields not yet in schema"));
    }

    #[test]
    fn test_section_3_emits_prior_session_when_bound() {
        let mut ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        ctx.task.session_id = Some("s-old".into());
        let out = build_kanban_prompt(ctx);
        assert!(out.contains("Session: s-old"));
        assert!(!out.contains("Session: (unbound)"));
    }

    #[test]
    fn test_section_4_uses_description_when_present() {
        let ctx = ctx_with("T", Some("do the thing"), "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Objective").unwrap();
        assert!(body.contains("do the thing"), "body: {body}");
    }

    #[test]
    fn test_section_4_falls_back_to_no_description_message() {
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Objective").unwrap();
        assert!(body.contains("(no description set)"), "body: {body}");
    }

    #[test]
    fn test_section_4_first_active_run_prefix() {
        let ctx = ctx_with("T", Some("body"), "Backlog", vec![], vec![]);
        // ctx_with sets is_first_active_run = true.
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Objective").unwrap();
        assert!(
            body.starts_with("You are the first card in this lane to start a session."),
            "body: {body}"
        );
    }

    #[test]
    fn test_section_4_prefix_absent_when_is_first_active_run_false() {
        let mut ctx = ctx_with("T", Some("body"), "Backlog", vec![], vec![]);
        ctx.is_first_active_run = false;
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Objective").unwrap();
        assert!(
            !body.starts_with("You are the first card"),
            "prefix should not appear: {body}"
        );
    }

    #[test]
    fn test_section_5_emitted_for_backlog_with_present_fields() {
        let mut ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        ctx.task.acceptance_criteria = Some("- item".into());
        ctx.task.completion_summary = Some("verify by running tests".into());
        ctx.task.verification_report = Some("done".into());
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Story Readiness").unwrap();
        assert!(
            body.contains("Acceptance Criteria: present"),
            "body: {body}"
        );
        assert!(body.contains("Verification Plan: present"), "body: {body}");
        assert!(
            body.contains("Acceptance Test Cases: present"),
            "body: {body}"
        );
    }

    #[test]
    fn test_section_5_emitted_for_backlog_with_missing_fields() {
        // ctx_with leaves the three optional fields None; only AC
        // is set (and the seed leaves VP / TC None).
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Story Readiness").unwrap();
        assert!(
            body.contains("Acceptance Criteria: present"),
            "body: {body}"
        );
        assert!(body.contains("Verification Plan: MISSING"), "body: {body}");
        assert!(
            body.contains("Acceptance Test Cases: MISSING"),
            "body: {body}"
        );
    }

    #[test]
    fn test_section_5_skipped_for_non_backlog_column() {
        let ctx = ctx_with("T", None, "In Progress", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        assert!(
            !out.contains("## Story Readiness"),
            "should be omitted: {out}"
        );
    }

    #[test]
    fn test_section_6_lists_required_artifact_types_with_status() {
        let ctx = ctx_with(
            "T",
            None,
            "Backlog",
            vec!["test_results", "screenshot"],
            vec!["test_results"],
        );
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Artifact Gates").unwrap();
        assert!(body.contains("test_results: present"), "body: {body}");
        assert!(body.contains("screenshot: MISSING"), "body: {body}");
        assert!(
            body.contains("provide_artifact"),
            "must point at remediation tool: {body}"
        );
    }

    #[test]
    fn test_section_6_empty_required_emits_no_gates_message() {
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Artifact Gates").unwrap();
        assert!(
            body.contains("No artifact gates on this lane."),
            "body: {body}"
        );
    }

    #[test]
    fn test_section_7_delivery_omitted_in_v1() {
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        assert!(
            !out.contains("## Delivery Gates"),
            "Delivery section must not appear in v1: {out}"
        );
    }

    #[test]
    fn test_section_8_present_when_yaml_block_in_description() {
        let ctx = ctx_with(
            "T",
            Some("intro\n```yaml\nfoo: bar\n```\n"),
            "Backlog",
            vec![],
            vec![],
        );
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Contract").unwrap();
        assert!(body.contains("present"), "body: {body}");
    }

    #[test]
    fn test_section_8_missing_when_no_yaml_block() {
        let ctx = ctx_with("T", Some("plain text"), "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Contract").unwrap();
        assert!(body.contains("MISSING"), "body: {body}");
    }

    #[test]
    fn test_section_8_skipped_for_non_backlog_column() {
        let ctx = ctx_with(
            "T",
            Some("```yaml\nx: 1\n```"),
            "In Progress",
            vec![],
            vec![],
        );
        let out = build_kanban_prompt(ctx);
        assert!(!out.contains("## Contract"), "should be omitted: {out}");
    }

    #[test]
    fn test_section_9_emits_solo_message_when_lane_empty() {
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Lane History").unwrap();
        assert!(
            body.contains("No other cards in this lane yet."),
            "body: {body}"
        );
    }

    #[test]
    fn test_section_9_emits_table_with_peers() {
        let mut ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        ctx.lane_sessions = vec![
            LaneSession {
                task_title: "Earlier card".into(),
                session_id: "s-1".into(),
                session_role: "dev".into(),
                session_status: "ready".into(),
                started_at: "2026-06-11T00:00:00Z".into(),
            },
            LaneSession {
                task_title: "Even earlier".into(),
                session_id: "s-2".into(),
                session_role: "review-guard".into(),
                session_status: "completed".into(),
                started_at: "2026-06-10T00:00:00Z".into(),
            },
        ];
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Lane History").unwrap();
        assert!(
            body.contains("| Task | Role | Status | Started |"),
            "body: {body}"
        );
        assert!(body.contains("Earlier card"), "body: {body}");
        assert!(body.contains("Even earlier"), "body: {body}");
        assert!(
            !body.contains("No other cards"),
            "should not be solo: {body}"
        );
    }

    #[test]
    fn test_section_10_lane_handoff_with_prior_session() {
        let mut ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        ctx.task.session_id = Some("s-old".into());
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Lane Handoff Context").unwrap();
        assert!(
            body.contains("Previous session on this card: s-old"),
            "body: {body}"
        );
    }

    #[test]
    fn test_section_10_lane_handoff_with_no_prior_session() {
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Lane Handoff Context").unwrap();
        assert!(
            body.contains("No prior session on this card."),
            "body: {body}"
        );
    }

    #[test]
    fn test_section_11_available_tools_lists_kanban_tools() {
        // Use "In Progress" (Dev stage) which lists all 6 tools.
        let ctx = ctx_with("T", None, "In Progress", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Available Tools").unwrap();
        for tool in [
            "update_card",
            "update_task",
            "move_card",
            "list_artifacts",
            "provide_artifact",
            "create_note",
        ] {
            assert!(body.contains(tool), "missing tool {tool} in: {body}");
        }
    }

    #[test]
    fn test_section_11_backlog_omits_artifact_tools() {
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Available Tools").unwrap();
        assert!(
            body.contains("update_card"),
            "backlog should have update_card"
        );
        assert!(body.contains("move_card"), "backlog should have move_card");
        assert!(
            !body.contains("list_artifacts"),
            "backlog should NOT have list_artifacts"
        );
        assert!(
            !body.contains("provide_artifact"),
            "backlog should NOT have provide_artifact"
        );
    }

    #[test]
    fn test_section_11_done_is_read_only() {
        let ctx = ctx_with("T", None, "Done", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Available Tools").unwrap();
        assert!(
            !body.contains("move_card"),
            "done stage should NOT have move_card"
        );
        assert!(
            !body.contains("update_card"),
            "done stage should NOT have update_card"
        );
        assert!(
            body.contains("list_artifacts"),
            "done should have list_artifacts"
        );
    }

    #[test]
    fn test_section_12_instructions_has_six_numbered_steps() {
        let ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Instructions").unwrap();
        // Exact-prefix check (no regex; the count-by-digit
        // heuristic was too permissive — `7 things to consider`
        // would have matched).
        for (idx, expected_prefix) in [
            "1. Read your Objective carefully.",
            "2. Check Story Readiness",
            "3. Check Artifact Gates",
            "4. When your work is complete",
            "5. If you are blocked",
            "6. Refer to your specialist system prompt",
        ]
        .iter()
        .enumerate()
        {
            let n = idx + 1;
            assert!(
                body.contains(expected_prefix),
                "missing step {n} prefix in body: {body}"
            );
        }
    }

    #[test]
    fn test_section_4_truncates_oversized_description_with_marker() {
        // 12 KB of body text — over the 8 KB cap.
        let big = "x".repeat(12_000);
        let ctx = ctx_with("T", Some(&big), "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Objective").unwrap();
        assert!(
            body.contains("[description truncated at"),
            "expected truncation marker in: {body}"
        );
        // The full 12 KB must NOT have leaked in.
        assert!(
            !body.contains(&"x".repeat(12_000)),
            "truncation failed — full body in prompt: len={}",
            body.len()
        );
    }

    #[test]
    fn test_section_4_handles_multibyte_description_at_boundary() {
        // 4-byte UTF-8 char repeated near the cap. The truncation
        // uses `chars().take(N)` so we never split a multi-byte
        // sequence mid-codepoint.
        let s: String = "🦀".repeat(MAX_DESCRIPTION_CHARS_IN_PROMPT + 100);
        let ctx = ctx_with("T", Some(&s), "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Objective").unwrap();
        assert!(
            body.contains("[description truncated at"),
            "truncation marker should appear for oversized body"
        );
        // All visible bytes should still be valid UTF-8 (the
        // prompt is a String — non-UTF-8 would have panicked at
        // construction time).
        assert!(body.is_char_boundary(body.len()));
    }

    #[test]
    fn test_section_9_escapes_pipe_in_task_title() {
        let mut ctx = ctx_with("T", None, "Backlog", vec![], vec![]);
        ctx.lane_sessions = vec![LaneSession {
            task_title: "foo | bar | baz".into(),
            session_id: "s-1".into(),
            session_role: "dev".into(),
            session_status: "ready".into(),
            started_at: "2026-06-11T00:00:00Z".into(),
        }];
        let out = build_kanban_prompt(ctx);
        let body = extract_section(&out, "Lane History").unwrap();
        // The pipe must be escaped, not raw.
        assert!(
            body.contains("foo \\| bar \\| baz"),
            "expected pipe-escaped title in: {body}"
        );
        // And there should be no malformed row with an unescaped
        // pipe (which would add extra cells to the markdown table).
        assert_eq!(
            body.matches(" | ").count(),
            // Header row + separator row + 1 data row = 3 rows ×
            // 3 internal ` | ` separators per row = 9. The
            // escaped `\|` in the title does NOT count because
            // it's `\|` (no spaces around the pipe) — the raw
            // ` | ` pattern requires spaces on both sides.
            9,
            "wrong cell count in lane-history table: {body}"
        );
    }

    #[test]
    fn test_prompt_starts_with_section_1_no_leading_blank_line() {
        let ctx = ctx_with("T", Some("body"), "Backlog", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        assert!(
            out.starts_with("## Assignment"),
            "prompt should start with '## Assignment', got: {out:?}"
        );
    }

    #[test]
    fn test_build_kanban_prompt_sections_in_order_for_backlog() {
        let ctx = ctx_with(
            "T",
            Some("```yaml\nx: 1\n```"),
            "Backlog",
            vec!["log"],
            vec!["log"],
        );
        let out = build_kanban_prompt(ctx);
        // The 11 expected headers in order. Delivery is omitted.
        let expected = [
            "## Assignment",
            "## Context",
            "## Task Details",
            "## Objective",
            "## Story Readiness",
            "## Artifact Gates",
            "## Contract",
            "## Lane History",
            "## Lane Handoff Context",
            "## Available Tools",
            "## Instructions",
        ];
        let mut cursor = 0usize;
        for header in &expected {
            let pos = out[cursor..]
                .find(header)
                .unwrap_or_else(|| panic!("header {header} not found in order: {out}"));
            cursor += pos + header.len();
        }
    }

    #[test]
    fn test_build_kanban_prompt_omits_backlog_sections_for_non_backlog() {
        let ctx = ctx_with("T", Some("body"), "In Progress", vec![], vec![]);
        let out = build_kanban_prompt(ctx);
        // 9 sections (no Story Readiness, no Contract).
        let expected = [
            "## Assignment",
            "## Context",
            "## Task Details",
            "## Objective",
            "## Artifact Gates",
            "## Lane History",
            "## Lane Handoff Context",
            "## Available Tools",
            "## Instructions",
        ];
        for header in &expected {
            assert!(out.contains(header), "missing {header}: {out}");
        }
        assert!(!out.contains("## Story Readiness"));
        assert!(!out.contains("## Contract"));
    }

    /// Extract the body between `## <Header>` and the next `## ` or
    /// end-of-string. Used by the per-section tests to assert on a
    /// single section in isolation.
    fn extract_section(prompt: &str, header: &str) -> Option<String> {
        let start_marker = format!("## {header}");
        let start = prompt.find(&start_marker)? + start_marker.len();
        // Skip the trailing `\n` that follows the header.
        let body_start = prompt[start..].find('\n').map(|i| start + i + 1)?;
        let rest = &prompt[body_start..];
        let end = rest.find("\n## ").unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}
