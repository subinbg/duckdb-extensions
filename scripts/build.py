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
    system = {"Linux": "linux", "Darwin": "osx"}[platform.system()]
    machine = {"x86_64": "amd64", "aarch64": "arm64", "arm64": "arm64"}[platform.machine()]
    return f"{system}_{machine}"


def download_core_extensions(names: list[str], directory: Path, url: str) -> None:
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


def compile_custom_extensions(names: list[str]) -> Iterator[tuple[Path, Path]]:
    packages = [argument for name in names for argument in ("--package", name)]
    cargo = ["cargo", "build", "--release", "--locked", "--message-format=json-render-diagnostics", *packages]
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


def build_custom_extensions(names: list[str], directory: Path, platform: str) -> None:
    """Compile custom DuckDB extensions written in Rust, and convert each library as a DuckDB extension file.

    To make a library as a valid DuckDB extension file, it requires a DuckDB footer:
    8 x 32-byte metadata fields and a 256-byte signature.

    The metadata contains the platform, and the stable C API version (ABI C_STRUCT).
    This API version should match `capi-version` under `[package.metadata.duckdb]` in `Cargo.toml`.
    """
    for manifest, library in compile_custom_extensions(names):
        package = tomllib.loads(manifest.read_text())["package"]
        name, version = package["name"], package["version"]
        capi_version = package["metadata"]["duckdb"]["capi-version"]

        if not NAME_PATTERN.match(name):
            raise SystemExit(f"package name {name!r} is not a valid extension name ([a-z0-9_]+)")

        # DuckDB reverses the 8 fields when it parses them.
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
    duckdb_version: str,
    core_directory: Path,
    custom_directory: Path,
    core_extensions: list[str] | None = None,
    custom_extensions: list[str] | None = None,
    dist_directory: Path | None = None,
) -> None:

    platform = duckdb_platform()
    core = core_directory / duckdb_version / platform
    custom = custom_directory / duckdb_version / platform
    core.mkdir(parents=True, exist_ok=True)
    custom.mkdir(parents=True, exist_ok=True)

    if core_extensions:
        download_core_extensions(core_extensions, core, f"{BASE_URL}/{duckdb_version}/{platform}")

    if custom_extensions:
        build_custom_extensions(custom_extensions, custom, platform)

    if dist_directory:
        tarball = dist_directory / f"duckdb-extensions-{duckdb_version}-{platform}.tar.gz"
        pack(tarball, {f"core/{duckdb_version}/{platform}": core, f"custom/{duckdb_version}/{platform}": custom})


if __name__ == "__main__":
    init()
