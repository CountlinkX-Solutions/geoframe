//! Attribute plumbing shared by the file formats.
//!
//! Every format splits a dataframe into a geometry column (kept in Rust) and attributes that
//! cross the NIF boundary as an Arrow IPC stream. This module holds the pieces all formats
//! need: encoding record batches to an IPC stream, decoding and validating the stream, and a
//! uniform per-cell view of the decoded columns for the row-oriented writers (GeoJSON,
//! FlatGeobuf).

use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Float32Type, Float64Type, Int16Type, Int32Type, Int64Type, Int8Type, UInt16Type, UInt32Type,
    UInt64Type, UInt8Type,
};
use arrow_array::{Array, ArrayRef, RecordBatch, RecordBatchOptions};
use arrow_cast::cast;
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use arrow_select::concat::concat_batches;

use crate::GeoColumn;

/// Result of reading a format that has no geometry column name of its own.
pub struct VectorRead {
    /// The geometry column, with the file's CRS.
    pub geometry: GeoColumn,
    /// All properties as an Arrow IPC stream; an empty-schema stream when there are none.
    pub attributes_ipc: Vec<u8>,
}

/// Encodes `batches` (all of `schema`) as an Arrow IPC stream.
pub(crate) fn ipc_from_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut out, schema).map_err(|e| e.to_string())?;
        for batch in batches {
            writer.write(batch).map_err(|e| e.to_string())?;
        }
        writer.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Encodes whole columns as one IPC stream. `rows` fixes the row count of a column-less table.
pub(crate) fn ipc_from_columns(
    fields: Vec<Field>,
    columns: Vec<ArrayRef>,
    rows: usize,
) -> Result<Vec<u8>, String> {
    let schema = Arc::new(Schema::new(fields));
    let options = RecordBatchOptions::new().with_row_count(Some(rows));
    let batch = RecordBatch::try_new_with_options(schema.clone(), columns, &options)
        .map_err(|e| e.to_string())?;
    ipc_from_batches(&schema, &[batch])
}

/// Decodes an attribute IPC stream into fields and whole columns. An empty-schema stream
/// gives empty vectors for any `rows`; otherwise the row count must equal `rows`.
pub(crate) fn decode_attributes(
    ipc: &[u8],
    rows: usize,
) -> Result<(Vec<Field>, Vec<ArrayRef>), String> {
    let reader =
        StreamReader::try_new(ipc, None).map_err(|e| format!("invalid attribute data: {e}"))?;
    let schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid attribute data: {e}"))?;
    if schema.fields().is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let table =
        concat_batches(&schema, &batches).map_err(|e| format!("invalid attribute data: {e}"))?;
    if table.num_rows() != rows {
        return Err(format!(
            "attributes have {} rows but geometry has {rows} rows",
            table.num_rows()
        ));
    }
    Ok((
        schema.fields().iter().map(|f| f.as_ref().clone()).collect(),
        table.columns().to_vec(),
    ))
}

/// One attribute value, widened: narrow integer and float types are restored by the
/// consumer from the column type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Cell<'a> {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(&'a str),
    Bytes(&'a [u8]),
}

/// Decoded attributes with every column reduced to a type [`Cell`] can express.
pub(crate) struct Table {
    pub names: Vec<String>,
    pub arrays: Vec<ArrayRef>,
}

impl Table {
    pub fn from_ipc(ipc: &[u8], rows: usize) -> Result<Self, String> {
        let (fields, columns) = decode_attributes(ipc, rows)?;
        let mut names = Vec::with_capacity(fields.len());
        let mut arrays = Vec::with_capacity(fields.len());
        for (field, column) in fields.iter().zip(columns) {
            names.push(field.name().clone());
            arrays.push(normalize(field.name(), column)?);
        }
        Ok(Self { names, arrays })
    }

    pub fn cell(&self, column: usize, row: usize) -> Cell<'_> {
        let array = &self.arrays[column];
        if array.is_null(row) {
            return Cell::Null;
        }
        match array.data_type() {
            DataType::Boolean => Cell::Bool(array.as_boolean().value(row)),
            DataType::Int8 => Cell::Int(array.as_primitive::<Int8Type>().value(row).into()),
            DataType::Int16 => Cell::Int(array.as_primitive::<Int16Type>().value(row).into()),
            DataType::Int32 => Cell::Int(array.as_primitive::<Int32Type>().value(row).into()),
            DataType::Int64 => Cell::Int(array.as_primitive::<Int64Type>().value(row)),
            DataType::UInt8 => Cell::UInt(array.as_primitive::<UInt8Type>().value(row).into()),
            DataType::UInt16 => Cell::UInt(array.as_primitive::<UInt16Type>().value(row).into()),
            DataType::UInt32 => Cell::UInt(array.as_primitive::<UInt32Type>().value(row).into()),
            DataType::UInt64 => Cell::UInt(array.as_primitive::<UInt64Type>().value(row)),
            DataType::Float32 => Cell::Float(array.as_primitive::<Float32Type>().value(row).into()),
            DataType::Float64 => Cell::Float(array.as_primitive::<Float64Type>().value(row)),
            DataType::Utf8 => Cell::Str(array.as_string::<i32>().value(row)),
            DataType::LargeUtf8 => Cell::Str(array.as_string::<i64>().value(row)),
            DataType::Utf8View => Cell::Str(array.as_string_view().value(row)),
            DataType::Binary => Cell::Bytes(array.as_binary::<i32>().value(row)),
            DataType::LargeBinary => Cell::Bytes(array.as_binary::<i64>().value(row)),
            // `normalize` admits no other type.
            _ => Cell::Null,
        }
    }
}

/// Reduces a column to a type [`Table::cell`] handles: dictionaries are decoded, dates,
/// times, timestamps, decimals and durations become ISO/decimal text, half floats widen.
/// Nested types (list, struct, map) are rejected.
fn normalize(name: &str, array: ArrayRef) -> Result<ArrayRef, String> {
    use DataType::*;
    match array.data_type() {
        Boolean | Int8 | Int16 | Int32 | Int64 | UInt8 | UInt16 | UInt32 | UInt64 | Float32
        | Float64 | Utf8 | LargeUtf8 | Utf8View | Binary | LargeBinary => Ok(array),
        Float16 => convert(name, &array, &Float64),
        Dictionary(_, value) => {
            let value = value.as_ref().clone();
            let decoded = convert(name, &array, &value)?;
            normalize(name, decoded)
        }
        Null
        | Date32
        | Date64
        | Timestamp(_, _)
        | Time32(_)
        | Time64(_)
        | Duration(_)
        | Decimal128(_, _)
        | Decimal256(_, _) => convert(name, &array, &Utf8),
        other => Err(format!(
            "column {name:?} has type {other}, which this format cannot store"
        )),
    }
}

fn convert(name: &str, array: &ArrayRef, to: &DataType) -> Result<ArrayRef, String> {
    cast(array, to).map_err(|e| format!("cannot convert column {name:?}: {e}"))
}
