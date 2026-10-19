# wrfm-raster

3D camera projection and braille rasterization.

The raster stage ports ratatui's `Canvas` algorithm: same Cohen–Sutherland
clipping, `Painter::get_point` scaling, Bresenham stepping and braille pattern
table. `tests/golden.rs` renders with both and asserts byte-for-byte equality
across centered, region-zoom and arbitrary windows.

## Modules

- `model` — the `Model` vertex/edge input every entry point takes
- `geometry` — rotations (`rot_x` / `rot_y` / `rot_z`, `rot_axis`, `world_rot`),
  `bounds`, `model_extent`, `auto_dist`, plus `DEFAULT_VFOV` / `MAX_VFOV` /
  `FIT_MARGIN`
- `projection` — `Camera` / `CameraF32`, `focal`, vertex → canvas projection
- `raster` — `Viewport`, `rasterize_line` / `rasterize_camera_line` (clip + Bresenham), `clip_near`, `dots_to_lines`

## Example

```rust
use wrfm_raster::geometry::{DEFAULT_VFOV, world_rot};
use wrfm_raster::projection::{focal, Camera};
use wrfm_raster::raster::{dots_to_lines, rasterize_line, Viewport};

let (cw, ch) = (20, 8);
let (px_w, px_h) = (cw * 2, ch * 4);
let cam = Camera::new(world_rot(0.0, 0.0), 8.0, 0.0, 0.0, 0.0);
let f = focal(px_h as f64, DEFAULT_VFOV);
let vp = Viewport::centered(px_w, px_h);
let mut dots = vec![0u8; cw * ch];

if let (Some(a), Some(b)) = (cam.project((-10.0, -5.0, 0.0), f),
                             cam.project((10.0, 5.0, 0.0), f)) {
    rasterize_line(a.0, a.1, b.0, b.1, vp,
                   |cell, bit| dots[cell] |= bit);
}

let lines = dots_to_lines(&dots, cw, ch);
```

## Out of scope

Rayon parallelism, the TUI's screen diff, `wrfm-cli`'s ASCII / density output.
Callers wrap `project_into` / `rasterize_line` in their own parallel loops.

## License

MIT.
