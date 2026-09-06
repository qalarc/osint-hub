# Qalarc OSINT Hub

Web app that wraps local OSINT scanner tools behind a clean UI — to be hosted as a
qalarc.com project. Built 2026-09-06 on top of the repo catalog at
`../public_repos/` (see `CATALOG.md` there for the tool research).

```
┌────────────────────┐        ┌─────────────────────────────────┐
│  frontend (SPA)    │  HTTP  │  FastAPI backend (port 8799)    │
│  frontend/         │ ─────► │  POST /api/scan → job → stream  │
│  single index.html │  SSE / │  jobs.py (threads, rate limit)  │
│  zero deps         │  poll  │  tools.py (runs scanners)       │
└────────────────────┘        │   ├ sherlock   (username→sites) │
                              │   ├ maigret    (incl. dating)   │
                              │   ├ phoneinfoga (phone intel)   │
                              │   ├ ignorant   (phone→accounts) │
                              │   └ searchphone (opt-in, keys)  │
                              └─────────────────────────────────┘
```

## Quickstart

```bash
scripts/setup.sh   # venv + API deps + scanner pips + phoneinfoga binary (idempotent)
scripts/run.sh     # serves API + frontend at http://127.0.0.1:8799
```

## API

| Endpoint | What |
|---|---|
| `GET /api/health` | ok, version, tools_installed, auth_required |
| `GET /api/tools` | tool cards (installed state + setup hints) |
| `POST /api/scan` | single: `{"type":"username\|phone\|email","value":"...","tools":[...],"options":{...}}` · **multi (Case File)**: `{"type":"multi","value":{"username":...,"phone":...,"email":...},"tools":[...],"options":{"timeout":10,"sites":[...]}}` → `{"job_id":...}` — any subset of the three identifiers, tools auto-pair to each by kind |
| `GET /api/jobs/{id}` | snapshot: status, per-tool results — each `{tool, qtype, qvalue, status, found:[{site,url}], info:[{key,value\|url}], error}` |
| `GET /api/jobs/{id}/stream` | SSE: `snapshot` / `log` / `found` / `info` / `tool_done` / `job_done` events (all carry `qtype` on multi jobs) |
| `GET /api/jobs?limit=15` | recent jobs |

- `found` = discovered accounts/profiles; `info` = structured intelligence (phoneinfoga number facts: E164/Local/Country/Carrier…, plus Google-dork "search pivot" links)
- identifier kinds: **username** → sherlock, maigret · **phone** → phoneinfoga, ignorant, searchphone · **email** → holehe

Auth: set `OSINT_HUB_TOKEN` → all scan/job endpoints require `Authorization: Bearer <token>`.
Rate limit: `OSINT_HUB_RATE_LIMIT` (default `20/hour` per IP). Concurrency: `OSINT_HUB_MAX_CONCURRENT` (default 2).
Full env reference: `backend/.env.example`.

## Tech stack

| Layer | What |
|---|---|
| API | Python 3 · FastAPI · uvicorn · pydantic (request validation) |
| Job engine | ThreadPoolExecutor job queue · per-tool subprocess execution with threading.Timer hard-kill (900s) · sliding-window rate limiter · atomic job persistence to `data/jobs/*.json` (200-file prune) · SSE streaming via cursor-based event log |
| Scanners | sherlock (username→400+ sites) · maigret (username→3000+ incl. dating) · phoneinfoga (Go binary; phone intel + dorks) · ignorant (phone→registered accounts) · holehe (email→120+ services via password-reset flows) · searchphone (opt-in, API keys) |
| Output parsing | ANSI-stripped stdout parsers per tool (`[+]`-line grammar, tool-specific require-url rules, legend-line filters, phoneinfoga key/value info extraction, country-code splitting via phonenumbers) |
| Frontend | single-file vanilla JS + CSS — zero dependencies, no build step, SSE with 1.5s polling fallback when authed, steddi engineering-drawing art style (blueprint grid, film grain, laser scanline, mono dossier labels) |
| Deploy model | frontend → any static host (Cloudflare Pages) · backend → any host with the scanner CLIs · API base URL configurable at runtime in the UI |

## Hosting on qalarc.com

The **frontend** is a single static file → deployable to Cloudflare Pages (see
`~/projects/qalarc.ai/CLOUDFLARE_DEPLOYMENT.md` for account specifics):
either its own Pages project (`osint.qalarc.com`) or a route under the main site.
The API base URL is set at runtime in the UI (Settings ⚙ → API base).

The **backend cannot run on Cloudflare Pages** (it spawns local scanner processes).
Options, in order of sanity:
1. VPS / home server with `scripts/setup.sh` + systemd unit, fronted by cloudflared tunnel
   or a reverse proxy with TLS (`OSINT_HUB_TOKEN` set, `OSINT_HUB_CORS=0`).
2. Tailscale-only deployment (bind 127.0.0.1, expose via Tailscale Serve) — private use.

**Before any public exposure:** set `OSINT_HUB_TOKEN`, keep rate limits low, and read the
legal note below. A public "scan anyone" endpoint is an abuse magnet — this is why auth
ships disabled-but-recommended for localhost, and mandatory in the deploy checklist above.

## Tools & quirks (learned during integration, 2026-09-06)

- **sherlock**: `--site` limits scope (used for fast tests); output parsed from `[+] Site: url` lines. Site list: `../public_repos/osint/sherlock/sherlock_project/resources/data.json`.
- **maigret**: needs `--no-autoupdate --no-recursion --no-extracting --no-progressbar` for
  bounded web-service runs; its DB builds on first run (slow first scan is normal). Covers
  Tinder/Badoo/MeetMe/AFF — powers the Dating tab (client-side domain filter).
- **phoneinfoga**: Go binary in `backend/bin/` (downloaded by setup.sh); output parsed into structured `info` items — number formats (Raw local/Local/E164/International), Country, Carrier when available, plus Google-dork `search pivot` links (capped at 80 info items).
- **holehe**: email → registered-account checks on 120+ platforms via password-reset flows (~10-60s per email). `[+]` = registered (parsed as found); `[x]`/`[-]` lines filtered out.
- **ignorant**: CLI wants country code and number as SEPARATE args (`ignorant +61 425228338`);
  backend splits via `phonenumbers`. Legend lines are filtered from results.
- **searchphone**: disabled unless `SEARCHPHONE_ENABLED=1` + API keys (see
  `../public_repos/osint/searchphone/example.env`).

## Legal / responsible use

Username and phone lookups surface **publicly posted** profiles. Use for exposure
assessment, identity verification, anti-fraud and authorized research. Respect platform
ToS (automated enumeration may violate them), GDPR/CCPA rights of data subjects, and local
law. Not for stalking, harassment, or doxxing. If hosting publicly, publish a purpose
statement and a takedown contact.

## Files

```
backend/    app.py (API+SSE) · jobs.py (job manager) · tools.py (scanner registry) · .venv/
frontend/   index.html (single-file SPA)
scripts/    setup.sh · run.sh
data/       server.log · jobs/ (persisted job JSON, pruned at 200)
```
