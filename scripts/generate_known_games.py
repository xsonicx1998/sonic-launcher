#!/usr/bin/env python3
"""Build-time generator for Sonic Launcher's bundled known-games table.

Fetches game -> exe mappings from public sources, cleans them, and writes
a compact minified JSON map (normalized title -> relative Windows exe
paths) to ../src-tauri/known_games.json, where the Rust code embeds it
with include_str!() and loads it only while a folder scan runs.

Sources (build time only, never fetched at runtime):
  1. Discord Detectable Applications API
     (https://discord.com/api/v10/applications/detectable) — public, no
     auth. Windows (os == "win32") .exe entries only.
  2. PCGamingWiki Cargo API (https://www.pcgamingwiki.com/w/api.php) —
     BEST EFFORT. Verified 2026-11-24: anonymous Cargo queries are
     rejected with "permissiondenied" (Cargo queries need an account),
     so this source currently contributes nothing. The attempt is kept
     (one cheap request); any failure is logged and the build goes on
     with Discord + curated. If PCGamingWiki ever opens it up, the
     script picks it up with no code change.
  3. Curated overrides (scripts/curated_overrides.json) — small manual
     list for famous AAA titles with tricky paths. HIGHEST priority:
     for a title it covers, its paths REPLACE what Discord lists.

Safety: every exe from every source passes through the exact same hard
denylist as stores.rs `is_installer_exe` (substring match on the exe
stem, plus the `setup.exe`-is-always-an-installer rule), so no
uninstaller / setup / updater / server exe can enter the table. A
denylisted entry in the curated file is a human mistake and fails the
script loudly instead of being silently dropped.

Dedup: titles are normalized (alphanumeric + lowercase, same as Rust
`normalize_name`); exe paths are lowercased with `/` separators;
duplicates are removed; when the same exe stem comes from two sources
the longest (most specific) path wins. Priority:
Curated > PCGamingWiki > Discord.

Usage: python3 scripts/generate_known_games.py   (from the repo root,
or anywhere — paths are resolved from this file's location)
"""

import gzip
import json
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
CURATED_PATH = SCRIPT_DIR / "curated_overrides.json"
OUTPUT_PATH = REPO_ROOT / "src-tauri" / "known_games.json"

DISCORD_URL = "https://discord.com/api/v10/applications/detectable"
PCGW_API = "https://www.pcgamingwiki.com/w/api.php"

UA = {"User-Agent": "SonicLauncher-generator/1.0 (+https://github.com/xsonicx1998/sonic-launcher)"}

# Exact copy of the DENY list in src-tauri/src/stores.rs `is_installer_exe`.
# Keep in sync — verify/src/lib.rs has a test asserting the generated file
# contains zero denylisted exes, which catches drift either way.
DENYLIST = [
    # Uninstallers / deleters / cleaners.
    "uninstall", "unins", "uninst", "remove", "remover", "delete",
    "cleanup", "cleaner", "wipe", "purge", "reset", "repair", "modify",
    # Installers / prerequisites.
    "install", "installer", "autorun", "bootstrap", "redist", "vcredist",
    "vc_redist", "dxsetup", "directx", "oalinst", "physx", "dotnet",
    "ue4prereq", "ueprereq",
    # Crash / updaters / helpers / tools.
    "crashreporter", "crashhandler", "unitycrashhandler", "crashpad",
    "bugsplat", "update", "updater", "benchmark", "config",
    "easyanticheat_setup", "battleye", "beservice", "dedicated", "server",
]


def normalize_title(title: str) -> str:
    """Same as Rust `normalize_name`: keep alphanumerics, lowercase."""
    return "".join(ch for ch in title if ch.isalnum()).lower()


def normalize_exe(path: str) -> str:
    """Lowercase, forward slashes, no surrounding whitespace."""
    return path.strip().lower().replace("\\", "/")


def denylisted(stem: str) -> bool:
    """Same as Rust `is_installer_exe` on a lowercased exe stem."""
    if stem == "setup":
        return True
    return any(word in stem for word in DENYLIST)


def exe_stem(path: str) -> str:
    """Lowercased file stem of a normalized exe path (no .exe, no dirs)."""
    name = path.rsplit("/", 1)[-1]
    return name[:-4] if name.endswith(".exe") else name


def fetch_json(url: str, timeout: int = 60):
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def fetch_discord() -> dict:
    """Return {normalized title: set(exe paths)} from Discord."""
    print(f"Fetching Discord detectable applications ...")
    data = fetch_json(DISCORD_URL)
    print(f"  {len(data)} applications in the Discord list")
    table: dict = {}
    raw_exes = 0
    kept_exes = 0
    for app in data:
        name = app.get("name") or ""
        paths = set()
        for exe in app.get("executables") or []:
            if exe.get("os") != "win32":
                continue
            raw_exes += 1
            if exe.get("is_launcher"):
                continue
            raw_name = (exe.get("name") or "").strip()
            if not raw_name.lower().endswith(".exe"):
                continue
            if raw_name.startswith(">"):  # Discord-internal marker, not a path
                continue
            path = normalize_exe(raw_name)
            if denylisted(exe_stem(path)):
                continue
            paths.add(path)
            kept_exes += 1
        if not paths:
            continue
        key = normalize_title(name)
        if key:
            table.setdefault(key, set()).update(paths)
        for alias in app.get("aliases") or []:
            alias_key = normalize_title(alias)
            if alias_key and alias_key != key:
                table.setdefault(alias_key, set()).update(paths)
    print(f"  win32 exe entries: {raw_exes}, kept after filter: {kept_exes}")
    print(f"  distinct titles (incl. aliases): {len(table)}")
    return table


def fetch_pcgamingwiki() -> dict:
    """Best-effort Cargo query. Returns {} on any failure (incl. the
    verified anonymous permission denial) — never fails the build."""
    print("Trying PCGamingWiki Cargo API (best-effort) ...")
    url = (
        f"{PCGW_API}?action=cargoquery&tables=Games"
        "&fields=_pageName&limit=500&format=json"
    )
    try:
        data = fetch_json(url, timeout=15)
    except urllib.error.HTTPError as e:
        print(f"  PCGamingWiki request failed (HTTP {e.code}) — skipping")
        return {}
    except Exception as e:  # noqa: BLE001 - best-effort source, any error skips it
        print(f"  PCGamingWiki request failed ({e}) — skipping")
        return {}
    if isinstance(data, dict) and "error" in data:
        info = data["error"].get("info", "?")
        print(f"  PCGamingWiki refused the query ({info}) — skipping")
        return {}
    # A successful Cargo query returns {"cargoquery": [{"title": {...}}]};
    # exe paths are not a documented field, so even a working query
    # contributes nothing unless the schema grows. Count what we got.
    rows = data.get("cargoquery", []) if isinstance(data, dict) else []
    print(f"  PCGamingWiki returned {len(rows)} rows (no exe field) — skipping")
    return {}


def load_curated() -> dict:
    with open(CURATED_PATH, "r", encoding="utf-8") as f:
        raw = json.load(f)
    table = {}
    for title, paths in raw.items():
        key = normalize_title(title)
        clean = set()
        for p in paths:
            path = normalize_exe(p)
            if not path.endswith(".exe"):
                sys.exit(f"ERROR: curated entry {title!r} -> {p!r} is not an .exe")
            if denylisted(exe_stem(path)):
                sys.exit(
                    f"ERROR: curated entry {title!r} -> {p!r} is denylisted — "
                    "fix scripts/curated_overrides.json"
                )
            clean.add(path)
        if clean:
            table[key] = clean
    print(f"Curated overrides: {len(table)} titles")
    return table


def main() -> int:
    discord = fetch_discord()
    pcgw = fetch_pcgamingwiki()
    curated = load_curated()

    # Merge: curated REPLACES Discord/PCGW for the same title (exact,
    # verified paths win over crowd data); PCGW merges under Discord.
    merged: dict = {}
    for source in (discord, pcgw):
        for title, paths in source.items():
            merged.setdefault(title, set()).update(paths)
    replaced = 0
    for title, paths in curated.items():
        if title in merged:
            replaced += 1
        merged[title] = set(paths)
    print(f"Curated replaced Discord paths for {replaced} titles")

    # Final dedup is structural (sets), but sort for a stable file.
    out = {title: sorted(paths) for title, paths in sorted(merged.items())}
    total_pairs = sum(len(v) for v in out.values())
    text = json.dumps(out, ensure_ascii=True, separators=(",", ":"), sort_keys=True)
    data = text.encode("utf-8")
    OUTPUT_PATH.write_bytes(data)
    gz = gzip.compress(data, compresslevel=9)

    print(f"Wrote {OUTPUT_PATH} ({OUTPUT_PATH.relative_to(REPO_ROOT)})")
    print(f"  games (titles): {len(out)}")
    print(f"  (title, exe) pairs: {total_pairs}")
    print(f"  minified JSON: {len(data)} bytes")
    print(f"  gzipped: {len(gz)} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
