"""Job manager: runs scanner tools in worker threads, streams parsed events,
persists job state to data/jobs/*.json."""

from __future__ import annotations

import collections
import json
import os
import re
import secrets
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from tools import TOOLS, resolve_exe, strip_ansi

JOBS_DIR = Path(__file__).resolve().parent.parent / "data" / "jobs"
JOBS_DIR.mkdir(parents=True, exist_ok=True)

MAX_EVENTS = 5000  # in-memory event cap per job
MAX_JOBS_KEPT = 200  # prune persisted job files beyond this
TOOL_HARD_TIMEOUT = int(os.environ.get("OSINT_HUB_TOOL_TIMEOUT", "900"))  # seconds


class RateLimiter:
    """Sliding-window limiter, spec 'N/hour' or 'N/minute'."""

    WINDOWS = {"hour": 3600, "minute": 60, "second": 1}

    def __init__(self, spec: str):
        m = re.match(r"^(\d+)\s*/\s*(hour|minute|second)$", spec.strip().lower())
        if not m:
            raise ValueError(f"bad rate limit spec: {spec!r} (expected e.g. '20/hour')")
        self.limit, self.window = int(m.group(1)), self.WINDOWS[m.group(2)]
        self._hits: dict[str, collections.deque] = {}
        self._lock = threading.Lock()

    def allow(self, key: str) -> tuple[bool, int]:
        now = time.time()
        with self._lock:
            dq = self._hits.setdefault(key, collections.deque())
            while dq and dq[0] <= now - self.window:
                dq.popleft()
            if len(dq) >= self.limit:
                retry = int(self.window - (now - dq[0])) + 1
                return False, retry
            dq.append(now)
            return True, 0


class Job:
    def __init__(
        self, type_: str, value: "str | dict", tools: list[str], options: dict
    ):
        self.id = "j_" + secrets.token_hex(4)
        self.type = type_
        self.value = value
        self.tools = tools
        self.options = options or {}
        # queries: [(qtype, qvalue)] — "multi" jobs fan out to one run per (query, tool)
        if type_ == "multi" and isinstance(value, dict):
            self.queries = [
                (k, str(value[k]).strip())
                for k in ("username", "phone", "email", "image")
                if value.get(k)
            ]
        else:
            self.queries = [(type_, str(value).strip())]
        self.status = "queued"
        self.created_at = time.time()
        self.started_at: float | None = None
        self.finished_at: float | None = None
        self.error: str | None = None
        self.results = [
            {
                "tool": t,
                "qtype": qt,
                "qvalue": qv,
                "status": "pending",
                "found": [],
                "info": [],
                "error": None,
            }
            for (qt, qv) in self.queries
            for t in tools
            if TOOLS.get(t, {}).get("kind") == qt
        ]
        self.events: list[dict] = []
        self.lock = threading.Lock()

    # -- events ------------------------------------------------------------
    def emit(self, event: str, **data) -> None:
        with self.lock:
            self.events.append({"event": event, "ts": round(time.time(), 3), **data})
            if len(self.events) > MAX_EVENTS:
                del self.events[: len(self.events) - MAX_EVENTS]

    def snapshot(self, log_tail: int = 50) -> dict:
        with self.lock:
            log_lines = [
                f"[{e.get('tool', '')}] {e.get('line', '')}"
                for e in self.events
                if e["event"] == "log"
            ]
            return {
                "id": self.id,
                "type": self.type,
                "value": self.value,
                "status": self.status,
                "created_at": self.created_at,
                "started_at": self.started_at,
                "finished_at": self.finished_at,
                "results": [dict(r) for r in self.results],
                "log_tail": "\n".join(log_lines[-log_tail:]),
            }

    def persist(self) -> None:
        try:
            snap = self.snapshot(log_tail=0)
            snap["events"] = self.events[-2000:]
            tmp = JOBS_DIR / f"{self.id}.tmp"
            tmp.write_text(json.dumps(snap))
            tmp.replace(JOBS_DIR / f"{self.id}.json")
        except Exception:
            pass  # persistence is best-effort


class JobsManager:
    def __init__(self):
        self.max_workers = int(os.environ.get("OSINT_HUB_MAX_CONCURRENT", "2"))
        self.executor = ThreadPoolExecutor(max_workers=self.max_workers)
        self.rate = RateLimiter(os.environ.get("OSINT_HUB_RATE_LIMIT", "20/hour"))
        self.jobs: "collections.OrderedDict[str, Job]" = collections.OrderedDict()
        self._prune_old_files()

    # -- public ------------------------------------------------------------
    def create(
        self, type_: str, value: "str | dict", tools: list[str], options: dict
    ) -> Job:
        job = Job(type_, value, tools, options)
        self.jobs[job.id] = job
        self.executor.submit(self._run_job, job)
        return job

    def get(self, job_id: str) -> Job | None:
        return self.jobs.get(job_id)

    def recent(self, limit: int = 15) -> list[dict]:
        out = []
        for job in reversed(self.jobs.values()):
            val = (
                job.value
                if isinstance(job.value, str)
                else " · ".join(f"{k}:{v}" for k, v in job.value.items() if v)
            )
            out.append(
                {
                    "id": job.id,
                    "type": job.type,
                    "value": val,
                    "status": job.status,
                    "created_at": job.created_at,
                }
            )
            if len(out) >= limit:
                break
        return out

    # -- internals -----------------------------------------------------------
    def _run_job(self, job: Job) -> None:
        job.status = "running"
        job.started_at = time.time()
        job.emit("job_start", tools=job.tools)
        job.persist()
        for res in job.results:
            res["status"] = "running"
            try:
                self._run_tool(job, res)
            except Exception as exc:  # noqa: BLE001
                res["status"] = "error"
                res["error"] = f"{type(exc).__name__}: {exc}"
                job.emit(
                    "log",
                    tool=res["tool"],
                    qtype=res.get("qtype"),
                    line=f"[hub] tool crashed: {res['error']}",
                )
            job.emit(
                "tool_done",
                tool=res["tool"],
                qtype=res.get("qtype"),
                found_count=len(res["found"]),
                info_count=len(res.get("info", [])),
                status=res["status"],
            )
            job.persist()
        job.status = (
            "error" if all(r["status"] == "error" for r in job.results) else "done"
        )
        job.finished_at = time.time()
        job.emit("job_done", status=job.status)
        job.persist()

    def _run_tool(self, job: Job, res: dict) -> None:
        tool_id = res["tool"]
        spec = TOOLS.get(tool_id)
        if spec is None:
            raise ValueError(f"unknown tool {tool_id}")
        cmd, cwd = spec["build"](res["qvalue"], job.options)
        exe = resolve_exe(cmd[0])
        if exe is None:
            raise FileNotFoundError(f"{cmd[0]} not installed — {spec['hint']}")
        cmd = [str(exe)] + cmd[1:]
        job.emit(
            "log", tool=tool_id, qtype=res.get("qtype"), line=f"[hub] $ {' '.join(cmd)}"
        )

        proc = subprocess.Popen(
            cmd,
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        watchdog = threading.Timer(
            TOOL_HARD_TIMEOUT,
            lambda: (
                proc.kill(),
                job.emit(
                    "log",
                    tool=tool_id,
                    qtype=res.get("qtype"),
                    line="[hub] hard timeout — killed",
                ),
            ),
        )
        watchdog.daemon = True
        watchdog.start()
        try:
            assert proc.stdout is not None
            for raw in proc.stdout:
                line = strip_ansi(raw.rstrip())
                if not line:
                    continue
                job.emit("log", tool=tool_id, qtype=res.get("qtype"), line=line)
                parser = spec.get("parse")
                if parser:
                    item = parser(line)
                    if item:
                        res["found"].append(item)
                        job.emit(
                            "found", tool=tool_id, qtype=res.get("qtype"), item=item
                        )
                iparser = spec.get("parse_info")
                if iparser and len(res["info"]) < 80:
                    iitem = iparser(line)
                    if iitem:
                        res["info"].append(iitem)
                        job.emit(
                            "info", tool=tool_id, qtype=res.get("qtype"), item=iitem
                        )
            rc = proc.wait(timeout=30)
        finally:
            watchdog.cancel()
        res["status"] = "done" if rc == 0 else "error"
        if rc != 0:
            res["error"] = f"exit code {rc}"

    def _prune_old_files(self) -> None:
        try:
            files = sorted(
                JOBS_DIR.glob("j_*.json"), key=lambda p: p.stat().st_mtime, reverse=True
            )
            for old in files[MAX_JOBS_KEPT:]:
                old.unlink(missing_ok=True)
        except Exception:
            pass
