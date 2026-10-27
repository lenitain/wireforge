# Wireforge SPEC

How to install Wireforge, use the viewer, and read and write the `.wrfm`
format. See [README.md](README.md) for the rest of the project.

## Installation

### AUR

```bash
yay -S wireforge
# or
paru -S wireforge
```

### crates.io

```bash
cargo install wireforge
```

### Build from source

```bash
git clone https://github.com/lenitain/wireforge.git
cd wireforge
cargo build --release
```

The `wrfm` CLI ships as a separate package; see
[crates/wrfm-cli/README.md](crates/wrfm-cli/README.md) for its installation
and full command reference.

## The viewer

Point `wireforge` at any `.wrfm` file. The format is detected from the file's
content, so the extension does not matter:

```bash
wireforge path/to/model.wrfm

# example
wireforge cube.wrfm
```

The file is read once, when the viewer starts; run `wireforge` again after
editing it.

Run `wireforge` with no file at all and it opens the viewer with an empty
model: the XYZ axes are drawn at the origin, so rotating, panning and zooming
still work, and the bar's pill reads `no file`.
A menu entry or launcher gets the same empty canvas if it runs the viewer in a
terminal and gives it nothing on stdin. If stdin is piped or redirected, a
bare `wireforge` reads the stream as a model instead
([Stream input](#stream-input)):

```bash
wireforge
```

### The screen

Two regions, top to bottom: the model, and one bar under it. The bar is laid
out like a powerline statusline: the model's name in an accent pill on the
left, two state lamps in a lighter block beside it (one dot per lamp: it lights
in the accent when on and goes deep when off), and the camera telemetry
right-aligned on the bar's own ground — yaw, pitch and roll in degrees, zoom as
a multiple of the start-up framing, and pan in model units. The telemetry is a
row of fixed columns, each label and its value in a column of its own, so a
value that grows (a minus sign, another digit) takes the room to its right and
the labels hold their places. Every block is closed by a wedge in its own
ground, so the pill's colour tapers into the lamps and the lamps into the bar,
and the two lamps of one block are divided by a thin seam:

```text
 utah-teapot ▶ ○ SPIN › ● AXES ▶           yaw 107°    pitch 26°     roll 0°      zoom 1.04×      pan 0.00, 0.00
```

The bar fills the row edge to edge, framing the model canvas above it. The
name pill yields at most half the row and marks a cut name with `…`. When the
terminal is too narrow, the lamps go first (the axes lamp before the spin
lamp), then the telemetry sheds from the right (pan first); a row too tight for
the pill's wedge keeps the name and drops the wedge. The bar carries no
key hints: the menu's Key Bindings always has the full list. The wedges and the
seam are powerline glyphs (U+E0B0 and U+E0B1, drawn as `▶` and `›` above) —
the separators lualine and powerline prompts draw — so the bar asks for a
powerline (Nerd Font) terminal font.

`Esc` opens the menu hub: **Key Bindings**, **About** and **Exit**. The help
panel is generated from the same binding table the keyboard uses, so the two
cannot drift apart; see [Menu](#menu).

### Stream input

`wireforge -` reads a model from stdin (or a FIFO such as `<( cat model.wrfm )`)
once, at start-up. Keyboard input still works via the controlling terminal.
A bare `wireforge` does the same whenever stdin is not a terminal, so a pipe
or a redirect is read as a model. The empty canvas appears only on a terminal,
where there is no stream to read.

```bash
cat model.wrfm | wireforge -
wireforge <( cat model.wrfm )
```

OBJ files are converted first with `wrfm convert` (part of
[`wrfm-cli`](crates/wrfm-cli/README.md)), then piped in like any other model:

```bash
wrfm convert mouse.obj | wireforge -
```

The result of a `wrfm-cli` transform can be viewed directly:

```bash
wrfm edit model.wrfm --extract-group cabinet | wrfm transform - --scale 2 | wireforge -
```

### Field of view

`--vfov N` sets the vertical field of view in **degrees** (default 30, valid
range (0, 180), decimals welcome). It is fixed for the session — it feeds the
focal length and the auto-fit camera distance at start-up. Degrees are the
surface unit: the command line and the camera readout speak degrees, the
projection math works in radians, and the boundary converts.

The vertical axis is the only one you set. The horizontal field of view
follows from the terminal's aspect ratio and is **never clamped**: unlike a
shooter that must fill the screen (and risks a horizontal FOV past 180° at
extreme aspect ratios), wireforge does not need to fill the screen — content
outside the frustum is simply not drawn.

```bash
wireforge model.wrfm --vfov 20   # narrow: zooms out (auto-fit frames farther)
wireforge model.wrfm --vfov 60   # wide: zooms in
```

### Key bindings

Bindings are grouped by what you are doing, not by which modifier you hold.
Chords are spelled with modifiers in `Ctrl+Shift+` order; the arrow keys
work wherever `hjkl` do.

The bindings are declared once, in one table in the source. The keyboard
resolver reads that table and the help panel is generated from it, and a test
keeps each help row's first key on the chord that actually fires — so a rebind
forces the help page to follow. The bar carries no hints: the menu's Key
Bindings is the only place the viewer lists the chords. The tables below are
that same list written out for reading.

Motion keys answer on the press. On a terminal that reports key-up (the Kitty
keyboard protocol), holding a motion key moves continuously until the Release;
the viewer probes for that support at start-up, so the first press of a session
behaves like every later one. After 200 ms the hold steps from the slow gear to
a fixed 2× fast gear; it is one step, not a ramp, and rotation, pan and zoom all
share it. Releasing resets the next press to the slow gear. A terminal that
cannot report key-up takes one finite step per press, and its auto-repeat steps
are scaled by the interval between events, so their steady speed does not follow
the terminal's repeat setting.

#### Rotate — the world's axes

Left and right read as you see them: the model faces out of the screen, so
`h` sweeps its nose toward your left.

| Key           | Action             |
| :------------ | :----------------- |
| `h` / `Left`  | yaw left           |
| `l` / `Right` | yaw right          |
| `k` / `Up`    | pitch up           |
| `j` / `Down`  | pitch down         |
| `d`           | roll anticlockwise |
| `f`           | roll clockwise     |

#### Rotate — the model's own axes

The same turns, but around the model's own current axes: `Ctrl+h` yaws it to
_its_ left. Unlike the world keys, the axis rides with the model, so the
control feels like a trackball rather than a turntable.

| Key                     | Action             |
| :---------------------- | :----------------- |
| `Ctrl+h` / `Ctrl+Left`  | yaw left           |
| `Ctrl+l` / `Ctrl+Right` | yaw right          |
| `Ctrl+k` / `Ctrl+Up`    | pitch up           |
| `Ctrl+j` / `Ctrl+Down`  | pitch down         |
| `Ctrl+d`                | roll anticlockwise |
| `Ctrl+f`                | roll clockwise     |

#### Move — the world's plane

| Key                       | Action        |
| :------------------------ | :------------ |
| `Shift+h` / `Shift+Left`  | pan left      |
| `Shift+l` / `Shift+Right` | pan right     |
| `Shift+k` / `Shift+Up`    | pan up        |
| `Shift+j` / `Shift+Down`  | pan down      |
| `+` / `=`                 | zoom in       |
| `-` / `_`                 | zoom out      |

#### View — where the model sits on screen

| Key     | Action          |
| :------ | :-------------- |
| `r`     | reset the view  |
| `Space` | toggle spinning |
| `Tab`   | toggle the axes |

#### Session

| Key            | Action           |
| :------------- | :--------------- |
| `Esc`          | open the menu    |
| `q` / `Ctrl+c` | quit             |

#### Menu

`Esc` opens the hub: **Key Bindings**, **About** and **Exit**. `j`/`k` or the
arrow keys move, `Enter` selects, and `Esc` closes. Panels opened from the menu
return there on `Esc`; `q` and `Ctrl+c` quit from anywhere, including the menu
and its panels.

The help panel lists every binding, grouped the same way, and is generated from
the binding table rather than maintained by hand.

## The `.wrfm` Format

The `.wrfm` format (v2) is a plain text format for 3D vertices and the edges
that connect them.

- The first line is the magic and version: `wrfm 2`.
- The counts header comes next, after any blank lines and `#` comments:
  `vertices <N>   edges <M>` (the declared counts must match the lines that
  follow).
- `v <x> <y> <z>` defines a vertex in 3D space.
- `e <index1> <index2>` defines an edge connecting two vertices (0-indexed
  based on the order they appear).
- Lines starting with `#` are comments; `group <name>` opens a named
  section (optional).

**Example: `tetrahedron.wrfm`**

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

The authoritative prose specification lives at
[crates/wrfm/docs/wrfm-spec.md](crates/wrfm/docs/wrfm-spec.md). It covers BOM
and whitespace rules, the `f64` round-trip guarantee, the parser's error
behaviour, and the canonical form.
