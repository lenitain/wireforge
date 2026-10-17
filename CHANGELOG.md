# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The viewer opens with an empty canvas when no `FILE` is given.
- A lualine-style statusline in the viewer.
- `R` resets the view in the viewer.
- Vertical FOV is a renderer parameter, defaulting to 1.0 rad. **Breaking** for
  anyone reading the renderer internals.
- Example asset `utah-teapot.wrfm`.
- `wrfm fmt <FILE>` re-emits a `.wrfm` file in canonical v2 surface form
  (shortest round-trip coordinates, single blank separators, BOM stripped,
  CRLF→LF). `--check` exits 1 if re-canonicalization would change bytes
  (mirrors `cargo fmt --check`); `-q` silences stdout. Decorative `#`
  comments are dropped in this version — preservation is a planned
  addition.
- `wrfm` parser rejects non-finite (`±inf`, `NaN`) vertex coordinates with the
  new `NotFinite` error variant.
- Fixture-driven parser tests in `crates/wrfm/tests/fixtures/` — 44 `.wrfm`
  files plus a single manifest of expected outcomes; `.gitattributes` pins the
  CRLF fixture as `binary` so `core.autocrlf=input` does not mangle it.

### Changed

- `d`/`f` roll the model instead of rotating the camera. **Breaking.**
- `Viewport` folds the former `grid` and `window` into one. **Breaking.**
- The authoritative `.wrfm` v2 format spec lives at
  `crates/wrfm/docs/wrfm-spec.md`. The root `SPEC.md`, `crates/wrfm/README.md`,
  and the CLI's inline `FORMAT_SPEC` are pointers now. **Breaking** for any
  downstream doc that scraped the old locations.

### Fixed

- HUD draws the model around the help panel.
- Segments are clipped at the near plane.
- Tarjan bridge walk runs on an explicit stack.
- `d`/`f` roll the right way round. **Breaking** for anyone who relied on the old
  sign.
- Camera roll drawing is restored after the near-plane refactor.
- Help panel no longer clips on narrow terminals.

### Performance

- `wrfm` parses with one token buffer, reserves model vectors from header counts,
  and stores edge lines as spans rather than string copies.
- `render` fuses the `f32` projection pass, refills buffers lazily, reuses the
  parallel scratch grids instead of reallocating, and clips segments at the near
  plane.
- `viewer` reads the model file once and probes only its head; caches the model
  extent and sizes the raster canvas only on `Resize`.
- `check` indexes edges once and detects redundant / dangling edges in a single
  pass.
- `cli` looks up symmetry by position and clips the view sampler to its grid.

### Refactor

- Drop `#[allow(dead_code)]` from test-only render accessors.
- Viewer render tests pin `vfov = 1.0`, decoupling the projection-coord
  assertions from `DEFAULT_VFOV` drift.
- Duplicated fit-content and vfov tests removed from
  `crates/wrfm-cli/src/render.rs` and `crates/wrfm-cli/tests/cli.rs`; the
  remaining `cli.rs` tests cover the same behaviour.

## [0.8.0] - 2026-10-09

`.wrfm` v2, a workspace split, and a viewer rewrite. Several public surfaces
moved, narrowed, or disappeared — see the **Breaking** notes throughout.

### Added

- `.wrfm` v2 format — magic line, counts header, groups, structured errors.
  **Breaking.**
- `wrfm-cli` gains `obj → wrfm` conversion and auto-detects the convert input
  format from content. **Breaking** for the input-format calls.
- `cli` narrows degree-2 health to collinear midpoints. **Breaking.**
- Viewer reads stdin when `FILE` is omitted.
- Viewer gains layout-independent chord keys (`wsad`-style).
- `wireforge` replaces the `x` reload panel with a one-line status row.

### Changed

- Repo is now a Cargo workspace with a shared `wrfm-raster` renderer.
  **Breaking.**
- `wrfm` lib split into `model`, `parse`, `error`, `tests` modules.
- `wrfm` drops `parse_with` strict mode. **Breaking.**
- The wireforge model is owned locally; `ratatui-wireframe` is gone.
  **Breaking.**
- `wrfm-cli` unifies the rules and adds `--weld` and `--fit` content.
- Viewer drops the OBJ / `ratty` rendering path. **Breaking.**
- Viewer unifies direction signs — body-frame keys, object-side views, yaw sign.
  **Breaking.**
- Viewer reads plain keys in the viewer's frame. **Breaking.**

### Removed

- File hot-reload and the `inotify` dependency from `wireforge`. **Breaking.**

### Fixed

- Input unifies key handling and fixes stuck holds.
- Input regressions from the kitty keyboard protocol.
- Viewer points the OBJ hint at `wrfm convert <file>`.
- Timer fires due timers on the idle event path.
- Pre-commit hooks check and format every crate in the workspace.
- Workspace pins the `wrfm-raster` version for publishing.

### Performance

- Build the binary with radical options.

## [0.7.0] - 2026-08-19

### Added

- `wrfm-cli` — streaming `check`, `info`, `group`, `geometry`, `query`, `view`,
  `render`, `transform`, `edit`, `diff` tool.
- `.wrfm` v1 format — magic line, counts header, groups, structured errors.
- `wireforge` gets a 6-DOF camera, HUD, stream input, and an event-driven render
  loop.

### Changed

- Projection / bounds run on parallel iterators, timer extraction, and motion is
  unified in one module.

### Fixed

- HUD help overlay arrows revert to ASCII for terminal compatibility.

## [0.6.0] - 2026-06-13

### Added

- 3D object viewing through `wrgp`.

## [0.5.0] - 2026-06-01

### Added

- `wrfm` crate with `no_std` support and a `read_from_string` entry point.
- Initial commit of the wireforge project.
- First cut of the viewer.
- CI on GitHub Actions for the workspace.

### Changed

- File extension renamed `wfrm` → `wrfm` across the workspace.

### Build

- `LICENSE` and crate metadata added for publishing.
