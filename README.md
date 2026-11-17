# duckdb-extensions

Builds our own extensions for the DuckDB Python client, and releases them as a bundle: one tarball per platform, a GitHub Release asset. They are written in Rust against DuckDB's stable C API: `hello`, a template, `webui`, a browser UI that needs no network access, and `bigquery`, which reads Google BigQuery.

DuckDB's official core extensions `httpfs`, `aws`, `iceberg`, `avro` (needed by iceberg), `sqlite_scanner` and `postgres_scanner` are downloaded to test ours with, and left out of the bundle: DuckDB installs them itself.

## Build and test

```bash
cargo test --locked  # unit tests of our extensions
make versions        # check that the versions named across the repository fit build.env
make build           # download the official extensions and compile ours
make test            # pytest on what was built
make dist            # compile ours, and the release tarball of them
```

`make build` calls `scripts/build.py`, and `build.env` holds every argument it is called with:

| `build.env` | `build.py` argument | Meaning |
|---|---|---|
| `DUCKDB_VERSION` | `--duckdb-version` | The DuckDB release to build for; `pyproject.toml` pins the same version of the client |
| `CORE_EXTENSIONS` | `--core-extensions`, once per name | The official extensions to download from `extensions.duckdb.org`. Left empty, none is. One already in the directory is not downloaded again |
| `CUSTOM_EXTENSIONS` | `--custom-extensions`, once per name | The crates under `extensions/` to compile. Left empty, none is |
| `CORE_DIRECTORY` | `--core-directory` | The extension directory the official extensions are written into |
| `CUSTOM_DIRECTORY` | `--custom-directory` | The extension directory ours are written into |
| `DIST_DIRECTORY` | `--dist-directory` | Where `make dist` writes the release tarball of our extensions, and its SHA-256 |

`build.py` works for the machine it runs on. An extension directory is what DuckDB loads from: `<directory>/<version>/<platform>/<name>.duckdb_extension`. Nothing is installed or copied after the build.

`make test` gives the tests the two directories, as the environment variables `CORE_DIRECTORY` and `CUSTOM_DIRECTORY`. pytest run by hand needs both set. It also gives `DIST_DIRECTORY`, where `tests/test_dist.py` checks the release tarball of the machine, when `make dist` wrote one.

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
| GitHub Actions, releases | `.github/workflows/release.yml` | A tag `release-<DUCKDB_VERSION>[-<suffix>]`, such as `release-v2.0.0`; by hand, it builds the tarballs without releasing them |

`build.py` uses the standard library only. `make` runs it with `uv run`; a system without uv names its Python, as the `Dockerfile` does with `make dist PYTHON=python3.12`.

The Docker system builds on AlmaLinux 8 and tests in a second image that has Python and no Rust, with `--network none`. `make docker-dist` writes the release tarball of that build.

## Using the extensions

A release holds one tarball per platform, `duckdb-extensions-<DuckDB version>-<platform>.tar.gz` for `linux_amd64` and `osx_arm64`, and `SHA256SUMS`. Unpacked, a tarball is an extension directory:

```bash
gh release download release-v2.0.0 -R subinbg/duckdb-extensions -p '*osx_arm64*' -p SHA256SUMS
shasum -a 256 --check --ignore-missing SHA256SUMS
mkdir -p /opt/duckdb/extensions && tar -xzf duckdb-extensions-v2.0.0-osx_arm64.tar.gz -C /opt/duckdb/extensions
```

A process opens its database with that directory, or with `build/custom` of a build:

```python
import duckdb

con = duckdb.connect(
    config={
        "extension_directories": ["/opt/duckdb/extensions"],
        "allow_unsigned_extensions": True,  # needed for our extensions
    }
)
con.execute("LOAD hello")  # ours are loaded by name
con.execute("SELECT hello('duck')").fetchall()  # [('Hello, duck!',)]
con.execute("ATTACH 'app.sqlite' AS app (TYPE sqlite)")  # official extensions install and load as DuckDB's always do
```

The client must be exactly the version the directories were built for (`pip install duckdb==2.0.0`). Extensions are loaded into an open database, so each newly opened database loads them again.

### Cloud storage

`httpfs` reads and writes S3 and Google Cloud Storage, and `iceberg` reads Iceberg tables stored there. Each signs in without a key written anywhere:

| | Amazon S3 | Google Cloud Storage |
|---|---|---|
| Sign in | `LOAD aws`, or `CREATE SECRET (TYPE s3, PROVIDER credential_chain)`: AWS's credential chain, which reads `aws configure`, SSO and the environment | `CREATE SECRET (TYPE gcs, BEARER_TOKEN '...')`, with the token `gcloud auth application-default print-access-token` prints; it expires within the hour. DuckDB's `credential_chain` for GCS is AWS's, which GCS refuses |
| Files | `read_parquet`, `read_csv`, `glob` and `COPY ... TO` on `s3://` paths | The same on `gs://` paths |
| Iceberg | `iceberg_scan` of a table's metadata `.json` file. A table's folder needs a `version-hint.text`, which PyIceberg does not write, or `SET unsafe_enable_version_guessing = true` | The same |

### Signature checks

DuckDB checks every extension it loads against its own keys, which the official extensions pass. Ours carry no signature, so a database that loads them is opened with `allow_unsigned_extensions`, and that setting turns the check off for every extension of that database.

1. Without the setting, the official extensions load and ours are refused.
2. With it, DuckDB checks nothing, the official extensions included. The release's `SHA256SUMS` covers ours on their way to another machine.

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
| Browser application | Downloaded on every page load, about 10 MB | Included in the extension, 80 KB; the page loads nothing from another host |
| Rows of a result | The first 50,000 | All of them: the grid loads rows as it scrolls, and the result is cached in DuckDB, which moves it to disk when memory runs short |
| Sort and filter | In SQL only | From the column headers, computed by DuckDB over every row |
| Row count and column summary | Computed after every run, on the notebook's connection | Rows are counted once all are loaded: counting an unfinished result would read its whole source. The `SUMMARIZE` of a column on request, on another connection, so it does not delay the next cell |
| A long text value | The first 10,000 characters, unless switched to raw view | Whole |
| A nested value | Tree; a `UNION` shows as empty | Tree for structs, lists, maps, unions and JSON |
| Export | The first 50,000 rows | Every row, as CSV, Parquet or JSON |
| Reading a source | Written by hand | **+ Add a source** asks for the fields of a template and adds a cell with its SQL: it attaches a DuckDB, SQLite or PostgreSQL database, or creates a view of Parquet, CSV, JSON, Avro or Iceberg files or of a BigQuery table or query, which reads no rows until it is queried |
| Access control | None: any local process can run SQL | A random token, part of the URL, required by every request |
| Databases attached to your session | Three, for its own use | None |
| Size of the extension | 56 MB | 1.0 MB |

The templates keep nothing: no connection and no secret is saved, and the cells they add hold no password. A PostgreSQL password comes from the `~/.pgpass` file or the `PGPASSWORD` variable of the DuckDB process, BigQuery signs in as `bigquery` describes, and a file template for an `s3://` path loads `aws` first, which signs in as [Cloud storage](#cloud-storage) describes. GCS takes a token, which the process that starts the UI gives in a secret, so that no cell holds it.

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
| Application | `app/` | The page: `index.html`, `style.css`, and JavaScript modules for the catalog tree, cells, editor, grid, inspector and source templates |

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
5. Column summaries and exports of an unfinished result run on a third connection, and the catalog is read on a fourth, so none of them waits for a running cell.

### Limits

| Limit | Detail |
|---|---|
| One server per process | A second database of the same process can load `webui`, but `webui_start()` fails there until the first one stops. |
| One notebook | Cells are saved to `notebook.json` in the state directory; there is no list of notebooks. |
| Temporary tables | The column summary of an unfinished result runs on another connection, which does not see the notebook's temporary tables. For such a statement a summary covers the rows loaded so far. |
| Running a statement again | Loading more rows of a statement that is no longer open assumes it returns the same rows in the same order. |
| Charts, MotherDuck | Not included. |

`tests/test_webui_extension.py` sends the application's requests, under `make docker-test` with `--network none`. The browser application itself is not run by the tests.

## BigQuery (`bigquery`)

`bigquery` reads Google BigQuery through Google's own Rust SDK (`google-cloud-bigquery`): it lists datasets and tables, reads tables, and runs queries, over views as well.

```sql
LOAD bigquery;
FROM bigquery_datasets('my-project');                    -- dataset, location
FROM bigquery_tables('my-project.my_dataset');           -- table, type (TABLE, VIEW, MATERIALIZED_VIEW, ...), created
FROM bigquery_scan('my-project.my_dataset.my_table');    -- the rows of a table
FROM bigquery_query('my-project', 'SELECT * FROM my_dataset.my_view WHERE ...');  -- GoogleSQL, run by BigQuery
```

| Function | Reads with | Detail |
|---|---|---|
| `bigquery_datasets(project)` | The BigQuery API | No cost |
| `bigquery_tables('project.dataset')` | The BigQuery API | No cost |
| `bigquery_scan('project.dataset.table', filter := ..., billing_project := ...)` | The Storage Read API, as Arrow | Only the columns DuckDB needs are read, by up to one stream per core. `filter` is a row restriction in GoogleSQL, such as `'year > 2020'`, applied by BigQuery. `billing_project` pays for the read, by default the table's project; set it to read another's tables, such as `bigquery-public-data`. Logical views cannot be read this way |
| `bigquery_query(project, sql)` | A query job, then its result through the Storage Read API | Only a `SELECT` is run: a dry run first tells the statement's type. The rows keep BigQuery's order. `project` runs the job and pays for it |

`DESCRIBE FROM bigquery_scan(...)` gives the columns of a table without reading its rows, and `INFORMATION_SCHEMA` those of views and more, as in `bigquery_query('my-project', 'SELECT * FROM my_dataset.INFORMATION_SCHEMA.COLUMNS')`. BigQuery's types become DuckDB's: `BIGNUMERIC`, whose 76 digits do not fit DuckDB's `DECIMAL`, and `GEOGRAPHY` (WKT) and `JSON` become `VARCHAR`; `RANGE` becomes a struct of `start` and `end`.

### Credentials

The extension holds no secret: it signs its calls with Google's [Application Default Credentials](https://cloud.google.com/docs/authentication/application-default-credentials), as Google's own libraries and tools do. On a workstation, sign in once with the Google Cloud CLI:

```bash
gcloud auth application-default login
```

The CLI writes a refresh token to `~/.config/gcloud/application_default_credentials.json`, readable only by you; the extension exchanges it for access tokens that expire within the hour. `gcloud auth application-default revoke` signs out. Elsewhere the same code finds, in order, the file named by `GOOGLE_APPLICATION_CREDENTIALS` (such as a Workload Identity Federation configuration), and the service account of a Google Cloud machine. Prefer those to a downloaded service account key, which does not expire.

To make Google itself refuse every write, read with an identity that may only read: create a service account with the roles BigQuery Data Viewer, BigQuery Job User and BigQuery Read Session User, and sign in as it with `gcloud auth application-default login --impersonate-service-account=<its email>`, which needs the role Service Account Token Creator on it.

Without credentials, the first call waits about a minute for the metadata server of a Google Cloud machine before it fails, with the advice to sign in.

### Limits

| Limit | Detail |
|---|---|
| Reading only | Nothing writes to BigQuery: no `ATTACH`, `INSERT` or `COPY`, which DuckDB's stable C API does not let an extension provide. |
| Filters | DuckDB's `WHERE` is applied by DuckDB after the columns arrive; `filter` is what BigQuery applies. |
| Query results | `bigquery_query` waits for the job to finish before DuckDB gets its first row, and reads its result with one stream to keep its order. |
| Size | The extension is 18 MB: the SDK, gRPC, TLS and Arrow are compiled into it. |

`tests/test_bigquery_extension.py` checks what needs no network everywhere. With `BIGQUERY_BILLING_PROJECT` set to a project that pays for the reads, a few megabytes of Google's public datasets per run, it also tests against BigQuery: `BIGQUERY_BILLING_PROJECT=my-project make test`.

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
| `.github/workflows/release.yml` | The releases: the tarball of each platform, tested, as assets of a GitHub Release |

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

The extensions are built on DuckDB's stable C API (ABI `C_STRUCT`), at version 1.5.6, and load in every DuckDB release that provides it. `hello` and `bigquery` use the safe wrappers of the `duckdb` crate. `webui` calls the C API itself through `libduckdb-sys`, for the connections it opens of its own. Each crate names the API version twice: where its entry point asks DuckDB for it, and as `capi-version` in its `Cargo.toml`, which `scripts/build.py` writes into the extension file.

## Upgrading DuckDB

1. Set `DUCKDB_VERSION` in `build.env`.
2. Run `uv add duckdb==<version>`, so `pyproject.toml` and `uv.lock` name the new client.
3. Raise `duckdb` and `libduckdb-sys` in `Cargo.toml` to the releases for that version, and run `cargo build`, which updates `Cargo.lock`.
4. Build and test as above. `make versions` stops the build while one of these still differs.

The C API version of the crates can stay: a later DuckDB release keeps providing it. To use a newer C API, raise it in each crate, in `Cargo.toml` and in `src/lib.rs`.

## Platforms

`build.py` builds for the machine it runs on, Linux or macOS on amd64 or arm64. Extensions of several platforms and versions can share a directory, since each has its own subdirectory.

A release for Linux comes from the Docker system, which compiles on AlmaLinux 8 (glibc 2.28), the same floor as DuckDB's own Linux binaries; an extension compiled on a newer Linux needs at least that system's glibc. The containers are of the host architecture; `ARCH=amd64` or `ARCH=arm64` overrides it. Run the `amd64` tests on an amd64 machine (such as a GitHub-hosted Ubuntu runner): under emulation on Apple Silicon, DuckDB's signature check fails even for official extensions installed online.
