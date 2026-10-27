# Changelog

All notable changes to `wrfm-raster` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`wrfm-raster` versions on its own axis; every version heading below is the
version this crate's `Cargo.toml` carried in git on that date. Sibling histories:
[wireforge](../../CHANGELOG.md), [wrfm](../wrfm/CHANGELOG.md),
[wrfm-cli](../wrfm-cli/CHANGELOG.md).

## [Unreleased]

## [0.3.0] - 2026-10-27

### Changed

- The FOV constants speak both surface units: `DEFAULT_VFOV_DEG = 30.0` is the
  default vertical FOV in degrees (the unit CLI parameters and TUI readouts
  use) and `DEFAULT_VFOV` derives from it — 30° in radians, the unit every
  projection entry point takes. `MAX_VFOV` (3.0 radians) is replaced by
  `MAX_VFOV_DEG = 180.0`, the exclusive upper bound the CLI surfaces validate
  against.

## [0.2.0] - 2026-10-19

### Added

- Model-extent helpers, so a viewer can measure the bounding box once per model
  instead of rescanning per frame.
- Extent and auto-fit handle an empty model, so a viewer can start on an empty
  canvas.

### Changed

- `Viewport` folds the former `grid` and `window` into one. **Breaking.**
- Vertical FOV is a projection parameter in radians (`DEFAULT_VFOV`, `MAX_VFOV`),
  defaulting to 0.5. **Breaking.**
- Batch projection fails fast on length mismatch.
- The auto-fit margin is tightened (2.0 → 0.7), putting the model at ~71% of
  canvas height.
- The crate owns its `Model` type; the `ratatui-wireframe` dependency is gone.

### Fixed

- Segments are clipped at the near plane.
- Camera roll drawing is restored after the near-plane refactor.

### Performance

- The `f32` projection pass is fused and buffers refill lazily.

## [0.1.0] - 2026-10-09

### Added

- The crate lands, extracted from `wireforge` and `wrfm-cli`: camera projection,
  braille rasterization, and shared geometry helpers.
