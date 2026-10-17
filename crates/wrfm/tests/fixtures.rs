//! Fixture-driven tests for the .wrfm v2 parser.
//!
//! The fixtures live in `tests/fixtures/{valid,invalid}/`. Each fixture is a
//! standalone `.wrfm` byte stream. A single manifest at
//! `tests/fixtures/manifest` describes the expected parse result for each one.
//!
//! The point of these tests is twofold:
//!
//! 1. They exercise the parser byte-for-byte from disk, so any byte change in
//!    a fixture is a real change in what the spec says is acceptable.
//! 2. They are language-agnostic: a Python, Go or JavaScript parser can be
//!    validated against the same fixture set, because the fixtures are plain
//!    text and the manifest is plain text.

use std::path::{Path, PathBuf};

use wrfm::{CountKind, EdgeError, ParseError, VertexError, WrfmModel};

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// Expected outcome for a single fixture, decoded from one section of the
/// manifest. Any field that is `None` is not checked (the parser may return
/// anything for it).
#[derive(Debug, Default)]
struct Fixture {
    expect: Option<String>,
    vertices: Option<usize>,
    edges: Option<usize>,
    groups: Option<usize>,
    error: Option<String>,
    detail: Option<String>,
    axis: Option<String>,
    which: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
    token: Option<String>,
}

/// A minimal manifest parser: `[section]` headers, `key = value` fields, `#`
/// comments, blank lines ignored. Unknown keys are an error so typos surface
/// at parse time.
fn parse_manifest(content: &str) -> Vec<(String, Fixture)> {
    let mut out = Vec::new();
    let mut current: Option<(String, Fixture)> = None;
    for (idx, raw) in content.lines().enumerate() {
        let lineno = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some(prev) = current.take() {
                out.push(prev);
            }
            current = Some((rest.to_string(), Fixture::default()));
            continue;
        }
        let cur = current.as_mut().unwrap_or_else(|| {
            panic!("line {lineno}: `key = value` outside any section: {line:?}")
        });
        let (k, v) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("line {lineno}: expected `key = value`, got {line:?}"));
        let k = k.trim();
        let v = v.trim();
        let target = &mut cur.1;
        let string = || strip_quotes(v);
        match k {
            "expect" => target.expect = Some(string()),
            "vertices" => target.vertices = Some(parse_usize(v, lineno, "vertices")),
            "edges" => target.edges = Some(parse_usize(v, lineno, "edges")),
            "groups" => target.groups = Some(parse_usize(v, lineno, "groups")),
            "error" => target.error = Some(string()),
            "detail" => target.detail = Some(string()),
            "axis" => target.axis = Some(string()),
            "which" => target.which = Some(string()),
            "line" => target.line = Some(parse_usize(v, lineno, "line")),
            "column" => target.column = Some(parse_usize(v, lineno, "column")),
            "token" => target.token = Some(string()),
            other => panic!("line {lineno}: unknown key {other:?}"),
        }
    }
    if let Some(prev) = current.take() {
        out.push(prev);
    }
    out
}

fn parse_usize(s: &str, lineno: usize, name: &str) -> usize {
    s.parse()
        .unwrap_or_else(|_| panic!("line {lineno}: bad {name}: {s:?}"))
}

/// If a string value is surrounded by matching `"` or `'` quotes, return the
/// inner text. Otherwise return it as-is. This lets the manifest write
/// `token = "abc"` or `token = abc` interchangeably.
fn strip_quotes(s: &str) -> String {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Verify a single parsed error against the manifest entry. Each manifest
/// field, when present, is asserted; absent fields are unchecked.
fn check_reject(name: &str, err: &ParseError, fx: &Fixture) -> Result<(), String> {
    let expected_error = fx
        .error
        .as_deref()
        .ok_or_else(|| format!("{name}: reject case missing `error`"))?;

    let actual_name = variant_name(err);
    if actual_name != expected_error {
        return Err(format!(
            "{name}: error variant: expected {expected_error:?}, got {actual_name:?}"
        ));
    }

    if let Some(line) = fx.line
        && err.line() != line
    {
        return Err(format!("{name}: line: expected {line}, got {}", err.line()));
    }
    if let Some(col) = fx.column
        && err.column() != col
    {
        return Err(format!(
            "{name}: column: expected {col}, got {}",
            err.column()
        ));
    }

    match err {
        ParseError::InvalidVertex { detail, .. } => {
            let expected = fx
                .detail
                .as_deref()
                .ok_or_else(|| format!("{name}: InvalidVertex missing `detail`"))?;
            let actual = match detail {
                VertexError::MissingCoordinate { .. } => "MissingCoordinate",
                VertexError::NotANumber { .. } => "NotANumber",
                VertexError::NotFinite { .. } => "NotFinite",
            };
            if actual != expected {
                return Err(format!(
                    "{name}: vertex detail: expected {expected:?}, got {actual:?}"
                ));
            }
            if let Some(expected_axis) = &fx.axis {
                let axis = match detail {
                    VertexError::MissingCoordinate { axis } => *axis,
                    VertexError::NotANumber { axis, .. } => *axis,
                    VertexError::NotFinite { axis, .. } => *axis,
                };
                let expected_char = expected_axis
                    .chars()
                    .next()
                    .ok_or_else(|| format!("{name}: empty axis"))?;
                if axis != expected_char {
                    return Err(format!(
                        "{name}: vertex axis: expected {expected_char:?}, got {axis:?}"
                    ));
                }
            }
            if let Some(expected_token) = &fx.token {
                let actual_token = match detail {
                    VertexError::NotANumber { token, .. } => token.as_str(),
                    VertexError::NotFinite { token, .. } => token.as_str(),
                    _ => {
                        return Err(format!(
                            "{name}: `token` is only valid for NotANumber/NotFinite, got {actual:?}"
                        ));
                    }
                };
                if actual_token != expected_token {
                    return Err(format!(
                        "{name}: vertex token: expected {expected_token:?}, got {actual_token:?}"
                    ));
                }
            }
        }
        ParseError::InvalidEdge { detail, .. } => {
            let expected = fx
                .detail
                .as_deref()
                .ok_or_else(|| format!("{name}: InvalidEdge missing `detail`"))?;
            let (actual, which) = match detail {
                EdgeError::MissingIndex { which } => ("MissingIndex", *which),
                EdgeError::NotAnIndex { which, .. } => ("NotAnIndex", *which),
                EdgeError::NegativeIndex { which, .. } => ("NegativeIndex", *which),
                EdgeError::OutOfRange { which, .. } => ("OutOfRange", *which),
            };
            if actual != expected {
                return Err(format!(
                    "{name}: edge detail: expected {expected:?}, got {actual:?}"
                ));
            }
            if let Some(expected_which) = &fx.which {
                let expected: u8 = expected_which.parse().map_err(|_| {
                    format!("{name}: `which` must be 1 or 2, got {expected_which:?}")
                })?;
                if which != expected {
                    return Err(format!(
                        "{name}: edge which: expected {expected}, got {which}"
                    ));
                }
            }
            if let Some(expected_token) = &fx.token {
                let actual_token = match detail {
                    EdgeError::NotAnIndex { token, .. } => Some(token.as_str()),
                    _ => None,
                };
                if let Some(actual_token) = actual_token
                    && actual_token != expected_token
                {
                    return Err(format!(
                        "{name}: edge token: expected {expected_token:?}, got {actual_token:?}"
                    ));
                }
            }
        }
        ParseError::CountMismatch { which, .. } => {
            let expected = fx
                .which
                .as_deref()
                .ok_or_else(|| format!("{name}: CountMismatch missing `which`"))?;
            let actual = match which {
                CountKind::Vertices => "Vertices",
                CountKind::Edges => "Edges",
            };
            if actual != expected {
                return Err(format!(
                    "{name}: count which: expected {expected:?}, got {actual:?}"
                ));
            }
        }
        ParseError::MalformedMagic { token, .. } | ParseError::MalformedHeader { token, .. } => {
            if let Some(expected_token) = &fx.token
                && token != expected_token
            {
                return Err(format!(
                    "{name}: token: expected {expected_token:?}, got {token:?}"
                ));
            }
        }
        ParseError::UnsupportedVersion { token, .. } => {
            if let Some(expected_token) = &fx.token
                && token != expected_token
            {
                return Err(format!(
                    "{name}: version token: expected {expected_token:?}, got {token:?}"
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn variant_name(err: &ParseError) -> &'static str {
    match err {
        ParseError::MissingMagic { .. } => "MissingMagic",
        ParseError::MalformedMagic { .. } => "MalformedMagic",
        ParseError::UnsupportedVersion { .. } => "UnsupportedVersion",
        ParseError::MissingHeader { .. } => "MissingHeader",
        ParseError::MalformedHeader { .. } => "MalformedHeader",
        ParseError::CountMismatch { .. } => "CountMismatch",
        ParseError::InvalidVertex { .. } => "InvalidVertex",
        ParseError::InvalidEdge { .. } => "InvalidEdge",
        ParseError::MissingGroupName { .. } => "MissingGroupName",
    }
}

/// Validate every fixture in the manifest and aggregate failures so a single
/// test run reports every divergence, not just the first one.
#[test]
fn fixtures_match_manifest() {
    let fixtures_dir = Path::new(FIXTURES_DIR);
    let manifest_path = fixtures_dir.join("manifest");
    let manifest_text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!("read manifest at {manifest_path:?}: {e}");
    });
    let fixtures = parse_manifest(&manifest_text);
    assert!(
        !fixtures.is_empty(),
        "no fixtures parsed from manifest at {manifest_path:?}"
    );

    let mut failures: Vec<String> = Vec::new();
    let mut seen = 0;
    for (name, fx) in &fixtures {
        seen += 1;
        if let Err(msg) = check_one(name, fx, fixtures_dir) {
            failures.push(msg);
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} fixtures diverged from the manifest:\n  - {}",
        failures.len(),
        seen,
        failures.join("\n  - ")
    );
}

fn check_one(name: &str, fx: &Fixture, fixtures_dir: &Path) -> Result<(), String> {
    let path = fixtures_dir.join(name);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{name}: read {path:?}: {e}"))?;
    let model = WrfmModel::from_str(name, &text);
    let expect = fx
        .expect
        .as_deref()
        .ok_or_else(|| format!("{name}: missing `expect`"))?;
    match expect {
        "accept" => match model {
            Ok(m) => {
                if let Some(v) = fx.vertices
                    && m.vertices.len() != v
                {
                    return Err(format!(
                        "{name}: vertices: expected {v}, got {}",
                        m.vertices.len()
                    ));
                }
                if let Some(v) = fx.edges
                    && m.edges.len() != v
                {
                    return Err(format!(
                        "{name}: edges: expected {v}, got {}",
                        m.edges.len()
                    ));
                }
                if let Some(v) = fx.groups
                    && m.groups.len() != v
                {
                    return Err(format!(
                        "{name}: groups: expected {v}, got {}",
                        m.groups.len()
                    ));
                }
                Ok(())
            }
            Err(e) => Err(format!("{name}: expected accept, got {e:?}")),
        },
        "reject" => match model {
            Ok(m) => Err(format!(
                "{name}: expected reject, got accept (vertices={}, edges={})",
                m.vertices.len(),
                m.edges.len()
            )),
            Err(e) => check_reject(name, &e, fx),
        },
        other => Err(format!("{name}: unknown expect value {other:?}")),
    }
}

/// Sanity check: the parser accepts its own canonical output. This pairs the
/// fixtures with the writer so a divergence between parser and writer
/// surfaces immediately.
#[test]
fn canonical_writer_round_trip() {
    use wrfm::{Group, WrfmModel};

    let mut model = WrfmModel::new("cube");
    for &(x, y, z) in &[
        (-1.0, -1.0, -1.0),
        (1.0, -1.0, -1.0),
        (1.0, 1.0, -1.0),
        (-1.0, 1.0, -1.0),
        (-1.0, -1.0, 1.0),
        (1.0, -1.0, 1.0),
        (1.0, 1.0, 1.0),
        (-1.0, 1.0, 1.0),
    ] {
        model.vertices.push((x, y, z));
    }
    for &(a, b) in &[
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        model.edges.push((a, b));
    }
    model.groups.push(Group {
        name: "base".to_string(),
        vertex_start: 0,
        vertex_end: 8,
    });

    let text = model.to_string();
    let dir = std::env::temp_dir().join(format!("wrfm-fixture-roundtrip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rt.wrfm");
    std::fs::write(&path, &text).unwrap();
    let back = WrfmModel::from_file(&path).unwrap();
    assert_eq!(back.vertices, model.vertices);
    assert_eq!(back.edges, model.edges);
    assert_eq!(back.groups, model.groups);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Quick smoke test: every fixture file referenced in the manifest actually
/// exists. Catches a manifest typo before the parser-side test reports a
/// confusing "missing fixture" error.
#[test]
fn manifest_fixture_files_exist() {
    let fixtures_dir = PathBuf::from(FIXTURES_DIR);
    let manifest_text = std::fs::read_to_string(fixtures_dir.join("manifest")).unwrap();
    let fixtures = parse_manifest(&manifest_text);
    let mut missing: Vec<String> = Vec::new();
    for (name, _) in &fixtures {
        let path = fixtures_dir.join(name);
        if !path.exists() {
            missing.push(format!("{name} ({path:?})"));
        }
    }
    assert!(
        missing.is_empty(),
        "{} manifest entries have no fixture file:\n  - {}",
        missing.len(),
        missing.join("\n  - ")
    );
}

/// Quick smoke test: every .wrfm file under `valid/` and `invalid/` is
/// referenced in the manifest. Catches an orphan fixture (typo in the
/// section name, or a fixture added without manifest entry).
#[test]
fn no_orphan_fixtures() {
    let fixtures_dir = PathBuf::from(FIXTURES_DIR);
    let manifest_text = std::fs::read_to_string(fixtures_dir.join("manifest")).unwrap();
    let manifest: std::collections::HashSet<String> = parse_manifest(&manifest_text)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let mut orphans: Vec<String> = Vec::new();
    for sub in ["valid", "invalid"] {
        let dir = fixtures_dir.join(sub);
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
        for entry in entries {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".wrfm") {
                continue;
            }
            let rel = format!("{sub}/{name}");
            if !manifest.contains(&rel) {
                orphans.push(rel);
            }
        }
    }
    assert!(
        orphans.is_empty(),
        "fixtures on disk not referenced in the manifest: {orphans:?}"
    );
}
