#!/usr/bin/env bash
# Run the Qalarc OSINT Hub (backend + serves frontend/ at /). Binds 127.0.0.1 by default.
# Env: HOST, PORT, OSINT_HUB_TOKEN, ... (see backend/.env.example). A .env file next to
# this script's parent (qalarc_osint/.env) is sourced if present.
set -uo pipefail
cd "$(dirname "$0")/.."
[ -f .env ] && set -a && . ./.env && set +a
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8799}"
exec backend/.venv/bin/uvicorn app:app --host "$HOST" --port "$PORT" --app-dir backend
