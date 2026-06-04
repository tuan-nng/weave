# web/src/components/ — Shared UI Components

Reusable presentational components used across multiple pages. No data fetching — props-only.

## Files

| File               | Contains                                                                    |
| ------------------ | --------------------------------------------------------------------------- |
| `error-banner.tsx` | Error display banner with dismiss — shows error message with red background |
| `modal.tsx`        | Generic modal dialog — overlay + close button + content slot                |
| `spinner.tsx`      | Loading spinner — animated SVG indicator                                    |
| `status-badge.tsx` | Status badge chip — color-coded by status (ready/completed/error/cancelled) |

## Key Patterns

- All components are pure presentational — no API calls, no TanStack Query
- Props-driven: receive data and callbacks, render UI
- Styled with Tailwind CSS v4 utility classes
- Used across `app/pages/*` components

## Connections

- **Used by:** `app/pages/*` components
- **No dependencies** on other frontend modules except `lib/types.ts`
