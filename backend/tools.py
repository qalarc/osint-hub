"""Scanner tool registry for Qalarc OSINT Hub.

Each tool defines: availability resolution, command builder, and an output-line
parser. CLI flags drift between tool versions — verify with `<tool> --help`
after installs and adjust build() functions here (this file is the source of
truth for scanner integration).
"""

from __future__ import annotations

import os
import re
import shutil
import sys
from pathlib import Path
from typing import Callable, Optional

BASE_DIR = Path(__file__).resolve().parent
VENV_BIN = BASE_DIR / ".venv" / "bin"  # NB: do NOT use sys.executable.resolve()
# (venv python is a symlink to system python; resolving it loses the venv bin dir)
PROJECT_DIR = BASE_DIR.parent
BIN_DIR = BASE_DIR / "bin"  # local binaries (phoneinfoga)
PUBLIC_REPOS = PROJECT_DIR.parent / "public_repos"  # cloned catalog (searchphone)

ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
URL_RE = re.compile(r"https?://[^\s\"')\]]+")

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


def strip_ansi(s: str) -> str:
    return ANSI_RE.sub("", s)


def resolve_exe(name: str) -> Optional[Path]:
    """Find an executable: venv bin → backend/bin → PATH."""
    for d in (VENV_BIN, BIN_DIR):
        p = d / name
        if p.is_file() and os.access(p, os.X_OK):
            return p
    found = shutil.which(name)
    return Path(found) if found else None


def parse_plus_line(line: str, require_url: bool = True) -> Optional[dict]:
    """Parse `[+] Site: https://url` style lines (sherlock/maigret/ignorant).

    require_url=True  → only lines containing a URL count as found (sherlock/maigret —
                        avoids maigret's banner lines like '[+] Using sites database')
    require_url=False → also accepts '[+] Site : note' (ignorant's phone-registered hits)

    Handles variants:
      [+] GitHub: https://github.com/john
      [+] tinder.com (Tinder): https://tinder.com/@john (200)
      [+] snapchat : Phone number found          (ignorant, no URL)
    """
    line = strip_ansi(line)
    if "[+]" not in line:
        return None
    text = line.split("[+]", 1)[1].strip()
    m = URL_RE.search(text)
    if m:
        url = m.group(0).rstrip(".,);")
        head = text[: m.start()].strip(" :-(").strip()
        head = re.sub(r"\s*\([^)]*\)\s*$", "", head).strip()  # drop trailing "(domain)"
        return {"site": head or url, "url": url}
    if require_url:
        return None
    txt = text.rstrip(".")
    if "[-]" in txt or "[x]" in txt or "not used" in txt.lower():
        return (
            None  # legend/summary lines, e.g. "[+] used, [-] not used, [x] rate limit"
        )
    if ":" in txt:
        txt = txt.split(":", 1)[0].strip()
    txt = txt.strip("()").strip()
    return {"site": txt, "url": None} if txt else None


# --- phoneinfoga structured-info parser -------------------------------------
# v3 scan output shape:
#   Results for googlesearch
#     Social media:            <- subsection headers end with ':' (skipped)
#     URL: https://...         <- Google-dork pivots -> info items
#   Results for local
#     Raw local: 0425228338    <- key: value pairs -> info items
#     Country: AU
PIG_INFO_KEYS = {
    "raw local",
    "local",
    "e164",
    "international",
    "country",
    "carrier",
    "line type",
    "number type",
    "region",
    "timezone",
    "valid",
    "type",
    "area",
}


def parse_phoneinfoga_info(line: str) -> Optional[dict]:
    line = strip_ansi(line).strip()
    if line.lower().startswith("url:"):
        url = line.split(":", 1)[1].strip()
        if url.startswith("http"):
            return {"key": "search pivot", "url": url}
        return None
    if ":" in line and not line.endswith(":"):
        k, v = line.split(":", 1)
        k, v = k.strip(), v.strip()
        if k.lower() in PIG_INFO_KEYS and v:
            return {"key": k, "value": v}
    return None


def _holehe(value: str, opts: dict):
    return ["holehe", value, "--no-color", "--no-clear"], None


# --- command builders -------------------------------------------------------
# each returns (cmd_list, cwd_or_None); first element is the exe name to resolve


def _sherlock(value: str, opts: dict):
    cmd = ["sherlock", value, "--timeout", str(int(opts.get("timeout", 10)))]
    for s in opts.get("sites") or []:
        cmd += ["--site", str(s)]
    return cmd, None


def _maigret(value: str, opts: dict):
    cmd = [
        "maigret",
        value,
        "--no-color",
        "--no-progressbar",
        "--no-autoupdate",
        "--no-recursion",
        "--no-extracting",
        "--timeout",
        str(int(opts.get("timeout", 10))),
    ]
    for s in opts.get("sites") or []:
        cmd += ["--site", str(s)]
    return cmd, None


def _phoneinfoga(value: str, opts: dict):
    return ["phoneinfoga", "scan", "-n", value], None


def _split_phone(value: str) -> tuple[str, str]:
    """('+' + country_code, national_number). Uses phonenumbers when installed,
    else a naive +CC split. ignorant's CLI wants them as separate positional args."""
    try:
        import phonenumbers  # optional dep, see requirements.txt

        n = phonenumbers.parse(value, None)
        return f"+{n.country_code}", str(n.national_number)
    except ImportError:
        pass
    v = re.sub(r"[ \-()]", "", value.strip())
    m = re.match(r"^\+(\d{1,3})(\d{4,})$", v)
    if not m:
        raise ValueError("phone must be international format, e.g. +61425228338")
    return f"+{m.group(1)}", m.group(2)


def _ignorant(value: str, opts: dict):
    cc, national = _split_phone(value)
    return ["ignorant", cc, national], None


def _searchphone(value: str, opts: dict):
    script = PUBLIC_REPOS / "osint" / "searchphone" / "search_phone.py"
    py = VENV_BIN / "python"
    cmd = [str(py), str(script), value]
    return cmd, str(script.parent)


# --- registry ---------------------------------------------------------------

TOOLS: dict[str, dict] = {
    "sherlock": {
        "name": "Sherlock",
        "kind": "username",
        "description": "Hunt down social media accounts by username across 400+ sites (sherlock-project).",
        "hint": ".venv/bin/pip install sherlock-project",
        "build": _sherlock,
        "parse": parse_plus_line,
    },
    "maigret": {
        "name": "Maigret",
        "kind": "username",
        "description": "Username → 3000+ sites including dating platforms (Tinder, Badoo, MeetMe, AFF). Powers the Dating tab.",
        "hint": ".venv/bin/pip install maigret",
        "build": _maigret,
        "parse": parse_plus_line,
    },
    "phoneinfoga": {
        "name": "PhoneInfoga",
        "kind": "phone",
        "description": "Phone number intelligence: number formats, country, carrier, reputation + Google-dork search pivots.",
        "hint": "scripts/setup.sh downloads the release binary into backend/bin/",
        "build": _phoneinfoga,
        "parse": None,  # no link 'found' items — structured info via parse_info
        "parse_info": parse_phoneinfoga_info,
    },
    "holehe": {
        "name": "Holehe",
        "kind": "email",
        "description": "Checks if an email is registered on 120+ platforms (Instagram, Adobe, etc.) via password-reset flows.",
        "hint": ".venv/bin/pip install holehe",
        "build": _holehe,
        "parse": lambda line: parse_plus_line(line, require_url=False),
    },
    "ignorant": {
        "name": "Ignorant",
        "kind": "phone",
        "description": "Checks whether a phone number is registered on platforms (Snapchat, Instagram…).",
        "hint": ".venv/bin/pip install ignorant",
        "build": _ignorant,
        "parse": lambda line: parse_plus_line(line, require_url=False),
    },
    "searchphone": {
        "name": "SearchPhone",
        "kind": "phone",
        "description": "Multi-API phone OSINT (Google/GitHub/Reddit/DDG + Hudson Rock) with report generation.",
        "hint": "requires SEARCHPHONE_ENABLED=1 + API keys — see public_repos/osint/searchphone/example.env",
        "build": _searchphone,
        "parse": parse_plus_line,
        "enabled_check": lambda: os.environ.get("SEARCHPHONE_ENABLED") == "1",
    },
}


def tool_status() -> list[dict]:
    out = []
    for tid, spec in TOOLS.items():
        installed = bool(resolve_exe(tid)) or bool(
            spec.get("enabled_check") and spec["enabled_check"]()
        )
        out.append(
            {
                "id": tid,
                "name": spec["name"],
                "kind": spec["kind"],
                "installed": installed,
                "description": spec["description"],
                "hint": spec["hint"],
            }
        )
    return out


def tools_installed_count() -> int:
    return sum(1 for t in tool_status() if t["installed"])
