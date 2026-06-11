// runtime-matrix.ts — pure-TS constants for the runtime × mode × provider-kind
// mapping. The backend stores `Provider.kind` as `"http" | "cli"` (DB column)
// and `Session.runtime_kind` / `Session.mode` as a wider kebab-case / snake_case
// enum pair (see `crates/weave-server/src/agent/registry.rs` and
// `crates/weave-server/src/migrations/011_session_runtime.sql`). This file
// owns the *client-side* policy that bridges the two: which Runtime Tool is
// the default for a given provider kind, and which specialist profiles a
// given runtime is currently willing to accept.
//
// Why client-side? The backend is moving toward a real compat table, but
// the current shape (per-runtime agents with all-allowed profiles) hasn't
// been tightened yet. Centralizing the policy here means a future
// tightening only needs to touch this file, not every wizard call site.
//
// feat-053: used by `NewSessionWizard` to (a) pick a default
// `RuntimeKind` + `SessionMode` when the user picks a provider, and
// (b) filter the specialist picker to the ones that match the chosen
// runtime. The server still runs its own checks (e.g.
// `runtime_mode_incompatible`); the matrix is a UX optimization, not
// the source of truth.

import type { Provider, RuntimeKind, SessionMode } from "./types";

/// Re-export so the wizard and the page can keep importing both
/// enums from this file (the runtime×mode matrix is the natural
/// home for them). The actual definitions live in `types.ts` — see
/// feat-054, which moved them there so SSE event types can carry
/// the same string-literal unions.
export type { RuntimeKind, SessionMode };

/// Pick a sensible `runtime_kind` + `mode` default for a given
/// provider kind. The matrix is currently:
///   - `http` providers → `anthropic-api` + `native`
///   - `cli` providers  → `claude-code` + `wrapped`
///
/// The user can later change either field in the wizard, but most
/// sessions never touch them. Hardcoding the defaults here means the
/// server never sees a "default-runtime" request shape — the client
/// always sends the resolved pair.
export function defaultRuntimeForProviderKind(kind: Provider["kind"]): {
  runtimeKind: RuntimeKind;
  mode: SessionMode;
} {
  if (kind === "http") {
    return { runtimeKind: "anthropic-api", mode: "native" };
  }
  // CLI is the only other kind in `Provider.kind` today (per
  // `crates/weave-server/migrations/012_provider_runtime_kind.sql`).
  // Future kinds (e.g. `local-llm`) would extend this branch.
  return { runtimeKind: "claude-code", mode: "wrapped" };
}

/// Specialist tool-profile compatibility per RuntimeKind. The current
/// matrix is "all runtimes accept all profiles" — but spelled out
/// explicitly so a future tightening is a one-line change here.
///
/// `ToolProfile` is intentionally a loose `string` to match the
/// `SpecialistInfo.tool_profile: string | null` field — the backend
/// doesn't constrain it to a closed enum. Profiles like
/// `"implementation"`, `"review"`, etc. are emergent naming.
export const SPECIALIST_PROFILE_COMPAT: Record<RuntimeKind, ReadonlySet<string>> = {
  "anthropic-api": new Set<string>(), // empty Set = "all" (current permissive state)
  "openai-api": new Set<string>(),
  "openai-compatible": new Set<string>(),
  "claude-code": new Set<string>(),
  codex: new Set<string>(),
  opencode: new Set<string>(),
};

/// Returns `true` if the given profile is acceptable on the given
/// runtime. An empty compat Set means "all profiles are accepted"
/// (the default today). A `null` profile is treated as accepted —
/// the specialist didn't opt into a profile, so any runtime runs it.
export function isProfileCompatible(runtimeKind: RuntimeKind, profile: string | null): boolean {
  if (profile === null) return true;
  const compat = SPECIALIST_PROFILE_COMPAT[runtimeKind];
  if (compat.size === 0) return true; // permissive default
  return compat.has(profile);
}
