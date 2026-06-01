//! Task context tools — bridge between kanban tasks and agent sessions.
//!
//! These tools let agents read and update task state. Unlike filesystem/shell/git
//! tools, task tools hold `Arc<Db>` for database access.

pub mod get;
pub mod list;
pub mod update_fields;
pub mod update_status;

pub use get::GetTaskTool;
pub use list::ListTasksTool;
pub use update_fields::UpdateTaskFieldsTool;
pub use update_status::UpdateTaskStatusTool;
