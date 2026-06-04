# tools/ — Agent Tool Implementations

Tools that AI coding agents can invoke during sessions. Each tool implements the `ToolExecutor` trait and is registered in `ToolRegistry` with a profile assignment.

## Core Infrastructure

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 19KB | `ToolExecutor` trait, `ToolRegistry` (register/get/resolve_profile), `ToolContext`, `ToolResult`, `ToolDefinition`, `MockTool` (for tests), 5 built-in tool profiles, helper fns: `spawn_read_task`, `truncate_bytes` |
| `shell.rs` | 12KB | `ShellExecutor` — `shell_exec` tool with timeout, working directory, 100KB output truncation |

## Tool Submodules

Each subdirectory contains a `mod.rs` (register entry point) + one file per tool.

### tools/fs/ (filesystem tools)

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 21KB | `FsToolRegistry` — registers 5 tools + `PathValidator` (prevents path traversal) |
| `read.rs` | 3KB | `fs_read` — read file contents with line range |
| `write.rs` | 5KB | `fs_write` — create/overwrite a file |
| `edit.rs` | 7KB | `fs_edit` — exact string replacement in file |
| `search.rs` | 10KB | `fs_search` — grep-like search with glob patterns |
| `list.rs` | 6KB | `fs_list` — list directory contents |

### tools/git/ (git tools)

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 13KB | `GitToolRegistry` — registers 5 tools |
| `status.rs` | 7KB | `git_status` — working tree status |
| `diff.rs` | 8KB | `git_diff` — staged/unstaged diffs |
| `log.rs` | 7KB | `git_log` — commit history |
| `commit.rs` | 13KB | `git_commit` — commit with identity validation |

### tools/kanban/ (kanban tools)

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 12KB | `KanbanToolRegistry` — registers 4 tools |
| `get_board.rs` | 5KB | `get_board` — read full board state |
| `create_card.rs` | 12KB | `create_card` — create a new kanban task |
| `move_card.rs` | 23KB | `move_card` — move task to another column (with transition gate check) |
| `search_cards.rs` | 9KB | `search_cards` — search tasks by title/description/status |

### tools/task/ (task context tools)

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 440B | `TaskToolRegistry` — registers 3 tools |
| `get.rs` | 5KB | `task_get` — get task by ID |
| `list.rs` | 7KB | `task_list` — list tasks for the current session |
| `update_fields.rs` | 7KB | `task_update_fields` — update task metadata |
| `update_status.rs` | 6KB | `task_update_status` — update task status |

### tools/note/ (note tools)

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 8KB | `NoteToolRegistry` — registers 5 tools |
| `create_note.rs` | 7KB | `note_create` — create a new note |
| `read_note.rs` | 4KB | `note_read` — read note by ID |
| `list_notes.rs` | 5KB | `note_list` — list notes with type filter |
| `set_note_content.rs` | 5KB | `note_set_content` — replace note content |
| `append_to_note.rs` | 5KB | `note_append` — append to note content |

### tools/artifact/ (artifact tools)

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 11KB | `ArtifactToolRegistry` — registers 3 tools |
| `request_artifact.rs` | 8KB | `artifact_request` — request an artifact from another agent |
| `provide_artifact.rs` | 9KB | `artifact_provide` — provide an artifact to a task |
| `list_artifacts.rs` | 7KB | `artifact_list` — list artifacts for a task |

## Key Patterns

- Every tool implements `ToolExecutor` trait: `name()`, `description()`, `input_schema()`, `execute()`
- Each submodule has a `register_all(registry: &mut ToolRegistry)` function
- Tool profiles: "full", "filesystem", "kanban", "task", "note" — registered at startup
- `ToolContext` carries workspace_id, db, trace_collector, and cwd path
- Path validation: `PathValidator` enforces base directory boundaries

## Connections

- **Used by:** `service/sessions.rs` (agent tool loop), tests
- **Depends on:** `store/*` for data access, `agent::ToolDefinition` for schema, `error::AppError`
