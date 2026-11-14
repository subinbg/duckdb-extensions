# duckdb-extensions

Builds an offline extension bundle for the DuckDB Python client, for servers that cannot reach `extensions.duckdb.org`. A bundle is one tarball (meant to be a GitHub Release asset) with:

1. DuckDB's official core extensions `httpfs`, `iceberg`, `avro` (needed by iceberg) and `sqlite_scanner`, byte for byte as DuckDB publishes them and pinned by SHA-256.
2. Our own extensions, written in Rust against DuckDB's C API and signed with our own RSA key: `hello`, a template, and `webui`, a browser UI that needs no network access.

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
| `versions.env` | Every pinned input: DuckDB version, core extension list, wasm-bindgen version and container images (`make` passes them to `docker build`) |
| `pyproject.toml`, `uv.lock` | Python dependencies (also what the test container installs) and ruff/pyrefly configuration |
| `core-extensions.sha256` | Pinned hashes of the official binaries for `linux_amd64` and `linux_arm64` |
| `extensions/<name>/` | One Rust crate per custom extension; the package name is the extension name |
| `extensions/webui/protocol/` | The requests and answers that `webui` and its browser application exchange, and the SQL lexer both use |
| `extensions/webui/app/` | The browser application of `webui`, a Rust crate compiled to WebAssembly and embedded in the extension |
| `scripts/argpr.py` | Builds the scripts' command lines from their typed function signatures (standard library only, on top of `argparse`) |
| `scripts/fetch_core.py` | Downloads core extensions and checks them against the lock file |
| `scripts/package_extension.py` | Appends DuckDB's extension metadata to a compiled library and signs it |
| `Dockerfile` | Stages `build-web`, `build-custom`, `fetch-core`, `package`, `dist`, `test` |
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
con.execute("INSTALL webui FROM custom")
```

After that, any process on the machine can use them:

```python
con = duckdb.connect(config={"autoinstall_known_extensions": "false"})
con.execute("ATTACH 'app.sqlite' AS app (TYPE sqlite)")  # core extensions autoload
con.execute("LOAD hello FROM custom")  # custom ones need LOAD ... FROM
con.execute("SELECT hello('duck')").fetchall()  # [('Hello, duck!',)]
```

Take the public key from a copy you trust (for example this repository once a real key is in use), not from each new tarball. `CREATE EXTENSION REPOSITORY` pins the key, so later bundles can only replace binaries that the same key signed.

## Browser UI (`webui`)

`webui` serves a notebook UI for the database from inside the Python process, without the DuckDB CLI and without any network access:

```python
con.execute("LOAD webui FROM custom")
(url,) = con.execute("CALL webui_start()").fetchone()  # http://127.0.0.1:4213/?token=...
```

Open the URL in a browser. From another machine, forward the port first (`ssh -L 4213:127.0.0.1:4213 <server>`); any local port works.
`CALL webui_status()` returns the URL again, or NULL when nothing is served, and `CALL webui_stop()` stops the server.
The UI works on the same database as `con`, and stops when the last connection other than its own closes.

It replaces DuckDB's `ui` extension, which is left out of the bundle: that extension downloads its browser application from `https://ui.duckdb.org` on every page load, so its page does not load on a server without that access.

| | DuckDB `ui` | `webui` |
|---|---|---|
| Browser application | Downloaded on every page load, about 10 MB | Embedded in the extension, 0.7 MB; the page loads nothing from another host |
| Rows of a result | The first 50,000 | All of them: the grid loads rows as it scrolls, and the result is cached in DuckDB, which moves it to disk when memory runs short |
| Sort and filter | In SQL only | From the column headers, computed by DuckDB over every row |
| Row count and column summary | Computed after every run, on the notebook's connection | Count in the background on another connection, the `SUMMARIZE` of a column on request; neither delays the next cell |
| A long text value | The first 10,000 characters, unless switched to raw view | Whole |
| A nested value | Tree; a `UNION` shows as empty | Tree for structs, lists, maps, unions and JSON |
| Export | The first 50,000 rows | Every row, as CSV, Parquet or JSON |
| Access control | None: any local process can run SQL | A random token, part of the URL, required by every request |
| Databases attached to your session | Three, for its own use | None |
| Size of the extension | 56 MB | 1.6 MB |

### Options

| `webui_start` parameter | Default | Meaning |
|---|---|---|
| `port` | 4213, or the next free port up to 4233 | Port to listen on; a port given here is used or the call fails |
| `host` | `127.0.0.1` | Address to listen on |
| `token` | 32 random hexadecimal characters | The access token |
| `state` | `~/.duckdb/extension_data/webui` | Directory where the notebook is saved; created if missing |
| `open` | `false` | Also open the URL in the default browser of the machine |

### How it works

`extensions/webui` holds three crates. Only the first is an extension; the other two are parts of it.

| Crate | Directory | Compiled for | Depends on | Role |
|---|---|---|---|---|
| `webui` | `extensions/webui` | The server, as the extension | `webui-protocol`, `libduckdb-sys`, `tiny_http`, `serde_json` | The HTTP server (`server.rs`), and the engine that runs SQL (`engine/`) through wrappers over DuckDB's C API (`duck.rs`) |
| `webui-protocol` | `extensions/webui/protocol` | Both | `serde` | One `Request` enum, the type of each answer, and the SQL lexer |
| `webui-app` | `extensions/webui/app` | The browser, as WebAssembly | `webui-protocol`, `leptos`, `gloo-net`, `serde_json` | The page: catalog tree, cells, editor, grid, inspector |

What reaches the server is one file, `webui.duckdb_extension`:

1. The build compiles `webui-app` to WebAssembly first, and `wasm-bindgen` writes the module and the JavaScript that loads it.
2. `build.rs` of `webui` embeds those files and `app/static/` in the extension as bytes. `webui-protocol` is linked into the extension as ordinary Rust code, and a second copy of it is compiled into the WebAssembly module.
3. `LOAD webui` makes DuckDB open that one file and call `webui_init_c_api`, which opens the extension's connections and registers `webui_start`, `webui_stop` and `webui_status`. Neither helper crate exists as a file at this point.
4. `CALL webui_start()` starts the HTTP server. The browser fetches the embedded files from it and runs the module, which posts `Request` values as JSON to `/api` and reads each answer as the type the protocol names for it.

Because both sides compile the same `webui-protocol`, a request or answer that one side changes and the other misses is a compile error.

The engine runs SQL on four connections of its own:

1. The notebook's cells run on one of them, so `SET`, `USE`, temporary tables and transactions carry from cell to cell. A cell's last statement is streamed: DuckDB computes rows only as the grid asks for them.
2. Streamed rows are appended, unconverted, to a temporary table on a second connection. DuckDB gives that table the column types of the result, and names them for the page. Pages are read from it by row number, with every value cast to text by DuckDB.
3. The session streams one statement at a time. When another cell runs, the open one stops growing. Scrolling it further runs it again as a subquery that skips the rows already cached; a statement that took more than a second is run again only on request.
4. A sort or filter on an unfinished result is added to the statement and streamed anew; on a finished result it is computed from the cache. Filter values are untyped literals, which DuckDB reads as the type of their column.
5. Counts, column summaries and exports of an unfinished result run on a third connection, and the catalog is read on a fourth, so none of them waits for a running cell.

### Limits

| Limit | Detail |
|---|---|
| One server per process | A second database of the same process can load `webui`, but `webui_start()` fails there until the first one stops. |
| One notebook | Cells are saved to `notebook.json` in the state directory; there is no list of notebooks. |
| Temporary tables | The row count and column summary of an unfinished result run on another connection, which does not see the notebook's temporary tables. For such a statement the count is not shown, and a summary covers the rows loaded so far. |
| Running a statement again | Loading more rows of a statement that is no longer open assumes it returns the same rows in the same order. |
| Charts, MotherDuck | Not included. |

`tests/test_webui_extension.py` sends the application's requests with `--network none`. The browser application itself is not run by the tests.

## Adding a custom extension

1. Copy `extensions/hello` to `extensions/<name>` and set `name` in its `Cargo.toml` (lowercase letters, digits, underscores).
2. Change the `extension_name` in the `#[duckdb_entrypoint_c_api_v2]` attribute to the same name.
3. Run `make cargo-lock`, then `make test`, and add tests for it under `tests/`.

An extension that runs SQL on its own cannot use that template: the C API v2 gives an extension no way to open a connection.
`webui` is built on the stable C API (v1) for that reason, through `libduckdb-sys`, and says so under `[package.metadata.duckdb]` in its `Cargo.toml`, which `scripts/package_extension.py` reads.

## Upgrading DuckDB

1. Set `DUCKDB_VERSION` in `versions.env`, and run `uv add duckdb==<version>` so `pyproject.toml` and `uv.lock` match.
2. Bump `duckdb-neo` and `libduckdb-sys` in `Cargo.toml` to the releases built for that DuckDB version, then run `make cargo-lock`.
3. Run `make lock` to pin the new core extension hashes, then `make test`.

`make pins` fails while `pyproject.toml`, the `Cargo.toml` files or `.python-version` disagree with `versions.env`, and `make test`, `make dist` and `make check` run it first.

`hello` uses the C API v2 through `duckdb-neo`, which DuckDB marks `C_STRUCT_UNSTABLE`: like the official C++ extensions, it loads only in the exact DuckDB version it was built for.
`webui` uses the stable C API at version 1.5.6 (`C_STRUCT`), and loads in every DuckDB release that provides it.

## Signing keys

The key must be RSA-2048 (DuckDB's signature slot is 256 bytes). Locally, `make` generates one at `keys/signing-key.pem`. In CI, store the private key as a secret and pass it with `SIGNING_KEY=path/to/key.pem make dist` (the build stops if that file is missing instead of creating a new key). It reaches the build only as a BuildKit secret, never as an image layer.

## Platforms

`make` builds for the host architecture; `ARCH=amd64` or `ARCH=arm64` overrides it. Custom extensions are compiled on AlmaLinux 8 (glibc 2.28), the same floor as DuckDB's own Linux binaries. Run the `amd64` tests on an amd64 machine (such as a GitHub-hosted Ubuntu runner): under emulation on Apple Silicon, DuckDB's signature check fails even for official extensions installed online.
