"""Official extensions, installed from the downloaded repository with no network access."""

import functools
import http.server
import sqlite3
import threading
from pathlib import Path

import duckdb
import pytest

from paths import CORE_REPOSITORY, extensions
from tests.support import Connect, loaded_extensions, scalar, tampered_copy


def test_repository_holds_expected_core_extensions() -> None:
    assert set(extensions(CORE_REPOSITORY)) == {"httpfs", "iceberg", "avro", "sqlite_scanner"}


def test_installed_from_repository(connect: Connect) -> None:
    rows = (
        connect().execute("SELECT extension_name, installed_from FROM duckdb_extensions() WHERE installed").fetchall()
    )
    assert dict(rows).items() >= {name: str(CORE_REPOSITORY) for name in extensions(CORE_REPOSITORY)}.items()


def test_httpfs_autoloads_for_http_urls(connect: Connect, tmp_path: Path) -> None:
    """Serve a CSV from localhost (loopback works under --network none) and read it over HTTP."""
    (tmp_path / "data.csv").write_text("x\n1\n2\n3\n")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(tmp_path))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        con = connect()
        url = f"http://127.0.0.1:{server.server_port}/data.csv"
        assert scalar(con, f"SELECT sum(x) FROM read_csv('{url}')") == 6
        assert "httpfs" in loaded_extensions(con)
    finally:
        server.shutdown()


def test_sqlite_autoloads_on_attach(connect: Connect, tmp_path: Path) -> None:
    path = tmp_path / "app.sqlite"
    with sqlite3.connect(path) as db:
        db.execute("CREATE TABLE ducks (name TEXT, age INTEGER)")
        db.executemany("INSERT INTO ducks VALUES (?, ?)", [("Huey", 3), ("Dewey", 4)])
    con = connect()
    con.execute(f"ATTACH '{path}' AS app (TYPE sqlite, READ_ONLY)")
    assert con.execute("SELECT name FROM app.ducks ORDER BY age").fetchall() == [("Huey",), ("Dewey",)]
    assert "sqlite_scanner" in loaded_extensions(con)


def test_iceberg_loads_with_its_avro_dependency(connect: Connect) -> None:
    con = connect()
    con.execute("LOAD iceberg")
    assert {"iceberg", "avro"} <= loaded_extensions(con)
    rows = con.execute("SELECT DISTINCT function_name FROM duckdb_functions()").fetchall()
    assert {"iceberg_scan", "iceberg_metadata", "iceberg_snapshots"} <= {str(name) for (name,) in rows}


def test_extension_missing_from_repository_is_not_downloaded(connect: Connect, tmp_path: Path) -> None:
    """INSTALL looks only in the repository; nothing falls back to extensions.duckdb.org."""
    con = connect(extension_directories=[str(tmp_path)], custom_extension_repository=str(CORE_REPOSITORY))
    with pytest.raises(duckdb.Error) as excinfo:
        con.execute("INSTALL spatial")
    assert str(CORE_REPOSITORY) in str(excinfo.value)


def test_tampered_core_extension_is_rejected(tmp_path: Path) -> None:
    bad = tampered_copy(extensions(CORE_REPOSITORY)["httpfs"], tmp_path / "httpfs.duckdb_extension")
    with pytest.raises(duckdb.IOException, match="signature"):
        duckdb.connect().execute(f"LOAD '{bad}'")
