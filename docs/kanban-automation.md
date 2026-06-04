# Kanban Lane Automation

When a task is moved into a column with `auto_trigger=true` and a bound `specialist_id`, the server automatically creates an agent session to process the task.

## Flow Diagram

```
User moves card to "In Progress" column
         │
         ▼
KanbanService.move_task(task_id, column_id)
         │
         ▼
    ┌────┴────┐
    │ Update  │
    │ task in │
    │   DB    │
    └────┬────┘
         │
         ▼
    ┌────┴────────────────────┐
    │ Broadcast task_moved    │
    │ event to board stream   │
    └────┬────────────────────┘
         │
         ▼
    ┌────┴────────────────────────┐
    │ Column has auto_trigger?    │
    │ AND specialist_id set?      │
    └────┬──────────┬─────────────┘
         │          │
        Yes         No → Done
         │
         ▼
    ┌────┴────────────────────┐
    │ Load specialist by ID   │
    │ from SpecialistLoader   │
    └────┬────────────────────┘
         │
         ▼
    ┌────┴────────────────────┐
    │ SessionService.create(  │
    │   specialist_id,        │
    │   initial_prompt:       │
    │   "Process task: ..."   │
    │ )                       │
    └────┬────────────────────┘
         │
         ▼
    ┌────┴────────────────────┐
    │ Update task.session_id  │
    │ Broadcast session_started│
    └────┬────────────────────┘
         │
         ▼
    Agent processes task autonomously
    (streaming events to session SSE)
```

## Default Board Template

When creating a board, users can specify columns. A default template:

| Position | Name | Specialist | Auto-trigger |
|----------|------|-----------|--------------|
| 0 | Backlog | backlog-refiner | true |
| 1 | To Do | todo-orchestrator | true |
| 2 | In Progress | dev-crafter | true |
| 3 | Review | review-guard | true |
| 4 | Done | done-reporter | false |
