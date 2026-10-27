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

Tags `v0.5.0`–`v0.9.1` mark `wireforge` releases; the crate-level axes are
untagged. Every version heading below is the version the root `Cargo.toml`
carried in git on that date.

## [Unreleased]

## [0.9.1] - 2026-10-27

### Added

- An `Esc` menu hub with Key Bindings, About and Exit, plus an About panel.
  Panels opened from the menu return there on `Esc`; `q` and `Ctrl+C` quit from
  anywhere.
- One bar under the model, laid out like a powerline statusline: the model's
  name in an accent pill, the state lamps in a block of their own, and the
  camera readout in degrees, zoom multiples and model units.
- A generated help panel: the page is built from the binding table, so a
  rebind, a new action or a reworded description is one edit.

### Changed

- `--vfov` now takes **degrees** (default 30, valid range (0, 180), decimals
  welcome) — the unit the camera readout already spoke. The projection keeps
  working in radians internally and the boundary converts, as it already did
  for pitch / yaw / roll; `wireforge model.wrfm --vfov 0.5` is now half a
  degree, not half a radian.
- Holding a motion key now steps from the slow gear to a fixed 2× fast gear
  after 200 ms. It is two gears, not continuous acceleration: releasing resets
  the next press to the slow gear, and rotation, pan and zoom share the same
  step. Auto-spin is unaffected; a terminal without key-up has no hold, so
  every event it sends is one finite step at that terminal's repeat cadence.
- Motion input is one state machine: a hold exists only while a key-up terminal
  says the key is down. The 10-second silence watchdog is gone, and a duplicate
  Press re-syncs a lost Release instead of being swallowed.
- The viewer reads terminal events through one input thread that reports read
  errors. The loop owns the session and ends it with the terminal restored
  instead of blocking forever on a dead input source; the periodic size check
  is also the terminal health check.
- The bar carries no key hints: the menu's Key Bindings is the only place the
  chords are listed. On a narrow row the lamps go first, then the readout sheds
  from the right.
- The bar's blocks are joined by the powerline separators (U+E0B0 and U+E0B1):
  the name pill tapers into the state lamps and those into the bar, the way
  lualine's sections do. The bar asks for a powerline (Nerd Font) terminal
  font.
- `Esc` now opens the menu (and closes or returns from its panels) instead of
  quitting; `q` and `Ctrl+C` quit from anywhere. `?` is unbound now that Key
  Bindings lives in the menu.
- `+` is the primary zoom-in chord (with `=` as the alias), and `-`/`_` zoom
  out.
- The telemetry labels on the bar (`yaw`, `pitch`, `roll`, `zoom`, `pan`) use a
  bright palette slot instead of the muted one, so the readout scans at a
  glance.
- The camera readout is a row of fixed columns: a value that grows (a minus
  sign, another digit) takes the room to its right, so the labels hold their
  places and the bar does not jump under the reader's feet. A value that
  outgrows its column is cut and marked with `…`.
- The key panel is laid out as one grid: a chord gutter measured over the whole
  table, group headings cut to their column's width as rules, and the frame
  named in the rotation headings (`ROTATE · world axes` / `· model axes`).
- The overlay panels (menu, Key Bindings, About) paint no ground of their own:
  they carry the terminal's own background, the one the transparent canvas
  shows, so panel and 3D space read as one surface. The menu's cursor is the
  `▸` marker and the accent ink instead of a selected band.

### Fixed

- The first motion key of a session is continuous on a terminal that reports
  key-up: the viewer probes for the keyboard protocol at start-up instead of
  waiting for the first Release, so a cold start no longer turns the first long
  press into a pause followed by tap-repeat.
- Idle wake no longer counts the blocked wait as one 100 ms motion frame; a
  slow frame uses its real elapsed time up to one second.
- A Release from any key teaches key-up support, so a still-held key's next
  Repeat starts its hold when the start-up probe was hidden.
- On a terminal that cannot report key-up, every Press and auto-repeat is one
  finite step instead of a one-second drift.
- `Ctrl+Shift+C` no longer quits (it is a terminal copy shortcut); `Ctrl+C`
  still quits.
- The key panel's right column starts at the same offset on every row: the
  offset was measured in bytes while the chords carry arrows, so every row
  holding an arrow shifted its right half two cells to the left.

## [0.9.0] - 2026-10-19

### Added

- The viewer opens with an empty canvas when no `FILE` is given.
- A lualine-style statusline in the viewer, with the hints trimmed to `?` and
  `q`.
- `r` resets the view in the viewer.
- Example model `wrfm_files/utah-teapot.wrfm`.

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

[Unreleased]: https://github.com/lenitain/wireforge/compare/v0.9.1...HEAD
[0.9.1]: https://github.com/lenitain/wireforge/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/lenitain/wireforge/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/lenitain/wireforge/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/lenitain/wireforge/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/lenitain/wireforge/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/lenitain/wireforge/releases/tag/v0.5.0
