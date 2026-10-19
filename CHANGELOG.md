# Changelog

All notable changes to `wireforge` (the root package) and to workspace-wide
history are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and `wireforge` adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Every crate keeps its own release axis and its own changelog; entries below are
the ones this package or the shared workspace experienced:

- [wrfm](crates/wrfm/CHANGELOG.md)
- [wrfm-cli](crates/wrfm-cli/CHANGELOG.md)
- [wrfm-raster](crates/wrfm-raster/CHANGELOG.md)

Tags `v0.5.0`–`v0.9.0` mark `wireforge` releases; the crate-level axes are
untagged. Every version heading below is the version the root `Cargo.toml`
carried in git on that date.

## [Unreleased]

## [0.9.0] - 2026-10-19

### Added

- The viewer opens with an empty canvas when no `FILE` is given.
- A lualine-style statusline in the viewer, with the hints trimmed to `?` and
  `q`.
- `r` resets the view in the viewer.
- Example asset `utah-teapot.wrfm`.

### Changed

- Vertical FOV is a renderer parameter in radians (`vfov`), defaulting to 0.5.
  **Breaking** for anyone reading the renderer internals.
- `d`/`f` roll the model instead of rotating the camera. **Breaking.**
- Pan and dolly speed are pinned to the initial distance (they close in 5 s at
  the initial pose), and the fit margin is tightened (2.0 → 0.7), putting the
  model at ~71% of canvas height.
- `Viewport` folds the former `grid` and `window` into one. **Breaking.**

### Fixed

- HUD draws the model around the help panel.
- Segments are clipped at the near plane.
- Camera roll drawing is restored after the near-plane refactor.
- `d`/`f` roll the right way round. **Breaking** for anyone who relied on the
  old sign.
- The help panel no longer clips on narrow terminals.

### Performance

- The viewer reads the model file once and probes only its head, caches the
  model extent, and sizes the raster canvas only on `Resize`.
- Per-frame heap allocations in labels and telemetry are gone.
- The `f32` projection pass is fused, buffers refill lazily, and the parallel
  scratch grids are reused.

### Refactor

- The viewer replaces `clap` with a hand-rolled argument parser.
- Test-only render accessors drop `#[allow(dead_code)]`.

### Build

- The changelog is maintained by hand again; the changelog bot workflow is
  gone.

## [0.8.0] - 2026-10-09

### Added

- The viewer reads stdin when `FILE` is omitted.
- Layout-independent chord keys (`wsad`-style).

### Changed

- The model format is now `.wrfm` v2 — magic line, counts header, groups, and
  structured errors. **Breaking.**
- The repo is a Cargo workspace; the shared renderer is extracted as
  `wrfm-raster`.
- The viewer owns its model type; the `ratatui-wireframe` dependency is gone.
- The `x` reload panel becomes a one-line status row.
- The viewer drops the OBJ / `ratty` rendering path. **Breaking.**
- Viewer direction signs are unified — body-frame keys, object-side views, yaw
  sign. **Breaking.**
- The viewer reads plain keys in the viewer's frame. **Breaking.**

### Removed

- File hot-reload and the `inotify` dependency. **Breaking.**

### Fixed

- Input handling is unified, and stuck holds are fixed.
- Regressions from the kitty keyboard protocol are fixed.
- The timer fires due timers on the idle event path.
- The OBJ hint points at `wrfm convert <file>`.

### Build

- Pre-commit hooks run `cargo fmt` / `clippy` per crate and check every crate in
  the workspace.
- The `wrfm-raster` version is pinned for publishing.
- The release profile builds with fat LTO, one codegen unit, `panic = "abort"`,
  and stripped symbols.

## [0.7.0] - 2026-08-19

### Added

- A 6-DOF camera, HUD, stream input, and an event-driven render loop.
- `wrfm-cli` — a streaming `check` / `info` / `group` / `geometry` / `query` /
  `view` / `render` / `transform` / `edit` / `diff` tool — joins the repo.

### Changed

- Models are `.wrfm` v1 — magic line, counts header, groups, structured errors.
  **Breaking.**

### Fixed

- The HUD help overlay arrows revert to ASCII for terminal compatibility.

### Performance

- Projection and bounds run on parallel iterators; the timer is its own module;
  motion is unified.

## [0.6.0] - 2026-06-13

### Added

- 3D object viewing via the `ratty` feature (`ratatui-ratty`), with bundled OBJ
  models (`model.obj`, `mouse.obj`).

## [0.5.0] - 2026-06-01

### Changed

- The `wrfm` dependency gains `no_std` support (ships as `wrfm` 0.4.0).

## [0.4.0] - 2026-05-28

### Changed

- The model file extension is renamed `wfrm` → `wrfm`. **Breaking.**

### Build

- `LICENSE` and crate metadata added for publishing.

## [0.3.0] - 2026-05-27

### Added

- The viewer renders `wrfm` models in the terminal, with file hot-reload.

### Build

- CI on GitHub Actions.

## [0.1.0] - 2026-05-27

### Added

- Initial commit.

[Unreleased]: https://github.com/lenitain/wireforge/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/lenitain/wireforge/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/lenitain/wireforge/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/lenitain/wireforge/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/lenitain/wireforge/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/lenitain/wireforge/releases/tag/v0.5.0
