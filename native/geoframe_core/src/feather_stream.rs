//! Batched reading of Feather v2 / Arrow IPC files.
//!
//! The file is read record batch by record batch and re-sliced to exactly `batch_size` rows
//! (the last batch may be shorter), whatever the record batch size the file was written with.
//! Each batch goes through the same decoding as [`crate::read_feather`] (`decode_table`), so
//! the batches concatenate to what the whole read returns. A file written as one huge record
//! batch (GeoFrame's own `write_feather` does) is still decoded one record batch at a time:
//! its record batch is held in memory, the stream just hands it out in `batch_size` pieces.

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;

use arrow_array::RecordBatch;
use arrow_ipc::reader::FileReader;
use arrow_schema::SchemaRef;
use arrow_select::concat::concat_batches;

use crate::arrow_io::{decode_table, TableRead};

/// A Feather file read `batch_size` rows at a time.
pub(crate) struct FeatherStream {
    path: String,
    reader: FileReader<BufReader<File>>,
    schema: SchemaRef,
    columns: Option<Vec<String>>,
    geometry: Option<String>,
    batch_size: usize,
    /// Rows read from the file but not yet handed out, in order.
    pending: VecDeque<RecordBatch>,
    pending_rows: usize,
}

impl FeatherStream {
    pub(crate) fn open(
        path: &str,
        columns: Option<&[String]>,
        geometry: Option<&str>,
        batch_size: usize,
    ) -> Result<Self, String> {
        if batch_size == 0 {
            return Err("batch_size must be a positive number of rows".to_string());
        }
        let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
        let reader = FileReader::try_new(BufReader::new(file), None)
            .map_err(|e| format!("cannot read {path} as Feather: {e}"))?;
        let schema = reader.schema();
        // Fail on open, not on the first batch, for a missing column or geometry column.
        decode_table(
            &schema,
            &[RecordBatch::new_empty(schema.clone())],
            columns,
            geometry,
        )
        .map_err(|e| format!("{path}: {e}"))?;
        Ok(Self {
            path: path.to_string(),
            reader,
            schema,
            columns: columns.map(<[String]>::to_vec),
            geometry: geometry.map(str::to_string),
            batch_size,
            pending: VecDeque::new(),
            pending_rows: 0,
        })
    }

    /// Drops the next `count` rows, whole record batches without decoding them.
    pub(crate) fn skip(&mut self, count: usize) -> Result<(), String> {
        let mut left = count;
        while left > 0 {
            let batch = match self.pending.pop_front() {
                Some(batch) => {
                    self.pending_rows -= batch.num_rows();
                    batch
                }
                None => match self.reader.next() {
                    None => break,
                    Some(batch) => batch.map_err(|e| format!("cannot read {}: {e}", self.path))?,
                },
            };
            if batch.num_rows() <= left {
                left -= batch.num_rows();
            } else {
                let rest = batch.slice(left, batch.num_rows() - left);
                self.pending_rows += rest.num_rows();
                self.pending.push_front(rest);
                left = 0;
            }
        }
        Ok(())
    }

    /// The next `batch_size` rows (fewer at the end), `None` when none are left.
    pub(crate) fn next_batch(&mut self) -> Result<Option<TableRead>, String> {
        while self.pending_rows < self.batch_size {
            let Some(batch) = self.reader.next() else {
                break;
            };
            let batch = batch.map_err(|e| format!("cannot read {}: {e}", self.path))?;
            if batch.num_rows() > 0 {
                self.pending_rows += batch.num_rows();
                self.pending.push_back(batch);
            }
        }
        if self.pending_rows == 0 {
            return Ok(None);
        }
        let mut parts = Vec::new();
        let mut wanted = self.batch_size;
        while wanted > 0 {
            let Some(front) = self.pending.pop_front() else {
                break;
            };
            if front.num_rows() > wanted {
                parts.push(front.slice(0, wanted));
                self.pending
                    .push_front(front.slice(wanted, front.num_rows() - wanted));
                wanted = 0;
            } else {
                wanted -= front.num_rows();
                parts.push(front);
            }
        }
        let batch = if let [only] = parts.as_slice() {
            only.clone()
        } else {
            concat_batches(&self.schema, &parts)
                .map_err(|e| format!("cannot read {}: {e}", self.path))?
        };
        self.pending_rows -= batch.num_rows();
        decode_table(
            &self.schema,
            &[batch],
            self.columns.as_deref(),
            self.geometry.as_deref(),
        )
        .map(Some)
        .map_err(|e| format!("{}: {e}", self.path))
    }
}
