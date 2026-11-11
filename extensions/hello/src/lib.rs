//! `hello`: a minimal DuckDB extension with one scalar function.
//!
//! ```sql
//! SELECT hello('duck');  -- 'Hello, duck!'
//! SELECT hello(NULL);    -- NULL
//! ```

use duckdb_neo::{
    DuckDBType, Result,
    connection::Context,
    data_chunk::VectorCollection,
    duckdb_entrypoint_c_api_v2,
    extension::Extension,
    scalar::{ScalarCallbacks, ScalarFunctionBuilder},
    signature::{FunctionParameter, SignatureBuilder},
    vector::{Unknown, Vector},
};

struct Hello;

impl ScalarCallbacks for Hello {
    type BindData = ();
    type InitData = ();

    /// Runs once per input chunk (up to 2048 rows): one output value per input row.
    fn exec(
        &self,
        _bind_data: Option<&Self::BindData>,
        _init_data: Option<&mut Self::InitData>,
        _context: &Context,
        vectors: &mut VectorCollection,
        output: Vector<'_, Unknown>,
    ) -> Result<()> {
        let mut output = output.cast::<String>()?;
        output.set_size(vectors.row_count()?)?;

        let input = vectors.get_vector_at::<String>(0)?;
        let reader = input.reader()?;
        let mut writer = output.writer()?;

        for (row, name) in reader.iter().enumerate() {
            match name {
                Some(name) => writer.set(row, Some(&format!("Hello, {name}!")))?,
                None => writer.set(row, None)?,
            }
        }
        Ok(())
    }
}

/// Entry point DuckDB calls on `LOAD hello` (exported as `hello_init_c_api_v2`).
#[duckdb_entrypoint_c_api_v2(extension_name = "hello", api_version = "2.0.0")]
pub fn entrypoint(extension: &Extension) -> Result<()> {
    ScalarFunctionBuilder::new(
        "hello",
        SignatureBuilder::new()
            .param(FunctionParameter::new::<String>("name"))
            .return_type(String::logical_type(extension)?),
        Hello,
    )
    .register(extension)
}
