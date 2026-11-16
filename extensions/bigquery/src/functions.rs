//! The table functions, and how they hand Arrow record batches to DuckDB, one vector at a time.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use duckdb::core::{DataChunkHandle, LogicalTypeHandle, LogicalTypeId};
use duckdb::vtab::to_duckdb_logical_type_for_field as duckdb_type;
use duckdb::vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab, record_batch_to_duckdb_data_chunk};

use crate::google::{self, Result, Session, Stream, Table};

/// Rows DuckDB takes per call.
const VECTOR_SIZE: usize = 2048;

fn text() -> LogicalTypeHandle {
    LogicalTypeId::Varchar.into()
}

fn argument(bind: &BindInfo, index: u64) -> String {
    bind.get_parameter(index).to_string()
}

fn named(bind: &BindInfo, name: &str) -> Option<String> {
    bind.get_named_parameter(name).map(|value| value.to_string())
}

/// Declare the columns of `schema` to DuckDB. A type DuckDB has no counterpart for is text, as is BIGNUMERIC:
/// its 76 digits do not fit DuckDB's DECIMAL.
fn declare(bind: &BindInfo, schema: &Schema) -> Result<SchemaRef> {
    let mut fields = vec![];
    for field in schema.fields() {
        let field = match (field.data_type(), duckdb_type(field)) {
            (DataType::Decimal256(..), _) | (_, Err(_)) => Field::new(field.name(), DataType::Utf8, true),
            _ => field.as_ref().clone(),
        };
        bind.add_result_column(field.name(), duckdb_type(&field)?);
        fields.push(field);
    }
    Ok(Arc::new(Schema::new(fields)))
}

/// Where a read is in one stream: the batch it is handing out, and how much of it is handed out already.
struct Cursor {
    stream: Option<Stream>,
    batch: RecordBatch,
    offset: usize,
}

/// The streams of one read, which DuckDB's threads share: a thread takes the first, hands out a vector of its rows,
/// and puts it back first. A thread that finds none left is done, while the others finish theirs.
///
/// Streams are so read one after the other, at most one per thread at a time. Their responses share one HTTP/2
/// connection, and one that nobody reads would hold its flow-control window, stalling the stream being waited on.
pub struct Rows {
    idle: Mutex<VecDeque<Cursor>>,
    /// The columns DuckDB asked for, in its order and with the types it was told.
    shape: SchemaRef,
}

impl Rows {
    fn of(batch: RecordBatch) -> Rows {
        let shape = batch.schema();
        Rows { idle: Mutex::new(VecDeque::from([Cursor { stream: None, batch, offset: 0 }])), shape }
    }

    fn take(&self) -> Option<Cursor> {
        self.idle.lock().unwrap_or_else(PoisonError::into_inner).pop_front()
    }

    /// Fill `output` with the next vector of rows of any stream, or with none when every stream is done.
    fn emit(&self, output: &mut DataChunkHandle) -> Result<()> {
        while let Some(mut cursor) = self.take() {
            if cursor.offset == cursor.batch.num_rows() && !self.advance(&mut cursor)? {
                continue;
            }
            let length = VECTOR_SIZE.min(cursor.batch.num_rows() - cursor.offset);
            record_batch_to_duckdb_data_chunk(&cursor.batch.slice(cursor.offset, length), output)?;
            cursor.offset += length;
            self.idle.lock().unwrap_or_else(PoisonError::into_inner).push_front(cursor);
            return Ok(());
        }
        output.set_len(0);
        Ok(())
    }

    /// Move `cursor` to the next batch of its stream that has rows, in the shape DuckDB expects; false at its end.
    fn advance(&self, cursor: &mut Cursor) -> Result<bool> {
        let Some(stream) = cursor.stream.as_mut() else { return Ok(false) };
        while let Some(batch) = stream.next()? {
            if batch.num_rows() == 0 {
                continue;
            }
            let columns = self.shape.fields().iter().map(|field| {
                let column = batch.column_by_name(field.name()).ok_or("BigQuery left out a column")?;
                Ok(cast(column, field.data_type())?)
            });
            // The row count is given for a count(*), which reads no column.
            let rows = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
            cursor.batch =
                RecordBatch::try_new_with_options(self.shape.clone(), columns.collect::<Result<_>>()?, &rows)?;
            cursor.offset = 0;
            return Ok(true);
        }
        Ok(false)
    }
}

/// What `bigquery_scan` and `bigquery_query` bind: the table to read, and how DuckDB was told its columns are.
pub struct Read {
    billing: String,
    table: Table,
    filter: String,
    schema: SchemaRef,
    /// Whether rows must come in BigQuery's order, as those of a query with ORDER BY do: then one stream is read.
    ordered: bool,
}

impl Read {
    fn bind(bind: &BindInfo, billing: String, table: Table, filter: String, ordered: bool) -> Result<Read> {
        // A session only for its schema: what DuckDB asks for is known only once it starts reading.
        let session = google::open(&billing, &table, &[], &filter, 1)?;
        bind.set_cardinality(session.estimated_rows, false);
        let schema = declare(bind, session.schema()?.as_ref())?;
        Ok(Read { billing, table, filter, schema, ordered })
    }

    /// Open a session for the columns DuckDB asked for, and one cursor per stream.
    fn init(init: &InitInfo) -> Result<Rows> {
        let read = unsafe { &*init.get_bind_data::<Read>() };
        let fields: Vec<Field> = init
            .get_column_indices()
            .iter()
            .filter_map(|&index| read.schema.fields().get(index as usize))
            .map(|field| field.as_ref().clone())
            .collect();

        // Reading no column reads them all, so a count(*) reads the first.
        let names: Vec<&str> = fields.iter().map(|field| field.name().as_str()).collect();
        let selected = if names.is_empty() { vec![read.schema.field(0).name().as_str()] } else { names };

        let parallel = std::thread::available_parallelism().map_or(1, usize::from);
        let streams = if read.ordered { 1 } else { parallel };
        let session: Session = google::open(&read.billing, &read.table, &selected, &read.filter, streams)?;

        let shape = Arc::new(Schema::new(fields));
        let empty = RecordBatch::new_empty(shape.clone());
        let mut cursors = VecDeque::new();
        for name in &session.streams {
            cursors.push_back(Cursor { stream: Some(Stream::new(&session, name)?), batch: empty.clone(), offset: 0 });
        }
        init.set_max_threads(cursors.len().max(1) as u64);
        Ok(Rows { idle: Mutex::new(cursors), shape })
    }
}

/// `bigquery_scan('project.dataset.table', filter := 'year > 2020', billing_project := 'project')`
pub struct Scan;

impl VTab for Scan {
    type BindData = Read;
    type InitData = Rows;

    fn bind(bind: &BindInfo) -> Result<Read> {
        let table = Table::parse(&argument(bind, 0))?;
        let billing = named(bind, "billing_project").unwrap_or_else(|| table.project.clone());
        Read::bind(bind, billing, table, named(bind, "filter").unwrap_or_default(), false)
    }

    fn init(init: &InitInfo) -> Result<Rows> {
        Read::init(init)
    }

    fn func(info: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> Result<()> {
        info.get_init_data().emit(output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![text()])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(vec![("filter".to_string(), text()), ("billing_project".to_string(), text())])
    }
}

/// `bigquery_query('project', 'SELECT ...')`: the project runs the query and pays for it.
pub struct Query;

impl VTab for Query {
    type BindData = Read;
    type InitData = Rows;

    fn bind(bind: &BindInfo) -> Result<Read> {
        let project = argument(bind, 0);
        let table = google::query(&project, &argument(bind, 1))?;
        Read::bind(bind, project, table, String::new(), true)
    }

    fn init(init: &InitInfo) -> Result<Rows> {
        Read::init(init)
    }

    fn func(info: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> Result<()> {
        info.get_init_data().emit(output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![text(), text()])
    }
}

/// A listing of the catalog, such as `google::datasets`, which takes the one argument of its table function.
pub type List = fn(&str) -> Result<RecordBatch>;

/// `bigquery_datasets('project')` and `bigquery_tables('project.dataset')`: registered with their [`List`].
pub struct Listing;

impl VTab for Listing {
    type BindData = RecordBatch;
    type InitData = Rows;

    fn bind(bind: &BindInfo) -> Result<RecordBatch> {
        let list = unsafe { *bind.get_extra_info::<List>() };
        let batch = list(&argument(bind, 0))?;
        declare(bind, &batch.schema())?;
        Ok(batch)
    }

    fn init(init: &InitInfo) -> Result<Rows> {
        Ok(Rows::of(unsafe { &*init.get_bind_data::<RecordBatch>() }.clone()))
    }

    fn func(info: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> Result<()> {
        info.get_init_data().emit(output)
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![text()])
    }
}
