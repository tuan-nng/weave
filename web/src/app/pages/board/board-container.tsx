// BoardContainer — wraps the column row in @dnd-kit's DndContext and
// resolves drops into useBoard.moveTask calls. The container is purely
// a presentational + dnd-wiring shell: it reads from useBoard (passed
// via props from the page) and renders columns / add-column placeholder.

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
import { sortableKeyboardCoordinates } from "@dnd-kit/sortable";
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

interface BoardContainerProps {
  columns: Column[];
  tasksByColumn: Map<string, Task[]>;
  onCardClick: (task: Task) => void;
  onCreateColumn: (data: CreateColumnRequest) => void;
  onCreateCard: (data: CreateCardRequest) => void;
  onMoveTask: (input: MoveTaskInput) => void;
  isCreatingColumn: boolean;
  isCreatingCard: boolean;
  isMovingTask: boolean;
}

export function BoardContainer({
  columns,
  tasksByColumn,
  onCardClick,
  onCreateColumn,
  onCreateCard,
  onMoveTask,
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
  const [addCardColumnId, setAddCardColumnId] = useState<string | null>(null);
  const [addColumnOpen, setAddColumnOpen] = useState(false);

  function findTaskById(id: string): Task | undefined {
    for (const arr of tasksByColumn.values()) {
      const found = arr.find((t) => t.id === id);
      if (found) return found;
    }
    return undefined;
  }

  function handleDragStart(event: DragStartEvent) {
    const id = String(event.active.id);
    const found = findTaskById(id);
    if (found) setActiveTask(found);
  }

  function handleDragEnd(event: DragEndEvent) {
    setActiveTask(null);
    const { active, over } = event;
    if (!over) return;
    const taskId = String(active.id);
    const overId = String(over.id);

    // Resolve the drop target. Two cases:
    //   1) `over` is a column droppable (`col:<cid>`) — drop at end.
    //   2) `over` is another card — drop above it in that column.
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
      onDragCancel={() => setActiveTask(null)}
    >
      <div className="flex items-start gap-3 px-5 py-5 min-h-0 h-full">
        {columns.map((col) => (
          <BoardColumn
            key={col.id}
            column={col}
            tasks={tasksByColumn.get(col.id) ?? []}
            onCardClick={onCardClick}
            onAddCard={() => setAddCardColumnId(col.id)}
          />
        ))}
        <AddColumnButton onClick={() => setAddColumnOpen(true)} />
      </div>
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
        ) : null}
      </DragOverlay>
      {addCardColumnId !== null && (
        <AddCardModal
          open
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
