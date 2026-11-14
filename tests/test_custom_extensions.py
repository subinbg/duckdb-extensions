"""Our own extensions, signed with our key and trusted through the repository that `install` registers."""

import shutil
from pathlib import Path

import duckdb
import pytest

from install import REPOSITORY, install_custom
from paths import CUSTOM_REPOSITORY, PUBLIC_KEY, extensions
from tests.support import Connect, loaded_extensions, scalar, tampered_copy


def test_repository_holds_hello() -> None:
    assert "hello" in extensions(CUSTOM_REPOSITORY)


def test_hello_from_signed_repository(connect: Connect) -> None:
    """Installed once; a later connection only needs LOAD ... FROM."""
    con = connect()
    con.execute(f"LOAD hello FROM {REPOSITORY}")
    assert scalar(con, "SELECT hello('duck')") == "Hello, duck!"
    rows = con.execute("SELECT hello(name) FROM (VALUES ('Huey'), (NULL), ('Dewey')) t(name)").fetchall()
    assert rows == [("Hello, Huey!",), (None,), ("Hello, Dewey!",)]
    assert {"hello"} <= loaded_extensions(con)


def test_signature_checks_stay_on(connect: Connect) -> None:
    assert scalar(connect(), "SELECT current_setting('allow_unsigned_extensions')") is False


def test_core_and_custom_extensions_together(connect: Connect) -> None:
    con = connect()
    con.execute("LOAD httpfs")
    con.execute(f"LOAD hello FROM {REPOSITORY}")
    assert {"httpfs", "hello"} <= loaded_extensions(con)


def test_core_keys_do_not_trust_our_extension() -> None:
    """A plain LOAD only accepts DuckDB-signed files, so our key must be registered explicitly."""
    with pytest.raises(duckdb.IOException, match="signature"):
        duckdb.connect().execute(f"LOAD '{extensions(CUSTOM_REPOSITORY)['hello']}'")


def test_tampered_custom_extension_is_rejected(tmp_path: Path) -> None:
    repository = tmp_path / "repository"
    shutil.copytree(CUSTOM_REPOSITORY, repository)
    hello = extensions(repository)["hello"]
    tampered_copy(hello, hello)
    with pytest.raises(duckdb.IOException, match="signature"):
        install_custom(repository, tmp_path / "extensions")


def test_repository_with_wrong_key_is_rejected(tmp_path: Path) -> None:
    """Same files, but the repository names a valid RSA key that did not sign them (from DuckDB's test suite)."""
    other_key = (
        "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAt9+KeDkWJa7SiBb7AMdXAPV2qpRunztoC2c7DtRmN9+Lq2YQm4rM"
        "QmmAQcwyxTxcK8vwPKOiE4mLMwmQMvTty9tL+buREfvg5KJ+FY3Bv05cZYsf7VuYgUDcw41zSEE5W1thh5PiafcrMn7CPF0jN0oF"
        "fVvAHB/fAM0yaZGPKs3WYqnHAHa6v/cVMeiWZ/lQORCBqqL2hii1fEiuIqw6AOEoZVsN0/lqm7GfnD899zrzLOPRuP4YFg46drL1"
        "UyNdg+4TquJ2OV9kR4CAfdGRexqHFWLmoxkTfnUxJ4yL+gkV57K1H+smEdKrDhDX68pEFM/34E2eVKZoRU/0paxDZQIDAQAB"
    )
    repository = tmp_path / "repository"
    shutil.copytree(CUSTOM_REPOSITORY, repository)
    (repository / PUBLIC_KEY).write_text(f"-----BEGIN PUBLIC KEY-----\n{other_key}\n-----END PUBLIC KEY-----\n")
    with pytest.raises(duckdb.IOException, match="signature"):
        install_custom(repository, tmp_path / "extensions")


def test_unsigned_mode_alternative() -> None:
    """Option A from the design notes: no repository, but signature checks off for everything."""
    con = duckdb.connect(config={"allow_unsigned_extensions": True})
    con.execute(f"LOAD '{extensions(CUSTOM_REPOSITORY)['hello']}'")
    assert scalar(con, "SELECT hello('unsigned')") == "Hello, unsigned!"
