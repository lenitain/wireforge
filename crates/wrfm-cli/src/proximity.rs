//! Point-identity tolerance and the shared spatial grid used to find
//! near-coincident vertices.
//!
//! `POINT_TOL` is the CLI's single definition of "the same point": two
//! vertices closer than this (world units) are near-duplicates in `check`
//! and are merged by `edit --weld`. Both use [`Grid`], so they always agree
//! on which pairs count.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// Points closer than this (world units, strict `<`) count as the same
/// point. Absolute, not relative: models whose units are far from 1 should
/// pass an explicit `--weld TOL` instead of relying on this default.
pub const POINT_TOL: f64 = 1e-6;

/// One cell's point indices.
///
/// A single point is the common case — near-duplicates are rare, so almost every
/// cell holds exactly one — so it lives inline and only spills to a heap `Vec`
/// when a second point lands in the same cell. This drops the one-allocation-per-
/// vertex the old always-`Vec` form paid on the common path.
enum Cell {
    One(usize),
    Many(Vec<usize>),
}

impl Cell {
    /// Append `index`, promoting the inline single value to a heap `Vec` only
    /// when a second point actually arrives.
    fn push(&mut self, index: usize) {
        if let Cell::Many(v) = self {
            v.push(index);
            return;
        }
        let first = match std::mem::replace(self, Cell::Many(Vec::new())) {
            Cell::One(i) => i,
            Cell::Many(v) => {
                // Unreachable: the `if let` above already routed `Many` here.
                *self = Cell::Many(v);
                return;
            }
        };
        *self = Cell::Many(vec![first, index]);
    }

    /// The cell's indices as a slice, so the 27-cell probe iterates one shape.
    fn as_slice(&self) -> &[usize] {
        match self {
            Cell::One(j) => std::slice::from_ref(j),
            Cell::Many(v) => v,
        }
    }
}

/// Uniform spatial hash with a fixed cell size.
///
/// Inserted points are bucketed by `floor(coord / cell)`; a query probes the
/// 27 neighbouring cells, so a pair straddling a cell boundary is still
/// found. A single-cell probe silently misses those pairs (regression test:
/// `straddling_a_cell_boundary_is_found`).
///
/// Construct with the same tolerance used for queries (`Grid::new(tol)`):
/// the 27-cell probe only covers every point within `tol` when the cell size
/// equals the query tolerance.
pub struct Grid {
    cell: f64,
    cells: HashMap<(i64, i64, i64), Cell>,
}

impl Grid {
    pub fn new(cell: f64) -> Self {
        debug_assert!(
            cell.is_finite() && cell > 0.0,
            "grid cell size must be finite and > 0"
        );
        Self {
            cell,
            cells: HashMap::new(),
        }
    }

    fn key(&self, p: (f64, f64, f64)) -> (i64, i64, i64) {
        let inv = 1.0 / self.cell;
        (
            (p.0 * inv).floor() as i64,
            (p.1 * inv).floor() as i64,
            (p.2 * inv).floor() as i64,
        )
    }

    /// Record `index` (an index into the point list the caller is scanning).
    pub fn insert(&mut self, index: usize, p: (f64, f64, f64)) {
        match self.cells.entry(self.key(p)) {
            Entry::Vacant(e) => {
                e.insert(Cell::One(index));
            }
            Entry::Occupied(mut e) => e.get_mut().push(index),
        }
    }

    /// Reserve cell capacity for `additional` more points before a bulk load.
    ///
    /// A spatial hash grows by repeated rehash, and a rehash keeps the old and
    /// new bucket arrays live at once — a transient peak above the steady state
    /// that massif flags as the top allocator for large `check` runs. Reserving
    /// up front trades that spike for one exact-size table.
    pub fn reserve(&mut self, additional: usize) {
        self.cells.reserve(additional);
    }

    /// The point indices already inserted into `p`'s own cell, in ascending
    /// index order.
    ///
    /// A point's bit-identical twin shares its cell — identical coordinates
    /// hash to the same key — so `check` finds exact duplicates by scanning
    /// this slice instead of a separate `bits -> index` table. Ascending order
    /// means a scan's first match is the lowest-index match.
    pub fn indices_in_cell(&self, p: (f64, f64, f64)) -> &[usize] {
        self.cells
            .get(&self.key(p))
            .map(|c| c.as_slice())
            .unwrap_or(&[])
    }

    /// The LOWEST index among the inserted points strictly closer than `tol`
    /// to `p`, or `None`. `points` maps every index back to its coordinates.
    ///
    /// Lowest-index ("first touch") makes the answer independent of hash
    /// iteration order: the survivor of a cluster is always its earliest
    /// member.
    pub fn nearest_within(
        &self,
        p: (f64, f64, f64),
        points: &[(f64, f64, f64)],
        tol: f64,
    ) -> Option<usize> {
        let tol2 = tol * tol;
        let (kx, ky, kz) = self.key(p);
        let mut best: Option<usize> = None;
        for dx in -1i64..=1 {
            for dy in -1i64..=1 {
                for dz in -1i64..=1 {
                    let cell = (
                        kx.saturating_add(dx),
                        ky.saturating_add(dy),
                        kz.saturating_add(dz),
                    );
                    let Some(cands) = self.cells.get(&cell) else {
                        continue;
                    };
                    for &j in cands.as_slice() {
                        let q = points[j];
                        let d2 = (p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2);
                        if d2 < tol2 {
                            best = Some(best.map_or(j, |b| b.min(j)));
                        }
                    }
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(v: &[(f64, f64, f64)]) -> Vec<(f64, f64, f64)> {
        v.to_vec()
    }

    #[test]
    fn straddling_a_cell_boundary_is_found() {
        // BUCKET-style hashing rounds, so points 2e-7 apart land in
        // DIFFERENT cells when they straddle a half-cell boundary; only the
        // 27-cell probe finds them.
        let tol = 1e-4;
        let points = pts(&[(tol * 0.5 - 1e-7, 0.0, 0.0), (tol * 0.5 + 1e-7, 0.0, 0.0)]);
        let mut g = Grid::new(tol);
        g.insert(0, points[0]);
        assert_eq!(g.nearest_within(points[1], &points, tol), Some(0));
    }

    #[test]
    fn strictly_closer_than_tol() {
        let tol = 1e-6;
        let points = pts(&[(0.0, 0.0, 0.0), (tol, 0.0, 0.0)]);
        let mut g = Grid::new(tol);
        g.insert(0, points[0]);
        assert_eq!(
            g.nearest_within(points[1], &points, tol),
            None,
            "a pair exactly tol apart is not the same point"
        );
    }

    #[test]
    fn lowest_index_wins() {
        let points = pts(&[(0.0, 0.0, 0.0), (1e-9, 0.0, 0.0), (2e-9, 0.0, 0.0)]);
        let mut g = Grid::new(POINT_TOL);
        g.insert(1, points[1]);
        g.insert(0, points[0]);
        g.insert(2, points[2]);
        assert_eq!(g.nearest_within(points[2], &points, POINT_TOL), Some(0));
    }

    #[test]
    fn far_points_are_not_candidates() {
        let points = pts(&[(0.0, 0.0, 0.0), (1.0, 0.0, 0.0)]);
        let mut g = Grid::new(POINT_TOL);
        g.insert(0, points[0]);
        assert_eq!(g.nearest_within(points[1], &points, POINT_TOL), None);
    }
}
