#!/usr/bin/env python3
"""Download official DuckDB core extensions and check them against a SHA-256 lock file.

Binaries are written in DuckDB's repository layout,
<out>/<version>/<platform>/<name>.duckdb_extension, so the output directory can be
used directly as `custom_extension_repository`.

DuckDB verifies each file's signature on INSTALL/LOAD anyway. The lock adds a second
check: CI fails if upstream ever serves different bytes for the same version.

Standard library only.
"""

import argparse
import gzip
import hashlib
import re
import sys
import urllib.request
from pathlib import Path

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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--version", required=True, help="DuckDB version, e.g. v2.0.0")
    parser.add_argument("--platform", required=True, action="append", help="DuckDB platform (repeatable)")
    parser.add_argument("--lock", required=True, type=Path)
    parser.add_argument("--out", type=Path, help="repository directory to write binaries into")
    parser.add_argument("--update-lock", action="store_true", help="rewrite the lock instead of verifying against it")
    parser.add_argument("extensions", nargs="+")
    args = parser.parse_args()

    for name in args.extensions:
        if not NAME_PATTERN.match(name):
            parser.error(f"invalid extension name {name!r}")

    lock: dict[str, str] = {} if args.update_lock else read_lock(args.lock)
    failures = 0
    for platform in args.platform:
        for name in args.extensions:
            rel = f"{args.version}/{platform}/{name}.duckdb_extension"
            data = download(args.version, platform, name)
            digest = hashlib.sha256(data).hexdigest()
            if args.update_lock:
                lock[rel] = digest
            elif lock.get(rel) != digest:
                print(f"FAIL {rel}: lock has {lock.get(rel)}, downloaded {digest}", file=sys.stderr)
                failures += 1
                continue
            if args.out:
                target = args.out / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
            print(f"ok   {rel}  {digest}")

    if args.update_lock:
        write_lock(args.lock, lock)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
