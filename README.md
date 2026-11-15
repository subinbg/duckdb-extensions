# duckdb-extensions

Builds an offline extension bundle for the DuckDB Python client, for machines that cannot reach `extensions.duckdb.org`. A bundle is one tarball (meant to be a GitHub Release asset) with:

1. DuckDB's official core extensions `httpfs`, `iceberg`, `avro` (needed by iceberg) and `sqlite_scanner`, byte for byte as DuckDB publishes them.
2. Our own extensions, written in Rust against DuckDB's stable C API: `hello`, a template, and `webui`, a browser UI that needs no network access.

## Build and test

```bash
cargo test --locked  # unit tests of our extensions
make versions        # check that the versions named across the repository fit build.env
make build           # download the official extensions and compile ours
make test            # pytest on what was built
make dist            # the build again, and the release tarball
```

`make build` calls `scripts/build.py`, and `build.env` holds every argument it is called with:

| `build.env` | `build.py` argument | Meaning |
|---|---|---|
| `DUCKDB_VERSION` | `--duckdb-version` | The DuckDB release to build for; `pyproject.toml` pins the same version of the client |
| `CORE_EXTENSIONS` | `--core-extensions`, once per name | The official extensions to download from `extensions.duckdb.org`. Left empty, none is. One already in the directory is not downloaded again |
| `CUSTOM_EXTENSIONS` | `--custom-extensions`, once per name | The crates under `extensions/` to compile. Left empty, none is |
| `CORE_DIRECTORY` | `--core-directory` | The extension directory the official extensions are written into |
| `CUSTOM_DIRECTORY` | `--custom-directory` | The extension directory ours are written into |
| `DIST_DIRECTORY` | `--dist-directory` | Where `make dist` also writes the release tarball and its SHA-256 |

`build.py` works for the machine it runs on. An extension directory is what DuckDB loads from: `<directory>/<version>/<platform>/<name>.duckdb_extension`. Nothing is installed or copied after the build.

`make test` gives the tests the two directories, as the environment variables `CORE_DIRECTORY` and `CUSTOM_DIRECTORY`. pytest run by hand needs both set.

A value given to `make` replaces the one in `build.env` for that run, such as `make build CUSTOM_EXTENSIONS=hello`.

`make versions` runs before every build, and stops it when a version named elsewhere does not fit `build.env`:

| Where | What must hold |
|---|---|
| `pyproject.toml` | The DuckDB client is pinned at `DUCKDB_VERSION` |
| `Cargo.toml` | `duckdb` and `libduckdb-sys` are the releases for `DUCKDB_VERSION` (`1.20000.x` is for `v2.0.0`) |
| `extensions/<name>/` | For each of `CUSTOM_EXTENSIONS`: the `capi-version` in its `Cargo.toml` is the one its `src/lib.rs` asks DuckDB for, and no newer than `DUCKDB_VERSION` |
| `Dockerfile` | The Python image is of the version in `.python-version` |

### Systems

A system provides cargo, make, and uv or Python 3.12. What is built and tested is the same everywhere: the commands above, with the arguments of `build.env`.

| System | Set up in | Run with |
|---|---|---|
| Your machine | [rustup](https://rustup.rs) and uv, installed once | The commands above |
| Docker | `Dockerfile` | `make docker-test`, `make docker-dist` |
| GitHub Actions | `.github/workflows/test.yml` | Every push |

`build.py` uses the standard library only. `make` runs it with `uv run`; a system without uv names its Python, as the `Dockerfile` does with `make dist PYTHON=python3.12`.

The Docker system builds on AlmaLinux 8 and tests in a second image that has Python and no Rust, with `--network none`. `make docker-dist` writes the release tarball of that build.

## Using the extensions

A process opens its database with the extension directories, from a build or from an unpacked tarball (`core/` and `custom/`):

```python
import duckdb

con = duckdb.connect(
    config={
        "extension_directories": ["/opt/duckdb/core", "/opt/duckdb/custom"],
        "allow_unsigned_extensions": True,  # needed for our extensions
        "autoinstall_known_extensions": False,  # never contact extensions.duckdb.org on its own
    }
)
con.execute("ATTACH 'app.sqlite' AS app (TYPE sqlite)")  # official extensions load on demand
con.execute("LOAD hello")  # ours are loaded by name
con.execute("SELECT hello('duck')").fetchall()  # [('Hello, duck!',)]
```

The client must be exactly the version the directories were built for (`pip install duckdb==2.0.0`). Extensions are loaded into an open database, so each newly opened database loads them again.

### Signature checks

DuckDB checks every extension it loads against its own keys, which the official extensions pass. Ours carry no signature, so a database that loads them is opened with `allow_unsigned_extensions`, and that setting turns the check off for every extension of that database.

1. Without the setting, the official extensions load as above and ours are refused.
2. With it, DuckDB checks nothing: the official files are then checked by us. `tests/test_core_extensions.py` loads each one in a database that has the check on, and the tarball's SHA-256 covers the way to another machine.

## Browser UI (`webui`)

`webui` serves a notebook UI for the database from inside the Python process, without the DuckDB CLI and without any network access:

```python
con.execute("LOAD webui")
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

`tests/test_webui_extension.py` sends the application's requests, under `make docker-test` with `--network none`. The browser application itself is not run by the tests.

## Repository layout

| Path | Purpose |
|---|---|
| `pyproject.toml`, `uv.lock` | The version of the DuckDB client, test dependencies, and ruff/pyrefly/pytest configuration |
| `extensions/<name>/` | One Rust crate per custom extension; the package name is the extension name |
| `extensions/webui/app/` | The browser application of `webui`: HTML, CSS and JavaScript files that the extension includes and serves as they are written |
| `build.env` | The arguments of `scripts/build.py`: DuckDB version, official extensions, directories |
| `Makefile` | The build and test commands, and how `docker` is called for the Docker system |
| `scripts/build.py` | Downloads the official extensions, compiles ours and appends DuckDB's extension metadata to each library |
| `scripts/argpr.py` | Builds the script's command line from its typed function signature (standard library only, on top of `argparse`) |
| `tests/` | pytest suite (`conftest.py` takes the two directories from the environment, `support.py` holds shared helpers) |
| `Dockerfile` | The Docker system |
| `.github/workflows/test.yml` | The GitHub Actions system |

## Python tooling

The Python code (`scripts/`, `tests/`) is a uv project (`pyproject.toml`, `package = false`: dependencies only, nothing is built or installed as a package). Its one runtime dependency is `duckdb`; the groups `test` (pytest) and `dev` (pytest, ruff and pyrefly) add to it. `scripts/build.py` uses the standard library only.

```bash
uv run ruff format        # format
uv run ruff check --fix   # lint
uv run pyrefly check      # type check
make check                # all three as checks, and that uv.lock is current
```

## Adding a custom extension

1. Copy `extensions/hello` to `extensions/<name>` and set `name` in its `Cargo.toml` (lowercase letters, digits, underscores).
2. Change `ext_name` in the `#[duckdb_entrypoint_c_api]` attribute to the same name.
3. Add the name to `CUSTOM_EXTENSIONS` in `build.env`, and run `cargo build`, which adds the crate to `Cargo.lock`.
4. Build and test as above, and add tests for it under `tests/`.

Both extensions are built on DuckDB's stable C API (ABI `C_STRUCT`), at version 1.5.6, and load in every DuckDB release that provides it. `hello` uses the safe wrappers of the `duckdb` crate. `webui` calls the C API itself through `libduckdb-sys`, for the connections it opens of its own. Each crate names the API version twice: where its entry point asks DuckDB for it, and as `capi-version` in its `Cargo.toml`, which `scripts/build.py` writes into the extension file.

## Upgrading DuckDB

1. Set `DUCKDB_VERSION` in `build.env`.
2. Run `uv add duckdb==<version>`, so `pyproject.toml` and `uv.lock` name the new client.
3. Raise `duckdb` and `libduckdb-sys` in `Cargo.toml` to the releases for that version, and run `cargo build`, which updates `Cargo.lock`.
4. Build and test as above. `make versions` stops the build while one of these still differs.

The C API version of the crates can stay: a later DuckDB release keeps providing it. To use a newer C API, raise it in each crate, in `Cargo.toml` and in `src/lib.rs`.

## Platforms

`build.py` builds for the machine it runs on, Linux or macOS on amd64 or arm64. Extensions of several platforms and versions can share a directory, since each has its own subdirectory.

A release for Linux comes from the Docker system, which compiles on AlmaLinux 8 (glibc 2.28), the same floor as DuckDB's own Linux binaries; an extension compiled on a newer Linux needs at least that system's glibc. The containers are of the host architecture; `ARCH=amd64` or `ARCH=arm64` overrides it. Run the `amd64` tests on an amd64 machine (such as a GitHub-hosted Ubuntu runner): under emulation on Apple Silicon, DuckDB's signature check fails even for official extensions installed online.
