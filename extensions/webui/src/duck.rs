//! Safe wrappers over the parts of DuckDB's stable C API (v1) that webui uses.
//!
//! DuckDB does the formatting and the typing.
//! Every value read back here was cast to VARCHAR in SQL, so [`Chunk::texts`] is the only vector reader,
//! and a type is only ever handed from one DuckDB call to another, never decoded or spelled in Rust.

use std::ffi::{CStr, CString, c_char};
use std::ptr;

use libduckdb_sys as ffi;
use webui_protocol::Rows;

pub type Result<T> = std::result::Result<T, String>;

const SUCCESS: ffi::duckdb_state = ffi::duckdb_state_DuckDBSuccess;

fn c_string(text: &str) -> Result<CString> {
    CString::new(text).map_err(|_| "text contains a NUL byte".to_string())
}

/// Copy a C string that DuckDB owns.
unsafe fn copy_text(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

/// Render a name as a double-quoted SQL identifier.
pub fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Render text as a single-quoted SQL string literal.
pub fn quote_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A connection to the database that loaded the extension.
///
/// One thread uses a connection at a time; [`ConnHandle`] covers the two calls that are safe from other threads.
pub struct Conn {
    raw: ffi::duckdb_connection,
}

// SAFETY: a connection may move between threads; callers serialize its use with a mutex.
unsafe impl Send for Conn {}

impl Conn {
    /// # Safety
    /// `database` must be the handle DuckDB lends to the extension entrypoint.
    pub unsafe fn open(database: ffi::duckdb_database) -> Result<Conn> {
        let mut raw = ptr::null_mut();
        if unsafe { ffi::duckdb_connect(database, &mut raw) } != SUCCESS {
            return Err("could not open a connection".to_string());
        }
        Ok(Conn { raw })
    }

    pub fn raw(&self) -> ffi::duckdb_connection {
        self.raw
    }

    pub fn handle(&self) -> ConnHandle {
        ConnHandle(self.raw)
    }

    /// Split SQL text into its statements without running them.
    pub fn statements(&self, sql: &str) -> Result<Statements> {
        let text = c_string(sql)?;
        let mut raw = ptr::null_mut();
        let count = unsafe { ffi::duckdb_extract_statements(self.raw, text.as_ptr(), &mut raw) } as usize;

        let statements = Statements { conn: self.raw, raw, count };
        let error = unsafe { copy_text(ffi::duckdb_extract_statements_error(raw)) };
        match count == 0 && !error.is_empty() {
            true => Err(error),
            false => Ok(statements),
        }
    }

    pub fn prepare(&self, sql: &str) -> Result<Prepared> {
        let text = c_string(sql)?;
        let mut raw = ptr::null_mut();
        let state = unsafe { ffi::duckdb_prepare(self.raw, text.as_ptr(), &mut raw) };
        Prepared::checked(raw, state)
    }

    /// Run one statement to completion, binding `parameters` to `$1`, `$2`, ... as text.
    pub fn run(&self, sql: &str, parameters: &[&str]) -> Result<QueryResult> {
        let prepared = self.prepare(sql)?;
        for (index, value) in parameters.iter().enumerate() {
            prepared.bind_text(index + 1, value)?;
        }
        prepared.execute()
    }

    /// Run one statement whose result columns are all VARCHAR, and return every row.
    pub fn rows(&self, sql: &str, parameters: &[&str]) -> Result<Rows> {
        let mut result = self.run(sql, parameters)?;
        let mut rows = Vec::new();

        while let Some(chunk) = result.fetch()? {
            let columns = (0..chunk.column_count()).map(|column| chunk.texts(column)).collect::<Result<Vec<_>>>()?;
            rows.extend((0..chunk.len()).map(|row| columns.iter().map(|column| column[row].clone()).collect()));
        }
        Ok(rows)
    }

    /// Create an empty temporary table whose columns c0, c1, ... have the types of the columns of `result`.
    ///
    /// Each type is bound as an empty list of that type, and `unnest` of the list is a column of the type with no rows.
    pub fn create_table_like(&self, table: &str, result: &mut QueryResult) -> Result<()> {
        let count = result.column_count();
        let columns: Vec<_> = (0..count).map(|index| format!("unnest(${}) AS c{index}", index + 1)).collect();
        let create = self.prepare(&format!("CREATE TEMP TABLE {table} AS SELECT {}", columns.join(", ")))?;

        for index in 0..count as u64 {
            let bound = unsafe {
                let mut column_type = ffi::duckdb_column_logical_type(&mut result.raw, index);
                // DuckDB makes no list from a null pointer, even an empty one.
                let mut no_values: [ffi::duckdb_value; 0] = [];
                let mut empty_list = ffi::duckdb_create_list_value(column_type, no_values.as_mut_ptr(), 0);
                let bound =
                    !empty_list.is_null() && ffi::duckdb_bind_value(create.raw, index + 1, empty_list) == SUCCESS;

                ffi::duckdb_destroy_value(&mut empty_list);
                ffi::duckdb_destroy_logical_type(&mut column_type);
                bound
            };
            if !bound {
                return Err(format!("could not bind the type of column {index}"));
            }
        }
        create.execute().map(drop)
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        unsafe { ffi::duckdb_disconnect(&mut self.raw) };
    }
}

/// The calls on a connection that DuckDB allows from a thread other than the one running a query.
///
/// It must not outlive the connection it was taken from.
#[derive(Clone, Copy)]
pub struct ConnHandle(ffi::duckdb_connection);

// SAFETY: interrupt is an atomic store and progress reads atomics.
unsafe impl Send for ConnHandle {}
unsafe impl Sync for ConnHandle {}

impl ConnHandle {
    pub fn interrupt(&self) {
        unsafe { ffi::duckdb_interrupt(self.0) };
    }

    /// Percentage done of the running query, or `None` when DuckDB has no estimate.
    pub fn progress(&self) -> Option<f64> {
        let progress = unsafe { ffi::duckdb_query_progress(self.0) };
        (progress.percentage >= 0.0).then_some(progress.percentage)
    }
}

/// The statements of one SQL text, prepared one at a time so that each sees the effects of those before it.
pub struct Statements {
    conn: ffi::duckdb_connection,
    raw: ffi::duckdb_extracted_statements,
    count: usize,
}

impl Statements {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn prepare(&self, index: usize) -> Result<Prepared> {
        let mut raw = ptr::null_mut();
        let state = unsafe { ffi::duckdb_prepare_extracted_statement(self.conn, self.raw, index as u64, &mut raw) };
        Prepared::checked(raw, state)
    }
}

impl Drop for Statements {
    fn drop(&mut self) {
        unsafe { ffi::duckdb_destroy_extracted(&mut self.raw) };
    }
}

pub struct Prepared {
    raw: ffi::duckdb_prepared_statement,
}

impl Prepared {
    fn checked(raw: ffi::duckdb_prepared_statement, state: ffi::duckdb_state) -> Result<Prepared> {
        let prepared = Prepared { raw };
        match state == SUCCESS {
            true => Ok(prepared),
            false => Err(unsafe { copy_text(ffi::duckdb_prepare_error(raw)) }),
        }
    }

    pub fn is_select(&self) -> bool {
        unsafe {
            ffi::duckdb_prepared_statement_type(self.raw) == ffi::duckdb_statement_type_DUCKDB_STATEMENT_TYPE_SELECT
        }
    }

    /// Columns of the result the statement will return.
    pub fn column_count(&self) -> usize {
        unsafe { ffi::duckdb_prepared_statement_column_count(self.raw) as usize }
    }

    /// Bind text to parameter `index` (1-based); DuckDB casts it where the statement needs another type.
    pub fn bind_text(&self, index: usize, value: &str) -> Result<()> {
        let text = value.as_ptr() as *const c_char;
        match unsafe { ffi::duckdb_bind_varchar_length(self.raw, index as u64, text, value.len() as u64) } {
            SUCCESS => Ok(()),
            _ => Err(format!("could not bind parameter {index}")),
        }
    }

    /// Run to completion; the result holds every row.
    pub fn execute(&self) -> Result<QueryResult> {
        let mut result = QueryResult::empty();
        let state = unsafe { ffi::duckdb_execute_prepared(self.raw, &mut result.raw) };
        result.checked(state)
    }

    /// Start the statement and return once its first rows are ready; later rows are computed as they are fetched.
    ///
    /// The connection cannot run anything else until the result is dropped.
    pub fn stream(&self) -> Result<QueryResult> {
        let mut pending = ptr::null_mut();
        if unsafe { ffi::duckdb_pending_prepared_streaming(self.raw, &mut pending) } != SUCCESS {
            let error = unsafe { copy_text(ffi::duckdb_pending_error(pending)) };
            unsafe { ffi::duckdb_destroy_pending(&mut pending) };
            return Err(error);
        }

        let mut result = QueryResult::empty();
        let state = unsafe { ffi::duckdb_execute_pending(pending, &mut result.raw) };
        unsafe { ffi::duckdb_destroy_pending(&mut pending) };
        result.checked(state)
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        unsafe { ffi::duckdb_destroy_prepare(&mut self.raw) };
    }
}

/// What a statement gave back.
pub enum Returned {
    Rows,
    /// The number of rows it changed.
    Changed(u64),
    Nothing,
}

pub struct QueryResult {
    raw: ffi::duckdb_result,
}

// SAFETY: a result belongs to its connection and is used under that connection's mutex.
unsafe impl Send for QueryResult {}

impl QueryResult {
    fn empty() -> QueryResult {
        // SAFETY: duckdb_result is a plain C struct that DuckDB fills in; all-zero is its empty state.
        QueryResult { raw: unsafe { std::mem::zeroed() } }
    }

    fn checked(mut self, state: ffi::duckdb_state) -> Result<QueryResult> {
        match state == SUCCESS {
            true => Ok(self),
            false => Err(self.error()),
        }
    }

    fn error(&mut self) -> String {
        unsafe { copy_text(ffi::duckdb_result_error(&mut self.raw)) }
    }

    pub fn returned(&mut self) -> Returned {
        match unsafe { ffi::duckdb_result_return_type(self.raw) } {
            ffi::duckdb_result_type_DUCKDB_RESULT_TYPE_QUERY_RESULT => Returned::Rows,
            ffi::duckdb_result_type_DUCKDB_RESULT_TYPE_CHANGED_ROWS => {
                Returned::Changed(unsafe { ffi::duckdb_rows_changed(&mut self.raw) })
            }
            _ => Returned::Nothing,
        }
    }

    pub fn column_count(&mut self) -> usize {
        unsafe { ffi::duckdb_column_count(&mut self.raw) as usize }
    }

    pub fn column_name(&mut self, column: usize) -> String {
        unsafe { copy_text(ffi::duckdb_column_name(&mut self.raw, column as u64)) }
    }

    /// The next chunk of rows, or `None` after the last one.
    pub fn fetch(&mut self) -> Result<Option<Chunk>> {
        let raw = unsafe { ffi::duckdb_fetch_chunk(self.raw) };
        if !raw.is_null() {
            return Ok(Some(Chunk { raw }));
        }

        // The end of the rows and a failure both come back as no chunk; only a failure leaves a message.
        let error = self.error();
        match error.is_empty() {
            true => Ok(None),
            false => Err(error),
        }
    }
}

impl Drop for QueryResult {
    fn drop(&mut self) {
        unsafe { ffi::duckdb_destroy_result(&mut self.raw) };
    }
}

/// Up to 2048 rows of a result, column by column.
pub struct Chunk {
    raw: ffi::duckdb_data_chunk,
}

// SAFETY: a chunk owns its data.
unsafe impl Send for Chunk {}

impl Chunk {
    pub fn len(&self) -> usize {
        unsafe { ffi::duckdb_data_chunk_get_size(self.raw) as usize }
    }

    pub fn column_count(&self) -> usize {
        unsafe { ffi::duckdb_data_chunk_get_column_count(self.raw) as usize }
    }

    /// Read a VARCHAR column; `None` is NULL.
    pub fn texts(&self, column: usize) -> Result<Vec<Option<String>>> {
        let vector = unsafe { ffi::duckdb_data_chunk_get_vector(self.raw, column as u64) };

        let mut logical_type = unsafe { ffi::duckdb_vector_get_column_type(vector) };
        let type_id = unsafe { ffi::duckdb_get_type_id(logical_type) };
        unsafe { ffi::duckdb_destroy_logical_type(&mut logical_type) };
        if type_id != ffi::DUCKDB_TYPE_DUCKDB_TYPE_VARCHAR {
            return Err(format!("column {column} is not VARCHAR"));
        }

        let strings = unsafe { ffi::duckdb_vector_get_data(vector) } as *mut ffi::duckdb_string_t;
        let validity = unsafe { ffi::duckdb_vector_get_validity(vector) };
        let text = |row: usize| unsafe {
            let string = strings.add(row);
            let length = ffi::duckdb_string_t_length(*string) as usize;
            let bytes = std::slice::from_raw_parts(ffi::duckdb_string_t_data(string) as *const u8, length);
            String::from_utf8_lossy(bytes).into_owned()
        };

        let is_valid =
            |row: usize| validity.is_null() || unsafe { ffi::duckdb_validity_row_is_valid(validity, row as u64) };
        Ok((0..self.len()).map(|row| is_valid(row).then(|| text(row))).collect())
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        unsafe { ffi::duckdb_destroy_data_chunk(&mut self.raw) };
    }
}

/// Appends chunks to a table without converting them, whatever their column types.
pub struct Appender {
    raw: ffi::duckdb_appender,
}

impl Appender {
    pub fn new(conn: &Conn, table: &str) -> Result<Appender> {
        let name = c_string(table)?;
        let mut raw = ptr::null_mut();
        let state = unsafe { ffi::duckdb_appender_create(conn.raw, ptr::null(), name.as_ptr(), &mut raw) };
        Appender { raw }.checked(state)
    }

    fn checked(self, state: ffi::duckdb_state) -> Result<Appender> {
        match state == SUCCESS {
            true => Ok(self),
            false => Err(unsafe { copy_text(ffi::duckdb_appender_error(self.raw)) }),
        }
    }

    pub fn append(self, chunk: &Chunk) -> Result<Appender> {
        let state = unsafe { ffi::duckdb_append_data_chunk(self.raw, chunk.raw) };
        self.checked(state)
    }

    /// Write the appended rows to the table.
    pub fn finish(self) -> Result<()> {
        let state = unsafe { ffi::duckdb_appender_close(self.raw) };
        self.checked(state).map(drop)
    }
}

impl Drop for Appender {
    fn drop(&mut self) {
        unsafe { ffi::duckdb_appender_destroy(&mut self.raw) };
    }
}
