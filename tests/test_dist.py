"""The release tarball that `make dist` wrote, when it did: our extensions only, each loading from the tarball alone."""

import os
import tarfile
from pathlib import Path

import pytest

from tests.support import SUFFIX, connect, files, scalar, target


@pytest.fixture
def tarball() -> Path:
    """The tarball for the installed DuckDB client, in the directory given to `build.py --dist-directory`."""
    directory = Path(os.environ.get("DIST_DIRECTORY", "dist"))
    if not any(directory.glob("*.tar.gz")):
        pytest.skip("no release tarball; run make dist")

    version, platform = target()
    return directory / f"duckdb-extensions-{version}-{platform}.tar.gz"


def test_tarball_holds_our_extensions_only(tarball: Path, custom: Path, tmp_path: Path) -> None:
    with tarfile.open(tarball) as tar:
        names = tar.getnames()
        tar.extractall(tmp_path, filter="data")

    version, platform = target()
    assert sorted(names) == sorted(f"{version}/{platform}/{name}{SUFFIX}" for name in files(custom))

    # Unpacked, the tarball is an extension directory that DuckDB loads ours from, and nothing else.
    con = connect([tmp_path], check_signatures=False)
    for name in files(tmp_path):
        con.execute(f"LOAD {name}")
    assert scalar(con, "SELECT hello('bundle')") == "Hello, bundle!"
