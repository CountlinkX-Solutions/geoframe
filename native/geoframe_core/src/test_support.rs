//! Helpers shared by the file-format tests.

use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch};
use arrow_ipc::reader::StreamReader;
use arrow_schema::{DataType, Field, Schema};
use tempfile::TempDir;

use crate::attributes::ipc_from_batches;
use crate::GeoColumn;

/// An IPC stream of the given `(name, array)` columns.
pub fn ipc(columns: Vec<(&str, ArrayRef)>) -> Vec<u8> {
    let fields: Vec<Field> = columns
        .iter()
        .map(|(name, array)| Field::new(*name, array.data_type().clone(), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let arrays: Vec<ArrayRef> = columns.into_iter().map(|(_, a)| a).collect();
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    ipc_from_batches(&schema, &[batch]).unwrap()
}

/// Decodes an attribute stream into `(name, type, array)` triples.
pub fn columns(bytes: &[u8]) -> Vec<(String, DataType, ArrayRef)> {
    let reader = StreamReader::try_new(bytes, None).unwrap();
    let schema = reader.schema();
    let batches: Vec<RecordBatch> = reader.map(|b| b.unwrap()).collect();
    let all = arrow_select::concat::concat_batches(&schema, &batches).unwrap();
    schema
        .fields()
        .iter()
        .zip(all.columns())
        .map(|(f, c)| (f.name().clone(), f.data_type().clone(), c.clone()))
        .collect()
}

pub fn column(bytes: &[u8], name: &str) -> (DataType, ArrayRef) {
    columns(bytes)
        .into_iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, t, a)| (t, a))
        .unwrap_or_else(|| panic!("no column {name}"))
}

pub fn names(bytes: &[u8]) -> Vec<String> {
    columns(bytes).into_iter().map(|(n, _, _)| n).collect()
}

pub fn path(dir: &TempDir, name: &str) -> String {
    dir.path().join(name).to_str().unwrap().to_string()
}

pub fn wkt(column: &GeoColumn) -> Vec<Option<String>> {
    column.to_wkt().unwrap()
}
