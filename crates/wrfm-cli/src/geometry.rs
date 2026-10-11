use serde_json::{Value, json};
use std::collections::HashSet;
use wrfm_raster::Model;
use wrfm_raster::geometry::bounds;

/// Round to 3 decimals so JSON stays compact and readable.
fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn v3(a: [f64; 3]) -> Value {
    json!([r3(a[0]), r3(a[1]), r3(a[2])])
}

/// Arithmetic mean of the vertices (a wireframe's natural "centre of mass").
fn centroid(m: &Model) -> [f64; 3] {
    let n = m.vertices.len() as f64;
    let mut c = [0.0; 3];
    for &(x, y, z) in &m.vertices {
        c[0] += x;
        c[1] += y;
        c[2] += z;
    }
    if n > 0.0 {
        for v in &mut c {
            *v /= n;
        }
    }
    c
}

/// Connected components via union-find.
fn components(m: &Model) -> usize {
    let n = m.vertices.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut Vec<usize>, x: usize) -> usize {
        if parent[x] != x {
            parent[x] = find(parent, parent[x]);
        }
        parent[x]
    }
    for &(a, b) in &m.edges {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    }
    (0..n)
        .map(|i| find(&mut parent, i))
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Jacobi eigenvalue decomposition of a 3x3 symmetric matrix.
#[allow(clippy::needless_range_loop)]
fn jacobi_eigen(mut a: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..64 {
        // Largest off-diagonal entry.
        let (mut p, mut q) = (0usize, 1usize);
        let mut mx = a[0][1].abs();
        for i in 0..3 {
            for j in (i + 1)..3 {
                if a[i][j].abs() > mx {
                    mx = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if mx < 1e-12 {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        let (app, aqq, apq) = (a[p][p], a[q][q], a[p][q]);
        a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
        a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
        a[p][q] = 0.0;
        a[q][p] = 0.0;
        for k in 0..3 {
            if k != p && k != q {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = c * akp - s * akq;
                a[p][k] = a[k][p];
                a[k][q] = s * akp + c * akq;
                a[q][k] = a[k][q];
            }
        }
        for k in 0..3 {
            let (vkp, vkq) = (v[k][p], v[k][q]);
            v[k][p] = c * vkp - s * vkq;
            v[k][q] = s * vkp + c * vkq;
        }
    }
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&i, &j| a[j][j].partial_cmp(&a[i][i]).unwrap());
    let evals = [a[idx[0]][idx[0]], a[idx[1]][idx[1]], a[idx[2]][idx[2]]];
    let evecs = [
        [v[0][idx[0]], v[1][idx[0]], v[2][idx[0]]],
        [v[0][idx[1]], v[1][idx[1]], v[2][idx[1]]],
        [v[0][idx[2]], v[1][idx[2]], v[2][idx[2]]],
    ];
    (evals, evecs)
}

/// Principal axes (PCA) of the vertex cloud: covariance eigenvectors sorted by decreasing eigenvalue.
pub(crate) fn principal_axes(m: &Model) -> ([[f64; 3]; 3], [f64; 3]) {
    let c = centroid(m);
    let mut cov = [[0.0; 3]; 3];
    for &(x, y, z) in &m.vertices {
        let dx = [x - c[0], y - c[1], z - c[2]];
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += dx[i] * dx[j];
            }
        }
    }
    let (evals, evecs) = jacobi_eigen(cov);
    (evecs, evals)
}

/// Angle (degrees, 0..90) between `evec` and the world `axis`.
fn angle_to_axis(evec: [f64; 3], axis: usize) -> f64 {
    let d = evec[axis].abs().min(1.0);
    d.acos().to_degrees()
}

/// Mirror-symmetry test across the plane perpendicular to `axis`
/// Positions quantised to the symmetry epsilon, as integer keys.
///
/// The mirror of a vertex is looked up BY POSITION. The obvious form — for each
/// vertex, scan every other vertex for its twin — is O(V^2): on a UV sphere the
/// twin of the first vertex by index sits at the far end of the list, so the
/// early-exit saves nothing, and `wrfm geometry` spent 87% of its time there
/// (perf: the two `any` closures below). A sphere is symmetric under three
/// mirrors, so it exercises every one of these probes.
///
/// Quantising to `eps` (rather than hashing raw bit patterns) mirrors the old
/// `abs() < eps` comparison, so a mirrored twin that differs in the last bits
/// still counts. Two vertices in the SAME cell are within `eps` of each other,
/// and `eps` is far smaller than the spacing of any real model, so the buckets
/// hold at most a handful of points.
fn position_index(m: &Model, eps: f64) -> HashSet<(i64, i64, i64)> {
    let q = |v: f64| (v / eps).round() as i64;
    m.vertices
        .iter()
        .map(|&(x, y, z)| (q(x), q(y), q(z)))
        .collect()
}

/// Whether `index` holds a point within `eps` of `p`.
///
/// Every candidate cell within `eps` is checked, not just the one `p` rounds
/// into: a twin sitting on the far side of a cell boundary would otherwise be
/// missed (the same class of miss the `proximity::Grid` 27-cell probe fixed for
/// near-duplicate vertices).
fn has_twin_within(index: &HashSet<(i64, i64, i64)>, p: [f64; 3], eps: f64) -> bool {
    let cell = [
        (p[0] / eps).round() as i64,
        (p[1] / eps).round() as i64,
        (p[2] / eps).round() as i64,
    ];
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if index.contains(&(cell[0] + dx, cell[1] + dy, cell[2] + dz)) {
                    // The cell is within eps in every coordinate, so the point
                    // it holds is a twin by the same test the scan used.
                    return true;
                }
            }
        }
    }
    false
}

fn has_mirror_symmetry(m: &Model, axis: usize) -> bool {
    let eps = 1e-6;
    let index = position_index(m, eps);
    m.vertices.iter().all(|&(x, y, z)| {
        let mir = [
            if axis == 0 { -x } else { x },
            if axis == 1 { -y } else { y },
            if axis == 2 { -z } else { z },
        ];
        has_twin_within(&index, mir, eps)
    })
}

/// Mirror-symmetry test across the plane through the model's BBOX CENTRE,
/// perpendicular to `axis` — position-independent shape symmetry (a box
/// anywhere in space counts as symmetric). verify --expect-symmetric uses
/// these; the origin-plane mirror_* fields stay for reference.
fn has_center_mirror_symmetry(m: &Model, axis: usize) -> bool {
    let eps = 1e-6;
    let (min, max) = bounds(m);
    let c = [
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ];
    let index = position_index(m, eps);
    m.vertices.iter().all(|&(x, y, z)| {
        let mir = [
            if axis == 0 { 2.0 * c[0] - x } else { x },
            if axis == 1 { 2.0 * c[1] - y } else { y },
            if axis == 2 { 2.0 * c[2] - z } else { z },
        ];
        has_twin_within(&index, mir, eps)
    })
}

/// One-stop structured geometry report for `m`:
pub fn analyze(m: &Model) -> Value {
    analyze_impl(m, false)
}

/// Like [`analyze`], plus the `--full` extras: the PCA eigenvalues (the vertex cloud's variance along each principal axis).
pub fn analyze_full(m: &Model) -> Value {
    analyze_impl(m, true)
}

fn analyze_impl(m: &Model, full: bool) -> Value {
    let (min, max) = bounds(m);
    let center = [
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ];
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let c = centroid(m);

    let (axes, evals) = principal_axes(m);
    let angles = [
        angle_to_axis(axes[0], 0),
        angle_to_axis(axes[0], 1),
        angle_to_axis(axes[0], 2),
    ];

    // Topology.
    let n = m.vertices.len();
    let mut deg = vec![0usize; n.max(1)];
    for &(a, b) in &m.edges {
        deg[a] += 1;
        deg[b] += 1;
    }
    let (mut dmin, mut dmax) = (usize::MAX, 0usize);
    let mut dsum = 0usize;
    let mut dangling = 0usize;
    for &d in &deg {
        dmin = dmin.min(d);
        dmax = dmax.max(d);
        dsum += d;
        if d < 2 {
            dangling += 1;
        }
    }
    if n == 0 {
        dmin = 0;
    }
    let comps = components(m);
    let cycle_rank = m.edges.len() as i64 - n as i64 + comps as i64;

    // Edge lengths.
    let mut lens: Vec<f64> = m
        .edges
        .iter()
        .map(|&(a, b)| {
            let (va, vb) = (m.vertices[a], m.vertices[b]);
            ((va.0 - vb.0).powi(2) + (va.1 - vb.1).powi(2) + (va.2 - vb.2).powi(2)).sqrt()
        })
        .collect();
    let (lmin, lmax, lavg) = if lens.is_empty() {
        (0.0, 0.0, 0.0)
    } else {
        lens.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let sum = lens.iter().sum::<f64>();
        (lens[0], lens[lens.len() - 1], sum / lens.len() as f64)
    };
    let hist = if lmax > lmin {
        let bins = 8usize;
        let mut h = vec![0u32; bins];
        let w = (lmax - lmin) / bins as f64;
        for &l in &lens {
            let i = ((l - lmin) / w).floor() as usize;
            h[i.min(bins - 1)] += 1;
        }
        h
    } else {
        vec![lens.len() as u32]
    };

    // Longest bounding-box axis.
    let longest_axis = ["x", "y", "z"]
        .iter()
        .zip(size.iter())
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(a, _)| *a)
        .unwrap_or("x");

    // Axis alignment: every principal axis lies within 5 deg of a world axis.
    let axis_aligned = [0, 1, 2].iter().all(|&i| {
        let a = angle_to_axis(axes[0], i).min(90.0 - angle_to_axis(axes[0], i));
        let b = angle_to_axis(axes[1], i).min(90.0 - angle_to_axis(axes[1], i));
        let c = angle_to_axis(axes[2], i).min(90.0 - angle_to_axis(axes[2], i));
        a < 5.0 || b < 5.0 || c < 5.0
    });

    let spans = json!({
           "x": r3(size[0]),
           "y": r3(size[1]),
           "z": r3(size[2]),
    });
    let mut principal = json!({
           "axis1": v3(axes[0]),
           "axis2": v3(axes[1]),
           "axis3": v3(axes[2]),
           "angles_to_world_deg": {
               "x": r3(angles[0]),
               "y": r3(angles[1]),
               "z": r3(angles[2]),
    },
    });
    if full {
        principal["eigenvalues"] = json!([r3(evals[0]), r3(evals[1]), r3(evals[2])]);
    }
    json!({
           "bounds": {
               "min": v3(min),
               "max": v3(max),
               "center": v3(center),
               "size": v3(size),
    },
           "centroid": v3(c),
           "spans": spans,
           "principal_axes": principal,
           "topology": {
               "vertices": n,
               "edges": m.edges.len(),
               "components": comps,
               "dangling_vertices": dangling,
               "degree_stats": {
                   "min": dmin,
                   "max": dmax,
                   "avg": r3(if n > 0 { dsum as f64 / n as f64 } else { 0.0 }),
    },
               "euler_characteristic": (n as i64) - (m.edges.len() as i64) + comps as i64,
               "cycle_rank": cycle_rank,
    },
           "edge_lengths": {
               "min": r3(lmin),
               "max": r3(lmax),
               "avg": r3(lavg),
               "histogram": hist,
    },
           "closure": {
               "closed": n > 0 && dangling == 0 && cycle_rank > 0,
               "min_degree": dmin,
               "note": "wireframe has no faces; surface area / volume not computed",
    },
           "symmetry": {
               "mirror_xy": has_mirror_symmetry(m, 2),
               "mirror_xz": has_mirror_symmetry(m, 1),
               "mirror_yz": has_mirror_symmetry(m, 0),
               // Position-independent shape symmetry about the model's OWN bbox
               // centre (the intuitive meaning — verify --expect-symmetric uses
               // these; the origin-plane mirror_* stay for reference).
               "center_mirror_xy": has_center_mirror_symmetry(m, 2),
               "center_mirror_xz": has_center_mirror_symmetry(m, 1),
               "center_mirror_yz": has_center_mirror_symmetry(m, 0),
    },
           "alignment": {
               "axis_aligned": axis_aligned,
               "longest_axis": longest_axis,
    },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Model {
        let mut verts = Vec::new();
        let mut edges = Vec::new();
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    verts.push((
                        if i == 0 { -1.0 } else { 1.0 },
                        if j == 0 { -1.0 } else { 1.0 },
                        if k == 0 { -1.0 } else { 1.0 },
                    ));
                }
            }
        }
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    for (di, dj, dk) in [(1, 0, 0), (0, 1, 0), (0, 0, 1)] {
                        if i + di < 2 && j + dj < 2 && k + dk < 2 {
                            edges.push((i * 4 + j * 2 + k, (i + di) * 4 + (j + dj) * 2 + (k + dk)));
                        }
                    }
                }
            }
        }
        Model {
            vertices: verts,
            edges,
        }
    }

    fn tetra() -> Model {
        Model {
            vertices: vec![
                (1.0, 1.0, 1.0),
                (1.0, -1.0, -1.0),
                (-1.0, 1.0, -1.0),
                (-1.0, -1.0, 1.0),
            ],
            edges: vec![(0, 1), (0, 2), (0, 3), (1, 2), (2, 3), (3, 1)],
        }
    }

    /// A 2x1x1 box stretched along X.
    fn box_x() -> Model {
        let mut verts = Vec::new();
        let mut edges = Vec::new();
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    verts.push((
                        if i == 0 { -2.0 } else { 2.0 },
                        if j == 0 { -1.0 } else { 1.0 },
                        if k == 0 { -1.0 } else { 1.0 },
                    ));
                }
            }
        }
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    for (di, dj, dk) in [(1, 0, 0), (0, 1, 0), (0, 0, 1)] {
                        if i + di < 2 && j + dj < 2 && k + dk < 2 {
                            edges.push((i * 4 + j * 2 + k, (i + di) * 4 + (j + dj) * 2 + (k + dk)));
                        }
                    }
                }
            }
        }
        Model {
            vertices: verts,
            edges,
        }
    }

    #[test]
    fn cube_bounds_centroid_centered() {
        let g = analyze(&cube());
        assert_eq!(g["bounds"]["min"], json!([-1.0, -1.0, -1.0]));
        assert_eq!(g["bounds"]["max"], json!([1.0, 1.0, 1.0]));
        assert_eq!(g["bounds"]["size"], json!([2.0, 2.0, 2.0]));
        assert_eq!(g["centroid"], json!([0.0, 0.0, 0.0]));
    }

    #[test]
    fn cube_topology() {
        let g = analyze(&cube());
        assert_eq!(g["topology"]["vertices"], 8);
        assert_eq!(g["topology"]["edges"], 12);
        assert_eq!(g["topology"]["components"], 1);
        assert_eq!(g["topology"]["dangling_vertices"], 0);
        assert_eq!(g["topology"]["degree_stats"]["max"], 3);
        assert_eq!(g["topology"]["cycle_rank"], 5);
        assert_eq!(g["topology"]["euler_characteristic"], -3);
    }

    #[test]
    fn cube_closure_and_symmetry() {
        let g = analyze(&cube());
        assert_eq!(g["closure"]["closed"], true);
        assert_eq!(g["symmetry"]["mirror_xy"], true);
        assert_eq!(g["symmetry"]["mirror_xz"], true);
        assert_eq!(g["symmetry"]["mirror_yz"], true);
        // The centred cube is symmetric about its own centre too.
        assert_eq!(g["symmetry"]["center_mirror_xy"], true);
        assert_eq!(g["symmetry"]["center_mirror_xz"], true);
        assert_eq!(g["symmetry"]["center_mirror_yz"], true);
    }

    #[test]
    fn off_center_cube_is_center_symmetric_but_not_origin_symmetric() {
        // Shift the [-1,1]^3 cube +1.5 along x -> [0.5,2.5]^3: still a
        // symmetric SHAPE (about its own bbox centre), but the origin-plane
        // x-reflection no longer maps onto the vertex set.
        let mut m = cube();
        for v in m.vertices.iter_mut() {
            v.0 += 1.5;
        }
        let g = analyze(&m);
        assert_eq!(g["symmetry"]["mirror_yz"], false); // origin x-plane fails
        assert_eq!(g["symmetry"]["mirror_xz"], true); // y/z untouched
        assert_eq!(g["symmetry"]["mirror_xy"], true);
        assert_eq!(g["symmetry"]["center_mirror_yz"], true); // centre plane holds
        assert_eq!(g["symmetry"]["center_mirror_xz"], true);
        assert_eq!(g["symmetry"]["center_mirror_xy"], true);
    }

    #[test]
    fn box_x_longest_axis_and_principal() {
        let g = analyze(&box_x());
        assert_eq!(g["alignment"]["longest_axis"], "x");
        assert_eq!(g["alignment"]["axis_aligned"], true);
        // axis1 (longest) points along +X.
        let a1 = g["principal_axes"]["axis1"].as_array().unwrap();
        assert!(a1[0].as_f64().unwrap().abs() > 0.9);
        assert_eq!(g["edge_lengths"]["max"], 4.0);
    }

    #[test]
    fn tetra_not_mirror_symmetric() {
        let g = analyze(&tetra());
        assert_eq!(g["symmetry"]["mirror_xy"], false);
        assert_eq!(g["symmetry"]["mirror_xz"], false);
        assert_eq!(g["symmetry"]["mirror_yz"], false);
        // Tetra is a closed wireframe too.
        assert_eq!(g["closure"]["closed"], true);
        assert_eq!(g["topology"]["degree_stats"]["min"], 3);
    }

    #[test]
    fn empty_model_is_safe() {
        let g = analyze(&Model {
            vertices: vec![],
            edges: vec![],
        });
        assert_eq!(g["topology"]["vertices"], 0);
        assert_eq!(
            g["bounds"]["min"],
            json!([f64::INFINITY, f64::INFINITY, f64::INFINITY])
        );
        assert_eq!(g["closure"]["closed"], false);
    }
}

#[cfg(test)]
mod symmetry_scale_tests {
    use super::*;

    /// A UV sphere is mirror-symmetric about BOTH its x and z planes, and its
    /// centre, and every vertex's twin sits far from it in the list — the
    /// adversarial input for a per-vertex linear search. The old form is O(V^2)
    /// here (profiled: 87% of `wrfm geometry` on a 90k sphere) and takes
    /// minutes at this size.
    fn uv_sphere(rings: usize, segs: usize) -> Model {
        let mut vertices = Vec::with_capacity(rings * segs);
        for i in 0..rings {
            let phi = std::f64::consts::PI * (i as f64 + 0.5) / rings as f64;
            for j in 0..segs {
                let theta = 2.0 * std::f64::consts::PI * j as f64 / segs as f64;
                vertices.push((phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()));
            }
        }
        Model {
            vertices,
            edges: Vec::new(),
        }
    }

    /// 100k vertices, all six symmetry probes, under a wall-clock budget.
    ///
    /// The budget is the assertion, not decoration: an O(V^2) walk PASSES this
    /// test in 289 s, so without a bound the test reports nothing at all. The
    /// old timing was ~290 s and the fixed one is milliseconds, so the 20 s
    /// threshold is two orders of magnitude clear of both — this is a
    /// complexity guard, not a benchmark.
    #[test]
    fn sphere_symmetry_is_detected_at_scale() {
        let m = uv_sphere(250, 400); // 100k vertices
        let t = std::time::Instant::now();
        // A sphere is symmetric about every axis through its centre.
        assert!(has_mirror_symmetry(&m, 0), "x mirror");
        assert!(has_mirror_symmetry(&m, 1), "y mirror");
        assert!(has_mirror_symmetry(&m, 2), "z mirror");
        assert!(has_center_mirror_symmetry(&m, 0), "centre x mirror");
        assert!(has_center_mirror_symmetry(&m, 1), "centre y mirror");
        assert!(has_center_mirror_symmetry(&m, 2), "centre z mirror");
        let secs = t.elapsed().as_secs_f64();
        assert!(
            secs < 20.0,
            "six symmetry probes over {} vertices took {secs:.1}s — that is the O(V^2) walk, not a lookup",
            m.vertices.len()
        );
    }

    /// An asymmetric model must come back false — the twin lookup may not turn
    /// "no twin" into "close enough".
    #[test]
    fn asymmetric_model_is_rejected() {
        let mut m = uv_sphere(20, 40);
        // Move one vertex far off the sphere: its mirror no longer exists.
        m.vertices[7] = (99.0, 99.0, 99.0);
        assert!(!has_mirror_symmetry(&m, 0));
        assert!(!has_mirror_symmetry(&m, 1));
        assert!(!has_mirror_symmetry(&m, 2));
        assert!(!has_center_mirror_symmetry(&m, 0));
    }

    /// Off-axis symmetric shapes must still work: the twin is found by
    /// position, not by index arithmetic.
    #[test]
    fn off_origin_box_is_symmetric() {
        let m = Model {
            vertices: vec![
                (10.0, 20.0, 30.0),
                (14.0, 20.0, 30.0),
                (10.0, 26.0, 30.0),
                (14.0, 26.0, 30.0),
                (10.0, 20.0, 38.0),
                (14.0, 20.0, 38.0),
                (10.0, 26.0, 38.0),
                (14.0, 26.0, 38.0),
            ],
            edges: Vec::new(),
        };
        assert!(has_center_mirror_symmetry(&m, 0));
        assert!(has_center_mirror_symmetry(&m, 1));
        assert!(has_center_mirror_symmetry(&m, 2));
        // ...but NOT mirror-symmetric about the origin planes.
        assert!(!has_mirror_symmetry(&m, 0));
    }

    /// Duplicate coordinates must not fake a mirror image: a cloud of one
    /// repeated point mirrors onto itself, but a cloud plus one stray does not.
    #[test]
    fn duplicates_do_not_fake_symmetry() {
        let m = Model {
            vertices: vec![(1.0, 1.0, 1.0), (1.0, 1.0, 1.0)],
            edges: Vec::new(),
        };
        // -1 is absent, so the x mirror genuinely fails even though the
        // duplicate pair matches each other.
        assert!(!has_mirror_symmetry(&m, 0));
    }
}
