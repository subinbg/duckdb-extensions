#!/usr/bin/env python3

from pathlib import Path

import duckdb

from argpr import command, init
from paths import CORE_DIRECTORY, CORE_REPOSITORY, CUSTOM_DIRECTORY, CUSTOM_REPOSITORY, PUBLIC_KEY, extensions

# The name our repository is registered under: `LOAD <name> FROM custom`.
REPOSITORY = "custom"

# Value types accepted by duckdb.connect(config=...).
type ConfigValue = bool | float | int | list[str] | str


def settings(
    core_directory: Path = CORE_DIRECTORY, custom_directory: Path = CUSTOM_DIRECTORY
) -> dict[str, ConfigValue]:
    """What `duckdb.connect(config=...)` takes to load the installed extensions, and to download none."""
    return {
        "extension_directories": [str(core_directory), str(custom_directory)],
        "extension_repository_directory": str(custom_directory),
        "autoinstall_known_extensions": False,
    }


def install_core(repository: Path, directory: Path) -> None:
    """Install the official extensions, which DuckDB checks against its own keys."""
    con = duckdb.connect(config={"extension_directories": [str(directory)]})

    for name in extensions(repository):
        con.execute(f"FORCE INSTALL {name} FROM '{repository}'")
        print(f"installed {name} into {directory}")


def install_custom(repository: Path, directory: Path) -> None:
    """Install our extensions, which DuckDB checks against the public key of our repository.

    The repository and its key are registered in `directory` too.
    Later connections trust that registration, so signature checks stay on.
    """
    lines = (repository / PUBLIC_KEY).read_text().splitlines()
    public_key = "".join(line for line in lines if not line.startswith("-----"))

    con = duckdb.connect(
        config={
            "extension_directories": [str(directory)],
            "extension_repository_directory": str(directory),
            "allow_extension_repositories": "allowed",
        }
    )
    con.execute(
        f"CREATE OR REPLACE EXTENSION REPOSITORY {REPOSITORY} "
        f"WITH PREFIX '{repository}' USING PUBLIC KEY '{public_key}'"
    )

    for name in extensions(repository):
        con.execute(f"FORCE INSTALL {name} FROM {REPOSITORY}")
        print(f"installed {name} into {directory}")


@command
def install(
    *,
    core_repository: Path = CORE_REPOSITORY,  # where fetch_core.py downloaded the official extensions
    core_directory: Path = CORE_DIRECTORY,  # where DuckDB will load them from
    custom_repository: Path = CUSTOM_REPOSITORY,  # where build_custom.py wrote our extensions
    custom_directory: Path = CUSTOM_DIRECTORY,  # where DuckDB will load them from
) -> None:
    """Install the extensions of both repositories into DuckDB extension directories, without network access.

    Afterwards a process opens its database with `duckdb.connect(config=settings(core_directory, custom_directory))`.
    Official extensions then load on demand, and ours with `LOAD <name> FROM custom`.
    """

    install_core(core_repository, core_directory)
    install_custom(custom_repository, custom_directory)


if __name__ == "__main__":
    init()
