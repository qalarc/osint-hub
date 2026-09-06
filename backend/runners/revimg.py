#!/usr/bin/env python
"""revimg.py — reverse-image ONLINE-DISCOVERY runner for the Qalarc OSINT Hub.

Discovery only: find pages/URLs where an image (or visually similar ones) appear,
via public reverse-image engines (PicImageSearch library). Face ANALYSIS/matching
is deliberately NOT here — that lives in qalarc's own face-engine stack
(~/projects/face-engine, smart-glasses-face-tracker). Pipeline: this finds
candidate pages; your face systems confirm identity; the Case File scans the rest.

Usage:  revimg.py IMAGE_URL [--engines yandex,bing,tineye] [--per-engine 15] [--max 60]
Output: '[+] engine: page-url | title' lines — parsed by tools.parse_plus_line.
Exit 0 even with partial engine failures (individual errors logged as '[*]').
"""

import argparse
import asyncio
import sys
from urllib.parse import urlparse


def _engines():
    from PicImageSearch import Bing, SauceNAO, Tineye, Yandex

    return {"yandex": Yandex, "bing": Bing, "tineye": Tineye, "saucenao": SauceNAO}


async def scan(url: str, engines: list, per_engine: int, max_results: int):
    from PicImageSearch import Network

    seen = {url}
    total = 0
    engine_map = _engines()
    async with Network() as client:
        for name in engines:
            cls = engine_map.get(name)
            if cls is None:
                print(f"[*] unknown engine skipped: {name}", flush=True)
                continue
            print(f"[*] querying {name}…", flush=True)
            try:
                engine = cls(client=client)
                resp = await engine.search(url=url)
                hits = getattr(resp, "raw", None) or []
                n = 0
                for r in hits:
                    u = getattr(r, "url", None)
                    if not u or u in seen:
                        continue
                    seen.add(u)
                    n += 1
                    total += 1
                    title = (
                        (getattr(r, "title", None) or "")
                        .strip()
                        .replace("\n", " ")[:100]
                    )
                    print(f"[+] {name}: {u} | {title}".rstrip(" |"), flush=True)
                    if n >= per_engine or total >= max_results:
                        break
                print(f"[*] {name}: {n} result(s)", flush=True)
            except Exception as exc:  # one engine failing must not kill the rest
                print(
                    f"[*] {name} error: {type(exc).__name__}: {exc}"[:300], flush=True
                )
    print(f"[*] image discovery complete — {total} unique page(s)", flush=True)


def main():
    ap = argparse.ArgumentParser(description="reverse-image online discovery")
    ap.add_argument("url", help="public image URL")
    ap.add_argument("--engines", default="yandex,bing,tineye")
    ap.add_argument("--per-engine", type=int, default=15)
    ap.add_argument("--max", type=int, default=60)
    a = ap.parse_args()
    p = urlparse(a.url)
    if p.scheme not in ("http", "https") or not p.netloc:
        print("[*] invalid image URL (must be http/https)", flush=True)
        sys.exit(2)
    engines = [e.strip().lower() for e in a.engines.split(",") if e.strip()]
    asyncio.run(scan(a.url, engines, a.per_engine, a.max))


if __name__ == "__main__":
    main()
