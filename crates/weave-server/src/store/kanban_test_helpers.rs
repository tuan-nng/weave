//! Test helpers shared by kanban store + API tests.
//!
//! Builds an in-memory `AppState` and seeds the minimum FK chain
//! (`workspace → board → column`) needed by kanban tests. Used by
//! `store::boards`, `store::columns`, `store::tasks` (the extended
//! tests), and `api::kanban` test modules.
//!
//! Why this lives here and not in `api/test_helpers.rs`: the seed
//! shape is store-shaped, not API-shaped. `api/kanban.rs` consumes
//! the same fixture as the store tests, and the future feat-025
//! `service/kanban.rs` will too.

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use crate::db::Db;
use crate::AppState;

/// Build an in-memory `Db` with migrations applied.
pub fn make_test_db() -> Arc<Db> {
    Arc::new(Db::open(Path::new(":memory:")).expect("failed to open test db"))
}

/// Build an `AppState` with a fresh in-memory database and all other
/// fields set to empty defaults.
pub fn make_test_state() -> AppState {
    let db = make_test_db();
    crate::store::workspaces::WorkspaceStore::ensure_default(&db).expect("ensure_default");
    AppState {
        db,
        registry: Arc::new(crate::agent::registry::ProviderRegistry::new()),
        active_sessions: Arc::new(crate::service::ActiveSessions::new()),
        sse_manager: Arc::new(crate::sse::SseManager::new()),
        specialists: Arc::new(crate::specialist::SpecialistRegistry::new()),
        tools: Arc::new(crate::tools::ToolRegistry::new()),
    }
}

/// Seed the FK chain: workspace → board → one default column.
///
/// Returns `(workspace_id, board_id, column_id)`. The workspace is the
/// "default" workspace from `WorkspaceStore::ensure_default`; the board
/// and column are freshly created.
pub fn seed_workspace_with_board(db: &Db) -> (String, String, String) {
    crate::store::workspaces::WorkspaceStore::ensure_default(db).expect("ensure_default");

    // Look up the default workspace id.
    let workspace_id: String = db
        .conn()
        .query_row(
            "SELECT id FROM workspaces WHERE name = 'default'",
            [],
            |r| r.get(0),
        )
        .expect("default workspace exists");

    let board_id = uuid::Uuid::new_v4().to_string();
    let column_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    db.conn()
        .execute(
            "INSERT INTO boards (id, workspace_id, name, created_at)
             VALUES (?1, ?2, 'test-board', ?3)",
            rusqlite::params![board_id, workspace_id, now],
        )
        .expect("insert board");

    db.conn()
        .execute(
            "INSERT INTO columns (id, board_id, name, position, created_at)
             VALUES (?1, ?2, 'test-col', 0, ?3)",
            rusqlite::params![column_id, board_id, now],
        )
        .expect("insert column");

    (workspace_id, board_id, column_id)
}

/// Seed: workspace → board → two columns (`col-1` at position 0, `col-2` at position 1024).
///
/// Returns `(workspace_id, board_id, col1_id, col2_id)`.
pub fn seed_workspace_with_two_columns(db: &Db) -> (String, String, String, String) {
    crate::store::workspaces::WorkspaceStore::ensure_default(db).expect("ensure_default");

    let workspace_id: String = db
        .conn()
        .query_row(
            "SELECT id FROM workspaces WHERE name = 'default'",
            [],
            |r| r.get(0),
        )
        .expect("default workspace exists");

    let board_id = uuid::Uuid::new_v4().to_string();
    let col1_id = uuid::Uuid::new_v4().to_string();
    let col2_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    db.conn()
        .execute(
            "INSERT INTO boards (id, workspace_id, name, created_at)
             VALUES (?1, ?2, 'test-board', ?3)",
            rusqlite::params![board_id, workspace_id, now],
        )
        .expect("insert board");

    db.conn()
        .execute(
            "INSERT INTO columns (id, board_id, name, position, created_at)
             VALUES (?1, ?2, 'col-1', 0, ?3)",
            rusqlite::params![col1_id, board_id, now],
        )
        .expect("insert col1");

    db.conn()
        .execute(
            "INSERT INTO columns (id, board_id, name, position, created_at)
             VALUES (?1, ?2, 'col-2', 1024, ?3)",
            rusqlite::params![col2_id, board_id, now],
        )
        .expect("insert col2");

    (workspace_id, board_id, col1_id, col2_id)
}

/// Seed an Anthropic provider in the given DB. Returns the provider id.
///
/// Mirrors the config shape used by `store::sessions::tests::seed_deps`
/// so tests that call both can share an AppState. Used by lane-automation
/// tests (feat-025) which require at least one provider to fire
/// `create_session`.
pub fn seed_provider(db: &Db) -> String {
    let config = serde_json::json!({
        "base_url": "https://api.anthropic.com",
        "api_key": "sk-test",
        "default_model": "claude-sonnet-4-20250514"
    })
    .to_string();
    let provider = crate::store::providers::ProviderStore::create(db, "anthropic", "Test", &config)
        .expect("seed provider");
    provider.id
}

/// Insert a specialist into the registry at `specialists`.
///
/// `SpecialistRegistry::insert` takes `&mut self`, but the test `AppState`
/// holds `Arc<SpecialistRegistry>`. We use `Arc::get_mut`, which only
/// succeeds when the strong count is 1 — true immediately after
/// `make_test_state()` (the Arc is moved into AppState once). Callers
/// that need to seed a specialist on a shared `Arc` must use
/// `Arc::make_mut` (which clones on conflict) or refactor to a different
/// injection point.
pub fn seed_specialist(
    specialists: &mut crate::specialist::SpecialistRegistry,
    name: &str,
    system_prompt: &str,
) {
    use crate::specialist::Specialist;
    specialists.insert(Specialist {
        name: name.to_string(),
        description: format!("Test specialist {}", name),
        model: None,
        tool_profile: None,
        tags: vec![],
        system_prompt: system_prompt.to_string(),
    });
}

/// Convenience: seed both a provider (in DB) and a specialist (in registry).
///
/// Returns `(provider_id, specialist_name)`. This is the common shape
/// for lane-automation tests in feat-025.
pub fn seed_provider_and_specialist(
    state: &mut AppState,
    specialist_name: &str,
) -> (String, String) {
    let provider_id = seed_provider(&state.db);
    // Arc::get_mut only works when the strong count is 1; make_test_state
    // gives us a fresh Arc per call so this is safe.
    let specialists = Arc::get_mut(&mut state.specialists).expect("specialists Arc is unique");
    seed_specialist(specialists, specialist_name, "You are a test specialist.");
    (provider_id, specialist_name.to_string())
}
