//! Google's side: credentials, clients, and each call the table functions make, through the Google Cloud Rust SDK.
//!
//! The SDK is asynchronous and DuckDB is not, so every call is run to completion on one runtime of the extension's.

use std::error::Error;
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use arrow::array::{StringArray, TimestampMillisecondArray};
use arrow::buffer::Buffer;
use arrow::datatypes::SchemaRef;
use arrow::ipc::reader::{StreamDecoder, StreamReader};
use arrow::record_batch::RecordBatch;
use google_cloud_auth::credentials::Builder as Credentials;
use google_cloud_bigquery::client::Read;
use google_cloud_bigquery::model::arrow_serialization_options::CompressionCodec;
use google_cloud_bigquery::model::read_session::TableReadOptions;
use google_cloud_bigquery::model::{ArrowSerializationOptions, DataFormat, ReadSession};
use google_cloud_bigquery::read::Reader;
use google_cloud_bigquery_v2::client::{DatasetService, JobService, TableService};
use google_cloud_bigquery_v2::model::{Job, JobConfiguration, JobConfigurationQuery};
use google_cloud_gax::paginator::ItemPaginator;
use tokio::runtime::Runtime;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The longest wait between two looks at a running query job.
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(2);

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime::new().expect("a Tokio runtime"));

/// Run a call of the SDK on the extension's runtime, and wait for it.
pub fn wait<F: Future>(future: F) -> F::Output {
    RUNTIME.block_on(future)
}

struct Clients {
    datasets: DatasetService,
    tables: TableService,
    jobs: JobService,
    read: Read,
}

/// The clients, built on first use with Application Default Credentials and kept for the process.
///
/// A failure is not kept, so a database that loaded the extension before signing in works once the user has.
fn clients() -> Result<&'static Clients> {
    static CLIENTS: OnceLock<Clients> = OnceLock::new();
    if let Some(clients) = CLIENTS.get() {
        return Ok(clients);
    }

    let clients = wait(async {
        let credentials = Credentials::default().build()?;
        Ok::<_, Box<dyn Error>>(Clients {
            datasets: DatasetService::builder().with_credentials(credentials.clone()).build().await?,
            tables: TableService::builder().with_credentials(credentials.clone()).build().await?,
            jobs: JobService::builder().with_credentials(credentials.clone()).build().await?,
            read: Read::builder().with_credentials(credentials).build().await?,
        })
    })?;
    Ok(CLIENTS.get_or_init(|| clients))
}

/// A table as BigQuery names it: `project.dataset.table`.
pub struct Table {
    pub project: String,
    pub dataset: String,
    pub table: String,
}

impl Table {
    pub fn parse(path: &str) -> Result<Table> {
        let [project, dataset, table] = split(path)?;
        Ok(Table { project, dataset, table })
    }

    fn resource(&self) -> String {
        format!("projects/{}/datasets/{}/tables/{}", self.project, self.dataset, self.table)
    }
}

/// The `N` dot-separated parts of a name; the project, which comes first, may itself hold dots (`example.com:p`).
fn split<const N: usize>(path: &str) -> Result<[String; N]> {
    let mut parts: Vec<String> = path.rsplitn(N, '.').map(str::to_string).collect();
    parts.reverse();
    match <[String; N]>::try_from(parts) {
        Ok(parts) if !parts.iter().any(String::is_empty) => Ok(parts),
        _ => Err(format!("'{path}' is not {N} names separated by dots").into()),
    }
}

/// `bigquery_datasets('project')`: one row per dataset, with its location.
pub fn datasets(project: &str) -> Result<RecordBatch> {
    let mut listing = clients()?.datasets.list_datasets().set_project_id(project).by_item();
    let (mut names, mut locations) = (vec![], vec![]);
    while let Some(dataset) = wait(listing.next()).transpose()? {
        names.push(dataset.dataset_reference.map(|reference| reference.dataset_id).unwrap_or_default());
        locations.push(dataset.location);
    }
    Ok(RecordBatch::try_from_iter([
        ("dataset", Arc::new(StringArray::from(names)) as _),
        ("location", Arc::new(StringArray::from(locations)) as _),
    ])?)
}

/// `bigquery_tables('project.dataset')`: one row per table, view, materialized view, snapshot or external table.
pub fn tables(path: &str) -> Result<RecordBatch> {
    let [project, dataset] = split(path)?;
    let mut listing = clients()?.tables.list_tables().set_project_id(project).set_dataset_id(dataset).by_item();
    let (mut names, mut kinds, mut created) = (vec![], vec![], vec![]);
    while let Some(table) = wait(listing.next()).transpose()? {
        names.push(table.table_reference.map(|reference| reference.table_id).unwrap_or_default());
        kinds.push(table.r#type);
        created.push(table.creation_time);
    }
    Ok(RecordBatch::try_from_iter([
        ("table", Arc::new(StringArray::from(names)) as _),
        ("type", Arc::new(StringArray::from(kinds)) as _),
        ("created", Arc::new(TimestampMillisecondArray::from(created)) as _),
    ])?)
}

/// Run a GoogleSQL query in `project`, which pays for it, and return the table BigQuery wrote its result to.
///
/// A dry run first tells what the statement is, and only a SELECT is run: the extension reads, whatever the
/// account may do. The jobs name their project in the request only. As a quota project, it would also ask the
/// caller for `serviceusage.services.use` on it, which BigQuery does not need.
pub fn query(project: &str, sql: &str) -> Result<Table> {
    let jobs = &clients()?.jobs;
    let job = |dry_run: bool| {
        let query = JobConfigurationQuery::new().set_query(sql).set_use_legacy_sql(false);
        Job::new().set_configuration(JobConfiguration::new().set_query(query).set_dry_run(dry_run))
    };

    let job = wait(async {
        let dry_run = jobs.insert_job().set_project_id(project).set_job(job(true)).send().await?;
        let statement = dry_run.statistics.and_then(|statistics| statistics.query).unwrap_or_default().statement_type;
        if statement != "SELECT" {
            return Err(format!("bigquery_query runs only SELECT statements, and this is {statement}").into());
        }

        let mut job = jobs.insert_job().set_project_id(project).set_job(job(false)).send().await?;
        let mut pause = Duration::from_millis(100);
        while job.status.as_ref().is_some_and(|status| status.state != "DONE") {
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(MAX_POLL_INTERVAL);
            let reference = job.job_reference.unwrap_or_default();
            let mut get = jobs.get_job().set_project_id(reference.project_id).set_job_id(reference.job_id);
            if let Some(location) = reference.location {
                get = get.set_location(location);
            }
            job = get.send().await?;
        }
        Ok::<_, Box<dyn Error>>(job)
    })?;

    if let Some(error) = job.status.and_then(|status| status.error_result) {
        return Err(error.message.into());
    }
    let result = job.configuration.and_then(|configuration| configuration.query?.destination_table);
    let result = result.ok_or("BigQuery wrote the result to no table")?;
    Ok(Table { project: result.project_id, dataset: result.dataset_id, table: result.table_id })
}

/// What a read session gives: the serialized Arrow schema of its rows, and the streams that together hold them.
pub struct Session {
    arrow_schema: Buffer,
    pub streams: Vec<String>,
    pub estimated_rows: u64,
}

/// Start reading `columns` of `table` (all of them when empty) through the Storage Read API, as Arrow.
///
/// `billing` pays for the read. `filter` is a row restriction in GoogleSQL, such as `year > 2020`, or empty.
/// BigQuery splits the rows into up to `streams` streams, which can be read at the same time.
pub fn open(billing: &str, table: &Table, columns: &[&str], filter: &str, streams: usize) -> Result<Session> {
    let compression = ArrowSerializationOptions::new().set_buffer_compression(CompressionCodec::Lz4Frame);
    let options = TableReadOptions::new()
        .set_selected_fields(columns.iter().copied())
        .set_row_restriction(filter)
        .set_arrow_serialization_options(compression);
    let request = ReadSession::new().set_table(table.resource()).set_data_format(DataFormat::Arrow);

    let session = wait(
        clients()?
            .read
            .create_read_session()
            .set_parent(format!("projects/{billing}"))
            .set_read_session(request.set_read_options(options))
            .set_max_stream_count(streams as i32)
            .send(),
    )?;
    let schema = session.arrow_schema().ok_or("BigQuery returned no Arrow schema")?;
    Ok(Session {
        arrow_schema: Buffer::from(schema.serialized_schema.clone()),
        streams: session.streams.into_iter().map(|stream| stream.name).collect(),
        estimated_rows: session.estimated_row_count.max(0) as u64,
    })
}

impl Session {
    /// The Arrow schema of the session's rows.
    pub fn schema(&self) -> Result<SchemaRef> {
        Ok(StreamReader::try_new(self.arrow_schema.as_slice(), None)?.schema())
    }
}

/// One stream of a session: its record batches, decoded as they arrive.
pub struct Stream {
    reader: Reader,
    decoder: StreamDecoder,
}

impl Stream {
    pub fn new(session: &Session, name: &str) -> Result<Stream> {
        // The decoder ends a message only on the bytes after it, so it reads this schema with the first batch.
        let mut decoder = StreamDecoder::new();
        decoder.decode(&mut session.arrow_schema.clone())?;
        Ok(Stream { reader: clients()?.read.read_rows().set_read_stream(name).into_reader(), decoder })
    }

    /// The next record batch of the stream, or `None` after the last one.
    pub fn next(&mut self) -> Result<Option<RecordBatch>> {
        while let Some(response) = wait(self.reader.next()).transpose()? {
            let Some(batch) = response.arrow_record_batch() else { continue };
            if let Some(batch) = self.decoder.decode(&mut Buffer::from(batch.serialized_record_batch.clone()))? {
                return Ok(Some(batch));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn splits_names_from_the_right() {
        assert_eq!(split::<3>("p.d.t").unwrap(), ["p", "d", "t"]);
        assert_eq!(split::<3>("example.com:p.d.t").unwrap(), ["example.com:p", "d", "t"]);
        assert_eq!(split::<2>("p.d").unwrap(), ["p", "d"]);
        assert!(split::<3>("d.t").is_err());
        assert!(split::<3>("p..t").is_err());
    }
}
