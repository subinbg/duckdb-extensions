"""Official extensions, installed from the bundle with no network access."""

import functools
import http.server
import sqlite3
import threading
from pathlib import Path

import duckdb
import pytest

from tests.support import Bundle, Connect, loaded_extensions, scalar, tampered_copy


@pytest.fixture
def installed(bundle: Bundle, connect: Connect) -> Connect:
    """Install every core extension from the bundle once, then hand out fresh connections."""
    con = connect(custom_extension_repository=str(bundle.core_repo))
    for name in bundle.core:
        con.execute(f"INSTALL {name}")
    con.close()
    return connect


def test_python_client_matches_bundle(bundle: Bundle) -> None:
    con = duckdb.connect()
    assert scalar(con, "PRAGMA version") == bundle.version
    assert scalar(con, "PRAGMA platform") == bundle.platform


def test_bundle_contains_expected_core_extensions(bundle: Bundle) -> None:
    assert set(bundle.core) == {"httpfs", "iceberg", "avro", "sqlite_scanner"}


def test_install_from_mirror(bundle: Bundle, connect: Connect) -> None:
    con = connect(custom_extension_repository=str(bundle.core_repo))
    for name in bundle.core:
        con.execute(f"INSTALL {name}")
    rows = con.execute(
        "SELECT extension_name, installed_from FROM duckdb_extensions() WHERE install_mode = 'REPOSITORY'"
    ).fetchall()
    assert dict(rows) == {name: str(bundle.core_repo) for name in bundle.core}


def test_httpfs_autoloads_for_http_urls(installed: Connect, tmp_path: Path) -> None:
    """Serve a CSV from localhost (loopback works under --network none) and read it over HTTP."""
    (tmp_path / "data.csv").write_text("x\n1\n2\n3\n")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(tmp_path))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        con = installed()
        url = f"http://127.0.0.1:{server.server_port}/data.csv"
        assert scalar(con, f"SELECT sum(x) FROM read_csv('{url}')") == 6
        assert "httpfs" in loaded_extensions(con)
    finally:
        server.shutdown()


def test_sqlite_autoloads_on_attach(installed: Connect, tmp_path: Path) -> None:
    path = tmp_path / "app.sqlite"
    with sqlite3.connect(path) as db:
        db.execute("CREATE TABLE ducks (name TEXT, age INTEGER)")
        db.executemany("INSERT INTO ducks VALUES (?, ?)", [("Huey", 3), ("Dewey", 4)])
    con = installed()
    con.execute(f"ATTACH '{path}' AS app (TYPE sqlite, READ_ONLY)")
    assert con.execute("SELECT name FROM app.ducks ORDER BY age").fetchall() == [("Huey",), ("Dewey",)]
    assert "sqlite_scanner" in loaded_extensions(con)


def test_iceberg_loads_with_its_avro_dependency(installed: Connect) -> None:
    con = installed()
    con.execute("LOAD iceberg")
    assert {"iceberg", "avro"} <= loaded_extensions(con)
    rows = con.execute("SELECT DISTINCT function_name FROM duckdb_functions()").fetchall()
    assert {"iceberg_scan", "iceberg_metadata", "iceberg_snapshots"} <= {str(name) for (name,) in rows}


def test_extension_missing_from_mirror_is_not_downloaded(bundle: Bundle, connect: Connect) -> None:
    """INSTALL looks only in the mirror; nothing falls back to extensions.duckdb.org."""
    con = connect(custom_extension_repository=str(bundle.core_repo))
    with pytest.raises(duckdb.Error) as excinfo:
        con.execute("INSTALL spatial")
    assert str(bundle.core_repo) in str(excinfo.value)


def test_tampered_core_extension_is_rejected(bundle: Bundle, tmp_path: Path) -> None:
    bad = tampered_copy(bundle.core["httpfs"], tmp_path / "httpfs.duckdb_extension")
    with pytest.raises(duckdb.IOException, match="signature"):
        duckdb.connect().execute(f"LOAD '{bad}'")
