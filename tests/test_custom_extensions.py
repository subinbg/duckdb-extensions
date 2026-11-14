"""Our own extensions, signed with our key and trusted through a user-provided repository."""

import shutil
from pathlib import Path

import duckdb
import pytest

from tests.support import (
    CUSTOM_REPOSITORY,
    Bundle,
    Connect,
    loaded_extensions,
    register_repository,
    scalar,
    tampered_copy,
)


@pytest.fixture
def trusted(bundle: Bundle, connect: Connect) -> duckdb.DuckDBPyConnection:
    """A connection that has registered the bundle's custom repository and our public key."""
    con = connect(allow_extension_repositories="allowed")
    register_repository(con, bundle.custom_repo, bundle.public_key)
    return con


def test_bundle_contains_hello(bundle: Bundle) -> None:
    assert "hello" in bundle.custom


def test_hello_from_signed_repository(trusted: duckdb.DuckDBPyConnection) -> None:
    trusted.execute(f"INSTALL hello FROM {CUSTOM_REPOSITORY}")
    trusted.execute(f"LOAD hello FROM {CUSTOM_REPOSITORY}")
    assert scalar(trusted, "SELECT hello('duck')") == "Hello, duck!"
    rows = trusted.execute("SELECT hello(name) FROM (VALUES ('Huey'), (NULL), ('Dewey')) t(name)").fetchall()
    assert rows == [("Hello, Huey!",), (None,), ("Hello, Dewey!",)]
    assert {"hello"} <= loaded_extensions(trusted)


def test_signature_checks_stay_on(trusted: duckdb.DuckDBPyConnection) -> None:
    assert scalar(trusted, "SELECT current_setting('allow_unsigned_extensions')") is False


def test_repository_and_install_persist_for_later_connections(
    trusted: duckdb.DuckDBPyConnection, connect: Connect
) -> None:
    """Register + INSTALL once; later processes only need LOAD ... FROM."""
    trusted.execute(f"INSTALL hello FROM {CUSTOM_REPOSITORY}")
    trusted.close()
    con = connect()
    con.execute(f"LOAD hello FROM {CUSTOM_REPOSITORY}")
    assert scalar(con, "SELECT hello('again')") == "Hello, again!"


def test_core_and_custom_extensions_together(bundle: Bundle, trusted: duckdb.DuckDBPyConnection) -> None:
    trusted.execute(f"SET custom_extension_repository = '{bundle.core_repo}'")
    trusted.execute("INSTALL httpfs")
    trusted.execute("LOAD httpfs")
    trusted.execute(f"INSTALL hello FROM {CUSTOM_REPOSITORY}")
    trusted.execute(f"LOAD hello FROM {CUSTOM_REPOSITORY}")
    assert {"httpfs", "hello"} <= loaded_extensions(trusted)


def test_core_keys_do_not_trust_our_extension(bundle: Bundle) -> None:
    """A plain LOAD only accepts DuckDB-signed files, so our key must be registered explicitly."""
    with pytest.raises(duckdb.IOException, match="signature"):
        duckdb.connect().execute(f"LOAD '{bundle.custom['hello']}'")


def test_tampered_custom_extension_is_rejected(bundle: Bundle, connect: Connect, tmp_path: Path) -> None:
    prefix = tmp_path / "evil-repo"
    shutil.copytree(bundle.custom_repo, prefix)
    tampered_copy(bundle.custom["hello"], prefix / bundle.version / bundle.platform / "hello.duckdb_extension")
    con = connect(allow_extension_repositories="allowed")
    register_repository(con, prefix, bundle.public_key)
    with pytest.raises(duckdb.IOException, match="signature"):
        con.execute(f"INSTALL hello FROM {CUSTOM_REPOSITORY}")


def test_repository_with_wrong_key_is_rejected(bundle: Bundle, connect: Connect) -> None:
    """Same files, but the repository pins a valid RSA key that did not sign them (from DuckDB's test suite)."""
    other_key = (
        "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAt9+KeDkWJa7SiBb7AMdXAPV2qpRunztoC2c7DtRmN9+Lq2YQm4rM"
        "QmmAQcwyxTxcK8vwPKOiE4mLMwmQMvTty9tL+buREfvg5KJ+FY3Bv05cZYsf7VuYgUDcw41zSEE5W1thh5PiafcrMn7CPF0jN0oF"
        "fVvAHB/fAM0yaZGPKs3WYqnHAHa6v/cVMeiWZ/lQORCBqqL2hii1fEiuIqw6AOEoZVsN0/lqm7GfnD899zrzLOPRuP4YFg46drL1"
        "UyNdg+4TquJ2OV9kR4CAfdGRexqHFWLmoxkTfnUxJ4yL+gkV57K1H+smEdKrDhDX68pEFM/34E2eVKZoRU/0paxDZQIDAQAB"
    )
    con = connect(allow_extension_repositories="allowed")
    register_repository(con, bundle.custom_repo, other_key)
    with pytest.raises(duckdb.IOException, match="signature"):
        con.execute(f"INSTALL hello FROM {CUSTOM_REPOSITORY}")


def test_unsigned_mode_alternative(bundle: Bundle) -> None:
    """Option A from the design notes: no repository, but signature checks off for everything."""
    con = duckdb.connect(config={"allow_unsigned_extensions": True})
    con.execute(f"LOAD '{bundle.custom['hello']}'")
    assert scalar(con, "SELECT hello('unsigned')") == "Hello, unsigned!"
