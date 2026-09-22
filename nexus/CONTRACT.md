# NEXUS — Investigation Workbench: SYSTEM CONTRACT v1

> Single source of truth for all nexus components. Every crate/module codes
> against THIS document. If reality and contract diverge, fix reality.
> All JSON is snake_case. All timestamps are RFC3339 UTC strings.
> All ids are prefixed: `c_` case, `e_` entity, `r_` relation, `v_` evidence,
> `k_` timeline event, `t_` task. Generated as `<prefix>_<16 hex>` unless
> uuids already exist.

Authorized-use OSINT only. Ethics framing mirrors flowsint ETHICS.md.

## 0. Layout & ownership (do NOT touch other zones)

| Zone | Owner | Path |
|---|---|---|
| core engine | agent-core | `nexus/crates/nexus-core/**` |
| server + connectors | agent-server | `nexus/crates/nexus-server/**` |
| desktop shell | agent-tauri | `nexus/crates/nexus-tauri/**` |
| web UI | agent-ui | `nexus/webui/**` |
| mcp + docs + scripts | orchestrator | `nexus/mcp/**`, `nexus/docs/**`, `scripts/**` |

## 1. Configuration (env vars, all optional)

| Var | Default | Meaning |
|---|---|---|
| `NEXUS_DB` | `<repo>/nexus/data/nexus.db` | SQLite file (WAL) |
| `NEXUS_PORT` | `8801` | HTTP port for nexus-server |
| `NEXUS_HUB_URL` | `http://127.0.0.1:8799` | qalarc OSINT hub base |
| `OSINT_HUB_TOKEN` | (none) | Bearer token for the hub (reused name) |
| `FLOWSINT_URL` | (none) | flowsint base (e.g. http://127.0.0.1:5173) |
| `OPENPLANTER_BIN` | (auto-discover `openplanter-agent` on PATH) | CLI agent binary |
| `OPENPLANTER_WORKSPACES` | `<repo>/nexus/data/workspaces` | agent workspaces root |
| `ARCHON_URL` | `http://127.0.0.1:7843` | archon canon/RAG pillar |
| `LAYA_URL` | (none) | laya typed-decision HTTP bridge; heuristic fallback when unset |
| `NEXUS_LAYA_LOG` | (none) | JSONL path to log triage decisions for future fine-tuning |
| `JEV_URL` | `https://api.typesafe.ai/v1/systemone` | Jev (TypeSafe AI) calibrated-decision endpoint |
| `JEV_API_KEY` | falls back to `TYPESAFE_API_KEY` env, else `~/.secrets/typesafe.env` | Jev bearer key. NEVER log; redact in admin/config responses |
| `NEXUS_CONFIG` | `<repo>/nexus/data/config.json` | persisted runtime overlay written by POST /api/admin/config (0600), reloaded at boot |

Health `{connector: up|down|unset}` for hub, flowsint, openplanter, archon, laya, jev.
Triage engine chain (configurable order): `jev` (remote calibrated) → `laya`
(local fine-tuned) → `heuristic` (always available). First *configured and
healthy* engine wins; response reports which engine ran.

## 2. Data model (nexus-core)

### Entity
```json
{"id":"e_ab12","case_id":"c_1","etype":"username","label":"johndoe",
 "data":{},"confidence":1.0,"pinned":false,
 "first_seen":"2026-09-22T00:00:00Z","last_seen":"2026-09-22T00:00:00Z"}
```
`etype` ∈ fixed enum: `person org username alias email phone domain subdomain
ip asn cidr website social_profile wallet transaction image document article
event location topic note`.
Normalization keys (for dedup): email→lowercase; domain/subdomain→lowercase,
strip leading `www.`; username→lowercase; phone→E.164 digits; ip/asn/cidr
verbatim; website→lowercased host+path. `data` is free JSON (e.g.
`{"platform":"github","url":"..."}`).

### Relation
```json
{"id":"r_1","case_id":"c_1","src":"e_1","dst":"e_2","rel":"uses",
 "weight":1.0,"confidence":0.9,"source":"hub:sherlock",
 "evidence":{"title":"sherlock hit","url":"https://github.com/johndoe"},
 "created_at":"..."}
```
`rel` free-form typed string, canonical set: `uses owns resolves_to
subdomain_of registered_to mentions contacted same_as paid_to located_in
member_of works_at sourced_from derived_from related_to posted_on`.

### Evidence
```json
{"id":"v_1","case_id":"c_1","subject":"e_1","kind":"tool_result",
 "title":"...","url":"...","snippet":"...","raw":{},
 "confidence":0.9,"ts":"..."}
```
`kind` ∈ `tool_result web_fetch note wiki_doc archon_answer
flowsint_enricher openplanter_finding manual`. Subject may be entity id or
relation id.

### Timeline event
```json
{"id":"k_1","case_id":"c_1","ts":"...","entity_id":"e_1","kind":"scan",
 "title":"...","detail":{},"source":"hub"}
```

### Case
```json
{"id":"c_1","name":"Case 41 — subject X","slug":"case-41-subject-x",
 "status":"active","notes":"","tags":[],"created_at":"...","updated_at":"..."}
```

### Wiki doc (OpenPlanter-style, per entity)
Stored 1:1 with entity (`nexus-core` auto-skeletons it):
`{"entity_id":"e_1","markdown":"# <label>\n\n## Summary ...\n## Known facts\n## Connections\n## Timeline\n## Open questions\n## Sources\n","updated_at":"..."}`
Sections are re-rendered by `render` (connections/timeline/sources refresh,
human prose preserved under Summary / Open questions).

## 3. nexus-core public API (crates/nexus-core)

```rust
pub struct Store { /* rusqlite Connection behind Mutex, WAL */ }
impl Store {
    pub fn open(path: &Path) -> Result<Self>;
    // cases
    pub fn create_case(&self, name: &str, notes: &str, tags: &[String]) -> Result<Case>;
    pub fn list_cases(&self) -> Result<Vec<Case>>;
    pub fn get_case(&self, id: &str) -> Result<Option<Case>>;
    pub fn update_case(&self, id: &str, status: Option<&str>, notes: Option<&str>) -> Result<()>;
    pub fn delete_case(&self, id: &str) -> Result<()>;
    // entities
    pub fn add_entity(&self, case_id: &str, NewEntity) -> Result<Entity>; // resolves/dedups
    pub fn get_entity(&self, id: &str) -> Result<Option<Entity>>;
    pub fn update_entity(&self, id: &str, patch: EntityPatch) -> Result<Entity>;
    pub fn delete_entity(&self, id: &str) -> Result<()>;
    pub fn merge_entities(&self, primary: &str, other: &str) -> Result<Entity>; // same_as edge, relations re-pointed
    pub fn find_entity(&self, case_id: &str, etype: &str, label: &str) -> Result<Option<Entity>>;
    pub fn list_entities(&self, case_id: &str) -> Result<Vec<Entity>>;
    // relations / evidence / timeline
    pub fn add_relation(&self, NewRelation) -> Result<Relation>; // dedup on (src,dst,rel)
    pub fn delete_relation(&self, id: &str) -> Result<()>;
    pub fn list_relations(&self, case_id: &str) -> Result<Vec<Relation>>;
    pub fn add_evidence(&self, NewEvidence) -> Result<Evidence>;
    pub fn list_evidence(&self, subject: &str) -> Result<Vec<Evidence>>;
    pub fn add_timeline(&self, NewEvent) -> Result<TimelineEvent>;
    pub fn list_timeline(&self, case_id: &str) -> Result<Vec<TimelineEvent>>;
    // wiki docs
    pub fn get_doc(&self, entity_id: &str) -> Result<Option<String>>;
    pub fn set_doc(&self, entity_id: &str, markdown: &str) -> Result<()>;
    pub fn render_doc(&self, entity_id: &str) -> Result<String>; // skeleton + graph facts, keeps prose
}
pub struct Graph { pub nodes: Vec<Entity>, pub edges: Vec<Relation> }
pub fn load_graph(store: &Store, case_id: &str) -> Result<Graph>;
// algorithms (graph.rs)
pub fn neighbors(g: &Graph, id: &str, depth: usize) -> Vec<&Entity>;
pub fn shortest_path(g: &Graph, a: &str, b: &str) -> Option<Vec<String>>; // entity ids
pub fn degree_centrality(g: &Graph) -> BTreeMap<String, usize>;
pub fn pagerank(g: &Graph, damping: f64, iters: usize) -> BTreeMap<String, f64>;
pub fn components(g: &Graph) -> Vec<Vec<String>>;        // connected components
pub fn communities(g: &Graph) -> BTreeMap<String, usize>; // label propagation
// pivot.rs
pub struct PivotSuggestion { pub entity_id: String, pub action: String,  // e.g. "hub_scan:username"
    pub reason: String, pub priority: f64 }
pub fn suggest_pivots(g: &Graph, entity_id: &str, connectors: &[&str]) -> Vec<PivotSuggestion>;
// resolve.rs — normalize(etype,label)->String, similarity(a,b)->f64 (jaro-winkler), merge candidates
// report.rs
pub fn dossier(g: &Graph, case: &Case, store: &Store) -> String; // full markdown dossier w/ evidence appendix
pub fn brief(g: &Graph, case: &Case, store: &Store) -> String;   // article brief: lede angles, claims+evidence status, open questions, sources
```
Schema auto-migrates on open (`CREATE TABLE IF NOT EXISTS`, `PRAGMA user_version`).

## 4. HTTP API (nexus-server, axum; serves webui/dist as static root)

All under `/api`. Errors: `{"error": "..."}` with 4xx/5xx. Success bodies as
specified. Long ops → tasks. Global SSE bus at `GET /api/events` emits JSON
lines: `{"type":"case_updated|task_update|entity_added","case_id":...,...}`.

### Cases / graph
- `GET  /api/health` → `{"ok":true,"version":"0.1.0","connectors":{"hub":"up","flowsint":"unset","openplanter":"down","archon":"up","laya":"unset"}}`
- `GET  /api/cases` → `{"cases":[Case...]}`
- `POST /api/cases` `{name, notes?, tags?}` → 201 `{"case":Case}`
- `GET  /api/cases/{id}` → `{"case":Case,"stats":{"entities":N,"relations":N,"evidence":N,"tasks":N}}`
- `PATCH /api/cases/{id}` `{status?, notes?}` → `{"case":Case}`
- `DELETE /api/cases/{id}` → `{"ok":true}`
- `GET  /api/cases/{id}/graph` → `{"nodes":[Entity...],"edges":[Relation...],
     "analysis":{"degree":{},"pagerank":{},"communities":{},"components":[[...]]}}`
- `GET  /api/cases/{id}/path?a=e_1&b=e_2` → `{"path":[ids] | null}`
- `GET  /api/cases/{id}/pivots?entity=e_1` → `{"pivots":[PivotSuggestion...]}`

### Entities / relations / evidence / timeline / docs
- `POST /api/cases/{id}/entities` `{etype,label,data?,confidence?}` → 201 `{"entity":Entity,"deduplicated":bool}`
- `GET  /api/entities/{id}` → `{"entity":E,"relations":[R...],"evidence":[V...],"doc":str|null}`
- `PATCH /api/entities/{id}` `{label?,data?,pinned?,confidence?}` → `{"entity":E}`
- `DELETE /api/entities/{id}` → `{"ok":true}`
- `POST /api/entities/{id}/merge` `{other:"e_2"}` → `{"entity":E}`
- `POST /api/relations` `{case_id,src,dst,rel,weight?,confidence?,source?,evidence?}` → 201 `{"relation":R}`
- `DELETE /api/relations/{id}` → `{"ok":true}`
- `POST /api/evidence` `{case_id,subject,kind,title?,url?,snippet?,raw?,confidence?}` → 201 `{"evidence":V}`
- `GET  /api/entities/{id}/evidence` → `{"evidence":[V...]}`
- `GET  /api/cases/{id}/timeline` → `{"events":[K...]}`
- `POST /api/cases/{id}/timeline` `{ts?,entity_id?,kind,title,detail?,source?}` → 201 `{"event":K}`
- `GET  /api/entities/{id}/doc` → `{"doc":str}`
- `PATCH /api/entities/{id}/doc` `{markdown}` → `{"doc":str}`
- `POST /api/entities/{id}/doc/render` → `{"doc":str}`

### Tasks (async connector runs)
- `POST /api/cases/{id}/tasks` `{kind, target?, params?}` → 202 `{"task":T}`
  - `hub_scan` — params `{scan:"username|phone|email|image", value?}` (value
    defaults to target entity label). Runs hub multi-scan, ingests every hit
    as entity + relation (`uses`/`resolves_to`) + evidence + timeline.
  - `flowsint_enrich` — params `{enrichers:["subdomain_discovery",...]}` on
    target entity; ingests produced nodes/edges.
  - `openplanter_research` — params `{question}`. Spawns CLI agent in the
    case workspace; ingests artifacts as documents/evidence + extracted
    entities (email/phone/domain/ip/wallet/username regex sweep).
  - `archon_ground` — params `{question, sandbox?, top_k?}`. Ask archon,
    attach answer+citations as evidence + doc "Sources" + timeline.
  - `web_pivots` — params `{}` → builds search-launcher set for target,
    stored as note entity data + timeline.
- `GET  /api/tasks?case_id=` → `{"tasks":[T...]}` (newest first, cap 100)
- `GET  /api/tasks/{id}` → `{"task":T}`
- `GET  /api/tasks/{id}/stream` → SSE `{"task_id","level","msg","ts"}` lines
Task JSON: `{"id":"t_1","case_id":"c_1","kind":"hub_scan","status":"queued|running|done|error","target":null,"params":{},"created_at":...,"finished_at":null,"summary":"14 sites hit","ingested":{"entities":12,"relations":14,"evidence":14},"logs":[...]}`

### Reports / interchange / laya / archon
- `GET /api/cases/{id}/dossier.md` → text/markdown
- `GET /api/cases/{id}/brief.md` → text/markdown
- `GET /api/cases/{id}/export.json` → full case (nodes, edges, evidence, timeline, docs)
- `POST /api/cases/import` — accepts nexus export.json shape → 201 new case
- `POST /api/triage` `{case_id}` → `{"ranked":[{"entity_id","score","verdict","reason"}...],"engine":"jev|laya|heuristic"}` (alias `POST /api/laya/triage` kept)
- `POST /api/entities/{id}/merge/preview` `{other:"e_2"}` → `{"similarity":f64,
  "verdict":"same|related|distinct","confidence":f64,"engine":"jev|heuristic"}`
  — similarity via strsim; when in the 0.82–0.97 gray band and jev is
  configured, ask Jev one `noul` question ("these two records refer to the
  same real-world entity") and use its calibrated confidence. Policy:
  confidence ≥0.8 → verdict same (UI offers auto-merge), 0.5–0.8 → related
  (suggest review), <0.5 → distinct.
- `GET  /api/archon/status` → `{"up":bool,"sandboxes":[...]}` (empty list when down)
- `POST /api/archon/ask` `{question, case_id?, sandbox?, top_k?}` → `{"ok":true,"answer","citations":[...],"evidence":V}` (evidence attached when case_id given)
- `POST /api/archon/push_case` `{case_id, sandbox?}` → creates/refreshes sandbox
  named `nexus-<slug>`, pushes dossier.md as canon doc → `{"ok":true,"sandbox":{...}}`

### Server bootstrap (nexus-server lib — REQUIRED signature for tauri)
```rust
pub struct Config { pub db_path: PathBuf, pub hub_url: String, pub hub_token: Option<String>,
    pub flowsint_url: Option<String>, pub openplanter_bin: Option<String>,
    pub workspaces: PathBuf, pub archon_url: String, pub laya_url: Option<String>,
    pub laya_log: Option<PathBuf>, pub web_dist: Option<PathBuf> }
impl Config { pub fn from_env() -> Self }
pub struct AppState { /* core Store + config + task engine + bus */ }
pub async fn init_state(cfg: Config) -> AppState;
pub fn build_router(state: AppState) -> axum::Router; // static UI mounted LAST (fallback "/" -> index.html)
pub async fn serve(cfg: Config) -> anyhow::Result<()>; // bind 127.0.0.1:NEXUS_PORT, convenience for CLI
```

## 5. Connector contracts (nexus-server/connectors/)

- **hub.rs** — `POST {NEXUS_HUB_URL}/api/scan` `{type:"multi",value,tools:[...],options:{}}`
  w/ `Authorization: Bearer $OSINT_HUB_TOKEN` (omit when unset) → 202
  `{job_id}`; poll `GET /api/jobs/{id}` every 2s until `status` ∈
  `done|error` (timeout 15min). Job snapshot shape:
  `{id,status,created,finished,results:[{qtype,tool,status,found:[{site,url}],info:[{k,v}],error}...],events:[...]}`.
  Digest → per hit: entity(etype by qtype: username/phone/email) + relation
  to the scanned target (`uses`) + evidence{tool_result,url,site}.
- **flowsint.rs** — base `FLOWSINT_URL`. `up` = `GET {base}/api/health`
  responds ok (any 2xx). Enricher run: `POST {base}/api/enrichers/run`
  `{type, value, enricher}` — VERIFY exact route/shape against flowsint docs
  (https://github.com/reconurge/flowsint/tree/main/docs) at build time; if
  the API differs, adapt here and note in `nexus/docs/INTEGRATIONS.md`.
  Ingest: map returned nodes to nexus etypes (their Domain/IP/ASN/CIDR/
  Individual/Organization/Email/Phone/Website/Social/Credentials/Wallet set),
  edges to canonical rels, keep provenance `flowsint:<enricher>`.
- **openplanter.rs** — run `{bin} --task {question} --workspace {dir}
  --headless --no-tui` (add `--provider ollama` when
  `OPENPLANTER_PROVIDER=ollama`), cwd = fresh per-task dir under
  `OPENPLANTER_WORKSPACES/<case_slug>/<task_id>/`, kill after 20min. After
  exit: walk dir for files written during run (md/txt/json/csv, <2MB);
  each → evidence{kind:openplanter_finding} + document entity; regex-sweep
  text for emails/phones/domains/ips/usernames/wallets → candidate entities
  linked `derived_from` the task document. stdout tail → task logs.
- **archon.rs** — exact shapes from tulpa pillars.py (verified 2026-09):
  `GET  {ARCHON_URL}/api/sandboxes` → 200 list[{id,name,library_id}]
  `POST {ARCHON_URL}/api/sandboxes` `{"name":...}` → 200/201 {id,...}
  `POST {ARCHON_URL}/api/ask` `{"question","sandbox_id"?,"libraries"?,"top_k"?}`
    → 200 `{answer,citations,libraries_searched,backend,found}`
  Down/unreachable → `{"ok":false,"detail"}`; NEVER hard-fail a case op on
  archon being down (degrade, mirror tulpa behavior).
- **laya.rs** — when `LAYA_URL` set: `POST {LAYA_URL}/predict`
  `{domain:"osint-grading", state, questions}` (typed decision; triage
  question set: `{"lead_value": {"type":"score", ...}}`) — tolerate failure →
  heuristic. Heuristic scorer: pinned boost, etype weights
  (person>username>email>phone>domain>ip>other), evidence-count log boost,
  connector-availability boost. Log every decision (engine, inputs, output)
  JSONL to `NEXUS_LAYA_LOG` when set → feeds future `nexus-triage` fine-tune.
- **jev.rs** — Jev (TypeSafe AI) calibrated decisions. REST (verified from
  typesafe_jev/mcp_server.py): `POST {JEV_URL}` JSON
  `{"state": str, "model": "jev-latest", "questions": {name: {"type":
  "choice"|"score"|"noul", "instructions": str, "criteria"?: dict|list}}}`
  with `Authorization: Bearer $JEV_API_KEY` → 200 dict keyed by question
  name, each {answer, probabilities, confidence}. Health = key present + one
  live `noul` ping ("alive": state non-empty), 5s timeout. Uses:
  (a) triage — per-entity `score` question "how valuable is this entity as
  an investigation lead?" with state = compact digest (etype, label,
  evidence count, relation count, top connection labels); top-25 entities,
  futures buffer_unordered(8), 30s total budget; (b) merge preview (§4);
  (c) hub-hit noise gate — optional `noul` "is this a genuine profile
  match?" when a sherlock hit is low-confidence; FLAG (evidence
  confidence 0.5, note in snippet) never DROP; (d) claim gate for brief
  gray-band claims. Unset key / any error → fall through the engine chain
  silently (log at debug). Batch multiple questions against one state in a
  single call when possible.
- **websearch.rs** — pure URL builders (no scraping): google/ddg/bing/news/
  wikipedia/reddit/youtube + site: dorks per etype + username guesses.

## 6. Web UI (webui/ — Vite + TypeScript + Cytoscape.js)

- `npm run build` → `webui/dist/` (server serves it; tauri webview loads server URL).
- Stack: vite + typescript, cytoscape + cytoscape-fcose (dagre/cose fallback
  if fcose troublesome), no CSS framework — hand-rolled, matching the hub's
  "steddi engineering-drawing" aesthetic: dark blueprint grid bg, film grain,
  mono type (ui-monospace stack), 1px hairlines, purple accent #a78bfa,
  amber warnings; panels with corner ticks. Zero external runtime requests.
- Panes: left case rail (cases, connector health dots, new case); center
  graph canvas (etype color map, label+, click select, shift-click = path
  mode A→B, search + type filter chips, layout buttons force/grid/circle);
  right inspector drawer (entity fields, relations list, evidence list,
  wiki doc editor w/ render button, action buttons: hub scan / flowsint
  enrich / openplanter research / archon ground / pivots); bottom console
  (tasks + SSE logs); top bar (case name, tabs Graph|Timeline|Analysis|Story,
  settings gear). Analysis tab: centrality+pagerank tables, communities,
  pivot suggestion cards w/ one-click run. Story tab: dossier.md + brief.md
  rendered (tiny in-house md renderer), copy/download, "push to archon".
- Settings modal writes to `localStorage: nexus_settings` `{hub_url,hub_token,
  flowsint_url,openplanter_bin,archon_url,laya_url,jev_url,jev_api_key}` →
  `POST /api/admin/config` (server merges runtime-overridable connector
  settings; env stays default). → so ADD `POST /api/admin/config` accepting a
  partial Config JSON (subset fields above, db path NOT overridable),
  persists the overlay to `NEXUS_CONFIG` (0600, reloaded at boot), returns
  effective config with `jev_api_key`/`hub_token` REDACTED.
- All fetches via `api()` wrapper (same-origin `/api`).

## 7. Tauri shell (nexus-tauri)

Tauri 2, app id `com.qalarc.nexus`, window 1440x900 min 1100x700, title
"NEXUS — qalarc investigation workbench". `main.rs` (tokio runtime):
`init_state(Config::from_env())` → bind `127.0.0.1:0` via
`axum::serve` on `build_router(state)` → get port → open WebviewWindow at
`http://127.0.0.1:{port}`. `tauri.conf.json` frontendDist points to a
placeholder (`src/`) since UI is server-served; devUrl unused. Cargo deps:
`tauri = { version = "2", features = [] }`, `tokio`, `nexus-server` (path).
Build: `cargo build -p nexus-tauri` (CLI `nexus-tauri` runs server-only with
`--headless` flag printing the URL — usable without webview libs).
Provide `nexus/crates/nexus-tauri/README.md` with `cargo tauri` dev/build
instructions (npx @tauri-apps/cli).

## 8. Done criteria per zone
- core: `cargo test -p nexus-core` green (≥15 unit tests: resolve dedup,
  merge, path, pagerank sanity, communities, doc render keeps prose, dossier
  contains sections, pivot ranking).
- server: `cargo check -p nexus-server` clean; `cargo test -p nexus-server`
  green (router tests w/ in-memory temp DB: case CRUD, entity dedup, task
  hub_scan against a **mock hub** (axum test server returning fixture job) —
  no real network in tests).
- ui: `npm install && npm run build` green, `dist/index.html` self-contained
  (no CDN), tsc strict no errors.
- tauri: `cargo check -p nexus-tauri` green (webview libs present on this
  machine).
