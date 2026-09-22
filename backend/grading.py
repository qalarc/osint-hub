"""grading.py — post-scan Jev grading hook (Laya_integrations protocol §1, strategy 2).

After a scan job's tools finish, every FOUND hit is graded by Jev cloud (evidence tier)
and appended as a protocol-format row to the osint-grading dataset. This is the
"agent-logged / live-traffic" collection strategy: real scan states, Jev as teacher,
zero extra human work. Later, user keep/discard outcomes upgrade the labels.

Design constraints:
- NON-FATAL: a Jev outage must never affect scan results (every failure swallowed+logged).
- PII-pseudonymised: the scan target (username/phone/email) never lands in the dataset —
  replaced by t-<sha1-10>. Site, URL shape, status and signals carry the training signal.
- Capped: max 20 graded hits per job (grading adds ~1s/hit to the job tail).

Enable/disable: env OSINT_GRADE=0 disables. Dataset path: env OSINT_GRADE_DATASET.
"""

from __future__ import annotations

import hashlib
import json
import os
import time
import urllib.error
import urllib.request
from pathlib import Path

API = "https://api.typesafe.ai/v1/systemone"
SECRETS = Path.home() / ".secrets" / "typesafe.env"
DEFAULT_DATASET = (
    Path.home() / "projects/GLM_projects/Laya_integrations/datasets/osint-grading.jsonl"
)
FALLBACK_DATASET = Path(__file__).resolve().parent.parent / "data" / "graded.jsonl"
MAX_PER_JOB = 20

# Same behavior notes the trainer uses (scripts/jev_shadow_label.py OSINT_SITES) so
# hook-collected states match the training distribution. Extended with common sites.
PLATFORM_BEHAVIOR = {
    "github": "Platform reliably 404s missing profiles",
    "reddit": "Platform reliably 404s missing profiles",
    "mastodon": "Platform reliably 404s missing profiles",
    "tumblr": "Platform reliably 404s missing profiles",
    "soundcloud": "Platform reliably 404s missing profiles",
    "vimeo": "Platform reliably 404s missing profiles",
    "flickr": "Platform reliably 404s missing profiles",
    "about.me": "Platform reliably 404s missing profiles",
    "pinterest": "Platform returns 200 for missing profiles sometimes",
    "facebook": "Platform returns a login wall regardless of profile existence",
    "medium": "Platform returns 200 with a 'stories not found' page sometimes",
    "deviantart": "Platform returns 200 for deactivated profiles sometimes",
    "instagram": "Platform returns a login wall regardless of profile existence",
    "twitter": "Platform returns 200 for suspended profiles sometimes",
    "x": "Platform returns 200 for suspended profiles sometimes",
    "tiktok": "Platform returns 200 for missing profiles sometimes",
    "telegram": "Platform behavior varies for missing profiles",
    "last.fm": "Platform reliably 404s missing profiles",
    "gravatar": "Platform reliably 404s missing profiles",
    "keybase": "Platform reliably 404s missing profiles",
    "steam": "Platform returns 200 for missing profiles sometimes",
    "spotify": "Platform reliably 404s missing profiles",
}
BEHAVIOR_FALLBACK = "Platform behavior unverified for this site"

Q_TIER = {
    "type": "choice",
    "instructions": "Evidence tier",
    "criteria": {
        "verified": "near-certain",
        "likely": "probably them",
        "weak": "coincidence possible",
        "junk": "false positive",
    },
}


def _key() -> str | None:
    k = os.environ.get("TYPESAFE_API_KEY")
    if k:
        return k
    try:
        for line in SECRETS.read_text().splitlines():
            if "=" in line and not line.strip().startswith("#"):
                k, v = line.split("=", 1)
                if k.strip() == "TYPESAFE_API_KEY":
                    return v.strip().strip('"')
    except OSError:
        pass
    return None


def _pseudonym(value: str) -> str:
    return "t-" + hashlib.sha1(value.encode()).hexdigest()[:10]


def _deidentify(url: str, target_raw: str, pseudonym: str) -> str:
    """Replace the target handle inside the URL with the pseudonym, keep URL shape."""
    if target_raw and len(target_raw) > 2:
        return url.replace(target_raw, pseudonym)
    return url


def _build_state(
    item: dict, tool_id: str, target_raw: str, pseudonym: str
) -> str | None:
    site = str(item.get("site", "")).strip()
    url = item.get("url")
    if not site:
        return None
    url_out = _deidentify(str(url), target_raw, pseudonym) if url else None
    behavior = None
    for k, v in PLATFORM_BEHAVIOR.items():
        if k in site.lower():
            behavior = v
            break
    behavior = behavior or BEHAVIOR_FALLBACK
    status = "200" if url_out else "unknown"
    url_part = f"url={url_out} " if url_out else ""
    return (
        f"site={site} {url_part}status={status}. Found via {tool_id} (hub scan). "
        f"{behavior}."
    )


def _jev(state: str, key: str, timeout: int = 25) -> dict | None:
    body = json.dumps(
        {"model": "jev-1.13.0", "state": state, "questions": {"tier": Q_TIER}}
    ).encode()
    req = urllib.request.Request(
        API,
        data=body,
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {key}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read())
    except (urllib.error.URLError, TimeoutError, OSError, ValueError):
        return None


def _dataset_path() -> Path:
    p = os.environ.get("OSINT_GRADE_DATASET")
    if p:
        return Path(p)
    return DEFAULT_DATASET if DEFAULT_DATASET.parent.is_dir() else FALLBACK_DATASET


def grade_job(job) -> int:
    """Grade a finished job's found hits. Returns rows written. Never raises."""
    key = _key()
    if key is None:
        return 0

    rows = 0
    out_path = _dataset_path()
    ts = time.strftime("%Y-%m-%dT%H:%M:%S")
    for res in job.results:
        if rows >= MAX_PER_JOB:
            break
        if res.get("status") != "done":
            continue
        target_raw = str(res.get("qvalue") or "")
        pseudonym = _pseudonym(target_raw) if len(target_raw) > 2 else "t-anon"
        for item in res.get("found", []):
            if rows >= MAX_PER_JOB:
                break
            state = _build_state(item, res["tool"], target_raw, pseudonym)
            if state is None:
                continue
            ans = _jev(state, key)
            if not ans:
                continue
            answers = ans.get("answers") or {}
            if "tier" not in answers:
                continue
            row = {
                "state": state,
                "questions": {"tier": Q_TIER},
                "answers": {"tier": answers["tier"]},
                "source": "jev-live",
                "ts": ts,
                "job_id": job.id,
            }
            try:
                out_path.parent.mkdir(parents=True, exist_ok=True)
                with out_path.open("a") as f:
                    f.write(json.dumps(row, ensure_ascii=False) + "\n")
                rows += 1
            except OSError:
                return rows
    if rows:
        job.emit(
            "log",
            tool="grader",
            qtype=None,
            line=f"[hub] Jev-graded {rows} hits -> dataset",
        )
    return rows
