// Unit tests for `applyBoardEvent` (the pure reducer that drives the
// useBoard hook's SSE handler) and the export of the hook's hook result
// shape. The hook itself is exercised via the integration-style tests
// in `kanban-board.test.tsx`.

import { describe, expect, it } from "vitest";
import { applyBoardEvent, boardReducer } from "../use-board";
import type { BoardDetail, Column, Task } from "../../lib/types";

function makeBoardDetail(): BoardDetail {
  const board = {
    id: "b1",
    workspace_id: "w1",
    name: "Test Board",
    created_at: "2026-06-01T00:00:00Z",
  };
  const columns: Column[] = [
    {
      id: "c1",
      board_id: "b1",
      name: "To Do",
      position: 0,
      specialist_id: null,
      auto_trigger: false,
      created_at: "2026-06-01T00:00:00Z",
    },
    {
      id: "c2",
      board_id: "b1",
      name: "Done",
      position: 1000,
      specialist_id: null,
      auto_trigger: false,
      created_at: "2026-06-01T00:00:00Z",
    },
  ];
  const tasks: Task[] = [
    {
      id: "t1",
      board_id: "b1",
      column_id: "c1",
      title: "Original task",
      description: null,
      position: 1000,
      status: "active",
      session_id: null,
      acceptance_criteria: null,
      completion_summary: null,
      verification_report: null,
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-01T00:00:00Z",
    },
  ];
  return { board, columns, tasks };
}

describe("applyBoardEvent", () => {
  it("appends a task on task_created", () => {
    const prev = makeBoardDetail();
    const next = applyBoardEvent(prev, {
      type: "task_created",
      task: { ...prev.tasks[0], id: "t2", title: "New task" },
    });
    expect(next.tasks).toHaveLength(2);
    expect(next.tasks[1].id).toBe("t2");
  });

  it("replaces a task in place on task_moved", () => {
    const prev = makeBoardDetail();
    const moved = { ...prev.tasks[0], column_id: "c2", position: 2000 };
    const next = applyBoardEvent(prev, {
      type: "task_moved",
      task: moved,
      from_column_id: "c1",
      to_column_id: "c2",
    });
    expect(next.tasks[0]).toEqual(moved);
  });

  it("replaces a task in place on task_updated", () => {
    const prev = makeBoardDetail();
    const updated = { ...prev.tasks[0], title: "Updated title", status: "done" as const };
    const next = applyBoardEvent(prev, { type: "task_updated", task: updated });
    expect(next.tasks[0].title).toBe("Updated title");
    expect(next.tasks[0].status).toBe("done");
  });

  it("removes a task on task_deleted", () => {
    const prev = makeBoardDetail();
    const next = applyBoardEvent(prev, {
      type: "task_deleted",
      task_id: "t1",
      column_id: "c1",
    });
    expect(next.tasks).toHaveLength(0);
  });

  it("appends and sorts a column on column_added", () => {
    const prev = makeBoardDetail();
    const inserted: Column = {
      ...prev.columns[0],
      id: "c-mid",
      name: "In Progress",
      position: 500,
    };
    const next = applyBoardEvent(prev, { type: "column_added", column: inserted });
    expect(next.columns).toHaveLength(3);
    expect(next.columns[0].id).toBe("c1");
    expect(next.columns[1].id).toBe("c-mid");
    expect(next.columns[2].id).toBe("c2");
  });

  it("patches session_id on session_started", () => {
    const prev = makeBoardDetail();
    const next = applyBoardEvent(prev, {
      type: "session_started",
      session_id: "sess-new",
      task_id: "t1",
      specialist_id: "code-agent",
      board_id: "b1",
    });
    expect(next.tasks[0].session_id).toBe("sess-new");
  });

  it("is a no-op for heartbeat", () => {
    const prev = makeBoardDetail();
    const next = applyBoardEvent(prev, { type: "heartbeat" });
    expect(next).toBe(prev);
  });

  it("is a no-op for connected", () => {
    const prev = makeBoardDetail();
    const next = applyBoardEvent(prev, { type: "connected", session_id: "" });
    expect(next).toBe(prev);
  });

  it("is a no-op for error", () => {
    const prev = makeBoardDetail();
    const next = applyBoardEvent(prev, { type: "error", message: "board not found" });
    expect(next).toBe(prev);
  });
});

describe("boardReducer", () => {
  it("applies the PATCH action by delegating to the patch function", () => {
    const prev = makeBoardDetail();
    const result = boardReducer(prev, {
      type: "PATCH",
      patch: (s) => ({ ...s, tasks: [...s.tasks, { ...s.tasks[0], id: "t-x" }] }),
    });
    expect(result.tasks).toHaveLength(prev.tasks.length + 1);
  });
});
