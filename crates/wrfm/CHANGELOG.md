# Changelog

All notable changes to `wrfm` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`wrfm` versions on its own axis; every version heading below is the version this
crate's `Cargo.toml` carried in git on that date. Sibling histories:
[wireforge](../../CHANGELOG.md), [wrfm-cli](../wrfm-cli/CHANGELOG.md),
[wrfm-raster](../wrfm-raster/CHANGELOG.md).

## [Unreleased]

### Changed

- The parser rejects non-finite (`±inf`, `NaN`) vertex coordinates with the new
  `NotFinite` error variant instead of letting them reach bbox and projection.
  Fixture-driven parser tests in `tests/fixtures/` back the grammar: 45 `.wrfm`
  files plus a single manifest of expected outcomes.
- The authoritative `.wrfm` v2 format spec lives at
  [`docs/wrfm-spec.md`](docs/wrfm-spec.md). **Breaking** for any downstream
  doc that scraped the old locations.

### Performance

- Parsing reuses one token buffer for the whole parse, reserves the model
  vectors from the header counts, and recovers edge source positions lazily on
  the error path rather than storing per-edge metadata.
- Incident-edge tables use a `usize::MAX` sentinel (half the memory) and are
  reused across the check pipeline instead of rebuilt.

## [0.5.0] - 2026-10-09

### Changed

- The model format is now `.wrfm` v2 — magic line, counts header, groups, and
  structured `ParseError` (line/column/context) and `LoadError`. **Breaking.**
  An intermediate v1 format landed inside this cycle and never shipped as its
  own release.
- The library is split into `model`, `parse`, `error`, and `tests` modules.

### Removed

- `parse_with` strict mode. **Breaking.**

## [0.4.0] - 2026-06-01

### Added

- `no_std` support.

## [0.3.0] - 2026-05-27

### Added

- `WrfmModel::from_str` loads a model from a string.

### Build

- `LICENSE` and crate metadata (`description`, `keywords`, `categories`) added
  for publishing.

## [0.2.0] - 2026-05-27

### Added

- The `wrfm` crate lands with `WrfmModel` — `new`, `from_file`, and
  `save_to_file` for `.wrfm` 3D wireframe models.
