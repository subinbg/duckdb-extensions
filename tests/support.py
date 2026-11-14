"""Types and helpers shared by the tests (fixtures live in conftest.py)."""

from pathlib import Path
from typing import Protocol

import duckdb

from install import ConfigValue


class Connect(Protocol):
    def __call__(self, **config: ConfigValue) -> duckdb.DuckDBPyConnection: ...


def scalar(con: duckdb.DuckDBPyConnection, query: str) -> object:
    """First column of the first row of `query`."""
    row = con.execute(query).fetchone()
    assert row is not None, f"no rows from {query!r}"
    return row[0]


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
