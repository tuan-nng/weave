// BoardPage — `/workspaces/:wid/boards/:bid`.
// Top-level page: header (back link, board name, monospace id, + Card / + Column
// buttons), horizontally-scrollable BoardContainer, slide-over TaskDetailPanel.

import { useState } from "react";
import { Link, useParams } from "react-router";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";
import { useBoard } from "../../hooks/use-board";
import { ROUTES } from "../../lib/routes";
import type { Task } from "../../lib/types";
import { BoardContainer, type MoveTaskInput } from "./board/board-container";
import { TaskDetailPanel } from "./board/task-detail-panel";

export default function BoardPage() {
  const { wid, bid } = useParams<{ wid: string; bid: string }>();
  const workspaceId = wid ?? "";
  const boardId = bid ?? "";

  const {
    board,
    columns,
    tasks,
    tasksByColumn,
    isLoading,
    isError,
    error,
    createCard,
    createColumn,
    updateTask,
    moveTask,
    deleteTask,
    isCreatingCard,
    isCreatingColumn,
    isUpdatingTask,
    isMovingTask,
    isDeletingTask,
  } = useBoard(workspaceId, boardId);

  // The id of the task currently shown in the slide-over. The panel
  // reads the canonical Task from the cache (via `useBoard.tasks`).
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const selectedTask = selectedTaskId ? (tasks.find((t) => t.id === selectedTaskId) ?? null) : null;

  // Rules-of-hooks: query is enabled only when params are set. If
  // either is missing, return early with a missing-id message.
  if (!workspaceId || !boardId) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-slate-500">Missing board id.</p>
      </div>
    );
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner />
      </div>
    );
  }

  if (isError) {
    return (
      <div className="p-8 max-w-4xl mx-auto">
        <ErrorBanner
          message={error?.message ?? "Failed to load board"}
          onDismiss={() => window.location.reload()}
        />
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full bg-[#fafafa]">
      {/* Fixed header */}
      <header className="flex-shrink-0 h-14 flex items-center justify-between px-5 bg-white/80 backdrop-blur-sm border-b border-slate-200/80">
        <div className="flex items-center gap-2.5">
          <Link
            to={ROUTES.boards}
            className="p-1.5 rounded-lg text-slate-400 hover:text-slate-600 hover:bg-slate-100/60 transition-colors"
            aria-label="Back to boards"
          >
            <svg
              width="18"
              height="18"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <polyline points="15 18 9 12 15 6" />
            </svg>
          </Link>
          <h1 className="text-sm font-semibold text-slate-900">{board?.name ?? "Board"}</h1>
          <span className="text-[10px] font-mono text-slate-400 bg-slate-50 border border-slate-200/60 rounded-md px-1.5 py-0.5">
            {boardId.slice(0, 8)}
          </span>
        </div>
        <div className="flex items-center gap-2">
          {/* Column creation is exposed via the AddColumnButton inside
              the BoardContainer; the + Card button here targets the
              first column as a sensible default. */}
          <button
            type="button"
            onClick={() => {
              if (columns[0]) {
                createCard({ column_id: columns[0].id, title: "New card" });
              }
            }}
            disabled={columns.length === 0 || isCreatingCard}
            className="h-9 px-4 bg-brand-blue-500 text-white rounded-xl text-sm font-medium hover:bg-brand-blue-600 transition-colors disabled:opacity-50"
          >
            + Card
          </button>
        </div>
      </header>

      {/* Horizontally-scrollable board */}
      <main className="flex-1 min-h-0 overflow-x-auto overflow-y-hidden">
        <BoardContainer
          columns={columns}
          tasksByColumn={tasksByColumn}
          onCardClick={(t: Task) => setSelectedTaskId(t.id)}
          onCreateColumn={createColumn}
          onCreateCard={createCard}
          onMoveTask={(input: MoveTaskInput) =>
            moveTask({
              taskId: input.taskId,
              toColumnId: input.toColumnId,
              toPosition: input.toPosition,
            })
          }
          isCreatingColumn={isCreatingColumn}
          isCreatingCard={isCreatingCard}
          isMovingTask={isMovingTask}
        />
      </main>

      <TaskDetailPanel
        task={selectedTask}
        onClose={() => setSelectedTaskId(null)}
        onSave={(taskId, data) => {
          updateTask(taskId, data);
        }}
        onDelete={(taskId) => {
          deleteTask(taskId);
          setSelectedTaskId(null);
        }}
        isSaving={isUpdatingTask}
        isDeleting={isDeletingTask}
      />
    </div>
  );
}
