"""The bigquery extension, which reads BigQuery with Google's Application Default Credentials.

Without network access, only what the extension checks before calling BigQuery is tested. To test against BigQuery
too, sign in (`gcloud auth application-default login`) and set BIGQUERY_BILLING_PROJECT to a project that pays for
the reads of Google's public datasets, a few megabytes per run.
"""

import os
from pathlib import Path

import duckdb
import pytest

from tests.support import connect

BILLING_PROJECT = os.environ.get("BIGQUERY_BILLING_PROJECT", "")
SHAKESPEARE = "bigquery-public-data.samples.shakespeare"
NAMES = "bigquery-public-data.usa_names.usa_1910_current"

live = pytest.mark.skipif(not BILLING_PROJECT, reason="set BIGQUERY_BILLING_PROJECT to test against BigQuery")


@pytest.fixture
def con(custom: Path) -> duckdb.DuckDBPyConnection:
    con = connect([custom], check_signatures=False)
    con.execute("LOAD bigquery")
    return con


def rows(con: duckdb.DuckDBPyConnection, sql: str, *parameters: object) -> list[tuple[object, ...]]:
    return con.execute(sql, list(parameters)).fetchall()


def scan(table: str, **options: str) -> str:
    """A call of `bigquery_scan` that bills the reads to BIGQUERY_BILLING_PROJECT."""
    named = "".join(f", {name} := '{value}'" for name, value in options.items())
    return f"bigquery_scan('{table}', billing_project := '{BILLING_PROJECT}'{named})"


def test_functions(con: duckdb.DuckDBPyConnection) -> None:
    functions = "SELECT function_name, parameters FROM duckdb_functions() WHERE function_name LIKE 'bigquery%'"
    assert rows(con, f"{functions} ORDER BY ALL") == [
        ("bigquery_datasets", ["col0"]),
        ("bigquery_query", ["col0", "col1"]),
        ("bigquery_scan", ["col0", "filter", "billing_project"]),
        ("bigquery_tables", ["col0"]),
    ]


@pytest.mark.parametrize(
    ("sql", "message"),
    [
        ("FROM bigquery_scan('dataset.table')", "'dataset.table' is not 3 names separated by dots"),
        ("FROM bigquery_scan('project..table')", "'project..table' is not 3 names separated by dots"),
        ("FROM bigquery_tables('dataset')", "'dataset' is not 2 names separated by dots"),
    ],
)
def test_names_are_checked_before_bigquery_is_called(con: duckdb.DuckDBPyConnection, sql: str, message: str) -> None:
    with pytest.raises(duckdb.BinderException, match=message):
        con.execute(sql)


@live
def test_listings(con: duckdb.DuckDBPyConnection) -> None:
    assert ("samples", "US") in rows(con, "FROM bigquery_datasets('bigquery-public-data')")
    tables = rows(con, "SELECT \"table\", type FROM bigquery_tables('bigquery-public-data.samples')")
    assert ("shakespeare", "TABLE") in tables


@live
def test_scan(con: duckdb.DuckDBPyConnection) -> None:
    assert rows(con, f"SELECT count(*), sum(word_count) FROM {scan(SHAKESPEARE)}") == [(164656, 945845)]

    # The filter is applied by BigQuery, and columns come in the order asked for.
    hamlet = scan(SHAKESPEARE, filter='corpus = "hamlet"')
    assert rows(con, f"SELECT count(*) FROM {hamlet}") == [(5318,)]
    assert rows(con, f"SELECT corpus, word_count FROM {hamlet} WHERE word = 'Hamlet'") == [("hamlet", 77)]


@live
def test_scan_of_several_streams_by_one_thread(con: duckdb.DuckDBPyConnection) -> None:
    """A table written in insertion order reads every stream on one thread, which must not wait on a stream forever."""
    con.execute(f"CREATE TABLE names AS SELECT name, number FROM {scan(NAMES)}")
    assert rows(con, "SELECT count(*) FROM names") == rows(con, f"SELECT count(*) FROM {scan(NAMES)}")


@live
def test_query_keeps_bigquery_order(con: duckdb.DuckDBPyConnection) -> None:
    sql = f"SELECT word_count FROM `{SHAKESPEARE}` ORDER BY word_count DESC"
    counts = [count for (count,) in con.execute("FROM bigquery_query(?, ?)", [BILLING_PROJECT, sql]).fetchall()]
    assert len(counts) == 164656
    assert counts == sorted(counts, reverse=True)


@live
def test_query_runs_only_select(con: duckdb.DuckDBPyConnection) -> None:
    with pytest.raises(duckdb.BinderException, match="runs only SELECT statements, and this is SCRIPT"):
        con.execute("FROM bigquery_query(?, 'SELECT 1; SELECT 2')", [BILLING_PROJECT])


@live
def test_types(con: duckdb.DuckDBPyConnection) -> None:
    sql = """SELECT NUMERIC '1.23' AS n, BIGNUMERIC '1.2345678901234567890123456789012345678' AS bn,
        b'\\x01' AS bytes, DATETIME '2024-01-02 03:04:05' AS dt, TIMESTAMP '2024-01-02 03:04:05+00' AS ts,
        ST_GEOGPOINT(1, 2) AS g, JSON '{"a": 1}' AS j, [1, 2] AS a, STRUCT(1 AS x, 'y' AS y) AS s,
        INTERVAL 1 DAY AS i, RANGE(DATE '2024-01-01', DATE '2024-02-01') AS r"""
    con.execute("SET TimeZone = 'UTC'")
    con.execute("CREATE TABLE typed AS FROM bigquery_query(?, ?)", [BILLING_PROJECT, sql])

    assert rows(con, "SELECT column_name, column_type FROM (DESCRIBE typed)") == [
        ("n", "DECIMAL(38,9)"),
        ("bn", "VARCHAR"),  # 76 digits, where DuckDB's DECIMAL has 38
        ("bytes", "BLOB"),
        ("dt", "TIMESTAMP"),
        ("ts", "TIMESTAMP WITH TIME ZONE"),
        ("g", "VARCHAR"),
        ("j", "VARCHAR"),
        ("a", "BIGINT[]"),
        ("s", "STRUCT(x BIGINT, y VARCHAR)"),
        ("i", "INTERVAL"),
        ("r", 'STRUCT("start" DATE, "end" DATE)'),
    ]
    assert rows(con, "SELECT COLUMNS(*)::VARCHAR FROM typed") == [
        (
            "1.230000000",
            "1.23456789012345678901234567890123456780",
            "\\x01",
            "2024-01-02 03:04:05",
            "2024-01-02 03:04:05+00",
            "POINT(1 2)",
            '{"a":1}',
            "[1, 2]",
            "{'x': 1, 'y': y}",
            "1 day",
            "{'start': 2024-01-01, 'end': 2024-02-01}",
        )
    ]
