//! What the webui extension and its browser application say to each other.
//!
//! The application posts one [`Request`] as JSON and gets back the type named beside its variant.
//! Both sides compile this crate, the extension natively and the application to WebAssembly,
//! so a change to the protocol that one side misses is a compile error.

use serde::{Deserialize, Serialize};

pub mod sql;

/// Longest cell text a page carries.
/// The application shows it in the grid and fetches the whole value when a cell is opened.
pub const CELL_PREVIEW_CHARS: usize = 200;

/// Rows a cell's result holds right after it ran; more are loaded as the grid scrolls.
pub const INITIAL_ROWS: u64 = 1000;

/// The header that carries the access token on every request.
pub const TOKEN_HEADER: &str = "X-Webui-Token";

/// Rows of text as DuckDB prints them; `None` is NULL.
pub type Rows = Vec<Vec<Option<String>>>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// Answered with [`Catalog`].
    Catalog,
    /// The tables and views of one database: `Vec<Relation>`.
    Relations { database: String },

    /// Run the statements of a cell: [`RunResponse`].
    Run { sql: String },
    /// Cancel the running cell and whatever is computed in the background: nothing.
    Interrupt,
    /// How far the running cell is: [`Progress`].
    Progress,

    /// Load rows until the result holds `want` of them: [`ResultInfo`].
    ///
    /// When the statement is no longer open it is run again only with `resume`.
    /// Running it again skips the rows already loaded, which for a slow statement takes as long as it first did.
    Load { result: u64, want: u64, resume: bool },
    /// Rows `offset..offset + limit`, each value cut to a preview: [`Rows`].
    Page { result: u64, offset: u64, limit: u64 },
    /// One whole value, its text cut at `limit` characters: [`CellValue`].
    Value { result: u64, row: u64, column: usize, limit: u64 },
    /// The result's rows sorted and filtered by DuckDB, as another result: [`ResultInfo`].
    Reshape { result: u64, view: View },
    /// The number of rows, counted at the source of an unfinished result: `u64`.
    Count { result: u64 },
    /// What DuckDB's `SUMMARIZE` says about one column: [`Summary`].
    Summarize { result: u64, column: usize },
    /// Write every row to a file the browser then downloads: the link, a `String` that works once.
    Export { result: u64, format: ExportFormat },
    /// Stop holding a result: nothing.
    Forget { result: u64 },

    /// The saved notebook: [`Notebook`].
    Notebook,
    /// Save the notebook: nothing.
    SaveNotebook(Notebook),
}

/// Every failed request answers with this body.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ErrorBody {
    pub error: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    pub duckdb_version: String,
    pub databases: Vec<Database>,
    /// Where unqualified names resolve for the notebook's connection.
    pub current_database: String,
    pub current_schema: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Database {
    pub name: String,
    /// `duckdb`, `sqlite`, `postgres`, ...
    pub kind: String,
    pub path: Option<String>,
    pub read_only: bool,
}

/// A table or view of one database.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Relation {
    pub schema: String,
    pub name: String,
    pub is_view: bool,
    pub estimated_rows: Option<u64>,
    pub columns: Vec<Column>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub data_type: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RunResponse {
    pub outcome: Outcome,
    pub elapsed_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// The last statement returned rows.
    Rows(ResultInfo),
    /// The last statement changed rows and returned none.
    Changed {
        rows: u64,
    },
    /// The last statement returned nothing, or there was no statement.
    Done,
    Error {
        message: String,
    },
}

/// A result held by the server: the rows loaded so far sit in a cache that pages are read from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ResultInfo {
    pub id: u64,
    pub columns: Vec<Column>,
    /// Rows in the cache.
    pub loaded: u64,
    /// Whether `loaded` is every row.
    pub complete: bool,
    /// Whether the statement is still open, so that more rows come without running it again.
    pub streaming: bool,
    /// Whether DuckDB can run the statement as a subquery: to load rows after it was closed,
    /// and to sort, filter, count and summarise an unfinished result.
    pub repeatable: bool,
    /// The sort and filters this result was built with.
    pub view: View,
}

impl ResultInfo {
    pub fn can_load_more(&self) -> bool {
        !self.complete && (self.streaming || self.repeatable)
    }
}

/// A sort order and filters applied to a result by DuckDB, over all of its rows.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct View {
    pub sort: Vec<SortKey>,
    pub filters: Vec<Filter>,
}

impl View {
    pub fn is_empty(&self) -> bool {
        self.sort.is_empty() && self.filters.is_empty()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SortKey {
    pub column: usize,
    pub descending: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Filter {
    pub column: usize,
    pub op: FilterOp,
    /// Ignored by `IsNull` and `IsNotNull`.
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Contains,
    NotContains,
    StartsWith,
    Equals,
    NotEquals,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
    IsNull,
    IsNotNull,
}

impl FilterOp {
    /// Every operator with the words the application shows for it.
    pub const ALL: [(FilterOp, &str); 11] = [
        (FilterOp::Contains, "contains"),
        (FilterOp::NotContains, "does not contain"),
        (FilterOp::StartsWith, "starts with"),
        (FilterOp::Equals, "="),
        (FilterOp::NotEquals, "≠"),
        (FilterOp::Greater, ">"),
        (FilterOp::GreaterOrEqual, "≥"),
        (FilterOp::Less, "<"),
        (FilterOp::LessOrEqual, "≤"),
        (FilterOp::IsNull, "is null"),
        (FilterOp::IsNotNull, "is not null"),
    ];

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(op, _)| *op == self).map_or("", |(_, label)| label)
    }
}

/// One whole value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CellValue {
    /// The value as text, cut at the requested limit; `None` is NULL.
    pub text: Option<String>,
    /// Characters in the whole text.
    pub length: u64,
    /// The value as JSON, when it is a struct, list, map, union or JSON document.
    pub json: Option<String>,
}

/// One column of a result, summarised by DuckDB.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Summary {
    /// Whether every row was summarised.
    /// False for an unfinished result whose statement only the notebook's own connection can run.
    pub all_rows: bool,
    pub rows: u64,
    /// What `SUMMARIZE` says about the column, each fact with its label, as DuckDB prints it.
    pub facts: Vec<(String, String)>,
    /// The most frequent values and their counts; empty when the column has too many distinct values.
    pub top: Vec<(Option<String>, u64)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Progress {
    pub running: bool,
    pub percentage: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Csv,
    Parquet,
    Json,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Parquet => "parquet",
            ExportFormat::Json => "json",
        }
    }
}

/// The cells of the notebook, saved by the server so they survive a reload.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Notebook {
    pub cells: Vec<String>,
}
