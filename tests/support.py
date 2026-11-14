"""Types and helpers shared by the tests (fixtures live in conftest.py).

Bundle layout (built by the Dockerfile's package stage):
    core/<version>/<platform>/<name>.duckdb_extension    official, DuckDB-signed
    custom/<version>/<platform>/<name>.duckdb_extension  ours, signed with our key
    custom.pub.pem                                       our public key
"""

import base64
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

import duckdb

CUSTOM_REPOSITORY = "custom"

# Value types accepted by duckdb.connect(config=...).
type ConfigValue = bool | float | int | list[str] | str


class Connect(Protocol):
    def __call__(self, **config: ConfigValue) -> duckdb.DuckDBPyConnection: ...


@dataclass(frozen=True)
class Bundle:
    root: Path
    version: str
    platform: str

    @property
    def core_repo(self) -> Path:
        return self.root / "core"

    @property
    def custom_repo(self) -> Path:
        return self.root / "custom"

    def files(self, repo: Path) -> dict[str, Path]:
        directory = repo / self.version / self.platform
        return {path.name.split(".")[0]: path for path in sorted(directory.glob("*.duckdb_extension"))}

    @property
    def core(self) -> dict[str, Path]:
        return self.files(self.core_repo)

    @property
    def custom(self) -> dict[str, Path]:
        return self.files(self.custom_repo)

    @property
    def public_key(self) -> str:
        """Base64 body of the PEM public key, the form CREATE EXTENSION REPOSITORY takes."""
        lines = (self.root / "custom.pub.pem").read_text().splitlines()
        body = "".join(line for line in lines if not line.startswith("-----"))
        base64.b64decode(body, validate=True)
        return body


def register_repository(
    con: duckdb.DuckDBPyConnection, prefix: Path, public_key: str, name: str = CUSTOM_REPOSITORY
) -> None:
    con.execute(f"CREATE OR REPLACE EXTENSION REPOSITORY {name} WITH PREFIX '{prefix}' USING PUBLIC KEY '{public_key}'")


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
