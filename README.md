# duckdb-extensions

Builds an offline extension bundle for the DuckDB Python client, for servers that cannot reach `extensions.duckdb.org`. A bundle is one tarball (meant to be a GitHub Release asset) with:

1. DuckDB's official core extensions `httpfs`, `iceberg`, `avro` (needed by iceberg) and `sqlite_scanner`, byte for byte as DuckDB publishes them and pinned by SHA-256.
2. Our own extensions, written in Rust against DuckDB's stable C API and signed with our own RSA key: `hello`, a template, and `webui`, a browser UI that needs no network access.

Building, installing and testing take cargo and [uv](https://docs.astral.sh/uv/), on any system that has them.

## Build, install, test

```bash
uv sync                         # Python dependencies, into .venv
uv run scripts/fetch_core.py    # download the official extensions; the only step that needs a network
cargo test --locked             # unit tests of our extensions
uv run scripts/build_custom.py  # compile our extensions, and sign them
uv run pytest                   # install both kinds into a temporary directory, and test what loads

uv run scripts/install.py       # install both kinds for use
uv run scripts/bundle.py        # pack both kinds into dist/ for a release
```

The commands take no arguments. Each works on the DuckDB version and platform of the installed Python client, and in the paths below, which a system sets as environment variables; `--help` shows the same paths as flags.

| Variable | Default, in this repository | Written by | Read by |
|---|---|---|---|
| `DUCKDB_CORE_REPOSITORY` | `build/repository/core` | `fetch_core.py`: the downloaded official extensions | `install.py`, tests |
| `DUCKDB_CUSTOM_REPOSITORY` | `build/repository/custom` | `build_custom.py`: our signed extensions and their public key | `install.py`, tests |
| `DUCKDB_CORE_DIRECTORY` | `build/extensions/core` | `install.py`: the installed official extensions | DuckDB, to load them |
| `DUCKDB_CUSTOM_DIRECTORY` | `build/extensions/custom` | `install.py`: our installed extensions, and the key DuckDB trusts for them | DuckDB, to load them |
| `DUCKDB_SIGNING_KEY` | `keys/signing-key.pem` | `build_custom.py`, once, as a development key | `build_custom.py` |

DuckDB installs an extension from a repository into an extension directory, and loads it from there. Both repositories are laid out as DuckDB reads them, `<repository>/<version>/<platform>/<name>.duckdb_extension`.

### Systems

A system provides cargo and uv, and sets the variables it wants to change. What is built and tested is the same everywhere: the commands above.

| System | Set up in | Run with |
|---|---|---|
| Your machine | [rustup](https://rustup.rs) and uv, installed once | The commands above |
| Docker | `Dockerfile` for the images and tools, `Makefile` for how `docker` is called | `make test`, `make dist` |
| GitHub Actions | `.github/workflows/test.yml` | Every push |

The Docker system builds on AlmaLinux 8 and tests in a second image that has Python and no Rust, with `--network none`. `make dist` writes the release tarball of that build to `dist/`.

## Using the extensions

After `install.py`, a process opens its database with the two extension directories:

```python
import duckdb

con = duckdb.connect(
    config={
        "extension_directories": ["/data/duckdb/core", "/data/duckdb/custom"],
        "extension_repository_directory": "/data/duckdb/custom",  # the key our extensions are checked against
        "autoinstall_known_extensions": False,  # never contact extensions.duckdb.org
    }
)
con.execute("ATTACH 'app.sqlite' AS app (TYPE sqlite)")  # official extensions load on demand
con.execute("LOAD hello FROM custom")  # ours need LOAD ... FROM
con.execute("SELECT hello('duck')").fetchall()  # [('Hello, duck!',)]
```

`settings()` in `scripts/install.py` returns this configuration for the configured directories. Signature checks stay on: DuckDB verifies the official extensions against its own keys, and ours against the public key that `install.py` registered under the name `custom`.

### On a server

A server installs from a bundle instead of building. It needs Python with `duckdb` at exactly the bundle's version (`pip install duckdb==2.0.0`) and a checkout of this repository for `scripts/install.py`.

```bash
mkdir -p /opt/duckdb/bundle && tar -xzf duckdb-extensions-v2.0.0-linux_amd64.tar.gz -C /opt/duckdb/bundle

export DUCKDB_CORE_REPOSITORY=/opt/duckdb/bundle/core DUCKDB_CUSTOM_REPOSITORY=/opt/duckdb/bundle/custom
export DUCKDB_CORE_DIRECTORY=/data/duckdb/core DUCKDB_CUSTOM_DIRECTORY=/data/duckdb/custom
python scripts/install.py
```

| Path in the tarball | Contents |
|---|---|
| `core/<version>/<platform>/*.duckdb_extension` | Official extensions, signed by DuckDB |
| `custom/<version>/<platform>/*.duckdb_extension` | Our extensions, signed with our key |
| `custom/public-key.pem` | Our public key |

`install.py` registers the public key it finds in the bundle. Before installing a new bundle, compare that file with a copy you trust (for example this repository once a real key is in use): a bundle from someone else carries their key.

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
| Browser application | Downloaded on every page load, about 10 MB | Included in the extension, 65 KB; the page loads nothing from another host |
| Rows of a result | The first 50,000 | All of them: the grid loads rows as it scrolls, and the result is cached in DuckDB, which moves it to disk when memory runs short |
| Sort and filter | In SQL only | From the column headers, computed by DuckDB over every row |
| Row count and column summary | Computed after every run, on the notebook's connection | Count in the background on another connection, the `SUMMARIZE` of a column on request; neither delays the next cell |
| A long text value | The first 10,000 characters, unless switched to raw view | Whole |
| A nested value | Tree; a `UNION` shows as empty | Tree for structs, lists, maps, unions and JSON |
| Export | The first 50,000 rows | Every row, as CSV, Parquet or JSON |
| Access control | None: any local process can run SQL | A random token, part of the URL, required by every request |
| Databases attached to your session | Three, for its own use | None |
| Size of the extension | 56 MB | 1.0 MB |

### Options

| `webui_start` parameter | Default | Meaning |
|---|---|---|
| `port` | 4213, or the next free port up to 4233 | Port to listen on; a port given here is used or the call fails |
| `host` | `127.0.0.1` | Address to listen on |
| `token` | 32 random hexadecimal characters | The access token |
| `state` | `~/.duckdb/extension_data/webui` | Directory where the notebook is saved; created if missing |
| `open` | `false` | Also open the URL in the default browser of the machine |

### How it works

`extensions/webui` is one Rust crate, the extension, and one directory of files for the browser.

| Part | Where | What it is |
|---|---|---|
| Server | `src/server.rs` | Serves the application's files, and answers the requests it posts to `/api` |
| Protocol | `src/protocol.rs` | One `Request` enum, and the type of the answer to each of its variants |
| Engine | `src/engine/`, `src/duck.rs` | Runs SQL and holds results, through wrappers over DuckDB's C API |
| Application | `app/` | The page: `index.html`, `style.css`, and JavaScript modules for the catalog tree, cells, editor, grid and inspector |

What reaches the server is one file, `webui.duckdb_extension`:

1. Compiling the extension includes the files of `app/` in it as they are written. Nothing compiles or bundles them, and the application has no dependencies.
2. `LOAD webui` makes DuckDB open that one file and call `webui_init_c_api`, which opens the extension's connections and registers `webui_start`, `webui_stop` and `webui_status`.
3. `CALL webui_start()` starts the HTTP server. The browser fetches the included files from it, and `app.js` imports the other modules.
4. The page posts each request as JSON to `/api`, such as `{"page": {"result": 1, "offset": 0, "limit": 200}}`, and reads the answer by the field names of `src/protocol.rs`.

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

`tests/test_webui_extension.py` sends the application's requests, under `make test` with `--network none`. The browser application itself is not run by the tests.

## Repository layout

| Path | Purpose |
|---|---|
| `pyproject.toml`, `uv.lock` | The DuckDB version, the core extensions to mirror, Python dependencies, and ruff/pyrefly/pytest configuration |
| `core-extensions.sha256` | Pinned hashes of the official binaries, for every platform that `pyproject.toml` lists |
| `extensions/<name>/` | One Rust crate per custom extension; the package name is the extension name |
| `extensions/webui/app/` | The browser application of `webui`: HTML, CSS and JavaScript files that the extension includes and serves as they are written |
| `scripts/paths.py` | The paths of the table above, and the version and platform of the installed DuckDB client |
| `scripts/fetch_core.py` | Downloads core extensions and checks them against the lock file |
| `scripts/build_custom.py` | Compiles the crates, appends DuckDB's extension metadata to each library and signs it |
| `scripts/install.py` | Installs both repositories into extension directories |
| `scripts/bundle.py` | Packs both repositories into the release tarball |
| `scripts/argpr.py` | Builds the scripts' command lines from their typed function signatures (standard library only, on top of `argparse`) |
| `tests/` | pytest suite; its fixtures install with `scripts/install.py` (`support.py` holds shared types and helpers) |
| `Dockerfile`, `Makefile` | The Docker system |
| `.github/workflows/test.yml` | The GitHub Actions system |

## Python tooling

The Python code (`scripts/`, `tests/`) is a uv project (`pyproject.toml`, `package = false`: dependencies only, nothing is built or installed as a package). Its one runtime dependency is `duckdb`, which is all `install.py` needs. Three groups add to it: `build` (cryptography, which signs our extensions), `test` (pytest) and `dev` (both, plus ruff and pyrefly). `uv sync` installs everything; the Docker images install one group each, from `uv.lock` (hash-checked, and the build fails if `uv.lock` is out of date with `pyproject.toml`).

```bash
uv run ruff format        # format
uv run ruff check --fix   # lint
uv run pyrefly check      # type check
make check                # all three as checks, and that uv.lock is current
```

## Adding a custom extension

1. Copy `extensions/hello` to `extensions/<name>` and set `name` in its `Cargo.toml` (lowercase letters, digits, underscores).
2. Change `ext_name` in the `#[duckdb_entrypoint_c_api]` attribute to the same name.
3. Run `cargo build`, which adds the crate to `Cargo.lock`, then build and test as above, and add tests for it under `tests/`.

Both extensions are built on DuckDB's stable C API (ABI `C_STRUCT`), at version 1.5.6, and load in every DuckDB release that provides it. `hello` uses the safe wrappers of the `duckdb` crate. `webui` calls the C API itself through `libduckdb-sys`, for the connections it opens of its own. Each crate names the API version twice: where its entry point asks DuckDB for it, and as `capi-version` in its `Cargo.toml`, which `scripts/build_custom.py` writes into the extension file.

## Upgrading DuckDB

1. Run `uv add duckdb==<version>`, so `pyproject.toml` and `uv.lock` name the new version. Every script takes the version from the installed client.
2. Run `uv run scripts/fetch_core.py --update-lock` to pin the new core extension hashes.
3. Build and test as above.

The Rust crates need no change for a DuckDB release that still provides their C API version. To use a newer C API, raise `duckdb` and `libduckdb-sys` in `Cargo.toml` together, and the API version in each crate.

## Signing keys

The key must be RSA-2048 (DuckDB's signature slot is 256 bytes). `build_custom.py` creates a development key at `keys/signing-key.pem` (ignored by git) when there is none. For a release, point `DUCKDB_SIGNING_KEY` at the private key; the build stops if that file is missing instead of creating a new key.

Under Docker the same holds for `SIGNING_KEY=path/to/key.pem make dist`. The key reaches the build only as a BuildKit secret, never as an image layer. Without `SIGNING_KEY`, `make` passes the development key of the checkout, and without that the build signs with a development key of its own.

## Platforms

Every command works for the platform it runs on: the scripts ask the installed DuckDB client for it. Extensions of several platforms can share a repository directory, since each has its own subdirectory.

A release for Linux comes from the Docker system, which compiles on AlmaLinux 8 (glibc 2.28), the same floor as DuckDB's own Linux binaries; an extension compiled on a newer Linux needs at least that system's glibc. `make` builds for the host architecture; `ARCH=amd64` or `ARCH=arm64` overrides it. Run the `amd64` tests on an amd64 machine (such as a GitHub-hosted Ubuntu runner): under emulation on Apple Silicon, DuckDB's signature check fails even for official extensions installed online.
