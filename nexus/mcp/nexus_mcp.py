#!/usr/bin/env python
"""Nexus MCP server — investigation graph operations for the qalarc agent fleet.

Exposes the NEXUS investigation workbench (nexus-server REST API) to MCP
clients: archon agents, chanalyse article pipeline, or any MCP host.

Tools:
  nexus_status            health + connector states
  nexus_cases             list investigation cases
  nexus_case_create       new case
  nexus_add_entity        add (auto-dedup) an entity to a case
  nexus_link              typed relation between two entities
  nexus_graph             compact node+edge view of a case
  nexus_find_connections  shortest path between two entities
  nexus_pivots            suggested next actions for an entity/case
  nexus_task_run          async connector run (hub_scan / flowsint_enrich /
                          openplanter_research / archon_ground / web_pivots)
  nexus_task_status       poll a task (or latest task of a case)
  nexus_triage            ranked leads (engine chain: jev -> laya -> heuristic)
  nexus_merge_preview     "same entity?" gate (jev-calibrated in the gray band)
  nexus_dossier           full markdown dossier for a case
  nexus_brief             journalism article brief (claim ledger, ledes)
  nexus_archon_ask        canon grounding via archon RAG (+ evidence attach)
  nexus_archon_push_case  create/refresh a nexus-<slug> archon sandbox

Env: NEXUS_URL (default http://127.0.0.1:8801), NEXUS_TOKEN (optional bearer).
Run:  <python-with-mcp-pkg> nexus_mcp.py   (stdio)

Authorized-use OSINT only — see PRIVACY.md at the repo root.
"""

import json
import os
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from mcp.server.mcpserver import MCPServer

mcp = MCPServer(
    name="nexus",
    instructions=(
        "NEXUS = qalarc investigation graph workbench. Use for OSINT case "
        "management: entities, typed relations, evidence, connector runs "
        "(qalarc hub scanners, flowsint enrichers, OpenPlanter recursive "
        "research, archon canon grounding), graph analysis, lead triage "
        "(jev/laya-calibrated) and journalism exports (dossier, article "
        "brief). Start with nexus_status + nexus_cases. All decisions are "
        "evidence-backed; respect authorized-use-only ethics."
    ),
)

BASE = os.environ.get("NEXUS_URL", "http://127.0.0.1:8801").rstrip("/")
TOKEN = os.environ.get("NEXUS_TOKEN", "")


def _req(
    method: str, path: str, body: dict | None = None, timeout: float = 30.0
) -> dict:
    url = BASE + path
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if TOKEN:
        req.add_header("Authorization", f"Bearer {TOKEN}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read().decode()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        try:
            return {"error": f"HTTP {exc.code}", "detail": exc.read().decode()[:400]}
        except Exception:
            return {"error": f"HTTP {exc.code}"}
    except Exception as exc:  # unreachable server etc — data, not a crash
        return {"error": f"nexus unreachable ({exc.__class__.__name__}: {exc})"}


def _text(method: str, path: str, timeout: float = 30.0) -> str:
    req = urllib.request.Request(BASE + path, method=method)
    if TOKEN:
        req.add_header("Authorization", f"Bearer {TOKEN}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.read().decode()
    except Exception as exc:
        return f"error: {exc.__class__.__name__}: {exc}"


# ---------------------------------------------------------------- status ---


@mcp.tool()
def nexus_status() -> dict:
    """Nexus server health + connector states (hub, flowsint, openplanter,
    archon, laya, jev). First call to make."""
    return _req("GET", "/api/health")


# ----------------------------------------------------------------- cases ---


@mcp.tool()
def nexus_cases() -> dict:
    """List investigation cases with stats."""
    return _req("GET", "/api/cases")


@mcp.tool()
def nexus_case_create(
    name: str, notes: str = "", tags: list[str] | None = None
) -> dict:
    """Create an investigation case. Returns the case incl. id used by every
    other tool."""
    return _req(
        "POST", "/api/cases", {"name": name, "notes": notes, "tags": tags or []}
    )


# -------------------------------------------------------------- entities ---


@mcp.tool()
def nexus_add_entity(
    case_id: str,
    etype: str,
    label: str,
    data: dict | None = None,
    confidence: float = 1.0,
) -> dict:
    """Add an entity to a case. etype: person|org|username|alias|email|phone|
    domain|subdomain|ip|asn|cidr|website|social_profile|wallet|transaction|
    image|document|article|event|location|topic|note. Auto-dedups within the
    case (deduplicated=true means merged with an existing entity)."""
    return _req(
        "POST",
        f"/api/cases/{case_id}/entities",
        {"etype": etype, "label": label, "data": data or {}, "confidence": confidence},
    )


@mcp.tool()
def nexus_link(
    case_id: str,
    src: str,
    dst: str,
    rel: str,
    confidence: float = 0.9,
    source: str = "mcp",
    evidence: dict | None = None,
) -> dict:
    """Create a typed relation src --rel--> dst. Canonical rels: uses, owns,
    resolves_to, subdomain_of, registered_to, mentions, contacted, same_as,
    paid_to, located_in, member_of, works_at, sourced_from, derived_from,
    related_to, posted_on. evidence = {title, url, snippet}."""
    return _req(
        "POST",
        "/api/relations",
        {
            "case_id": case_id,
            "src": src,
            "dst": dst,
            "rel": rel,
            "confidence": confidence,
            "source": source,
            "evidence": evidence,
        },
    )


@mcp.tool()
def nexus_graph(case_id: str) -> dict:
    """Compact graph view of a case: nodes (id, etype, label, pinned) + edges
    (src, dst, rel, source). For deeper analysis use nexus_pivots /
    nexus_find_connections / nexus_triage."""
    out = _req("GET", f"/api/cases/{case_id}/graph")
    if "nodes" in out:
        out["nodes"] = [
            {k: n.get(k) for k in ("id", "etype", "label", "pinned")}
            for n in out["nodes"]
        ]
        out["edges"] = [
            {k: e.get(k) for k in ("src", "dst", "rel", "source")}
            for e in out.get("edges", [])
        ]
    return out


@mcp.tool()
def nexus_find_connections(case_id: str, a: str, b: str) -> dict:
    """Shortest connection path between two entity ids (BFS, undirected).
    Returns {"path": [entity ids]} or null when unconnected."""
    q = urllib.parse.urlencode({"a": a, "b": b})
    return _req("GET", f"/api/cases/{case_id}/path?{q}", timeout=15.0)


@mcp.tool()
def nexus_pivots(case_id: str, entity: str = "") -> dict:
    """Suggested next investigative actions (priority-ranked, connector-gated).
    Pass entity id to focus; omit for case-wide. Actions map to nexus_task_run
    kinds: hub_scan / flowsint_enrich:<enricher> / openplanter_research /
    archon_ground."""
    q = f"?entity={urllib.parse.quote(entity)}" if entity else ""
    return _req("GET", f"/api/cases/{case_id}/pivots{q}")


# ----------------------------------------------------------------- tasks ---


@mcp.tool()
def nexus_task_run(
    case_id: str, kind: str, target: str = "", params: dict | None = None
) -> dict:
    """Launch an async connector task on a case (202 + task id). kinds:
    hub_scan {scan: username|phone|email|image, value?},
    flowsint_enrich {enrichers: [names]},
    openplanter_research {question} (recursive agent deep-dive),
    archon_ground {question, sandbox?, top_k?},
    web_pivots {}. target = entity id when the task operates on one."""
    body = {"kind": kind, "params": params or {}}
    if target:
        body["target"] = target
    return _req("POST", f"/api/cases/{case_id}/tasks", body)


@mcp.tool()
def nexus_task_status(task_id: str, case_id: str = "") -> dict:
    """Poll a task; with case_id and no task_id returns that case's tasks
    (newest first)."""
    if task_id:
        return _req("GET", f"/api/tasks/{task_id}")
    return _req("GET", f"/api/tasks?case_id={urllib.parse.quote(case_id)}")


# ------------------------------------------------------- decisions / gate ---


@mcp.tool()
def nexus_triage(case_id: str) -> dict:
    """Rank case entities as investigation leads. engine chain:
    jev (calibrated remote) -> laya (local fine-tuned) -> heuristic."""
    return _req("POST", "/api/triage", {"case_id": case_id}, timeout=60.0)


@mcp.tool()
def nexus_merge_preview(entity_id: str, other: str) -> dict:
    """'Same real-world entity?' gate before merging: similarity + verdict
    (same|related|distinct) + confidence; jev-calibrated in the gray band."""
    return _req("POST", f"/api/entities/{entity_id}/merge/preview", {"other": other})


# ---------------------------------------------------------- journalism -----


@mcp.tool()
def nexus_dossier(case_id: str) -> str:
    """Full markdown dossier for a case: executive summary, key entities,
    key connections, communities, timeline, evidence appendix, sources."""
    return _text("GET", f"/api/cases/{case_id}/dossier.md")


@mcp.tool()
def nexus_brief(case_id: str) -> str:
    """Journalism article brief: working lede angles, nut graf, claim ledger
    (claim vs evidence status), open questions, source list. Feed this to the
    article writer (chanalyse)."""
    return _text("GET", f"/api/cases/{case_id}/brief.md")


# ---------------------------------------------------------------- archon ---


@mcp.tool()
def nexus_archon_ask(
    question: str, case_id: str = "", sandbox: str = "", top_k: int = 6
) -> dict:
    """Ground a question against the archon canon/RAG pillar. With case_id the
    answer + citations are attached as case evidence."""
    body: dict = {"question": question, "top_k": top_k}
    if case_id:
        body["case_id"] = case_id
    if sandbox:
        body["sandbox"] = sandbox
    return _req("POST", "/api/archon/ask", body, timeout=45.0)


@mcp.tool()
def nexus_archon_push_case(case_id: str) -> dict:
    """Create/refresh the nexus-<slug> archon sandbox for a case (grounding
    corpus for article writing)."""
    return _req("POST", "/api/archon/push_case", {"case_id": case_id}, timeout=45.0)


if __name__ == "__main__":
    import asyncio

    asyncio.run(mcp.run_stdio_async())
