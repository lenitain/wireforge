# Changelog

All notable changes to `wrfm-cli` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`wrfm-cli` versions on its own axis; every version heading below is the version
this crate's `Cargo.toml` carried in git on that date. Sibling histories:
[wireforge](../../CHANGELOG.md), [wrfm](../wrfm/CHANGELOG.md),
[wrfm-raster](../wrfm-raster/CHANGELOG.md).

## [Unreleased]

## [0.2.0] - 2026-10-19

### Added

- `wrfm convert` turns OBJ models into `.wrfm`.
- `wrfm fmt [--check] [-q]` re-emits a `.wrfm` file in canonical v2 surface form
  (shortest round-trip coordinates, single blank separators, BOM stripped,
  CRLF→LF). `--check` exits 1 if the file is not already in canonical form
  (mirrors `cargo fmt --check`; a leading BOM and CRLF line endings are
  normalized away first, so they never count as a difference); `-q` silences
  stdout. `fmt` reports the surface only and carries no health tier, like
  `diff` / `format`. Decorative `#` comments are dropped in this version —
  preservation is a planned addition.
- `edit --weld TOL` merges vertices closer than `TOL`, and `render --fit
  content` frames each view so it fills the canvas.

### Changed

- The rules are unified: point identity is defined once (`POINT_TOL = 1e-6`,
  strict `<`), every exit code has one meaning, and every finding maps to one
  repair action. **Breaking.**
- Degree-2 health is narrowed to collinear midpoints (`non-manifold` renamed
  `redundant`), and `verify --expect-redundant N` declares an accepted exact
  count. **Breaking.**
- `convert` auto-detects its input format from content (`.wrfm` magic vs OBJ
  geometry records) and loses `--from` / `--to`, so stdin works like files and a
  mistyped path cannot yield a silent empty model. **Breaking.**
- The tool reads and writes `.wrfm` v2. **Breaking.**
- Direction signs are unified — object-side views and the yaw sign. **Breaking.**
- The render projection parameterizes the vertical FOV in radians, defaulting to
  0.5.

### Fixed

- Tarjan bridge walk runs on an explicit stack.
- Segments are clipped at the near plane in `render`.
- Camera roll drawing is restored after the near-plane refactor.

### Performance

- `check` indexes edges once and detects redundant / dangling edges in a single
  pass.
- Symmetry lookup is by position, and the view sampler is clipped to its grid.
- Incident-edge tables and grid cells are reused across the check pipeline; the
  90k-vertex verdict no longer spends ~11 s recomputing exact duplicates.

## [0.1.0] - 2026-08-11

### Added

- The `wrfm` CLI lands as a streaming tool: `check`, `info`, `group`,
  `geometry`, `query`, `view`, `render`, `transform`, `edit`, `diff`. Every
  model-input command accepts `-` for stdin; nothing writes a file.
