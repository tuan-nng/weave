// A single column in the board: header + scrollable card list +
// AddCardButton. The card list is the SortableContext target (per
// @dnd-kit's recommended pattern for sortable lists); the column body
// itself is also a useDroppable so a card can be dropped on an empty
// column without a list of cards to land on.
//
// feat-068 (F-3 / F-6): the column is also `useSortable` so the
// header acts as a drag handle for column reordering. The header
// also gets ◀ / ▶ buttons (`onMoveLeft` / `onMoveRight`) as a
// keyboard-accessible fallback — dnd-kit's PointerSensor does not
// announce its drag affordance to screen readers. The stage
// badge (F-3) renders in the header next to the name.

import { useDroppable } from "@dnd-kit/core";
import { SortableContext, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { AddCardButton } from "./add-card-button";
import { AutoTriggerDot, RuntimeKindBadge, SpecialistChip, StageBadge } from "./agent-pill";
import { KanbanCard } from "./kanban-card";
import type { Column, Task } from "../../../lib/types";

interface BoardColumnProps {
  column: Column;
  tasks: Task[];
  /// `true` when the column is the leftmost on the board
  /// (Move-left button disabled). Mirrored for right.
  isFirst: boolean;
  isLast: boolean;
  onCardClick: (task: Task) => void;
  onAddCard: () => void;
  onMoveLeft: () => void;
  onMoveRight: () => void;
}

export function BoardColumn({
  column,
  tasks,
  isFirst,
  isLast,
  onCardClick,
  onAddCard,
  onMoveLeft,
  onMoveRight,
}: BoardColumnProps) {
  // The droppable id is namespaced so it can't collide with a card id
  // (which is a plain UUID).
  const { setNodeRef, isOver } = useDroppable({ id: `col:${column.id}` });

  // Column-level sortable. The id is the column id directly
  // (dnd-kit's `useSortable` expects a string id, and we have
  // one). The `data.kind = "column"` discriminator lets the
  // DndContext's `onDragStart` distinguish card drags from
  // column drags.
  const {
    attributes,
    listeners,
    setNodeRef: setColumnRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: column.id,
    data: { kind: "column" },
  });
  const columnStyle: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
  };

  return (
    <div
      ref={setColumnRef}
      style={columnStyle}
      className={`w-[280px] flex-shrink-0 flex flex-col bg-white border rounded-2xl max-h-full transition-colors ${
        isOver ? "border-brand-blue-300" : "border-black/[0.06]"
      }`}
    >
      <div className="flex items-center justify-between px-2 py-2 border-b border-slate-100">
        <div className="flex items-center gap-1.5 min-w-0 flex-1 select-none">
          {/* Dedicated drag handle. The `listeners` are isolated to
              this icon so clicks on the column name / badges / ◀▶
              buttons don't accidentally start a column drag. The
              `attributes` (e.g. `aria-roledescription`) are
              spread on the parent so screen readers still announce
              the whole header as a draggable region. */}
          <button
            type="button"
            {...attributes}
            {...listeners}
            className="w-5 h-5 inline-flex items-center justify-center rounded text-slate-300 hover:text-slate-500 cursor-grab active:cursor-grabbing touch-none"
            aria-label={`Drag ${column.name} to reorder`}
            title="Drag to reorder"
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2.5}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <circle cx="9" cy="6" r="1" />
              <circle cx="9" cy="12" r="1" />
              <circle cx="9" cy="18" r="1" />
              <circle cx="15" cy="6" r="1" />
              <circle cx="15" cy="12" r="1" />
              <circle cx="15" cy="18" r="1" />
            </svg>
          </button>
          <span className="text-sm font-semibold text-slate-900 truncate">{column.name}</span>
          <StageBadge stage={column.stage} />
          {column.specialist_id && <SpecialistChip name={column.specialist_id} />}
          {column.auto_trigger && <AutoTriggerDot />}
          {column.auto_trigger && column.runtime_kind && (
            <RuntimeKindBadge kind={column.runtime_kind} />
          )}
          <span className="text-[10px] font-mono text-slate-400 ml-auto pl-2">{tasks.length}</span>
        </div>
        <div className="flex items-center gap-0.5 ml-1">
          <button
            type="button"
            onClick={onMoveLeft}
            disabled={isFirst}
            aria-label={`Move ${column.name} left`}
            title="Move left"
            className="w-6 h-6 inline-flex items-center justify-center rounded text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition-colors disabled:opacity-30 disabled:hover:bg-transparent"
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2.5}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <polyline points="15 18 9 12 15 6" />
            </svg>
          </button>
          <button
            type="button"
            onClick={onMoveRight}
            disabled={isLast}
            aria-label={`Move ${column.name} right`}
            title="Move right"
            className="w-6 h-6 inline-flex items-center justify-center rounded text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition-colors disabled:opacity-30 disabled:hover:bg-transparent"
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2.5}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <polyline points="9 18 15 12 9 6" />
            </svg>
          </button>
        </div>
      </div>
      <div ref={setNodeRef} className="flex-1 overflow-y-auto p-2 space-y-1.5 min-h-[60px]">
        {tasks.length === 0 ? (
          <div className="text-[11px] text-slate-400 italic text-center py-6">
            No cards yet — drag one here or add a new card below.
          </div>
        ) : (
          <SortableContext items={tasks.map((t) => t.id)} strategy={verticalListSortingStrategy}>
            {tasks.map((task) => (
              <KanbanCard key={task.id} task={task} onClick={onCardClick} />
            ))}
          </SortableContext>
        )}
      </div>
      <AddCardButton onClick={onAddCard} />
    </div>
  );
}
