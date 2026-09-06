"""Qalarc OSINT Hub — FastAPI backend wrapping local OSINT scanner tools.

Run: scripts/run.sh  (or: cd backend && .venv/bin/uvicorn app:app --port 8788)
Env: see .env.example — OSINT_HUB_TOKEN, OSINT_HUB_RATE_LIMIT, OSINT_HUB_MAX_CONCURRENT,
     OSINT_HUB_CORS, SEARCHPHONE_ENABLED.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
from pathlib import Path

from fastapi import Depends, FastAPI, Header, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, Field

import jobs
import tools

FRONTEND_DIR = Path(__file__).resolve().parent.parent / "frontend"
AUTH_TOKEN = os.environ.get("OSINT_HUB_TOKEN", "")

app = FastAPI(title="Qalarc OSINT Hub", version="1.0.0")
manager = jobs.JobsManager()

if os.environ.get("OSINT_HUB_CORS", "1") == "1":
    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],  # dev default; set OSINT_HUB_CORS=0 in production
        allow_methods=["*"],
        allow_headers=["*"],
    )

USERNAME_RE = re.compile(r"^[A-Za-z0-9._-]{1,64}$")
PHONE_RE = re.compile(r"^\+?[0-9][0-9 ()\-]{6,24}$")
EMAIL_RE = re.compile(r"^[^@\s]+@[^@\s]+\.[^@\s]{2,}$")


def require_auth(authorization: str | None = Header(default=None)) -> None:
    if AUTH_TOKEN and authorization != f"Bearer {AUTH_TOKEN}":
        raise HTTPException(status_code=401, detail="unauthorized")


class ScanRequest(BaseModel):
    type: str = Field(..., pattern="^(username|phone|email|multi)$")
    value: str | dict
    tools: list[str]
    options: dict = {}


@app.get("/api/health")
def health():
    return {
        "ok": True,
        "version": app.version,
        "tools_installed": tools.tools_installed_count(),
        "auth_required": bool(AUTH_TOKEN),
    }


@app.get("/api/tools")
def list_tools():
    return {"tools": tools.tool_status()}


def _validate_single(kind: str, value: str) -> str:
    value = value.strip()
    if kind == "username":
        if not USERNAME_RE.match(value):
            raise HTTPException(
                400, "invalid username (allowed: A-Z a-z 0-9 . _ - , max 64 chars)"
            )
    elif kind == "phone":
        digits = re.sub(r"\D", "", value)
        if not PHONE_RE.match(value) or not (7 <= len(digits) <= 15):
            raise HTTPException(
                400, "invalid phone number (expected E.164-ish, 7-15 digits)"
            )
    else:  # email
        if not EMAIL_RE.match(value):
            raise HTTPException(400, "invalid email address")
    return value


@app.post("/api/scan", status_code=202, dependencies=[Depends(require_auth)])
def scan(req: ScanRequest, request: Request):
    if req.type == "multi":
        if not isinstance(req.value, dict):
            raise HTTPException(
                400, "multi scan expects value: {username?, phone?, email?}"
            )
        clean = {}
        for kind in ("username", "phone", "email"):
            if req.value.get(kind):
                clean[kind] = _validate_single(kind, str(req.value[kind]))
        if not clean:
            raise HTTPException(400, "provide at least one of: username, phone, email")
        value: str | dict = clean
        kinds_present = set(clean)
    else:
        if not isinstance(req.value, str):
            raise HTTPException(400, "value must be a string for this scan type")
        value = _validate_single(req.type, req.value)
        kinds_present = {req.type}

    valid = [t for t in dict.fromkeys(req.tools) if t in tools.TOOLS]
    valid = [t for t in valid if tools.TOOLS[t]["kind"] in kinds_present]
    if not valid:
        raise HTTPException(
            400, f"no valid tools selected for: {', '.join(sorted(kinds_present))}"
        )

    allowed, retry = manager.rate.allow(
        request.client.host if request.client else "anon"
    )
    if not allowed:
        return JSONResponse(
            {"error": f"rate limited: retry in {retry}s"}, status_code=429
        )

    options = {
        "timeout": min(int(req.options.get("timeout", 10) or 10), 60),
        "sites": [str(s) for s in (req.options.get("sites") or [])][:10],
        "sites_filter": req.options.get("sites_filter"),
    }
    job = manager.create(req.type, value, valid, options)
    return {"job_id": job.id}


@app.get("/api/jobs", dependencies=[Depends(require_auth)])
def list_jobs(limit: int = 15):
    return {"jobs": manager.recent(min(limit, 50))}


@app.get("/api/jobs/{job_id}", dependencies=[Depends(require_auth)])
def get_job(job_id: str):
    job = manager.get(job_id)
    if not job:
        raise HTTPException(
            404, "job not found (jobs are in-memory; restart clears them)"
        )
    return job.snapshot()


@app.get("/api/jobs/{job_id}/stream", dependencies=[Depends(require_auth)])
async def stream_job(job_id: str):
    job = manager.get(job_id)
    if not job:
        raise HTTPException(404, "job not found")

    async def gen():
        yield f"event: snapshot\ndata: {json.dumps(job.snapshot())}\n\n"
        cursor = 0
        while True:
            if job.status in ("done", "error") and job.finished_at:
                with job.lock:
                    pending = job.events[cursor:]
                    cursor = len(job.events)
                for e in pending:
                    yield f"event: {e['event']}\ndata: {json.dumps(e)}\n\n"
                break
            with job.lock:
                pending = job.events[cursor:]
                cursor = len(job.events)
            for e in pending:
                yield f"event: {e['event']}\ndata: {json.dumps(e)}\n\n"
            await asyncio.sleep(0.7)

    from fastapi.responses import StreamingResponse

    return StreamingResponse(gen(), media_type="text/event-stream")


# frontend (served same-origin so the page works with no API base config)
if FRONTEND_DIR.is_dir():
    app.mount("/", StaticFiles(directory=str(FRONTEND_DIR), html=True), name="frontend")
