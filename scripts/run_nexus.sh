#!/usr/bin/env bash
# NEXUS investigation workbench — build & run helper.
#   scripts/run_nexus.sh              build (if needed) + run on :8801
#   scripts/run_nexus.sh --build      force rebuild webui + rust
#   scripts/run_nexus.sh --check      run all test suites (core, server, ui)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PORT="${NEXUS_PORT:-8801}"
FORCE=0
[[ "${1:-}" == "--build" ]] && FORCE=1
if [[ "${1:-}" == "--check" ]]; then
  echo "== nexus-core tests ==" && cargo test -p nexus-core --manifest-path nexus/Cargo.toml
  echo "== nexus-server tests ==" && cargo test -p nexus-server --manifest-path nexus/Cargo.toml
  echo "== webui typecheck+build ==" && (cd nexus/webui && npm run typecheck && npm run build)
  echo "== ALL GREEN =="
  exit 0
fi

# 1. webui dist (build when missing or forced)
if [[ $FORCE -eq 1 || ! -f nexus/webui/dist/index.html ]]; then
  echo "== building webui =="
  (cd nexus/webui && npm install --no-fund --no-audit && npm run build)
fi

# 2. run nexus-server (desktop build: cargo run -p nexus-tauri --features desktop)
echo "== nexus on http://127.0.0.1:$PORT =="
exec cargo run --release -p nexus-server --manifest-path nexus/Cargo.toml -- --port "$PORT"
