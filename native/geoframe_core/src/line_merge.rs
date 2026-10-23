//! `line_merge`: joins line parts that meet end to end into longer lines.
//!
//! Endpoints are nodes of a graph. A node joins two parts only when exactly two part-ends
//! meet there, so a T-junction (three ends) stays a break. Undirected, the parts may be
//! flipped to fit; directed, the two ends must be one end and one start.
//!
//! Output is a `LineString` when everything merges into one chain, otherwise a
//! `MultiLineString` of chains in the order of their first part. A geometry with no lines
//! gives an empty `GeometryCollection` (Shapely/GEOS). Nodes are compared bit for bit.

use std::collections::HashMap;

use geo::{Coord, Geometry, GeometryCollection, LineString, MultiLineString};

use crate::GeoColumn;

type Node = (u64, u64);

fn node(c: Coord<f64>) -> Node {
    // `+ 0.0` folds -0.0 into 0.0 so both spellings of zero are one node.
    ((c.x + 0.0).to_bits(), (c.y + 0.0).to_bits())
}

/// Collects every line of a geometry, descending into collections. Polygons and points have
/// none.
fn collect_lines(g: &Geometry<f64>, out: &mut Vec<LineString<f64>>) {
    match g {
        Geometry::LineString(l) => out.push(l.clone()),
        Geometry::Line(l) => out.push(LineString::from(vec![l.start, l.end])),
        Geometry::MultiLineString(m) => out.extend(m.0.iter().cloned()),
        Geometry::GeometryCollection(c) => c.0.iter().for_each(|m| collect_lines(m, out)),
        _ => {}
    }
}

/// A part-end meeting at a node: which part, and whether it is that part's start.
#[derive(Clone, Copy)]
struct End {
    part: usize,
    is_start: bool,
}

struct Graph<'a> {
    parts: &'a [LineString<f64>],
    ends: HashMap<Node, Vec<End>>,
    directed: bool,
}

impl<'a> Graph<'a> {
    fn new(parts: &'a [LineString<f64>], directed: bool) -> Self {
        let mut ends: HashMap<Node, Vec<End>> = HashMap::new();
        for (part, line) in parts.iter().enumerate() {
            ends.entry(node(line.0[0])).or_default().push(End {
                part,
                is_start: true,
            });
            ends.entry(node(*line.0.last().unwrap()))
                .or_default()
                .push(End {
                    part,
                    is_start: false,
                });
        }
        Self {
            parts,
            ends,
            directed,
        }
    }

    /// The part to continue with after leaving `part` at its end (`at_tail`) or start, if the
    /// node is a merge node.
    fn next(&self, part: usize, at_tail: bool) -> Option<End> {
        let line = &self.parts[part];
        let c = if at_tail { *line.0.last()? } else { line.0[0] };
        let here = self.ends.get(&node(c))?;
        if here.len() != 2 {
            return None;
        }
        let this = here
            .iter()
            .position(|e| e.part == part && e.is_start == !at_tail)?;
        let other = here[1 - this];
        // Directed: leaving a tail must enter a start, leaving a head must meet a tail.
        if self.directed && other.is_start != at_tail {
            return None;
        }
        Some(other)
    }
}

fn merge(parts: &[LineString<f64>], directed: bool) -> Vec<LineString<f64>> {
    let graph = Graph::new(parts, directed);
    let mut used = vec![false; parts.len()];
    let mut chains = Vec::new();
    for first in 0..parts.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut forward: Vec<Coord<f64>> = parts[first].0.clone();
        // Extend past the tail.
        let (mut part, mut at_tail) = (first, true);
        while let Some(next) = graph.next(part, at_tail) {
            if used[next.part] {
                break;
            }
            used[next.part] = true;
            let coords = &parts[next.part].0;
            if next.is_start {
                forward.extend(coords.iter().skip(1));
            } else {
                forward.extend(coords.iter().rev().skip(1));
            }
            (part, at_tail) = (next.part, next.is_start);
        }
        // Extend before the head.
        let mut backward: Vec<Coord<f64>> = Vec::new();
        let (mut part, mut at_tail) = (first, false);
        while let Some(next) = graph.next(part, at_tail) {
            if used[next.part] {
                break;
            }
            used[next.part] = true;
            let coords = &parts[next.part].0;
            if next.is_start {
                backward.extend(coords.iter().skip(1));
            } else {
                backward.extend(coords.iter().rev().skip(1));
            }
            (part, at_tail) = (next.part, next.is_start);
        }
        backward.reverse();
        backward.extend(forward);
        chains.push(LineString::new(backward));
    }
    chains
}

fn merge_geometry(g: &Geometry<f64>, directed: bool) -> Geometry<f64> {
    if let Geometry::LineString(_) = g {
        return g.clone();
    }
    let mut lines = Vec::new();
    collect_lines(g, &mut lines);
    lines.retain(|l| l.0.len() >= 2);
    if lines.is_empty() {
        return Geometry::GeometryCollection(GeometryCollection(Vec::new()));
    }
    let mut chains = merge(&lines, directed);
    if chains.len() == 1 {
        Geometry::LineString(chains.remove(0))
    } else {
        Geometry::MultiLineString(MultiLineString::new(chains))
    }
}

impl GeoColumn {
    /// Merges connected line parts (see the module notes). A single `LineString` is returned
    /// unchanged.
    pub fn line_merge(&self, directed: bool) -> Result<Self, String> {
        self.map_geometry(|g| Some(merge_geometry(g, directed)))
    }
}
