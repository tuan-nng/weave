// A single column in the board: header + scrollable card list +
// AddCardButton. The card list is the SortableContext target (per
// @dnd-kit's recommended pattern for sortable lists); the column body
// itself is also a useDroppable so a card can be dropped on an empty
// column without a list of cards to land on.

import { useDroppable } from "@dnd-kit/core";
import { SortableContext, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { AddCardButton } from "./add-card-button";
import { AutoTriggerDot, SpecialistChip } from "./agent-pill";
import { KanbanCard } from "./kanban-card";
import type { Column, Task } from "../../../lib/types";

interface BoardColumnProps {
  column: Column;
  tasks: Task[];
  onCardClick: (task: Task) => void;
  onAddCard: () => void;
}

export function BoardColumn({ column, tasks, onCardClick, onAddCard }: BoardColumnProps) {
  // The droppable id is namespaced so it can't collide with a card id
  // (which is a plain UUID).
  const { setNodeRef, isOver } = useDroppable({ id: `col:${column.id}` });

  return (
    <div
      className={`w-[280px] flex-shrink-0 flex flex-col bg-white border rounded-2xl max-h-full transition-colors ${
        isOver ? "border-brand-blue-300" : "border-black/[0.06]"
      }`}
    >
      <div className="flex items-center justify-between px-3.5 py-2.5 border-b border-slate-100">
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-sm font-semibold text-slate-900 truncate">{column.name}</span>
          {column.specialist_id && <SpecialistChip name={column.specialist_id} />}
          {column.auto_trigger && <AutoTriggerDot />}
        </div>
        <span className="text-[10px] font-mono text-slate-400">{tasks.length}</span>
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
