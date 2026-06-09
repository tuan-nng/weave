import { useCallback, useEffect, useMemo, useReducer, useRef } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type {
  Board,
  BoardDetail,
  Column,
  CreateBoardRequest,
  CreateCardRequest,
  CreateColumnRequest,
  SseBoardEvent,
  Task,
  UpdateColumnRequest,
  UpdateTaskRequest,
} from "../lib/types";

// ---------------------------------------------------------------------------
// List boards per workspace (used by the /boards list page)
// ---------------------------------------------------------------------------

export function useBoards(workspaceId: string) {
  return useQuery({
    queryKey: queryKeys.boards.list(workspaceId),
    queryFn: () => api.kanban.boards.list(workspaceId),
    enabled: Boolean(workspaceId),
  });
}

// ---------------------------------------------------------------------------
// Create board (used by NewBoardModal, called from the per-workspace trigger
// on /boards and from the page-level trigger on /workspaces/:id). Bound to a
// single workspace. Mirrors `useCreateCodebase` in use-codebase.ts.
// ---------------------------------------------------------------------------

export function useCreateBoard(workspaceId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: CreateBoardRequest) => api.kanban.boards.create(workspaceId, data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.boards.list(workspaceId) });
    },
  });
}

// ---------------------------------------------------------------------------
// Public hook result
// ---------------------------------------------------------------------------

export interface UseBoardResult {
  // Source of truth
  board: Board | undefined;
  columns: Column[];
  tasks: Task[];

  // Derived
  tasksByColumn: Map<string, Task[]>;

  // Query state
  isLoading: boolean;
  isError: boolean;
  error: Error | null;

  // SSE
  isStreamConnected: boolean;

  // Mutations
  createBoard: (data: CreateBoardRequest) => void;
  createCard: (data: CreateCardRequest) => void;
  updateTask: (taskId: string, data: UpdateTaskRequest) => void;
  moveTask: (input: {
    taskId: string;
    toColumnId: string;
    /** Position relative to neighbors. Server rebalances when adjacent
     *  gaps fall below MIN_GAP; the optimistic value is overwritten by
     *  the SSE `task_moved` event's server-canonical position. */
    toPosition: number;
  }) => void;
  deleteTask: (taskId: string) => void;
  createColumn: (data: CreateColumnRequest) => void;
  updateColumn: (columnId: string, data: UpdateColumnRequest) => void;
  deleteBoard: () => void;

  isCreatingBoard: boolean;
  isCreatingCard: boolean;
  isUpdatingTask: boolean;
  isMovingTask: boolean;
  isDeletingTask: boolean;
  isCreatingColumn: boolean;
  isUpdatingColumn: boolean;
  isDeletingBoard: boolean;
}

// ---------------------------------------------------------------------------
// Reducer: SSE event → cache patch. The reducer is the single writer for
// the board's TanStack Query cache once the initial fetch is in place. SSE
// patches and the moveTask optimistic update both go through it, which
// makes the cache a single-writer invariant (no race between reducer and
// mutation response). The reducer is exported for unit tests.
// ---------------------------------------------------------------------------

export type BoardAction = { type: "PATCH"; patch: (prev: BoardDetail) => BoardDetail };

/**
 * @internal
 * Exported for unit testing only. The hook is the public API.
 */
export function boardReducer(state: BoardDetail, action: BoardAction): BoardDetail {
  switch (action.type) {
    case "PATCH":
      return action.patch(state);
  }
}

// ---------------------------------------------------------------------------
// SSE event → cache patch (pure function, exported for tests)
// ---------------------------------------------------------------------------

export function applyBoardEvent(prev: BoardDetail, event: SseBoardEvent): BoardDetail {
  switch (event.type) {
    case "task_created": {
      return { ...prev, tasks: [...prev.tasks, event.task] };
    }
    case "task_moved":
    case "task_updated": {
      return {
        ...prev,
        tasks: prev.tasks.map((t) => (t.id === event.task.id ? event.task : t)),
      };
    }
    case "task_deleted": {
      return {
        ...prev,
        tasks: prev.tasks.filter((t) => t.id !== event.task_id),
      };
    }
    case "column_added": {
      return {
        ...prev,
        columns: [...prev.columns, event.column].sort((a, b) => {
          if (a.position !== b.position) return a.position - b.position;
          return a.id.localeCompare(b.id);
        }),
      };
    }
    case "session_started": {
      return {
        ...prev,
        tasks: prev.tasks.map((t) =>
          t.id === event.task_id ? { ...t, session_id: event.session_id } : t,
        ),
      };
    }
    case "heartbeat":
    case "connected":
    case "error":
      // Lifecycle / keep-alive / protocol — no cache effect.
      return prev;
  }
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

export function useBoard(workspaceId: string, boardId: string): UseBoardResult {
  const qc = useQueryClient();
  const qcRef = useRef(qc);
  qcRef.current = qc;

  // --- Composite query ---
  const detailQuery = useQuery({
    queryKey: queryKeys.boards.detail(workspaceId, boardId),
    queryFn: () => api.kanban.boards.get(workspaceId, boardId),
    enabled: Boolean(workspaceId) && Boolean(boardId),
  });

  // --- Local reducer for derived stream state (e.g. connected) ---
  const [streamState, dispatchStream] = useReducer(
    (_state: { connected: boolean }, action: { type: "CONNECTED" | "DISCONNECTED" }) => {
      switch (action.type) {
        case "CONNECTED":
          return { connected: true };
        case "DISCONNECTED":
          return { connected: false };
      }
    },
    { connected: false },
  );

  // --- Snapshot for optimistic move rollback ---
  const moveSnapshotRef = useRef<BoardDetail | undefined>(undefined);

  // --- SSE ---
  useEffect(() => {
    if (!boardId) return;

    const es = new EventSource(`/api/boards/${boardId}/stream`);

    const handleEvent = (type: string, data: unknown) => {
      const event = { type, ...(data as object) } as SseBoardEvent;
      const key = queryKeys.boards.detail(workspaceId, boardId);

      switch (event.type) {
        case "connected":
          dispatchStream({ type: "CONNECTED" });
          break;
        case "heartbeat":
          // keep-alive only
          break;
        case "error":
          // Protocol event from the board stream when the board id is
          // not found. Logged; surfaced to the user via the query's
          // 404 state on initial load.
          console.warn("[useBoard] SSE error event:", event.message);
          break;
        default:
          // All other events patch the cache. The cache update is
          // idempotent: replaying the same event twice has the same
          // effect as playing it once.
          qcRef.current.setQueryData<BoardDetail>(key, (prev) =>
            prev ? applyBoardEvent(prev, event) : prev,
          );
          break;
      }
    };

    const eventTypes: SseBoardEvent["type"][] = [
      "task_created",
      "task_moved",
      "task_updated",
      "task_deleted",
      "column_added",
      "session_started",
      "heartbeat",
      "connected",
      "error",
    ];

    for (const type of eventTypes) {
      es.addEventListener(type, (e: MessageEvent) => {
        try {
          const data = JSON.parse(e.data);
          handleEvent(type, data);
        } catch (err) {
          console.warn("[useBoard] Failed to parse SSE event:", type, e.data, err);
        }
      });
    }

    es.onerror = () => {
      // Browser auto-reconnects. Disconnect state surfaces in the UI
      // for the brief reconnect window.
      dispatchStream({ type: "DISCONNECTED" });
    };

    return () => {
      es.close();
    };
  }, [boardId, workspaceId]);

  // --- Mutations ---

  const createBoardMutation = useMutation({
    mutationFn: (data: CreateBoardRequest) => api.kanban.boards.create(workspaceId, data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.boards.list(workspaceId) });
    },
  });

  const createCardMutation = useMutation({
    mutationFn: (data: CreateCardRequest) => api.kanban.cards.create(workspaceId, boardId, data),
    // No optimistic patch — the create_card handler broadcasts a
    // task_created SSE event within ~ms which the cache patch handles.
  });

  const updateTaskMutation = useMutation({
    mutationFn: ({ taskId, data }: { taskId: string; data: UpdateTaskRequest }) =>
      api.kanban.tasks.update(taskId, data),
    // SSE `task_updated` event patches the cache with the server's row.
  });

  const moveTaskMutation = useMutation({
    mutationFn: ({
      taskId,
      toColumnId,
      toPosition,
    }: {
      taskId: string;
      toColumnId: string;
      toPosition: number;
    }) =>
      api.kanban.tasks.update(taskId, {
        column_id: toColumnId,
        position: toPosition,
      }),
    onMutate: async (vars) => {
      const key = queryKeys.boards.detail(workspaceId, boardId);
      await qc.cancelQueries({ queryKey: key });
      const prev = qc.getQueryData<BoardDetail>(key);
      moveSnapshotRef.current = prev;
      if (prev) {
        qc.setQueryData<BoardDetail>(key, (curr) => {
          if (!curr) return curr;
          return {
            ...curr,
            tasks: curr.tasks.map((t) =>
              t.id === vars.taskId
                ? { ...t, column_id: vars.toColumnId, position: vars.toPosition }
                : t,
            ),
          };
        });
      }
      return { prev };
    },
    onError: (_err, _vars, ctx) => {
      if (ctx?.prev) {
        qc.setQueryData(queryKeys.boards.detail(workspaceId, boardId), ctx.prev);
      }
      moveSnapshotRef.current = undefined;
    },
    // onSettled does nothing — the SSE `task_moved` event is the
    // authoritative patch. The optimistic value and the server's value
    // (id + column_id + position) are idempotent, so the final cache
    // state matches the server regardless of arrival order.
  });

  const deleteTaskMutation = useMutation({
    mutationFn: (taskId: string) => api.kanban.tasks.delete(taskId),
    onMutate: async (taskId) => {
      const key = queryKeys.boards.detail(workspaceId, boardId);
      await qc.cancelQueries({ queryKey: key });
      const prev = qc.getQueryData<BoardDetail>(key);
      if (prev) {
        qc.setQueryData<BoardDetail>(key, {
          ...prev,
          tasks: prev.tasks.filter((t) => t.id !== taskId),
        });
      }
      return { prev };
    },
    onError: (_err, _vars, ctx) => {
      if (ctx?.prev) {
        qc.setQueryData(queryKeys.boards.detail(workspaceId, boardId), ctx.prev);
      }
    },
  });

  const createColumnMutation = useMutation({
    mutationFn: (data: CreateColumnRequest) =>
      api.kanban.columns.create(workspaceId, boardId, data),
  });

  const updateColumnMutation = useMutation({
    mutationFn: ({ columnId, data }: { columnId: string; data: UpdateColumnRequest }) =>
      api.kanban.columns.update(columnId, data),
  });

  const deleteBoardMutation = useMutation({
    mutationFn: () => api.kanban.boards.delete(workspaceId, boardId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.boards.list(workspaceId) });
    },
  });

  // --- Stable callbacks ---
  const createBoard = useCallback(
    (data: CreateBoardRequest) => createBoardMutation.mutate(data),
    [createBoardMutation],
  );
  const createCard = useCallback(
    (data: CreateCardRequest) => createCardMutation.mutate(data),
    [createCardMutation],
  );
  const updateTask = useCallback(
    (taskId: string, data: UpdateTaskRequest) => updateTaskMutation.mutate({ taskId, data }),
    [updateTaskMutation],
  );
  const moveTask = useCallback(
    (input: { taskId: string; toColumnId: string; toPosition: number }) =>
      moveTaskMutation.mutate(input),
    [moveTaskMutation],
  );
  const deleteTask = useCallback(
    (taskId: string) => deleteTaskMutation.mutate(taskId),
    [deleteTaskMutation],
  );
  const createColumn = useCallback(
    (data: CreateColumnRequest) => createColumnMutation.mutate(data),
    [createColumnMutation],
  );
  const updateColumn = useCallback(
    (columnId: string, data: UpdateColumnRequest) =>
      updateColumnMutation.mutate({ columnId, data }),
    [updateColumnMutation],
  );
  const deleteBoard = useCallback(() => deleteBoardMutation.mutate(), [deleteBoardMutation]);

  // --- Derived state ---
  const detail = detailQuery.data;
  const tasksByColumn = useMemo(() => {
    const map = new Map<string, Task[]>();
    if (!detail) return map;
    for (const task of detail.tasks) {
      const arr = map.get(task.column_id) ?? [];
      arr.push(task);
      map.set(task.column_id, arr);
    }
    // Sort each column's tasks by `position` ASC then id ASC (matches
    // the backend's `ORDER BY position ASC, id ASC` at tasks.rs:212-214).
    for (const [, arr] of map) {
      arr.sort((a, b) => {
        if (a.position !== b.position) return a.position - b.position;
        return a.id.localeCompare(b.id);
      });
    }
    return map;
  }, [detail]);

  return {
    board: detail?.board,
    columns: detail?.columns ?? [],
    tasks: detail?.tasks ?? [],
    tasksByColumn,

    isLoading: detailQuery.isLoading,
    isError: detailQuery.isError,
    error: (detailQuery.error as Error) ?? null,

    isStreamConnected: streamState.connected,

    createBoard,
    createCard,
    updateTask,
    moveTask,
    deleteTask,
    createColumn,
    updateColumn,
    deleteBoard,

    isCreatingBoard: createBoardMutation.isPending,
    isCreatingCard: createCardMutation.isPending,
    isUpdatingTask: updateTaskMutation.isPending,
    isMovingTask: moveTaskMutation.isPending,
    isDeletingTask: deleteTaskMutation.isPending,
    isCreatingColumn: createColumnMutation.isPending,
    isUpdatingColumn: updateColumnMutation.isPending,
    isDeletingBoard: deleteBoardMutation.isPending,
  };
}
