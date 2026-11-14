//! `webui`: a browser UI for the DuckDB instance that loads it, served from inside the process with no network.
//!
//! ```sql
//! CALL webui_start();   -- the URL to open in a browser
//! CALL webui_status();  -- that URL again, or NULL when nothing is served
//! CALL webui_stop();
//! ```
//!
//! | Directory | What it is                                                                                         |
//! |-----------|----------------------------------------------------------------------------------------------------|
//! | `src/`    | This library: `server` answers HTTP, `engine` runs SQL through `duck`, `protocol` is what they say. |
//! | `app/`    | The browser application: plain files, which `server` includes and serves as they are written.       |
//!
//! The extension is built against DuckDB's stable C API (v1) because it needs connections of its own,
//! and the v2 API that `duckdb-neo` wraps gives an extension none.
//! The database handle is lent only while the extension loads, so every connection is opened then.
//!
//! A process may load the extension into several databases; each gets its own [`Engine`].
//! One server runs per process, for the database `webui_start()` was called in.

mod duck;
mod engine;
mod functions;
mod protocol;
mod server;
mod sql;

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use libduckdb_sys as ffi;

use duck::{Conn, Result};
use engine::Engine;
use functions::Parameters;
use server::{Options, Server};

/// The stable C API version this extension is built against; DuckDB 1.5.6 and later provide it.
const C_API_VERSION: &str = "v1.5.6";

static SERVER: Mutex<Option<Server>> = Mutex::new(None);

/// Lock a mutex, ignoring that a thread panicked while holding it: the state it guards stays usable.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `CALL webui_start(port := 4213, host := '127.0.0.1', token := '...', state := '~/dir', open := true)`
///
/// Starts the server unless it runs already, and returns the URL to open.
fn start(engine: &Arc<Engine>, parameters: &Parameters) -> Result<Option<String>> {
    let mut server = lock(&SERVER);
    if server.as_ref().is_some_and(|running| !running.serves(engine)) {
        return Err("webui is serving another database of this process; CALL webui_stop() first".to_string());
    }

    if server.is_none() {
        let port = parameters.get("port").map(|port| port.parse::<u16>());
        let port = port.transpose().map_err(|_| "port must be a number between 0 and 65535")?;

        // The notebook is saved under the home directory unless told otherwise; without either, in memory.
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
        let default_state = home.map(|home| PathBuf::from(home).join(".duckdb/extension_data/webui"));

        let options = Options {
            host: parameters.get("host").unwrap_or("127.0.0.1").to_string(),
            port,
            token: parameters.get("token").map(str::to_string),
            state_directory: parameters.get("state").map(PathBuf::from).or(default_state),
        };
        *server = Some(Server::start(engine.clone(), options)?);
    }

    let url = server.as_ref().map(|running| running.url.clone());
    if let (Some(url), Some("true")) = (&url, parameters.get("open")) {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(opener).arg(url).spawn();
    }
    Ok(url)
}

/// `CALL webui_stop()` returns the URL that is no longer served, or NULL when nothing was.
fn stop(_engine: &Arc<Engine>, _parameters: &Parameters) -> Result<Option<String>> {
    let running = lock(&SERVER).take();
    Ok(running.map(|running| {
        let url = running.url.clone();
        running.stop();
        url
    }))
}

/// `CALL webui_status()` returns the URL being served, or NULL when nothing is.
fn status(_engine: &Arc<Engine>, _parameters: &Parameters) -> Result<Option<String>> {
    Ok(lock(&SERVER).as_ref().map(|running| running.url.clone()))
}

unsafe fn initialise(info: ffi::duckdb_extension_info, access: *const ffi::duckdb_extension_access) -> Result<()> {
    if !unsafe { ffi::duckdb_rs_extension_api_init(info, access, C_API_VERSION) }.map_err(str::to_string)? {
        // DuckDB does not provide this API version and has recorded the reason.
        return Ok(());
    }

    let no_database = "DuckDB gave no access to the database";
    let database = unsafe { (*access).get_database.ok_or(no_database)?(info) };
    if database.is_null() {
        return Err(no_database.to_string());
    }

    // The database handle is valid only during this call, so every connection is opened now.
    let connect = || unsafe { Conn::open(*database) };
    let engine = Arc::new(Engine::new(connect()?, connect()?, connect()?, connect()?));

    // Functions are registered for the whole database; this connection only gives access to it.
    let registrar = connect()?;
    functions::register(&registrar, &engine, "webui_start", &["port", "host", "token", "state", "open"], start)?;
    functions::register(&registrar, &engine, "webui_stop", &[], stop)?;
    functions::register(&registrar, &engine, "webui_status", &[], status)?;

    std::thread::spawn(move || release_when_unused(engine));
    Ok(())
}

/// Own the engine for as long as its database has other connections, then stop serving it and drop it.
///
/// The engine's connections would otherwise keep the database open after its last user closed theirs,
/// and opening the same file again in this process would wait for it forever.
fn release_when_unused(engine: Arc<Engine>) {
    // A count that cannot be read keeps the engine: its database may be in use.
    while engine.has_other_connections().unwrap_or(true) {
        std::thread::sleep(Duration::from_millis(250));
    }

    let serving = lock(&SERVER).take_if(|running| running.serves(&engine));
    if let Some(serving) = serving {
        serving.stop();
    }
}

/// Entry point DuckDB calls on `LOAD webui`.
///
/// # Safety
/// Only DuckDB may call this, with its own arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn webui_init_c_api(
    info: ffi::duckdb_extension_info,
    access: *const ffi::duckdb_extension_access,
) -> bool {
    let Err(error) = (unsafe { initialise(info, access) }) else {
        return true;
    };

    let error = CString::new(error.replace('\0', " ")).unwrap();
    if let Some(set_error) = unsafe { (*access).set_error } {
        unsafe { set_error(info, error.as_ptr()) };
    }
    false
}
