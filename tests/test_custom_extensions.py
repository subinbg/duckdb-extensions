"""Our own extensions, which carry no signature and load only in a database that does not check one."""

from pathlib import Path

import duckdb
import pytest

from tests.support import connect, loaded_extensions, scalar


def test_hello(custom: Path) -> None:
    con = connect([custom], check_signatures=False)
    con.execute("LOAD hello")
    assert scalar(con, "SELECT hello('duck')") == "Hello, duck!"
    rows = con.execute("SELECT hello(name) FROM (VALUES ('Huey'), (NULL), ('Dewey')) t(name)").fetchall()
    assert rows == [("Hello, Huey!",), (None,), ("Hello, Dewey!",)]
    assert {"hello"} <= loaded_extensions(con)


def test_signature_check_refuses_our_extensions(custom: Path) -> None:
    with pytest.raises(duckdb.IOException, match="signature"):
        connect([custom]).execute("LOAD hello")


def test_core_and_custom_extensions_together(core: Path, custom: Path) -> None:
    con = connect([core, custom], check_signatures=False)
    con.execute("LOAD httpfs")
    con.execute("LOAD hello")
    assert {"httpfs", "hello"} <= loaded_extensions(con)
