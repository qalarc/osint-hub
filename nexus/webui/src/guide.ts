/**
 * Guide tab: the in-app field handbook, one long markdown document rendered
 * through md.ts (which gives h2/h3 slug ids). A sticky in-page TOC is built
 * from the generated ids. Static content — no server calls, no refresh().
 */
import { api, withBase } from './api';
import { clear, h, openModal } from './dom';
import { renderMarkdownInto } from './md';

const GUIDE_MD = `# NEXUS field guide

NEXUS is an investigation workbench for **authorized-use OSINT**: you point it
only at subjects and platforms you are permitted to inspect, and it keeps the
receipts for everything it collects. This handbook covers both layers of the
system and every surface of this UI. Reading time is about five minutes — and
the walkthrough below really is that fast in practice.

## NEXUS in one minute

Two layers, one pipeline:

- **The OSINT hub** (root app, port 8799) finds raw footprint. Give it a
  username, phone number, email address or image URL and it fans out through
  the toolchain — \`sherlock\` + \`maigret\` sweep a username across social
  platforms, \`phoneinfoga\` + \`ignorant\` produce phone intel and
  registration checks, \`holehe\` finds where an email is registered, and
  \`revimg\` discovers pages where an image appears.
- **NEXUS** (this workbench, port 8801) turns those findings into an evidence
  graph. Every hit becomes an entity; entities get typed relations; every
  claim carries provenance — which tool produced it, and the URL it came
  from. On top of the graph you get wiki docs, dossiers, article briefs, and
  a handoff path to archon for cited drafting.

Hub = collection. NEXUS = case file. Nothing is asserted without a source row
behind it.

## Your first investigation (5 minutes)

1. **① New case** — in the left rail, type a name into the "new case name…"
   field and press Enter. The case owns everything that follows.
2. **② Add a seed entity** — pick an etype in the add-entity form (defaults
   to \`username\`) and give it a label, e.g. \`janedoe\`. The node appears in
   the graph immediately.
3. **③ Hub scan** — click the node to open it in the inspector, find ACTIONS,
   press "▶ hub scan". This runs the sherlock+maigret sweep via the hub, and
   every hit is ingested automatically as entities + \`uses\` relations +
   evidence rows carrying tool and URL provenance. No copy-pasting.
4. **④ Watch it live** — the console strip along the bottom lists tasks.
   Click a row to expand its per-task SSE log stream while the sweep runs;
   ingested counts land on the task row when it finishes.
5. **⑤ Read the graph** — back in the Graph tab, nodes are colored by type
   (the chips above the graph are the color map and double as hide/show
   filters). Shift-click two nodes to highlight the shortest connection path
   between them; Esc clears it.
6. **⑥ Analyze** — the Analysis tab ranks PageRank centrality (who matters
   in this graph), groups communities (clusters of closely related entities),
   and deals pivot cards: next-best actions computed from the graph shape,
   each runnable with one click.
7. **⑦ Write it up** — in the inspector, switch to WIKI DOC and hit render.
   The skeleton auto-fills the Connections / Timeline / Sources sections from
   the graph; your prose under Summary and Open questions is preserved on
   every regeneration.
8. **⑧ Draft for publication** — the Story tab renders Dossier.md (findings
   with an evidence appendix) and an article brief: lede angles, a nut graf,
   a claim ledger where every claim is tagged Supported / Single-source /
   Unverified, and the source list behind them.
9. **⑨ Hand off** — "⇪ push case to archon" seeds a canon sandbox with the
   case, so drafting in archon stays grounded in this evidence, citations
   included.

## Connectors

Connectors are the server's outboards; their health shows as dots in the left
rail. \`up\` is green. \`down\` means configured but unreachable (red) — its
tasks fail fast with a clear error while everything else keeps working.
\`unset\` means not configured (gray): the connector's actions simply are not
offered. Degradation is graceful by design.

| connector | what it does | needs |
| --- | --- | --- |
| hub | username / phone / email / image scans via the qalarc OSINT hub (sherlock, maigret, phoneinfoga, ignorant, holehe, revimg) | hub_url, optional hub_token |
| flowsint | subdomain, WHOIS, ASN, breach and crypto enrichers | flowsint_url pointing at a running flowsint instance |
| openplanter | recursive AI agent deep-dives seeded from a node | openplanter-agent binary on PATH |
| archon | canon / RAG grounding — evidence push, cited answers | archon_url |
| laya | local fine-tuned decision model for lead triage | laya_url (empty = heuristic) |
| jev | TypeSafe calibrated decision engine (remote) | jev_url + jev_api_key |

Set FLOWSINT_URL (or the matching Settings field) to a running flowsint
instance to enable its enrichers; openplanter needs the agent binary
discoverable on PATH unless you set the path explicitly in Settings.

## Calibrated decisions (jev → laya → heuristic)

Where the workbench has to rank things — chiefly lead triage in Analysis —
it walks a fixed chain and takes the first link that answers:

1. **jev** — remote and calibrated. Set jev_url and jev_api_key in Settings
   and it works instantly, returning probabilities with honest confidence.
2. **laya** — local and trainable. Every decision it serves is appended to
   laya-decisions.jsonl, so you can fine-tune the model on your own verdicts
   and redeploy it.
3. **heuristic** — a graph-shaped fallback (centrality, degree, recency).
   Never configured, never down.

### The merge gate

The inspector's merge flow offers candidate duplicates for an entity.
Confirming one runs a same-entity check (jaro-winkler similarity on labels):
**≥ 0.8** produces an auto-merge offer, **0.5–0.8** is flagged for human
review, and **< 0.5** is treated as distinct. A confirmed merge re-points
relations, keeps a \`same_as\` edge, and writes an audit entry — nothing is
silently rewritten.

## Tabs reference

- **Graph** — the evidence map. Nodes colored by etype (chips are legend and
  filters), label search, force / grid / circle layouts, fit view.
  Shift-click two nodes for the shortest connection path.
- **Console** — task queue with status, ingested entity / relation / evidence
  counts, and expandable per-task SSE logs (the last 500 lines are kept).
- **Timeline** — case chronology. Ingested milestones land here; add manual
  events with a kind, title, optional entity link and source.
- **Analysis** — PageRank centrality table, communities, pivot cards,
  laya-ranked leads.
- **Story** — Dossier.md and the article brief, with copy / download buttons
  and push-to-archon.

## Desktop & hosting

- **Desktop app (Tauri)** — from the repository's \`nexus/\` directory:
  \`cargo run -p nexus-tauri --features desktop\`.
- **Server-only** — \`scripts/run_nexus.sh\` builds and serves UI + API on
  \`127.0.0.1:8801\`.
- **Hosting tiers** — localhost is tier zero. For a shareable URL, a quick
  tunnel: \`cloudflared tunnel --url http://127.0.0.1:8801\`. For a stable
  subdomain, run a named cloudflared tunnel and put Cloudflare Access in
  front of it. For a static showcase, host the built UI anywhere and point it
  at the server with the API-base override: open the page once with
  \`?api=<server-url>\` or set nexus_api_base in Settings. The server must
  allow the UI's origin in its CORS config for a cross-origin showcase to
  connect. If the server runs
  with NEXUS_TOKEN set, put that token in nexus_token (Settings) and every
  request — API and SSE alike — carries it as a bearer header.

## Settings & keys

The gear icon (top right) opens the settings modal. Connector fields are
stored in this browser (localStorage \`nexus_settings\`) **and** applied to
the server via POST /api/admin/config; environment variables remain the
defaults and empty fields are sent as null. Secrets are redacted in responses
— you only ever see whether they are set. The two connection fields stay in
this browser only.

| field | what it does |
| --- | --- |
| hub_url | OSINT hub endpoint (default 127.0.0.1:8799) |
| hub_token | bearer token if the hub requires one |
| flowsint_url | flowsint enrichers endpoint; unset disables them |
| openplanter_bin | openplanter-agent binary (auto-discovered on PATH) |
| archon_url | archon canon / RAG endpoint |
| laya_url | local laya decision server; empty = heuristic only |
| jev_url | TypeSafe decision engine endpoint |
| jev_api_key | TypeSafe API key — enables calibrated jev decisions |
| nexus_api_base | browser-only: where this UI finds the API (empty = same-origin) |
| nexus_token | browser-only: bearer token for NEXUS_TOKEN-gated servers |

/api/health stays open even when the token gate is on, so the rail can show
connector state without credentials.

## Ethics & privacy

This is a tool for **authorized-use OSINT**: investigate subjects you have
the right and the mandate to investigate — your own footprint, consented
research, or engagements with a written scope. Results are investigative
leads with provenance, not proof; that is exactly why the claim ledger exists
with Supported / Single-source / Unverified statuses, so journalism stays
honest about what any one source supports. Only public footprint is
collected, and only on request. Respect platform terms and local law. The
full data-handling statement lives in PRIVACY.md at the repository root.

[Read the full privacy notice →](/api/privacy.md)

---

[Read the full privacy notice →](/api/privacy.md)
`;

/**
 * In-app privacy notice: fetches /api/privacy.md (through withBase), renders
 * it with md.ts, and offers the raw markdown in a new tab.
 */
function openPrivacyModal(): void {
  const content = h('div', { class: 'md privacy-md' },
    h('div', { class: 'muted small pad6' }, 'loading privacy notice…'));
  openModal('privacy notice', (body) => {
    body.append(
      h('div', { class: 'row privacy-actions' },
        h('a', {
          class: 'btn btn-sm', href: withBase('/api/privacy.md'),
          target: '_blank', rel: 'noopener noreferrer',
          title: 'open the raw markdown in a new tab',
        }, 'raw: /api/privacy.md ↗'),
      ),
      content,
    );
  }, { wide: true });
  api.privacyMd().then((md) => {
    clear(content);
    renderMarkdownInto(content, md);
  }).catch((err) => {
    clear(content);
    content.append(h('div', { class: 'error-text small pad6' },
      String(err instanceof Error ? err.message : err)));
  });
}

export class GuidePane {
  readonly el: HTMLElement;

  constructor() {
    const md = h('div', { class: 'md guide-md' });
    renderMarkdownInto(md, GUIDE_MD);

    // privacy links (ethics section + page bottom) open the in-app modal
    for (const a of Array.from(md.querySelectorAll<HTMLAnchorElement>('a[href="/api/privacy.md"]'))) {
      a.classList.add('privacy-link');
      a.addEventListener('click', (e) => {
        e.preventDefault();
        openPrivacyModal();
      });
    }

    // sticky in-page TOC from the slug ids md.ts generates for h2/h3
    const tocLinks: HTMLElement[] = [];
    for (const hd of Array.from(md.querySelectorAll('h2[id], h3[id]'))) {
      const a = h('a', {
        class: `toc-l${hd.tagName === 'H3' ? ' toc-l3' : ''}`,
        href: `#${hd.id}`,
        title: hd.textContent ?? hd.id,
      }, hd.textContent ?? hd.id);
      a.addEventListener('click', (e) => {
        e.preventDefault();
        try { history.replaceState(null, '', `#${hd.id}`); } catch { /* file:// quirks */ }
        hd.scrollIntoView({ behavior: 'smooth', block: 'start' });
      });
      tocLinks.push(a);
    }
    const toc = h('nav', { class: 'guide-toc', 'aria-label': 'guide table of contents' },
      h('span', { class: 'panel-label' }, 'contents'),
      h('div', { class: 'guide-toc-links' }, tocLinks),
    );

    this.el = h('section', { class: 'pane', id: 'pane-guide', hidden: true },
      h('header', { class: 'pane-head' },
        h('span', { class: 'panel-label' }, 'guide'),
        h('span', { class: 'muted small' }, 'field handbook — both layers, end to end'),
      ),
      h('div', { class: 'pane-body guide-body' }, toc, md),
    );
  }
}
