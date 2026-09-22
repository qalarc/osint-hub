# NEXUS — Integration Notes

Per-connector status: what is verified against the real service, what is
best-effort, and how to configure each.

## qalarc OSINT hub (`hub`)

- **Status: verified** — this repo's own backend (`backend/app.py`, `:8799`).
- Config: `NEXUS_HUB_URL` (default `http://127.0.0.1:8799`), `OSINT_HUB_TOKEN`.
- Flow: `POST /api/scan {type:"multi", value, tools, options:{}}` → 202
  `{job_id}` → poll `GET /api/jobs/{id}` → ingest `results[].found[]` /
  `results[].info[]`. Tools per scan kind: username→sherlock+maigret,
  phone→phoneinfoga+ignorant, email→holehe, image→revimg.
- Ingest: entity per hit (dedup via normalized key), relation `uses` from the
  scanned entity, evidence per hit with tool+URL provenance.

## flowsint (`flowsint`) — VERIFIED LIVE 2026-09-22

- **Status: working end-to-end** (register → token → investigation+sketch →
  node sync → enricher launch → graph ingest; live test: `domain_to_subdomains`
  on qalarc.com → 3 subdomain entities + relations ingested).
- Config: `FLOWSINT_URL` (e.g. `http://127.0.0.1:5173`). Docker bring-up:
  `git clone https://github.com/reconurge/flowsint && cd flowsint && cp .env.example .env`
  (+ flowsint-api/core/app/.env), set AUTH_SECRET / MASTER_VAULT_KEY_V1 /
  NEO4J_PASSWORD, then `sudo docker compose -f docker-compose.prod.yml -p flowsint up -d`
  (~170 MB RAM total). Stack footprint verified tiny.
- **API notes (from live openapi.json):** auth is `POST /api/auth/token`
  (OAuth2 password form; register once via `POST /api/auth/register` — email
  must NOT be a special-use domain like `.local`); enrichers launch via
  `POST /api/enrichers/{slug}/launch {sketch_id, node_ids}` where node ids
  are **neo4j elementIds** (`4:<db>:n`, resolved from the graph after
  nodes/add); graph reads use keys **`nds`/`rls`**; nodes must carry their
  **typed primary property** (`Domain.domain`, `Ip.address`, `Phone.number`,
  `Username.value`, …) or the graph endpoint 500s on pydantic validation.
- Real enricher slugs: `domain_to_subdomains/dns/whois/asn`, `ip_to_asn/domain/
  intelligence/ports`, `email_to_breaches/gravatar/domains`,
  `username_to_socials_maigret/socials_sherlock`, `org_to_infos/domains/asn`,
  `website_to_crawler/links/webtrackers/text`, `cryptowallet_to_transactions/nfts`.
- Bridge state: `nexus/data/flowsint-bridge.json` (service creds + token +
  per-case investigation/sketch/node maps).

## OpenPlanter (`openplanter`)

- **Status: subprocess bridge** — runs the upstream CLI agent headlessly.
- Install (one-time):
  `git clone https://github.com/ShinMegamiBoson/OpenPlanter && cd OpenPlanter/agent && pip install -e .`
  (or point `OPENPLANTER_BIN` at the binary). `OPENPLANTER_PROVIDER=ollama`
  switches it to local models.
- Per task: fresh workspace `nexus/data/workspaces/<case>/<task>/`, run
  `openplanter-agent --task "<question>" --workspace <dir> --headless --no-tui`
  (20 min cap), then ingest: artifacts → evidence + `document` entities;
  regex sweep (emails, phones, domains, IPv4, wallets, profile-URL
  usernames) → candidate entities linked `derived_from`.
- **Local ollama mode** (`OPENPLANTER_PROVIDER=ollama`): also passes
  `--model` + `--reasoning-effort none`. Defaults to the **abliterated
  4.7 flash** (`hf.co/mradermacher/Huihui-GLM-4.7-Flash-abliterated-GGUF:Q4_K_M`,
  override `OPENPLANTER_OLLAMA_MODEL`). Hard-won model notes: OpenPlanter
  needs **native OpenAI tool_calls** — ollama 400s for non-tools models
  ("does not support tools"/"does not support thinking"); thinking models
  (glm-4.7-flash stock, qwen3.8, ornith) stall its parser with empty
  responses; use a tools-capable model with reasoning disabled.

## archon (`archon`)

- **Status: verified** against tulpa_platform `pillars.py` (2026-09):
  `GET/POST {ARCHON_URL}/api/sandboxes`, `POST {ARCHON_URL}/api/ask`
  `{question, sandbox_id?, top_k?} → {answer, citations, backend, found}`.
- Config: `ARCHON_URL` (default `http://127.0.0.1:7843`). Down archon never
  fails a case operation — degrades like every tulpa pillar.
- `archon_ground` task: answer + citations attached as evidence and appended
  to the entity's wiki doc Sources.
- `archon/push_case`: ensures sandbox `nexus-<slug>` (grounding corpus for
  chanalyse drafting). NOTE: the verified archon surface has no
  content-upload endpoint, so push currently creates/returns the sandbox;
  dossier hand-off goes through archon's own library import. Documented
  limitation, not a silent one.

## laya (`laya`)

- Config: `LAYA_URL` → `POST {url}/predict {"domain":"osint-grading", "state", "questions"}`.
- Datasets already exist (monitor-triage 1207, osint-grading 602, rfai-feed
  402, gmux-routing 554 rows). Register a fine-tuned checkpoint via the laya
  finetune MCP (`finetune_eval` gate → `finetune_register`) and point nexus
  at it.
- Decisions logged to `NEXUS_LAYA_LOG` (default
  `nexus/data/laya-decisions.jsonl`) → future `nexus-triage` fine-tune rows.

## jev (`jev`) — TypeSafe AI calibrated decisions

- **Status: verified** — `POST https://api.typesafe.ai/v1/systemone`
  `{"state", "model":"jev-latest", "questions":{name:{type:choice|score|noul,
  instructions, criteria?}}}`, Bearer `JEV_API_KEY` (fallback
  `TYPESAFE_API_KEY` env, else `~/.secrets/typesafe.env`).
- Key input: **user-suppliable** — settings modal (webui) → `POST
  /api/admin/config` → persisted to `nexus/data/config.json` (0600). Never
  logged; redacted in admin responses.
- Used for: lead triage (top-25, buffer-unordered 8, 30 s budget), merge
  preview gray band (0.82–0.97 similarity → one `noul` "same real-world
  entity?" question; ≥0.8 auto-merge offer / 0.5–0.8 review / <0.5 discard),
  hub-hit noise gate (flag, never drop), claim-support grading in briefs.
- Unset key or any error → silent fall-through to the next engine in the
  chain (laya → heuristic).

## websearch

Pure URL builders (no scraping): Google/DDG/Bing/news/Wikipedia/Reddit/
YouTube plus per-etype dorks (crt.sh, urlscan, shodan, abuseipdb,
blockchair, haveibeenpwned, LinkedIn, OpenCorporates). Emitted as note
entities + timeline; the UI opens them externally.
