# Weave — task runner
# Run `just` to see available commands

# ── Default ──────────────────────────────────────────────────────────────────
default:
    @just --list

# ── Setup ────────────────────────────────────────────────────────────────────
# Install all dependencies (Rust + frontend)
setup:
    cargo fetch
    cd web && npm install

# ── Development ──────────────────────────────────────────────────────────────
# Start backend dev server with auto-reload
dev:
    cargo watch -x 'run -p weave-server'

# Start frontend dev server (Vite)
dev-web:
    cd web && npm run dev

# ── Build ────────────────────────────────────────────────────────────────────
# Build release binary (includes frontend)
build:
    cargo build --release -p weave-server

# Build debug binary
build-debug:
    cargo build -p weave-server

# ── Verification ─────────────────────────────────────────────────────────────
# Fast verification (lint + tests, no build)
check: lint test

# Run all linters (clippy + frontend)
lint:
    cargo clippy -p weave-server -- -D warnings
    cd web && npm run lint

# Check formatting (Rust + frontend)
fmt:
    cargo fmt --check
    cd web && npm run format:check 2>/dev/null || true

# Auto-fix formatting
fmt-fix:
    cargo fmt
    cd web && npm run format 2>/dev/null || true

# Run all tests (Rust + frontend)
test: test-rust test-web

# Run Rust tests only
test-rust:
    cargo test -p weave-server

# Run frontend tests only
test-web:
    cd web && npm test

# ── Clean ────────────────────────────────────────────────────────────────────
# Remove build artifacts
clean:
    cargo clean
    rm -rf web/node_modules web/dist
