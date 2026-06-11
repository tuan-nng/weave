import { useCallback, useEffect, useState } from "react";

// ---------------------------------------------------------------------------
// WrappedSessionBanner — feat-054
//
// Small first-turn callout shown above the message list when the
// session is in `wrapped` mode. The banner surfaces the new
// interaction model in plain language ("Weave runs Claude Code in a
// subprocess this turn, then captures the response."). It is
// dismissable; the dismissal is persisted in localStorage under a
// per-session key, so reopening a wrapped session does not redisplay
// the banner, but a *new* wrapped session does.
//
// The banner is rendered only when:
//   1. The session is in `wrapped` mode (page-level gate), and
//   2. It is the first turn of the session — i.e. no assistant
//      message has been persisted yet. (Passing `firstTurn: false`
//      hides the banner unconditionally.)
// ---------------------------------------------------------------------------

/// Build the localStorage key for a given session. The session id is
/// server-issued and stable; prefixing with `weave.` keeps the
/// namespace from colliding with other apps on the same origin.
function dismissedKey(sessionId: string): string {
  return `weave.dismissed.wrappedSessionBanner.${sessionId}`;
}

interface WrappedSessionBannerProps {
  sessionId: string;
  /// When `false`, the banner is hidden entirely (used after the
  /// first turn completes). Defaults to `true` so the page-level
  /// `session.mode === "wrapped"` check is the only gate.
  firstTurn?: boolean;
}

export function WrappedSessionBanner({ sessionId, firstTurn = true }: WrappedSessionBannerProps) {
  // Local mount-time check, then a `useEffect` for any change to the
  // localStorage value (e.g. when the user dismisses the banner in
  // another tab). The `useState` initializer reads once; the
  // `storage` listener keeps two tabs in sync.
  const [dismissed, setDismissed] = useState<boolean>(() => {
    if (typeof window === "undefined") return false;
    try {
      return window.localStorage.getItem(dismissedKey(sessionId)) === "true";
    } catch {
      return false;
    }
  });

  useEffect(() => {
    if (typeof window === "undefined") return;
    const handler = (e: StorageEvent) => {
      if (e.key !== dismissedKey(sessionId)) return;
      setDismissed(e.newValue === "true");
    };
    window.addEventListener("storage", handler);
    return () => window.removeEventListener("storage", handler);
  }, [sessionId]);

  const handleDismiss = useCallback(() => {
    setDismissed(true);
    try {
      window.localStorage.setItem(dismissedKey(sessionId), "true");
    } catch {
      // localStorage may be unavailable (private browsing, quota
      // exceeded). The in-memory `dismissed` flag is still flipped
      // for the current session, so the banner is gone for the
      // user even if the write failed.
    }
  }, [sessionId]);

  if (!firstTurn || dismissed) return null;

  return (
    <div
      data-testid="wrapped-session-banner"
      className="rounded-xl border border-brand-orchid-200/60 bg-brand-orchid-50/60 px-4 py-3 flex items-start gap-3"
    >
      <div className="w-7 h-7 flex-shrink-0 rounded-lg bg-white border border-brand-orchid-200/60 flex items-center justify-center">
        <svg
          className="w-4 h-4 text-brand-orchid-600"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
          />
        </svg>
      </div>
      <div className="flex-1 min-w-0">
        <p className="text-[13px] font-medium text-brand-orchid-900">
          Wrapped runtime — subprocess per turn
        </p>
        <p className="text-[12px] text-brand-orchid-800/80 mt-0.5 leading-relaxed">
          Weave runs Claude Code in a subprocess for this turn and captures the conversation back.
          Each prompt re-invokes the CLI; resumes use a stored session id when the CLI accepts it.
        </p>
      </div>
      <button
        type="button"
        onClick={handleDismiss}
        aria-label="Dismiss banner"
        className="w-6 h-6 flex-shrink-0 flex items-center justify-center rounded-md text-brand-orchid-600 hover:bg-white/60 transition-colors"
      >
        <svg
          className="w-3.5 h-3.5"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2.5}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
        </svg>
      </button>
    </div>
  );
}
