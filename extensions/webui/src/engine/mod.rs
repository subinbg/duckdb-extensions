//! Runs SQL for the browser application and holds the results.
//!
//! Four connections, each opened when the extension loads and used by one thread at a time:
//!
//! - `session` runs the notebook's statements, so `SET`, `USE`, temporary tables and transactions carry from one
//!   cell to the next. A cell's last statement is streamed: DuckDB computes rows only as they are asked for.
//! - `cache` owns one temporary table per result. Streamed rows are appended to it unchanged, and pages are read
//!   from it by row number. DuckDB moves the table to disk when memory runs short, so a result may exceed memory.
//! - `background` counts, summarises and exports without blocking the next cell (`shape.rs`).
//! - `meta` reads the catalog (`catalog.rs`).
//!
//! The session streams one result at a time. When another statement needs it, the open result stops growing;
//! if that result is scrolled further, its statement is run again as a subquery that skips the rows already cached.
//!
//! The connections keep the database open, so whoever owns the engine drops it once
//! [`Engine::has_other_connections`] says no one else is left.

mod catalog;
mod shape;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::duck::{Appender, Chunk, Conn, ConnHandle, QueryResult, Result, Returned};
use crate::protocol::{CellValue, Column, Outcome, Progress, ResultInfo, Rows, RunResponse, View};
use crate::{lock, sql};

/// Rows a cell's result holds right after it ran; more are loaded as the grid scrolls.
const INITIAL_ROWS: u64 = 1000;
/// Longest text of a value in a page of rows; the application fetches the whole value when it is opened.
const PREVIEW_CHARS: usize = 200;
/// Rows one page request may return.
const MAX_PAGE_ROWS: u64 = 2000;
/// Chunks (of up to 2048 rows) appended to the cache in one transaction.
const APPEND_BATCH_CHUNKS: usize = 32;
/// Results kept; the oldest is dropped when another would exceed this.
const MAX_RESULTS: usize = 64;
/// How long an interrupt keeps being repeated while the cell it is meant for is still running.
const INTERRUPT_PATIENCE: Duration = Duration::from_secs(5);
/// Connections the engine holds on its database.
const OWN_CONNECTIONS: u64 = 4;

pub struct Engine {
    session: Mutex<Session>,
    session_handle: ConnHandle,
    cache: Mutex<Conn>,
    background: Mutex<Conn>,
    background_handle: ConnHandle,
    meta: Mutex<Conn>,

    results: Mutex<HashMap<u64, Arc<Held>>>,
    next_id: AtomicU64,
    /// The session's current database and schema, which `background` adopts before it runs a subquery.
    search_path: Mutex<(String, String)>,

    /// Whether a cell is running, for the progress bar.
    running: AtomicBool,
    /// Counts the cells run, so that an interrupt meant for one does not reach the next.
    runs: AtomicU64,
    /// Whether the session is loading rows for a scrolling grid, which gives way to a cell that wants to run.
    loading: AtomicBool,
    /// Cells waiting for the session.
    waiting: AtomicUsize,
}

struct Session {
    /// The result being streamed.
    /// It must be dropped before the connection runs anything else, and before the connection itself,
    /// which is why it is the first field.
    cursor: Option<Cursor>,
    conn: Conn,
}

struct Cursor {
    held: Arc<Held>,
    rows: QueryResult,
}

impl Drop for Cursor {
    fn drop(&mut self) {
        self.held.streaming.store(false, SeqCst);
    }
}

/// A result the server holds for the application.
struct Held {
    id: u64,
    columns: Vec<Column>,
    /// The cache table; its columns are named c0, c1, ... so that duplicate result names cannot collide.
    table: String,
    /// How DuckDB can produce this result's rows again from their source, when the statement can be a subquery.
    /// Loading after a pause, counting, summarising, exporting and reshaping run against it.
    source: Option<Source>,
    view: View,
    /// Rows cached, and whether they are all of them.
    progress: Mutex<(u64, bool)>,
    /// Whether the session still has this result's cursor.
    streaming: AtomicBool,
}

/// Two queries over the source of a result, both with columns c0, c1, ...
struct Source {
    /// Returns the rows in the result's order, from the first one.
    ordered: String,
    /// Returns the same rows in any order: counting and summarising do not pay for a sort they do not need.
    unordered: String,
}

impl Held {
    fn progress(&self) -> (u64, bool) {
        *lock(&self.progress)
    }

    fn info(&self) -> ResultInfo {
        let (loaded, complete) = self.progress();
        ResultInfo {
            id: self.id,
            columns: self.columns.clone(),
            loaded,
            complete,
            streaming: self.streaming.load(SeqCst),
            repeatable: self.source.is_some(),
            view: self.view.clone(),
        }
    }
}

/// Clears a flag when a scope ends, however it ends.
struct FlagGuard<'a>(&'a AtomicBool);

impl<'a> FlagGuard<'a> {
    fn set(flag: &'a AtomicBool) -> FlagGuard<'a> {
        flag.store(true, SeqCst);
        FlagGuard(flag)
    }
}

impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, SeqCst);
    }
}

/// Read a count that DuckDB printed.
fn number(text: &Option<String>) -> u64 {
    text.as_ref().and_then(|text| text.parse().ok()).unwrap_or(0)
}

/// The number of rows of a table or a parenthesised subquery.
fn count_rows(conn: &Conn, rows: &str) -> Result<u64> {
    let counted = conn.rows(&format!("SELECT count(*)::VARCHAR FROM {rows}"), &[])?;
    Ok(number(&counted[0][0]))
}

/// Where unqualified names resolve for `conn`: its current database and schema.
fn search_path(conn: &Conn) -> Option<(String, String)> {
    let mut rows: Rows = conn.rows("SELECT current_database(), current_schema()", &[]).ok()?;
    let mut row = rows.pop()?;
    Some((row[0].take()?, row[1].take()?))
}

impl Engine {
    pub fn new(session: Conn, cache: Conn, background: Conn, meta: Conn) -> Engine {
        // DuckDB tracks a query's progress only while its progress bar is enabled; printing it is turned off.
        let _ = session.run("SET enable_progress_bar = true", &[]);
        let _ = session.run("SET enable_progress_bar_print = false", &[]);

        Engine {
            search_path: Mutex::new(search_path(&session).unwrap_or_default()),
            session_handle: session.handle(),
            session: Mutex::new(Session { cursor: None, conn: session }),
            cache: Mutex::new(cache),
            background_handle: background.handle(),
            background: Mutex::new(background),
            meta: Mutex::new(meta),
            results: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            running: AtomicBool::new(false),
            runs: AtomicU64::new(0),
            loading: AtomicBool::new(false),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Take the session for a cell: interrupt a row load that is in its way, and close the result it has open.
    /// The cell counts as running until the returned flag is dropped.
    fn begin_run(&self) -> (MutexGuard<'_, Session>, FlagGuard<'_>) {
        self.waiting.fetch_add(1, SeqCst);
        if self.loading.load(SeqCst) {
            self.session_handle.interrupt();
        }
        let mut session = lock(&self.session);
        self.waiting.fetch_sub(1, SeqCst);

        session.cursor = None;
        self.runs.fetch_add(1, SeqCst);
        (session, FlagGuard::set(&self.running))
    }

    fn held(&self, id: u64) -> Result<Arc<Held>> {
        let held = lock(&self.results).get(&id).cloned();
        held.ok_or_else(|| format!("result {id} is no longer held; run the cell again"))
    }

    /// Run the statements of a cell and hold the rows of the last one.
    pub fn run(&self, text: &str) -> RunResponse {
        let started = Instant::now();
        let outcome = self.run_statements(text).unwrap_or_else(|message| Outcome::Error { message });
        RunResponse { outcome, elapsed_ms: started.elapsed().as_millis() as u64 }
    }

    fn run_statements(&self, text: &str) -> Result<Outcome> {
        let (mut session, _running) = self.begin_run();

        let statements = session.conn.statements(text)?;
        let Some(last_index) = statements.len().checked_sub(1) else {
            return Ok(Outcome::Done);
        };
        for index in 0..last_index {
            statements.prepare(index)?.execute()?;
        }
        let last = statements.prepare(last_index)?;

        // Only a SELECT is streamed. Other statements run to completion, since stopping one early would undo it.
        if !last.is_select() {
            let rows = last.execute();
            self.remember_search_path(&session.conn);

            let mut rows = rows?;
            return match rows.returned() {
                Returned::Rows => self.hold(&mut session, rows, None, None, View::default()).map(Outcome::Rows),
                Returned::Changed(rows) => Ok(Outcome::Changed { rows }),
                Returned::Nothing => Ok(Outcome::Done),
            };
        }
        self.remember_search_path(&session.conn);

        // The statement's text is needed to run it again as a subquery.
        // It is known when this lexer and DuckDB's parser agree on where the statements end,
        // and usable when DuckDB accepts the subquery.
        let texts = sql::split_statements(text);
        let subquery = (texts.len() == statements.len()).then(|| {
            let names: Vec<_> = (0..last.column_count()).map(|index| format!("c{index}")).collect();
            format!("SELECT * FROM ({}\n) AS _q({})", texts[last_index], names.join(", "))
        });
        let source = subquery
            .filter(|subquery| session.conn.prepare(subquery).is_ok())
            .map(|subquery| Source { ordered: subquery.clone(), unordered: subquery });

        let mut rows = last.stream()?;
        match rows.returned() {
            Returned::Rows => self.hold(&mut session, rows, None, source, View::default()).map(Outcome::Rows),
            _ => Ok(Outcome::Done),
        }
    }

    /// Record where unqualified names resolve for the session, while it has no result open.
    fn remember_search_path(&self, session: &Conn) {
        if let Some(path) = search_path(session) {
            *lock(&self.search_path) = path;
        }
    }

    /// Start holding `rows`: create the cache table and move the first rows into it.
    ///
    /// `columns` is given when the rows are another result's, sorted or filtered: they keep its columns.
    fn hold(
        &self,
        session: &mut Session,
        mut rows: QueryResult,
        columns: Option<Vec<Column>>,
        source: Option<Source>,
        view: View,
    ) -> Result<ResultInfo> {
        let id = self.next_id.fetch_add(1, SeqCst);
        let table = format!("webui_result_{id}");

        // DuckDB names the types: the cache table takes them from the result, and DESCRIBE spells them.
        lock(&self.cache).create_table_like(&table, &mut rows)?;
        let columns = match columns {
            Some(columns) => columns,
            None => {
                let types = lock(&self.cache).rows(&format!("SELECT column_type FROM (DESCRIBE {table})"), &[])?;
                let column = |(index, mut described): (usize, Vec<Option<String>>)| Column {
                    name: rows.column_name(index),
                    data_type: described[0].take().unwrap_or_default(),
                };
                types.into_iter().enumerate().map(column).collect()
            }
        };

        let progress = Mutex::new((0, false));
        let held = Arc::new(Held { id, columns, table, source, view, progress, streaming: AtomicBool::new(true) });

        session.cursor = Some(Cursor { held: held.clone(), rows });
        if let Err(error) = self.pump(session, &held, INITIAL_ROWS, false) {
            session.cursor = None;
            self.drop_cache_table(&held.table);
            return Err(error);
        }

        self.keep(held.clone());
        Ok(held.info())
    }

    /// Move rows from the session's cursor into the cache, until it holds `want` rows or the cursor ends.
    ///
    /// With `give_way`, stop as soon as a cell is waiting for the session.
    fn pump(&self, session: &mut Session, held: &Held, want: u64, give_way: bool) -> Result<()> {
        let cell_waits = || give_way && self.waiting.load(SeqCst) > 0;

        loop {
            let (loaded, complete) = held.progress();
            if complete || loaded >= want || cell_waits() {
                return Ok(());
            }
            let Some(cursor) = session.cursor.as_mut() else {
                return Ok(());
            };

            let mut chunks: Vec<Chunk> = Vec::new();
            let mut rows = 0;
            let mut ended = false;

            while chunks.len() < APPEND_BATCH_CHUNKS && loaded + rows < want && !ended {
                match cursor.rows.fetch() {
                    Ok(Some(chunk)) => {
                        rows += chunk.len() as u64;
                        chunks.push(chunk);
                    }
                    Ok(None) => ended = true,
                    Err(error) => {
                        session.cursor = None;
                        // A load that a waiting cell interrupted is not an error: its rows can be loaded later.
                        return if cell_waits() { Ok(()) } else { Err(error) };
                    }
                }
            }

            if !chunks.is_empty() {
                let cache = lock(&self.cache);
                let appender = chunks.iter().try_fold(Appender::new(&cache, &held.table)?, Appender::append)?;
                appender.finish()?;
            }
            *lock(&held.progress) = (loaded + rows, ended);
            if ended {
                session.cursor = None;
            }
        }
    }

    fn keep(&self, held: Arc<Held>) {
        let mut results = lock(&self.results);
        results.insert(held.id, held);

        while results.len() > MAX_RESULTS {
            let oldest = *results.keys().min().unwrap();
            let dropped = results.remove(&oldest).unwrap();
            self.drop_cache_table(&dropped.table);
        }
    }

    fn drop_cache_table(&self, table: &str) {
        let _ = lock(&self.cache).run(&format!("DROP TABLE IF EXISTS {table}"), &[]);
    }

    /// Load rows until the cache holds `want` of them, the result ends, or a cell needs the session.
    ///
    /// When the statement is no longer open, it is run again only with `resume`.
    pub fn load(&self, id: u64, want: u64, resume: bool) -> Result<ResultInfo> {
        let held = self.held(id)?;
        let (loaded, complete) = held.progress();
        if complete || loaded >= want {
            return Ok(held.info());
        }

        let mut session = lock(&self.session);
        let _loading = FlagGuard::set(&self.loading);

        if session.cursor.as_ref().is_none_or(|cursor| cursor.held.id != id) {
            let Some(source) = held.source.as_ref().filter(|_| resume) else {
                return Ok(held.info());
            };

            // Another statement took the session since: run this one again, past the rows already cached.
            session.cursor = None;
            let loaded = held.progress().0;
            let rows = session.conn.prepare(&format!("{} OFFSET {loaded}", source.ordered))?.stream()?;

            held.streaming.store(true, SeqCst);
            session.cursor = Some(Cursor { held: held.clone(), rows });
        }

        self.pump(&mut session, &held, want, true)?;
        Ok(held.info())
    }

    /// Rows `offset..offset + limit` of the cache, every value cut to a preview.
    pub fn page(&self, id: u64, offset: u64, limit: u64) -> Result<Rows> {
        let held = self.held(id)?;
        let end = offset.saturating_add(limit.min(MAX_PAGE_ROWS));

        let previews: Vec<_> =
            (0..held.columns.len()).map(|index| format!("left(c{index}::VARCHAR, {PREVIEW_CHARS})")).collect();
        let query = format!(
            "SELECT {} FROM {} WHERE rowid >= {offset} AND rowid < {end} ORDER BY rowid",
            previews.join(", "),
            held.table
        );
        lock(&self.cache).rows(&query, &[])
    }

    /// One whole value as text, cut at `limit` characters, and as JSON when it has parts.
    pub fn value(&self, id: u64, row: u64, column: usize, limit: u64) -> Result<CellValue> {
        let held = self.held(id)?;

        // to_json() of a struct, list, map, union or JSON document starts with a brace or a bracket;
        // of any other value it is a number or a quoted string.
        let query = format!(
            "SELECT left(text, {limit}), length(text)::VARCHAR, \
             CASE WHEN json[1] IN ('{{', '[') AND length(json) <= {limit} THEN json END \
             FROM (SELECT c{column}::VARCHAR AS text, to_json(c{column})::VARCHAR AS json FROM {} WHERE rowid = {row})",
            held.table
        );
        let mut found = lock(&self.cache).rows(&query, &[])?.pop().ok_or("no such row")?;

        Ok(CellValue { text: found[0].take(), length: number(&found[1]), json: found[2].take() })
    }

    /// Stop holding a result and free its cache.
    pub fn forget(&self, id: u64) {
        let Some(held) = lock(&self.results).remove(&id) else {
            return;
        };

        // Its cursor is closed only if the session is idle; otherwise the next statement closes it.
        if let Ok(mut session) = self.session.try_lock()
            && session.cursor.as_ref().is_some_and(|cursor| cursor.held.id == id)
        {
            session.cursor = None;
        }
        self.drop_cache_table(&held.table);
    }

    /// Stop holding every result.
    pub fn forget_all(&self) {
        let ids: Vec<u64> = lock(&self.results).keys().copied().collect();
        ids.into_iter().for_each(|id| self.forget(id));
    }

    /// Cancel the cell or row load that is running, and whatever the background connection is computing.
    pub fn interrupt(&self) {
        self.background_handle.interrupt();

        // An idle session may hold an open result, which an interrupt would end:
        // only a running cell or a row load is interrupted.
        if self.loading.load(SeqCst) {
            self.session_handle.interrupt();
        }

        // DuckDB forgets an interrupt that arrives just before a statement starts,
        // so the one for a cell is repeated until that cell has stopped.
        let run = self.runs.load(SeqCst);
        let deadline = Instant::now() + INTERRUPT_PATIENCE;
        while self.running.load(SeqCst) && self.runs.load(SeqCst) == run && Instant::now() < deadline {
            self.session_handle.interrupt();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn progress(&self) -> Progress {
        let running = self.running.load(SeqCst);
        Progress { running, percentage: running.then(|| self.session_handle.progress()).flatten() }
    }

    /// Whether the database has a connection besides the engine's own.
    pub fn has_other_connections(&self) -> Result<bool> {
        let counted = lock(&self.meta).rows("SELECT count::VARCHAR FROM duckdb_connection_count()", &[])?;
        Ok(number(&counted[0][0]) > OWN_CONNECTIONS)
    }
}
