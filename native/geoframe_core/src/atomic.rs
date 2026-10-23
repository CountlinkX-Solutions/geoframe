//! Crash- and failure-safe file output. A writer streams into `<path>.tmp-<unique>` next to
//! the destination and renames it over `path` only when it succeeded, so a failure never
//! leaves a partial file and never removes a file that already existed at `path`.

use std::fs::{File, OpenOptions};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_path(path: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{path}.tmp-{:x}-{nanos:x}-{n:x}", std::process::id())
}

/// Runs `write` against a fresh temp file beside `path` and renames it over `path` on
/// success. On failure only the temp file is removed.
pub(crate) fn write_atomic<F>(path: &str, write: F) -> Result<(), String>
where
    F: FnOnce(File) -> Result<(), String>,
{
    let temp = temp_path(path);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("cannot create {path}: {e}"))?;
    let outcome = write(file).and_then(|()| {
        std::fs::rename(&temp, path).map_err(|e| format!("cannot write {path}: {e}"))
    });
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    outcome
}
