"""The webui extension: a browser UI served from inside the DuckDB process, tested with no network access.

The browser application itself is JavaScript and is not run here.
These tests cover what it is served from and everything it asks the extension to do, with the same requests.
"""

import json
import re
import socket
import threading
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from dataclasses import dataclass
from email.message import Message
from pathlib import Path
from typing import Any

import duckdb
import pytest

from tests.support import CUSTOM_REPOSITORY, Bundle, ConfigValue, Connect, register_repository, scalar

TOKEN_HEADER = "X-Webui-Token"


@dataclass(frozen=True)
class Reply:
    status: int
    headers: Message
    body: bytes

    def json(self) -> Any:
        return json.loads(self.body)


@dataclass(frozen=True)
class Ui:
    con: duckdb.DuckDBPyConnection
    url: str
    token: str
    state: Path

    def request(self, method: str, path: str, body: object = None, *, token: str | None = None) -> Reply:
        """Call the server and return its reply, whatever its status; `token` replaces the right one."""
        headers = {TOKEN_HEADER: self.token if token is None else token}
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.url + path, data=data, method=method, headers=headers)

        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return Reply(response.status, response.headers, response.read())
        except urllib.error.HTTPError as error:
            return Reply(error.code, error.headers, error.read())

    def post(self, kind: str, **fields: object) -> Reply:
        """Send one request of the application's protocol: its kind, and its fields if it has any."""
        return self.request("POST", "/api", {kind: fields} if fields else kind)

    def ask(self, kind: str, **fields: object) -> Any:
        """Send one request and return its answer."""
        reply = self.post(kind, **fields)
        assert reply.status == 200, reply.body
        return reply.json()

    def run(self, sql: str) -> Any:
        """Run a cell and return its outcome."""
        return self.ask("run", sql=sql)["outcome"]

    def rows(self, result: int, offset: int = 0, limit: int = 100) -> list[list[str | None]]:
        return self.ask("page", result=result, offset=offset, limit=limit)


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def load_webui(con: duckdb.DuckDBPyConnection, bundle: Bundle) -> None:
    register_repository(con, bundle.custom_repo, bundle.public_key)
    con.execute(f"INSTALL webui FROM {CUSTOM_REPOSITORY}")
    con.execute(f"LOAD webui FROM {CUSTOM_REPOSITORY}")


@pytest.fixture
def ui(bundle: Bundle, connect: Connect, tmp_path: Path) -> Iterator[Ui]:
    """A database with the UI server running on a free port.

    One server runs per process, so the fixture stops it for the next test.
    """
    con = connect(allow_extension_repositories="allowed")
    load_webui(con, bundle)

    state = tmp_path / "state"
    started = scalar(con, f"SELECT url FROM webui_start(port := {free_port()}, state := '{state}')")
    url, token = str(started).split("/?token=")

    yield Ui(con=con, url=url, token=token, state=state)

    con.execute("CALL webui_stop()")


def test_bundle_contains_webui(bundle: Bundle) -> None:
    assert "webui" in bundle.custom


def test_start_reports_where_to_open_the_ui(ui: Ui) -> None:
    assert ui.con.execute("CALL webui_status()").fetchall() == [(f"{ui.url}/?token={ui.token}",)]

    # Starting again changes nothing.
    assert scalar(ui.con, "SELECT url FROM webui_start()") == f"{ui.url}/?token={ui.token}"


def application_files(ui: Ui) -> dict[str, Reply]:
    """The page and every file it loads: its style sheet, its icon, and the modules that `app.js` imports."""
    files = {name: ui.request("GET", f"/{name}", token="") for name in ["", "style.css", "favicon.svg"]}
    wanted = ["app.js"]
    while wanted:
        name = wanted.pop()
        files[name] = ui.request("GET", f"/{name}", token="")
        imported = re.findall(r'from "\./([a-z]+\.js)"', files[name].body.decode())
        wanted += [module for module in imported if module not in files]
    return files


def test_application_is_served_from_the_extension(ui: Ui) -> None:
    files = application_files(ui)
    assert len(files) > 10
    assert b'src="/app.js"' in files[""].body
    assert "default-src 'self'" in files[""].headers["Content-Security-Policy"]

    content_types = {"": "text/html", "style.css": "text/css", "favicon.svg": "image/svg+xml"}
    for name, file in files.items():
        expected = content_types.get(name, "text/javascript")
        assert (file.status, file.headers.get_content_type()) == (200, expected), name

    assert ui.request("GET", "/missing.js", token="").status == 404


def test_application_refers_to_no_other_host(ui: Ui) -> None:
    """Nothing the browser loads names another host; the only URL is an XML namespace, which is not fetched."""
    urls: set[str] = set()
    for file in application_files(ui).values():
        urls.update(re.findall(r"https?://[^\s\"'<>)`]+", file.body.decode()))

    assert urls <= {"http://www.w3.org/2000/svg"}


def test_requests_need_the_token(ui: Ui) -> None:
    for token in ["", "0" * len(ui.token)]:
        assert ui.request("POST", "/api", "catalog", token=token).status == 401
        assert ui.request("POST", "/api", {"run": {"sql": "SELECT 42"}}, token=token).status == 401

    assert ui.ask("catalog")["duckdb_version"] == scalar(ui.con, "SELECT version()")


def test_cells_run_in_the_database_of_the_python_connection(ui: Ui) -> None:
    ui.con.execute("CREATE TABLE ducks AS SELECT * FROM (VALUES ('Huey', 3), ('Dewey', 4)) t(name, age)")

    seen = ui.run("SELECT name FROM ducks ORDER BY age")
    assert ui.rows(seen["id"]) == [["Huey"], ["Dewey"]]

    assert ui.run("CREATE TABLE made_in_ui AS SELECT 42 AS answer")["kind"] in {"changed", "done"}
    assert scalar(ui.con, "SELECT answer FROM made_in_ui") == 42


def test_catalog_lists_databases_tables_and_columns(ui: Ui, tmp_path: Path) -> None:
    ui.con.execute("CREATE TABLE ducks (name VARCHAR, age INTEGER)")
    ui.con.execute("CREATE VIEW adults AS SELECT * FROM ducks WHERE age > 3")
    ui.con.execute(f"ATTACH '{tmp_path / 'other.duckdb'}' AS other")

    catalog = ui.ask("catalog")
    assert {database["name"] for database in catalog["databases"]} == {"memory", "other"}
    assert (catalog["current_database"], catalog["current_schema"]) == ("memory", "main")
    assert ui.ask("relations", database="other") == []

    relations = {relation["name"]: relation for relation in ui.ask("relations", database="memory")}
    assert {name: relation["is_view"] for name, relation in relations.items()} == {"ducks": False, "adults": True}
    assert relations["ducks"]["columns"] == [
        {"name": "name", "data_type": "VARCHAR"},
        {"name": "age", "data_type": "INTEGER"},
    ]

    # The catalog follows the notebook's connection.
    ui.run("USE other")
    assert ui.ask("catalog")["current_database"] == "other"


def test_outcome_of_each_kind_of_statement(ui: Ui) -> None:
    assert ui.run("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1), (2), (3)") == {"kind": "changed", "rows": 3}
    assert ui.run("SET threads = 2") == {"kind": "done"}
    assert ui.run("") == {"kind": "done"}

    # Only the last statement's rows are held.
    last = ui.run("SELECT 'first'; SELECT x FROM t ORDER BY x")
    assert ui.rows(last["id"]) == [["1"], ["2"], ["3"]]

    error = ui.run("SELEC 1")
    assert error["kind"] == "error"
    assert "syntax error" in error["message"]

    # Statements that cannot be a subquery return rows too.
    for statement in [
        "PRAGMA database_list",
        "EXPLAIN SELECT 1",
        "CALL duckdb_settings()",
        "INSERT INTO t VALUES (4) RETURNING x",
    ]:
        assert ui.run(statement)["kind"] == "rows", statement


def test_session_state_carries_from_cell_to_cell(ui: Ui) -> None:
    ui.run("CREATE TEMP TABLE scratch AS SELECT 7 AS x; SET VARIABLE factor = 6")
    answer = ui.run("SELECT x * getvariable('factor') FROM scratch")
    assert ui.rows(answer["id"]) == [["42"]]


def test_rows_are_loaded_on_demand(ui: Ui) -> None:
    result = ui.run("SELECT i, i * 2 AS twice FROM range(100000) t(i)")
    assert result["columns"] == [{"name": "i", "data_type": "BIGINT"}, {"name": "twice", "data_type": "BIGINT"}]
    assert 0 < result["loaded"] < 100000
    assert (result["complete"], result["streaming"], result["repeatable"]) == (False, True, True)

    # Counting loads nothing.
    assert ui.ask("count", result=result["id"]) == 100000
    assert ui.ask("load", result=result["id"], want=0, resume=False)["loaded"] == result["loaded"]

    loaded = ui.ask("load", result=result["id"], want=50000, resume=False)
    assert 50000 <= loaded["loaded"] < 100000
    assert ui.rows(result["id"], offset=49999, limit=2) == [["49999", "99998"], ["50000", "100000"]]

    everything = ui.ask("load", result=result["id"], want=10**9, resume=False)
    assert (everything["loaded"], everything["complete"], everything["streaming"]) == (100000, True, False)


def test_statement_is_run_again_only_when_asked_once_another_cell_took_the_session(ui: Ui) -> None:
    result = ui.run("SELECT i FROM range(100000) t(i)")
    ui.run("SELECT 'another cell'")

    waiting = ui.ask("load", result=result["id"], want=20000, resume=False)
    assert (waiting["loaded"], waiting["streaming"], waiting["repeatable"]) == (result["loaded"], False, True)

    resumed = ui.ask("load", result=result["id"], want=20000, resume=True)
    assert resumed["loaded"] >= 20000
    assert ui.rows(result["id"], offset=19999, limit=1) == [["19999"]]


def test_sort_and_filter_cover_rows_that_are_not_loaded(ui: Ui) -> None:
    result = ui.run("SELECT i, i % 10 AS digit, 'row ' || i AS label FROM range(100000) t(i)")
    assert not result["complete"]

    view = {
        "sort": [{"column": 0, "descending": True}],
        "filters": [{"column": 1, "op": "equals", "value": "7"}, {"column": 2, "op": "contains", "value": "ROW 9"}],
    }
    shaped = ui.ask("reshape", result=result["id"], view=view)
    assert shaped["view"] == view
    assert shaped["columns"] == result["columns"]

    assert ui.rows(shaped["id"], limit=3) == [
        ["99997", "7", "row 99997"],
        ["99987", "7", "row 99987"],
        ["99977", "7", "row 99977"],
    ]
    assert ui.ask("count", result=shaped["id"]) == 1111


def test_sort_and_filter_of_a_finished_result_use_its_cache(ui: Ui) -> None:
    result = ui.run("SELECT i, CASE WHEN i % 3 = 0 THEN NULL ELSE 'v' || (i % 4) END AS label FROM range(12) t(i)")
    assert result["complete"]

    sort = [{"column": 1, "descending": True}, {"column": 0, "descending": True}]
    shaped = ui.ask("reshape", result=result["id"], view={"sort": sort, "filters": []})
    assert (shaped["complete"], shaped["loaded"]) == (True, 12)
    assert ui.rows(shaped["id"], limit=3) == [["11", "v3"], ["7", "v3"], ["10", "v2"]]
    assert ui.rows(shaped["id"], offset=8) == [["9", None], ["6", None], ["3", None], ["0", None]]

    is_null = {"column": 1, "op": "is_null", "value": ""}
    nulls = ui.ask("reshape", result=result["id"], view={"sort": [], "filters": [is_null]})
    assert ui.rows(nulls["id"]) == [["0", None], ["3", None], ["6", None], ["9", None]]

    not_a_number = {"column": 0, "op": "greater", "value": "abc"}
    wrong = ui.post("reshape", result=result["id"], view={"sort": [], "filters": [not_a_number]})
    assert wrong.status == 400
    assert wrong.json() == {"error": "Conversion Error: Could not convert string 'abc' to INT64"}


def test_filters_compare_values_as_the_type_of_their_column(ui: Ui) -> None:
    result = ui.run(
        "SELECT i, DATE '2024-01-01' + i::INTEGER AS day, {'n': i} AS record, [i, i] AS pair FROM range(20) t(i)"
    )

    def matching(column: int, op: str, value: str) -> list[str | None]:
        kept = {"column": column, "op": op, "value": value}
        shaped = ui.ask("reshape", result=result["id"], view={"sort": [], "filters": [kept]})
        return [row[0] for row in ui.rows(shaped["id"])]

    assert matching(0, "greater_or_equal", "9.5") == [str(i) for i in range(10, 20)]
    assert matching(1, "less", "2024-01-03") == ["0", "1"]
    assert matching(2, "equals", "{'n': 7}") == ["7"]
    assert matching(3, "equals", "[5, 5]") == ["5"]
    assert matching(3, "not_equals", "[5, 5]") == [str(i) for i in range(20) if i != 5]
    assert matching(2, "contains", "'N': 1") == ["1", *(str(i) for i in range(10, 20))]


def test_values_of_every_type_can_be_shown(ui: Ui) -> None:
    result = ui.run("SELECT * FROM test_all_types()")
    assert result["complete"]
    assert len(ui.rows(result["id"])) == 3

    names = [column["name"] for column in result["columns"]]
    values = [ui.ask("value", result=result["id"], row=1, column=index, limit=100000) for index in range(len(names))]
    assert all(value["text"] is not None for value in values)

    # Values with parts come as JSON too, and only those.
    with_parts = {name for name, value in zip(names, values, strict=True) if value["json"] is not None}
    assert {"struct", "int_array", "map", "union", "fixed_struct_array", "array_of_structs"} <= with_parts
    assert not {"int", "varchar", "date", "blob", "uuid", "small_enum", "dec38_10"} & with_parts
    for value in values:
        json.loads(value["json"] or "null")

    types = {column["name"]: column["data_type"] for column in result["columns"]}
    assert types["dec38_10"] == "DECIMAL(38,10)"
    assert types["struct"] == "STRUCT(a INTEGER, b VARCHAR)"
    assert types["timestamp_tz"] == "TIMESTAMP WITH TIME ZONE"
    assert types["small_enum"] == "ENUM('DUCK_DUCK_ENUM', 'GOOSE')"

    assert values[names.index("hugeint")]["text"] == "170141183460469231731687303715884105727"
    assert json.loads(values[names.index("union")]["json"]) == {"age": 5}


def test_columns_with_the_same_name_stay_apart(ui: Ui) -> None:
    result = ui.run("SELECT 1 AS a, 'two' AS a, 3.5 AS a, NULL AS a")
    assert [column["name"] for column in result["columns"]] == ["a", "a", "a", "a"]
    assert ui.rows(result["id"]) == [["1", "two", "3.5", None]]


def test_long_values_are_previewed_in_pages_and_whole_on_request(ui: Ui) -> None:
    result = ui.run("SELECT repeat('duck ', 200000) AS text, [1, 2, 3] AS numbers, NULL AS nothing")

    (row,) = ui.rows(result["id"])
    assert row[0] is not None
    assert len(row[0]) == 200
    assert row[1:] == ["[1, 2, 3]", None]

    first = ui.ask("value", result=result["id"], row=0, column=0, limit=1000)
    assert (len(first["text"]), first["length"], first["json"]) == (1000, 1000000, None)
    whole = ui.ask("value", result=result["id"], row=0, column=0, limit=16000000)
    assert whole["text"] == "duck " * 200000

    assert ui.ask("value", result=result["id"], row=0, column=1, limit=100)["json"] == "[1,2,3]"
    assert ui.ask("value", result=result["id"], row=0, column=2, limit=100) == {"text": None, "length": 0, "json": None}


def test_summary_covers_every_row(ui: Ui) -> None:
    result = ui.run("SELECT i, CASE WHEN i % 2 = 0 THEN NULL ELSE 'k' || (i % 3) END AS key FROM range(100000) t(i)")
    assert not result["complete"]

    numbers = ui.ask("summarize", result=result["id"], column=0)
    assert (numbers["all_rows"], numbers["rows"]) == (True, 100000)
    facts = dict(numbers["facts"])
    assert (facts["Type"], facts["Nulls"], facts["Min"], facts["Max"], facts["Mean"]) == (
        "BIGINT",
        "0.00%",
        "0",
        "99999",
        "49999.5",
    )
    # Every value occurs once, so none is listed as frequent.
    assert numbers["top"] == [] or all(count == 1 for _, count in numbers["top"])

    keys = ui.ask("summarize", result=result["id"], column=1)
    facts = dict(keys["facts"])
    assert (facts["Nulls"], facts["Distinct"]) == ("50.00%", "≈ 3")
    assert "Mean" not in facts
    assert keys["top"][0] == [None, 50000]
    assert {value for value, _ in keys["top"]} == {None, "k0", "k1", "k2"}


def test_temporary_table_is_counted_and_summarised_as_far_as_it_is_loaded(ui: Ui) -> None:
    """Counts and summaries run on another connection, which does not see the notebook's temporary tables."""
    result = ui.run("CREATE TEMP TABLE notes AS SELECT i FROM range(100000) t(i); SELECT * FROM notes")
    assert not result["complete"]

    count = ui.post("count", result=result["id"])
    assert count.json() == {
        "error": 'Catalog Error: Table with name notes does not exist!\nDid you mean "pg_namespace"?'
    }

    summary = ui.ask("summarize", result=result["id"], column=0)
    assert (summary["all_rows"], summary["rows"]) == (False, result["loaded"])

    # Sorting runs on the notebook's connection, and covers every row.
    shaped = ui.ask("reshape", result=result["id"], view={"sort": [{"column": 0, "descending": True}], "filters": []})
    assert ui.rows(shaped["id"], limit=1) == [["99999"]]


def test_export_writes_every_row_and_its_link_works_once(ui: Ui, tmp_path: Path) -> None:
    result = ui.run("SELECT i AS id, 'row ' || i AS id FROM range(100000) t(i)")
    assert not result["complete"]

    link = ui.ask("export", result=result["id"], format="parquet")
    download = ui.request("GET", link, token="")
    assert download.status == 200
    assert download.headers.get_filename() == "result.parquet"

    exported = tmp_path / "result.parquet"
    exported.write_bytes(download.body)
    # The second column got a suffix, since a Parquet file cannot have two columns of one name.
    assert duckdb.execute(f"SELECT count(*), max(id), max(id_2) FROM '{exported}'").fetchall() == [
        (100000, 99999, "row 99999")
    ]

    assert ui.request("GET", link, token="").status == 404

    csv = ui.request("GET", ui.ask("export", result=result["id"], format="csv"), token="")
    assert csv.body.startswith(b"id,id_2\n0,row 0\n")


def test_running_cell_reports_progress_and_can_be_interrupted(ui: Ui) -> None:
    seen: list[Any] = []

    def interrupt() -> None:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and not ui.ask("progress")["running"]:
            time.sleep(0.05)
        seen.append(ui.ask("progress"))
        ui.ask("interrupt")

    thread = threading.Thread(target=interrupt)
    thread.start()
    outcome = ui.run("SELECT count(*) FROM range(100000000000) t(i) WHERE i % 7 = 3")
    thread.join()

    assert seen[0]["running"]
    assert outcome["kind"] == "error"
    assert "Interrupted" in outcome["message"]

    assert ui.ask("progress") == {"running": False, "percentage": None}
    assert ui.rows(ui.run("SELECT 'still works'")["id"]) == [["still works"]]


def test_interrupt_leaves_an_open_result_alone(ui: Ui) -> None:
    result = ui.run("SELECT i FROM range(100000) t(i)")
    ui.ask("interrupt")

    loaded = ui.ask("load", result=result["id"], want=30000, resume=False)
    assert (loaded["loaded"] >= 30000, loaded["streaming"]) == (True, True)


def test_notebook_is_saved_in_the_state_directory(ui: Ui) -> None:
    assert ui.ask("notebook") == {"cells": []}

    notebook = {"cells": ["SELECT 1", "FROM ducks -- 오리"]}
    assert ui.ask("save_notebook", **notebook) is None

    assert ui.ask("notebook") == notebook
    assert json.loads((ui.state / "notebook.json").read_text()) == notebook


def test_forgotten_results_are_gone(ui: Ui) -> None:
    result = ui.run("SELECT 1")
    assert ui.ask("forget", result=result["id"]) is None

    gone = ui.post("page", result=result["id"], offset=0, limit=1)
    assert gone.status == 400
    assert "no longer held" in gone.json()["error"]


def test_nothing_is_attached_to_the_users_database(ui: Ui) -> None:
    """Results are cached in temporary tables of the extension's own connection, which no one else sees."""
    before = ui.con.execute("SELECT database_name FROM duckdb_databases() ORDER BY 1").fetchall()

    result = ui.run("SELECT * FROM range(5000)")
    ui.ask("reshape", result=result["id"], view={"sort": [{"column": 0, "descending": True}], "filters": []})

    assert ui.con.execute("SELECT database_name FROM duckdb_databases() ORDER BY 1").fetchall() == before
    assert ui.con.execute("SELECT table_name FROM duckdb_tables()").fetchall() == []


def test_stops_and_starts_again(ui: Ui) -> None:
    assert ui.con.execute("CALL webui_stop()").fetchall() == [(f"{ui.url}/?token={ui.token}",)]
    assert ui.con.execute("CALL webui_status()").fetchall() == [(None,)]
    with pytest.raises(urllib.error.URLError):
        ui.post("catalog")

    port = free_port()
    chosen = f"http://127.0.0.1:{port}/?token=chosen"
    assert ui.con.execute(f"CALL webui_start(port := {port}, token := 'chosen')").fetchall() == [(chosen,)]
    assert ui.con.execute("CALL webui_stop()").fetchall() == [(chosen,)]
    assert ui.con.execute("CALL webui_stop()").fetchall() == [(None,)]

    with pytest.raises(duckdb.Error, match="port must be a number"):
        ui.con.execute("CALL webui_start(port := 70000)")


def test_one_database_is_served_per_process(ui: Ui, bundle: Bundle, connect: Connect) -> None:
    other = connect(allow_extension_repositories="allowed")
    other.execute(f"LOAD webui FROM {CUSTOM_REPOSITORY}")

    with pytest.raises(duckdb.Error, match="serving another database"):
        other.execute(f"CALL webui_start(port := {free_port()})")


def test_database_is_released_when_its_last_connection_closes(bundle: Bundle, tmp_path: Path) -> None:
    """The extension's own connections must not keep a database open, or opening its file again would wait forever."""
    settings: dict[str, ConfigValue] = {
        "extension_directory": str(tmp_path / "extensions"),
        "extension_repository_directory": str(tmp_path / "extension_repositories"),
        "allow_extension_repositories": "allowed",
    }
    path = str(tmp_path / "file.duckdb")

    con = duckdb.connect(path, config=settings)
    con.execute("CREATE TABLE kept AS SELECT 42 AS answer")
    load_webui(con, bundle)
    url = scalar(con, f"SELECT url FROM webui_start(port := {free_port()})")
    con.close()

    answers: list[object] = []
    reopen = threading.Thread(
        target=lambda: answers.append(scalar(duckdb.connect(path, config=settings), "SELECT answer FROM kept")),
        daemon=True,
    )
    reopen.start()
    reopen.join(timeout=30)
    assert answers == [42]

    # The server stopped with the database it was serving.
    with pytest.raises(urllib.error.URLError):
        urllib.request.urlopen(str(url), timeout=5)
