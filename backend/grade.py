"""Jev grading client + evidence grading for the OSINT hub.

Grades found hits (verified/likely/weak/junk) via Jev (TypeSafe), attaches results to
job items, and logs every decision to the Laya fine-tuning dataset
(~/projects/GLM_projects/Laya_integrations/datasets/osint-grading.jsonl) —
the collect-while-working half of the fine-tuning protocol.

Env: TYPESAFE_API_KEY (read from env or ~/.secrets/typesafe.env)
"""

from __future__ import annotations

import json
import os
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ENDPOINT = "https://api.typesafe.ai/v1/systemone"
LAYA_DATASET = (
    Path.home()
    / "projects"
    / "GLM_projects"
    / "Laya_integrations"
    / "datasets"
    / "osint-grading.jsonl"
)

TIER_Q = {
    "type": "choice",
    "instructions": "Evidence tier for an OSINT report: is this found account really the searched person?",
    "criteria": {
        "verified": "Near-certain match — direct handle match or corroborated identity",
        "likely": "Probably them — plausible handle, worth listing with caveat",
        "weak": "Possible coincidence — common handle or noisy platform",
        "junk": "Likely false positive — platform returns 200 for missing profiles etc.",
    },
}


def _key() -> str:
    k = os.environ.get("TYPESAFE_API_KEY", "")
    if not k:
        f = Path.home() / ".secrets" / "typesafe.env"
        if f.is_file():
            for line in f.read_text().splitlines():
                if line.startswith("TYPESAFE_API_KEY="):
                    k = line.split("=", 1)[1].strip()
    return k


def jev_decide(state: str, questions: dict) -> dict:
    body = json.dumps(
        {"state": state, "model": "jev-latest", "questions": questions}
    ).encode()
    req = urllib.request.Request(ENDPOINT, data=body, method="POST")
    req.add_header("Authorization", f"Bearer {_key()}")
    req.add_header("Content-Type", "application/json")
    req.add_header(
        "User-Agent", "Mozilla/5.0 (X11; Linux x86_64) qalarc-osint-grade/1.0"
    )
    with urllib.request.urlopen(req, timeout=45) as r:
        return json.loads(r.read())


def _grade_one(tool: str, item: dict, value: str) -> dict | None:
    state = (
        f"OSINT scan for '{value}'. Tool={tool}. site={item.get('site')} "
        f"url={item.get('url') or 'no url'}. Found by public-page check."
    )
    try:
        r = jev_decide(state, {"tier": TIER_Q})
        a = r["answers"]["tier"]
        return {
            "tier": a["choice"],
            "confidence": a.get("confidence"),
            "probabilities": a.get("probabilities"),
        }
    except Exception as exc:  # grading must never break a report
        return {"tier": "ungraded", "error": str(exc)[:120]}


def _log_dataset(tool: str, item: dict, value: str, grade: dict) -> None:
    if grade.get("tier") == "ungraded":
        return
    rec = {
        "state": f"Tool={tool}. site={item.get('site')} url={item.get('url') or '-'}. username/subject: {value}",
        "questions": {"tier": TIER_Q},
        "answers": {"tier": grade["tier"]},
        "source": "jev-live",
        "ts": time.strftime("%Y-%m-%dT%H:%M:%S"),
    }
    try:
        LAYA_DATASET.parent.mkdir(parents=True, exist_ok=True)
        with LAYA_DATASET.open("a") as f:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    except Exception:
        pass


def grade_job(job) -> dict:
    """Grade every found item of a Job (in-memory), attach .grade, log to dataset."""
    graded = 0
    with ThreadPoolExecutor(max_workers=8) as pool:
        futures = []
        for res in job.results:
            value = res.get("qvalue") or str(job.value)
            for item in res.get("found", []):
                futures.append(pool.submit(_grade_one, res["tool"], item, value))
        idx = 0
        for res in job.results:
            value = res.get("qvalue") or str(job.value)
            for item in res.get("found", []):
                grade = futures[idx].result()
                idx += 1
                if grade:
                    item["grade"] = grade
                    graded += 1
                    _log_dataset(res["tool"], item, value, grade)
    job.emit(
        "log",
        tool="grader",
        line=f"[hub] Jev graded {graded} found item(s); logged to laya osint-grading dataset",
    )
    job.persist()
    return {"graded": graded}
