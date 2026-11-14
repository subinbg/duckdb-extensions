#!/usr/bin/env python3

import hashlib
import json
import re
import subprocess
import tomllib
from collections.abc import Iterator
from pathlib import Path

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa, utils

from argpr import command, init
from paths import CUSTOM_REPOSITORY, DEVELOPMENT_KEY, PLATFORM, PUBLIC_KEY, ROOT, SIGNING_KEY, SUFFIX, VERSION

SIGNATURE_SIZE = 256
HASH_CHUNK_SIZE = 1 << 20
NAME_PATTERN = re.compile(r"^[a-z0-9_]+$")
PEM = serialization.Encoding.PEM


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


def metadata(capi_version: str, extension_version: str) -> bytes:
    # Stored last-to-first: DuckDB reverses the 8 fields when it parses them.
    fields = ["", "", "", "C_STRUCT", extension_version, capi_version, PLATFORM, "4"]
    return b"".join(metadata_field(f) for f in fields)


def extension_hash(signed_part: bytes) -> bytes:
    digests = b"".join(
        hashlib.sha256(signed_part[offset : offset + HASH_CHUNK_SIZE]).digest()
        for offset in range(0, len(signed_part), HASH_CHUNK_SIZE)
    )
    return hashlib.sha256(digests).digest()


def read_key(path: Path) -> rsa.RSAPrivateKey:
    """Read the private key that signs our extensions.

    The development key is created on first use.
    A key at any other path must exist, so that a release is never signed by a key made on the spot.
    """

    if path == DEVELOPMENT_KEY and not path.exists():
        key = rsa.generate_private_key(public_exponent=65537, key_size=8 * SIGNATURE_SIZE)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(key.private_bytes(PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
        path.chmod(0o600)

    key = serialization.load_pem_private_key(path.read_bytes(), password=None)

    if not isinstance(key, rsa.RSAPrivateKey) or key.key_size != 8 * SIGNATURE_SIZE:
        raise SystemExit(f"{path} is not an RSA-2048 key; DuckDB takes {SIGNATURE_SIZE}-byte signatures")

    return key


@command
def build_custom(
    *,
    out: Path = CUSTOM_REPOSITORY,  # repository directory to write into
    signing_key: Path = SIGNING_KEY,  # RSA-2048 private key (PEM)
) -> None:
    """Compile every crate under extensions/ and write each as a signed DuckDB extension file.

    1. `cargo build --release --locked` compiles the libraries.
    2. Each library gets DuckDB's extension footer: 8 x 32-byte metadata fields and a 256-byte signature.
       The metadata names the installed DuckDB client's platform, and the stable C API version the extension
       asks for (ABI C_STRUCT), read from `capi-version` under [package.metadata.duckdb] in its manifest.
    3. The signature is made the way DuckDB verifies it (duckdb/scripts/compute-extension-hash.sh):
       SHA-256 each 1 MiB chunk of the file up to the signature, SHA-256 the concatenated digests,
       and RSA-sign that with SHA-256.

    The output lands in DuckDB's repository layout, <out>/<version>/<platform>/<name>.duckdb_extension,
    with the public key that verifies it at <out>/public-key.pem.
    """

    key = read_key(signing_key)
    directory = out / VERSION / PLATFORM
    directory.mkdir(parents=True, exist_ok=True)

    for manifest, library in compile_extensions():
        package = tomllib.loads(manifest.read_text())["package"]
        name, version = package["name"], package["version"]
        capi_version = package["metadata"]["duckdb"]["capi-version"]

        if not NAME_PATTERN.match(name):
            raise SystemExit(f"package name {name!r} is not a valid extension name ([a-z0-9_]+)")

        signed_part = library.read_bytes() + metadata(capi_version, version)
        digest = extension_hash(signed_part)
        signature = key.sign(digest, padding.PKCS1v15(), utils.Prehashed(hashes.SHA256()))

        target = directory / f"{name}{SUFFIX}"
        target.write_bytes(signed_part + signature)
        print(f"signed {target} ({name} {version}, C API {capi_version})")

    public_key = key.public_key().public_bytes(PEM, serialization.PublicFormat.SubjectPublicKeyInfo)
    (out / PUBLIC_KEY).write_bytes(public_key)


if __name__ == "__main__":
    init()
