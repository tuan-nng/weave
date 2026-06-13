// BoardContainer — wraps the column row in @dnd-kit's DndContext and
// resolves drops into useBoard.moveTask calls. The container is purely
// a presentational + dnd-wiring shell: it reads from useBoard (passed
// via props from the page) and renders columns / add-column placeholder.
//
// feat-068 (F-6 / F-9): the DndContext now also handles
// column-level drags. The `data.kind` discriminator on the
// sortable item separates card drags from column drags in
// `handleDragStart` / `handleDragEnd`. Column reordering uses
// the same midpoint-position computation as card reordering.
// The scrollable main element gets a `mask-image` to fade
// off-screen content (F-9).

import { useState } from "react";
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  closestCorners,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import { SortableContext, arrayMove, sortableKeyboardCoordinates } from "@dnd-kit/sortable";
import { AddColumnButton } from "./add-column-button";
import { AddColumnModal } from "./add-column-modal";
import { AddCardModal } from "./add-card-modal";
import { BoardColumn } from "./board-column";
import { TaskStatusChip } from "./task-status-chip";
import type { Column, CreateCardRequest, CreateColumnRequest, Task } from "../../../lib/types";

export interface MoveTaskInput {
  taskId: string;
  toColumnId: string;
  toPosition: number;
}

/// One column reordering step. The container passes a stable
/// `columns` prop, so we can't reorder in-place — we have to
/// compute a new position for the moved column and ask the
/// parent to PATCH it.
export interface MoveColumnInput {
  columnId: string;
  /// New position relative to neighbors. The server rebalances
  /// when adjacent gaps fall below `MIN_GAP = 2`; the optimistic
  /// value is overwritten by the `column_added` SSE event's
  /// server-canonical position (the same pattern as card moves).
  toPosition: number;
}

interface BoardContainerProps {
  /// F-15: workspace id, threaded down to AddCardModal so the
  /// codebase dropdown can fetch the right list.
  workspaceId: string;
  columns: Column[];
  tasksByColumn: Map<string, Task[]>;
  onCardClick: (task: Task) => void;
  onCreateColumn: (data: CreateColumnRequest) => void;
  onCreateCard: (data: CreateCardRequest) => void;
  onMoveTask: (input: MoveTaskInput) => void;
  onMoveColumn: (input: MoveColumnInput) => void;
  isCreatingColumn: boolean;
  isCreatingCard: boolean;
  isMovingTask: boolean;
}

export function BoardContainer({
  workspaceId,
  columns,
  tasksByColumn,
  onCardClick,
  onCreateColumn,
  onCreateCard,
  onMoveTask,
  onMoveColumn,
  isCreatingColumn,
  isCreatingCard,
  isMovingTask,
}: BoardContainerProps) {
  // dnd-kit's PointerSensor needs a small activation distance so click
  // events on cards still open the TaskDetailPanel.
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  // Active card shown in the DragOverlay. Using DragOverlay (not the
  // original card) keeps the live card visible at the cursor and lets
  // the source position collapse cleanly.
  const [activeTask, setActiveTask] = useState<Task | null>(null);
  /// Active column shown in the DragOverlay for column drags.
  /// Distinct from `activeTask` so card and column drags don't
  /// share overlay state.
  const [activeColumn, setActiveColumn] = useState<Column | null>(null);
  const [addCardColumnId, setAddCardColumnId] = useState<string | null>(null);
  const [addColumnOpen, setAddColumnOpen] = useState(false);

  function findTaskById(id: string): Task | undefined {
    for (const arr of tasksByColumn.values()) {
      const found = arr.find((t) => t.id === id);
      if (found) return found;
    }
    return undefined;
  }

  function findColumnById(id: string): Column | undefined {
    return columns.find((c) => c.id === id);
  }

  function handleDragStart(event: DragStartEvent) {
    const id = String(event.active.id);
    const data = event.active.data.current as { kind?: "card" | "column" } | undefined;
    if (data?.kind === "column") {
      const col = findColumnById(id);
      if (col) setActiveColumn(col);
      return;
    }
    const found = findTaskById(id);
    if (found) setActiveTask(found);
  }

  function handleDragEnd(event: DragEndEvent) {
    setActiveTask(null);
    setActiveColumn(null);
    const { active, over } = event;
    if (!over) return;
    const activeId = String(active.id);
    const overId = String(over.id);
    const activeData = active.data.current as { kind?: "card" | "column" } | undefined;

    if (activeData?.kind === "column") {
      // Column drag. `over` is another column id. Compute the new
      // position as the midpoint of the neighbors' positions in the
      // ordered list. The server rebalances when adjacent gaps fall
      // below MIN_GAP (the same discipline as card moves).
      if (activeId === overId) return;
      const fromIdx = columns.findIndex((c) => c.id === activeId);
      const toIdx = columns.findIndex((c) => c.id === overId);
      if (fromIdx === -1 || toIdx === -1) return;
      const reordered = arrayMove(columns, fromIdx, toIdx);
      // `reordered[toIdx]` is the moved column. Its new position
      // is the midpoint of its new neighbors.
      const newNeighbors = reordered.filter((c) => c.id !== activeId);
      const before = reordered[toIdx - 1]?.position;
      const after = reordered[toIdx + 1]?.position;
      let toPosition: number;
      if (before === undefined && after === undefined) {
        toPosition = 1000;
      } else if (before === undefined) {
        toPosition = (after ?? 1000) - 1000;
      } else if (after === undefined) {
        toPosition = before + 1000;
      } else {
        toPosition = Math.floor((before + after) / 2);
      }
      onMoveColumn({ columnId: activeId, toPosition });
      // `newNeighbors` is the post-move column list; unused below.
      void newNeighbors;
      return;
    }

    // Card drag. (Existing logic, copied verbatim.)
    const taskId = activeId;
    let toColumnId: string;
    let toIndex: number;

    if (overId.startsWith("col:")) {
      toColumnId = overId.slice(4);
      const colTasks = tasksByColumn.get(toColumnId) ?? [];
      toIndex = colTasks.length;
    } else {
      const overTask = findTaskById(overId);
      if (!overTask) return;
      toColumnId = overTask.column_id;
      const colTasks = tasksByColumn.get(toColumnId) ?? [];
      toIndex = colTasks.findIndex((t) => t.id === overId);
      if (toIndex === -1) return;
    }

    const fromTask = findTaskById(taskId);
    if (!fromTask) return;

    // Compute the new position. Use the midpoint of the neighbors'
    // positions in the target column. The server rebalances if the
    // resulting gap is < MIN_GAP (2), so this is best-effort; the SSE
    // `task_moved` event will overwrite with the canonical value.
    const targetTasks = (tasksByColumn.get(toColumnId) ?? []).filter((t) => t.id !== taskId);
    let toPosition: number;
    if (toIndex >= targetTasks.length) {
      // Dropping at the end: max position + 1000 (POSITION_STEP).
      toPosition = targetTasks.reduce((m, t) => Math.max(m, t.position), 0) + 1000;
    } else {
      const before = targetTasks[toIndex - 1]?.position;
      const after = targetTasks[toIndex]?.position;
      if (before === undefined) {
        toPosition = (after ?? 1000) - 1000;
      } else if (after === undefined) {
        toPosition = before + 1000;
      } else {
        toPosition = Math.floor((before + after) / 2);
      }
    }

    // No-op: same column, same position.
    if (fromTask.column_id === toColumnId && fromTask.position === toPosition) return;

    onMoveTask({ taskId, toColumnId, toPosition });
  }

  // The closure above uses `tasksByColumn` and the various handlers —
  // no other props read here. The unused `boardId` prop was removed
  // to keep the interface minimal.

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCorners}
      onDragStart={handleDragStart}
      onDragEnd={handleDragEnd}
      onDragCancel={() => {
        setActiveTask(null);
        setActiveColumn(null);
      }}
    >
      <SortableContext items={columns.map((c) => c.id)}>
        {/*
          F-9: horizontal scroll affordance. The `mask-image` fades
          the right edge so off-screen columns are visible. The
          scroll container is the parent <main> in board.tsx; this
          inner div just lays out the columns.
        */}
        <div
          className="flex items-start gap-3 px-5 py-5 min-h-0 h-full"
          style={{
            maskImage: "linear-gradient(to right, black 0%, black 92%, transparent 100%)",
            WebkitMaskImage: "linear-gradient(to right, black 0%, black 92%, transparent 100%)",
          }}
        >
          {columns.map((col, idx) => (
            <BoardColumn
              key={col.id}
              column={col}
              tasks={tasksByColumn.get(col.id) ?? []}
              isFirst={idx === 0}
              isLast={idx === columns.length - 1}
              onCardClick={onCardClick}
              onAddCard={() => setAddCardColumnId(col.id)}
              onMoveLeft={() => {
                if (idx === 0) return;
                const prev = columns[idx - 1];
                const toPosition = (prev.position + col.position) / 2;
                onMoveColumn({ columnId: col.id, toPosition });
              }}
              onMoveRight={() => {
                if (idx === columns.length - 1) return;
                const next = columns[idx + 1];
                const toPosition = (col.position + next.position) / 2;
                onMoveColumn({ columnId: col.id, toPosition });
              }}
            />
          ))}
          <AddColumnButton onClick={() => setAddColumnOpen(true)} />
        </div>
      </SortableContext>
      <DragOverlay>
        {activeTask ? (
          <div className="rounded-xl border border-slate-300 bg-white px-3 py-2.5 shadow-lg w-[256px] rotate-1">
            <span className="text-[13px] font-medium text-slate-900 leading-snug">
              {activeTask.title}
            </span>
            <div className="mt-2">
              <TaskStatusChip status={activeTask.status} />
            </div>
          </div>
        ) : activeColumn ? (
          <div className="w-[280px] flex-shrink-0 flex flex-col bg-white border border-black/[0.06] rounded-2xl max-h-full shadow-lg rotate-1">
            <div className="flex items-center gap-1.5 px-3.5 py-2.5 border-b border-slate-100">
              <span className="text-sm font-semibold text-slate-900 truncate">
                {activeColumn.name}
              </span>
              {activeColumn.specialist_id && <span className="text-[10px] text-slate-400">·</span>}
              {activeColumn.specialist_id && (
                <span className="text-[10px] text-slate-500">{activeColumn.specialist_id}</span>
              )}
            </div>
          </div>
        ) : null}
      </DragOverlay>
      {addCardColumnId !== null && (
        <AddCardModal
          open
          workspaceId={workspaceId}
          onClose={() => setAddCardColumnId(null)}
          onSubmit={(data) => {
            onCreateCard({ ...data, column_id: addCardColumnId });
            setAddCardColumnId(null);
          }}
          isSubmitting={isCreatingCard || isMovingTask}
        />
      )}
      <AddColumnModal
        open={addColumnOpen}
        onClose={() => setAddColumnOpen(false)}
        onSubmit={(data) => {
          onCreateColumn(data);
          setAddColumnOpen(false);
        }}
        isSubmitting={isCreatingColumn}
      />
    </DndContext>
  );
}
