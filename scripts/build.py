#!/usr/bin/env python3

import gzip
import hashlib
import json
import platform
import re
import subprocess
import tarfile
import tomllib
import urllib.request
from collections.abc import Iterator
from pathlib import Path

from argpr import ArgumentError, command, init

ROOT = Path(__file__).resolve().parent.parent
BASE_URL = "https://extensions.duckdb.org"
SUFFIX = ".duckdb_extension"
NAME_PATTERN = re.compile(r"^[a-z0-9_]+$")
SIGNATURE_SIZE = 256


def duckdb_platform() -> str:
    """DuckDB's name for the machine this runs on, such as `linux_amd64` or `osx_arm64`."""
    system = {"Linux": "linux", "Darwin": "osx"}[platform.system()]
    machine = {"x86_64": "amd64", "aarch64": "arm64", "arm64": "arm64"}[platform.machine()]
    return f"{system}_{machine}"


def fetch_core(names: list[str], directory: Path, url: str) -> None:
    """Download the official extensions that `directory` does not hold yet."""
    for name in names:
        if not NAME_PATTERN.match(name):
            raise ArgumentError(f"invalid extension name {name!r}")

        target = directory / f"{name}{SUFFIX}"
        if not target.exists():
            # The CDN answers 403 to urllib's default "Python-urllib/x.y" User-Agent.
            request = urllib.request.Request(
                f"{url}/{target.name}.gz", headers={"User-Agent": "duckdb-extension-mirror"}
            )
            with urllib.request.urlopen(request, timeout=300) as response:
                target.write_bytes(gzip.decompress(response.read()))

        print(f"core    {target}")


def compile_extensions() -> Iterator[tuple[Path, Path]]:
    """Compile the workspace, and give the manifest and the library of each extension in it."""
    cargo = ["cargo", "build", "--release", "--locked", "--message-format=json-render-diagnostics"]
    messages = subprocess.run(cargo, cwd=ROOT, stdout=subprocess.PIPE, check=True).stdout

    for line in messages.splitlines():
        message = json.loads(line)
        if message["reason"] == "compiler-artifact" and "cdylib" in message["target"]["kind"]:
            yield Path(message["manifest_path"]), Path(message["filenames"][0])


def metadata_field(value: str) -> bytes:
    raw = value.encode("ascii")

    if len(raw) > 32:
        raise ValueError(f"metadata field longer than 32 bytes: {value!r}")

    return raw.ljust(32, b"\x00")


def build_custom(directory: Path, platform: str) -> None:
    """Compile every crate under extensions/, and write each library as a DuckDB extension file.

    A library becomes an extension file by DuckDB's footer: 8 x 32-byte metadata fields and a 256-byte signature.
    The metadata names the platform, and the stable C API version the extension asks for (ABI C_STRUCT),
    read from `capi-version` under [package.metadata.duckdb] in its manifest.
    The signature is left empty, so DuckDB loads the file only with allow_unsigned_extensions.
    """
    for manifest, library in compile_extensions():
        package = tomllib.loads(manifest.read_text())["package"]
        name, version = package["name"], package["version"]
        capi_version = package["metadata"]["duckdb"]["capi-version"]

        if not NAME_PATTERN.match(name):
            raise SystemExit(f"package name {name!r} is not a valid extension name ([a-z0-9_]+)")

        # Stored last-to-first: DuckDB reverses the 8 fields when it parses them.
        fields = ["", "", "", "C_STRUCT", version, capi_version, platform, "4"]
        footer = b"".join(metadata_field(field) for field in fields) + b"\x00" * SIGNATURE_SIZE

        target = directory / f"{name}{SUFFIX}"
        target.write_bytes(library.read_bytes() + footer)
        print(f"custom  {target} ({name} {version}, C API {capi_version})")


def pack(tarball: Path, directories: dict[str, Path]) -> None:
    """Pack extension directories into a release tarball, with its SHA-256 beside it."""
    tarball.parent.mkdir(parents=True, exist_ok=True)

    with tarfile.open(tarball, "w:gz") as tar:
        for name, directory in directories.items():
            tar.add(directory, name)

    digest = hashlib.sha256(tarball.read_bytes()).hexdigest()
    tarball.with_name(f"{tarball.name}.sha256").write_text(f"{digest}  {tarball.name}\n")
    print(f"tarball {tarball}  {digest}")


@command
def build(
    extensions: list[str],  # official extensions to download, such as httpfs
    /,
    *,
    duckdb_version: str,  # the DuckDB release to build for, such as v2.0.0
    core: Path,  # extension directory for the official extensions
    custom: Path,  # extension directory for our extensions
    dist: Path | None = None,  # also pack both into a release tarball in this directory
) -> None:
    """Fill two DuckDB extension directories, for one DuckDB version and for the machine this runs on.

    1. The official extensions named on the command line are downloaded from extensions.duckdb.org into <core>.
    2. Every crate under extensions/ is compiled with cargo and written into <custom>.

    Both are laid out as DuckDB reads an extension directory: <directory>/<version>/<platform>/<name>.duckdb_extension
    A database opened with `extension_directories` set to them loads what they hold.
    """

    platform = duckdb_platform()
    files = Path(duckdb_version) / platform
    (core / files).mkdir(parents=True, exist_ok=True)
    (custom / files).mkdir(parents=True, exist_ok=True)

    fetch_core(extensions, core / files, f"{BASE_URL}/{duckdb_version}/{platform}")
    build_custom(custom / files, platform)

    if dist:
        tarball = dist / f"duckdb-extensions-{duckdb_version}-{platform}.tar.gz"
        pack(tarball, {str("core" / files): core / files, str("custom" / files): custom / files})


if __name__ == "__main__":
    init()
