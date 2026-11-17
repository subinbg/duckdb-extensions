//! What DuckDB computes over every row of a result: its sorted and filtered form, its count, a column's summary,
//! and its export to a file.
//!
//! A finished result has all its rows in the cache, and they are read from there.
//! An unfinished one is computed at its source, the statement run again as a subquery, so nothing has to be loaded.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{Engine, Held, PREVIEW_CHARS, Source, count_rows, number};
use crate::duck::{Conn, Result, quote_identifier, quote_literal};
use crate::lock;
use crate::protocol::{ExportFormat, Filter, FilterOp, ResultInfo, Summary, View};

/// A column with more distinct values than this gets no list of its most frequent values.
const MAX_GROUPS_FOR_TOP_VALUES: u64 = 100_000;

/// The columns of DuckDB's `SUMMARIZE` that a summary shows, as SQL and as the application labels them.
const FACTS: [(&str, &str); 10] = [
    ("column_type", "Type"),
    ("null_percentage::VARCHAR || '%'", "Nulls"),
    ("'≈ ' || approx_unique", "Distinct"),
    ("min", "Min"),
    ("max", "Max"),
    ("avg", "Mean"),
    ("std", "Std dev"),
    ("q25", "25%"),
    ("q50", "Median"),
    ("q75", "75%"),
];

impl Engine {
    /// The background connection, resolving unqualified names as the session does.
    fn background(&self) -> MutexGuard<'_, Conn> {
        let background = lock(&self.background);
        let (database, schema) = &*lock(&self.search_path);
        let _ = background.run(&format!("USE {}.{}", quote_identifier(database), quote_identifier(schema)), &[]);
        background
    }

    /// Hold `base`'s rows sorted and filtered by `view`, computed by DuckDB over all of them.
    pub fn reshape(&self, base_id: u64, view: View) -> Result<ResultInfo> {
        let base = self.held(base_id)?;
        if view.is_empty() {
            return Ok(base.info());
        }

        let conditions: Vec<_> = view.filters.iter().map(condition).collect();
        let filter = match conditions.is_empty() {
            true => String::new(),
            false => format!(" WHERE {}", conditions.join(" AND ")),
        };
        let direction = |descending| if descending { "DESC" } else { "ASC" };
        let mut keys: Vec<_> =
            view.sort.iter().map(|key| format!("c{} {} NULLS LAST", key.column, direction(key.descending))).collect();

        // A finished result is reshaped from its cache, into another cache table.
        if base.progress().1 {
            let id = self.next_id.fetch_add(1, SeqCst);
            let table = format!("webui_result_{id}");
            keys.push("rowid".to_string());

            let cache = lock(&self.cache);
            let select = format!("SELECT * FROM {}{filter} ORDER BY {}", base.table, keys.join(", "));
            cache.run(&format!("CREATE TEMP TABLE {table} AS {select}"), &[]).map_err(first_paragraph)?;
            let progress = Mutex::new((count_rows(&cache, &table)?, true));
            drop(cache);

            let columns = base.columns.clone();
            let streaming = AtomicBool::new(false);
            let held = Arc::new(Held { id, columns, table, source: None, view, progress, streaming });
            self.keep(held.clone());
            return Ok(held.info());
        }

        // An unfinished one is reshaped at its source, as a new stream on the session.
        let source =
            base.source.as_ref().ok_or("this result can be sorted and filtered once all its rows are loaded")?;
        let order = match keys.is_empty() {
            true => String::new(),
            false => format!(" ORDER BY {}", keys.join(", ")),
        };
        let unordered = format!("SELECT * FROM ({}) AS _v{filter}", source.unordered);
        let reshaped = Source { ordered: format!("{unordered}{order}"), unordered };

        let (mut session, _running) = self.begin_run();
        let rows =
            session.conn.prepare(&reshaped.ordered).and_then(|prepared| prepared.stream()).map_err(first_paragraph)?;
        self.hold(&mut session, rows, Some(base.columns.clone()), Some(reshaped), view)
    }

    /// Summarise one column: over every row where DuckDB can compute that, otherwise over the rows loaded so far.
    pub fn summarize(&self, id: u64, column: usize) -> Result<Summary> {
        let held = self.held(id)?;
        let complete = held.progress().1;

        if let (Some(source), false) = (&held.source, complete) {
            let at_source = summary(&self.background(), &format!("({}) AS _s", source.unordered), column, true);

            match at_source {
                // The background connection cannot run a statement that reads a temporary table of the session:
                // the rows in the cache are summarised instead.
                Err(error) if !error.contains("INTERRUPT") => {}
                at_source => return at_source,
            }
        }
        summary(&lock(&self.cache), &held.table, column, complete)
    }

    /// Write every row of a result to `path`, from its source when it is unfinished.
    pub fn export(&self, id: u64, format: ExportFormat, path: &str) -> Result<()> {
        let held = self.held(id)?;

        // Parquet needs distinct column names; a repeated name gets a numbered suffix.
        let mut uses: HashMap<&str, usize> = HashMap::new();
        let mut columns = Vec::new();
        for (index, column) in held.columns.iter().enumerate() {
            let name = match *uses.entry(&column.name).and_modify(|uses| *uses += 1).or_insert(1) {
                1 => column.name.clone(),
                uses => format!("{}_{uses}", column.name),
            };
            columns.push(format!("c{index} AS {}", quote_identifier(&name)));
        }

        let options = match format {
            ExportFormat::Csv => "FORMAT csv, HEADER",
            ExportFormat::Parquet => "FORMAT parquet",
            ExportFormat::Json => "FORMAT json, ARRAY",
        };
        let copy = |rows: &str| {
            format!("COPY (SELECT {} FROM {rows}) TO {} ({options})", columns.join(", "), quote_literal(path))
        };

        match (&held.source, held.progress().1) {
            (_, true) => lock(&self.cache).run(&copy(&format!("{} ORDER BY rowid", held.table)), &[]).map(drop),
            (Some(source), false) => {
                self.background().run(&copy(&format!("({}) AS _e", source.ordered)), &[]).map(drop)
            }
            (None, false) => Err("this result can be exported once all its rows are loaded".to_string()),
        }
    }
}

/// Keep what went wrong and drop the excerpt of SQL after it, which quotes a query the user did not write.
fn first_paragraph(error: String) -> String {
    error.split("\n\n").next().unwrap_or_default().to_string()
}

/// Spell one filter as a SQL condition on its column.
///
/// The value is an untyped literal, which DuckDB reads as the column's type: numbers and dates compare as such.
/// Matching looks at the value's text.
fn condition(filter: &Filter) -> String {
    let column = format!("c{}", filter.column);
    let value = quote_literal(&filter.value);
    let text = format!("lower({column}::VARCHAR)");

    match filter.op {
        FilterOp::Contains => format!("contains({text}, lower({value}))"),
        FilterOp::NotContains => format!("NOT coalesce(contains({text}, lower({value})), false)"),
        FilterOp::StartsWith => format!("starts_with({text}, lower({value}))"),
        FilterOp::Equals => format!("{column} = {value}"),
        FilterOp::NotEquals => format!("{column} IS DISTINCT FROM {value}"),
        FilterOp::Greater => format!("{column} > {value}"),
        FilterOp::GreaterOrEqual => format!("{column} >= {value}"),
        FilterOp::Less => format!("{column} < {value}"),
        FilterOp::LessOrEqual => format!("{column} <= {value}"),
        FilterOp::IsNull => format!("{column} IS NULL"),
        FilterOp::IsNotNull => format!("{column} IS NOT NULL"),
    }
}

/// Summarise column `column` of `rows`, a table or a parenthesised subquery, with DuckDB's `SUMMARIZE`.
fn summary(conn: &Conn, rows: &str, column: usize, all_rows: bool) -> Result<Summary> {
    let facts: Vec<_> = FACTS.iter().map(|(fact, _)| format!("left({fact}, {PREVIEW_CHARS})")).collect();
    let summarized = conn.rows(
        &format!(
            "SELECT count::VARCHAR, approx_unique::VARCHAR, {} FROM (SUMMARIZE SELECT c{column} FROM {rows})",
            facts.join(", ")
        ),
        &[],
    )?;
    let (counts, facts) = summarized.first().ok_or("DuckDB summarised nothing")?.split_at(2);

    let top = match number(&counts[1]) <= MAX_GROUPS_FOR_TOP_VALUES {
        true => conn.rows(
            &format!(
                "SELECT left(value::VARCHAR, {PREVIEW_CHARS}), rows::VARCHAR FROM \
                 (SELECT c{column} AS value, count(*) AS rows FROM {rows} GROUP BY ALL ORDER BY rows DESC LIMIT 10)"
            ),
            &[],
        )?,
        false => Vec::new(),
    };

    Ok(Summary {
        all_rows,
        rows: number(&counts[0]),
        facts: FACTS
            .iter()
            .zip(facts)
            .filter_map(|((_, label), fact)| Some((label.to_string(), fact.clone()?)))
            .collect(),
        top: top.into_iter().map(|mut row| (row[0].take(), number(&row[1]))).collect(),
    })
}
