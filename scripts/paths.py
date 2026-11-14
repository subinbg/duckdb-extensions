import os
from pathlib import Path

import duckdb

ROOT = Path(__file__).resolve().parent.parent
SUFFIX = ".duckdb_extension"


def configured(variable: str, default: Path) -> Path:
    """The path an environment variable names, or `default` when it is unset."""
    return Path(os.environ.get(variable) or default)


# A repository holds extension files the way DuckDB installs from it:
# <repository>/<version>/<platform>/<name>.duckdb_extension
CORE_REPOSITORY = configured("DUCKDB_CORE_REPOSITORY", ROOT / "build/repository/core")
CUSTOM_REPOSITORY = configured("DUCKDB_CUSTOM_REPOSITORY", ROOT / "build/repository/custom")

# An extension directory is where DuckDB installs extensions, and loads them from.
CORE_DIRECTORY = configured("DUCKDB_CORE_DIRECTORY", ROOT / "build/extensions/core")
CUSTOM_DIRECTORY = configured("DUCKDB_CUSTOM_DIRECTORY", ROOT / "build/extensions/custom")

# The RSA-2048 private key that signs our extensions, and the name of its public half in our repository.
# Unless a system names another key, a development key signs; build_custom.py creates it on first use.
DEVELOPMENT_KEY = ROOT / "keys/signing-key.pem"
SIGNING_KEY = configured("DUCKDB_SIGNING_KEY", DEVELOPMENT_KEY)
PUBLIC_KEY = "public-key.pem"


def client() -> tuple[str, str]:
    """The version and platform of the installed DuckDB client, such as `("v2.0.0", "linux_amd64")`."""
    rows = duckdb.connect().execute("SELECT library_version, platform FROM pragma_version(), pragma_platform()")
    ((version, platform),) = rows.fetchall()
    return str(version), str(platform)


# Extensions are fetched, built and installed for the DuckDB client that runs the scripts.
VERSION, PLATFORM = client()


def extensions(repository: Path) -> dict[str, Path]:
    """The extension files a repository holds for this client, by extension name."""
    files = sorted((repository / VERSION / PLATFORM).glob(f"*{SUFFIX}"))
    return {file.name.removesuffix(SUFFIX): file for file in files}
