#!/usr/bin/env python
"""OSINT Hub MCP server — agent-facing OSINT tools (stdio transport).

Auto-resolves which hub to talk to:
  1. $OSINT_HUB_URL (explicit override)
  2. local full backend  http://127.0.0.1:8799   (sherlock/maigret/phoneinfoga/holehe/revimg)
  3. live lite engine    https://osint.qalarc.com (edge username checks + phone pivots)

Tools:
  osint_status()               – mode + capabilities of the connected hub
  osint_case()                 – one call: username + phone + email + image URL
  osint_username/phone/email() – focused single-subject scans
  osint_image_search()         – reverse-image discovery: pages where a photo appears
  osint_dating()               – dating-platform scan (maigret, full backend only)
  osint_search_pivots()        – instant topic/person search launchers (no hub needed)

Run:    backend/.venv/bin/python mcp/server.py
Env:    OSINT_HUB_URL, OSINT_HUB_TOKEN (if the hub requires auth)
Results are heuristic — agents should verify before acting on findings.
"""

from __future__ import annotations

import asyncio
import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request

from mcp.server.mcpserver import MCPServer

mcp = MCPServer(
    name="osint-hub",
    instructions=(
        "Qalarc OSINT tools. Call osint_status() first to see the connected mode. "
        "osint_case() accepts any mix of username / phone / email / image_url and returns "
        "per-subject findings. osint_dating() sweeps dating platforms (full backend only). "
        "osint_search_pivots(topic) gives instant search launchers with no hub required. "
        "Findings are heuristic (false positives/negatives happen) — verify before acting."
    ),
)

TOKEN = os.environ.get("OSINT_HUB_TOKEN", "")
_CANDIDATES: list[str] = []
if os.environ.get("OSINT_HUB_URL"):
    _CANDIDATES.append(os.environ["OSINT_HUB_URL"].rstrip("/"))
_CANDIDATES += ["http://127.0.0.1:8799", "https://osint.qalarc.com"]

DATING_DOMAINS = {
    "tinder.com",
    "badoo.com",
    "meetme.com",
    "adultfriendfinder.com",
    "okcupid.com",
    "pof.com",
    "match.com",
    "hinge.co",
    "bumble.com",
    "grindr.com",
    "feeld.co",
    "zoosk.com",
    "ashleymadison.com",
    "eharmony.com",
}

_ALL_TOOLS = ["sherlock", "maigret", "phoneinfoga", "ignorant", "holehe", "revimg"]
_cache: dict = {"base": None, "mode": None}


# ---------------------------------------------------------------- hub client
def _http(method: str, url: str, body=None, timeout: float = 45):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    # browser-ish UA: Cloudflare bot protection 403s the default Python-urllib UA
    req.add_header(
        "User-Agent",
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36 qalarc-osint-mcp/1.0",
    )
    if TOKEN:
        req.add_header("Authorization", f"Bearer {TOKEN}")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        raw = r.read().decode()
        return json.loads(raw) if raw else {}


def _health(base: str):
    try:
        h = _http("GET", base + "/api/health", timeout=6)
        return h if h.get("ok") else None
    except Exception:
        return None


def _resolve():
    """Return (base_url, mode) — 'full' or 'lite' — or (None, None)."""
    if _cache["base"]:
        h = _health(_cache["base"])
        if h:
            _cache["mode"] = h.get("mode", "full")
            return _cache["base"], _cache["mode"]
        _cache.update(base=None, mode=None)
    for base in _CANDIDATES:
        h = _health(base)
        if h:
            _cache.update(base=base, mode=h.get("mode", "full"))
            return base, _cache["mode"]
    return None, None


def _wait_job(base: str, job_id: str, timeout: float):
    deadline = time.time() + timeout
    while time.time() < deadline:
        j = _http("GET", f"{base}/api/jobs/{job_id}")
        if j.get("status") in ("done", "error", "timeout"):
            return j
        time.sleep(2.5)
    return {"id": job_id, "status": "timeout", "results": []}


def _digest(job: dict, cap: int = 60) -> dict:
    out = {"job": job.get("id"), "status": job.get("status"), "subjects": {}}
    for r in job.get("results", []):
        qt = r.get("qtype", "?")
        subject = out["subjects"].setdefault(
            qt, {"value": r.get("qvalue"), "tools": {}}
        )
        t = subject["tools"].setdefault(r["tool"], {"status": r.get("status")})
        if r.get("error"):
            t["error"] = r["error"]
        found = r.get("found") or []
        if found:
            t["found_count"] = len(found)
            t["found"] = [
                {"site": f.get("site"), **({"url": f["url"]} if f.get("url") else {})}
                for f in found[:cap]
            ]
            if len(found) > cap:
                t["truncated"] = f"+{len(found) - cap} more"
        info = r.get("info") or []
        if info:
            facts = [i for i in info if i.get("value")]
            pivots = [i.get("url") for i in info if i.get("url")][:20]
            if facts:
                t["facts"] = {i["key"]: i["value"] for i in facts}
            if pivots:
                t["search_pivots"] = pivots
    return out


def _full_scan(value: dict, max_wait: float) -> dict:
    base, mode = _resolve()
    if base is None:
        return {
            "error": "no OSINT hub reachable — start the backend (scripts/run.sh) or set OSINT_HUB_URL"
        }
    if mode == "lite":
        return {
            "error": "connected hub is lite mode (edge) — only username checks and phone pivots are available; start the full backend for this scan",
            "hub": base,
        }
    r = _http(
        "POST",
        base + "/api/scan",
        {
            "type": "multi",
            "value": value,
            "tools": _ALL_TOOLS,
            "options": {"timeout": 10},
        },
    )
    if "job_id" not in r:
        return {"error": "scan rejected", "detail": r}
    job = _wait_job(base, r["job_id"], max_wait)
    d = _digest(job)
    d["hub"] = base
    return d


def _lite_or_full_case(
    username=None, phone=None, email=None, image_url=None, max_wait=180.0
) -> dict:
    value = {
        k: v
        for k, v in {
            "username": username,
            "phone": phone,
            "email": email,
            "image": image_url,
        }.items()
        if v
    }
    if not value:
        return {"error": "provide at least one of: username, phone, email, image_url"}
    base, mode = _resolve()
    if base is None:
        return {
            "error": "no OSINT hub reachable — start the backend (scripts/run.sh) or set OSINT_HUB_URL"
        }
    if mode == "full":
        return _full_scan(value, max_wait)
    # ---- lite mode: synchronous edge endpoints ----
    out = {"hub": base, "mode": "lite", "subjects": {}}
    if value.get("username"):
        try:
            r = _http(
                "POST",
                base + "/api/lite/check",
                {"value": value["username"]},
                timeout=60,
            )
            out["subjects"]["username"] = {
                "value": value["username"],
                "tools": {
                    "cloudcheck": {
                        "status": "done",
                        "found_count": len(r.get("found", [])),
                        "found": r.get("found", [])[:60],
                    }
                },
            }
        except Exception as exc:
            out["subjects"]["username"] = {"error": str(exc)[:200]}
    if value.get("phone"):
        try:
            r = _http(
                "POST", base + "/api/lite/pivots", {"value": value["phone"]}, timeout=30
            )
            info = r.get("info", [])
            out["subjects"]["phone"] = {
                "value": value["phone"],
                "tools": {
                    "pivots": {
                        "status": "done",
                        "facts": {i["key"]: i["value"] for i in info if i.get("value")},
                        "search_pivots": [i.get("url") for i in info if i.get("url")][
                            :20
                        ],
                    }
                },
            }
        except Exception as exc:
            out["subjects"]["phone"] = {"error": str(exc)[:200]}
    if value.get("email"):
        out["subjects"]["email"] = {
            "error": "email checks (holehe) need the full backend"
        }
    if value.get("image"):
        out["subjects"]["image"] = {
            "error": "image discovery (revimg) needs the full backend"
        }
    return out


# ---------------------------------------------------------------- local tools
def _pivot_urls(topic: str, kind: str) -> dict:
    q = urllib.parse.quote_plus(topic)
    quoted = urllib.parse.quote_plus(f'"{topic}"')
    links = [
        ("google", f"https://www.google.com/search?q={q}"),
        ("bing", f"https://www.bing.com/search?q={q}"),
        ("duckduckgo", f"https://duckduckgo.com/?q={q}"),
        ("google-news", f"https://news.google.com/search?q={q}"),
        ("wikipedia", f"https://en.wikipedia.org/w/index.php?search={q}"),
        ("reddit", f"https://www.google.com/search?q=site%3Areddit.com+{q}"),
        ("youtube", f"https://www.youtube.com/results?search_query={q}"),
    ]
    if kind == "person":
        socials = [
            "facebook.com",
            "instagram.com",
            "linkedin.com",
            "x.com",
            "tiktok.com",
            "tinder.com/@{u}",
        ]
        person = (
            topic.split()[0] if " " not in topic else topic.split()[-1]
        )  # last name guess
        username_guess = "".join(c for c in topic.lower() if c.isalnum())[:20]
        for s in socials[:5]:
            links.append(
                (
                    f"google-site:{s}",
                    f"https://www.google.com/search?q={quoted}+site%3A{s}",
                )
            )
        links += [
            ("exact-phrase", f"https://www.google.com/search?q={quoted}"),
            (
                "username-guess",
                f"https://osint.qalarc.com/#case (scan username: {username_guess})",
            ),
            ("tinder-handle-guess", f"https://tinder.com/@{username_guess}"),
            (
                "maigret-dating",
                "ask agent: osint_dating(username) with the full backend",
            ),
        ]
    return {
        "topic": topic,
        "kind": kind,
        "pivots": [{"engine": e, "url": u} for e, u in links],
    }


# ---------------------------------------------------------------- MCP tools
@mcp.tool()
def osint_status() -> dict:
    """Which OSINT hub is connected: base URL, mode (full/lite), installed tools."""
    base, mode = _resolve()
    if base is None:
        return {
            "connected": False,
            "hint": "start the backend (scripts/run.sh) or set OSINT_HUB_URL; lite fallback https://osint.qalarc.com",
        }
    try:
        tools = {
            t["id"]: t["installed"]
            for t in _http("GET", base + "/api/tools").get("tools", [])
        }
    except Exception:
        tools = {}
    return {"connected": True, "hub": base, "mode": mode, "tools": tools}


@mcp.tool()
def osint_case(
    username: str | None = None,
    phone: str | None = None,
    email: str | None = None,
    image_url: str | None = None,
    max_wait: float = 180.0,
) -> dict:
    """Full case scan — any mix of username, phone, email, image URL.
    Full backend: sherlock+maigret (accounts), phoneinfoga+ignorant (phone facts),
    holehe (email registrations), revimg (pages where the image appears).
    Lite mode: username checks + phone pivots only."""
    return _lite_or_full_case(username, phone, email, image_url, max_wait)


@mcp.tool()
def osint_username(username: str, max_wait: float = 180.0) -> dict:
    """Find accounts for a username across platforms (sherlock+maigret / edge cloudcheck)."""
    return _lite_or_full_case(username=username, max_wait=max_wait)


@mcp.tool()
def osint_phone(phone: str, max_wait: float = 120.0) -> dict:
    """Phone intelligence: number facts, country/carrier (full) + search pivots; registrations (ignorant)."""
    return _lite_or_full_case(phone=phone, max_wait=max_wait)


@mcp.tool()
def osint_email(email: str, max_wait: float = 120.0) -> dict:
    """Which platforms an email is registered on (holehe — full backend only)."""
    return _lite_or_full_case(email=email, max_wait=max_wait)


@mcp.tool()
def osint_image_search(image_url: str, max_wait: float = 120.0) -> dict:
    """Reverse-image ONLINE DISCOVERY: pages where this image (or similar) appears — Yandex/Bing/TinEye. Full backend only."""
    return _lite_or_full_case(image_url=image_url, max_wait=max_wait)


@mcp.tool()
def osint_dating(username: str, max_wait: float = 300.0) -> dict:
    """Dating-platform sweep for a username (Tinder/Badoo/MeetMe/AFF/…) via maigret.
    Full backend only. Filters results to dating domains. Responsible use only."""
    base, mode = _resolve()
    if base is None or mode == "lite":
        return {"error": "dating sweeps need the full backend (maigret, 3000+ sites)"}
    r = _http(
        "POST",
        base + "/api/scan",
        {
            "type": "username",
            "value": username,
            "tools": ["maigret"],
            "options": {"timeout": 15},
        },
    )
    if "job_id" not in r:
        return {"error": "scan rejected", "detail": r}
    job = _wait_job(base, r["job_id"], max_wait)
    hits = []
    for res in job.get("results", []):
        for f in res.get("found", []):
            url = f.get("url") or ""
            try:
                host = urllib.parse.urlparse(url).hostname or ""
            except ValueError:
                host = ""
            if any(host == d or host.endswith("." + d) for d in DATING_DOMAINS):
                hits.append({"site": f.get("site"), "url": url})
    all_found = sum(len(x.get("found", [])) for x in job.get("results", []))
    return {
        "username": username,
        "status": job.get("status"),
        "total_accounts_found": all_found,
        "dating_hits": hits,
        "note": "dating-only filter; run osint_case for the full picture",
    }


@mcp.tool()
def osint_search_pivots(topic: str, kind: str = "topic") -> dict:
    """Instant search launchers for a topic or a person's name (no hub needed).
    kind='person' adds social-site scoped queries + username guesses."""
    return _pivot_urls(topic, kind)


if __name__ == "__main__":
    asyncio.run(mcp.run_stdio_async())
