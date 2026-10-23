//! dBase III (`.dbf`) reading and writing for Shapefile attributes.
//!
//! The `shapefile` crate's dBase layer hides the decimal count of numeric fields and widens
//! integers through `f64`, so the table is handled here instead, following what GDAL does.
//!
//! # Types
//!
//! | dBase            | Arrow (read)                     | Arrow (write)                       |
//! |------------------|----------------------------------|-------------------------------------|
//! | `C`              | Utf8 (trailing blanks trimmed)   | Utf8, timestamps as ISO 8601 text   |
//! | `N`, 0 decimals  | Int32 (width <= 9), Int64 (<= 18), else Float64 | Int8/16/32 `N(9,0)`, Int64 and unsigned `N(18,0)` |
//! | `N` (decimals), `F` | Float64                       | Float32/64 `N(24,15)`, fewer decimals for big values |
//! | `L`              | Boolean                          | Boolean (`T`, `F`, `?` for null)    |
//! | `D`              | Date32                           | Date32/Date64 (`YYYYMMDD`)          |
//!
//! Blank character, numeric, date and logical fields are null, so an empty string and a null
//! string are the same thing in a dBase file (GDAL has the same limit). Strings are limited
//! to 254 bytes and truncated beyond that; binary columns cannot be stored. Records flagged
//! as deleted are skipped. Memo and other binary field types are rejected.
//!
//! # Encodings
//!
//! UTF-8 (the default), ASCII, ISO-8859-1 and Windows-1252 (the encodings that need no
//! tables beyond the C1 range). The encoding is not read from the dBase language-driver byte,
//! matching pyogrio; the `.cpg` file or an explicit option selects it.
//!
//! # Field names
//!
//! A dBase field name holds at most 10 bytes. Longer names are truncated at a character
//! boundary and, when the result collides with an earlier name (compared case-insensitively),
//! the stem is cut so that `_1`, `_2`, ... fits in 10 bytes: `population_total`,
//! `population_urban` become `population` and `populati_1`. This is GDAL's behaviour.

use std::collections::HashSet;
use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, Date32Builder, Float64Builder, Int32Builder, Int64Builder, StringBuilder,
};
use arrow_array::{ArrayRef, BooleanArray};
use arrow_schema::{DataType, Field};
use arrow_select::filter::filter;

use crate::attributes::{Cell, Table};

/// Longest string a dBase `C` field holds.
const MAX_TEXT: usize = 254;
/// GDAL's default numeric field: 24 characters, 15 decimals.
const REAL_WIDTH: usize = 24;
const REAL_DECIMALS: usize = 15;

// -- encodings --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf8,
    Ascii,
    Latin1,
    Cp1252,
}

/// Windows-1252 characters for the bytes 0x80..=0x9F (`\0` marks the five undefined bytes).
const CP1252_C1: [char; 32] = [
    '€', '\0', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\0', 'Ž', '\0', '\0', '‘',
    '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\0', 'ž', 'Ÿ',
];

impl Encoding {
    pub(crate) fn parse(name: &str) -> Result<Self, String> {
        let key: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_uppercase();
        match key.as_str() {
            "UTF8" | "65001" | "CP65001" => Ok(Self::Utf8),
            "ASCII" | "USASCII" | "ANSIX3" => Ok(Self::Ascii),
            "ISO88591" | "LATIN1" | "L1" | "8859" | "ISO8859" => Ok(Self::Latin1),
            "CP1252" | "WINDOWS1252" | "1252" | "ANSI1252" | "WINDOWS" | "ANSI" => Ok(Self::Cp1252),
            _ => Err(format!(
                "unsupported encoding {name:?}; supported: UTF-8, ASCII, ISO-8859-1, CP1252"
            )),
        }
    }

    /// The name written to the `.cpg` file.
    pub(crate) fn cpg_name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Ascii => "ASCII",
            Self::Latin1 => "ISO-8859-1",
            Self::Cp1252 => "CP1252",
        }
    }

    /// The dBase language-driver byte: 0 (unspecified) for UTF-8 and ASCII, 0x57 (ANSI) for
    /// the single-byte code pages.
    fn language_driver(self) -> u8 {
        match self {
            Self::Utf8 | Self::Ascii => 0,
            Self::Latin1 | Self::Cp1252 => 0x57,
        }
    }

    fn decode(self, bytes: &[u8]) -> String {
        match self {
            Self::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Self::Ascii | Self::Latin1 => bytes.iter().map(|&b| b as char).collect(),
            Self::Cp1252 => bytes
                .iter()
                .map(|&b| match b {
                    0x80..=0x9F => match CP1252_C1[usize::from(b - 0x80)] {
                        '\0' => char::REPLACEMENT_CHARACTER,
                        c => c,
                    },
                    _ => b as char,
                })
                .collect(),
        }
    }

    /// Encodes `text`; characters the encoding lacks become `?` and set `lossy`.
    fn encode(self, text: &str, lossy: &mut bool) -> Vec<u8> {
        match self {
            Self::Utf8 => text.as_bytes().to_vec(),
            Self::Ascii => text
                .chars()
                .map(|c| {
                    if c.is_ascii() {
                        c as u8
                    } else {
                        *lossy = true;
                        b'?'
                    }
                })
                .collect(),
            Self::Latin1 => text
                .chars()
                .map(|c| {
                    if (c as u32) < 256 {
                        c as u8
                    } else {
                        *lossy = true;
                        b'?'
                    }
                })
                .collect(),
            Self::Cp1252 => text
                .chars()
                .map(|c| {
                    let code = c as u32;
                    if code < 0x80 || (0xA0..256).contains(&code) {
                        code as u8
                    } else if let Some(i) = CP1252_C1.iter().position(|&m| m == c && m != '\0') {
                        0x80 + i as u8
                    } else {
                        *lossy = true;
                        b'?'
                    }
                })
                .collect(),
        }
    }
}

// -- field names ------------------------------------------------------------------------

/// The longest prefix of `text` that fits in `max` bytes without splitting a character.
fn prefix_bytes(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// GDAL's field-name laundering; see the module documentation. Returns the new names and
/// whether each one differs from its input.
pub(crate) fn launder_field_names(names: &[String]) -> (Vec<String>, Vec<bool>) {
    let mut used: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let mut candidate = prefix_bytes(name, 10).to_string();
        let mut n = 1;
        while used.contains(&candidate.to_ascii_lowercase()) {
            let suffix = format!("_{n}");
            candidate = format!("{}{suffix}", prefix_bytes(name, 10 - suffix.len()));
            n += 1;
        }
        used.insert(candidate.to_ascii_lowercase());
        out.push(candidate);
    }
    let changed = names.iter().zip(&out).map(|(a, b)| a != b).collect();
    (out, changed)
}

// -- header -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct FieldDesc {
    pub name: String,
    pub kind: u8,
    pub length: usize,
    pub decimals: usize,
}

pub(crate) struct Header {
    pub records: usize,
    pub header_len: usize,
    pub record_len: usize,
    pub fields: Vec<FieldDesc>,
}

pub(crate) fn parse_header(bytes: &[u8], encoding: Encoding) -> Result<Header, String> {
    if bytes.len() < 33 {
        return Err("the .dbf file is truncated".to_string());
    }
    let records = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes")) as usize;
    let header_len = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
    let record_len = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
    let mut fields = Vec::new();
    let mut pos = 32;
    while pos + 32 <= bytes.len() && bytes[pos] != 0x0D && pos < header_len {
        let descriptor = &bytes[pos..pos + 32];
        let name_bytes = descriptor[..11].split(|&b| b == 0).next().unwrap_or(&[]);
        fields.push(FieldDesc {
            name: encoding.decode(name_bytes),
            kind: descriptor[11].to_ascii_uppercase(),
            length: usize::from(descriptor[16]),
            decimals: usize::from(descriptor[17]),
        });
        pos += 32;
    }
    Ok(Header {
        records,
        header_len,
        record_len,
        fields,
    })
}

/// The pandas-style dtype GDAL / pyogrio report for a field.
pub(crate) fn dtype_name(field: &FieldDesc) -> &'static str {
    match (field.kind, field.decimals) {
        (b'N', 0) if field.length <= 9 => "int32",
        (b'N', 0) if field.length <= 18 => "int64",
        (b'N' | b'F', _) => "float64",
        (b'L', _) => "bool",
        (b'D', _) => "datetime64[D]",
        _ => "object",
    }
}

fn arrow_type(field: &FieldDesc) -> Result<DataType, String> {
    Ok(match field.kind {
        b'C' => DataType::Utf8,
        b'N' | b'F' => match dtype_name(field) {
            "int32" => DataType::Int32,
            "int64" => DataType::Int64,
            _ => DataType::Float64,
        },
        b'L' => DataType::Boolean,
        b'D' => DataType::Date32,
        other => {
            return Err(format!(
                "field {:?} has the unsupported dBase type {:?}",
                field.name, other as char
            ))
        }
    })
}

// -- reading ----------------------------------------------------------------------------

pub(crate) struct DbfRead {
    pub fields: Vec<Field>,
    pub columns: Vec<ArrayRef>,
    /// Number of records in the file, deleted ones included.
    pub records: usize,
    /// Which records carry the deleted flag.
    pub deleted: Vec<bool>,
}

pub(crate) fn read_dbf(bytes: &[u8], encoding: Encoding) -> Result<DbfRead, String> {
    let header = parse_header(bytes, encoding)?;
    let available = bytes
        .len()
        .saturating_sub(header.header_len)
        .checked_div(header.record_len)
        .unwrap_or(0);
    let records = header.records.min(available);

    let types: Vec<DataType> = header
        .fields
        .iter()
        .map(arrow_type)
        .collect::<Result<_, _>>()?;
    let mut deleted = Vec::with_capacity(records);
    let mut texts: Vec<Vec<Option<&[u8]>>> = vec![Vec::with_capacity(records); header.fields.len()];
    for row in 0..records {
        let start = header.header_len + row * header.record_len;
        let record = &bytes[start..start + header.record_len];
        deleted.push(record.first() == Some(&b'*'));
        let mut pos = 1;
        for (column, field) in texts.iter_mut().zip(&header.fields) {
            let end = (pos + field.length).min(record.len());
            column.push(Some(&record[pos.min(end)..end]));
            pos = end;
        }
    }

    let columns = types
        .iter()
        .zip(&texts)
        .map(|(data_type, cells)| build_column(data_type, cells, encoding))
        .collect::<Vec<_>>();
    let fields = header
        .fields
        .iter()
        .zip(&types)
        .map(|(f, t)| Field::new(&f.name, t.clone(), true))
        .collect();
    Ok(DbfRead {
        fields,
        columns,
        records,
        deleted,
    })
}

/// Drops the rows flagged in `deleted` from every column.
pub(crate) fn drop_deleted(
    columns: &[ArrayRef],
    deleted: &[bool],
) -> Result<Vec<ArrayRef>, String> {
    let keep = BooleanArray::from_iter(deleted.iter().map(|d| Some(!d)));
    columns
        .iter()
        .map(|c| filter(c, &keep).map_err(|e| e.to_string()))
        .collect()
}

fn trimmed(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|&b| b != b' ' && b != 0)
        .map_or(0, |i| i + 1);
    &bytes[..end]
}

fn numeric_text(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes)
        .ok()?
        .trim_matches(|c: char| c == ' ' || c == '\0');
    (!text.is_empty() && !text.starts_with('*')).then_some(text)
}

fn build_column(data_type: &DataType, cells: &[Option<&[u8]>], encoding: Encoding) -> ArrayRef {
    let rows = cells.iter().map(|c| c.unwrap_or(&[]));
    match data_type {
        DataType::Utf8 => {
            let mut builder = StringBuilder::new();
            for cell in rows {
                let text = trimmed(cell);
                if text.is_empty() {
                    builder.append_null();
                } else {
                    builder.append_value(encoding.decode(text));
                }
            }
            Arc::new(builder.finish())
        }
        DataType::Int32 => {
            let mut builder = Int32Builder::new();
            for cell in rows {
                builder.append_option(
                    numeric_text(cell)
                        .and_then(parse_integer)
                        .and_then(|v| i32::try_from(v).ok()),
                );
            }
            Arc::new(builder.finish())
        }
        DataType::Int64 => {
            let mut builder = Int64Builder::new();
            for cell in rows {
                builder.append_option(numeric_text(cell).and_then(parse_integer));
            }
            Arc::new(builder.finish())
        }
        DataType::Float64 => {
            let mut builder = Float64Builder::new();
            for cell in rows {
                builder.append_option(numeric_text(cell).and_then(|t| t.parse::<f64>().ok()));
            }
            Arc::new(builder.finish())
        }
        DataType::Boolean => {
            let mut builder = BooleanBuilder::new();
            for cell in rows {
                builder.append_option(match cell.first() {
                    Some(b'T' | b't' | b'Y' | b'y') => Some(true),
                    Some(b'F' | b'f' | b'N' | b'n') => Some(false),
                    _ => None,
                });
            }
            Arc::new(builder.finish())
        }
        _ => {
            let mut builder = Date32Builder::new();
            for cell in rows {
                builder.append_option(parse_date(cell));
            }
            Arc::new(builder.finish())
        }
    }
}

fn parse_integer(text: &str) -> Option<i64> {
    text.parse::<i64>().ok().or_else(|| {
        // "12.0" in a field that claims no decimals
        let value = text.parse::<f64>().ok()?;
        (value.fract() == 0.0 && value.abs() < 9.0e18).then_some(value as i64)
    })
}

fn parse_date(bytes: &[u8]) -> Option<i32> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.len() != 8 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i64 = text[..4].parse().ok()?;
    let month: u32 = text[4..6].parse().ok()?;
    let day: u32 = text[6..8].parse().ok()?;
    if year == 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    i32::try_from(days_from_civil(year, month, day)).ok()
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// -- writing ----------------------------------------------------------------------------

/// A column ready for the record writer.
struct Prepared {
    name: String,
    kind: u8,
    length: usize,
    decimals: usize,
    cells: Vec<Vec<u8>>,
}

/// Encodes the attributes as a dBase III file. `rows` is the record count (which a table
/// without columns cannot tell). Appends warnings for changed names and truncated values.
pub(crate) fn write_dbf(
    table: &Table,
    rows: usize,
    encoding: Encoding,
    warnings: &mut Vec<String>,
) -> Result<Vec<u8>, String> {
    let mut columns: Vec<Prepared> = Vec::new();
    if table.names.is_empty() {
        // GDAL gives an attribute-less layer a dummy FID column; a dBase file needs a field.
        columns.push(Prepared {
            name: "FID".to_string(),
            kind: b'N',
            length: 9,
            decimals: 0,
            cells: (0..rows).map(|i| format!("{i:>9}").into_bytes()).collect(),
        });
    } else {
        let (names, changed) = launder_field_names(&table.names);
        for ((original, new), changed) in table.names.iter().zip(&names).zip(changed) {
            if changed {
                warnings.push(format!(
                    "column {original:?} was truncated to {new:?} (dBase field names hold at most 10 bytes)"
                ));
            }
        }
        for (index, name) in names.into_iter().enumerate() {
            columns.push(prepare_column(
                table, index, rows, name, encoding, warnings,
            )?);
        }
    }

    if columns.len() > 2046 {
        return Err("a dBase file holds at most 2046 fields".to_string());
    }
    let record_len = 1 + columns.iter().map(|c| c.length).sum::<usize>();
    let header_len = 32 + 32 * columns.len() + 1;
    if record_len > usize::from(u16::MAX) {
        return Err("the attribute record is too long for a dBase file".to_string());
    }

    let mut out = Vec::with_capacity(header_len + rows * record_len + 1);
    out.push(0x03);
    out.extend_from_slice(&[126, 1, 1]); // last update: a fixed date keeps the output reproducible
    out.extend_from_slice(&(rows as u32).to_le_bytes());
    out.extend_from_slice(&(header_len as u16).to_le_bytes());
    out.extend_from_slice(&(record_len as u16).to_le_bytes());
    out.extend_from_slice(&[0; 17]);
    out.push(encoding.language_driver());
    out.extend_from_slice(&[0; 2]);
    for column in &columns {
        let mut name = encoding.encode(&column.name, &mut false);
        name.truncate(10);
        name.resize(11, 0);
        out.extend_from_slice(&name);
        out.push(column.kind);
        out.extend_from_slice(&[0; 4]);
        out.push(column.length as u8);
        out.push(column.decimals as u8);
        out.extend_from_slice(&[0; 14]);
    }
    out.push(0x0D);
    for row in 0..rows {
        out.push(b' ');
        for column in &columns {
            out.extend_from_slice(&column.cells[row]);
        }
    }
    out.push(0x1A);
    Ok(out)
}

fn prepare_column(
    table: &Table,
    index: usize,
    rows: usize,
    name: String,
    encoding: Encoding,
    warnings: &mut Vec<String>,
) -> Result<Prepared, String> {
    let source = &table.names[index];
    let original = &table.source_types[index];
    let cell = |row| table.cell(index, row);

    match table.arrays[index].data_type() {
        DataType::Boolean => Ok(Prepared {
            name,
            kind: b'L',
            length: 1,
            decimals: 0,
            cells: (0..rows)
                .map(|r| {
                    vec![match cell(r) {
                        Cell::Bool(true) => b'T',
                        Cell::Bool(false) => b'F',
                        _ => b'?',
                    }]
                })
                .collect(),
        }),
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64 => {
            let base = if matches!(original, DataType::Int8 | DataType::Int16 | DataType::Int32) {
                9
            } else {
                18
            };
            let texts: Vec<Option<String>> = (0..rows)
                .map(|r| match cell(r) {
                    Cell::Int(v) => Some(v.to_string()),
                    Cell::UInt(v) => Some(v.to_string()),
                    _ => None,
                })
                .collect();
            let width = texts
                .iter()
                .flatten()
                .map(String::len)
                .max()
                .unwrap_or(1)
                .max(base);
            if width > 20 {
                return Err(format!(
                    "column {source:?} holds an integer wider than 20 digits"
                ));
            }
            Ok(Prepared {
                name,
                kind: b'N',
                length: width,
                decimals: 0,
                cells: texts
                    .into_iter()
                    .map(|t| format!("{:>width$}", t.unwrap_or_default()).into_bytes())
                    .collect(),
            })
        }
        DataType::Float32 | DataType::Float64 => {
            let values: Vec<Option<f64>> = (0..rows)
                .map(|r| match cell(r) {
                    Cell::Float(v) if v.is_finite() => Some(v),
                    _ => None,
                })
                .collect();
            let integer_digits = values
                .iter()
                .flatten()
                .map(|v| {
                    let digits = format!("{:.0}", v.abs()).len();
                    digits + usize::from(*v < 0.0)
                })
                .max()
                .unwrap_or(1);
            if integer_digits + 2 > REAL_WIDTH {
                return Err(format!(
                    "column {source:?} holds a value too large for a dBase numeric field"
                ));
            }
            let decimals = REAL_DECIMALS.min(REAL_WIDTH - integer_digits - 1);
            Ok(Prepared {
                name,
                kind: b'N',
                length: REAL_WIDTH,
                decimals,
                cells: values
                    .into_iter()
                    .map(|v| match v {
                        Some(v) => format!("{v:>REAL_WIDTH$.decimals$}").into_bytes(),
                        None => vec![b' '; REAL_WIDTH],
                    })
                    .collect(),
            })
        }
        DataType::Binary | DataType::LargeBinary => Err(format!(
            "column {source:?} holds binary data, which a shapefile cannot store"
        )),
        _ => {
            if matches!(original, DataType::Date32 | DataType::Date64) {
                return Ok(Prepared {
                    name,
                    kind: b'D',
                    length: 8,
                    decimals: 0,
                    cells: (0..rows)
                        .map(|r| match cell(r) {
                            Cell::Str(text) => {
                                let digits: String =
                                    text.chars().filter(char::is_ascii_digit).collect();
                                if digits.len() == 8 {
                                    digits.into_bytes()
                                } else {
                                    vec![b' '; 8]
                                }
                            }
                            _ => vec![b' '; 8],
                        })
                        .collect(),
                });
            }
            let mut lossy = false;
            let mut truncated = 0usize;
            let mut texts: Vec<Vec<u8>> = Vec::with_capacity(rows);
            for r in 0..rows {
                let Cell::Str(text) = cell(r) else {
                    texts.push(Vec::new());
                    continue;
                };
                let mut bytes = encoding.encode(text, &mut lossy);
                if bytes.len() > MAX_TEXT {
                    truncated += 1;
                    let mut kept = text;
                    while encoding.encode(kept, &mut false).len() > MAX_TEXT {
                        let cut = kept.char_indices().next_back().map_or(0, |(i, _)| i);
                        kept = &kept[..cut];
                    }
                    bytes = encoding.encode(kept, &mut false);
                }
                texts.push(bytes);
            }
            if lossy {
                warnings.push(format!(
                    "column {source:?} has characters {} cannot represent; they were replaced by '?'",
                    encoding.cpg_name()
                ));
            }
            if truncated > 0 {
                warnings.push(format!(
                    "column {source:?}: {truncated} value(s) longer than {MAX_TEXT} bytes were truncated to {MAX_TEXT}"
                ));
            }
            let width = texts.iter().map(Vec::len).max().unwrap_or(1).max(1);
            for text in &mut texts {
                text.resize(width, b' ');
            }
            Ok(Prepared {
                name,
                kind: b'C',
                length: width,
                decimals: 0,
                cells: texts,
            })
        }
    }
}
