#!/usr/bin/env python3

import hashlib
import tarfile
from pathlib import Path

from argpr import command, init
from paths import CORE_REPOSITORY, CUSTOM_REPOSITORY, PLATFORM, PUBLIC_KEY, ROOT, VERSION


@command
def bundle(
    *,
    core_repository: Path = CORE_REPOSITORY,
    custom_repository: Path = CUSTOM_REPOSITORY,
    out: Path = ROOT / "dist",  # directory to write the tarball into
) -> None:
    """Pack both repositories into one release tarball, with its SHA-256 beside it.

    The tarball holds the extensions of the installed DuckDB client's version and platform.
    Unpacked on another machine, its `core/` and `custom/` are the repositories that install.py installs from.
    """

    name = f"duckdb-extensions-{VERSION}-{PLATFORM}.tar.gz"
    files = Path(VERSION) / PLATFORM
    out.mkdir(parents=True, exist_ok=True)

    with tarfile.open(out / name, "w:gz") as tar:
        tar.add(core_repository / files, str("core" / files))
        tar.add(custom_repository / files, str("custom" / files))
        tar.add(custom_repository / PUBLIC_KEY, f"custom/{PUBLIC_KEY}")

    digest = hashlib.sha256((out / name).read_bytes()).hexdigest()
    (out / f"{name}.sha256").write_text(f"{digest}  {name}\n")
    print(f"{digest}  {out / name}")


if __name__ == "__main__":
    init()
