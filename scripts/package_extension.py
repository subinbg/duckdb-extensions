#!/usr/bin/env python3
"""Turn a compiled Rust cdylib into a (signed) DuckDB extension file.

1. Appends DuckDB's extension footer, byte for byte the layout written by
   duckdb/extension-ci-tools scripts/append_extension_metadata.py: a Wasm custom
   section header, 8 x 32-byte metadata fields, and a 256-byte signature slot.
2. Fills the signature slot the way DuckDB verifies it (duckdb/scripts/
   compute-extension-hash.sh): SHA-256 each 1 MiB chunk of the file minus its last
   256 bytes, SHA-256 the concatenated digests, and RSA-sign that with SHA-256.

The output lands in DuckDB's repository layout:
<out>/<duckdb_version>/<platform>/<name>.duckdb_extension

Standard library only (argpr.py, next to this script, builds the command line), plus the
openssl CLI for signing.
"""

import hashlib
import re
import subprocess
import sys
import tomllib
from pathlib import Path

from argpr import command, init

SIGNATURE_SIZE = 256
HASH_CHUNK_SIZE = 1 << 20
NAME_PATTERN = re.compile(r"^[a-z0-9_]+$")
# Wasm custom section (id 0) of 531 bytes named "duckdb_signature" with 512 bytes of content.
# Native loaders ignore it and read only the trailing 512 bytes; official binaries carry it too.
WASM_SECTION_HEADER = b"\x00\x93\x04\x10duckdb_signature\x80\x04"


def metadata_field(value: str) -> bytes:
    raw = value.encode("ascii")
    if len(raw) > 32:
        raise ValueError(f"metadata field longer than 32 bytes: {value!r}")
    return raw.ljust(32, b"\x00")


def footer(platform: str, duckdb_version: str, extension_version: str, abi_type: str) -> bytes:
    # Stored last-to-first: DuckDB reverses the 8 fields when it parses them.
    fields = ["", "", "", abi_type, extension_version, duckdb_version, platform, "4"]
    return WASM_SECTION_HEADER + b"".join(metadata_field(f) for f in fields) + b"\x00" * SIGNATURE_SIZE


def extension_hash(signed_part: bytes) -> bytes:
    digests = b"".join(
        hashlib.sha256(signed_part[offset : offset + HASH_CHUNK_SIZE]).digest()
        for offset in range(0, len(signed_part), HASH_CHUNK_SIZE)
    )
    return hashlib.sha256(digests).digest()


def rsa_sign(digest: bytes, key_path: Path) -> bytes:
    signature = subprocess.run(
        ["openssl", "pkeyutl", "-sign", "-inkey", str(key_path), "-pkeyopt", "digest:sha256"],
        input=digest,
        stdout=subprocess.PIPE,
        check=True,
    ).stdout
    if len(signature) != SIGNATURE_SIZE:
        raise SystemExit(f"signature is {len(signature)} bytes; DuckDB needs an RSA-2048 key (256-byte signatures)")
    return signature


@command
def package_extension(
    *,
    manifest: Path,  # the extension's Cargo.toml
    library_dir: Path,  # directory holding lib<name>.so
    platform: str,  # e.g. linux_amd64
    duckdb_version: str,  # exact DuckDB version, e.g. v2.0.0
    abi_type: str = "C_STRUCT_UNSTABLE",  # C_STRUCT_UNSTABLE (C API v2, what duckdb-neo builds) or C_STRUCT
    signing_key: Path | None = None,  # RSA-2048 private key (PEM); omit for unsigned
    out: Path,  # repository directory
) -> None:
    package = tomllib.loads(manifest.read_text())["package"]
    name, version = package["name"], package["version"]
    if not NAME_PATTERN.match(name):
        raise SystemExit(f"package name {name!r} is not a valid extension name ([a-z0-9_]+)")

    library = library_dir / f"lib{name}.so"
    signed_part = library.read_bytes() + footer(platform, duckdb_version, version, abi_type)
    signed_part = signed_part[:-SIGNATURE_SIZE]
    if signing_key:
        signature = rsa_sign(extension_hash(signed_part), signing_key)
    else:
        print(f"WARNING: {name} is unsigned; it loads only with allow_unsigned_extensions", file=sys.stderr)
        signature = b"\x00" * SIGNATURE_SIZE

    target = out / duckdb_version / platform / f"{name}.duckdb_extension"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(signed_part + signature)
    state = "signed" if signing_key else "unsigned"
    print(f"{state} {target} ({name} {version}, {abi_type}, {platform}, {duckdb_version})")


if __name__ == "__main__":
    init()
