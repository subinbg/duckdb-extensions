//! SQL table functions that take named parameters and return one row with one text column, `url`,
//! such as `CALL webui_start(port := 4213)`.

use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Weak};

use libduckdb_sys as ffi;

use crate::duck::{Conn, Result};
use crate::engine::Engine;

const COLUMN: &CStr = c"url";

/// What a function does: `engine` belongs to the database it is called in, and the returned text is its one value.
pub type Call = fn(engine: &Arc<Engine>, parameters: &Parameters) -> Result<Option<String>>;

/// The named parameters of one call, as text; a parameter the caller left out is absent.
pub struct Parameters(Vec<(&'static str, String)>);

impl Parameters {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(parameter, _)| *parameter == name).map(|(_, value)| value.as_str())
    }
}

struct Function {
    name: &'static str,
    parameters: &'static [&'static str],
    call: Call,
    /// Not a strong reference: the function lives as long as the database, which the engine's connections keep open.
    engine: Weak<Engine>,
}

/// Register `name(parameters := ...)` in the database of `conn`.
pub fn register(
    conn: &Conn,
    engine: &Arc<Engine>,
    name: &'static str,
    parameters: &'static [&'static str],
    call: Call,
) -> Result<()> {
    let c_name = CString::new(name).unwrap();
    let function = Box::new(Function { name, parameters, call, engine: Arc::downgrade(engine) });

    let state = unsafe {
        let mut text = ffi::duckdb_create_logical_type(ffi::DUCKDB_TYPE_DUCKDB_TYPE_VARCHAR);
        let mut table_function = ffi::duckdb_create_table_function();
        ffi::duckdb_table_function_set_name(table_function, c_name.as_ptr());

        // Every parameter is text, which DuckDB casts a number or a boolean to.
        for parameter in parameters {
            let parameter = CString::new(*parameter).unwrap();
            ffi::duckdb_table_function_add_named_parameter(table_function, parameter.as_ptr(), text);
        }

        let function = Box::into_raw(function) as *mut c_void;
        ffi::duckdb_table_function_set_extra_info(table_function, function, Some(drop_box::<Function>));
        ffi::duckdb_table_function_set_bind(table_function, Some(bind));
        ffi::duckdb_table_function_set_init(table_function, Some(init));
        ffi::duckdb_table_function_set_function(table_function, Some(run));

        let state = ffi::duckdb_register_table_function(conn.raw(), table_function);
        ffi::duckdb_destroy_table_function(&mut table_function);
        ffi::duckdb_destroy_logical_type(&mut text);
        state
    };

    match state == ffi::duckdb_state_DuckDBSuccess {
        true => Ok(()),
        false => Err(format!("could not register the table function {name}")),
    }
}

unsafe extern "C" fn drop_box<T>(data: *mut c_void) {
    drop(unsafe { Box::from_raw(data as *mut T) });
}

/// Declares the result column and copies the named parameters for [`run`].
unsafe extern "C" fn bind(info: ffi::duckdb_bind_info) {
    unsafe {
        let function = &*(ffi::duckdb_bind_get_extra_info(info) as *const Function);

        let mut text = ffi::duckdb_create_logical_type(ffi::DUCKDB_TYPE_DUCKDB_TYPE_VARCHAR);
        ffi::duckdb_bind_add_result_column(info, COLUMN.as_ptr(), text);
        ffi::duckdb_destroy_logical_type(&mut text);

        let mut parameters = Vec::new();
        for parameter in function.parameters {
            let name = CString::new(*parameter).unwrap();
            let mut value = ffi::duckdb_bind_get_named_parameter(info, name.as_ptr());

            if !value.is_null() && !ffi::duckdb_is_null_value(value) {
                let given = ffi::duckdb_get_varchar(value);
                parameters.push((*parameter, CStr::from_ptr(given).to_string_lossy().into_owned()));
                ffi::duckdb_free(given as *mut c_void);
            }
            ffi::duckdb_destroy_value(&mut value);
        }

        let parameters = Box::into_raw(Box::new(Parameters(parameters))) as *mut c_void;
        ffi::duckdb_bind_set_bind_data(info, parameters, Some(drop_box::<Parameters>));
    }
}

/// Remembers whether the row was already returned: DuckDB keeps calling [`run`] until it returns none.
unsafe extern "C" fn init(info: ffi::duckdb_init_info) {
    let done = Box::into_raw(Box::new(AtomicBool::new(false))) as *mut c_void;
    unsafe { ffi::duckdb_init_set_init_data(info, done, Some(drop_box::<AtomicBool>)) };
}

unsafe extern "C" fn run(info: ffi::duckdb_function_info, output: ffi::duckdb_data_chunk) {
    let done = unsafe { &*(ffi::duckdb_function_get_init_data(info) as *const AtomicBool) };
    if done.swap(true, SeqCst) {
        return unsafe { ffi::duckdb_data_chunk_set_size(output, 0) };
    }

    let function = unsafe { &*(ffi::duckdb_function_get_extra_info(info) as *const Function) };
    let parameters = unsafe { &*(ffi::duckdb_function_get_bind_data(info) as *const Parameters) };

    // A panic must not unwind into DuckDB.
    let call = || (function.call)(&function.engine.upgrade().ok_or("the database was closed")?, parameters);
    let returned = catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|_| Err(format!("{} panicked", function.name)));

    unsafe {
        let vector = ffi::duckdb_data_chunk_get_vector(output, 0);
        match returned {
            Ok(Some(text)) => {
                let length = text.len() as u64;
                ffi::duckdb_vector_assign_string_element_len(vector, 0, text.as_ptr() as *const c_char, length);
            }
            Ok(None) => {
                ffi::duckdb_vector_ensure_validity_writable(vector);
                ffi::duckdb_validity_set_row_invalid(ffi::duckdb_vector_get_validity(vector), 0);
            }
            Err(error) => {
                let error = CString::new(error.replace('\0', " ")).unwrap();
                return ffi::duckdb_function_set_error(info, error.as_ptr());
            }
        }
        ffi::duckdb_data_chunk_set_size(output, 1);
    }
}
