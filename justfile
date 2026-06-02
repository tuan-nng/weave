# Weave — task runner
# Run `just` to see available commands

# ── Default ──────────────────────────────────────────────────────────────────
default:
    @just --list

# ── Setup ────────────────────────────────────────────────────────────────────
# Install all dependencies (Rust + frontend)
setup:
    cargo fetch
    @if [ -d "web" ] && [ -f "web/package.json" ]; then cd web && bun install --frozen-lockfile; else echo "No web/ directory — skipping frontend deps"; fi

# ── Development ──────────────────────────────────────────────────────────────
# Start backend dev server with auto-reload
dev:
    cargo watch -x 'run -p weave-server'

# Start frontend dev server (Vite)
dev-web:
    cd web && bun run dev

# ── Build ────────────────────────────────────────────────────────────────────
# Build release binary (includes frontend)
build:
    cargo build --release -p weave-server

# Build debug binary
build-debug:
    cargo build -p weave-server

# Build only the frontend bundle (skip cargo). Use when iterating on
# UI without touching Rust, to avoid the slow `cargo build` cycle.
# Build.rs runs the same on every cargo build; this recipe is the
# out-of-band equivalent. We invoke `vite build` directly (not
# `bun run build`) because the production build only needs the
# bundle — type-check is handled by `just lint` and `just test`.
build-frontend:
    cd web && bunx vite build

# ── Verification ─────────────────────────────────────────────────────────────
# Fast verification (lint + tests, no build)
check: lint test

# Run all linters (clippy + frontend)
lint:
    cargo clippy -p weave-server -- -D warnings
    @if [ -d "web" ] && [ -f "web/package.json" ]; then cd web && bun run lint; else echo "No web/ directory — skipping frontend lint"; fi

# Check formatting (Rust + frontend)
fmt:
    cargo fmt --check
    @if [ -d "web" ] && [ -f "web/package.json" ]; then cd web && bun run format:check; else echo "No web/ directory — skipping frontend format check"; fi

# Auto-fix formatting
fmt-fix:
    cargo fmt
    @if [ -d "web" ] && [ -f "web/package.json" ]; then cd web && bun run format; else echo "No web/ directory — skipping frontend format fix"; fi

# Run all tests (Rust + frontend)
test: test-rust test-web

# Run Rust tests only
test-rust:
    cargo test -p weave-server

# Run frontend tests only (skips if web/ doesn't exist)
test-web:
    @if [ -d "web" ] && [ -f "web/package.json" ]; then cd web && bun run test; else echo "No web/ directory — skipping frontend tests"; fi

# ── Clean ────────────────────────────────────────────────────────────────────
# Remove build artifacts
clean:
    cargo clean
    rm -rf web/node_modules web/dist
