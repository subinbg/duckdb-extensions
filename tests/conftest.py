"""Fixtures: the two extension directories that `scripts/build.py` filled.

A system that built somewhere other than `build/` names the directories in the environment.
"""

import os
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent


@pytest.fixture(scope="session")
def core() -> Path:
    """The directory given to `build.py --core`."""
    return Path(os.environ.get("DUCKDB_CORE_EXTENSIONS", ROOT / "build/core")).resolve()


@pytest.fixture(scope="session")
def custom() -> Path:
    """The directory given to `build.py --custom`."""
    return Path(os.environ.get("DUCKDB_CUSTOM_EXTENSIONS", ROOT / "build/custom")).resolve()
