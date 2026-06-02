//! Build script for `weave-server`.
//!
//! Runs `bunx vite build` in `../../web` so that the binary has a fresh
//! `web/dist/` to serve at runtime. We invoke the bundler directly
//! (rather than `bun run build`, which would also run `tsc -b`) because
//! the production build only needs the dist artifact; full type-check
//! runs in `just lint` and `just test`.
//!
//! The dist path is embedded at compile time via
//! `env!("CARGO_MANIFEST_DIR")` in `src/api/static_assets.rs` — resolution
//! is CWD-independent, so the binary can be invoked from any directory.
//!
//! To skip the frontend build (e.g. CI cache priming or quick rebuild
//! after a Rust-only change), set `WEAVE_SKIP_FRONTEND_BUILD=1`.
//!
//! See `just build-frontend` for an out-of-band build that bypasses
//! cargo and writes the dist directly.

use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let web_dir = Path::new(manifest_dir).join("../../web");

    // Re-run cargo whenever any frontend source of truth changes.
    // Paths are relative to this build script (i.e. `crates/weave-server/`).
    println!("cargo:rerun-if-changed=../../web/src");
    println!("cargo:rerun-if-changed=../../web/index.html");
    println!("cargo:rerun-if-changed=../../web/package.json");
    println!("cargo:rerun-if-changed=../../web/vite.config.ts");
    println!("cargo:rerun-if-changed=../../web/tsconfig.json");
    // bun.lock pins transitive versions; emit only if it exists today
    // (forward-compatible — the next agent to add it shouldn't need to
    // touch this file).
    if web_dir.join("bun.lock").exists() {
        println!("cargo:rerun-if-changed=../../web/bun.lock");
    }
    if web_dir.join("bun.lockb").exists() {
        println!("cargo:rerun-if-changed=../../web/bun.lockb");
    }
    // Vite's publicDir (default `web/public/`) is copied verbatim into
    // web/dist/. Doesn't exist today, but if a future agent adds a
    // favicon or robots.txt, this rerun line picks up changes without
    // needing a build.rs edit.
    if web_dir.join("public").exists() {
        println!("cargo:rerun-if-changed=../../web/public");
    }

    // Opt-out for CI cache priming or rust-only iteration.
    if std::env::var_os("WEAVE_SKIP_FRONTEND_BUILD").is_some() {
        return;
    }

    // Run `bunx vite build` in ../../web.
    let status = Command::new("bunx")
        .args(["vite", "build"])
        .current_dir(&web_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "feat-023 build.rs: failed to spawn `bunx vite build` (cwd={}).\n\
                 \n\
                 What:  could not launch the bun process: {e}.\n\
                 Why:   the binary embeds the frontend at compile time; \
                        without a fresh web/dist/, the server cannot serve \
                        the UI.\n\
                 How:   1) install bun (https://bun.sh) and ensure it is on PATH,\n\
                        2) re-run `cargo build -p weave-server`.\n\
                 \n\
                 If you want to skip the frontend build (e.g. you're iterating \
                 on Rust and `web/dist/` is already fresh), set \
                 WEAVE_SKIP_FRONTEND_BUILD=1 in the environment.",
                web_dir.display(),
            )
        });

    if !status.success() {
        panic!(
            "feat-023 build.rs: `bunx vite build` exited with {status:?} in {}.\n\
             \n\
             What:  frontend bundle failed.\n\
             Why:   the binary embeds the frontend at compile time.\n\
             How:   1) re-run the build directly to see the full output:\n\
                       cd web && bunx vite build\n\
                   2) fix the error, then re-run `cargo build -p weave-server`.",
            web_dir.display(),
        );
    }
}
