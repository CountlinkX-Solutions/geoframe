//! Timing benchmarks for the quadratic paths of issue #57. They are `#[ignore]`d (slow, and
//! timings are not assertions); run with
//! `cargo test --release -p geoframe_core perf_bench -- --ignored --nocapture`.
//! Each prints `BENCH <name> n=<size> <ms> ms`.

use std::time::Instant;

use crate::{GeoColumn, SetOp, UnaryOp};

/// Small deterministic generator (xorshift) so runs are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn timed<T>(name: &str, n: usize, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    println!(
        "BENCH {name} n={n} {:.1} ms",
        start.elapsed().as_secs_f64() * 1e3
    );
    out
}

fn sizes() -> Vec<usize> {
    std::env::var("BENCH_SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|v| v.parse().ok()).collect())
        .unwrap_or_else(|| vec![10_000, 100_000])
}

fn wkt_one(wkt: String) -> GeoColumn {
    GeoColumn::from_wkt(&[Some(wkt)], None).expect("wkt")
}

fn random_points(n: usize) -> GeoColumn {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let pts: Vec<String> = (0..n)
        .map(|_| format!("{} {}", rng.next() * 1000.0, rng.next() * 1000.0))
        .collect();
    wkt_one(format!("MULTIPOINT ({})", pts.join(", ")))
}

#[test]
#[ignore]
fn perf_bench_voronoi_tolerance_sites() {
    for n in sizes() {
        let cloud = random_points(n);
        timed("voronoi_tolerance", n, || {
            cloud.voronoi_polygons(0.05, None, false, false).unwrap()
        });
        timed("delaunay_tolerance", n, || {
            cloud.delaunay_triangles(0.05, false).unwrap()
        });
    }
}

/// `n` short random segments, each a separate row.
fn random_segment_rows(n: usize) -> GeoColumn {
    let mut rng = Rng(42);
    let rows: Vec<String> = (0..n)
        .map(|_| {
            let (x, y) = (rng.next() * 1000.0, rng.next() * 1000.0);
            format!(
                "LINESTRING ({x} {y}, {} {})",
                x + rng.next() * 2.0,
                y + rng.next() * 2.0
            )
        })
        .collect();
    GeoColumn::from_wkt(
        &rows.iter().map(|r| Some(r.as_str())).collect::<Vec<_>>(),
        None,
    )
    .expect("wkt")
}

#[test]
#[ignore]
fn perf_bench_union_all_lines() {
    let sizes: Vec<usize> = sizes().into_iter().map(|n| n / 5).collect();
    for n in sizes {
        let rows = random_segment_rows(n);
        timed("union_all_lines", n, || rows.union_all().unwrap());
    }
}

#[test]
#[ignore]
fn perf_bench_noding_split_collinear() {
    for n in sizes().into_iter().map(|n| n / 5) {
        // One long segment overlapped by n collinear pieces: n cut points and n spans.
        let pieces: Vec<String> = (0..n)
            .map(|i| format!("({} 0, {} 0)", i * 2, i * 2 + 1))
            .collect();
        let long = wkt_one(format!("LINESTRING (0 0, {} 0)", n * 2));
        let multi = wkt_one(format!("MULTILINESTRING ({})", pieces.join(", ")));
        timed("noding_split_collinear", n, || {
            long.set_op(&multi, SetOp::Union).unwrap()
        });
    }
}

#[test]
#[ignore]
fn perf_bench_line_boundary() {
    for n in sizes() {
        let mut rng = Rng(7);
        let lines: Vec<String> = (0..n)
            .map(|_| {
                let (x, y) = (rng.next() * 1e6, rng.next() * 1e6);
                format!("({x} {y}, {} {})", x + 1.0, y + 1.0)
            })
            .collect();
        let col = wkt_one(format!("MULTILINESTRING ({})", lines.join(", ")));
        timed("line_boundary", n, || col.unary(UnaryOp::Boundary).unwrap());
    }
}
