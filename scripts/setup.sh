#!/usr/bin/env bash
# Setup for Qalarc OSINT Hub: venv + API deps + scanner tools + phoneinfoga binary.
# Idempotent — safe to re-run. Run from anywhere.
set -uo pipefail
cd "$(dirname "$0")/.."
BACKEND="backend"

echo "== 1/4 python venv =="
python3 -m venv "$BACKEND/.venv"
"$BACKEND/.venv/bin/pip" install -q -U pip wheel

echo "== 2/4 API dependencies =="
"$BACKEND/.venv/bin/pip" install -q -r "$BACKEND/requirements.txt" || { echo "FAIL: api deps"; exit 1; }

echo "== 3/4 scanner tools (pip) =="
for pkg in sherlock-project maigret ignorant holehe; do
  if "$BACKEND/.venv/bin/pip" install -q "$pkg"; then
    echo "  installed: $pkg"
  else
    echo "  WARN: could not install $pkg (tool will show as 'not installed')"
  fi
done

echo "== 4/4 phoneinfoga binary (best-effort) =="
mkdir -p "$BACKEND/bin"
if [ -x "$BACKEND/bin/phoneinfoga" ]; then
  echo "  phoneinfoga already present"
else
  URL=$(python3 - <<'PY'
import json, urllib.request
try:
    with urllib.request.urlopen("https://api.github.com/repos/sundowndev/phoneinfoga/releases/latest", timeout=20) as r:
        for a in json.load(r).get("assets", []):
            n = a["name"].lower()
            if "linux" in n and ("x86_64" in n or "amd64" in n) and n.endswith((".tar.gz", ".zip")):
                print(a["browser_download_url"]); break
except Exception:
    pass
PY
)
  if [ -n "${URL:-}" ]; then
    curl -sL "$URL" -o /tmp/phoneinfoga_dl && \
    (cd "$BACKEND/bin" && (tar xzf /tmp/phoneinfoga_dl 2>/dev/null || unzip -oq /tmp/phoneinfoga_dl)) && \
    chmod +x "$BACKEND/bin/phoneinfoga" && echo "  phoneinfoga installed" || echo "  WARN: phoneinfoga download failed"
    rm -f /tmp/phoneinfoga_dl
  else
    echo "  WARN: could not resolve phoneinfoga release URL (offline?)"
  fi
fi

echo
echo "== status =="
cd "$BACKEND" && ./.venv/bin/python - <<'PY'
import tools
for t in tools.tool_status():
    print(f"  {'OK ' if t['installed'] else '-- '} {t['id']:12s} {t['hint'] if not t['installed'] else ''}")
PY
echo
echo "Next: scripts/run.sh  →  http://127.0.0.1:8799"
