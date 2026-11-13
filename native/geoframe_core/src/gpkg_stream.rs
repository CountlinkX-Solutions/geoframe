//! Batched reading of GeoPackage layers.
//!
//! A layer is read in pages of `batch_size` rows with a key cursor, `WHERE key > ?
//! ORDER BY key LIMIT ?`, where the key is the layer's integer primary key (the feature id,
//! which is also what [`crate::read_gpkg`] orders by) or the `rowid`. Each page costs one index
//! seek, so the whole layer is read in O(n), unlike `LIMIT/OFFSET`, and no statement is kept
//! open between batches.
//!
//! Column types are fixed when the reader opens: a column whose declared type does not settle
//! its Arrow type (an untyped `NUMERIC` column, or a narrow integer that holds a wider value)
//! is scanned once, so every batch has the types the whole read would give. That scan reads
//! only those columns and keeps nothing.
//!
//! `bbox` goes through the layer's R-tree (`rtree_<table>_<geometry column>`) when it has one;
//! the R-tree stores single precision bounds, so its answer is a superset and every row is then
//! checked against its exact bounds, as it is when there is no R-tree. A row without a geometry
//! never matches a bbox. `columns` limits the attribute columns that are read.

use rusqlite::types::Value;
use rusqlite::{Connection, ToSql};

use crate::column::GeoColumn;
use crate::gpkg_io::{
    build_column_as, crs_for_srs, decode_geometry_blob, geometry_column, is_fid, layer_rows,
    open_read, quote, select_layer, sql_error, table_columns, Declared, KindScan,
};
use crate::parquet_io::bbox_keep;
use crate::{attributes::ipc_from_columns, VectorRead};
use arrow_schema::Field;

/// An attribute column to read: its name, and the type every batch gives it.
struct Attribute {
    name: String,
    kind: Declared,
}

/// One row of a page: its key, its geometry blob and its attribute cells.
struct PageRow {
    key: i64,
    blob: Option<Vec<u8>>,
    cells: Vec<Value>,
}

/// A GeoPackage layer read `batch_size` rows at a time.
pub(crate) struct GpkgStream {
    conn: Connection,
    table: String,
    crs: Option<String>,
    attributes: Vec<Attribute>,
    /// The SQL of one page; `?1` is the cursor, `?2` the page size, `?3..=?6` the bbox.
    sql: String,
    bbox: Option<[f64; 4]>,
    /// Whether `sql` filters through the R-tree (and so takes the bbox as parameters).
    use_rtree: bool,
    batch_size: usize,
    /// The key of the last row consumed.
    cursor: i64,
    finished: bool,
}

impl GpkgStream {
    pub(crate) fn open(
        path: &str,
        layer: Option<&str>,
        columns: Option<&[String]>,
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
        let conn = open_read(path)?;
        let rows = layer_rows(&conn)?;
        let table = rows[select_layer(&rows, layer, true, path)?].name.clone();
        let geometry = geometry_column(&conn, &table)?;
        let crs = crs_for_srs(&conn, geometry.srs_id)?;

        let all = table_columns(&conn, &table)?;
        let fid = all.iter().find(|c| is_fid(c, &all)).map(|c| c.name.clone());
        if let Some(wanted) = columns {
            for name in wanted {
                let known = all.iter().any(|c| {
                    &c.name == name && c.name != geometry.name && Some(&c.name) != fid.as_ref()
                });
                if !known {
                    return Err(format!("column {name:?} not found in layer {table:?}"));
                }
            }
        }
        let selected: Vec<_> = all
            .iter()
            .filter(|c| c.name != geometry.name && Some(&c.name) != fid.as_ref())
            .filter(|c| columns.is_none_or(|w| w.contains(&c.name)))
            .collect();
        let kinds = settle_kinds(&conn, &table, &selected)?;
        let attributes: Vec<Attribute> = selected
            .iter()
            .zip(kinds)
            .map(|(c, kind)| Attribute {
                name: c.name.clone(),
                kind,
            })
            .collect();

        let key = fid.as_deref().map_or_else(|| "rowid".to_string(), quote);
        let select = [key.clone(), quote(&geometry.name)]
            .into_iter()
            .chain(attributes.iter().map(|a| quote(&a.name)))
            .collect::<Vec<_>>()
            .join(", ");
        let rtree = format!("rtree_{table}_{}", geometry.name);
        let use_rtree = bbox.is_some() && has_table(&conn, &rtree)?;
        let within = if use_rtree {
            format!(
                " AND {key} IN (SELECT id FROM {} WHERE minx <= ?5 AND maxx >= ?3 \
                 AND miny <= ?6 AND maxy >= ?4)",
                quote(&rtree)
            )
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT {select} FROM {} WHERE {key} > ?1{within} ORDER BY {key} LIMIT ?2",
            quote(&table)
        );
        // Prepare once so a bad statement fails on open.
        conn.prepare_cached(&sql).map_err(sql_error)?;
        Ok(Self {
            conn,
            table,
            crs,
            attributes,
            sql,
            bbox,
            use_rtree,
            batch_size,
            cursor: i64::MIN,
            finished: false,
        })
    }

    pub(crate) fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// Changes the rows per batch (used to read past rows in exact amounts).
    pub(crate) fn set_batch_size(&mut self, batch_size: usize) {
        self.batch_size = batch_size;
    }

    /// The next `batch_size` rows (fewer at the end), `None` when none are left.
    pub(crate) fn next_batch(&mut self) -> Result<Option<VectorRead>, String> {
        let mut wkb_rows: Vec<Option<Vec<u8>>> = Vec::new();
        let mut cells: Vec<Vec<Value>> = vec![Vec::new(); self.attributes.len()];
        while wkb_rows.len() < self.batch_size && !self.finished {
            let page = self.fetch_page()?;
            let full = page.len() == self.batch_size;
            let decoded = self.decode_page(&page)?;
            let mut consumed_all = true;
            for (row, wkb) in page.into_iter().zip(decoded) {
                if wkb_rows.len() == self.batch_size {
                    consumed_all = false;
                    break;
                }
                self.cursor = row.key;
                if let Some(keep) = wkb.1 {
                    if !keep {
                        continue;
                    }
                }
                wkb_rows.push(wkb.0);
                for (column, value) in cells.iter_mut().zip(row.cells) {
                    column.push(value);
                }
            }
            if consumed_all && !full {
                self.finished = true;
            }
        }
        if wkb_rows.is_empty() {
            return Ok(None);
        }
        let refs: Vec<Option<&[u8]>> = wkb_rows.iter().map(|w| w.as_deref()).collect();
        let geometry = GeoColumn::from_wkb(&refs, self.crs.as_deref())
            .map_err(|e| format!("layer {:?}: {e}", self.table))?;
        let mut fields = Vec::with_capacity(cells.len());
        let mut arrays = Vec::with_capacity(cells.len());
        for (attribute, values) in self.attributes.iter().zip(&cells) {
            let array = build_column_as(attribute.kind, values)?;
            fields.push(Field::new(&attribute.name, array.data_type().clone(), true));
            arrays.push(array);
        }
        Ok(Some(VectorRead {
            geometry,
            attributes_ipc: ipc_from_columns(fields, arrays, wkb_rows.len())?,
        }))
    }

    /// Up to `batch_size` rows after the cursor.
    fn fetch_page(&self) -> Result<Vec<PageRow>, String> {
        let mut statement = self.conn.prepare_cached(&self.sql).map_err(sql_error)?;
        let limit = i64::try_from(self.batch_size).unwrap_or(i64::MAX);
        let bounds = self.bbox.unwrap_or([0.0; 4]);
        let mut params: Vec<&dyn ToSql> = vec![&self.cursor, &limit];
        if self.use_rtree {
            params.extend(bounds.iter().map(|b| b as &dyn ToSql));
        }
        let mut query = statement.query(params.as_slice()).map_err(sql_error)?;
        let mut page = Vec::new();
        while let Some(row) = query.next().map_err(sql_error)? {
            let mut cells = Vec::with_capacity(self.attributes.len());
            for index in 0..self.attributes.len() {
                cells.push(row.get::<_, Value>(index + 2).map_err(sql_error)?);
            }
            page.push(PageRow {
                key: row.get(0).map_err(sql_error)?,
                blob: row.get(1).map_err(sql_error)?,
                cells,
            });
        }
        Ok(page)
    }

    /// The WKB of every row of the page (`None` for no or an empty geometry) and, when there
    /// is a bbox, whether the row's exact bounds intersect it.
    #[allow(clippy::type_complexity)]
    fn decode_page(
        &self,
        page: &[PageRow],
    ) -> Result<Vec<(Option<Vec<u8>>, Option<bool>)>, String> {
        let mut wkb: Vec<Option<Vec<u8>>> = Vec::with_capacity(page.len());
        for row in page {
            wkb.push(match &row.blob {
                None => None,
                Some(blob) => {
                    let decoded = decode_geometry_blob(blob)
                        .map_err(|e| format!("row {} of layer {:?}: {e}", row.key, self.table))?;
                    (!decoded.empty).then(|| decoded.wkb.to_vec())
                }
            });
        }
        let keep: Vec<Option<bool>> = match self.bbox {
            None => vec![None; wkb.len()],
            Some(query) => {
                let refs: Vec<Option<&[u8]>> = wkb.iter().map(|w| w.as_deref()).collect();
                let column = GeoColumn::from_wkb(&refs, None)
                    .map_err(|e| format!("layer {:?}: {e}", self.table))?;
                bbox_keep(&column, query).into_iter().map(Some).collect()
            }
        };
        Ok(wkb.into_iter().zip(keep).collect())
    }
}

fn has_table(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(sql_error)
}

/// The type each selected column is read as: its declared type, widened or settled from the
/// cells when the declaration alone does not decide (one pass over just those columns).
fn settle_kinds(
    conn: &Connection,
    table: &str,
    columns: &[&crate::gpkg_io::Column],
) -> Result<Vec<Declared>, String> {
    let mut scans: Vec<KindScan> = columns.iter().map(|c| KindScan::new(&c.declared)).collect();
    let scanned: Vec<usize> = (0..scans.len())
        .filter(|&i| scans[i].needs_scan())
        .collect();
    if !scanned.is_empty() {
        let select = scanned
            .iter()
            .map(|&i| quote(&columns[i].name))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = conn
            .prepare(&format!("SELECT {select} FROM {}", quote(table)))
            .map_err(sql_error)?;
        let mut query = statement.query([]).map_err(sql_error)?;
        while let Some(row) = query.next().map_err(sql_error)? {
            for (position, &index) in scanned.iter().enumerate() {
                scans[index].add(&row.get::<_, Value>(position).map_err(sql_error)?);
            }
        }
    }
    Ok(scans.iter().map(KindScan::finish).collect())
}
