//! Batched reading of shapefiles.
//!
//! The `.shp` is read record by record and the `.dbf` in chunks of the same number of
//! records, so neither file sits in memory whole. Records are decoded exactly as
//! [`crate::read_shapefile`] decodes them (`shape_to_wkb`, the `dbf` module), which makes the
//! batches concatenate to the whole read. Records flagged deleted in the `.dbf` are skipped and
//! batches are refilled, so every batch but the last has `batch_size` rows.
//!
//! `bbox` is applied per record, after decoding: a shapefile has no index the reader could use
//! (the `.shx` only locates records). A null shape never matches a bbox.
//!
//! A `.zip` cannot be streamed from the archive (the zip reader needs to own the entry while
//! it is read, and the shapefile reader needs a seekable file), so the `.shp` and `.dbf`
//! members are extracted to a scratch directory in the system temp dir, bounded by the same
//! per-member size limit as the whole read. The directory is removed when the stream is
//! dropped or closed, or when opening fails.

use std::fs::{self, File};
use std::io::{BufReader, Cursor, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use arrow_array::{Array, ArrayRef, BooleanArray};
use arrow_schema::Field;
use arrow_select::concat::concat;
use arrow_select::filter::filter;
use geoarrow_array::scalar::Geometry as Scalar;
use shapefile::ReadableShape;

use crate::attributes::ipc_from_columns;
use crate::column::ColumnBuilder;
use crate::crs_wkt::prj_wkt_to_crs;
use crate::dbf::{DbfCursor, Encoding, MAX_CELLS};
use crate::parquet_io::bbox_keep_exact;
use crate::shapefile_io::{
    is_zip, resolve_encoding, shape_to_wkb, sibling, zip_member, zip_stem, MAX_ZIP_MEMBER_BYTES,
};
use crate::{GeoColumn, VectorRead};

static SCRATCH: AtomicUsize = AtomicUsize::new(0);

/// A directory in the system temp dir, removed (with its content) when dropped.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn create() -> Result<Self, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "geoframe-shp-{}-{}-{nanos}",
            std::process::id(),
            SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Where the parts of one shapefile are.
struct Located {
    shp: PathBuf,
    dbf: Option<PathBuf>,
    prj: Option<String>,
    cpg: Option<String>,
}

fn locate_files(path: &str) -> Located {
    let shp = PathBuf::from(path);
    let text = |extension: &str| sibling(&shp, extension).and_then(|p| fs::read(p).ok());
    Located {
        dbf: sibling(&shp, "dbf"),
        prj: text("prj").map(|b| String::from_utf8_lossy(&b).into_owned()),
        cpg: text("cpg").map(|b| String::from_utf8_lossy(&b).into_owned()),
        shp,
    }
}

/// Copies one archive entry to `target`, refusing more than `cap` decompressed bytes.
fn extract_member<R: Read>(entry: R, target: &Path, cap: u64, label: &str) -> Result<(), String> {
    let mut out =
        File::create(target).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    let copied = std::io::copy(&mut entry.take(cap + 1), &mut out)
        .map_err(|e| format!("cannot extract {label}: {e}"))?;
    if copied > cap {
        return Err(format!("{label} is larger than the {cap} byte limit"));
    }
    out.flush().map_err(|e| e.to_string())
}

fn extract_zip(path: &str, scratch: &Path) -> Result<Located, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("{path} is not a readable zip file: {e}"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .map(|n| n.into_owned())
        .collect();
    let stem = zip_stem(path, &names)?;
    let cap = MAX_ZIP_MEMBER_BYTES;
    let mut extract = |extension: &str| -> Result<Option<PathBuf>, String> {
        let Some(name) = zip_member(&names, &stem, extension) else {
            return Ok(None);
        };
        let Ok(entry) = archive.by_name(name) else {
            return Ok(None);
        };
        let label = format!("{path}: member {name:?}");
        if entry.size() > cap {
            return Err(format!("{label} is larger than the {cap} byte limit"));
        }
        let target = scratch.join(format!("part.{extension}"));
        extract_member(entry, &target, cap, &label)?;
        Ok(Some(target))
    };
    let shp = extract("shp")?.ok_or_else(|| format!("cannot read the .shp in {path}"))?;
    let dbf = extract("dbf")?;
    let mut small = |extension: &str| -> Result<Option<String>, String> {
        Ok(extract(extension)?
            .and_then(|p| fs::read(p).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned()))
    };
    let prj = small("prj")?;
    let cpg = small("cpg")?;
    Ok(Located { shp, dbf, prj, cpg })
}

/// A shapefile read `batch_size` records at a time.
pub(crate) struct ShapefileStream {
    path: String,
    shp: BufReader<File>,
    /// Bytes of the `.shp` consumed, and where the shape records end (from its header).
    position: u64,
    end: u64,
    file_len: u64,
    dbf: Option<DbfCursor<BufReader<File>>>,
    crs: Option<String>,
    bbox: Option<[f64; 4]>,
    batch_size: usize,
    /// Dropped last, after the files above are closed.
    _scratch: Option<ScratchDir>,
}

impl ShapefileStream {
    pub(crate) fn open(
        path: &str,
        encoding: Option<&str>,
        bbox: Option<[f64; 4]>,
        batch_size: usize,
    ) -> Result<Self, String> {
        if batch_size == 0 {
            return Err("batch_size must be a positive number of rows".to_string());
        }
        if let Some([min_x, min_y, max_x, max_y]) = bbox {
            if !(min_x <= max_x && min_y <= max_y) {
                return Err("bbox minimum exceeds its maximum (or contains NaN)".to_string());
            }
        }
        catch_unwind(AssertUnwindSafe(|| {
            Self::open_files(path, encoding, bbox, batch_size)
        }))
        .unwrap_or_else(|_| Err(not_readable(path)))
    }

    fn open_files(
        path: &str,
        encoding: Option<&str>,
        bbox: Option<[f64; 4]>,
        batch_size: usize,
    ) -> Result<Self, String> {
        let (located, scratch) = if is_zip(path) {
            let scratch = ScratchDir::create()?;
            let located = extract_zip(path, &scratch.0)?;
            (located, Some(scratch))
        } else {
            (locate_files(path), None)
        };
        let encoding: Encoding = resolve_encoding(encoding, located.cpg.as_deref())?;
        let crs = located
            .prj
            .as_deref()
            .map(prj_wkt_to_crs)
            .filter(|c| !c.is_empty());

        let file = File::open(&located.shp).map_err(|e| format!("cannot read {path}: {e}"))?;
        let file_len = file.metadata().map_err(|e| e.to_string())?.len();
        let mut shp = BufReader::new(file);
        let header = shapefile::header::Header::read_from(&mut shp)
            .map_err(|e| format!("cannot read {path} as a shapefile: {e}"))?;
        let end = u64::try_from(header.file_length).unwrap_or(0) * 2;

        let dbf = located
            .dbf
            .map(|dbf_path| -> Result<_, String> {
                let file = File::open(&dbf_path).map_err(|e| e.to_string())?;
                let len = file.metadata().map_err(|e| e.to_string())?.len();
                DbfCursor::open(BufReader::new(file), len, encoding, MAX_CELLS)
            })
            .transpose()
            .map_err(|e| format!("cannot read the attributes of {path}: {e}"))?;
        Ok(Self {
            path: path.to_string(),
            shp,
            position: 100,
            end,
            file_len,
            dbf,
            crs,
            bbox,
            batch_size,
            _scratch: scratch,
        })
    }

    pub(crate) fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// Changes the rows per batch (used to read past rows in exact amounts).
    pub(crate) fn set_batch_size(&mut self, batch_size: usize) {
        self.batch_size = batch_size;
    }

    /// The next `batch_size` records (fewer at the end), `None` when none are left.
    pub(crate) fn next_batch(&mut self) -> Result<Option<VectorRead>, String> {
        catch_unwind(AssertUnwindSafe(|| self.next()))
            .unwrap_or_else(|_| Err(not_readable(&self.path)))
    }

    fn next(&mut self) -> Result<Option<VectorRead>, String> {
        let mut geometries: Vec<GeoColumn> = Vec::new();
        let mut pieces: Vec<Vec<ArrayRef>> = Vec::new();
        let mut kept = 0usize;
        while kept < self.batch_size && self.position < self.end {
            let (geometry, columns) = self.read_chunk(self.batch_size - kept)?;
            kept += geometry.len();
            if !geometry.is_empty() {
                geometries.push(geometry);
                pieces.push(columns);
            }
        }
        if kept == 0 {
            return Ok(None);
        }
        let refs: Vec<&GeoColumn> = geometries.iter().collect();
        let geometry = GeoColumn::concat(&refs)?;
        let fields: Vec<Field> = self.dbf.as_ref().map_or_else(Vec::new, DbfCursor::fields);
        let mut columns = Vec::with_capacity(fields.len());
        for index in 0..fields.len() {
            let parts: Vec<&dyn Array> = pieces.iter().map(|p| p[index].as_ref()).collect();
            columns.push(concat(&parts).map_err(|e| e.to_string())?);
        }
        Ok(Some(VectorRead {
            geometry,
            attributes_ipc: ipc_from_columns(fields, columns, kept)?,
        }))
    }

    /// Reads up to `count` records and returns the ones that are not deleted and match the
    /// bbox: their geometry and their attribute columns.
    fn read_chunk(&mut self, count: usize) -> Result<(GeoColumn, Vec<ArrayRef>), String> {
        let mut builder = ColumnBuilder::new(self.crs.as_deref());
        let mut shapes = 0usize;
        while shapes < count && self.position < self.end {
            let shape = self.read_shape()?;
            match shape_to_wkb(shape)? {
                Some(bytes) => {
                    let parsed = wkb::reader::read_wkb(&bytes).map_err(|e| e.to_string())?;
                    builder.push(Some(&parsed))?;
                }
                None => builder.push(None::<&Scalar<'_>>)?,
            }
            shapes += 1;
        }
        let shapes_done = self.position >= self.end;
        let (columns, deleted) = match self.dbf.as_mut() {
            Some(dbf) => {
                if dbf.remaining() < shapes || (shapes_done && dbf.remaining() > shapes) {
                    return Err(format!(
                        "{} has a different number of shapes and attribute records",
                        self.path
                    ));
                }
                dbf.read(shapes)?
            }
            None => (Vec::new(), vec![false; shapes]),
        };
        let geometry = builder.finish();
        let mut keep: Vec<bool> = deleted.iter().map(|d| !d).collect();
        if let Some(query) = self.bbox {
            for (keep, inside) in keep.iter_mut().zip(bbox_keep_exact(&geometry, query)) {
                *keep &= inside;
            }
        }
        if keep.iter().all(|k| *k) {
            return Ok((geometry, columns));
        }
        let indices: Vec<usize> = (0..keep.len()).filter(|&i| keep[i]).collect();
        let mask = BooleanArray::from(keep);
        let columns = columns
            .iter()
            .map(|c| filter(c, &mask).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((geometry.take(&indices)?, columns))
    }

    /// The next record of the `.shp`.
    fn read_shape(&mut self) -> Result<shapefile::Shape, String> {
        let path = &self.path;
        let fail = |e: &dyn std::fmt::Display| format!("cannot read {path} as a shapefile: {e}");
        let mut head = [0u8; 8];
        self.shp.read_exact(&mut head).map_err(|e| fail(&e))?;
        let words = i32::from_be_bytes([head[4], head[5], head[6], head[7]]);
        let size = usize::try_from(words)
            .ok()
            .and_then(|w| w.checked_mul(2))
            .filter(|s| (*s as u64) <= self.file_len.saturating_sub(self.position + 8))
            .ok_or_else(|| fail(&"a record has an invalid size"))?;
        let mut content = vec![0u8; size];
        self.shp.read_exact(&mut content).map_err(|e| fail(&e))?;
        let shape = shapefile::Shape::read_from(&mut Cursor::new(content), size as i32)
            .map_err(|e| fail(&e))?;
        self.position += 8 + size as u64;
        Ok(shape)
    }
}

fn not_readable(path: &str) -> String {
    format!("{path} is not a readable shapefile")
}
