//! Byte-oriented SQL functions backed by the same algorithms as capture.
use duckdb::{
    Connection,
    core::{DataChunkHandle, FlatVector, Inserter, LogicalTypeId},
    ffi::duckdb_string_t,
    types::DuckString,
    vscalar::{ScalarFunctionSignature, VScalar},
    vtab::arrow::WritableVector,
};
use std::error::Error;

pub(super) fn register(connection: &Connection) -> duckdb::Result<()> {
    connection.register_scalar_function::<Blake3>("llm_history_blake3")?;
    connection.register_scalar_function::<Xxh64>("llm_history_xxh64")
}

fn blob_signature(result: LogicalTypeId) -> Vec<ScalarFunctionSignature> {
    vec![ScalarFunctionSignature::exact(
        vec![LogicalTypeId::Blob.into()],
        result.into(),
    )]
}

fn hash_chunk(
    input: &DataChunkHandle,
    output: &mut FlatVector<'_>,
    mut write: impl FnMut(&FlatVector<'_>, usize, &[u8]),
) {
    let blobs = input.flat_vector(0);
    // SAFETY: The signature fixes the input type to BLOB. DuckDB's C scalar
    // bridge flattens the chunk; its BLOB elements are duckdb_string_t values.
    let data = unsafe { blobs.as_mut_ptr::<duckdb_string_t>() };
    for row in 0..input.len() {
        if blobs.row_is_null(row as u64) {
            output.set_null(row);
            continue;
        }
        // SAFETY: This row is in bounds and valid. The string descriptor and
        // its bytes stay live through the call to write.
        let mut blob = unsafe { data.add(row).read() };
        write(output, row, DuckString::new(&mut blob).as_bytes());
    }
}

struct Blake3;

impl VScalar for Blake3 {
    type State = ();

    fn signatures() -> Vec<ScalarFunctionSignature> {
        blob_signature(LogicalTypeId::Blob)
    }

    fn invoke(
        _: &(),
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        hash_chunk(input, &mut output.flat_vector(), |out, row, bytes| {
            out.insert(row, blake3::hash(bytes).as_bytes().as_slice());
        });
        Ok(())
    }
}

struct Xxh64;

impl VScalar for Xxh64 {
    type State = ();

    fn signatures() -> Vec<ScalarFunctionSignature> {
        blob_signature(LogicalTypeId::UBigint)
    }

    fn invoke(
        _: &(),
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let mut output = output.flat_vector();
        // SAFETY: The declared result type UBIGINT is stored as u64.
        let hashes = unsafe { output.as_mut_ptr::<u64>() };
        hash_chunk(input, &mut output, |_, row, bytes| {
            // SAFETY: hash_chunk only calls this for rows in the current chunk.
            // Null rows are marked invalid without reading the output buffer.
            unsafe { hashes.add(row).write(xxhash_rust::xxh64::xxh64(bytes, 0)) };
        });
        Ok(())
    }
}
