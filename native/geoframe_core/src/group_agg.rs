//! Per-group geometry aggregation in one pass: `aggregate_groups` takes a geometry column and
//! a group id per row and returns one geometry per group, each exactly what aggregating the
//! group's rows alone with the whole-column method gives.

use crate::unary::UnaryOp;
use crate::GeoColumn;

/// A geometry aggregation applied to each group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupAgg {
    /// [`GeoColumn::union_all`] of the group.
    UnionAll,
    /// [`GeoColumn::intersection_all`] of the group.
    IntersectionAll,
    /// [`GeoColumn::collect`] of the group (`multi = false`).
    Collect,
    /// The envelope of the group's total bounds (its collection's `envelope`).
    Envelope,
    /// The convex hull of all the group's geometries.
    ConvexHull,
    /// The centroid of the group's union.
    Centroid,
    /// The first non-null geometry of the group.
    First,
    /// The last non-null geometry of the group.
    Last,
}

impl GroupAgg {
    /// Parses the name used by the NIF layer (`"union_all"`, `"convex_hull"`, ...).
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "union_all" => Self::UnionAll,
            "intersection_all" => Self::IntersectionAll,
            "collect" => Self::Collect,
            "envelope" => Self::Envelope,
            "convex_hull" => Self::ConvexHull,
            "centroid" => Self::Centroid,
            "first" => Self::First,
            "last" => Self::Last,
            _ => return None,
        })
    }
}

impl GeoColumn {
    /// Row `i` belongs to group `group_ids[i]` (`< n_groups`); the result has one row per
    /// group with the same CRS. A group without rows or with only null rows gives what the
    /// whole-column method gives for no geometries (`GEOMETRYCOLLECTION EMPTY` for the
    /// unions, intersection and collection, null for the rest).
    pub fn aggregate_groups(
        &self,
        group_ids: &[u32],
        n_groups: usize,
        agg: GroupAgg,
    ) -> Result<GeoColumn, String> {
        if agg == GroupAgg::UnionAll {
            return self.union_groups(group_ids, n_groups);
        }
        if group_ids.len() != self.len() {
            return Err(format!(
                "group_ids has {} entries but the column has {} rows",
                group_ids.len(),
                self.len()
            ));
        }
        if n_groups > group_ids.len() {
            return Err(format!(
                "n_groups ({n_groups}) cannot exceed the number of group ids ({})",
                group_ids.len()
            ));
        }
        if let Some(&id) = group_ids.iter().find(|&&id| id as usize >= n_groups) {
            return Err(format!("group id {id} out of range for {n_groups} groups"));
        }
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); n_groups];
        for (row, &id) in group_ids.iter().enumerate() {
            members[id as usize].push(row);
        }
        if n_groups == 0 {
            return self.take(&[]);
        }
        let pieces = members
            .iter()
            .map(|rows| self.aggregate_rows(rows, agg))
            .collect::<Result<Vec<_>, _>>()?;
        GeoColumn::concat(&pieces.iter().collect::<Vec<_>>())
    }

    /// One group: a one-row column.
    fn aggregate_rows(&self, rows: &[usize], agg: GroupAgg) -> Result<GeoColumn, String> {
        let group = self.take(rows)?;
        match agg {
            GroupAgg::UnionAll => group.union_all(),
            GroupAgg::IntersectionAll => group.intersection_all(),
            GroupAgg::Collect => group.collect(false),
            GroupAgg::Envelope => group.collect(false)?.unary(UnaryOp::Envelope),
            GroupAgg::ConvexHull => group.collect(false)?.unary(UnaryOp::ConvexHull),
            GroupAgg::Centroid => group.union_all()?.centroid(),
            GroupAgg::First | GroupAgg::Last => {
                let present: Vec<usize> = group
                    .null_rows()
                    .iter()
                    .enumerate()
                    .filter(|(_, null)| !**null)
                    .map(|(i, _)| i)
                    .collect();
                let pick = if agg == GroupAgg::First {
                    present.first()
                } else {
                    present.last()
                };
                match pick {
                    Some(&i) => group.take(&[i]),
                    None => group.rebuild(&[None]),
                }
            }
        }
    }
}
