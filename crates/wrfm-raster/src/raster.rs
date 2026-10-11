//! Line rasterization: Cohen-Sutherland clipping, canvas -> dot-grid
//! mapping, Bresenham stepping and braille cell encoding.
//!
//! This is the algorithm behind ratatui's `Canvas` widget (its clipping is
//! the `line_clipping` crate's Cohen-Sutherland, its mapping is
//! `Painter::get_point`, its stepping is `for_each_line_point`), ported so
//! callers can rasterize into their own buffers without a widget tree —
//! with per-cell color, incremental frames, or parallel edge chunks.
//! `tests/golden.rs` renders with both and asserts byte-identical output.

/// ratatui's braille pattern table: a row-major 2x4 dot pattern -> the
/// Unicode braille char. A permutation of U+2800+pattern (e.g. `[0x55] ==
/// '\u{2847}'`), NOT the identity — using ratatui's own table is what keeps
/// the cells byte-identical to the Canvas path.
pub use ratatui::symbols::braille::BRAILLE;

/// Where a frame is drawn: the braille dot grid and the canvas-space
/// window `[left, right] x [bottom, top]` that lines are clipped against
/// and mapped into (canvas y grows UP).
///
/// The two are one value because they are one decision — every mapping
/// needs both, and the dots-per-cell factor (`2` wide, `4` tall) relates
/// them. As separate arguments a caller could pair a grid with a window of
/// a different shape, which would scale the frame wrong; there is no way
/// to express that here.
///
/// Build it ONCE per frame and pass it by value ([`Viewport`] is `Copy`).
/// The window is a property of the frame, not of a line, and rebuilding it
/// per line means re-deriving the same spans on every line.
///
/// For the full frame use [`Viewport::centered`] — the centered box
/// `[-px_w/2, px_w/2] x [-px_h/2, px_h/2]`. A region zoom (CLI `--region`
/// / `--fit content`) yields an off-center window with the same mapping:
/// [`Viewport::from_arrays`].
///
/// A degenerate window (`right <= left` or `top <= bottom`, i.e. a
/// zero-or-negative span) draws nothing, matching ratatui's `get_point`
/// guard; that is what a `--width 0` canvas derives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Canvas-space clipping window, `[left, right] x [bottom, top]`.
    left: f64,
    right: f64,
    bottom: f64,
    top: f64,
    /// The grid in DOTS — `cells * 2` wide, `cells * 4` tall.
    px_w: usize,
    px_h: usize,
}

impl Viewport {
    /// The full-frame window for a `px_w` x `px_h` dot grid centered on the
    /// origin — the shape the TUI paints every frame.
    pub fn centered(px_w: usize, px_h: usize) -> Self {
        let (w, h) = (px_w as f64, px_h as f64);
        Self {
            left: -w / 2.0,
            right: w / 2.0,
            bottom: -h / 2.0,
            top: h / 2.0,
            px_w,
            px_h,
        }
    }

    /// A `px_w` x `px_h` dot grid over an explicit canvas window, given as
    /// the x/y bounds pair (`[min, max]` per axis) ratatui's
    /// `Canvas::x_bounds` / `y_bounds` take.
    pub fn from_arrays(px_w: usize, px_h: usize, x_bounds: [f64; 2], y_bounds: [f64; 2]) -> Self {
        Self {
            left: x_bounds[0],
            right: x_bounds[1],
            bottom: y_bounds[0],
            top: y_bounds[1],
            px_w,
            px_h,
        }
    }

    /// The grid width in DOTS (`cells * 2`).
    #[inline]
    pub fn px_w(&self) -> usize {
        self.px_w
    }

    /// The grid height in DOTS (`cells * 4`).
    #[inline]
    pub fn px_h(&self) -> usize {
        self.px_h
    }

    /// The number of character CELLS this viewport covers — the length of a
    /// per-cell pattern/color buffer. [`dots_to_lines`] takes the CELL count
    /// (`cw`, `ch`), not the dot count.
    #[inline]
    pub fn cells(&self) -> usize {
        (self.px_w / 2) * (self.px_h / 4)
    }

    /// The canvas window being clipped against, `[left, right, bottom, top]`
    /// — what a ratatui `Canvas` needs for `x_bounds` / `y_bounds` to
    /// reproduce this frame.
    #[inline]
    pub fn window(&self) -> [f64; 4] {
        [self.left, self.right, self.bottom, self.top]
    }

    /// The window's horizontal span (`right - left`).
    ///
    /// Subtraction is exact, so deriving it on demand rather than storing it
    /// loses nothing and keeps the struct one word smaller.
    #[inline]
    fn width(&self) -> f64 {
        self.right - self.left
    }

    /// The window's vertical span (`top - bottom`).
    #[inline]
    fn height(&self) -> f64 {
        self.top - self.bottom
    }

    /// Map a coordinate, measured from `origin` and spanning `span`, onto
    /// dot `0..=dots - 1`; `None` when `inside` says it is out of window.
    ///
    /// `inside` is the window test, and it cannot be folded into a range
    /// check on the mapped value even though the mapping sends the window's
    /// edges to dot 0 and dot `dots - 1`: a coordinate half a dot OUTSIDE
    /// the window still rounds into `0..=dots - 1` (`x = left - 0.4` maps to
    /// dot 0), so the range check alone lights a cell just past the frame.
    /// Both tests are load-bearing, and `inside` is the one that rejects a
    /// non-finite coordinate — every comparison against NaN is false.
    ///
    /// The multiply happens BEFORE the divide on purpose. Written that way
    /// it is one correctly rounded operation: the rounded result of the
    /// exact expression, with no error of its own to contribute. Hoisting
    /// the ratio into a per-frame constant instead (`k = (dots - 1) / span`,
    /// then `v = (x - origin) * k`) trades that away — `k` is rounded first,
    /// and the error is then scaled by the coordinate, so a value that lands
    /// exactly on a `.5` tie can round to the neighbouring dot. Measured on
    /// this grid: 4 of 149 tie values at 200 dots and 23 of 146 at 199 dots
    /// flipped a whole cell, while this form flipped none. It is a
    /// micro-optimization worth ~1 ns per coordinate — do not take it.
    #[inline]
    fn dot(
        x: f64,
        origin: f64,
        dots: usize,
        span: f64,
        inside: impl FnOnce() -> bool,
    ) -> Option<usize> {
        if !inside() {
            return None;
        }
        let v = (x - origin) * (dots as f64 - 1.0) / span;
        (v >= 0.0 && v <= dots as f64 - 1.0).then(|| v.round() as usize)
    }

    /// Canvas `x` -> dot column: measured up from the window's left edge.
    #[inline]
    fn dot_x(&self, x: f64) -> Option<usize> {
        Self::dot(x, self.left, self.px_w, self.width(), || {
            x >= self.left && x <= self.right
        })
    }

    /// Canvas `y` (+y UP) -> dot row (down): measured down from the window's
    /// top, so the sign flips here rather than at every call site.
    #[inline]
    fn dot_y(&self, y: f64) -> Option<usize> {
        Self::dot(self.top - y, 0.0, self.px_h, self.height(), || {
            y >= self.bottom && y <= self.top
        })
    }
}

/// Rasterize one line segment into a 2x4-dot-per-cell braille grid.
///
/// * `(x1, y1) -> (x2, y2)` — canvas coordinates (+y up).
/// * `vp` — the grid and the window to clip and map against; see
///   [`Viewport`]. Build it once per frame and reuse it for every line.
/// * `on_cell(cell, bit)` — called for every dot on the line with its cell
///   index and pattern bit; the caller owns the buffers (pattern + color).
///   It is called even for dots that are already lit, so a later line can
///   overwrite the cell's color — exactly like the Canvas layer order.
#[inline]
pub fn rasterize_line(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    vp: Viewport,
    mut on_cell: impl FnMut(usize, u8),
) {
    let Some((cx1, cy1, cx2, cy2)) = clip_line(x1, y1, x2, y2, vp) else {
        return;
    };
    // A degenerate window (zero/negative span — what a `--width 0` canvas
    // derives) maps nothing. It has to be checked before mapping: the
    // per-coordinate range test below already rejects a non-finite
    // coordinate, but a plain `0 / 0` span leaves `v` NaN for a point
    // sitting exactly on the window edge, and `f64 as usize` would turn
    // that into dot 0.
    if vp.width() <= 0.0 || vp.height() <= 0.0 {
        return;
    }
    // A non-finite coordinate has to be rejected HERE, and it is the one
    // case `clip_line` cannot see: every region-code comparison against NaN
    // is false, so a NaN point comes out of the clipper looking like a point
    // inside the window. Without this it maps to a real-looking cell (the
    // `as usize` cast turns NaN into 0) and the segment is drawn as a line
    // invented out of nothing.
    if !(cx1.is_finite() && cy1.is_finite() && cx2.is_finite() && cy2.is_finite()) {
        return;
    }
    // One endpoint at a time, exactly as the pair of `get_point` calls this
    // replaced. Mapping all four coordinates in one pass against spans
    // derived once is the obvious cleanup and measured 26-34% SLOWER here:
    // each endpoint's two divisions currently schedule against the other
    // endpoint's, and splitting x from y across the line loses that overlap.
    // Every `dot_x`/`dot_y` call therefore re-derives its span, and that is
    // the fast shape.
    let Some(dx1) = vp.dot_x(cx1) else {
        return;
    };
    let Some(dy1) = vp.dot_y(cy1) else {
        return;
    };
    let Some(dx2) = vp.dot_x(cx2) else {
        return;
    };
    let Some(dy2) = vp.dot_y(cy2) else {
        return;
    };
    let (px_w, px_h) = (vp.px_w(), vp.px_h());
    bresenham(dx1, dy1, dx2, dy2, |x, y| {
        if x >= px_w || y >= px_h {
            return;
        }
        let cell = (y >> 2) * (px_w / 2) + (x >> 1);
        on_cell(cell, 1 << ((x & 1) + 2 * (y & 3)));
    });
}

/// The braille char for a lit pattern (0..=255); empty cells are handled by
/// [`dots_to_lines`], which emits a plain space instead of U+2800.
#[inline]
pub fn braille_char(pattern: u8) -> char {
    BRAILLE[pattern as usize]
}

/// Clip one segment against the camera's near plane and rasterize the
/// visible part into a braille grid.
///
/// `a` and `b` are camera-space points (`[x, y, z]`, `z` = distance in front
/// of the camera, with `z <= NEAR` behind it, see
/// [`crate::projection::NEAR`]); `project` maps such a point to canvas
/// coordinates — for [`crate::projection::Camera`] that is the focal divide
/// `[x, y] * focal / z`. [`crate::projection::project_camera_point`] is the
/// free-function form of it.
///
/// Dropping a segment because ONE endpoint sits behind the camera plane
/// throws away the part that is still in view: the whole segment vanishes.
/// This clips the segment at the plane first and rasterizes the remainder
/// through the SAME `project`, so the visible part lands exactly where its
/// unclipped projection would. A segment entirely behind the plane draws
/// nothing, and `project` is only ever called with points in front of the
/// plane, so it never has to divide by a zero depth.
pub fn rasterize_camera_line(
    a: [f64; 3],
    b: [f64; 3],
    project: impl Fn([f64; 3]) -> (f64, f64),
    vp: Viewport,
    on_cell: impl FnMut(usize, u8),
) {
    let Some((p1, p2)) = clip_near(a, b) else {
        return;
    };
    let (x1, y1) = project(p1);
    let (x2, y2) = project(p2);
    rasterize_line(x1, y1, x2, y2, vp, on_cell);
}

/// Clip a camera-space segment at the near plane, returning two
/// camera-space points that are both in front of it (`z > NEAR`).
///
/// `None` when the whole segment is behind the plane. This is what
/// [`rasterize_camera_line`] clips with; callers that want the clipped
/// endpoint itself (to hang a label on, say) can call it directly.
pub fn clip_near(a: [f64; 3], b: [f64; 3]) -> Option<([f64; 3], [f64; 3])> {
    let near = crate::projection::NEAR;
    let behind_a = a[2] <= near;
    let behind_b = b[2] <= near;
    if behind_a && behind_b {
        return None;
    }
    if !behind_a && !behind_b {
        return Some((a, b));
    }
    // One endpoint is behind the plane: solve `lerp(front, behind, t).z == near`.
    let (front, behind) = if behind_a { (b, a) } else { (a, b) };
    let t = (near - front[2]) / (behind[2] - front[2]);
    let hit = [
        front[0] + t * (behind[0] - front[0]),
        front[1] + t * (behind[1] - front[1]),
        near,
    ];
    if behind_a {
        Some((hit, b))
    } else {
        Some((a, hit))
    }
}

/// Encode a dot grid (`cw * ch` per-cell patterns, row-major) as braille
/// lines. A zero pattern becomes a space — that is what ratatui's Canvas
/// leaves in an empty buffer cell, so text output stays byte-identical.
pub fn dots_to_lines(dots: &[u8], cw: usize, ch: usize) -> Vec<String> {
    debug_assert_eq!(dots.len(), cw * ch);
    (0..ch)
        .map(|cy| {
            (0..cw)
                .map(|cx| {
                    let p = dots[cy * cw + cx];
                    if p == 0 { ' ' } else { BRAILLE[p as usize] }
                })
                .collect()
        })
        .collect()
}

/// Cohen-Sutherland clip against the viewport's window — the same
/// algorithm, region order and intersection expressions as the
/// `line_clipping` crate ratatui uses, so the visible part lands where the
/// Canvas path puts it.
///
/// The endpoint it had to move lands exactly ON a window edge, or inside;
/// the endpoint that was already inside passes through untouched. That is
/// why a caller cannot skip its own window test on the strength of having
/// clipped (see [`rasterize_line`]).
fn clip_line(x1: f64, y1: f64, x2: f64, y2: f64, vp: Viewport) -> Option<(f64, f64, f64, f64)> {
    let (left, right, bottom, top) = (vp.left, vp.right, vp.bottom, vp.top);
    let mut p1 = (x1, y1);
    let mut p2 = (x2, y2);
    let mut r1 = region_code(p1, left, right, bottom, top);
    let mut r2 = region_code(p2, left, right, bottom, top);
    loop {
        if r1 & r2 != 0 {
            return None;
        }
        if r1 != 0 {
            p1 = intersect(p1, p2, r1, left, right, bottom, top);
            r1 = region_code(p1, left, right, bottom, top);
        } else if r2 != 0 {
            p2 = intersect(p2, p1, r2, left, right, bottom, top);
            r2 = region_code(p2, left, right, bottom, top);
        } else {
            return Some((p1.0, p1.1, p2.0, p2.1));
        }
    }
}

fn region_code(p: (f64, f64), left: f64, right: f64, bottom: f64, top: f64) -> u8 {
    let mut r = 0u8;
    if p.0 < left {
        r |= 1;
    } else if p.0 > right {
        r |= 2;
    }
    if p.1 < bottom {
        r |= 4;
    } else if p.1 > top {
        r |= 8;
    }
    r
}

/// Intersect the segment with the window boundary identified by `code`.
fn intersect(
    p1: (f64, f64),
    p2: (f64, f64),
    region: u8,
    left: f64,
    right: f64,
    bottom: f64,
    top: f64,
) -> (f64, f64) {
    let dx = p2.0 - p1.0;
    let dy = p2.1 - p1.1;
    if region & 1 != 0 {
        let y = p1.1 + (left - p1.0) * dy / dx;
        return (left, y);
    }
    if region & 2 != 0 {
        let y = p1.1 + (right - p1.0) * dy / dx;
        return (right, y);
    }
    if region & 4 != 0 {
        let x = p1.0 + (bottom - p1.1) * dx / dy;
        return (x, bottom);
    }
    debug_assert!(region & 8 != 0);
    let x = p1.0 + (top - p1.1) * dx / dy;
    (x, top)
}

/// Bresenham line stepping, byte-identical to ratatui's
/// `for_each_line_point`.
fn bresenham(x1: usize, y1: usize, x2: usize, y2: usize, mut f: impl FnMut(usize, usize)) {
    let dx = x2.abs_diff(x1);
    let dy = y2.abs_diff(y1);
    if dx == 0 {
        for y in y1.min(y2)..=y1.max(y2) {
            f(x1, y);
        }
    } else if dy == 0 {
        for x in x1.min(x2)..=x1.max(x2) {
            f(x, y1);
        }
    } else if dy < dx {
        if x1 > x2 {
            line_low(x2, y2, x1, y1, &mut f);
        } else {
            line_low(x1, y1, x2, y2, &mut f);
        }
    } else if y1 > y2 {
        line_high(x2, y2, x1, y1, &mut f);
    } else {
        line_high(x1, y1, x2, y2, &mut f);
    }
}

fn line_low(x1: usize, y1: usize, x2: usize, y2: usize, f: &mut impl FnMut(usize, usize)) {
    let dx = (x2 - x1) as isize;
    let dy = (y2 as isize - y1 as isize).abs();
    let mut d = 2 * dy - dx;
    let mut y = y1;
    for x in x1..=x2 {
        f(x, y);
        if d > 0 {
            y = if y1 > y2 {
                y.saturating_sub(1)
            } else {
                y.saturating_add(1)
            };
            d -= 2 * dx;
        }
        d += 2 * dy;
    }
}

fn line_high(x1: usize, y1: usize, x2: usize, y2: usize, f: &mut impl FnMut(usize, usize)) {
    let dx = (x2 as isize - x1 as isize).abs();
    let dy = (y2 - y1) as isize;
    let mut d = 2 * dx - dy;
    let mut x = x1;
    for y in y1..=y2 {
        f(x, y);
        if d > 0 {
            x = if x1 > x2 {
                x.saturating_sub(1)
            } else {
                x.saturating_add(1)
            };
            d -= 2 * dy;
        }
        d += 2 * dx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect the dots a single line lights, as (cell, bit) pairs.
    fn dots_of(x1: f64, y1: f64, x2: f64, y2: f64, vp: Viewport) -> Vec<(usize, u8)> {
        let mut v = Vec::new();
        rasterize_line(x1, y1, x2, y2, vp, |cell, bit| v.push((cell, bit)));
        v
    }

    #[test]
    fn braille_table_is_ratatui_permutation() {
        for (i, c) in BRAILLE.iter().enumerate() {
            assert!((0x2800u32..=0x28ff).contains(&(*c as u32)), "LUT[{i}]");
        }
        assert_eq!(BRAILLE[0x55], '\u{2847}');
        assert_eq!(BRAILLE[0x02], '\u{2808}');
        assert_eq!(BRAILLE[255], '\u{28ff}');
        assert_eq!(braille_char(0x01), BRAILLE[1]);
    }

    #[test]
    fn dots_to_lines_uses_spaces_for_empty_cells() {
        // ratatui's Canvas leaves the buffer's default space in empty
        // cells (its PatternGrid saves pattern 0 as "no symbol") — never
        // U+2800.
        assert_eq!(dots_to_lines(&[0, 0, 0, 0], 2, 2), ["  ", "  "]);
        assert_eq!(dots_to_lines(&[0x01, 0], 2, 1), ["⠁ "]);
        assert_eq!(dots_to_lines(&[0xff], 1, 1), ["⣿"]);
    }

    #[test]
    fn centered_line_lights_the_expected_cells() {
        // 4x2 cells = 8x8 dots; window [-4,4]x[-4,4].
        let vp = Viewport::centered(8, 8);
        // A horizontal line across the middle row of dots (y=0 -> dot row 3).
        let dots = dots_of(-4.0, 0.0, 4.0, 0.0, vp);
        assert!(!dots.is_empty(), "a full-width line must light dots");
        // Every dot: cell = (y>>2)*4 + (x>>1), bit = (x&1) + 2*(y&3) — all
        // in range for an 8x8 grid.
        for (cell, bit) in &dots {
            assert!(*cell < 8, "cell {cell} out of range");
            assert!(bit.count_ones() == 1 && *bit != 0, "bit {bit}");
        }
        // Single dot at the origin maps to the center-ish dot of the grid.
        let dots = dots_of(0.0, 0.0, 0.0, 0.0, vp);
        assert_eq!(dots.len(), 1, "a point lights exactly one dot");
    }

    #[test]
    fn degenerate_bounds_draw_nothing() {
        // A zero-width/zero-height window (what a w=0/h=0 CLI canvas
        // derives) maps nothing, so the frame stays blank.
        let vp = Viewport::from_arrays(2, 4, [0.0, 0.0], [0.0, 0.0]);
        let dots = dots_of(-5.0, -5.0, 5.0, 5.0, vp);
        assert!(dots.is_empty(), "degenerate window must draw nothing");
        let vp = Viewport::from_arrays(2, 4, [-10.0, 10.0], [5.0, -5.0]); // inverted y
        let dots = dots_of(-5.0, -5.0, 5.0, 5.0, vp);
        assert!(dots.is_empty(), "inverted window must draw nothing");
    }

    #[test]
    fn off_window_lines_are_clipped_or_rejected() {
        let vp = Viewport::centered(8, 8);
        // Fully outside -> nothing.
        assert!(dots_of(100.0, 100.0, 200.0, 200.0, vp).is_empty());
        // Crossing the window -> clipped to it (endpoints inside).
        let dots = dots_of(-100.0, 0.0, 100.0, 0.0, vp);
        assert!(!dots.is_empty());
        // Partially outside, diagonal.
        let dots = dots_of(-50.0, -50.0, 1.0, 1.0, vp);
        assert!(!dots.is_empty());
    }

    #[test]
    fn dot_mapping_rejects_outside_and_maps_edges() {
        let vp = Viewport::centered(40, 32);
        // The window corners land on dot 0 and the last dot.
        assert_eq!(vp.dot_x(0.0), Some(20));
        assert_eq!(vp.dot_y(0.0), Some(16));
        assert_eq!(vp.dot_x(-20.0), Some(0));
        assert_eq!(vp.dot_y(16.0), Some(0));
        assert_eq!(vp.dot_x(20.0), Some(39));
        assert_eq!(vp.dot_y(-16.0), Some(31));
        // One step outside the window maps to nothing.
        assert_eq!(vp.dot_x(-20.1), None);
        assert_eq!(vp.dot_x(20.1), None);
        assert_eq!(vp.dot_y(16.1), None);
        assert_eq!(vp.dot_y(-16.1), None);
        // Non-finite coordinates map to nothing rather than to dot 0: the
        // mapping's own `v >= 0.0 && v <= last` test fails on NaN, which is
        // what keeps a NaN projection from lighting the top-left corner.
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, 1e300] {
            assert_eq!(vp.dot_x(bad), None, "dot_x({bad})");
            assert_eq!(vp.dot_y(bad), None, "dot_y({bad})");
        }
        // The same must hold for a whole line built from non-finite input.
        assert!(dots_of(f64::NAN, 0.0, 0.0, 0.0, vp).is_empty());
        assert!(dots_of(0.0, 0.0, f64::INFINITY, f64::INFINITY, vp).is_empty());
    }

    /// Collect the cells `rasterize_camera_line` lights. A scale of 1 makes
    /// the canvases below read as camera-space coordinates.
    fn camera_line_cells(a: [f64; 3], b: [f64; 3], scale: f64) -> Vec<usize> {
        let (px_w, px_h) = (40usize, 32usize);
        let mut cells = Vec::new();
        rasterize_camera_line(
            a,
            b,
            |p| crate::projection::project_camera_point(p, scale),
            Viewport::centered(px_w, px_h),
            |cell, _| cells.push(cell),
        );
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    #[test]
    fn camera_line_clips_at_the_near_plane() {
        // 40x32 dots with scale 1: the canvas covers x in [-20, 20].
        // A segment from (30, 0, 2) — off the canvas to the right — to
        // (2.5, 0, 0.05), whose second endpoint is behind the camera plane.
        // It crosses the plane at x = 5.286…, so the visible part runs from
        // the canvas edge in to x = 5.286: cells must be lit. Dropping the
        // segment with its culled endpoint lit nothing at all.
        let scale = 1.0;
        let cells = camera_line_cells([30.0, 0.0, 2.0], [2.5, 0.0, 0.05], scale);
        assert!(
            !cells.is_empty(),
            "the part of the segment in front of the camera must be drawn"
        );
        // The visible part ends mid-canvas, so the head of the clipped
        // segment is lit while the tail (off the canvas edge) is not: the
        // clipped segment spans a proper sub-run of the row, not the whole
        // canvas width.
        let (px_w, _px_h) = (40usize, 32usize);
        assert!(
            cells.len() < px_w / 2,
            "the clipped segment must not span the canvas: {cells:?}"
        );
        assert!(
            cells
                .iter()
                .all(|c| c / (px_w / 2) == cells[0] / (px_w / 2)),
            "the segment stays on one row: {cells:?}"
        );
        // A segment whose visible part lies entirely outside the canvas
        // draws nothing — clipping must not invent geometry.
        assert!(
            camera_line_cells([60.0, 0.0, 2.0], [40.0, 0.0, 0.05], scale).is_empty(),
            "an off-canvas visible part stays off canvas"
        );
    }

    #[test]
    fn camera_line_keeps_fully_visible_segments_identical() {
        // Both endpoints in front of the plane: this entry point must
        // rasterize exactly what `rasterize_line` does with the same
        // projection applied to the endpoints.
        let f = crate::projection::focal(32.0);
        let project = |p: [f64; 3]| crate::projection::project_camera_point(p, f);
        let (a, b) = ([3.0, -2.0, 6.0], [-4.0, 5.0, 9.0]);
        let want = {
            let (px_w, px_h) = (40usize, 32usize);
            let (x1, y1) = project(a);
            let (x2, y2) = project(b);
            let mut cells = Vec::new();
            rasterize_line(x1, y1, x2, y2, Viewport::centered(px_w, px_h), |cell, _| {
                cells.push(cell)
            });
            cells.sort_unstable();
            cells.dedup();
            cells
        };
        assert_eq!(camera_line_cells(a, b, f), want);
    }

    #[test]
    fn camera_line_behind_the_camera_draws_nothing() {
        // Both endpoints behind the plane: no dot, no panic (the clip
        // denominator would be zero or negative here).
        for b in [[0.0, 0.0, 0.05], [0.0, 0.0, 0.0], [0.0, 0.0, -5.0]] {
            assert!(
                camera_line_cells([1.0, 1.0, 0.1], b, 100.0).is_empty(),
                "segment behind the camera plane drew dots: b={b:?}"
            );
        }
    }

    #[test]
    fn camera_line_at_the_plane_does_not_produce_nan() {
        // An endpoint exactly ON the plane with the other one in front:
        // the interpolation divides by (behind - front).z, which is zero
        // when the "behind" endpoint sits exactly at NEAR. The clipped dot
        // grid must stay a real coordinate (no NaN cell index).
        let cells = camera_line_cells([0.0, 0.0, 1.0], [10.0, 0.0, crate::projection::NEAR], 1.0);
        assert!(!cells.is_empty(), "the segment in front must still draw");
        assert!(
            cells.iter().all(|&c| c < 40 * 32),
            "cell indices must stay in the grid: {cells:?}"
        );
    }
}
