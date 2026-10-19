# wrfm

A zero-dependency parser and serializer for the `.wrfm` 3D wireframe
format.

This crate provides a `WrfmModel` struct to load, manipulate, and save
models of vertices and edges. It is the parser used by `wireforge`, the TUI
viewer, and by the `wrfm` command line tool.

**The authoritative format specification** lives at
[`docs/wrfm-spec.md`](docs/wrfm-spec.md) inside this crate, and
is reproduced online at <https://github.com/lenitain/wireforge/blob/main/crates/wrfm/docs/wrfm-spec.md>.
This README covers the library API only; for the byte-level syntax,
EBNF grammar, error variants, and canonical output form, see the spec.

## Library overview

`WrfmModel` is a parsed v2 model: a display `name`, an ordered list of
vertices, an unordered set of edges between them (both global), the
parsed format version (`2`), and an optional list of named groups over
the global vertex list. The model is the contract between the file on
disk and every consumer (viewer, CLI, third-party tool).

```rust
use wrfm::{Group, WrfmModel};

let mut model = WrfmModel::new("cube");
model.vertices.push((-1.0, -1.0, -1.0));
model.vertices.push(( 1.0, -1.0, -1.0));
model.edges.push((0, 1));
model.groups.push(Group {
    name: "base".to_string(),
    vertex_start: 0,
    vertex_end: 2,
});
let text = model.to_string();
assert!(text.starts_with("wrfm 2\nvertices 2   edges 1\n"));
```

## Usage

Add `wrfm` to your `Cargo.toml`.

### `no_std`

The `std` feature is on by default. Turn it off to parse models without the
standard library (the crate then needs only `alloc`):

```toml
wrfm = { version = "0.6", default-features = false }
```

`from_str` and the model types work either way; `from_file`, `save_to_file`,
`write_canonical`, `LoadError` and the `Display` impl that serializes a model
are `std`-only.

### Loading a Model

You can parse a model directly from a file path. The parser skips blank lines
and `#` comments. `from_file` returns a `LoadError`, which is either an I/O
error or a structured `ParseError`:

```rust
use wrfm::{LoadError, WrfmModel};

fn main() -> Result<(), LoadError> {
    // Loads the model and extracts the filename stem as the model name
    let model = WrfmModel::from_file("path/to/model.wrfm")?;

    println!("Loaded model: {}", model.name);
    println!("Vertices: {}", model.vertices.len());
    println!("Edges: {}", model.edges.len());
    println!("Version: {}", model.version);

    Ok(())
}
```

### Handling Parse Errors

`from_str` returns a classified, positioned `ParseError`.
Render it with `Display` for a rustc-style report (line/column header, source
line, caret, expected-vs-found), or inspect the structured fields:

```rust
use wrfm::{ParseError, WrfmModel};

fn main() {
    let input = "wrfm 2\nvertices 1   edges 0\n\nv 1.0 2.0 abc\n";

    match WrfmModel::from_str("model", input) {
        Ok(model) => println!("{} vertices", model.vertices.len()),
        Err(err) => {
            // Positioned accessors + the classified variant.
            eprintln!("line {}: {}", err.line(), err);
            match err {
                ParseError::InvalidVertex { detail, .. } => eprintln!("bad vertex: {detail:?}"),
                ParseError::InvalidEdge { detail, .. } => eprintln!("bad edge: {detail:?}"),
                // Magic, counts header, group and count errors carry no finer
                // detail to unwrap; `Display` already reported them above.
                other => eprintln!("{other}"),
            }
        }
    }
}
```

Parsing skips unknown lines, but only after the counts header. Blank lines and
`#` comments are skipped anywhere after the magic line. Unknown directives are
skipped once the header has been seen; before that, the first line that is not
`vertices <N>   edges <M>` is an error. Out-of-range edge indices are never
tolerated (validated against the final vertex count, so forward references to
later-defined vertices are fine). The v2 magic, counts header and declared
counts are always enforced.

### Creating and Saving a Model

You can build a `WrfmModel` programmatically and serialize it back to disk.
`save_to_file` writes canonical v2, containing the magic line, a counts
header, optional `group` sections (in order) and then all edges.

```rust
use wrfm::{Group, WrfmModel};

fn main() -> std::io::Result<()> {
    let mut model = WrfmModel::new("triangle");

    // Add vertices
    model.vertices.push((0.0, 1.0, 0.0));
    model.vertices.push((-1.0, -1.0, 0.0));
    model.vertices.push((1.0, -1.0, 0.0));

    // Connect them
    model.edges.push((0, 1));
    model.edges.push((1, 2));
    model.edges.push((2, 0));

    // Name the section (optional)
    model.groups.push(Group {
        name: "main".to_string(),
        vertex_start: 0,
        vertex_end: 3,
    });

    // Save to disk
    model.save_to_file("triangle.wrfm")?;

    Ok(())
}
```

## Changelog

[CHANGELOG.md](CHANGELOG.md) — release history for this crate.
