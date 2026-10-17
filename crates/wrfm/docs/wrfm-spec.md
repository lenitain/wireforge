# `.wrfm` Format Specification — Version 2

This document is the **authoritative normative specification** of the `.wrfm`
plain-text 3D wireframe format. It is independent of any particular
implementation; the wireforge viewer, the `wrfm` CLI and the `wrfm` Rust
parser library are all bindings to this contract.

Conventions: **MUST** / **MUST NOT** / **SHALL** / **SHALL NOT** are mandatory;
**SHOULD** / **SHOULD NOT** are strong recommendations; **MAY** is truly
optional. The key words are capitalised for clarity.

The current format is **v2**. The version marker is `wrfm 2` on the first
line. Anything else (no magic line, an unknown magic version, a malformed
magic line) is rejected by the parser as `MissingMagic`, `MalformedMagic`,
or `UnsupportedVersion` (§8).

---

## 1. Scope

A `.wrfm` file represents one 3D wireframe model: an ordered list of vertices
in ℝ³ and an unordered set of edges between them, optionally partitioned into
named sections. The format is plain text, line-oriented, UTF-8 encoded.

The format does **not** prescribe:

* a coordinate frame, a unit, or an orientation (a model is unitless until a
  viewer maps it to a camera)
* a semantic interpretation of groups (the wireforge viewer uses them as
  named parts, but readers MUST NOT rely on that)
* a minimum or maximum vertex / edge count (any value that fits the encoding
  is acceptable)
* a guarantee that the geometry is well-formed (e.g. closed, manifold,
  acyclic). Geometry-level checks are a separate concern of the reader
  (see `wrfm check`); the format only guarantees that the file is
  syntactically well-formed.

## 2. Conformance

### 2.1 Conformance roles

* **Writer** — produces a `.wrfm` byte stream.
* **Reader** — consumes a `.wrfm` byte stream and reports either a parsed
  model or a structured error.

A **conforming writer** SHALL emit only files that conform to §3–§9 of this
specification. In particular, a conforming writer SHALL produce the
**canonical form** of §9 (specific whitespace, indentation, blank lines).

A **conforming reader** SHALL:

1. Accept any byte stream that is a valid encoding of the **strict syntax**
   of §5.
2. Accept any byte stream that conforms to the **canonical form** of §9.
3. Apply the **lenient extensions** of §8 (CRLF, BOM, trailing tokens,
   unknown directives, comments) without error.
4. Reject every other byte stream with a structured error from §7.

### 2.2 Two forms: strict and lenient

This specification distinguishes:

* **Strict syntax** — the minimal form a reader MUST accept. Defined by the
  grammar in §5.
* **Canonical form** — the unique byte sequence every writer SHOULD emit.
  Defined in §9.
* **Lenient extensions** — additional forms a reader MUST tolerate beyond
  strict syntax. Defined in §8.

Anything not in strict syntax AND not in the lenient extensions is a
format error.

## 3. File layout (overview)

A v2 file is a sequence of lines. The first three lines have fixed roles;
the rest may interleave freely.

| Line | Role | Required |
| :--- | :--- | :--- |
| 1 | magic | yes — and only the magic. **No comment, blank line, or whitespace may appear before it** (except the BOM, §4.1). Anything else on line 1 is `MissingMagic` (§6.1). |
| 2 | counts header | yes (must precede the first `v` / `e` / `group`) |
| 3+ | vertices, edges, group openers, comments, blank lines | interleaved freely |

After the counts header, content may appear in any order: any number of
vertices, any number of edges, any number of `group` openers, blank lines,
and `#` comments may be interleaved. The file MAY end with a trailing
newline or not.

The file MUST NOT contain anything after the magic line that is not one of:
a blank line, a comment, a vertex, a group opener, an edge, or an unknown
directive (which is silently skipped, see §8.5).

## 4. Lexical tokens

The reader tokenises every line by splitting on runs of whitespace
(§4.2). A token is therefore **a maximal run of non-whitespace bytes**.
Comments are recognised at the line level, before tokenisation (§4.3).

### 4.1 Encoding and line terminators

* **Encoding**: the byte stream MUST be valid UTF-8. The reader SHALL
  reject invalid byte sequences: `from_file` surfaces them as a
  `LoadError::Io` (because it reads via `std::fs::read_to_string`); other
  readers SHALL report an implementation-defined malformed-input error
  keyed on byte offset. The format does not otherwise pin down encoding-
  error reporting because UTF-8 is the contract of the `&str` Rust API
  that `from_str` consumes.
* **BOM**: a leading UTF-8 byte order mark (`EF BB BF`, i.e. U+FEFF) MAY
  appear before the magic line. A conforming reader MUST strip exactly one
  such BOM before parsing begins; it MUST NOT occur anywhere else.
* **Line terminators**: each line is terminated by either
  * LF (`0x0A`) or
  * CRLF (`0x0D 0x0A`).

  Both forms are accepted everywhere. A trailing line terminator at the
  end of the file is optional (the last line MAY be unterminated). A line
  terminator SHALL NOT appear inside a token (a token is a maximal run of
  non-whitespace).

### 4.2 Whitespace

Whitespace characters are:

| Byte | Name |
| :--- | :--- |
| `0x20` | space |
| `0x09` | horizontal tab |
| `0x0D` | carriage return (only valid as part of CRLF line terminator) |
| `0x0A` | line feed (only valid as part of LF or CRLF line terminator) |

Inside a content line, whitespace separates tokens but has no other
meaning. Conforming writers SHOULD use single spaces between tokens; multiple
spaces are tolerated by readers (§8.4). Leading and trailing whitespace on
a content line is ignored.

### 4.3 Comments

A line whose first non-whitespace byte is `0x23` (`#`) is a **comment** and
SHALL be ignored in its entirety. A line that is empty after trimming
whitespace is a **blank line** and SHALL also be ignored.

A comment or blank line MAY appear anywhere **after** the magic line (§8.1).
A comment or blank line at the very start of the file (before the magic)
is **NOT** a comment: line 1 is reserved for the magic (optionally preceded
by exactly one BOM, §4.1) and any other first line — including a `#`
comment, a blank line, or a `v` / `e` / `vertices` line — is reported as
`ParseError::MissingMagic` (§6.1).

A trailing `# ...` comment on the same line as the counts header is
permitted (§5.1). A trailing `# ...` comment on the same line as a `v` or
`e` line is permitted because parsers stop reading after the required
tokens (§8.6); the `#` is not specially recognised there, but the
observable behaviour is identical to a trailing comment.

### 4.4 Numbers

Two numeric token types are recognised:

#### 4.4.1 Floating-point (`float`)

A `float` token is interpreted as an IEEE-754 binary64 (Rust `f64`) value
using the same lexical grammar as `f64::from_str`. Specifically, a token
matching any of the following forms is accepted:

* `[+-]? DIGIT+ ( . DIGIT* )? ( [eE] [+-]? DIGIT+ )?`
* `[+-]? . DIGIT+ ( [eE] [+-]? DIGIT+ )?`
* `inf` / `infinity` (case-insensitive) — represents `+∞`
* `-inf` / `-infinity` (case-insensitive) — represents `-∞`
* `nan` (case-insensitive) — represents a NaN. The sign and payload of the
  resulting NaN are unspecified.

Any other form is a non-number and SHALL be rejected as
`ParseError::InvalidVertex { detail: NotANumber { ... } }` (§7).

A `float` token is the only accepted form for vertex coordinates. Tokens
that lexically parse to a non-finite value (`inf`, `-inf`, `nan` and the
long forms `infinity`, `-infinity`, case-insensitive) are accepted as
floats by the lexical grammar but are **rejected when used as a vertex
coordinate**, with `ParseError::InvalidVertex { detail: NotFinite { token,
axis } }` (§6.3). Wireframe geometry cannot carry non-finite coordinates
without poisoning every downstream computation (bbox, distance,
projection), so the format mandates finite IEEE-754 floats on every
`v` line; readers SHALL reject `inf`/`nan` up front rather than produce
a model that cannot be rendered.

#### 4.4.2 Unsigned integer (`uint`)

A `uint` token is interpreted as a non-negative integer using the same
lexical grammar as `usize::from_str`. Specifically:

* `DIGIT+` — a non-empty run of ASCII digits, optionally preceded by `+`.

A leading `-` is rejected as `ParseError::InvalidEdge::NegativeIndex`
(§7) and as `ParseError::MalformedHeader` (§7) when it appears in the
counts header. Decimal point, exponent, base prefix and underscores are
NOT accepted and SHALL be rejected as
`ParseError::InvalidEdge::NotAnIndex` or `ParseError::MalformedHeader`.

A `uint` token is the only accepted form for declared counts (§5.1) and
edge indices (§5.3).

### 4.5 Group name

A group name is a single token. Its content has no further restriction:
the name is the verbatim first token after `group`, the reader does not
interpret it, and there are no reserved words at the group-context
level. Whitespace-resembling bytes that occur inside multi-byte UTF-8
sequences are part of the name (the tokeniser splits only on whitespace
runs, see §4.2). A name MAY equal a header-context keyword such as
`vertices` or `edges`; the parser distinguishes the directive
**position**, not the spelling.

## 5. Grammar (EBNF)

The following grammar in Extended Backus–Naur Form describes the
**strict syntax** of v2. Productions not listed as part of strict syntax
are governed by §8.

```ebnf
file          = [ bom ] magic eol header eol { content-line eol } [ eol ]

(* --- line-level --- *)
magic         = "wrfm" SP+ version
version       = uint-non-zero          ; only "2" is accepted; others → UnsupportedVersion
header        = "vertices" SP+ uint SP+ "edges" SP+ uint [ SP+ "#" *comment-text ]
content-line  = blank / comment / vertex / edge / group / unknown
blank         = *SP
comment       = "#" *comment-text non-eol
vertex        = "v"   SP+ float SP+ float SP+ float
edge          = "e"   SP+ uint SP+ uint
group         = "group" SP+ name
unknown       = token                  ; any other directive; see §8.5
name          = token                  ; a single non-whitespace token

(* --- lexical atoms --- *)
float         = float-decimal / "inf" / "infinity" / "nan"        ; case-insensitive on inf/infinity/nan
                ; ±inf and nan parse as IEEE-754 floats but are REJECTED on vertex
                ; lines — see §6.3 (NotFinite).
float-decimal = [ sign ] mantissa [ exponent ]
mantissa      = digits [ "." *digits ] / "." digits
exponent      = ( "e" / "E" ) [ sign ] digits
sign          = "+" / "-"
uint          = [ "+" ] digits         ; no sign means non-negative; "-" is rejected (§4.4.2)
uint-non-zero = uint                   ; 0 is accepted but rejected by version check (§6.1)
digits        = 1*DIGIT
token         = 1*non-space-ws         ; maximal run of bytes that are not SP/LF/CR
bom           = %xEF %xBB %xBF        ; optional, at most one, at file start only

(* --- whitespace and line terminators --- *)
eol           = LF / CRLF              ; a line terminator
LF            = %x0A
CRLF          = %x0D %x0A
SP            = %x20 / %x09            ; space or tab (the only in-line separators)
non-space-ws  = %x0A / %x0D            ; whitespace bytes that are NOT in-line separators
                                          ; these never appear in a token by §4.2

(* --- comments inside content --- *)
comment-text  = *non-eol-non-hash-start
```

Notes on the grammar:

* Productions are case-sensitive: `wrfm`, `vertices`, `edges`, `v`, `e`,
  `group` are all lowercase ASCII.
* `unknown` is listed as a production solely so the grammar is complete;
  a conforming reader SHALL silently ignore it after the counts header
  (§8.5) and SHALL reject it before the counts header (§7,
  `MissingHeader`).
* The grammar above is **strict syntax**; §8 lists the lenient extensions.

## 6. Semantics

### 6.1 Magic line

The first non-BOM line MUST be `wrfm` followed by exactly one token: the
version number. The reader SHALL accept the following outcomes:

| Input | Reader outcome |
| :--- | :--- |
| `wrfm 2` | parsed, version = 2 |
| `wrfm <N>` where `<N>` parses as an unsigned integer but `N != 2` | `ParseError::UnsupportedVersion` |
| `wrfm` with no second token | `ParseError::MalformedMagic` |
| `wrfm <X>` where `<X>` does not parse as an unsigned integer (e.g. `2.0`, `02a`, `-1`) | `ParseError::MalformedMagic` |
| Any other first line (including empty, comment, `vertices`, `v`, ...) | `ParseError::MissingMagic` |

The version number is part of the magic line and cannot be repeated.

### 6.2 Counts header

The first line after the magic MUST be a counts header of exactly the
form

```text
vertices <N>   edges <M>
```

with `<N>` and `<M>` non-negative integers (§4.5.2). A trailing `# ...`
comment on the same line is permitted (the first `#` token truncates the
header for validation).

The header declares the **expected** number of vertex lines (`N`) and edge
lines (`M`) in the file as a whole. Reader behaviour:

* On parsing a `v` line: if the vertex count would exceed `<N>`, the
  reader SHALL report `ParseError::CountMismatch { which: Vertices }`.
* On parsing an `e` line: if the edge count would exceed `<M>`, the reader
  SHALL report `ParseError::CountMismatch { which: Edges }`.
* After all content lines: if the actual count does not equal `<N>` (resp.
  `<M>`), the reader SHALL report `ParseError::CountMismatch`.

The header MUST appear exactly once and MUST precede the first `v`, `e` or
`group` line. Any `v`, `e`, `unknown` or `group` line that appears before
the header is `ParseError::MissingHeader`. Blank lines and `#` comments
before the header are skipped (see §8.1).

### 6.3 Vertices

A vertex line defines one vertex in the global vertex list. The reader
appends it to `vertices` in file order. Vertices are **0-indexed** in
file order: the first `v` line is vertex 0, the second is vertex 1, and so
on. The global vertex list is the only indexing space used by edges.

The three coordinates are read as `float` (§4.4.1). Reader behaviour on
malformed input:

* `v` alone, or `v <x>` only, or `v <x> <y>` only:
  `ParseError::InvalidVertex { detail: MissingCoordinate { axis } }`
  where `axis` is `'x'`, `'y'` or `'z'` respectively.
* `v <x> ...` where the next token after `v` does not parse as a `float`,
  or the same for `y` or `z`:
  `ParseError::InvalidVertex { detail: NotANumber { token, axis } }`.
* `v <x> ...` where the token does parse as a finite `float` but represents
  `+∞`, `−∞`, or `NaN` (case-insensitive, including the long forms
  `infinity` and `−infinity`):
  `ParseError::InvalidVertex { detail: NotFinite { token, axis } }`.
  This applies independently to x, y, and z: a single bad coordinate in a
  three-token `v` line fails the whole vertex.

`v` lines that contain more than the three coordinate tokens are accepted;
the first three tokens are read and the rest are silently ignored (§8.6).
A `#` token at any position after the three coordinates is *observably* a
trailing comment because the parser stops reading after three coordinates
(§8.6) — the `#` itself is not specially recognised on a `v` line, but the
resulting behaviour matches.

### 6.4 Edges

An edge line defines one unordered edge between two existing vertices in
the global vertex list. The reader appends it to `edges` in file order.

The two indices are read as `uint` (§4.4.2). Reader behaviour:

* Missing first or second index:
  `ParseError::InvalidEdge { detail: MissingIndex { which } }`.
* First or second index does not parse as a `uint`:
  `ParseError::InvalidEdge { detail: NotAnIndex { token, which } }`.
* First or second index begins with `-`:
  `ParseError::InvalidEdge { detail: NegativeIndex { index, which } }`.
  (Note: `-0` is rejected. The parser special-cases the leading sign.)
* After all content lines, any edge index `>= vertices.len()`:
  `ParseError::InvalidEdge { detail: OutOfRange { index, vertex_count, which } }`.

Edge indices are validated against the **final** vertex count, not the
count at the time the edge is read. Forward references to vertices
defined later in the file are accepted. The reader makes no assumption
about ordering; vertices may be defined after their first appearance in
an edge.

**Self-loops (`e i i`) and duplicate edges (`e a b` repeated)** are
syntactically valid: the parser accepts them and records them in `edges`
exactly as written. Whether a model is geometrically non-trivial (no
self-loops, no repeated edges) is a separate concern handled by
geometric readers such as `wrfm check`; the format only guarantees that
every parsed index points to an existing vertex and that the file is
syntactically well-formed.

`e` lines that contain more than the two index tokens are accepted; the
first two tokens are read and the rest are silently ignored (§8.6). A `#`
token at any position after the two indices is *observably* a trailing
comment because the parser stops reading after two indices (§8.6); the
`#` itself is not specially recognised on an `e` line, but the resulting
behaviour matches.

### 6.5 Groups

A `group` line opens a named section that contains subsequent vertex
lines until the next `group` line **or end of file**. The reader holds an
open group through the closing `group` line (or the end of parsing) and
emits its `Group` record during parse finalisation, so a trailing group
that runs to EOF is emitted just like one closed by a successor line.
Vertices before the first `group` line are the **implicit unnamed
section** and are not recorded as a `Group` value.

The reader represents groups as `Group { name: String, vertex_start:
usize, vertex_end: usize }`, where `[vertex_start, vertex_end)` is a
**half-open** range over the global vertex list. `vertex_start` is the
length of `vertices` at the time the group was opened; `vertex_end` is
the length when the group was closed (next `group` line or end of file).

Group rules:

* A `group` line MUST be followed by exactly one token (the name). A
  `group` line with no name produces `ParseError::MissingGroupName`.
* The name is the verbatim first token; the reader does not interpret
  it. Whitespace inside the name is impossible (it would split the
  token); characters inside multi-byte UTF-8 sequences are part of the
  name.
* An **empty group** (a `group` line followed by no `v` lines before the
  next `group` or end of file) produces no `Group` record. Successive
  empty `group` lines are each ignored, so an empty group is not even
  "opened" — it never sees its first `v`.
* Nested groups are not supported: a `group` line inside an open section
  **closes** the previous section (with the same finalisation rule as
  EOF) and opens a new one. There is no nesting depth.
* Group names are not required to be distinct. Two groups with the same
  name produce two `Group` records with different ranges.
* Edges are **global** and are not associated with any group. An edge
  may reference vertices from any group, including groups defined later
  in the file (forward reference).

**Writer-side invariant.** Conforming writers SHALL clamp
`vertex_start` and `vertex_end` to the model's vertex count (`min(n)`
per range end) and SHOULD emit no `v` lines for a group whose range is
empty after clamping. The reference writer (`wrfm` crate
`WrfmModel::write_to`) clamps in this way; a writer that violates the
clamp produces a model that parses back to a smaller group range than it
wrote.

## 7. Errors (defined `ParseError` variants)

A conforming reader SHALL report exactly one of the following variants on
malformed input. Each variant carries a 1-based `line` and 1-based
`column` of the offending position, plus the raw offending line text. The
column is **character-based**, not byte-based (UTF-8 multibyte characters
count as one column).

| Variant | Triggers |
| :--- | :--- |
| `MissingMagic { line, column, line_text }` | Line 1 (after BOM stripping) does not begin with `wrfm`. Includes the empty file. |
| `MalformedMagic { line, column, line_text, token }` | Line 1 is `wrfm` with no version token, or the version token does not parse as a `uint`. `token` is the offending version token (empty when missing). |
| `UnsupportedVersion { line, column, line_text, version, token }` | Line 1 declares a version that parses as a `uint` but is not `2`. `version` is the parsed integer; `token` is the literal text. |
| `MissingHeader { line, column, line_text }` | The first non-magic content line is not the counts header. A `#` comment or blank line before the header is tolerated (§8.1). |
| `MalformedHeader { line, column, line_text, token }` | The counts header line exists but does not match `vertices <N>   edges <M>`. `token` is the offending token (empty when the header is merely truncated). |
| `CountMismatch { line, column, line_text, which, declared, actual }` | The actual count of vertices (`which: Vertices`) or edges (`which: Edges`) disagrees with the declared count. `line` and `column` point to the header for M, or to the over-counting `v`/`e` line for N. |
| `InvalidVertex { line, column, line_text, detail }` | A malformed `v` line. `detail` is one of `MissingCoordinate { axis }`, `NotANumber { token, axis }`, or `NotFinite { token, axis }` (a coordinate that parses as a `float` but is `±inf` or `nan`; see §6.3). |
| `InvalidEdge { line, column, line_text, detail }` | A malformed `e` line. `detail` is one of `MissingIndex { which }`, `NotAnIndex { token, which }`, `NegativeIndex { index, which }`, or `OutOfRange { index, vertex_count, which }`. |
| `MissingGroupName { line, column, line_text }` | A `group` line with no name token. |

A conforming reader SHALL NOT report any error not in this table.

The reader reports the **first** error it encounters and stops; later
errors are not reported. The reader MAY distinguish position information
by character column, byte offset, or both, but SHALL provide at least
character-based column and 1-based line.

## 8. Lenient extensions

This section lists every place a conforming reader MUST accept input that
is not strictly required by the grammar in §5. A conforming writer
SHOULD NOT produce any of these forms; canonical form (§9) is what
writers emit.

### 8.1 Blank lines and comments before the counts header

The reader MUST skip blank lines and `#` comments anywhere before the
counts header. The header is the **first non-blank, non-comment line**
after the magic.

### 8.2 Leading UTF-8 BOM

A single UTF-8 BOM (`EF BB BF`, U+FEFF) at the very start of the file
SHALL be stripped before parsing. The BOM is the only byte sequence
allowed before the magic line.

### 8.3 CRLF line terminators

Lines terminated by `0x0D 0x0A` (CRLF) MUST be accepted in addition to
the canonical LF (`0x0A`). A `0x0D` byte that is not followed by `0x0A`
inside a line is undefined behaviour; canonical form never produces it.

### 8.4 Multiple whitespace bytes between tokens

The tokeniser splits on **runs** of whitespace (§4.2). Any combination
of spaces and tabs between tokens is accepted. Canonical form uses
single spaces (§9).

### 8.5 Unknown directive lines after the header

After the counts header has been parsed, any line whose first token is
not `v`, `e`, `group`, `#` or a blank line is treated as an **unknown
directive** and silently ignored. The line is NOT reported as an error.

The reader MUST NOT skip unknown directives before the counts header:
the first non-blank, non-comment line after the magic MUST be the
header (§6.2).

### 8.6 Extra tokens on `v`, `e`, `group` and the counts header

* `v <x> <y> <z> <extra...>`: only the first three tokens after `v` are
  read; the rest (including any `#`) are silently ignored. The
  observable behaviour matches a trailing comment because the parser
  stops reading after the third coordinate, but `#` itself is NOT a
  syntactic marker on a `v` line — it is one more ignored token.
* `e <i> <j> <extra...>`: only the first two tokens after `e` are read;
  the rest (including any `#`) are silently ignored. Same caveat: the
  `#` is not a syntactic marker on an `e` line, it is just an extra
  token that nobody reads.
* `group <name> <extra...>`: only the first token after `group` is read
  as the name; the rest are silently ignored.
* **Counts header** is the one exception: extra tokens after the four
  required tokens are an error (`MalformedHeader`), unless a `#` token
  truncates them as a trailing comment. On the header line only, the
  parser actively scans for `#` to find where the syntactic header ends.

### 8.7 Leading `+` on `uint` tokens

A `uint` token MAY be preceded by `+`. The reader treats `+N` and `N` as
the same value. Leading `-` is **not** an extension: it is the
`NegativeIndex` / `MalformedHeader` error (§7).

### 8.8 Forward edge references

An edge line MAY reference a vertex that is defined later in the file.
The reader validates indices against the final vertex count, not the
current count. There is no restriction on the ordering of vertices and
edges.

### 8.9 Trailing line terminator at end of file

The last line of the file MAY or MAY NOT be terminated. Both forms are
accepted.

## 9. Canonical output (writer requirements)

A conforming writer SHALL produce the following canonical byte sequence.
Two files that differ only in non-canonical whitespace / indentation are
NOT considered distinct by a conforming reader (§2.1) but are NOT
canonical.

The canonical form is what the reference Rust implementation emits and
what every writer SHOULD emit to maximise diff stability and minimise
byte size.

### 9.1 Top-level structure

```text
wrfm <V>                          LF
vertices <N>   edges <M>          LF
                                  LF  ; one blank line
{ vertex-or-group-block }         LF
                                  LF  ; one blank line
{ edge }                          LF
```

`<V>` is the version number (always `2`); `<N>` is the total vertex
count; `<M>` is the total edge count.

**Canonical whitespace on the header line:** exactly one ASCII space
(`0x20`) between `vertices` and `<N>`, and between `edges` and `<M>`;
exactly three ASCII spaces between `<N>` and `edges`. Any other spacing
(zero, two, four, mixed tabs, etc.) is accepted by readers (§8.4) but is
NOT canonical and SHOULD NOT be emitted by writers. The three-space gap
between `<N>` and `edges` is intentional — it makes the two counts
visually balanced with the three-token header — and is the only place in
canonical form where two or more consecutive spaces appear.

### 9.2 Vertex block

If the model has no groups, the writer emits one `v <x> <y> <z>` line per
vertex, in global order, unindented.

If the model has groups, the writer emits vertices in file order with
the following structure:

```text
{ unindented v line }             LF  ; for vertices before the first group
group <name>                      LF  ; the group opener
  { indented v line }             LF  ; for vertices in [vertex_start, vertex_end)
group <name>                      LF
  { indented v line }             LF
{ unindented v line }             LF  ; for vertices after the last group
```

Indentation is exactly two spaces (`0x20 0x20`). Lines inside groups are
indented; lines outside are not.

### 9.3 Edge block

Each edge is one line: `e <a> <b>`. Edges are global (§6.5) and are
emitted after the vertex block, regardless of any group the vertices
belong to.

### 9.4 Number formatting

* **Coordinates** are written in the shortest decimal representation
  that round-trips to the same `f64` value. `1.0/3.0` writes as
  `0.3333333333333333`; integers write without a decimal point or
  exponent.
* **Counts and indices** are written as decimal integers. Leading zeros
  SHALL NOT be emitted. `0` is written as `0`; `100` as `100`.

### 9.5 Examples

The smallest valid canonical file (the empty model):

```text
wrfm 2
vertices 0   edges 0
```

A tetrahedron (§A.1).

```text
wrfm 2
vertices 4   edges 6

# Name: Regular Tetrahedron
v 1.0 1.0 1.0
v 1.0 -1.0 -1.0
v -1.0 1.0 -1.0
v -1.0 -1.0 1.0

e 0 1
e 0 2
e 0 3
e 1 2
e 2 3
e 3 1
```

Note: the trailing comment line `# Name: ...` is preserved by readers
but its presence is not normative — it is a convention some sample files
carry.

## 10. Reserved for future versions

A new major version of `.wrfm` SHALL be introduced by an explicit
amendment to this specification and SHALL be signalled by a new magic
version token. Writers and readers SHALL NOT change the format's
behaviour in ways that would break a v2 reader without first moving the
magic version.

The following syntactic slots are reserved by this version and SHALL
NOT be used by v2 conforming writers. They exist so that future versions
have unambiguous expansions without invalidating v2 readers.

* **A version other than `2` on the magic line** (§6.1). Writers SHALL
  emit `wrfm 2`; readers SHALL reject other values with
  `ParseError::UnsupportedVersion`.
* **A second magic line anywhere in the file.** Writers SHALL emit the
  magic exactly once, on line 1. If a future file does contain a string
  that lexically looks like another magic line, v2 readers treat it as
  an unknown directive (the `wrfm` token would not parse as `v`, `e` or
  `group`) and skip it per §8.5 — this is forward-compatible and is NOT
  required to be an error.
* **A second counts header anywhere in the file.** Writers SHALL emit
  the header exactly once, immediately after the magic. v2 readers
  treat any later `vertices ...` line as an unknown directive for the
  same reason as above, and skip it per §8.5.
* **Non-finite `v` coordinates** (§6.3). Writers SHALL emit finite
  IEEE-754 floats only. Readers SHALL reject `±inf` and `nan` with
  `ParseError::InvalidVertex { detail: NotFinite { ... } }`.

These reservations replace the older §8.5 contradiction: under the
current grammar a second magic or a second header WOULD be silently
skipped as an unknown directive — that is the deliberate forward-
compatibility behaviour, not a bug, and readers do NOT need to detect
these slots specially on `v2`.

## 11. Conventions (non-normative)

These conventions apply to projects in the wireforge ecosystem but are
not part of the `.wrfm` format itself. The format is orientation-, unit-
and frame-agnostic (§1); the conventions below describe how the
wireforge tools interpret a file once parsed.

* **Y-UP**. +Y is vertical height; X and Z form the ground plane. A
  model built with Z as height is seen lying on its side. Writers SHOULD
  emit Y-UP files; readers MAY use any orientation when rendering.
* **Front-on-`+Z`.** The object's semantic face (appliance door, TV
  screen, vehicle nose, ...) faces +Z, so the `front` view shows that
  face and the left–right extent lives on X.
* **Ground at `y = 0`** unless an upstream tool declares otherwise.
* **Point identity.** Vertices strictly closer than `1e-6` world units
  are the SAME point — this is a CLI / viewer threshold for
  deduplication (`wrfm check` reports it as `near_duplicate_vertices`;
  `wrfm edit --weld 1e-6` merges them) and is enforced by wireforge
  tooling, not by the format parser. Pass an explicit `--weld TOL` when
  the model's units are far from 1.
* **Repair-level operations are OUT of the format spec.** `wrfm edit
  --clean`, `--dedupe`, `--weld`, `wrfm transform --normalize`, etc. are
  CLI-level conveniences that emit valid v2 files; the format itself
  guarantees only that a syntactically well-formed file round-trips a
  model the same way.

---

## A. Worked examples

### A.1 Tetrahedron (canonical)

```text
wrfm 2
vertices 4   edges 6

v 1.0 1.0 1.0
v 1.0 -1.0 -1.0
v -1.0 1.0 -1.0
v -1.0 -1.0 1.0

e 0 1
e 0 2
e 0 3
e 1 2
e 2 3
e 3 1
```

### A.2 Grouped model

```text
wrfm 2
vertices 5   edges 4

v 0 0 0          ; implicit unnamed section (global vertex 0)
group body
  v 1 0 0        ; global vertex 1
  v 1 1 0        ; global vertex 2
group head
  v 0.5 2 0.25   ; global vertex 3
  v 0 1 2        ; global vertex 4

e 0 1            ; edges are global — vertex 0 is unnamed; vertex 1 is in `body`
e 1 2
e 2 0
e 0 4            ; vertex 4 is in `head` — index is GLOBAL
```

After parsing, the model reports `groups`:

```text
[
  Group { name: "body", vertex_start: 1, vertex_end: 3 },
  Group { name: "head", vertex_start: 3, vertex_end: 5 },
]
```

Note that the vertex `0 0 0` is part of no group: it is in the implicit
unnamed section.

### A.3 Lenient forms (accepted, non-canonical)

A BOM and CRLF terminators:

```text
[EF BB BF]wrfm 2[CR][LF]
vertices 2   edges 1[CR][LF]
[CR][LF]
v 0 0 0[CR][LF]
v 1 1 1[CR][LF]
[CR][LF]
e 0 1[CR][LF]
```

Extra tokens and an unknown directive:

```text
wrfm 2 # this is fine because comments are allowed
vertices 2   edges 1

v  0  0  0  extra tokens are ignored
v  1  1  1
unrecognized-directive arg1 arg2 # silently skipped

e 0 1 trailing
```

A trailing `#` comment on the counts header:

```text
wrfm 2
vertices 2   edges 1 # authored 2026-10-17 by lenitain
```

### A.4 Erroneous forms (rejected)

Missing magic:

```text
# this is a comment
v 0 0 0
```

→ `MissingMagic` at line 1. (Comments and blank lines are tolerated
**after** the magic, not before it; see §3, §4.3.)

Blank line / whitespace on line 1:

```text

wrfm 2
vertices 0   edges 0
```

→ `MissingMagic` at line 1. Line 1 must be the magic; nothing (except a
single BOM) may precede it.

Unsupported version:

```text
wrfm 3
vertices 0   edges 0
```

→ `UnsupportedVersion { version: 3 }` at line 1.

Negative edge index:

```text
wrfm 2
vertices 2   edges 1

v 0 0 0
v 1 1 1
e 0 -1
```

→ `InvalidEdge { detail: NegativeIndex { index: -1, which: 2 } }` at line 6.

Edge out of range:

```text
wrfm 2
vertices 2   edges 1

v 0 0 0
v 1 1 1
e 0 5
```

→ `InvalidEdge { detail: OutOfRange { index: 5, vertex_count: 2, which: 2 } }`
at line 6.

Count mismatch (declared 5 vertices, only 3 follow):

```text
wrfm 2
vertices 5   edges 0

v 0 0 0
v 1 1 1
v 2 2 2
```

→ `CountMismatch { which: Vertices, declared: 5, actual: 3 }` at line 2.

Vertex line with non-numeric coordinate:

```text
wrfm 2
vertices 1   edges 0

v 1.0 2.0 abc
```

→ `InvalidVertex { detail: NotANumber { token: "abc", axis: 'z' } }` at
line 4, column 11.

Vertex line with non-finite coordinate (±inf, NaN):

```text
wrfm 2
vertices 1   edges 0

v 1.0 2.0 inf
```

→ `InvalidVertex { detail: NotFinite { token: "inf", axis: 'z' } }` at
line 4. The coordinate lexes as an IEEE-754 `+∞` but is rejected because
wireframe geometry cannot carry non-finite values (§4.4.1, §6.3).

---

## B. Glossary

Only terms whose meaning is *not* obvious from immediate context are
listed here. Inline definitions in §1–§A take precedence over this
glossary.

| Term | Definition |
| :--- | :--- |
| **canonical form** | The unique byte sequence conforming writers emit (§9). Distinct from "valid": a file may be valid (`v2`) but non-canonical, in which case a reader still accepts it. |
| **lenient extension** | A syntactic form the reader accepts but the writer SHOULD NOT emit (§8). |
| **strict syntax** | The grammar of §5; the minimal form a reader MUST accept. |
| **forward reference** | An edge line that names a vertex defined by a later `v` line in the file (§8.8). The reader accepts this. |
| **round-trip** | Parsing a file with `WrfmModel::from_str` and serializing the result back to canonical text yields the same vertex list, edge list, and group ranges (coordinates may differ in form but must compare equal as `f64`). |

---

This specification is the canonical normative reference. The accompanying
authoritative implementations are the `wrfm` Rust parser library and the
`wrfm` CLI; the wireforge viewer is a non-normative consumer that reads
the format but does not define it. Where this document and the `wrfm`
CLI's `wrfm format` output disagree, this document is authoritative.
