# NEXUS — Architecture

> qalarc investigation workbench: graph-first OSINT case management,
> connector engine, calibrated decision chain, journalism exports.
> Binding API/data contract: [`../CONTRACT.md`](../CONTRACT.md).

```
                                ┌───────────────────────────────┐
                                │  chanalyse / archon / agents  │
                                └──────┬───────────────┬────────┘
                          MCP (stdio)  │               │  REST / SSE
                     nexus/mcp/nexus_mcp.py            │
                                   │                   │
┌──────────────────────────┐   ┌───▼───────────────────▼───────────────┐
│ Tauri 2 desktop (nexus-  │   │        nexus-server (axum, :8801)     │
│ tauri: headless server + │──►│  routes · task engine · SSE bus       │
│ in-process server +      │   ├───────────────────────────────────────┤
│ webview → workbench UI)  │   │ connectors:                           │
└──────────────────────────┘   │  hub ← qalarc OSINT hub (:8799)       │
                               │  flowsint ← flowsint enrichers        │
┌──────────────────────────┐   │  openplanter ← recursive agent CLI    │
│ webui (Vite+TS+Cytoscape)│──►│  archon ← canon RAG (:7843)           │
│ served from webui/dist   │   │  laya ← local fine-tuned decisions    │
└──────────────────────────┘   │  jev ← TypeSafe calibrated decisions  │
                               │  websearch ← pure pivot URL builders  │
                               ├───────────────────────────────────────┤
                               │        nexus-core (Rust lib)          │
                               │  SQLite store · entity resolution ·   │
                               │  graph algorithms · pivots · dossiers │
                               └───────────────────────────────────────┘
```

## Components

| Component | Path | Role |
|---|---|---|
| **nexus-core** | `crates/nexus-core` | Entity-graph engine: 22-type entity model, typed relations, evidence, timeline, wiki docs per entity, normalization+dedup (per-type keys, jaro-winkler), graph algorithms (BFS paths, degree, PageRank, components, label-propagation communities), pivot suggestions, dossier + article-brief generators. 52 tests. |
| **nexus-server** | `crates/nexus-server` | axum REST+SSE on `:8801`, task engine (2 workers, per-task SSE logs), all connectors, static UI serving, admin config overlay (persisted `nexus/data/config.json`). |
| **nexus-tauri** | `crates/nexus-tauri` | Dual-mode binary: headless `nexus-tauri --port N` runs the server; `--features desktop` boots the same server in-process on a random loopback port and opens the workbench in a webview window. |
| **webui** | `webui/` | The workbench: case rail, Cytoscape graph (etype color map, path mode, filters), inspector with per-entity actions, task console with live SSE logs, timeline, analysis, story tabs (dossier/brief/push-to-archon). Self-contained `dist/`. |
| **nexus-mcp** | `mcp/nexus_mcp.py` | 16-tool MCP stdio server → the integration surface for **archon** agents, chanalyse, and any MCP host. |

## The investigation loop

1. **Intake** — case + seed entities (manual, MCP, or `hub_scan`).
2. **Collect** — connector tasks fan out: hub scanners (sherlock/maigret/
   phoneinfoga/ignorant/holehe/revimg via the qalarc hub job API), flowsint
   enrichers (subdomains, WHOIS, ASN, breaches, crypto), OpenPlanter
   recursive agent research (workspace artifacts → documents → candidate
   entities), archon canon grounding.
3. **Link** — every hit is normalized, deduped, attached with provenance
   (source tool, URL, confidence). Merge decisions in the fuzzy gray band go
   through the calibrated `same_entity?` gate.
4. **Decide** — triage engine chain `jev → laya → heuristic` ranks leads;
   pivot engine suggests the next best actions (connector-gated).
5. **Publish** — dossier.md (evidence appendix) and brief.md (lede angles,
   nut graf, claim ledger with evidence status, open questions) feed article
   writing; `archon/push_case` seeds the grounding sandbox chanalyse drafts
   against.

## Decision engines (triage chain)

| Engine | What | Cost | When it wins |
|---|---|---|---|
| **jev** | TypeSafe SystemOne calibrated typed decisions (`choice`/`score`/`noul`), confidence-gated policy (≥0.8 act, 0.5–0.8 flag, <0.5 discard) | ~$0.00002/question | key configured; semantic gray-zone calls (merge gate, lead value, hub-hit noise filter, claim support) |
| **laya** | local fine-tuned decision model (e.g. `osint-grading` domain) | free, offline | `LAYA_URL` configured; domain checkpoint registered; learns from logged decisions |
| **heuristic** | deterministic scoring (etype weight + evidence boost + pinned boost) | free | always-available fallback |

Every triage/merge decision is append-logged (`nexus/data/laya-decisions.jsonl`,
`NEXUS_LAYA_LOG`) → training rows for the next laya fine-tune.

## Storage

Single SQLite DB (WAL): `nexus/data/nexus.db` (`NEXUS_DB`). Tables: cases,
entities (unique per `(case_id, etype, norm_key)`), relations (dedup
`src+dst+rel`), evidence, timeline, docs. Tasks are in-memory (restart clears
queue; case data persists).

## Ports & services

| Service | Port | Notes |
|---|---|---|
| nexus-server / desktop | 8801 | `NEXUS_PORT` |
| qalarc OSINT hub (existing) | 8799 | `scripts/run.sh` |
| flowsint (optional, docker) | 5173 | `FLOWSINT_URL` |
| archon pillar (existing) | 7843 | `ARCHON_URL` |

## Ethics

Authorized-use OSINT only (mirror of flowsint ETHICS.md and repo PRIVACY.md):
no stalking/doxxing/bulk enumeration; results are investigative leads with
provenance, not evidence of wrongdoing; journalism outputs carry claim-status
labels (Supported / Single-source / Unverified) and cite their sources.
