# duckdb-extensions

Builds an offline extension bundle for the DuckDB Python client, for servers that cannot reach `extensions.duckdb.org`. A bundle is one tarball (meant to be a GitHub Release asset) with:

1. DuckDB's official core extensions `httpfs`, `iceberg`, `avro` (needed by iceberg) and `sqlite_scanner`, byte for byte as DuckDB publishes them and pinned by SHA-256.
2. Our own extensions, written in Rust against DuckDB's C API and signed with our own RSA key.

Everything is built and tested in containers. The tests unpack the tarball the way a server would and run with `--network none`.

## Quick start

Requires Docker with BuildKit, `make`, `openssl` and [uv](https://docs.astral.sh/uv/).

```bash
make test   # build the bundle, then run the loading tests offline
make dist   # write dist/duckdb-extensions-<version>-linux_<arch>.tar.gz and its .sha256
make check  # version pins, ruff lint + format check, pyrefly (strict preset)
```

The first run creates a development signing key at `keys/signing-key.pem` (ignored by git).

## Python tooling

The Python code (`scripts/`, `tests/`) is a uv project (`pyproject.toml`, `package = false`: dependencies only, nothing is built or installed as a package). Dependencies are split into two groups: `test` (pytest) and `dev` (the test group plus ruff and pyrefly). Locally, `uv sync` creates `.venv` with everything, which gives editors type information for the tests. The tests themselves run in the container, since they need the built bundle; its `test` stage installs from `uv.lock` with `uv sync --locked --no-dev --group test` (hash-checked, and the build fails if `uv.lock` is out of date with `pyproject.toml`).

```bash
uv run ruff format        # format
uv run ruff check --fix   # lint
uv run pyrefly check      # type check
```

## Repository layout

| Path | Purpose |
|---|---|
| `versions.env` | Every pinned input: DuckDB version, core extension list and container images (`make` passes them to `docker build`) |
| `pyproject.toml`, `uv.lock` | Python dependencies (also what the test container installs) and ruff/pyrefly configuration |
| `core-extensions.sha256` | Pinned hashes of the official binaries for `linux_amd64` and `linux_arm64` |
| `extensions/<name>/` | One Rust crate per custom extension; the package name is the extension name |
| `scripts/argpr.py` | Builds the scripts' command lines from their typed function signatures (standard library only, on top of `argparse`) |
| `scripts/fetch_core.py` | Downloads core extensions and checks them against the lock file |
| `scripts/package_extension.py` | Appends DuckDB's extension metadata to a compiled library and signs it |
| `Dockerfile` | Stages `build-custom`, `fetch-core`, `package`, `dist`, `test` |
| `tests/` | pytest suite run inside the `test` image (`support.py` holds shared types and helpers) |

## Bundle layout

| Path in the tarball | Contents |
|---|---|
| `core/<version>/<platform>/*.duckdb_extension` | Official extensions, signed by DuckDB |
| `custom/<version>/<platform>/*.duckdb_extension` | Our extensions, signed with our key |
| `custom.pub.pem` | Our public key |

Both `core/` and `custom/` use DuckDB's repository layout, so DuckDB can install from them directly.

## Using a bundle on a server

Install `duckdb` with exactly the bundle's version (`pip install duckdb==2.0.0`), unpack the tarball (here to `/opt/duckdb/bundle`), and run this once:

```python
import duckdb

con = duckdb.connect(
    config={
        "autoinstall_known_extensions": "false",  # never contact extensions.duckdb.org
        "allow_extension_repositories": "allowed",  # needed only to register our repository
    }
)

# Core extensions: copied into ~/.duckdb/extensions, signature checked against DuckDB's keys.
con.execute("SET custom_extension_repository = '/opt/duckdb/bundle/core'")
for name in ["httpfs", "iceberg", "avro", "sqlite_scanner"]:
    con.execute(f"INSTALL {name}")

# Custom extensions: pin our public key once (stored in ~/.duckdb/extension_repositories).
public_key = "".join(
    line for line in open("/path/to/trusted/custom.pub.pem").read().splitlines() if "-----" not in line
)
con.execute(
    "CREATE OR REPLACE EXTENSION REPOSITORY custom "
    f"WITH PREFIX '/opt/duckdb/bundle/custom' USING PUBLIC KEY '{public_key}'"
)
con.execute("INSTALL hello FROM custom")
```

After that, any process on the machine can use them:

```python
con = duckdb.connect(config={"autoinstall_known_extensions": "false"})
con.execute("ATTACH 'app.sqlite' AS app (TYPE sqlite)")  # core extensions autoload
con.execute("LOAD hello FROM custom")  # custom ones need LOAD ... FROM
con.execute("SELECT hello('duck')").fetchall()  # [('Hello, duck!',)]
```

Take the public key from a copy you trust (for example this repository once a real key is in use), not from each new tarball. `CREATE EXTENSION REPOSITORY` pins the key, so later bundles can only replace binaries that the same key signed.

## Adding a custom extension

1. Copy `extensions/hello` to `extensions/<name>` and set `name` in its `Cargo.toml` (lowercase letters, digits, underscores).
2. Change the `extension_name` in the `#[duckdb_entrypoint_c_api_v2]` attribute to the same name.
3. Run `make cargo-lock`, then `make test`, and add tests for it under `tests/`.

## Upgrading DuckDB

1. Set `DUCKDB_VERSION` in `versions.env`, and run `uv add duckdb==<version>` so `pyproject.toml` and `uv.lock` match.
2. Bump `duckdb-neo` in `Cargo.toml` to the release built for that DuckDB version, then run `make cargo-lock`.
3. Run `make lock` to pin the new core extension hashes, then `make test`.

`make pins` fails while `pyproject.toml`, `Cargo.toml` or `.python-version` disagree with `versions.env`, and `make test`, `make dist` and `make check` run it first.

Custom extensions use the C API v2 through `duckdb-neo`, which DuckDB marks `C_STRUCT_UNSTABLE`: like the official C++ extensions, they load only in the exact DuckDB version they were built for.

## Signing keys

The key must be RSA-2048 (DuckDB's signature slot is 256 bytes). Locally, `make` generates one at `keys/signing-key.pem`. In CI, store the private key as a secret and pass it with `SIGNING_KEY=path/to/key.pem make dist` (the build stops if that file is missing instead of creating a new key). It reaches the build only as a BuildKit secret, never as an image layer.

## Platforms

`make` builds for the host architecture; `ARCH=amd64` or `ARCH=arm64` overrides it. Custom extensions are compiled on AlmaLinux 8 (glibc 2.28), the same floor as DuckDB's own Linux binaries. Run the `amd64` tests on an amd64 machine (such as a GitHub-hosted Ubuntu runner): under emulation on Apple Silicon, DuckDB's signature check fails even for official extensions installed online.
