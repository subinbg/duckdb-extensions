#!/usr/bin/env python3

import gzip
import hashlib
import sys
import tomllib
import urllib.request
from pathlib import Path

from argpr import command, init
from paths import CORE_REPOSITORY, PLATFORM, ROOT, SUFFIX, VERSION

BASE_URL = "https://extensions.duckdb.org"
LOCK = ROOT / "core-extensions.sha256"
LOCK_HEADER = (
    "# SHA-256 of the uncompressed official extension binaries. Regenerate with `fetch_core.py --update-lock`."
)


def read_lock(path: Path) -> dict[str, str]:
    entries: dict[str, str] = {}

    if path.exists():
        for line in path.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                digest, rel = line.split(maxsplit=1)
                entries[rel] = digest

    return entries


def write_lock(path: Path, entries: dict[str, str]) -> None:
    lines = [LOCK_HEADER] + [f"{digest}  {rel}" for rel, digest in sorted(entries.items())]
    path.write_text("\n".join(lines) + "\n")


def download(rel: str) -> bytes:
    # The CDN answers 403 to urllib's default "Python-urllib/x.y" User-Agent.
    request = urllib.request.Request(f"{BASE_URL}/{rel}.gz", headers={"User-Agent": "duckdb-extension-mirror"})
    with urllib.request.urlopen(request, timeout=300) as response:
        return gzip.decompress(response.read())


@command
def fetch_core(
    *,
    out: Path = CORE_REPOSITORY,  # repository directory to download into
    update_lock: bool = False,  # rewrite the lock for every platform instead of downloading for this one
) -> int:
    """Download DuckDB's official core extensions and check them against the SHA-256 lock file.

    pyproject.toml lists the extensions under [tool.duckdb-extensions].
    They are downloaded for the installed DuckDB client, its version and its platform,
    and written in DuckDB's repository layout: <out>/<version>/<platform>/<name>.duckdb_extension

    DuckDB verifies each file's signature on INSTALL/LOAD anyway.
    The lock adds a second check: the download fails if upstream ever serves different bytes for the same version.

    --update-lock writes no extension: it pins the hashes of every platform that pyproject.toml lists.
    """

    config = tomllib.loads((ROOT / "pyproject.toml").read_text())["tool"]["duckdb-extensions"]
    platforms: list[str] = config["platforms"] if update_lock else [PLATFORM]
    pinned: dict[str, str] = {} if update_lock else read_lock(LOCK)
    failures = 0

    for platform in platforms:
        for name in config["core"]:
            rel = f"{VERSION}/{platform}/{name}{SUFFIX}"
            data = download(rel)
            digest = hashlib.sha256(data).hexdigest()

            if update_lock:
                pinned[rel] = digest
            elif pinned.get(rel) != digest:
                print(f"FAIL {rel}: lock has {pinned.get(rel)}, downloaded {digest}", file=sys.stderr)
                failures += 1
                continue
            else:
                target = out / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)

            print(f"ok   {rel}  {digest}")

    if update_lock:
        write_lock(LOCK, pinned)

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(init())
