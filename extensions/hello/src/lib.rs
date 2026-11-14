//! `hello`: a minimal DuckDB extension with one scalar function.
//!
//! ```sql
//! SELECT hello('duck');  -- 'Hello, duck!'
//! SELECT hello(NULL);    -- NULL
//! ```

use std::error::Error;

use duckdb::core::{DataChunkHandle, Inserter, LogicalTypeId};
use duckdb::types::DuckString;
use duckdb::vscalar::{ScalarFunctionSignature, VScalar};
use duckdb::vtab::arrow::WritableVector;
use duckdb::{Connection, duckdb_entrypoint_c_api, ffi};

struct Hello;

impl VScalar for Hello {
    type State = ();

    /// Runs once per input chunk (up to 2048 rows): one output value per input row.
    fn invoke(
        _state: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let names = input.flat_vector(0);
        let mut greetings = output.flat_vector();

        // The column is VARCHAR, as `signatures` declares, and holds `input.len()` rows.
        let texts = unsafe { names.as_slice_with_len::<ffi::duckdb_string_t>(input.len()) };

        for (row, text) in texts.iter().enumerate() {
            if names.row_is_null(row as u64) {
                greetings.set_null(row);
                continue;
            }

            let name = DuckString::new(&mut { *text }).as_str().to_string();
            greetings.insert(row, format!("Hello, {name}!").as_str());
        }
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(vec![LogicalTypeId::Varchar.into()], LogicalTypeId::Varchar.into())]
    }
}

/// Entry point DuckDB calls on `LOAD hello` (exported as `hello_init_c_api`).
///
/// `min_duckdb_version` is the stable C API version asked for; `capi-version` in `Cargo.toml` repeats it.
#[duckdb_entrypoint_c_api(ext_name = "hello", min_duckdb_version = "v1.5.6")]
pub unsafe fn entrypoint(connection: Connection) -> Result<(), Box<dyn Error>> {
    connection.register_scalar_function::<Hello>("hello")?;
    Ok(())
}
