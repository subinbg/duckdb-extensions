"""Official extensions, loaded from their directory with signature checks on and no network access."""

import functools
import http.server
import sqlite3
import threading
from pathlib import Path

import duckdb
import pytest

from tests.support import connect, files, loaded_extensions, scalar, tampered_copy


def test_built_for_the_installed_client(core: Path, custom: Path) -> None:
    """`scripts/build.py --duckdb-version` named the version of the DuckDB client that runs the tests."""
    assert files(core), f"{core} holds no extension for the installed DuckDB client"
    assert files(custom), f"{custom} holds no extension for the installed DuckDB client"


def test_every_core_extension_passes_the_signature_check(core: Path) -> None:
    """Our own check of the official files: a database that loads our extensions does not check them."""
    con = connect([core])
    for name in files(core):
        con.execute(f"LOAD {name}")
    assert loaded_extensions(con) == set(files(core))


def test_httpfs_autoloads_for_http_urls(core: Path, tmp_path: Path) -> None:
    """Serve a CSV from localhost (loopback works under --network none) and read it over HTTP."""
    (tmp_path / "data.csv").write_text("x\n1\n2\n3\n")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(tmp_path))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        con = connect([core])
        url = f"http://127.0.0.1:{server.server_port}/data.csv"
        assert scalar(con, f"SELECT sum(x) FROM read_csv('{url}')") == 6
        assert "httpfs" in loaded_extensions(con)
    finally:
        server.shutdown()


def test_sqlite_autoloads_on_attach(core: Path, tmp_path: Path) -> None:
    path = tmp_path / "app.sqlite"
    with sqlite3.connect(path) as db:
        db.execute("CREATE TABLE ducks (name TEXT, age INTEGER)")
        db.executemany("INSERT INTO ducks VALUES (?, ?)", [("Huey", 3), ("Dewey", 4)])
    con = connect([core])
    con.execute(f"ATTACH '{path}' AS app (TYPE sqlite, READ_ONLY)")
    assert con.execute("SELECT name FROM app.ducks ORDER BY age").fetchall() == [("Huey",), ("Dewey",)]
    assert "sqlite_scanner" in loaded_extensions(con)


def test_iceberg_loads_with_its_avro_dependency(core: Path) -> None:
    con = connect([core])
    con.execute("LOAD iceberg")
    assert {"iceberg", "avro"} <= loaded_extensions(con)
    rows = con.execute("SELECT DISTINCT function_name FROM duckdb_functions()").fetchall()
    assert {"iceberg_scan", "iceberg_metadata", "iceberg_snapshots"} <= {str(name) for (name,) in rows}


def test_extension_that_was_not_fetched_is_not_downloaded(core: Path) -> None:
    """A query that needs another extension fails; nothing is fetched from extensions.duckdb.org for it."""
    with pytest.raises(duckdb.CatalogException, match="spatial"):
        connect([core]).execute("SELECT ST_Point(1, 2)")
    assert "spatial" not in files(core)


def test_tampered_core_extension_is_rejected(core: Path, tmp_path: Path) -> None:
    bad = tampered_copy(files(core)["httpfs"], tmp_path / "httpfs.duckdb_extension")
    with pytest.raises(duckdb.IOException, match="signature"):
        connect([core]).execute(f"LOAD '{bad}'")
