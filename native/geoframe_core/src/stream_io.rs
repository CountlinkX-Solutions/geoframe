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

use crate::feather_stream::FeatherStream;
use crate::flatgeobuf_io::FgbStream;
use crate::geojson_stream::GeojsonStream;
use crate::gpkg_stream::GpkgStream;
use crate::parquet_io::ParquetStream;
use crate::shapefile_stream::ShapefileStream;
use crate::GeoColumn;

static OPEN_READERS: AtomicUsize = AtomicUsize::new(0);

/// How many [`BatchReader`]s are alive in this process.
pub fn open_batch_readers() -> usize {
    OPEN_READERS.load(Ordering::SeqCst)
}

/// The most rows read at once when rows are dropped by reading them.
const SKIP_CHUNK: usize = 65_536;

/// Reads and drops `count` rows from `stream` in chunks (batch sizes changed to match, so no
/// row past `count` is consumed), then restores its batch size.
macro_rules! discard_batches {
    ($stream:expr, $count:expr) => {{
        let original = $stream.batch_size();
        let mut left: usize = $count;
        let mut outcome: Result<(), String> = Ok(());
        while left > 0 {
            $stream.set_batch_size(left.min(SKIP_CHUNK));
            match $stream.next_batch() {
                Ok(Some(read)) => left -= read.geometry.len(),
                Ok(None) => break,
                Err(e) => {
                    outcome = Err(e);
                    break;
                }
            }
        }
        $stream.set_batch_size(original);
        outcome
    }};
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
    Feather(Box<FeatherStream>),
    Shapefile(Box<ShapefileStream>),
    GeoPackage(Box<GpkgStream>),
    GeoJson(Box<GeojsonStream>),
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

    /// Opens a Feather v2 / Arrow IPC file; `columns` and `geometry` are as in `read_feather`.
    /// `batch_size` is the number of rows per batch, whatever the file's record batch size.
    pub fn open_feather(
        path: &str,
        columns: Option<&[String]>,
        geometry: Option<&str>,
        batch_size: usize,
    ) -> Result<Self, String> {
        let stream = FeatherStream::open(path, columns, geometry, batch_size)?;
        Ok(Self::counted(Source::Feather(Box::new(stream))))
    }

    /// Opens a shapefile (`.shp`, or a `.zip` holding one); `encoding` is as in
    /// `read_shapefile`. `bbox` keeps the records whose geometry bounding box intersects it.
    pub fn open_shapefile(
        path: &str,
        encoding: Option<&str>,
        bbox: Option<[f64; 4]>,
        batch_size: usize,
    ) -> Result<Self, String> {
        let stream = ShapefileStream::open(path, encoding, bbox, batch_size)?;
        Ok(Self::counted(Source::Shapefile(Box::new(stream))))
    }

    /// Opens one layer of a GeoPackage (`layer` as in `read_gpkg`). `columns` restricts the
    /// attribute columns; `bbox` keeps the rows whose geometry bounding box intersects it, through
    /// the layer's R-tree when it has one.
    pub fn open_gpkg(
        path: &str,
        layer: Option<&str>,
        columns: Option<&[String]>,
        bbox: Option<[f64; 4]>,
        batch_size: usize,
    ) -> Result<Self, String> {
        let stream = GpkgStream::open(path, layer, columns, bbox, batch_size)?;
        Ok(Self::counted(Source::GeoPackage(Box::new(stream))))
    }

    /// Opens a GeoJSON file (a `FeatureCollection` read feature by feature; a lone `Feature` or
    /// geometry is a one-row file). `bbox` keeps the features whose geometry bounding box
    /// intersects it.
    pub fn open_geojson(
        path: &str,
        bbox: Option<[f64; 4]>,
        batch_size: usize,
    ) -> Result<Self, String> {
        let stream = GeojsonStream::open(path, bbox, batch_size)?;
        Ok(Self::counted(Source::GeoJson(Box::new(stream))))
    }

    /// Drops the first `count` rows of what the reader yields (after any `bbox`), so the next
    /// batch starts at row `count`. GeoParquet skips whole row groups and pages without
    /// reading them (Feather whole record batches, FlatGeobuf unfiltered features, GeoJSON
    /// features, undecoded); a bbox the format cannot filter itself, Shapefile and GeoPackage
    /// read past the rows in chunks of at most [`SKIP_CHUNK`]. Past the end of the file the
    /// reader is simply exhausted. Call it before the first `next_batch`.
    pub fn skip_rows(&mut self, count: usize) -> Result<(), String> {
        if count == 0 {
            return Ok(());
        }
        match &mut self.source {
            Source::GeoParquet(stream) => {
                if !stream.skip(count)? {
                    discard_batches!(stream, count)?;
                }
            }
            Source::FlatGeobuf(stream) => {
                if !stream.skip(count)? {
                    discard_batches!(stream, count)?;
                }
            }
            Source::Feather(stream) => stream.skip(count)?,
            Source::GeoJson(stream) => {
                stream.skip(count)?;
            }
            Source::Shapefile(stream) => discard_batches!(stream, count)?,
            Source::GeoPackage(stream) => discard_batches!(stream, count)?,
        }
        Ok(())
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
            Source::Feather(stream) => Ok(stream.next_batch()?.map(|read| StreamBatch {
                geometry: read.geometry,
                geometry_column: Some(read.geometry_column),
                secondary_geometries: read.secondary_geometries,
                attributes_ipc: read.attributes_ipc,
            })),
            Source::Shapefile(stream) => Ok(stream.next_batch()?.map(|read| StreamBatch {
                geometry: read.geometry,
                geometry_column: None,
                secondary_geometries: Vec::new(),
                attributes_ipc: read.attributes_ipc,
            })),
            Source::GeoPackage(stream) => Ok(stream.next_batch()?.map(|read| StreamBatch {
                geometry: read.geometry,
                geometry_column: None,
                secondary_geometries: Vec::new(),
                attributes_ipc: read.attributes_ipc,
            })),
            Source::GeoJson(stream) => Ok(stream.next_batch()?.map(|read| StreamBatch {
                geometry: read.geometry,
                geometry_column: None,
                secondary_geometries: Vec::new(),
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
