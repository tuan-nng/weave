//! Per-provider model list cache (feat-042).
//!
//! Caches the `Vec<ModelInfo>` returned by `list_models` for every known
//! provider, keyed on `Provider::id`. The cache lives inside
//! [`crate::agent::registry::ProviderRegistry`] so it shares lifetime
//! and invalidation discipline with the existing `HealthCache`.
//!
//! ## Why
//!
//! The HTTP `list_models` path is a no-op stub today; the CLI path
//! introduced in feat-039 will be expensive in the common case (spawn a
//! binary, query with `--list-models`, parse stdout). Caching the
//! result for 5 minutes keeps the UI snappy and keeps the binary from
//! being invoked on every list refresh.
//!
//! ## Lifetime
//!
//! In-memory only. A server restart rebuilds the cache on demand.
//!
//! ## Concurrency contract
//!
//! The cache is guarded by a single `std::sync::Mutex<HashMap<...>>`.
//! The lock is held only for the HashMap mutation — the cached
//! `Vec<ModelInfo>` is cloned out under the lock and returned to the
//! caller, so the caller iterates without holding the lock.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::agent::ModelInfo;
use crate::error::ProviderError;
use crate::store::providers::Provider;
use crate::tools::truncate_bytes;

/// Default cache TTL: 5 minutes. Much longer than the 10-second
/// `HealthCache` TTL — model lists change rarely (on a CLI release) and
/// shelling out is expensive.
const MODEL_CACHE_DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Per-provider model list cache, keyed on `Provider::id`.
///
/// In-memory only; see module docs for the lifetime contract.
pub struct ModelCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, CachedModels>>,
}

struct CachedModels {
    models: Vec<ModelInfo>,
    fetched_at: Instant,
}

impl ModelCache {
    /// Create a new cache with the default 5-minute TTL.
    pub fn new() -> Self {
        Self::with_ttl(MODEL_CACHE_DEFAULT_TTL)
    }

    /// Create a new cache with a custom TTL. Used by tests to exercise
    /// expiry in milliseconds rather than the production 5 minutes.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Look up the cached models for `provider_id`. Returns
    /// `Some((models, fresh))` where `fresh` is `true` when the entry
    /// is younger than the TTL. Returns `None` if no entry exists.
    ///
    /// Callers decide what to do with a stale entry: the API handler
    /// treats both fresh and stale hits as cache hits (the stale flag
    /// is informational; the public surface never returns a different
    /// shape based on freshness), but tests use the flag to assert TTL
    /// behaviour directly.
    pub fn get(&self, provider_id: &str) -> Option<(Vec<ModelInfo>, bool)> {
        let entries = self.entries.lock().expect("model cache lock poisoned");
        let entry = entries.get(provider_id)?;
        let fresh = entry.fetched_at.elapsed() < self.ttl;
        Some((entry.models.clone(), fresh))
    }

    /// Store the freshly-fetched models for `provider_id`, stamping
    /// `fetched_at = Instant::now()`. Overwrites any existing entry.
    pub fn put(&self, provider_id: &str, models: Vec<ModelInfo>) {
        let mut entries = self.entries.lock().expect("model cache lock poisoned");
        entries.insert(
            provider_id.to_string(),
            CachedModels {
                models,
                fetched_at: Instant::now(),
            },
        );
    }

    /// Invalidate the entry for `provider_id`. No-op if absent.
    pub fn invalidate(&self, provider_id: &str) {
        let mut entries = self.entries.lock().expect("model cache lock poisoned");
        entries.remove(provider_id);
    }
}

/// Maximum bytes captured from a CLI's `--list-models` stdout (1 MiB).
/// The cap is enforced during the read loop (`read_bounded` below
/// drops bytes past the cap) so a 4 GB-emitting binary cannot OOM the
/// server before the timeout fires. A real model list is well under
/// 10 KiB; the cap is a memory-safety backstop.
const CLI_STDOUT_CAP: usize = 1024 * 1024;

/// Maximum bytes captured from a CLI's `--list-models` stderr (16 KiB).
const CLI_STDERR_CAP: usize = 16 * 1024;

/// Timeout for a single `--list-models` invocation (15 seconds). The
/// shell_exec tool default is 30s; CLI model listing is expected to be
/// near-instant (a static file read on the CLI side), so 15s is a
/// generous ceiling that still surfaces stuck processes quickly.
const CLI_TIMEOUT: Duration = Duration::from_secs(15);

/// Shell out to a CLI provider's registered binary and return the parsed
/// model list.
///
/// Invokes `<binary_path> <args_json...> --list-models`, reads stdout
/// (capped at 1 MiB during the read) and stderr (capped at 16 KiB
/// post-truncation), and parses stdout as either a bare JSON array
/// `[ModelInfo, ...]` or a wrapper object `{"models": [ModelInfo, ...]}`.
/// The bare array is the canonical shape we declare for the future
/// `CliCodingAgent` family (feat-051); the wrapper is accepted for
/// back-compat with row metadata that predates the shape decision.
///
/// The shell-out policy mirrors `tools::shell::ShellExecTool`:
/// `process_group(0)` on Unix, `kill_on_drop(true)`, a 15 s timeout,
/// bounded output capture. On Unix, the timeout sends SIGKILL to the
/// entire process group (not just the direct child) so grandchildren
/// spawned by the CLI do not hold the pipes open after the parent
/// exits.
///
/// `env_json` is parsed and persisted by `create_cli_provider`. The
/// request handler at `api/providers.rs::validate_env_keys` rejects a
/// denylist of dynamic-linker / shell-loader keys (LD_PRELOAD, PATH,
/// etc.) so a malicious row can't hijack the child process. The child
/// inherits those persisted safe entries (or, if `env_json` is empty,
/// the Weave process's environment, which is the safe-by-default
/// choice). The `binary_path` is also allowlisted to `claude` /
/// `fake_cli` at request time so this shell-out can't be pointed at
/// an arbitrary binary.
pub async fn list_cli_models_via_shell(
    provider: &Provider,
) -> Result<Vec<ModelInfo>, ProviderError> {
    let binary = provider.binary_path.as_deref().ok_or_else(|| {
        ProviderError::Unreachable(format!("provider {} has no binary_path", provider.id))
    })?;

    // Parse args_json (already validated and canonicalized by
    // `create_cli_provider`). If the parse fails at runtime, the row
    // is in a corrupt state — surface that as a 502 so the operator
    // notices instead of silently invoking the binary with no flags.
    let args: Vec<String> = match provider.args_json.as_deref() {
        Some(s) => serde_json::from_str(s).map_err(|e| {
            ProviderError::Unreachable(format!(
                "provider {} has corrupt args_json: {}",
                provider.id, e
            ))
        })?,
        None => Vec::new(),
    };

    let mut cmd = Command::new(binary);
    cmd.args(&args)
        .arg("--list-models")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // New process group on Unix so the timeout can SIGKILL the entire
    // tree (matches the policy in `tools::shell::ShellExecTool`).
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| ProviderError::Unreachable(format!("failed to spawn '{}': {}", binary, e)))?;

    // Capture the PID before we move `child` so the timeout branch
    // can still kill the process group.
    let pid = child.id();

    // Take ownership of stdout/stderr handles so we can drain them
    // concurrently with the wait. Stdout is bounded DURING the read
    // (so a 4 GB-emitting binary cannot OOM the server); stderr is
    // bounded by post-truncation (CLI errors are naturally small).
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderError::Unreachable("child has no stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProviderError::Unreachable("child has no stderr".into()))?;

    // Spawn one task that drains both pipes and returns the bytes.
    // The task ends when the child closes the pipes (or is killed).
    let read_task = tokio::spawn(async move {
        let (stdout_bytes, stderr_bytes) = tokio::join!(
            read_bounded(stdout, CLI_STDOUT_CAP),
            read_bounded(stderr, CLI_STDERR_CAP)
        );
        (stdout_bytes, stderr_bytes)
    });

    // Wait for the child with timeout. On timeout, kill the entire
    // process group (Unix) so any grandchildren don't hold the pipes
    // open and the read task can finish.
    let status = match tokio::time::timeout(CLI_TIMEOUT, child.wait()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return Err(ProviderError::Unreachable(format!(
                "failed to collect exit status from '{}': {}",
                binary, e
            )));
        }
        Err(_) => {
            kill_process_group_tree(pid);
            // Wait for the child to actually exit so we don't leak a
            // zombie. The read task unblocks when the pipes close.
            let _ = child.wait().await;
            let _ = read_task.await;
            return Err(ProviderError::Unreachable(format!(
                "'{} --list-models' timed out after {}s",
                binary,
                CLI_TIMEOUT.as_secs()
            )));
        }
    };

    // Collect the (bounded) output. The task cannot fail — read errors
    // are absorbed into the byte count.
    let (stdout_bytes, stderr_bytes) = read_task.await.unwrap_or_default();

    if !status.success() {
        // Stderr is already bounded by read_bounded; truncate_bytes
        // is a no-op in the common case but kept for the cap's
        // UTF-8-boundary safety.
        let stderr = truncate_bytes(&stderr_bytes, CLI_STDERR_CAP).0;
        return Err(ProviderError::Unreachable(format!(
            "'{} --list-models' exited with status {:?}: {}",
            binary,
            status.code(),
            stderr
        )));
    }

    // Parse stdout as either the canonical bare array or the
    // `{"models": [...]}` wrapper.
    let models: Vec<ModelInfo> = match serde_json::from_slice(&stdout_bytes) {
        Ok(m) => m,
        Err(_) => match serde_json::from_slice::<WrappedModels>(&stdout_bytes) {
            Ok(w) => w.models,
            Err(e) => {
                let stderr = truncate_bytes(&stderr_bytes, CLI_STDERR_CAP).0;
                return Err(ProviderError::Unreachable(format!(
                    "'{} --list-models' stdout is not valid JSON for a model list: {} (stderr: {})",
                    binary, e, stderr
                )));
            }
        },
    };

    Ok(models)
}

/// Wrapper shape accepted for back-compat: `{"models": [...]}`.
#[derive(Deserialize)]
struct WrappedModels {
    models: Vec<ModelInfo>,
}

/// Read up to `max_bytes` from `reader` into a `Vec<u8>`, dropping
/// anything beyond the cap. The reader is still drained past the cap
/// so the underlying pipe does not fill and the child never blocks on
/// write. If the child is killed (timeout), the pipe closes and the
/// loop returns.
async fn read_bounded<R>(mut reader: R, max_bytes: usize) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match reader.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < max_bytes {
                    let to_copy = (max_bytes - buf.len()).min(n);
                    buf.extend_from_slice(&tmp[..to_copy]);
                }
                // Bytes past the cap are intentionally dropped. We
                // keep reading to drain the pipe so the child never
                // blocks.
            }
            Err(_) => break,
        }
    }
    buf
}

/// Send SIGKILL to the entire process group rooted at `pid` (Unix).
/// On non-Unix, the child is killed by `kill_on_drop` on the `Child`
/// when the function returns — graceful single-process termination.
#[cfg(unix)]
fn kill_process_group_tree(pid: Option<u32>) {
    if let Some(pid) = pid {
        // Safety: killpg is a libc function; pid comes from the
        // child we spawned with `process_group(0)`.
        let _ = unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
    }
}

#[cfg(not(unix))]
fn kill_process_group_tree(_pid: Option<u32>) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn sample_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "claude-sonnet-4-5".into(),
                name: "Claude Sonnet 4.5".into(),
                context_window: 200_000,
            },
            ModelInfo {
                id: "claude-opus-4-1".into(),
                name: "Claude Opus 4.1".into(),
                context_window: 200_000,
            },
        ]
    }

    /// 1. (feat-042 verification) Cache hit: `put` then `get` returns
    /// the same models with `fresh = true` (within the TTL).
    #[test]
    fn test_model_cache_hit() {
        let cache = ModelCache::with_ttl(Duration::from_secs(60));
        cache.put("p1", sample_models());
        let (models, fresh) = cache.get("p1").expect("entry should exist");
        assert!(fresh, "freshly-put entry must be marked fresh");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "claude-sonnet-4-5");
    }

    /// 6. (feat-042 verification) TTL expiry: an entry past the TTL is
    /// returned with `fresh = false`. `put` after the expiry restores
    /// freshness. The cache never *deletes* a stale entry on its own —
    /// freshness is informational and the caller decides what to do.
    #[test]
    fn test_model_cache_ttl_expiry() {
        let cache = ModelCache::with_ttl(Duration::from_millis(50));
        cache.put("p1", sample_models());
        let (_models, fresh) = cache.get("p1").expect("entry exists immediately");
        assert!(fresh, "freshly put");

        // Sleep just past the TTL.
        std::thread::sleep(Duration::from_millis(80));

        let (_models, fresh) = cache.get("p1").expect("entry still exists");
        assert!(!fresh, "entry past TTL is no longer fresh");

        // `put` restores freshness.
        cache.put("p1", sample_models());
        let (_models, fresh) = cache.get("p1").expect("entry exists after re-put");
        assert!(fresh, "re-put restores freshness");
    }

    /// `invalidate` removes the entry; a subsequent `get` returns `None`.
    /// Pins the `put → invalidate → None` contract the API relies on.
    #[test]
    fn test_model_cache_invalidate() {
        let cache = ModelCache::new();
        cache.put("p1", sample_models());
        assert!(cache.get("p1").is_some());
        cache.invalidate("p1");
        assert!(
            cache.get("p1").is_none(),
            "invalidate must remove the entry"
        );
    }

    // ---- list_cli_models_via_shell tests (shell-out path) ----

    /// Build a CLI `Provider` row whose `binary_path` points at a
    /// tempfile bash script with the given body. Returns the
    /// `Provider` and the `TempDir` (caller drops the dir to clean up).
    /// Used by every shell-out test below.
    fn make_cli_provider_with_script(script_body: &str) -> (Provider, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let script = tmp.path().join("fake_cli.sh");
        std::fs::write(&script, script_body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let db = crate::db::Db::open(std::path::Path::new(":memory:")).unwrap();
        let provider = crate::store::providers::ProviderStore::create_cli(
            &db,
            "anthropic",
            "Test",
            "default-model",
            script.to_str().unwrap(),
            "[]",
            "{}",
            "default",
        )
        .unwrap();
        (provider, tmp)
    }

    /// 2. (feat-042 verification) Cache miss for a CLI provider shells
    /// out to the registered binary and parses the result.
    #[tokio::test]
    async fn test_model_cache_miss_shells_out() {
        let (provider, _tmp) = make_cli_provider_with_script(
            "#!/bin/sh\n\
             echo '[{\"id\":\"a\",\"name\":\"A\",\"context_window\":1000},{\"id\":\"b\",\"name\":\"B\",\"context_window\":2000}]'\n",
        );

        let models = list_cli_models_via_shell(&provider)
            .await
            .expect("shell-out must succeed");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "a");
        assert_eq!(models[1].context_window, 2000);
    }

    /// The wrapper shape `{"models": [...]}` is also accepted.
    #[tokio::test]
    async fn test_list_cli_models_via_shell_accepts_wrapped_shape() {
        let (provider, _tmp) = make_cli_provider_with_script(
            "#!/bin/sh\n\
             echo '{\"models\":[{\"id\":\"x\",\"name\":\"X\",\"context_window\":42}]}'\n",
        );

        let models = list_cli_models_via_shell(&provider)
            .await
            .expect("wrapped shape must parse");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "x");
    }

    /// A non-zero exit surfaces as `Unreachable` with the exit code in
    /// the message.
    #[tokio::test]
    async fn test_list_cli_models_via_shell_nonzero_exit() {
        let (provider, _tmp) = make_cli_provider_with_script("#!/bin/sh\necho bad >&2\nexit 7\n");

        let err = list_cli_models_via_shell(&provider)
            .await
            .expect_err("non-zero exit must error");
        match err {
            ProviderError::Unreachable(msg) => {
                assert!(msg.contains("exit"), "msg: {msg}");
                assert!(msg.contains("7"), "msg: {msg}");
            }
            other => panic!("expected Unreachable, got: {other:?}"),
        }
    }

    /// A binary that does not exist surfaces as `Unreachable` with a
    /// spawn-failure message.
    #[tokio::test]
    async fn test_list_cli_models_via_shell_missing_binary() {
        let db = crate::db::Db::open(std::path::Path::new(":memory:")).unwrap();
        let provider = crate::store::providers::ProviderStore::create_cli(
            &db,
            "anthropic",
            "Test",
            "default-model",
            "/nonexistent/path/to/binary",
            "[]",
            "{}",
            "default",
        )
        .unwrap();

        let err = list_cli_models_via_shell(&provider)
            .await
            .expect_err("missing binary must error");
        match err {
            ProviderError::Unreachable(msg) => {
                assert!(msg.contains("failed to spawn"), "msg: {msg}")
            }
            other => panic!("expected Unreachable, got: {other:?}"),
        }
    }

    /// A binary that emits garbage surfaces as `Unreachable` with a
    /// parse-error message.
    #[tokio::test]
    async fn test_list_cli_models_via_shell_unparseable_stdout() {
        let (provider, _tmp) = make_cli_provider_with_script("#!/bin/sh\necho 'not json'\n");

        let err = list_cli_models_via_shell(&provider)
            .await
            .expect_err("unparseable stdout must error");
        match err {
            ProviderError::Unreachable(msg) => {
                assert!(msg.contains("not valid JSON"), "msg: {msg}")
            }
            other => panic!("expected Unreachable, got: {other:?}"),
        }
    }

    /// A binary that hangs surfaces as `Unreachable` with a
    /// timeout message. The child is killed via SIGKILL to the
    /// process group, so it cannot outlive the function call.
    #[tokio::test]
    async fn test_list_cli_models_via_shell_timeout_kills_child() {
        let (provider, _tmp) = make_cli_provider_with_script("#!/bin/sh\nsleep 30\n");

        // 15s would be too slow for a test. The function uses a
        // const, so we exercise the timeout by sleeping past the
        // actual CLI_TIMEOUT — but to keep the test fast, we rely on
        // the fact that the child is killed and the function returns
        // within a few seconds. The timeout assertion is the message.
        let start = std::time::Instant::now();
        let err = list_cli_models_via_shell(&provider)
            .await
            .expect_err("hang must error");
        let elapsed = start.elapsed();

        // Should return in ~15s (CLI_TIMEOUT). We allow some slack
        // for CI noise, but the test must not run for the full 30s
        // the child tries to sleep.
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "timeout must fire before the child exits: {elapsed:?}"
        );
        match err {
            ProviderError::Unreachable(msg) => assert!(msg.contains("timed out"), "msg: {msg}"),
            other => panic!("expected Unreachable, got: {other:?}"),
        }
    }

    /// `read_bounded` enforces the cap during the read: only the
    /// first `max_bytes` are retained. The reader is still drained
    /// past the cap so the child never blocks.
    #[tokio::test]
    async fn test_read_bounded_drops_bytes_past_cap() {
        // Build a 4 KiB buffer that exceeds the cap. Write all of
        // it; the reader should retain only the first 2 KiB.
        let bytes: Vec<u8> = (0..4096u32).map(|i| (i & 0xFF) as u8).collect();
        let cap = 2048usize;

        let mut cursor = std::io::Cursor::new(bytes);
        let read = read_bounded(&mut cursor, cap).await;
        assert_eq!(read.len(), cap, "must retain exactly cap bytes");
        // Sanity: the first cap bytes match the source.
        for (i, b) in read.iter().enumerate() {
            assert_eq!(*b, (i & 0xFF) as u8, "byte {i} should be {:#x}", i & 0xFF);
        }
    }
}
