//! The HTTP server: the browser application's files, and the one address it posts its requests to.
//!
//! Nothing here reaches out to the network.
//! The application's files are compiled into the extension, and every request must carry the token that is part of
//! the URL `webui_start()` returned, so other users of the machine cannot run SQL through it.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;
use tiny_http::{Header, Method, Response, ResponseBox};

use crate::duck::Result;
use crate::engine::Engine;
use crate::lock;
use crate::protocol::{ErrorBody, ExportFormat, Notebook, Request};

/// The files of the browser application in `app/`, by the name they are served under.
macro_rules! assets {
    ($($name:literal),*) => { [$(($name, include_bytes!(concat!("../app/", $name)).as_slice())),*] };
}
static ASSETS: [(&str, &[u8]); 15] = assets![
    "index.html",
    "style.css",
    "favicon.svg",
    "app.js",
    "api.js",
    "catalog.js",
    "cell.js",
    "dom.js",
    "editor.js",
    "format.js",
    "grid.js",
    "inspector.js",
    "menu.js",
    "sql.js",
    "templates.js"
];

/// The header that carries the access token on every request.
const TOKEN_HEADER: &str = "X-Webui-Token";
pub const DEFAULT_PORT: u16 = 4213;
/// Ports tried after the default one when it is taken and none was asked for.
const PORT_ATTEMPTS: u16 = 20;
const WORKERS: usize = 12;
const MAX_BODY_BYTES: u64 = 16 << 20;
/// Characters of one value the application may ask for at once.
const MAX_VALUE_CHARS: u64 = 16 << 20;

/// No page of the application may load anything from another origin.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; base-uri 'none'; form-action 'none'";

pub struct Options {
    pub host: String,
    /// `None` takes the default port, or the next free one after it.
    pub port: Option<u16>,
    /// `None` generates a random token.
    pub token: Option<String>,
    /// Where the notebook is saved; `None` keeps it in memory only.
    pub state_directory: Option<PathBuf>,
}

pub struct Server {
    /// The address to open in a browser, with the token.
    pub url: String,
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

struct Shared {
    engine: Arc<Engine>,
    token: String,
    stopping: AtomicBool,
    state_directory: Option<PathBuf>,
    /// The notebook, when it cannot be saved to a file.
    notebook: Mutex<Notebook>,
    /// Exported files waiting to be downloaded, by their one-time key.
    downloads: Mutex<HashMap<String, PathBuf>>,
}

impl Server {
    pub fn start(engine: Arc<Engine>, options: Options) -> Result<Server> {
        let host = options.host.as_str();
        let ports = match options.port {
            Some(port) => port..=port,
            None => DEFAULT_PORT..=DEFAULT_PORT + PORT_ATTEMPTS,
        };

        let mut listening = Err("no port to listen on".to_string());
        for port in ports {
            listening = tiny_http::Server::http((host, port))
                .map(|listener| (Arc::new(listener), port))
                .map_err(|error| format!("cannot listen on {host}:{port}: {error}"));
            if listening.is_ok() {
                break;
            }
        }
        let (listener, port) = listening?;

        let token = match options.token {
            Some(token) => token,
            None => random_hex(16)?,
        };
        if let Some(directory) = &options.state_directory {
            // A directory that cannot be created only costs persistence: the notebook then lives in memory.
            let _ = fs::create_dir_all(directory);
        }

        let shared = Arc::new(Shared {
            engine,
            token: token.clone(),
            stopping: AtomicBool::new(false),
            state_directory: options.state_directory,
            notebook: Mutex::new(Notebook::default()),
            downloads: Mutex::new(HashMap::new()),
        });
        let worker = || {
            let (listener, shared) = (listener.clone(), shared.clone());
            std::thread::spawn(move || serve(&listener, &shared))
        };
        let workers = (0..WORKERS).map(|_| worker()).collect();

        let host = match host {
            "0.0.0.0" | "::" => "127.0.0.1",
            host => host,
        };
        Ok(Server { url: format!("http://{host}:{port}/?token={token}"), shared, workers })
    }

    /// Whether this server runs SQL in the database that `engine` belongs to.
    pub fn serves(&self, engine: &Arc<Engine>) -> bool {
        Arc::ptr_eq(&self.shared.engine, engine)
    }

    /// Stop serving, cancel running queries and free every held result.
    pub fn stop(self) {
        self.shared.stopping.store(true, SeqCst);
        self.shared.engine.interrupt();

        for worker in self.workers {
            let _ = worker.join();
        }
        self.shared.engine.forget_all();
        for (_, file) in lock(&self.shared.downloads).drain() {
            let _ = fs::remove_file(file);
        }
    }
}

fn serve(listener: &tiny_http::Server, shared: &Shared) {
    while !shared.stopping.load(SeqCst) {
        match listener.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(mut request)) => {
                let response = respond(shared, &mut request);
                let _ = request.respond(response);
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut buffer = vec![0u8; bytes];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut buffer))
        .map_err(|error| format!("cannot read random bytes: {error}"))?;
    Ok(buffer.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// A failed request: the HTTP status and what to tell the application.
struct Failure(u16, String);

impl From<String> for Failure {
    /// DuckDB and the engine report what was wrong with the request.
    fn from(message: String) -> Failure {
        Failure(400, message)
    }
}

type Reply = std::result::Result<ResponseBox, Failure>;

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("header is ASCII")
}

fn json<T: Serialize>(value: &T) -> ResponseBox {
    let body = serde_json::to_vec(value).unwrap_or_default();
    Response::from_data(body)
        .with_header(header("Content-Type", "application/json"))
        .with_header(header("Cache-Control", "no-store"))
        .boxed()
}

fn respond(shared: &Shared, request: &mut tiny_http::Request) -> ResponseBox {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or_default();

    let reply = match (request.method(), path) {
        (Method::Post, "/api") => api(shared, request),
        (Method::Get, "/") => asset("index.html"),
        (Method::Get, path) => match path.strip_prefix("/download/") {
            Some(key) => download(shared, key),
            None => asset(&path[1..]),
        },
        _ => Err(Failure(404, "not found".to_string())),
    };
    reply.unwrap_or_else(|Failure(status, error)| json(&ErrorBody { error }).with_status_code(status))
}

/// Do what the application asks, if the request carries the token.
fn api(shared: &Shared, request: &mut tiny_http::Request) -> Reply {
    let given = request.headers().iter().find(|header| header.field.equiv(TOKEN_HEADER));
    let given = given.map(|header| header.value.as_str()).unwrap_or_default();

    // Compare every byte, so the time taken does not tell how many leading characters were right.
    let expected = shared.token.as_bytes();
    let difference = given.bytes().zip(expected).fold(0, |difference, (a, b)| difference | (a ^ b));
    if given.len() != expected.len() || difference != 0 {
        let message = "missing or wrong access token; open the URL that webui_start() returned";
        return Err(Failure(401, message.to_string()));
    }

    let mut body = Vec::new();
    let read = request.as_reader().take(MAX_BODY_BYTES).read_to_end(&mut body);
    read.map_err(|error| error.to_string())?;
    let asked = serde_json::from_slice(&body).map_err(|error| format!("invalid request: {error}"))?;

    let engine = &shared.engine;
    Ok(match asked {
        Request::Catalog => json(&engine.catalog()?),
        Request::Relations { database } => json(&engine.relations(&database)?),
        Request::DropRelation { database, schema, name, is_view } => {
            json(&engine.drop_relation(&database, &schema, &name, is_view)?)
        }
        Request::Detach { database } => json(&engine.detach(&database)?),

        Request::Run { sql } => json(&engine.run(&sql)),
        Request::Interrupt => json(&engine.interrupt()),
        Request::Progress => json(&engine.progress()),

        Request::Load { result, want, resume } => json(&engine.load(result, want, resume)?),
        Request::Page { result, offset, limit } => json(&engine.page(result, offset, limit)?),
        Request::Value { result, row, column, limit } => {
            json(&engine.value(result, row, column, limit.min(MAX_VALUE_CHARS))?)
        }
        Request::Reshape { result, view } => json(&engine.reshape(result, view)?),
        Request::Summarize { result, column } => json(&engine.summarize(result, column)?),
        Request::Export { result, format } => json(&export(shared, result, format)?),
        Request::Forget { result } => json(&engine.forget(result)),

        Request::Notebook => json(&load_notebook(shared)),
        Request::SaveNotebook(notebook) => json(&save_notebook(shared, notebook)),
    })
}

/// Serve one file of the embedded application.
fn asset(name: &str) -> Reply {
    let not_found = || Failure(404, "not found".to_string());
    let (_, content) = ASSETS.iter().find(|(asset, _)| *asset == name).ok_or_else(not_found)?;

    let content_type = match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        _ => "image/svg+xml",
    };
    Ok(Response::from_data(*content)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Content-Security-Policy", CONTENT_SECURITY_POLICY))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Referrer-Policy", "no-referrer"))
        .with_header(header("Cache-Control", "no-cache"))
        .boxed())
}

/// Have DuckDB write a result to a temporary file, and return the link that downloads it.
fn export(shared: &Shared, result: u64, format: ExportFormat) -> Result<String> {
    let key = random_hex(16)?;
    let file = std::env::temp_dir().join(format!("webui-export-{key}.{}", format.extension()));
    shared.engine.export(result, format, &file.to_string_lossy())?;

    lock(&shared.downloads).insert(key.clone(), file);
    Ok(format!("/download/{key}"))
}

/// Send an exported file. The key works once, and the file is removed as soon as it is opened.
fn download(shared: &Shared, key: &str) -> Reply {
    let used = || Failure(404, "this download link was already used".to_string());
    let file = lock(&shared.downloads).remove(key).ok_or_else(used)?;

    let content = File::open(&file).map_err(|error| Failure(500, error.to_string()))?;
    let _ = fs::remove_file(&file);

    let extension = file.extension().unwrap_or_default().to_string_lossy();
    let disposition = format!("attachment; filename=\"result.{extension}\"");
    Ok(Response::from_file(content)
        .with_header(header("Content-Type", "application/octet-stream"))
        .with_header(header("Content-Disposition", &disposition))
        .boxed())
}

fn notebook_file(shared: &Shared) -> Option<PathBuf> {
    shared.state_directory.as_ref().map(|directory| directory.join("notebook.json"))
}

fn load_notebook(shared: &Shared) -> Notebook {
    let saved = notebook_file(shared).and_then(|file| fs::read(file).ok());
    let saved = saved.and_then(|bytes| serde_json::from_slice(&bytes).ok());
    saved.unwrap_or_else(|| lock(&shared.notebook).clone())
}

/// Save the notebook to its file, or keep it in memory when there is none to write.
fn save_notebook(shared: &Shared, notebook: Notebook) {
    // Written beside the target and renamed over it, so a reader never sees half a file.
    let write = |file: PathBuf| {
        let temporary = file.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(&notebook).ok()?).ok()?;
        fs::rename(&temporary, &file).ok()
    };

    if notebook_file(shared).and_then(write).is_none() {
        *lock(&shared.notebook) = notebook;
    }
}
