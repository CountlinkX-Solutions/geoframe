//! Dev tool of the issue #97 sweep: reads lines `source x y` on stdin (source is a CRS label),
//! writes `x y` in EPSG:4326 (lon/lat degrees) or `ERR <message>` per line. Driven by
//! `test/reference/sweep_crs.py`.
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout());
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let mut it = line.split_whitespace();
        let (src, x, y) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        let (x, y): (f64, f64) = (x.parse().unwrap(), y.parse().unwrap());
        match geoframe_core::transform_xy(src, "EPSG:4326", x, y) {
            Ok((x, y)) => writeln!(out, "{x:.12} {y:.12}").unwrap(),
            Err(e) => writeln!(out, "ERR {}", e.replace('\n', " ")).unwrap(),
        }
    }
}
