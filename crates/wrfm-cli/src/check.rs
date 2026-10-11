use crate::proximity::{Grid, POINT_TOL};
use serde_json::{Value, json};
use std::collections::HashMap;
use wrfm_raster::Model;

/// Tarjan bridge-finding: every edge whose removal disconnects its connected
/// component (an "open edge" — no cycle covers it). Runs the DFS over EACH
/// connected component separately (a model may be disconnected). Returns
/// vertex pairs (a, b). Parallel edges are handled by tracking the DFS tree
/// edge id, so a duplicated pair is never a bridge.
///
/// The walk keeps its own stack instead of recursing. A recursive Tarjan
/// descends one frame per vertex, so a path-like component overflows the 8 MB
/// main stack at roughly 40k vertices — a 51k-vertex sphere crashed every
/// model-loading subcommand, because `info` runs the full check. Measured
/// against the recursive form this is also slightly FASTER (best of 20: −25%
/// at a 10k path, −44% at 15k): the frames become contiguous heap writes
/// instead of call frames. The reason to prefer it, though, is that it has no
/// depth limit at all.
pub(crate) fn bridges(m: &Model) -> Vec<(usize, usize)> {
    let n = m.vertices.len();
    // (neighbor, edge id) adjacency — edge ids let a parallel edge to the
    // parent be treated as a back edge (never a bridge).
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (ei, &(a, b)) in m.edges.iter().enumerate() {
        adj[a].push((b, ei));
        adj[b].push((a, ei));
    }
    let mut disc = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut visited = vec![false; n];
    let mut out = Vec::new();
    let mut time = 0usize;

    // One frame per vertex on the CURRENT descent path: the vertex, the edge it
    // was reached by (so the parent edge is never mistaken for a back edge),
    // and how many of its neighbours have been examined. That cursor is what
    // replaces the recursive call's program counter — resuming a vertex means
    // reading it back, not returning into the middle of a loop.
    let mut stack: Vec<(usize, Option<usize>, usize)> = Vec::new();

    for root in 0..n {
        if visited[root] {
            continue;
        }
        visited[root] = true;
        disc[root] = time;
        low[root] = time;
        time += 1;
        stack.push((root, None, 0));

        while let Some(top) = stack.last_mut() {
            let u = top.0;
            if top.2 < adj[u].len() {
                let (v, ei) = adj[u][top.2];
                top.2 += 1;
                if !visited[v] {
                    // Descend: the recursive call, minus the call.
                    visited[v] = true;
                    disc[v] = time;
                    low[v] = time;
                    time += 1;
                    stack.push((v, Some(ei), 0));
                } else if Some(ei) != top.1 {
                    low[u] = low[u].min(disc[v]);
                }
            } else {
                // Exhausted: this is the recursive return. Fold the subtree's
                // low-link into the parent's, and record the bridge when the
                // subtree cannot reach back above the parent.
                stack.pop();
                if let Some(parent) = stack.last().map(|f| f.0) {
                    low[parent] = low[parent].min(low[u]);
                    if low[u] > disc[parent] {
                        out.push((parent, u));
                    }
                }
            }
        }
    }
    out
}

/// Per-vertex degree over every edge — the table `check` itself is built on
/// (out-of-range endpoints are ignored so a corrupt index can never panic).
pub fn degrees(m: &Model) -> Vec<usize> {
    let mut deg = vec![0usize; m.vertices.len()];
    for &(a, b) in &m.edges {
        // One endpoint at a time: two `get_mut` calls cannot live in the
        // same borrow expression, and an out-of-range endpoint is skipped.
        if let Some(x) = deg.get_mut(a) {
            *x += 1;
        }
        if let Some(y) = deg.get_mut(b) {
            *y += 1;
        }
    }
    deg
}

/// Up to two incident EDGE INDICES per vertex, in one pass over the edges.
///
/// This is the lookup `redundant_vertices` needs: the old form walked the whole
/// edge list once per degree-2 vertex to find its neighbours, which is O(V x E)
/// and quadratic on the shapes that are almost entirely interior vertices — a
/// 90k-vertex chain spent ~11 s deciding a verdict. `first`/`second` are what
/// replaced that scan.
fn incident_edges(m: &Model) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let n = m.vertices.len();
    let mut first: Vec<Option<usize>> = vec![None; n];
    let mut second: Vec<Option<usize>> = vec![None; n];
    for (ei, &(a, b)) in m.edges.iter().enumerate() {
        for v in [a, b] {
            if let Some(slot) = first.get_mut(v)
                && slot.is_none()
            {
                *slot = Some(ei);
                continue;
            }
            if let Some(slot) = second.get_mut(v)
                && slot.is_none()
            {
                *slot = Some(ei);
            }
        }
    }
    (first, second)
}

/// Degree table AND the incident-edge pairs from ONE pass.
///
/// `degrees` is kept as the authority on what a degree IS (see its callers);
/// this derives the identical table from the same walk that fills the pairs, so
/// `check` does not traverse the edge list twice.
fn degrees_and_incident(m: &Model) -> (Vec<usize>, Vec<Option<usize>>, Vec<Option<usize>>) {
    let n = m.vertices.len();
    let mut deg = vec![0usize; n];
    let mut first: Vec<Option<usize>> = vec![None; n];
    let mut second: Vec<Option<usize>> = vec![None; n];
    for (ei, &(a, b)) in m.edges.iter().enumerate() {
        // Degree: both endpoints count, and `a == b` counts twice — exactly
        // what `degrees` does.
        if let Some(x) = deg.get_mut(a) {
            *x += 1;
        }
        if let Some(y) = deg.get_mut(b) {
            *y += 1;
        }
        for v in [a, b] {
            if let Some(slot) = first.get_mut(v)
                && slot.is_none()
            {
                *slot = Some(ei);
                continue;
            }
            if let Some(slot) = second.get_mut(v)
                && slot.is_none()
            {
                *slot = Some(ei);
            }
        }
    }
    (deg, first, second)
}

/// A degree-2 vertex that lies within the point-identity tolerance of the
/// straight chord between its two neighbours is REDUNDANT: the wire passes
/// straight through it, so it is a degenerate (pinch) midpoint that carries
/// no shape — an exactly collapsed control grid is full of them (the teapot
/// case in docs/wrfm-cli-review.md). A degree-2 vertex that actually TURNS
/// a corner is healthy and is NOT reported; that distinction is the whole
/// difference from the old blanket "every degree 2 is non-manifold" rule.
pub fn redundant_vertices(m: &Model, deg: &[usize]) -> Vec<usize> {
    let (first, second) = incident_edges(m);
    m.vertices
        .iter()
        .enumerate()
        .filter(|&(i, _)| deg.get(i).copied().unwrap_or(0) == 2)
        .filter(|&(i, _)| is_redundant_midpoint(m, i, first[i], second[i]))
        .map(|(i, _)| i)
        .collect()
}

/// True when the two incident edges of `i` form a straight run through it.
/// Shapes that leave fewer than two distinct neighbours (self-loops,
/// duplicated edges — both broken verdicts caught elsewhere) are never
/// redundant, and a corrupt out-of-range neighbour can never panic.
///
/// `e1`/`e2` are two incident edge indices from [`incident_edges`]; taking them
/// as arguments is what keeps this O(1) instead of a scan of the whole edge
/// list.
fn is_redundant_midpoint(m: &Model, i: usize, e1: Option<usize>, e2: Option<usize>) -> bool {
    let (Some(e1), Some(e2)) = (e1, e2) else {
        return false;
    };
    let other = |(a, b): (usize, usize)| if a == i { b } else { a };
    // A self-loop incident to `i` reports `i` as its other end, so it is
    // rejected below along with any duplicated neighbour.
    let (n1, n2) = (other(m.edges[e1]), other(m.edges[e2]));
    if n1 == n2 || n1 == i || n2 == i {
        return false;
    }
    let (Some(&p), Some(&a), Some(&b)) =
        (m.vertices.get(i), m.vertices.get(n1), m.vertices.get(n2))
    else {
        return false;
    };
    point_segment_distance(p, a, b) < POINT_TOL
}

/// Distance from `p` to the segment `a..b` (clamped projection, so a point
/// beyond either end measures to the nearer endpoint). `a == b` degenerates
/// to a point distance — never a division by zero.
fn point_segment_distance(p: (f64, f64, f64), a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    let ab = (b.0 - a.0, b.1 - a.1, b.2 - a.2);
    let len2 = ab.0 * ab.0 + ab.1 * ab.1 + ab.2 * ab.2;
    let t = if len2 > 0.0 {
        let ap = (p.0 - a.0, p.1 - a.1, p.2 - a.2);
        ((ap.0 * ab.0 + ap.1 * ab.1 + ap.2 * ab.2) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = (a.0 + t * ab.0, a.1 + t * ab.1, a.2 + t * ab.2);
    ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2)).sqrt()
}

/// What the health check found, as lists for the report plus the counts the
/// verdict is computed from.
///
/// The lists exist only for `wrfm check`'s output; `with_issues = false` skips
/// building them (and the near-duplicate scan, which is the one check with no
/// cheap form) so a caller that wants nothing but the exit code does not pay
/// for JSON it will throw away.
struct Findings {
    vertices: usize,
    edges: usize,
    /// How many of each category were found. The verdict reads THESE, so it is
    /// the same answer whether or not the lists below were built — detection
    /// always runs, only the JSON is optional.
    n_duplicate_vertices: usize,
    n_near_duplicate_vertices: usize,
    n_zero_length_edges: usize,
    n_duplicate_edges: usize,
    n_dangling_edges: usize,
    n_isolated_vertices: usize,
    n_redundant_vertices: usize,
    duplicate_vertices: Vec<Value>,
    near_duplicate_vertices: Vec<Value>,
    zero_length_edges: Vec<Value>,
    duplicate_edges: Vec<Value>,
    dangling_edges: Vec<Value>,
    isolated_vertices: Vec<Value>,
    redundant_vertices: Vec<Value>,
}

impl Findings {
    /// The verdict rule. It reads counts, never the JSON bodies, so it is the
    /// same answer whether or not the lists were built.
    fn verdict(&self, strict: bool) -> &'static str {
        let broken = self.n_zero_length_edges > 0
            || self.n_duplicate_edges > 0
            || self.n_isolated_vertices > 0;
        let warn = self.n_duplicate_vertices > 0
            || self.n_near_duplicate_vertices > 0
            || self.n_dangling_edges > 0
            || self.n_redundant_vertices > 0;
        if broken || (warn && strict) {
            "broken"
        } else if warn {
            "warn"
        } else {
            "ok"
        }
    }

    fn issue_count(&self) -> usize {
        self.n_duplicate_vertices
            + self.n_near_duplicate_vertices
            + self.n_zero_length_edges
            + self.n_duplicate_edges
            + self.n_dangling_edges
            + self.n_isolated_vertices
            + self.n_redundant_vertices
    }

    fn summary(&self) -> String {
        format!(
            "{} issue(s): {} duplicate vertices, {} near-duplicate vertices, {} zero-length edges, {} duplicate edges, {} dangling edges, {} isolated vertices, {} redundant vertices",
            self.issue_count(),
            self.n_duplicate_vertices,
            self.n_near_duplicate_vertices,
            self.n_zero_length_edges,
            self.n_duplicate_edges,
            self.n_dangling_edges,
            self.n_isolated_vertices,
            self.n_redundant_vertices
        )
    }
}

/// Run the health check and BUILD THE REPORT. `strict` upgrades the
/// warning-level issues (duplicate vertices, dangling and redundant vertices)
/// to `broken`. Call [`check_verdict`] instead when only the exit code matters:
/// this one also computes the informational `quality` block, which is the
/// expensive part (it runs the bridge walk and indexes every edge).
pub fn check(m: &Model, strict: bool) -> Value {
    let f = findings(m, true);
    let verdict = f.verdict(strict);
    let summary = f.summary();
    let quality = quality_of(m);
    json!({
        // No "parseable" field: any file reaching wrfm_check already passed the
        // L1 load gate — syntax errors fail at load, so the answer is always
        // "true" by construction.
        "vertices": f.vertices,
        "edges": f.edges,
        "issues": {
            "duplicate_vertices": f.duplicate_vertices,
            "near_duplicate_vertices": f.near_duplicate_vertices,
            "zero_length_edges": f.zero_length_edges,
            "duplicate_edges": f.duplicate_edges,
            "dangling_edges": f.dangling_edges,
            "isolated_vertices": f.isolated_vertices,
            "redundant_vertices": f.redundant_vertices,
        },
        "tolerance": POINT_TOL,
        "quality": quality,
        "verdict": verdict,
        "summary": summary,
    })
}

/// The verdict alone, without the report.
///
/// Every data command reports the model's health on its EXIT CODE, so all of
/// them need this answer; only `wrfm check` needs the report. Skipping the
/// report also skips `quality_of`, which is where the bridge walk lives — so a
/// command like `render` no longer pays for a global bridge analysis it never
/// looks at.
pub fn check_verdict(m: &Model, strict: bool) -> &'static str {
    findings(m, false).verdict(strict)
}

/// Detect every issue category. See [`Findings`] for why the lists are
/// optional. The detection itself is the same either way.
fn findings(m: &Model, with_issues: bool) -> Findings {
    let n = m.vertices.len();

    // Degree table (needed by several checks) — built by the shared `degrees`
    // helper, so `check` counts exactly the degrees any other caller sees.
    // All edge indices are valid (see the module-level invariant). An EMPTY
    // model keeps its historical one-slot table, so the scan below still
    // reports it `broken` (isolated vertex) instead of silently `ok`.
    let (mut deg, first_incident, _second_incident) = degrees_and_incident(m);
    if n == 0 {
        deg.push(0);
    }

    // Duplicate vertices, in TWO categories — the repair action differs:
    //
    // * `duplicate_vertices`      — the exact same coordinates (bit pattern).
    //                               Repair: `wrfm edit --dedupe`.
    // * `near_duplicate_vertices` — closer than POINT_TOL but not bit-equal
    //                               (independently sampled copies of one point
    //                               differ by ~1e-15). Repair: `wrfm edit
    //                               --weld 1e-6`.
    //
    // The near-duplicate scan uses the shared `proximity::Grid`, whose 27-cell
    // probe also finds pairs that straddle a cell boundary; the old
    // single-bucket probe silently missed those (regression test:
    // `check_finds_near_duplicates_across_a_cell_boundary`).
    let mut dup_vertices: Vec<Value> = Vec::new();
    let mut near_vertices: Vec<Value> = Vec::new();
    let (mut n_dup_vertices, mut n_near_vertices) = (0usize, 0usize);
    {
        let mut exact: HashMap<(u64, u64, u64), usize> = HashMap::new();
        let mut grid = Grid::new(POINT_TOL);
        for (i, &v) in m.vertices.iter().enumerate() {
            let bits = (v.0.to_bits(), v.1.to_bits(), v.2.to_bits());
            if let Some(&twin) = exact.get(&bits) {
                n_dup_vertices += 1;
                if with_issues {
                    dup_vertices.push(json!({
                                    "index": i,
                                    "coords": [v.0, v.1, v.2],
                                    "twin": twin,
                    }));
                }
            } else {
                exact.insert(bits, i);
                // Detection, ALWAYS: a near-duplicate is a `warn`, and the warn
                // travels on the exit code. Skipping it here made `wrfm edit
                // --dedupe` exit 0 on a model with a 2e-16 twin (caught by
                // tests/cli.rs). Only the JSON body is optional.
                if let Some(twin) = grid.nearest_within(v, &m.vertices, POINT_TOL) {
                    let w = m.vertices[twin];
                    let d =
                        ((v.0 - w.0).powi(2) + (v.1 - w.1).powi(2) + (v.2 - w.2).powi(2)).sqrt();
                    n_near_vertices += 1;
                    if with_issues {
                        near_vertices.push(json!({
                                           "index": i,
                                           "coords": [v.0, v.1, v.2],
                                           "twin": twin,
                                           "distance": d,
                        }));
                    }
                }
            }
            grid.insert(i, v);
        }
    }

    // Zero-length edges.
    let mut zero_edges: Vec<Value> = Vec::new();
    let mut n_zero_edges = 0usize;
    // Duplicate edges (unordered pairs).
    let mut edge_count: HashMap<(usize, usize), usize> = HashMap::new();
    // `first_incident` came from `degrees_and_incident` above: it replaces the
    // dangling pass's rescan of the whole edge list once per degree-1 vertex
    // (a star graph was quadratic: 30k leaves cost 2.5 s in a command that only
    // wanted a verdict).
    for &(a, b) in m.edges.iter() {
        // Edge indices are valid (module-level invariant), so `va`/`vb`
        // exist; the old `a < n && b < n` guard was removed as unreachable: the
        // module-level invariant already guarantees valid edge indices.
        let (va, vb) = (m.vertices[a], m.vertices[b]);
        let d2 = (va.0 - vb.0).powi(2) + (va.1 - vb.1).powi(2) + (va.2 - vb.2).powi(2);
        if d2 < POINT_TOL * POINT_TOL {
            n_zero_edges += 1;
            if with_issues {
                zero_edges.push(json!({ "edge": [a, b], "distance": d2.sqrt() }));
            }
        }
        let key = (a.min(b), a.max(b));
        *edge_count.entry(key).or_insert(0) += 1;
    }
    let mut dup_edges: Vec<Value> = Vec::new();
    let mut counts: Vec<(usize, usize, usize)> = edge_count
        .iter()
        .filter(|(_, c)| **c > 1)
        .map(|(&(a, b), &c)| (a, b, c))
        .collect();
    counts.sort();
    let n_dup_edges = counts.len();
    if with_issues {
        for (a, b, c) in counts {
            dup_edges.push(json!({ "edge": [a, b], "repeats": c }));
        }
    }

    // Dangling edges (touch a degree-1 vertex) and isolated vertices.
    let mut dangling: Vec<Value> = Vec::new();
    let mut isolated: Vec<usize> = Vec::new();
    let mut n_dangling = 0usize;
    for (i, &d) in deg.iter().enumerate() {
        if d == 0 {
            isolated.push(i);
        } else if d == 1 {
            // A degree-1 vertex has exactly one incident edge; `first_incident`
            // remembers which without scanning the edge list again.
            if let Some(ei) = first_incident.get(i).copied().flatten() {
                n_dangling += 1;
                if with_issues {
                    let (a, b) = m.edges[ei];
                    dangling.push(json!({ "edge": [a, b], "vertex": i, "edge_index": ei }));
                }
            }
        }
    }
    // Isolated vertices are reported as objects like every other category
    // (they used to be bare integers, the one shape outlier in `issues`).
    let isolated_json: Vec<Value> = isolated.iter().map(|&i| json!({ "index": i })).collect();

    // Redundant vertices: degree-2 midpoints on the straight chord between
    // their neighbours (see `redundant_vertices`) — pass-throughs, not
    // corners.
    let found_redundant = redundant_vertices(m, &deg);
    let n_redundant = found_redundant.len();
    let redundant: Vec<Value> = if with_issues {
        found_redundant
            .into_iter()
            .map(|i| json!({ "vertex": i, "degree": 2 }))
            .collect()
    } else {
        Vec::new()
    };

    Findings {
        vertices: n,
        edges: m.edges.len(),
        n_duplicate_vertices: n_dup_vertices,
        n_near_duplicate_vertices: n_near_vertices,
        n_zero_length_edges: n_zero_edges,
        n_duplicate_edges: n_dup_edges,
        n_dangling_edges: n_dangling,
        n_isolated_vertices: isolated.len(),
        n_redundant_vertices: n_redundant,
        duplicate_vertices: dup_vertices,
        near_duplicate_vertices: near_vertices,
        zero_length_edges: zero_edges,
        duplicate_edges: dup_edges,
        dangling_edges: dangling,
        isolated_vertices: isolated_json,
        redundant_vertices: redundant,
    }
}

/// Informational quality diagnostics (open edges / proportion / orientation).
/// Computed for every model but NEVER affect the verdict and are NEVER
/// upgraded by `--strict` — they exist so `wrfm check` doubles as a
/// geometry-hint channel without changing the health contract.
fn quality_of(m: &Model) -> Value {
    // Axis spans of the bounding box.
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for &(x, y, z) in &m.vertices {
        min[0] = min[0].min(x);
        min[1] = min[1].min(y);
        min[2] = min[2].min(z);
        max[0] = max[0].max(x);
        max[1] = max[1].max(y);
        max[2] = max[2].max(z);
    }
    let empty = m.vertices.is_empty();
    let spans = [
        if empty { 0.0 } else { max[0] - min[0] },
        if empty { 0.0 } else { max[1] - min[1] },
        if empty { 0.0 } else { max[2] - min[2] },
    ];

    // Open edges = bridges, mapped back to their original edge indices.
    //
    // The index is built ONCE and probed per bridge. The obvious form — for
    // each bridge, walk the edge list looking for the pair — is O(E x bridges),
    // and a bridge-heavy model makes that quadratic for real: a 90k-vertex
    // chain (~90k bridges over 90k edges) spent 24 s resolving indices for a
    // command that only wanted its verdict. First occurrence wins, which is
    // what the scanning form reported too (its `flat_map` emitted every match,
    // but a simple graph has one).
    //
    // No bridges short-circuits: the index would otherwise cost more than the
    // scan it replaces, since a closed model gives the scan no work to do.
    let open_edges: Vec<Value> = {
        let br = bridges(m);
        if br.is_empty() {
            Vec::new()
        } else {
            let mut edge_index: HashMap<(usize, usize), usize> =
                HashMap::with_capacity(m.edges.len());
            for (ei, &(a, b)) in m.edges.iter().enumerate() {
                edge_index.entry((a.min(b), a.max(b))).or_insert(ei);
            }
            br.iter()
                .filter_map(|&(a, b)| {
                    edge_index
                        .get(&(a.min(b), a.max(b)))
                        .map(|&ei| json!({ "edge": [a, b], "edge_index": ei }))
                })
                .collect()
        }
    };

    let mut quality = json!({
        "open_edges": open_edges,
        "orientation": {
            "x_span": spans[0],
            "y_span": spans[1],
            "z_span": spans[2],
            "lying_on_side": spans[2] > 2.0 * spans[1],
        },
    });

    // Proportion: omit the object when the model has no measurable span
    // (empty / single-point) — informational only, no threshold verdict.
    let min_span = spans[0].min(spans[1]).min(spans[2]);
    let max_span = spans[0].max(spans[1]).max(spans[2]);
    if min_span >= 1e-9 {
        let max_axis = if max_span == spans[0] {
            "x"
        } else if max_span == spans[1] {
            "y"
        } else {
            "z"
        };
        quality["proportion"] = json!({
            "aspect_ratio": max_span / min_span,
            "min_span": min_span,
            "max_span": max_span,
            "max_axis": max_axis,
        });
    }
    quality
}

/// Render the L2 check report as the `wrfm check` stdout text:
pub fn report_text(name: &str, vertices: usize, edges: usize, c: &serde_json::Value) -> String {
    let verdict = c["verdict"].as_str().unwrap_or("ok");
    let mut out = format!("{verdict}: {name} ({vertices} vertices, {edges} edges)\n");
    let issues = &c["issues"];
    let summary = c["summary"].as_str().unwrap_or("");
    if verdict != "ok" {
        out.push_str(summary);
        out.push('\n');
        for (kind, label) in [
            ("duplicate_vertices", "duplicate vertices"),
            ("near_duplicate_vertices", "near-duplicate vertices"),
            ("zero_length_edges", "zero-length edges"),
            ("duplicate_edges", "duplicate edges"),
            ("dangling_edges", "dangling edges"),
            ("isolated_vertices", "isolated vertices"),
            ("redundant_vertices", "redundant vertices"),
        ] {
            let list = issues.get(kind).and_then(|v| v.as_array());
            if let Some(list) = list.filter(|l| !l.is_empty()) {
                out.push_str(&format!("  {label}:\n"));
                for item in list {
                    match kind {
                        "duplicate_vertices" => out.push_str(&format!(
                            "    vertex {} at [{}, {}, {}] (twin {})\n",
                            item["index"],
                            item["coords"][0],
                            item["coords"][1],
                            item["coords"][2],
                            item["twin"],
                        )),
                        "near_duplicate_vertices" => out.push_str(&format!(
                            "    vertex {} at [{}, {}, {}] (twin {}, distance {:.3e})\n",
                            item["index"],
                            item["coords"][0],
                            item["coords"][1],
                            item["coords"][2],
                            item["twin"],
                            item["distance"].as_f64().unwrap_or(0.0),
                        )),
                        "zero_length_edges" => out.push_str(&format!(
                            "    edge [{}, {}]\n",
                            item["edge"][0], item["edge"][1]
                        )),
                        "duplicate_edges" => out.push_str(&format!(
                            "    edge [{}, {}] ({} repeats)\n",
                            item["edge"][0], item["edge"][1], item["repeats"],
                        )),
                        "dangling_edges" => out.push_str(&format!(
                            "    edge [{}, {}] (edge index {}, touches degree-1 vertex {})\n",
                            item["edge"][0], item["edge"][1], item["edge_index"], item["vertex"],
                        )),
                        "isolated_vertices" => {
                            out.push_str(&format!("    vertex {}\n", item["index"]))
                        }
                        "redundant_vertices" => out.push_str(&format!(
                            "    vertex {} (collinear midpoint)\n",
                            item["vertex"],
                        )),
                        _ => out.push_str(&format!(
                            "    vertex {} (degree {})\n",
                            item["vertex"], item["degree"],
                        )),
                    }
                }
                // The actionable threshold, printed ONCE and only under the list it
                // clears: `--weld` takes an explicit tolerance, and this is the value
                // that repairs exactly the near-duplicates above.
                if kind == "near_duplicate_vertices" {
                    out.push_str(&format!(
 "  tolerance: {POINT_TOL:e} world units (repair with `wrfm edit <file> --weld {POINT_TOL:e}`)\n"
 ));
                }
            }
        }
    }
    // Informational quality line (never affects the verdict): emitted only
    // when at least one diagnostic is non-trivial (open edges / stretched
    // proportion / lying on side). The JSON keeps the full bridge list.
    if let Some(q) = c.get("quality") {
        let open = q["open_edges"].as_array().map(|a| a.len()).unwrap_or(0);
        let lying = q["orientation"]["lying_on_side"].as_bool().unwrap_or(false);
        let aspect = q["proportion"]["aspect_ratio"].as_f64();
        if open > 0 || lying || aspect.is_some_and(|a| a > 1.0) {
            let max_axis = q["proportion"]["max_axis"].as_str().unwrap_or("-");
            let y = q["orientation"]["y_span"].as_f64().unwrap_or(0.0);
            let z = q["orientation"]["z_span"].as_f64().unwrap_or(0.0);
            let mut line = format!("quality: {open} open edges");
            if let Some(a) = aspect {
                line.push_str(&format!(", aspect {a:.2} ({max_axis} longest)"));
            }
            if lying {
                line.push_str(&format!(", z {z:.1}x y {y:.1} - possibly lying on side"));
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
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

    #[test]
    fn clean_cube_is_ok() {
        let r = check(&cube(), false);
        assert_eq!(r["verdict"], "ok");
        assert_eq!(
            r["issues"]["duplicate_vertices"].as_array().unwrap().len(),
            0
        );
        assert_eq!(
            r["issues"]["zero_length_edges"].as_array().unwrap().len(),
            0
        );
    }

    #[test]
    fn duplicate_vertex_detected() {
        let mut m = cube();
        m.vertices.push((1.0, 1.0, 1.0)); // twin of vertex 7
        m.edges.push((8, 0)); // keep it attached (degree 1, not isolated)
        let r = check(&m, false);
        assert_eq!(r["verdict"], "warn");
        let dups = r["issues"]["duplicate_vertices"].as_array().unwrap();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0]["twin"], 7);
        assert_eq!(dups[0]["index"], 8);
    }

    #[test]
    fn zero_length_edge_is_broken() {
        let mut m = cube();
        m.vertices.push((0.0, 0.0, 0.0));
        m.edges.push((8, 8)); // same vertex
        let r = check(&m, false);
        assert_eq!(r["verdict"], "broken");
        assert_eq!(
            r["issues"]["zero_length_edges"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn dangling_and_isolated_detected() {
        let mut m = cube();
        // Dangling: an edge whose other end is a fresh degree-1 vertex.
        m.vertices.push((5.0, 5.0, 5.0));
        m.edges.push((0, 8));
        // Isolated: a fresh vertex with no edges.
        m.vertices.push((9.0, 9.0, 9.0));
        let r = check(&m, false);
        assert_eq!(r["verdict"], "broken"); // isolated -> broken
        assert_eq!(
            r["issues"]["isolated_vertices"].as_array().unwrap().len(),
            1
        );
        assert!(
            r["issues"]["dangling_edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["edge_index"] == 12)
        );
    }

    #[test]
    fn duplicate_edge_detected() {
        let mut m = cube();
        m.edges.push((0, 1)); // duplicate of edge 0-1
        let r = check(&m, false);
        assert_eq!(r["verdict"], "broken");
        assert_eq!(r["issues"]["duplicate_edges"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn strict_upgrades_warn_to_broken() {
        let mut m = cube();
        m.vertices.push((1.0, 1.0, 1.0)); // duplicate -> warn normally
        let r = check(&m, true);
        assert_eq!(r["verdict"], "broken");
    }
    #[test]
    fn quality_open_edges_reported_but_verdict_unchanged() {
        // Two tetrahedra joined by ONE edge: every vertex has degree 3 (fully
        // "ok" health) yet edge (0, 4) is a bridge — removing it disconnects
        // the two components. (A literal chain would be `warn` for its dangling
        // degree-1 ends; this model isolates the "open edges" signal.)
        let m = Model {
            vertices: vec![
                (1.0, 1.0, 1.0),
                (1.0, -1.0, -1.0),
                (-1.0, 1.0, -1.0),
                (-1.0, -1.0, 1.0),
                (6.0, 1.0, 1.0),
                (6.0, -1.0, -1.0),
                (4.0, 1.0, -1.0),
                (4.0, -1.0, 1.0),
            ],
            edges: vec![
                (0, 1),
                (0, 2),
                (0, 3),
                (1, 2),
                (2, 3),
                (3, 1),
                (4, 5),
                (4, 6),
                (4, 7),
                (5, 6),
                (6, 7),
                (7, 5),
                (0, 4),
            ],
        };
        let r = check(&m, false);
        assert_eq!(r["verdict"], "ok");
        let open = r["quality"]["open_edges"].as_array().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0]["edge"], json!([0, 4]));
    }

    #[test]
    fn quality_orientation_flags_lying_model() {
        // z-span 4, y-span 1 -> lying_on_side true; verdict unchanged.
        let m = Model {
            vertices: vec![
                (0.0, 0.0, 0.0),
                (0.0, 0.0, 4.0),
                (1.0, 0.0, 0.0),
                (1.0, 0.0, 4.0),
                (0.0, 1.0, 0.0),
                (0.0, 1.0, 4.0),
                (1.0, 1.0, 0.0),
                (1.0, 1.0, 4.0),
            ],
            edges: vec![
                (0, 1),
                (2, 3),
                (4, 5),
                (6, 7),
                (0, 2),
                (1, 3),
                (4, 6),
                (5, 7),
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
            ],
        };
        let r = check(&m, false);
        assert_eq!(r["quality"]["orientation"]["lying_on_side"], true);
        assert_eq!(r["quality"]["orientation"]["z_span"], 4.0);
        assert_eq!(r["quality"]["orientation"]["y_span"], 1.0);
        assert_eq!(r["verdict"], "ok");
    }

    /// A closed loop: (0,0)-(1,0)-(2,0)-(2,1)-(0,1)-(0,0). Vertex 1 is a
    /// dead-straight midpoint on the bottom run; the other four turn corners.
    fn loop_with_midpoint() -> Model {
        Model {
            vertices: vec![
                (0.0, 0.0, 0.0),
                (1.0, 0.0, 0.0),
                (2.0, 0.0, 0.0),
                (2.0, 1.0, 0.0),
                (0.0, 1.0, 0.0),
            ],
            edges: vec![(0, 1), (1, 2), (2, 3), (3, 4), (4, 0)],
        }
    }

    #[test]
    fn redundant_midpoint_is_the_only_degree2_category() {
        let r = check(&loop_with_midpoint(), false);
        assert_eq!(r["verdict"], "warn");
        let items = r["issues"]["redundant_vertices"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["vertex"], 1);

        // Bend the midpoint off the chord: every vertex is now a real
        // corner and the loop is fully healthy — the old blanket degree-2
        // rule warned here.
        let mut bent = loop_with_midpoint();
        bent.vertices[1] = (1.0, 0.2, 0.0);
        let r = check(&bent, false);
        assert_eq!(r["verdict"], "ok");
        assert!(
            r["issues"]["redundant_vertices"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn redundancy_threshold_is_the_point_identity_tolerance() {
        // A shallow but real bend (1e-4 off the chord, 100x POINT_TOL) is
        // NOT redundant: only a midpoint whose removal stays inside
        // point-identity granularity counts.
        let mut m = loop_with_midpoint();
        m.vertices[1] = (1.0, 1e-4, 0.0);
        let r = check(&m, false);
        assert_eq!(r["verdict"], "ok");
    }

    #[test]
    fn strict_upgrades_redundant_to_broken() {
        let r = check(&loop_with_midpoint(), true);
        assert_eq!(r["verdict"], "broken");
    }

    #[test]
    fn quality_proportion_reports_aspect() {
        // Unit cube [-1,1]^3 -> aspect 1.0 (max_axis tie resolves to x).
        let r = check(&cube(), false);
        assert_eq!(r["quality"]["proportion"]["aspect_ratio"], 1.0);
        assert_eq!(r["quality"]["proportion"]["max_axis"], "x");
        // A 4 x 2 x 2 box -> aspect 2.0, x longest.
        let m = Model {
            vertices: vec![
                (-2.0, -1.0, -1.0),
                (2.0, -1.0, -1.0),
                (-2.0, 1.0, -1.0),
                (2.0, 1.0, -1.0),
                (-2.0, -1.0, 1.0),
                (2.0, -1.0, 1.0),
                (-2.0, 1.0, 1.0),
                (2.0, 1.0, 1.0),
            ],
            edges: vec![
                (0, 1),
                (2, 3),
                (4, 5),
                (6, 7),
                (0, 2),
                (1, 3),
                (4, 6),
                (5, 7),
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
            ],
        };
        let r2 = check(&m, false);
        assert_eq!(r2["quality"]["proportion"]["aspect_ratio"], 2.0);
        assert_eq!(r2["quality"]["proportion"]["max_axis"], "x");
        assert_eq!(r2["verdict"], "ok");
    }

    /// A long simple path is the shape a recursive DFS cannot survive: the
    /// Tarjan walk descends one vertex per frame, so depth == vertex count and
    /// any model past ~40k vertices in a single component overflows the 8 MB
    /// main stack. Real inputs hit this (a 90k-vertex sphere does, reproduced
    /// with gdb: 20,083 `bridges::dfs` frames on the stack).
    ///
    /// `wrfm info` runs the full check, so EVERY command inherited the crash,
    /// not just `check`.
    #[test]
    fn bridges_survives_a_path_deep_enough_to_overflow_a_recursive_dfs() {
        let n = 200_000usize;
        let m = Model {
            vertices: (0..n).map(|i| (i as f64, 0.0, 0.0)).collect(),
            edges: (0..n - 1).map(|i| (i, i + 1)).collect(),
        };
        let open = bridges(&m);
        // Every edge of a path is a bridge: that is the definition, and it also
        // pins that the iterative walk still CLASSIFIES, not merely survives.
        // The order is deepest-first, exactly as the recursive walk produced it
        // (the walk unwinds from the far end back to the root).
        assert_eq!(open.len(), n - 1);
        assert_eq!(open[0], (n - 2, n - 1));
        assert_eq!(open[n - 2], (0, 1));
    }

    /// The deep path with ONE extra edge between its ends: the cycle removes
    /// every bridge. Guards the parent-edge bookkeeping the iterative form has
    /// to get right (the walk must not re-enter the vertex it came from).
    #[test]
    fn bridges_of_a_long_cycle_are_empty() {
        let n = 200_000usize;
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        edges.push((n - 1, 0));
        let m = Model {
            vertices: (0..n).map(|i| (i as f64, 0.0, 0.0)).collect(),
            edges,
        };
        assert!(bridges(&m).is_empty());
    }

    /// Disconnected components are walked separately, so a deep one must not
    /// mask the components after it.
    #[test]
    fn bridges_walks_every_component_after_a_deep_one() {
        let n = 120_000usize;
        let mut vertices: Vec<(f64, f64, f64)> = (0..n).map(|i| (i as f64, 0.0, 0.0)).collect();
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        // plus a separate triangle, whose edges are never bridges
        vertices.extend([(0.0, 5.0, 0.0), (1.0, 5.0, 0.0), (0.0, 6.0, 0.0)]);
        edges.extend([(n, n + 1), (n + 1, n + 2), (n + 2, n)]);
        let m = Model { vertices, edges };
        assert_eq!(bridges(&m).len(), n - 1);
    }

    /// Redundant-vertex detection must be the same on a shape with MANY
    /// degree-2 vertices as on a small one. The old form rescanned every edge
    /// once per degree-2 vertex, so a 90k chain (every interior vertex is a
    /// straight pass-through) spent ~11 s in a command that only wanted a
    /// verdict.
    #[test]
    fn redundant_detection_is_shape_independent() {
        // A chain: every interior vertex is a collinear pass-through.
        let n = 20_000usize;
        let chain = Model {
            vertices: (0..n).map(|i| (i as f64, 0.0, 0.0)).collect(),
            edges: (0..n - 1).map(|i| (i, i + 1)).collect(),
        };
        let deg = degrees(&chain);
        let red = redundant_vertices(&chain, &deg);
        assert_eq!(
            red.len(),
            n - 2,
            "every interior vertex of a chain is redundant"
        );
        assert_eq!(red[0], 1);
        assert_eq!(*red.last().unwrap(), n - 2);

        // A chain that TURNS a corner at the middle: that vertex is not a
        // pass-through, and neither are the ends.
        let half = n / 2;
        let bent = Model {
            vertices: (0..n)
                .map(|i| {
                    if i <= half {
                        (i as f64, 0.0, 0.0)
                    } else {
                        (half as f64, (i - half) as f64, 0.0)
                    }
                })
                .collect(),
            edges: (0..n - 1).map(|i| (i, i + 1)).collect(),
        };
        let deg = degrees(&bent);
        let red = redundant_vertices(&bent, &deg);
        assert!(!red.contains(&half), "the corner is not a pass-through");
        assert_eq!(red.len(), n - 3, "only the corner is excluded");
    }

    /// Parallel edges and self-loops leave a vertex without two DISTINCT
    /// neighbours, so they are never redundant — and the cheap neighbour lookup
    /// must not mistake one edge for two.
    #[test]
    fn redundant_excludes_shapes_without_two_distinct_neighbours() {
        // Two vertices joined twice: each has degree 2 but only one neighbour.
        let doubled = Model {
            vertices: vec![(0.0, 0.0, 0.0), (1.0, 0.0, 0.0)],
            edges: vec![(0, 1), (0, 1)],
        };
        let deg = degrees(&doubled);
        assert!(redundant_vertices(&doubled, &deg).is_empty());

        // A self-loop plus one real edge: the loop's two "incidences" are the
        // same vertex.
        let looped = Model {
            vertices: vec![(0.0, 0.0, 0.0), (1.0, 0.0, 0.0)],
            edges: vec![(0, 0), (0, 1)],
        };
        let deg = degrees(&looped);
        assert!(redundant_vertices(&looped, &deg).is_empty());
    }

    /// A model per HEALTH OUTCOME, so the cheap entry point is compared against
    /// the full report on every branch of the verdict rule rather than on one
    /// lucky input.
    fn verdict_fixtures() -> Vec<(&'static str, Model)> {
        vec![
            ("cube", cube()),
            ("empty", Model::default()),
            ("duplicate vertex", {
                let mut m = cube();
                m.vertices.push((1.0, 1.0, 1.0));
                m.edges.push((8, 0));
                m
            }),
            ("zero-length edge", {
                let mut m = cube();
                m.vertices.push((0.0, 0.0, 0.0));
                m.edges.push((8, 8));
                m
            }),
            ("isolated vertex", {
                let mut m = cube();
                m.vertices.push((9.0, 9.0, 9.0));
                m
            }),
            ("dangling edge", {
                let mut m = cube();
                m.vertices.push((5.0, 5.0, 5.0));
                m.edges.push((0, 8));
                m
            }),
            // The warn branch that --strict upgrades to broken.
            ("redundant midpoint", {
                let mut m = cube();
                let base = m.vertices.len();
                m.vertices.push((0.0, 0.0, 0.0));
                m.vertices.push((2.0, 0.0, 0.0));
                m.edges.push((base, base + 1));
                m
            }),
        ]
    }

    /// `check_verdict` must answer exactly what `check` answers, on every model
    /// and under both `strict` settings — it is the same rule, reached without
    /// building the JSON report.
    #[test]
    fn check_verdict_agrees_with_the_full_report() {
        for (name, m) in verdict_fixtures() {
            for strict in [false, true] {
                assert_eq!(
                    check_verdict(&m, strict),
                    check(&m, strict)["verdict"].as_str().unwrap(),
                    "{name} (strict={strict}): the cheap path disagrees with the report"
                );
            }
        }
        // ...and every branch actually occurred, so the loop above cannot pass
        // by all fixtures landing in the same arm.
        let seen: Vec<&str> = verdict_fixtures()
            .iter()
            .map(|(_, m)| check_verdict(m, false))
            .collect();
        for want in ["ok", "warn", "broken"] {
            assert!(seen.contains(&want), "no fixture produced verdict {want}");
        }
    }

    /// A report verdict of `ok` is the claim that EVERY category was empty, so
    /// check it against the arrays the report prints.
    #[test]
    fn an_ok_verdict_means_every_issue_category_is_empty() {
        const CATEGORIES: [&str; 7] = [
            "duplicate_vertices",
            "near_duplicate_vertices",
            "zero_length_edges",
            "duplicate_edges",
            "dangling_edges",
            "isolated_vertices",
            "redundant_vertices",
        ];
        for (name, m) in verdict_fixtures() {
            let r = check(&m, false);
            for key in CATEGORIES {
                assert!(r["issues"][key].is_array(), "{name}: {key} missing");
            }
            if r["verdict"] == "ok" {
                for key in CATEGORIES {
                    assert_eq!(
                        r["issues"][key].as_array().unwrap().len(),
                        0,
                        "{name}: verdict ok but {key} is not empty"
                    );
                }
            }
        }
    }

    /// The dangling/isolated split must not depend on how MANY degree-1
    /// vertices there are: the old form rescanned every edge once per degree-1
    /// vertex, so a star graph was quadratic. Both shapes pin the count.
    #[test]
    fn dangling_detection_is_shape_independent() {
        // A star: one hub, many leaves. Every leaf is degree-1 -> dangling.
        let n = 5_000usize;
        let mut vertices = vec![(0.0, 0.0, 0.0)];
        let mut edges = Vec::new();
        for i in 1..=n {
            vertices.push((i as f64, 0.0, 0.0));
            edges.push((0, i));
        }
        let star = Model { vertices, edges };
        let r = check(&star, false);
        assert_eq!(r["issues"]["dangling_edges"].as_array().unwrap().len(), n);
        assert_eq!(
            r["issues"]["isolated_vertices"].as_array().unwrap().len(),
            0
        );
        assert_eq!(r["verdict"], "warn"); // dangling only warns, never breaks
        assert_eq!(check_verdict(&star, false), "warn");

        // A plain chain: exactly TWO degree-1 ends -> exactly two dangling.
        let n = 1_000usize;
        let chain = Model {
            vertices: (0..n).map(|i| (i as f64, 0.0, 0.0)).collect(),
            edges: (0..n - 1).map(|i| (i, i + 1)).collect(),
        };
        let rc = check(&chain, false);
        assert_eq!(rc["issues"]["dangling_edges"].as_array().unwrap().len(), 2);
    }
}
