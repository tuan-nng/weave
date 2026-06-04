# Frontend Architecture

React SPA served from the same binary as the API. Data fetching via TanStack Query, real-time updates via SSE, styling via Tailwind CSS v4.

## Routing

```tsx
const router = createBrowserRouter([
    { path: "/", element: <Home /> },
    { path: "/workspaces/:wid", element: <WorkspaceLayout />, children: [
        { index: true, element: <Overview /> },
        { path: "sessions", element: <Sessions /> },
        { path: "sessions/:sid", element: <Session /> },
        { path: "kanban", element: <Kanban /> },
        { path: "codebases", element: <Codebases /> },
    ]},
    { path: "/settings", element: <Settings /> },
]);
```

## Key Hooks

### useSession (Chat + SSE)
```typescript
function useSession(sessionId: string) {
    // TanStack Query for initial data
    const { data: session } = useQuery({ queryKey: ["session", sessionId], ... });
    const { data: history } = useQuery({ queryKey: ["session", sessionId, "history"], ... });

    // SSE connection for real-time updates
    const [events, setEvents] = useState<StreamEvent[]>([]);

    useEffect(() => {
        const es = new EventSource(`/api/sessions/${sessionId}/stream`);

        es.addEventListener("text_delta", (e) => {
            const data = JSON.parse(e.data);
            setEvents(prev => [...prev, { type: "text_delta", ...data }]);
        });

        es.addEventListener("done", () => {
            queryClient.invalidateQueries(["session", sessionId, "history"]);
        });

        return () => es.close();
    }, [sessionId]);

    const prompt = useMutation({
        mutationFn: (message: string) =>
            api.post(`/api/sessions/${sessionId}/prompt`, { message }),
    });

    return { session, history, events, prompt };
}
```

### useKanban (Board + SSE)
```typescript
function useKanban(boardId: string) {
    const { data: board } = useQuery({ queryKey: ["board", boardId], ... });

    useEffect(() => {
        const es = new EventSource(`/api/boards/${boardId}/stream`);
        // Listen for task_moved, task_created, session_started, etc.
        // Invalidate queries on changes
        return () => es.close();
    }, [boardId]);

    const moveTask = useMutation({
        mutationFn: ({ taskId, columnId, position }) =>
            api.patch(`/api/tasks/${taskId}`, { column_id: columnId, position }),
    });

    return { board, moveTask };
}
```

## Component Trees

### Chat View
```
SessionPage
├── SessionHeader (session status, specialist badge, cancel button)
├── MessageList
│   ├── MessageBubble (user)
│   │   └── MarkdownRenderer
│   ├── MessageBubble (assistant)
│   │   ├── MarkdownRenderer
│   │   └── ToolCallBlock (expandable)
│   │       ├── ToolCallHeader (tool name, duration)
│   │       └── ToolCallBody (input JSON, output)
│   └── StreamingIndicator (when agent is thinking)
├── JourneySidebar (collapsible)
│   ├── JourneyTimeline
│   │   └── DecisionNode (timestamp, summary, expandable)
│   └── FileChangesList
│       └── FileChangeItem (path, action badge)
└── MessageInput
    ├── Textarea (auto-resize)
    └── SendButton
```

### Kanban View
```
KanbanPage
├── BoardHeader (board name, add column button)
├── BoardContainer (horizontal scroll)
│   ├── Column
│   │   ├── ColumnHeader (name, specialist badge, auto-trigger toggle)
│   │   ├── CardList (drag-and-drop)
│   │   │   └── Card
│   │   │       ├── CardTitle
│   │   │       ├── CardStatus (badge: idle, agent working, done)
│   │   │       └── CardAgentIndicator (if session active)
│   │   └── AddCardButton
│   └── AddColumnButton
└── TaskDetailPanel (slide-over when card clicked)
    ├── TaskTitle (editable)
    ├── TaskDescription (editable)
    ├── TaskSession (link to session if agent is working)
    └── TaskHistory (trace summary)
```

## SSE → Cache Sync Strategy

SSE events update TanStack Query cache incrementally rather than triggering full refetches:

| Event | Strategy | Reason |
|-------|----------|--------|
| `text_delta` | Local state only | High frequency, no cache update needed |
| `done` | Invalidate history | Final state needs full message with metadata |
| `task_moved` | Patch cache | Single field change, cheap to apply |
| `task_created` | Patch cache | Add item to column's card list |
| `session_started` | Invalidate board | Task now has session_id, may need specialist info |

### Error Handling Pattern
```typescript
const { data, error, isLoading } = useQuery({ ... });

if (isLoading) return <Skeleton />;
if (error) return <ErrorBanner message={error.message} retry={() => refetch()} />;
```

**SSE error handling:**
```typescript
es.onerror = () => {
    // EventSource auto-reconnects by default
    // After 3 failed reconnects, show connection-lost banner
    setConnectionStatus("disconnected");
};
```

### Loading States
| Component | Loading State |
|-----------|--------------|
| Session page | Skeleton message bubbles (3 placeholders) |
| Kanban board | Skeleton columns with card placeholders |
| Message history | Spinner at top (loading older messages) |
| Tool call block | Collapsed by default, expandable spinner while streaming |
