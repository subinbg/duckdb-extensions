"""Helpers shared by the tests (fixtures live in conftest.py)."""

from collections.abc import Sequence
from pathlib import Path

import duckdb

SUFFIX = ".duckdb_extension"


def connect(
    directories: Sequence[Path], *, check_signatures: bool = True, database: str = ":memory:"
) -> duckdb.DuckDBPyConnection:
    """Open a database that loads extensions from `directories`, and downloads none on its own.

    DuckDB checks every extension it loads against its own keys, which only the official ones pass.
    Ours load with `check_signatures=False`, which turns the check off for the whole database.
    """
    return duckdb.connect(
        database,
        config={
            "extension_directories": [str(directory) for directory in directories],
            "allow_unsigned_extensions": not check_signatures,
            "autoinstall_known_extensions": False,
        },
    )


def scalar(con: duckdb.DuckDBPyConnection, query: str) -> object:
    """First column of the first row of `query`."""
    row = con.execute(query).fetchone()
    assert row is not None, f"no rows from {query!r}"
    return row[0]


def target() -> tuple[str, str]:
    """The DuckDB version and platform of the installed client, such as `("v2.0.0", "osx_arm64")`."""
    con = duckdb.connect()
    return str(scalar(con, "PRAGMA version")), str(scalar(con, "PRAGMA platform"))


def files(directory: Path) -> dict[str, Path]:
    """The extension files that `directory` holds for the installed DuckDB client, by extension name."""
    version, platform = target()
    paths = sorted((directory / version / platform).glob(f"*{SUFFIX}"))
    return {path.name.removesuffix(SUFFIX): path for path in paths}


def loaded_extensions(con: duckdb.DuckDBPyConnection) -> set[str]:
    rows = con.execute(
        "SELECT extension_name FROM duckdb_extensions() WHERE loaded AND install_mode <> 'STATICALLY_LINKED'"
    ).fetchall()
    return {str(name) for (name,) in rows}


def tampered_copy(source: Path, target: Path) -> Path:
    """Copy an extension and flip one byte in the middle of its code."""
    data = bytearray(source.read_bytes())
    data[len(data) // 2] ^= 0xFF
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(bytes(data))
    return target
