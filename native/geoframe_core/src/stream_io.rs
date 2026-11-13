//! Batched (streaming) readers.
//!
//! A [`BatchReader`] opens a file once and then hands out its rows `batch_size` at a time, so
//! a file larger than memory can be processed. Each batch has the shape of the whole read
//! (primary geometry with the file's CRS, secondary geometries, an attribute IPC stream), and
//! concatenating the batches gives exactly what `read_geoparquet_filtered` or
//! `read_flatgeobuf` return.
//!
//! The file handle lives until the reader is dropped (or the caller drops it early); every
//! live reader is counted so tests can observe that one was released.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::flatgeobuf_io::FgbStream;
use crate::parquet_io::ParquetStream;
use crate::GeoColumn;

static OPEN_READERS: AtomicUsize = AtomicUsize::new(0);

/// How many [`BatchReader`]s are alive in this process.
pub fn open_batch_readers() -> usize {
    OPEN_READERS.load(Ordering::SeqCst)
}

/// One batch of a stream.
pub struct StreamBatch {
    /// The primary geometry column, with the file's CRS.
    pub geometry: GeoColumn,
    /// The primary geometry column's name when the file stores one (GeoParquet); `None` when
    /// the caller must pick it (FlatGeobuf).
    pub geometry_column: Option<String>,
    /// The other geometry columns of the file (GeoParquet), `(name, column)`.
    pub secondary_geometries: Vec<(String, GeoColumn)>,
    /// The attribute columns as an Arrow IPC stream.
    pub attributes_ipc: Vec<u8>,
}

enum Source {
    GeoParquet(Box<ParquetStream>),
    FlatGeobuf(Box<FgbStream>),
}

/// An open file read batch by batch.
pub struct BatchReader {
    source: Source,
}

impl BatchReader {
    fn counted(source: Source) -> Self {
        OPEN_READERS.fetch_add(1, Ordering::SeqCst);
        Self { source }
    }

    /// Opens a GeoParquet file; `columns` and `bbox` are as in `read_geoparquet_filtered`.
    /// `batch_size` is the number of rows per batch, whatever the row group size.
    pub fn open_geoparquet(
        path: &str,
        columns: Option<&[String]>,
        bbox: Option<[f64; 4]>,
        batch_size: usize,
    ) -> Result<Self, String> {
        let stream = ParquetStream::open(path, columns, bbox, batch_size)?;
        Ok(Self::counted(Source::GeoParquet(Box::new(stream))))
    }

    /// Opens a FlatGeobuf file; `bbox` is answered by the packed R-tree index, as in
    /// `read_flatgeobuf` (an error without one).
    pub fn open_flatgeobuf(
        path: &str,
        bbox: Option<[f64; 4]>,
        batch_size: usize,
    ) -> Result<Self, String> {
        let stream = FgbStream::open(path, bbox, batch_size)?;
        Ok(Self::counted(Source::FlatGeobuf(Box::new(stream))))
    }

    /// The next batch, `None` once the file is exhausted. A batch has `batch_size` rows except
    /// the last one.
    pub fn next_batch(&mut self) -> Result<Option<StreamBatch>, String> {
        match &mut self.source {
            Source::GeoParquet(stream) => Ok(stream.next_batch()?.map(|read| StreamBatch {
                geometry: read.geometry,
                geometry_column: Some(read.geometry_column),
                secondary_geometries: read.secondary_geometries,
                attributes_ipc: read.attributes_ipc,
            })),
            Source::FlatGeobuf(stream) => Ok(stream.next_batch()?.map(|read| StreamBatch {
                geometry: read.geometry,
                geometry_column: None,
                secondary_geometries: Vec::new(),
                attributes_ipc: read.attributes_ipc,
            })),
        }
    }
}

impl Drop for BatchReader {
    fn drop(&mut self) {
        OPEN_READERS.fetch_sub(1, Ordering::SeqCst);
    }
}
