//! `bigquery`: read BigQuery from DuckDB, through the Google Cloud Rust SDK.
//!
//! ```sql
//! FROM bigquery_datasets('my-project');                    -- dataset, location
//! FROM bigquery_tables('my-project.my_dataset');           -- table, type (TABLE, VIEW, ...), created
//! FROM bigquery_scan('my-project.my_dataset.my_table');    -- a table's rows, through the Storage Read API
//! FROM bigquery_query('my-project', 'SELECT * FROM my_dataset.my_view WHERE ...');  -- GoogleSQL, run by BigQuery
//! ```
//!
//! | File          | What it is                                                                               |
//! |---------------|------------------------------------------------------------------------------------------|
//! | `google.rs`   | Google's side: credentials, clients, and each call to BigQuery.                           |
//! | `functions.rs`| DuckDB's side: the table functions, which hand BigQuery's Arrow record batches to DuckDB. |
//!
//! Google's Application Default Credentials sign every call: the extension holds no secret of its own.
//! It only reads: `bigquery_query` runs a statement only once a dry run has shown that it is a SELECT.

mod functions;
mod google;

use std::error::Error;

use duckdb::{Connection, duckdb_entrypoint_c_api};

use functions::{List, Listing, Query, Scan};

/// Entry point DuckDB calls on `LOAD bigquery` (exported as `bigquery_init_c_api`).
///
/// `min_duckdb_version` is the stable C API version asked for; `capi-version` in `Cargo.toml` repeats it.
#[duckdb_entrypoint_c_api(ext_name = "bigquery", min_duckdb_version = "v1.5.6")]
pub unsafe fn entrypoint(connection: Connection) -> Result<(), Box<dyn Error>> {
    let listings: [(&str, List); 2] = [("bigquery_datasets", google::datasets), ("bigquery_tables", google::tables)];
    for (name, list) in &listings {
        connection.register_table_function_with_extra_info::<Listing, _>(name, list)?;
    }
    connection.register_table_function::<Scan>("bigquery_scan")?;
    connection.register_table_function::<Query>("bigquery_query")?;
    Ok(())
}
