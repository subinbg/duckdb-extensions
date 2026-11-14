"""Fixtures: both repositories, installed once the way `scripts/install.py` installs them on a server."""

import duckdb
import pytest

from install import ConfigValue, install, settings
from paths import CORE_REPOSITORY, CUSTOM_REPOSITORY
from tests.support import Connect


@pytest.fixture(scope="session")
def installed(tmp_path_factory: pytest.TempPathFactory) -> dict[str, ConfigValue]:
    """The settings of a database that loads from the session's own extension directories."""
    root = tmp_path_factory.mktemp("installed")
    install(
        core_repository=CORE_REPOSITORY,
        core_directory=root / "core",
        custom_repository=CUSTOM_REPOSITORY,
        custom_directory=root / "custom",
    )
    return settings(root / "core", root / "custom")


@pytest.fixture
def connect(installed: dict[str, ConfigValue]) -> Connect:
    """Open a database that loads the installed extensions, as any later process on a server would."""

    def _connect(**config: ConfigValue) -> duckdb.DuckDBPyConnection:
        return duckdb.connect(config={**installed, **config})

    return _connect
