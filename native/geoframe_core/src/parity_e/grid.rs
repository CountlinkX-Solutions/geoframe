//! `make_grid` (GeoPandas 1.2): square and hexagonal grids over the total bounds of a series.
//!
//! This follows `geopandas.tools.make_grid` index for index: the same NumPy `arange`/meshgrid
//! layout, the same corner grouping for hexagons and the same output order (the grid is built
//! column by column, hexagons in two interleaved families), so the result can be compared
//! element by element with GeoPandas.

use geo::{BoundingRect, Coord, Geometry, Intersects, LineString, Point, Polygon, Rect};

/// Cell shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridType {
    Square,
    Hexagon,
}

impl GridType {
    /// Parses `"square"` or `"hexagon"`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "square" => Some(Self::Square),
            "hexagon" => Some(Self::Hexagon),
            _ => None,
        }
    }
}

/// What the grid returns: cell centers, cell corners (unique vertices) or cell polygons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridFeature {
    Centers,
    Corners,
    Polygons,
}

impl GridFeature {
    /// Parses `"centers"`, `"corners"` or `"polygons"`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "centers" => Some(Self::Centers),
            "corners" => Some(Self::Corners),
            "polygons" => Some(Self::Polygons),
            _ => None,
        }
    }
}

/// Most cells (or points) any grid may produce.
pub const MAX_GRID_CELLS: u64 = 10_000_000;

fn too_large(cells: u64) -> String {
    format!("grid too large ({cells} cells; max 10_000_000)")
}

/// `numpy.arange(start, stop, step)` for a positive step. NumPy fills the array from the first
/// two values, `start + i * ((start + step) - start)`, which can differ from `start + i * step`
/// in the last bit; grid lines that touch a polygon edge depend on it.
fn arange(start: f64, stop: f64, step: f64) -> Vec<f64> {
    let n = ((stop - start) / step).ceil();
    if n.is_nan() || n <= 0.0 {
        return Vec::new();
    }
    let delta = (start + step) - start;
    (0..n as usize).map(|i| start + i as f64 * delta).collect()
}

fn polygon(ring: &[Coord<f64>]) -> Geometry<f64> {
    let mut closed = ring.to_vec();
    closed.push(ring[0]);
    Geometry::Polygon(Polygon::new(LineString::new(closed), vec![]))
}

fn point(c: Coord<f64>) -> Geometry<f64> {
    Geometry::Point(Point::from(c))
}

/// A square grid: `x` is the outer loop and `y` the inner one, as in GeoPandas.
fn square(
    bounds: Rect<f64>,
    origin: Coord<f64>,
    cell: f64,
    feature: GridFeature,
) -> Result<Vec<Geometry<f64>>, String> {
    // Count before allocating (float to integer casts saturate, so an overflow stays huge).
    let steps = |lo: f64, hi: f64| (((hi + cell - lo) / cell).ceil().max(0.0)) as u64;
    let cells = steps(origin.x, bounds.max().x).saturating_mul(steps(origin.y, bounds.max().y));
    if cells > MAX_GRID_CELLS {
        return Err(too_large(cells));
    }
    let xs = arange(origin.x, bounds.max().x + cell, cell);
    let ys = arange(origin.y, bounds.max().y + cell, cell);
    let mut out = Vec::new();
    match feature {
        GridFeature::Corners => {
            for &x in &xs {
                for &y in &ys {
                    out.push(point(Coord { x, y }));
                }
            }
        }
        GridFeature::Centers => {
            for &x in xs.iter().take(xs.len().saturating_sub(1)) {
                for &y in ys.iter().take(ys.len().saturating_sub(1)) {
                    out.push(point(Coord {
                        x: x + cell / 2.0,
                        y: y + cell / 2.0,
                    }));
                }
            }
        }
        GridFeature::Polygons => {
            for xw in xs.windows(2) {
                for yw in ys.windows(2) {
                    out.push(polygon(&[
                        Coord { x: xw[0], y: yw[0] },
                        Coord { x: xw[1], y: yw[0] },
                        Coord { x: xw[1], y: yw[1] },
                        Coord { x: xw[0], y: yw[1] },
                    ]));
                }
            }
        }
    }
    Ok(out)
}

/// A NumPy slice `start..stop` with a positive step, clamped to `len`.
fn slice(start: i64, stop: Option<i64>, step: usize, len: usize) -> Vec<usize> {
    let end = stop.map_or(len as i64, |s| s.min(len as i64));
    let mut out = Vec::new();
    let mut i = start.max(0);
    while i < end {
        out.push(i as usize);
        i += step as i64;
    }
    out
}

struct Mesh {
    x: Vec<Vec<f64>>,
    y: Vec<Vec<f64>>,
}

impl Mesh {
    fn at(&self, r: usize, c: usize) -> Coord<f64> {
        Coord {
            x: self.x[r][c],
            y: self.y[r][c],
        }
    }

    /// `np.array([X, Y]).T.reshape(-1, 1, 2)` of a row/column selection: columns outermost.
    fn pick(&self, rows: &[usize], cols: &[usize]) -> Vec<Coord<f64>> {
        cols.iter()
            .flat_map(|&c| rows.iter().map(move |&r| (r, c)))
            .map(|(r, c)| self.at(r, c))
            .collect()
    }
}

/// `_hex_polygon_corners`: the hexagons whose six corners start at mesh index `(i_x0, i_y0)`.
fn hexagon_family(mesh: &Mesh, i_x0: i64, i_y0: i64) -> Result<Vec<[Coord<f64>; 6]>, String> {
    let (n_rows, n_cols) = (mesh.x.len(), mesh.x[0].len());
    let x_corr = i64::from(i_y0 % 2 == 0);
    let count = |start: i64, len: usize, step: i64| -> i64 {
        let remaining = len as i64 - start;
        if remaining <= 0 {
            0
        } else {
            (remaining + step - 1) / step
        }
    };
    let n_poly_x = count(i_x0 + 2 - x_corr, n_cols, 3);
    let max_i_x = i_x0 + n_poly_x * 3 - 1;
    let n_poly_y = count(i_y0 + 2, n_rows, 2);
    let max_i_y = i_y0 + n_poly_y * 2;

    let bottom_left = mesh.pick(
        &slice(i_y0, Some(max_i_y), 2, n_rows),
        &slice(i_x0, Some(max_i_x), 3, n_cols),
    );
    let bottom_right = mesh.pick(
        &slice(i_y0, Some(max_i_y), 2, n_rows),
        &slice(i_x0 + 1, Some(max_i_x), 3, n_cols),
    );
    let middle_right = mesh.pick(
        &slice(i_y0 + 1, Some(max_i_y), 2, n_rows),
        &slice(i_x0 + 2 - x_corr, None, 3, n_cols),
    );
    let top_right = mesh.pick(
        &slice(i_y0 + 2, None, 2, n_rows),
        &slice(i_x0 + 1, Some(max_i_x), 3, n_cols),
    );
    let top_left = mesh.pick(
        &slice(i_y0 + 2, None, 2, n_rows),
        &slice(i_x0, Some(max_i_x), 3, n_cols),
    );
    let middle_left = mesh.pick(
        &slice(i_y0 + 1, Some(max_i_y), 2, n_rows),
        &slice(i_x0 - x_corr, Some(max_i_x), 3, n_cols),
    );
    let n = bottom_left.len();
    if [
        bottom_right.len(),
        middle_right.len(),
        top_right.len(),
        top_left.len(),
        middle_left.len(),
    ]
    .iter()
    .any(|&len| len != n)
    {
        return Err("hexagon grid: corner groups do not line up".to_string());
    }
    Ok((0..n)
        .map(|i| {
            [
                bottom_left[i],
                bottom_right[i],
                middle_right[i],
                top_right[i],
                top_left[i],
                middle_left[i],
            ]
        })
        .collect())
}

fn hexagon(
    bounds: Rect<f64>,
    origin: Coord<f64>,
    cell: f64,
    feature: GridFeature,
    flat_topped: bool,
) -> Result<Vec<Geometry<f64>>, String> {
    let mut x_dist = bounds.max().x - origin.x;
    let mut y_dist = bounds.max().y - origin.y;
    if !flat_topped {
        std::mem::swap(&mut x_dist, &mut y_dist);
    }
    let dx = cell / 3.0_f64.sqrt();
    let dy = cell / 2.0;
    let n_x = (((x_dist / dx - 1.0) / 1.5).ceil() + 2.0) * 1.5;
    let n_y = ((y_dist / dy) as i64).saturating_add(4);
    let (n_x, n_y) = (n_x as i64, n_y);
    if n_x <= 0 || n_y <= 0 {
        return Ok(Vec::new());
    }
    let (n_x, n_y) = (n_x as usize, n_y as usize);
    let cells = (n_x as u64).saturating_mul(n_y as u64);
    if cells > MAX_GRID_CELLS {
        return Err(too_large(cells));
    }

    let mesh = Mesh {
        x: (0..n_y)
            .map(|r| {
                (0..n_x)
                    .map(|c| {
                        let x = c as f64 * dx;
                        if r % 2 == 0 {
                            x - dx / 2.0
                        } else {
                            x
                        }
                    })
                    .collect()
            })
            .collect(),
        y: (0..n_y).map(|r| vec![r as f64 * dy - dy; n_x]).collect(),
    };
    let shift = |c: Coord<f64>| {
        let c = if flat_topped {
            c
        } else {
            Coord { x: c.y, y: c.x }
        };
        Coord {
            x: c.x + origin.x,
            y: c.y + origin.y,
        }
    };
    let is_center = |r: usize, c: usize| {
        if r % 2 == 1 {
            c % 3 == 2
        } else {
            c % 3 == 1
        }
    };

    Ok(match feature {
        GridFeature::Centers | GridFeature::Corners => {
            let want_center = feature == GridFeature::Centers;
            let mut out = Vec::new();
            for r in 0..n_y {
                for c in 0..n_x {
                    if is_center(r, c) == want_center {
                        out.push(point(shift(mesh.at(r, c))));
                    }
                }
            }
            out
        }
        GridFeature::Polygons => {
            let mut hexes = hexagon_family(&mesh, 0, 1)?;
            hexes.extend(hexagon_family(&mesh, 2, 0)?);
            hexes
                .into_iter()
                .map(|corners| polygon(&corners.map(shift)))
                .collect()
        }
    })
}

/// Everything `make_grid` needs besides the geometry.
pub(super) struct GridSpec {
    pub cell_size: f64,
    pub grid: GridType,
    pub feature: GridFeature,
    pub offset: Option<(f64, f64)>,
    pub intersect: bool,
    pub flat_topped: bool,
}

/// The grid over `bounds`, optionally keeping only elements that intersect one of `inputs`.
pub(super) fn make_grid(
    bounds: Rect<f64>,
    inputs: &[Geometry<f64>],
    spec: &GridSpec,
) -> Result<Vec<Geometry<f64>>, String> {
    let cell_size = spec.cell_size;
    if cell_size.is_nan() || cell_size <= 0.0 || cell_size.is_infinite() {
        return Err(format!("cell_size should be positive, got {cell_size}"));
    }
    let origin = spec.offset.map_or(bounds.min(), |(x, y)| Coord { x, y });
    let cells = match spec.grid {
        GridType::Square => square(bounds, origin, cell_size, spec.feature)?,
        GridType::Hexagon => hexagon(bounds, origin, cell_size, spec.feature, spec.flat_topped)?,
    };
    if !spec.intersect {
        return Ok(cells);
    }
    let boxes: Vec<Option<Rect<f64>>> = inputs.iter().map(|g| g.bounding_rect()).collect();
    Ok(cells
        .into_iter()
        .filter(|cell| {
            let Some(cell_box) = cell.bounding_rect() else {
                return false;
            };
            inputs.iter().zip(&boxes).any(|(input, input_box)| {
                input_box.is_some_and(|b| b.intersects(&cell_box)) && input.intersects(cell)
            })
        })
        .collect())
}
