//! The catalog the sidebar shows: attached databases, and the tables and views of one of them.
//!
//! DuckDB builds each answer as one JSON document in the shape of the protocol's type, which is read as that type.

use serde::de::DeserializeOwned;

use super::Engine;
use crate::duck::{Conn, Result, quote_identifier};
use crate::lock;
use crate::protocol::{Catalog, Relation};

const CATALOG: &str = "
    SELECT to_json({
        duckdb_version: version(),
        current_database: $1::VARCHAR,
        current_schema: $2::VARCHAR,
        default_database: current_database(),
        databases: (
            SELECT list({name: database_name, kind: type, path: path, read_only: readonly} ORDER BY database_name)
            FROM duckdb_databases()
            WHERE NOT internal
        ),
        memory: (
            SELECT {
                used: coalesce(sum(memory_usage_bytes), 0),
                spilled: coalesce(sum(temporary_storage_bytes), 0),
                \"limit\": parse_formatted_bytes(current_setting('memory_limit')),
            }
            FROM duckdb_memory()
        ),
    })::VARCHAR";

const RELATIONS: &str = "
    WITH relations AS (
        -- PostgreSQL and other attached databases report 0 when they have no estimate, which is no row count.
        SELECT schema_name, table_name AS name, false AS is_view, nullif(estimated_size, 0) AS estimated_rows
        FROM duckdb_tables()
        WHERE database_name = $1 AND NOT internal
        UNION ALL
        SELECT schema_name, view_name, true, NULL
        FROM duckdb_views()
        WHERE database_name = $1 AND NOT internal
    )
    SELECT coalesce(to_json(list({
        schema: schema_name,
        name: name,
        is_view: is_view,
        estimated_rows: estimated_rows,
        columns: (
            SELECT list({name: column_name, data_type: data_type} ORDER BY column_index)
            FROM duckdb_columns() AS c
            WHERE c.database_name = $1 AND c.schema_name = relations.schema_name AND c.table_name = relations.name
        ),
    } ORDER BY schema_name, name)), '[]')::VARCHAR
    FROM relations";

/// Run a statement that returns one JSON document, and read it as `T`.
fn document<T: DeserializeOwned>(conn: &Conn, sql: &str, parameters: &[&str]) -> Result<T> {
    let rows = conn.rows(sql, parameters)?;
    let json = rows.first().and_then(|row| row[0].as_ref()).ok_or("DuckDB returned no document")?;
    serde_json::from_str(json).map_err(|error| format!("unexpected catalog document: {error}"))
}

impl Engine {
    pub fn catalog(&self) -> Result<Catalog> {
        let (database, schema) = lock(&self.search_path).clone();
        document(&lock(&self.meta), CATALOG, &[&database, &schema])
    }

    /// The tables and views of one database, with their columns.
    pub fn relations(&self, database: &str) -> Result<Vec<Relation>> {
        document(&lock(&self.meta), RELATIONS, &[database])
    }

    /// Drop a table, whose rows are deleted, or a view, of which only the definition is.
    pub fn drop_relation(&self, database: &str, schema: &str, name: &str, is_view: bool) -> Result<()> {
        let path = [database, schema, name].map(quote_identifier).join(".");
        let kind = if is_view { "VIEW" } else { "TABLE" };
        lock(&self.meta).run(&format!("DROP {kind} {path}"), &[]).map(drop)
    }

    /// Detach a database: DuckDB forgets it, and its data stays where it is.
    pub fn detach(&self, database: &str) -> Result<()> {
        lock(&self.meta).run(&format!("DETACH {}", quote_identifier(database)), &[]).map(drop)
    }
}
