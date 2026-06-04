# Weave

Multi-agent coordination platform. Web-based, Linux-first, extensible.

## Quick Start

```bash
# Build
cargo build --release

# Run
./target/release/weave-server --port 3000

# Open
http://localhost:3000
```

## Architecture

Single Rust binary serving API + web UI, backed by SQLite.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for full architecture.

## Development

```bash
# Backend
cargo run -p weave-server

# Frontend (dev mode)
cd web && npm install && npm run dev
```

## Docs

- [Architecture](docs/ARCHITECTURE.md) — system design, domain model, API surface
- [Plan](docs/road-map/PLAN.md) — implementation phases and file structure
