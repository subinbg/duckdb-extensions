#!/usr/bin/env python3
"""Download official DuckDB core extensions and check them against a SHA-256 lock file.

Binaries are written in DuckDB's repository layout,
<out>/<version>/<platform>/<name>.duckdb_extension, so the output directory can be
used directly as `custom_extension_repository`.

DuckDB verifies each file's signature on INSTALL/LOAD anyway. The lock adds a second
check: CI fails if upstream ever serves different bytes for the same version.

Standard library only (argpr.py, next to this script, builds the command line).
"""

import gzip
import hashlib
import re
import sys
import urllib.request
from pathlib import Path

from argpr import ArgumentError, command, init

BASE_URL = "https://extensions.duckdb.org"
NAME_PATTERN = re.compile(r"^[a-z0-9_]+$")
LOCK_HEADER = "# SHA-256 of the uncompressed official extension binaries. Regenerate with `make lock`."


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


def download(version: str, platform: str, name: str) -> bytes:
    url = f"{BASE_URL}/{version}/{platform}/{name}.duckdb_extension.gz"
    # The CDN answers 403 to urllib's default "Python-urllib/x.y" User-Agent.
    request = urllib.request.Request(url, headers={"User-Agent": "duckdb-extension-mirror"})
    with urllib.request.urlopen(request, timeout=300) as response:
        return gzip.decompress(response.read())


@command
def fetch_core(
    extensions: list[str],
    /,
    *,
    version: str,  # e.g. v2.0.0
    platform: list[str],  # e.g. linux_amd64; repeat the flag for several
    lock: Path,
    out: Path | None = None,  # repository directory to write binaries into
    update_lock: bool = False,  # rewrite the lock instead of verifying against it
) -> int:
    for name in extensions:
        if not NAME_PATTERN.match(name):
            raise ArgumentError(f"invalid extension name {name!r}")

    pinned: dict[str, str] = {} if update_lock else read_lock(lock)
    failures = 0
    for plat in platform:
        for name in extensions:
            rel = f"{version}/{plat}/{name}.duckdb_extension"
            data = download(version, plat, name)
            digest = hashlib.sha256(data).hexdigest()
            if update_lock:
                pinned[rel] = digest
            elif pinned.get(rel) != digest:
                print(f"FAIL {rel}: lock has {pinned.get(rel)}, downloaded {digest}", file=sys.stderr)
                failures += 1
                continue
            if out:
                target = out / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
            print(f"ok   {rel}  {digest}")

    if update_lock:
        write_lock(lock, pinned)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(init())
