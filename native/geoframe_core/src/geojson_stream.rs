//! Batched reading of GeoJSON.
//!
//! A `FeatureCollection` is read feature by feature: a byte scanner finds the top-level
//! `features` array and cuts out one feature object at a time (tracking string and nesting
//! state, so braces inside strings and foreign members in any order are fine), and each
//! feature is parsed on its own. Memory is one batch of features, whatever the file size.
//!
//! The whole read infers each property's type over every feature, which a stream that has
//! only seen the first batches cannot do. So the file is read **twice**: a first pass over all
//! features (O(n) time, O(1) memory: it keeps only the property names in first-seen order, a
//! type per property and whether every feature has an `id`) fixes the schema, and the second
//! pass produces the batches. Every batch then has the columns and dtypes
//! [`crate::read_geojson`] gives the whole file, including a column that only a late feature
//! has (all null in earlier batches) and a type decided by a late feature. The first pass also
//! surfaces a malformed file when the reader is opened rather than part way through the
//! stream. `bbox` does not narrow the first pass (the whole read has no bbox, and GDAL infers
//! the schema over the whole layer).
//!
//! A lone `Feature` or a bare geometry object (and anything the scanner cannot frame as a
//! collection) is read whole by the same parser as the whole read and handed out in batches:
//! such a file is a single row. Newline-delimited GeoJSON (GeoJSONSeq) is not supported by
//! the whole read either, so it is not streamed.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::str::FromStr;

use arrow_array::ArrayRef;
use arrow_schema::Field;
use geojson::{feature::Id, Feature, GeometryValue};
use serde_json::Value;

use crate::attributes::ipc_from_columns;
use crate::column::ColumnBuilder;
use crate::geojson_io::{build_column_as, features_of, kind_of, merge, push_geometry, Kind, CRS84};
use crate::VectorRead;

// -- scanner ------------------------------------------------------------------------------

/// Cuts a JSON document into raw values without parsing them.
struct Scanner<R: BufRead> {
    reader: R,
    path: String,
    /// The top-level `type` member, once seen.
    root_type: Option<String>,
    /// Whether the first element of the `features` array was handed out (or the array ended).
    started: bool,
}

impl<R: BufRead> Scanner<R> {
    fn new(reader: R, path: &str) -> Self {
        Self {
            reader,
            path: path.to_string(),
            root_type: None,
            started: false,
        }
    }

    fn peek(&mut self) -> Result<Option<u8>, String> {
        let buf = self
            .reader
            .fill_buf()
            .map_err(|e| format!("cannot read {}: {e}", self.path))?;
        Ok(buf.first().copied())
    }

    fn bump(&mut self) {
        self.reader.consume(1);
    }

    fn skip_whitespace(&mut self) -> Result<Option<u8>, String> {
        loop {
            match self.peek()? {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.bump(),
                other => return Ok(other),
            }
        }
    }

    fn invalid(&self, what: &str) -> String {
        format!("invalid GeoJSON in {}: {what}", self.path)
    }

    fn end_of_file(&self) -> String {
        self.invalid("unexpected end of file")
    }

    /// Reads one JSON value, appending its raw bytes to `out` when given.
    fn read_value(&mut self, mut out: Option<&mut Vec<u8>>) -> Result<(), String> {
        match self.skip_whitespace()? {
            None => Err(self.end_of_file()),
            Some(b'{' | b'[') => self.read_nested(out),
            Some(b'"') => self.read_string(out),
            Some(_) => {
                let mut any = false;
                while let Some(byte) = self.peek()? {
                    if matches!(byte, b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r') {
                        break;
                    }
                    if let Some(out) = out.as_deref_mut() {
                        out.push(byte);
                    }
                    any = true;
                    self.bump();
                }
                if any {
                    Ok(())
                } else {
                    Err(self.invalid("expected a value"))
                }
            }
        }
    }

    /// An object or array, up to its matching close.
    fn read_nested(&mut self, mut out: Option<&mut Vec<u8>>) -> Result<(), String> {
        let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
        loop {
            let buf = self
                .reader
                .fill_buf()
                .map_err(|e| format!("cannot read {}: {e}", self.path))?;
            if buf.is_empty() {
                return Err(self.end_of_file());
            }
            let mut used = 0;
            let mut done = false;
            for &byte in buf {
                used += 1;
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                } else {
                    match byte {
                        b'"' => in_string = true,
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth = depth.saturating_sub(1);
                            done = depth == 0;
                        }
                        _ => {}
                    }
                }
                if done {
                    break;
                }
            }
            if let Some(out) = out.as_deref_mut() {
                out.extend_from_slice(&buf[..used]);
            }
            self.reader.consume(used);
            if done {
                return Ok(());
            }
        }
    }

    /// A string including its quotes.
    fn read_string(&mut self, mut out: Option<&mut Vec<u8>>) -> Result<(), String> {
        self.bump();
        if let Some(out) = out.as_deref_mut() {
            out.push(b'"');
        }
        let mut escaped = false;
        loop {
            let buf = self
                .reader
                .fill_buf()
                .map_err(|e| format!("cannot read {}: {e}", self.path))?;
            if buf.is_empty() {
                return Err(self.end_of_file());
            }
            let mut used = 0;
            let mut done = false;
            for &byte in buf {
                used += 1;
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    done = true;
                    break;
                }
            }
            if let Some(out) = out.as_deref_mut() {
                out.extend_from_slice(&buf[..used]);
            }
            self.reader.consume(used);
            if done {
                return Ok(());
            }
        }
    }

    /// Reads the next top-level member up to its value. Returns the key, or `None` at the
    /// closing brace. The value is left unread.
    fn next_key(&mut self, first: bool) -> Result<Option<String>, String> {
        let mut next = self.skip_whitespace()?;
        if !first {
            match next {
                Some(b',') => {
                    self.bump();
                    next = self.skip_whitespace()?;
                }
                Some(b'}') => {
                    self.bump();
                    return Ok(None);
                }
                None => return Err(self.end_of_file()),
                Some(_) => return Err(self.invalid("expected `,` or `}`")),
            }
        }
        match next {
            Some(b'}') if first => {
                self.bump();
                return Ok(None);
            }
            Some(b'"') => {}
            None => return Err(self.end_of_file()),
            Some(_) => return Err(self.invalid("expected a member name")),
        }
        let mut raw = Vec::new();
        self.read_string(Some(&mut raw))?;
        let key: String = serde_json::from_slice(&raw).map_err(|e| self.invalid(&e.to_string()))?;
        if self.skip_whitespace()? != Some(b':') {
            return Err(self.invalid("expected `:` after a member name"));
        }
        self.bump();
        Ok(Some(key))
    }

    /// Reads a member value, remembering a top-level `type`.
    fn skip_member(&mut self, key: &str) -> Result<(), String> {
        if key == "type" {
            let mut raw = Vec::new();
            self.read_value(Some(&mut raw))?;
            self.root_type = serde_json::from_slice::<String>(&raw).ok();
            Ok(())
        } else {
            self.read_value(None)
        }
    }

    /// Moves to the first element of the top-level `features` array. `false` when the document
    /// is not an object with a `features` array.
    fn locate_features(&mut self) -> Result<bool, String> {
        if self.skip_whitespace()? != Some(b'{') {
            return Ok(false);
        }
        self.bump();
        let mut first = true;
        while let Some(key) = self.next_key(first)? {
            first = false;
            if key == "features" {
                if self.skip_whitespace()? != Some(b'[') {
                    return Ok(false);
                }
                self.bump();
                return Ok(true);
            }
            self.skip_member(&key)?;
        }
        Ok(false)
    }

    /// The raw bytes of the next feature into `out`; `false` at the end of the array.
    fn next_element(&mut self, out: &mut Vec<u8>) -> Result<bool, String> {
        out.clear();
        let mut next = self.skip_whitespace()?;
        if self.started {
            match next {
                Some(b',') => {
                    self.bump();
                    next = self.skip_whitespace()?;
                }
                Some(b']') => {
                    self.bump();
                    return Ok(false);
                }
                None => return Err(self.end_of_file()),
                Some(_) => return Err(self.invalid("expected `,` or `]` in the features")),
            }
        } else {
            self.started = true;
            if next == Some(b']') {
                self.bump();
                return Ok(false);
            }
        }
        match next {
            Some(b'{') => {
                self.read_nested(Some(out))?;
                Ok(true)
            }
            None => Err(self.end_of_file()),
            Some(_) => Err(self.invalid("a feature must be an object")),
        }
    }

    /// Reads the members after the `features` array (a `type` may come last).
    fn finish_root(&mut self) -> Result<(), String> {
        while let Some(key) = self.next_key(false)? {
            self.skip_member(&key)?;
        }
        Ok(())
    }
}

// -- feature sources ----------------------------------------------------------------------

type FileScanner = Scanner<BufReader<File>>;

/// Where the features come from.
enum Features {
    /// A `FeatureCollection` scanned from the file.
    File { scanner: FileScanner, buf: Vec<u8> },
    /// A lone feature or geometry, parsed whole.
    Memory(std::vec::IntoIter<Feature>),
}

impl Features {
    fn open_file(path: &str) -> Result<Option<FileScanner>, String> {
        let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
        let mut scanner = Scanner::new(BufReader::with_capacity(1 << 16, file), path);
        Ok(scanner.locate_features()?.then_some(scanner))
    }

    fn next(&mut self) -> Result<Option<Feature>, String> {
        match self {
            Self::Memory(features) => Ok(features.next()),
            Self::File { scanner, buf } => {
                if !scanner.next_element(buf)? {
                    return Ok(None);
                }
                let text = std::str::from_utf8(buf)
                    .map_err(|e| scanner.invalid(&format!("a feature is not UTF-8: {e}")))?;
                Feature::from_str(text)
                    .map(Some)
                    .map_err(|e| scanner.invalid(&e.to_string()))
            }
        }
    }
}

// -- schema -------------------------------------------------------------------------------

/// The columns of every batch: the leading feature id (when present on every feature) and the
/// properties in first-seen order, each with its type over the whole file.
struct Schema {
    id: Option<Kind>,
    properties: Vec<(String, Kind)>,
}

/// One pass over the features.
fn infer(mut next: impl FnMut() -> Result<Option<Feature>, String>) -> Result<Schema, String> {
    let mut properties: Vec<(String, Kind)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let (mut rows, mut all_ids, mut id_kind) = (0usize, true, Kind::Unseen);
    while let Some(feature) = next()? {
        rows += 1;
        if let Some(map) = &feature.properties {
            for (key, value) in map {
                let slot = *index.entry(key.clone()).or_insert_with(|| {
                    properties.push((key.clone(), Kind::Unseen));
                    properties.len() - 1
                });
                if !value.is_null() {
                    properties[slot].1 = merge(properties[slot].1, kind_of(value));
                }
            }
        }
        match &feature.id {
            Some(Id::String(_)) => id_kind = merge(id_kind, Kind::Text),
            Some(Id::Number(n)) => id_kind = merge(id_kind, kind_of(&Value::Number(n.clone()))),
            None => all_ids = false,
        }
    }
    let id = (all_ids && rows > 0 && !index.contains_key("id")).then_some(id_kind);
    Ok(Schema { id, properties })
}

fn id_value(feature: &Feature) -> Option<Value> {
    match feature.id.as_ref()? {
        Id::String(s) => Some(Value::String(s.clone())),
        Id::Number(n) => Some(Value::Number(n.clone())),
    }
}

// -- geometry bounds ----------------------------------------------------------------------

fn extend_bounds(bounds: &mut Option<[f64; 4]>, position: &[f64]) {
    if let [x, y, ..] = position {
        let b = bounds.get_or_insert([*x, *y, *x, *y]);
        b[0] = b[0].min(*x);
        b[1] = b[1].min(*y);
        b[2] = b[2].max(*x);
        b[3] = b[3].max(*y);
    }
}

/// The 2-D bounds of a geometry, `None` when it has no positions.
fn bounds_of(value: &GeometryValue, bounds: &mut Option<[f64; 4]>) {
    let mut add = |p: &geojson::Position| extend_bounds(bounds, p.as_slice());
    match value {
        GeometryValue::Point { coordinates } => add(coordinates),
        GeometryValue::MultiPoint { coordinates } | GeometryValue::LineString { coordinates } => {
            coordinates.iter().for_each(add);
        }
        GeometryValue::MultiLineString { coordinates } | GeometryValue::Polygon { coordinates } => {
            coordinates.iter().flatten().for_each(add);
        }
        GeometryValue::MultiPolygon { coordinates } => {
            coordinates.iter().flatten().flatten().for_each(add);
        }
        GeometryValue::GeometryCollection { geometries } => {
            for geometry in geometries {
                bounds_of(&geometry.value, bounds);
            }
        }
    }
}

/// Whether the feature's geometry has a bounding box intersecting `query` (no geometry never
/// does, as in every other streamed format).
fn intersects(feature: &Feature, query: [f64; 4]) -> bool {
    let mut bounds = None;
    if let Some(geometry) = &feature.geometry {
        bounds_of(&geometry.value, &mut bounds);
    }
    bounds.is_some_and(|b| {
        b[0] <= query[2] && b[2] >= query[0] && b[1] <= query[3] && b[3] >= query[1]
    })
}

// -- stream -------------------------------------------------------------------------------

/// A GeoJSON file read `batch_size` features at a time.
pub(crate) struct GeojsonStream {
    features: Features,
    schema: Schema,
    bbox: Option<[f64; 4]>,
    batch_size: usize,
    /// Features consumed so far (for error messages).
    row: usize,
    /// The features ran out (the scanner must not be asked again).
    finished: bool,
}

impl GeojsonStream {
    pub(crate) fn open(
        path: &str,
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
        let (features, schema) = match Features::open_file(path)? {
            Some(scanner) => {
                let mut pass = Features::File {
                    scanner,
                    buf: Vec::new(),
                };
                let schema = infer(|| pass.next())?;
                let Features::File { scanner: mut s, .. } = pass else {
                    unreachable!("the first pass reads a file");
                };
                s.finish_root()?;
                if s.root_type.as_deref() == Some("FeatureCollection") {
                    let second = Features::open_file(path)?
                        .ok_or_else(|| format!("{path} changed while it was being read"))?;
                    (
                        Features::File {
                            scanner: second,
                            buf: Vec::new(),
                        },
                        schema,
                    )
                } else {
                    Self::whole(path)?
                }
            }
            None => Self::whole(path)?,
        };
        Ok(Self {
            features,
            schema,
            bbox,
            batch_size,
            row: 0,
            finished: false,
        })
    }

    /// A file that is not a plain `FeatureCollection`: parsed whole, as the whole read does.
    fn whole(path: &str) -> Result<(Features, Schema), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let features = features_of(&text, path)?;
        let mut pass = features.iter().cloned();
        let schema = infer(|| Ok(pass.next()))?;
        Ok((Features::Memory(features.into_iter()), schema))
    }

    /// The next feature that passes the bbox.
    fn next_kept(&mut self) -> Result<Option<(usize, Feature)>, String> {
        while !self.finished {
            let Some(feature) = self.features.next()? else {
                self.finished = true;
                break;
            };
            let row = self.row;
            self.row += 1;
            if self.bbox.is_none_or(|query| intersects(&feature, query)) {
                return Ok(Some((row, feature)));
            }
        }
        Ok(None)
    }

    /// Drops up to `count` rows without building anything; returns how many were dropped.
    pub(crate) fn skip(&mut self, count: usize) -> Result<usize, String> {
        let mut skipped = 0;
        while skipped < count && self.next_kept()?.is_some() {
            skipped += 1;
        }
        Ok(skipped)
    }

    /// The next `batch_size` features (fewer at the end), `None` when none are left.
    pub(crate) fn next_batch(&mut self) -> Result<Option<VectorRead>, String> {
        let mut builder = ColumnBuilder::new(Some(CRS84));
        let mut kept: Vec<Feature> = Vec::new();
        while kept.len() < self.batch_size {
            let Some((row, feature)) = self.next_kept()? else {
                break;
            };
            push_geometry(&mut builder, row, &feature)?;
            kept.push(feature);
        }
        if kept.is_empty() {
            return Ok(None);
        }

        let mut fields = Vec::new();
        let mut columns: Vec<ArrayRef> = Vec::new();
        if let Some(kind) = self.schema.id {
            let ids: Vec<Option<Value>> = kept.iter().map(id_value).collect();
            let cells: Vec<Option<&Value>> = ids.iter().map(Option::as_ref).collect();
            let array = build_column_as(kind, &cells);
            fields.push(Field::new("id", array.data_type().clone(), true));
            columns.push(array);
        }
        for (name, kind) in &self.schema.properties {
            let cells: Vec<Option<&Value>> = kept
                .iter()
                .map(|f| {
                    f.properties
                        .as_ref()
                        .and_then(|p| p.get(name))
                        .filter(|v| !v.is_null())
                })
                .collect();
            let array = build_column_as(*kind, &cells);
            fields.push(Field::new(name, array.data_type().clone(), true));
            columns.push(array);
        }
        Ok(Some(VectorRead {
            geometry: builder.finish(),
            attributes_ipc: ipc_from_columns(fields, columns, kept.len())?,
        }))
    }
}
