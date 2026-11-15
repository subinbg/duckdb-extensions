"""Fixtures: the two extension directories that `scripts/build.py` filled.

The tests are told where they are, as `make test` does from build.env.
"""

import os
from pathlib import Path

import pytest

_ENV_CORE_EXTENSION_DIR = "CORE_DIRECTORY"
_ENV_CUSTOM_EXTENSION_DIR = "CUSTOM_DIRECTORY"


def pytest_configure() -> None:
    missing = [
        variable for variable in (_ENV_CORE_EXTENSION_DIR, _ENV_CUSTOM_EXTENSION_DIR) if variable not in os.environ
    ]
    if missing:
        raise pytest.UsageError(f"set {' and '.join(missing)} to the directories given to scripts/build.py")


@pytest.fixture(scope="session")
def core() -> Path:
    """The directory given to `build.py --core-directory`."""
    return Path(os.environ[_ENV_CORE_EXTENSION_DIR]).resolve()


@pytest.fixture(scope="session")
def custom() -> Path:
    """The directory given to `build.py --custom-directory`."""
    return Path(os.environ[_ENV_CUSTOM_EXTENSION_DIR]).resolve()
