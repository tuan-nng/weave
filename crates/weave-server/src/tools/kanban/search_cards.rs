//! `search_cards` — list tasks with optional filters and free-text query.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::tasks::{TaskStore, VALID_TASK_STATUSES};
use crate::tools::fs::{check_optional_status, error, optional_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct SearchCardsTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for SearchCardsTool {
    fn name(&self) -> &str {
        "search_cards"
    }

    fn description(&self) -> &str {
        "Search cards (tasks) with optional filters. All filters are AND-ed. \
         `query` is a free-text substring matched against title and description \
         (case-insensitive). Results are scoped to the current workspace and \
         capped at 500 cards. Returns the matching tasks and a count."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Optional free-text query. Case-insensitive substring match against title and description."
                },
                "board_id": {
                    "type": "string",
                    "description": "Filter by board ID."
                },
                "column_id": {
                    "type": "string",
                    "description": "Filter by column ID."
                },
                "status": {
                    "type": "string",
                    "description": format!(
                        "Filter by status. Valid values: {}",
                        VALID_TASK_STATUSES.join(", ")
                    )
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        // Pass query through as-is; `TaskStore::list` does the canonical
        // blank-filtering (treats None or whitespace-only as "no filter").
        let query = optional_string(&input, "query");
        let board_id = optional_string(&input, "board_id");
        let column_id = optional_string(&input, "column_id");
        let status = optional_string(&input, "status");

        if let Err(e) = check_optional_status(status.as_deref()) {
            return e;
        }

        match TaskStore::list(
            &self.db,
            &ctx.workspace_id,
            board_id.as_deref(),
            column_id.as_deref(),
            status.as_deref(),
            query.as_deref(),
        ) {
            Ok(tasks) => {
                let count = tasks.len();
                success(json!({"tasks": tasks, "count": count}))
            }
            Err(e) => error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::make_test_db;
    use crate::tools::test_support::make_context;
    use tempfile::TempDir;

    const TEST_WS: &str = "test-workspace";

    /// Seed: workspace → board → 3 tasks with different titles + descriptions.
    /// Returns (board_id, col_id).
    fn seed_cards(db: &Db) -> (String, String) {
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'ws', 'active', ?2, ?2)",
                rusqlite::params![TEST_WS, now],
            )
            .unwrap();
        let bid = uuid::Uuid::new_v4().to_string();
        let cid = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'board', ?3)",
                rusqlite::params![bid, TEST_WS, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'col', 0, ?3)",
                rusqlite::params![cid, bid, now],
            )
            .unwrap();

        for (i, (title, desc, status)) in [
            ("Implement auth flow", "JWT-based session tokens", "active"),
            ("Fix login bug", "Users see 500 on invalid creds", "active"),
            ("Add logout endpoint", "Already in production", "done"),
        ]
        .iter()
        .enumerate()
        {
            db.conn()
                .execute(
                    "INSERT INTO tasks (id, board_id, column_id, title, description, position, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                    rusqlite::params![
                        uuid::Uuid::new_v4().to_string(),
                        bid,
                        cid,
                        title,
                        desc,
                        i as i64,
                        status,
                        now,
                    ],
                )
                .unwrap();
        }
        (bid, cid)
    }

    #[tokio::test]
    async fn test_search_cards_no_filters_returns_all() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        seed_cards(&db);

        let tool = SearchCardsTool { db };
        let result = tool.execute(json!({}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 3);
    }

    #[tokio::test]
    async fn test_search_cards_query_matches_title() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        seed_cards(&db);

        let tool = SearchCardsTool { db };
        let result = tool.execute(json!({"query": "auth"}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 1);
        assert_eq!(result.data["tasks"][0]["title"], "Implement auth flow");
    }

    #[tokio::test]
    async fn test_search_cards_query_matches_description() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        seed_cards(&db);

        let tool = SearchCardsTool { db };
        let result = tool.execute(json!({"query": "JWT"}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 1);
        assert_eq!(result.data["tasks"][0]["title"], "Implement auth flow");
    }

    #[tokio::test]
    async fn test_search_cards_query_no_match() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        seed_cards(&db);

        let tool = SearchCardsTool { db };
        let result = tool
            .execute(json!({"query": "nonexistent-term-xyz"}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["count"], 0);
    }

    #[tokio::test]
    async fn test_search_cards_combined_query_and_status() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        seed_cards(&db);

        let tool = SearchCardsTool { db };
        // "endpoint" matches "Add logout endpoint" (status=done).
        let result = tool
            .execute(json!({"query": "endpoint", "status": "done"}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["count"], 1);
        assert_eq!(result.data["tasks"][0]["title"], "Add logout endpoint");
    }

    #[tokio::test]
    async fn test_search_cards_filter_by_status() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        seed_cards(&db);

        let tool = SearchCardsTool { db };
        let result = tool.execute(json!({"status": "done"}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 1);
    }

    #[tokio::test]
    async fn test_search_cards_filter_by_board() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (bid, _) = seed_cards(&db);

        let tool = SearchCardsTool { db };
        let result = tool.execute(json!({"board_id": bid}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 3);
    }

    #[tokio::test]
    async fn test_search_cards_invalid_status() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();

        let tool = SearchCardsTool { db };
        let result = tool.execute(json!({"status": "invalid"}), &ctx).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid task status"));
    }

    #[tokio::test]
    async fn test_search_cards_empty_query_treated_as_none() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        seed_cards(&db);

        let tool = SearchCardsTool { db };
        let result = tool.execute(json!({"query": "   "}), &ctx).await;

        assert!(result.success);
        assert_eq!(
            result.data["count"], 3,
            "whitespace query should be treated as no filter"
        );
    }
}
