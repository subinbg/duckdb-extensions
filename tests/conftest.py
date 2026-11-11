"""Fixtures: the release tarball, unpacked the way a server would unpack it."""

import os
import tarfile
from pathlib import Path

import duckdb
import pytest

from tests.support import Bundle, ConfigValue, Connect

ARTIFACT_DIR = Path(os.environ.get("DUCKDB_ARTIFACT_DIR", "/artifacts"))


@pytest.fixture(scope="session")
def bundle(tmp_path_factory: pytest.TempPathFactory) -> Bundle:
    tarballs = sorted(ARTIFACT_DIR.glob("duckdb-extensions-*.tar.gz"))
    assert len(tarballs) == 1, f"expected exactly one release tarball in {ARTIFACT_DIR}, found {tarballs}"
    root = tmp_path_factory.mktemp("bundle")
    with tarfile.open(tarballs[0]) as tar:
        tar.extractall(root, filter="data")
    (version_dir,) = (root / "core").iterdir()
    (platform_dir,) = version_dir.iterdir()
    return Bundle(root=root, version=version_dir.name, platform=platform_dir.name)


@pytest.fixture
def connect(tmp_path: Path) -> Connect:
    """Open an isolated database: its own extension and repository directories, no auto-install.

    Connections from one test share those directories, so a second connection sees what the
    first one installed, just like separate processes on a server sharing ~/.duckdb.
    """
    extension_directory = tmp_path / "extensions"
    repository_directory = tmp_path / "extension_repositories"

    def _connect(**config: ConfigValue) -> duckdb.DuckDBPyConnection:
        settings: dict[str, ConfigValue] = {
            "extension_directory": str(extension_directory),
            "extension_repository_directory": str(repository_directory),
            "autoinstall_known_extensions": False,
            "autoload_known_extensions": True,
        }
        settings.update(config)
        return duckdb.connect(config=settings)

    return _connect
