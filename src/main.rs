use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableFocusChange, EnableFocusChange, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Padding, Paragraph, Widget},
};

use std::{
    collections::HashMap,
    error::Error,
    io::{self, IsTerminal, Read},
    path::{Path, PathBuf},
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};
use wrfm::WrfmModel;
use wrfm_raster::Model;
use wrfm_raster::geometry::{DEFAULT_VFOV, MAX_VFOV};

mod render;
mod timer;
mod view;
use render::Ink;
use timer::{TimerId, TimerScheduler};
use view::ViewState;

// Speed constants are dyadic fractions (exact in binary): rotation and
// auto-spin rates are pure radians per second (parameter-free by design).
// Translation speed is set per session from the initial camera distance
// (`App::speed = dist_initial / 5.0`), so this unit owns only `ROT_RATE`
// / `SPIN_RATE` — every translation step is just `speed * dt`.

/// Smooth continuous rotation rate (radians per second) while a key is held.
const ROT_RATE: f64 = 169.0 / 128.0;

/// Auto-spin yaw rate (radians per second): 169/256 = 0.66015625, a dyadic (exact-in-binary) constant.
const SPIN_RATE: f64 = 169.0 / 256.0;
/// How often the idle loop re-checks the terminal size (resize fallback).
const RESIZE_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// A tap's rotation: 3 degrees, enough to feel like an answer without making
/// fine alignment a chore. Terminals that cannot report key-up use this for
/// every press and auto-repeat, so a tap is always finite; terminals with
/// key-up also get continuous motion on top while the key is down.
const TAP_ROT: f64 = std::f64::consts::PI / 60.0;
/// A tap's pan, as a fraction of the session's translation speed.
const TAP_PAN: f64 = 1.0 / 20.0;
/// A tap's dolly, as a fraction of the session's translation speed. Zoom
/// wants to feel more immediate than a nudge of the model, hence the larger
/// fraction.
const TAP_DOLLY: f64 = 1.0 / 8.0;

/// Command-line arguments the viewer accepts: one optional positional `FILE`
/// and `--vfov`. Parsed by hand in [`Args::parse`]; the whole surface is two
/// arguments, so a parser crate would weigh more than the code that reads
/// them.
struct Args {
    /// `.wrfm` file to open, or `-` to read from stdin (the default when stdin is
    /// not a terminal). Omit it on a terminal to open an empty canvas.
    file: Option<PathBuf>,
    /// Vertical field of view in RADIANS (default 0.5; must be > 0 and <= 3).
    /// The horizontal field follows from the terminal's aspect ratio and is
    /// never clamped — content outside the frustum is simply not drawn.
    vfov: f64,
}

/// The `--help` text, kept close to the conventional layout so a user who
/// knows clap finds the same facts here.
const USAGE: &str = concat!(
    "TUI viewer for .wrfm 3D wireframe models\n",
    "\n",
    "Usage: wireforge [OPTIONS] [FILE]\n",
    "\n",
    "Arguments:\n",
    "  [FILE]  `.wrfm` file to open, or `-` to read from stdin (the default when stdin is\n",
    "          not a terminal). Omit it on a terminal to open an empty canvas\n",
    "\n",
    "Options:\n",
    "      --vfov <VFOV>  Vertical field of view in RADIANS (default 0.5; must be > 0 and <= 3).\n",
    "                     The horizontal field follows from the terminal's aspect ratio and is\n",
    "                     never clamped — content outside the frustum is simply not drawn\n",
    "  -h, --help         Print help\n",
    "  -V, --version      Print version\n",
);

/// Print a command-line mistake to stderr and exit with the conventional usage
/// code (2), as a parser crate would.
fn fail_usage(message: &str) -> ! {
    eprintln!("error: {message}\n\nFor more information, try '--help'.");
    std::process::exit(2);
}

/// Parse a `--vfov` value into a finite `f64`. Range validation against
/// `(0, MAX_VFOV]` happens in [`main`], before any terminal setup.
fn parse_vfov(raw: &str) -> f64 {
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() => v,
        _ => fail_usage(&format!(
            "invalid value '{raw}' for '--vfov <VFOV>': expected a number in radians"
        )),
    }
}

impl Args {
    /// Read the command line from the process arguments.
    ///
    /// `--help`/`-h` and `--version`/`-V` print to stdout and exit 0; anything
    /// unrecognized is a usage error. `--` ends option parsing so a file whose
    /// name starts with `-` is still reachable.
    fn parse() -> Args {
        let mut args = Args {
            file: None,
            vfov: DEFAULT_VFOV,
        };
        let mut argv = std::env::args_os().skip(1);
        while let Some(arg) = argv.next() {
            let text = arg.to_string_lossy().into_owned();
            match text.as_str() {
                "-h" | "--help" => {
                    print!("{USAGE}");
                    std::process::exit(0);
                }
                "-V" | "--version" => {
                    println!("wireforge {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                }
                // Everything after `--` is positional, so a filename that
                // begins with a dash is never read as an option.
                "--" => {
                    for rest in argv.by_ref() {
                        if args.file.is_none() {
                            args.file = Some(PathBuf::from(rest));
                        }
                    }
                }
                _ if text.starts_with("--vfov=") => {
                    args.vfov = parse_vfov(&text["--vfov=".len()..]);
                }
                "--vfov" => {
                    let Some(value) = argv.next() else {
                        fail_usage("a value is required for '--vfov <VFOV>' but none was supplied");
                    };
                    args.vfov = parse_vfov(&value.to_string_lossy());
                }
                // A lone `-` is the stdin sentinel, not an option.
                _ if text.starts_with('-') && text != "-" => {
                    fail_usage(&format!("unexpected argument '{text}'"));
                }
                _ => {
                    if args.file.is_some() {
                        fail_usage(&format!("unexpected argument '{text}'"));
                    }
                    args.file = Some(PathBuf::from(arg));
                }
            }
        }
        args
    }
}

/// What is covering the canvas. Every overlay is modal for input: the menu,
/// the key reference and About all swallow viewer keys while they are up.
/// Esc closes them; Enter activates a menu item or closes Help/About.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    /// The Esc hub: Key Bindings, About, Exit.
    Menu,
    /// The key reference, reached from the menu and modal like the other
    /// panels: Esc/Enter goes back to the hub.
    Help,
    About,
}

/// The Esc menu's items, in display order. The indices are named so the
/// return paths from the sub-panels do not carry magic numbers.
const MENU_ITEMS: &[&str] = &["Key Bindings", "About", "Exit"];
const MENU_KEY_BINDINGS: usize = 0;
const MENU_ABOUT: usize = 1;
const MENU_EXIT: usize = 2;

/// One continuous degree of freedom: rotation (yaw / pitch / roll) and
/// translation (pan / dolly). Rotation comes in two frames: plain keys turn
/// around the fixed world axes (turntable), Ctrl keys around the model's own
/// current axes (trackball). The frame picks the axis, never the direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Motion {
    YawLeft,
    YawRight,
    PitchUp,
    PitchDown,
    RollPlus,
    RollMinus,
    /// Rotation around the model's own current axes.
    LocalYawLeft,
    LocalYawRight,
    LocalPitchUp,
    LocalPitchDown,
    LocalRollPlus,
    LocalRollMinus,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveForward,
    MoveBack,
}

/// What one key press does. A `Motion` key is continuous while held on a
/// terminal that reports key-up; a terminal that cannot report it gets one
/// finite step per press and auto-repeat. Every other action fires once per
/// press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Quit,
    Menu,
    Spin,
    Axes,
    Reset,
    Motion(Motion),
}

/// The task a binding belongs to. Groups drive the help panel's two columns and
/// keep the table readable; they are not a second source of truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Rotate,
    Local,
    Move,
    View,
    Session,
}

impl Group {
    /// The help panel's columns, top to bottom. World and model rotation lead
    /// the left column; the camera and session groups fill the right.
    const COLUMNS: [&[Group]; 2] = [
        &[Group::Rotate, Group::Local],
        &[Group::Move, Group::View, Group::Session],
    ];

    fn title(self) -> &'static str {
        match self {
            Group::Rotate => "ROTATE · world axes",
            Group::Local => "ROTATE · model axes",
            Group::Move => "MOVE",
            Group::View => "VIEW",
            Group::Session => "SESSION",
        }
    }
}

/// Fold one terminal key event into the key's identity (its *base*, un-shifted
/// form) plus whether SHIFT is in effect, so every later decision sees one
/// protocol-independent shape instead of two:
///
/// * the kitty protocol sends the layout's **shifted glyph** for every key
///   that has an alternate (`REPORT_ALTERNATE_KEYS` is pushed): `Shift+h` ->
///   `Char('H')`, `Shift+-` -> `Char('_')` — crossterm applies the alternate
///   and clears SHIFT. Keys without an alternate keep the modifier instead
///   (`Shift+Left` -> `Left` + SHIFT, `Shift+Space` -> `Char(' ')` + SHIFT);
/// * a legacy terminal sends the shifted **glyph** too, and crossterm
///   additionally synthesises SHIFT for upper-case letters (`Shift+h` ->
///   `Char('H')` + SHIFT, `Shift+/` -> `Char('?')` with NO modifier at all).
///
/// So an upper-case char implies shift, a known shifted glyph folds back to
/// its base key + shift, and anything else keeps the SHIFT modifier. The glyph
/// table maps the shifted glyphs the bindings use back to their US base keys —
/// the layouts' own glyphs arrive unchanged, so the same table serves both
/// paths. Caps lock therefore counts as shift on the legacy path and is a
/// no-op under kitty (crossterm files the kitty caps-lock bit under
/// `KeyEventState`, which is never read here) — the two cannot be told apart
/// from here.
const fn canonical_key(code: KeyCode, mods: KeyModifiers) -> (KeyCode, bool) {
    use KeyCode::Char;
    let shift = mods.contains(KeyModifiers::SHIFT);
    // Const-evaluable throughout: this may not reach for Unicode tables
    // (`is_uppercase`) or allocating case folding (`to_lowercase`). Every
    // chord is ASCII.
    match code {
        Char(c) if c.is_ascii_uppercase() => (Char(c.to_ascii_lowercase()), true),
        Char('?') => (Char('/'), true),
        Char('_') => (Char('-'), true),
        Char('+') => (Char('='), true),
        // BackTab *is* Shift+Tab: the legacy encoding reports it as its own
        // key with no modifier, the kitty one keeps SHIFT. Folding both
        // spellings to the shifted form lets one chord cover them.
        KeyCode::BackTab => (KeyCode::BackTab, true),
        KeyCode::Tab if shift => (KeyCode::BackTab, true),
        // Space and Esc are the two keys a terminal cannot report SHIFT for:
        // the legacy encoding sends the same bytes either way, so treating the
        // shifted spelling as a different key could only make the two terminal
        // paths disagree about a key that is in fact the same one.
        Char(' ') | KeyCode::Esc => (code, false),
        _ => (code, shift),
    }
}

/// One way to press a binding: the key event as the terminal reports it.
///
/// `code` / `mods` are the legacy (no kitty protocol) encoding, which is the
/// more explicit of the two: a shifted letter arrives as its upper-case glyph
/// **with** SHIFT still set, and a shifted symbol as that symbol (`?`, `+`,
/// `_`) with no modifier. Feeding these through `canonical_key` folds them
/// back to the documented chord, so the resolution tests can prove each
/// binding resolves to the action it claims. The kitty encoding is covered
/// separately by `resolve_key_event_folds_both_terminal_encodings`.
#[derive(Clone, Copy)]
struct Chord {
    code: KeyCode,
    mods: KeyModifiers,
}

/// A chord with no modifier.
const fn key(code: KeyCode) -> Chord {
    Chord {
        code,
        mods: KeyModifiers::NONE,
    }
}

/// `Shift` + `code`.
const fn shift(code: KeyCode) -> Chord {
    Chord {
        code,
        mods: KeyModifiers::SHIFT,
    }
}

/// `Ctrl` + `code`.
const fn ctrl(code: KeyCode) -> Chord {
    Chord {
        code,
        mods: KeyModifiers::CONTROL,
    }
}

/// One binding: an action, the task it belongs to, the keys the help panel
/// shows, and every way to press it.
///
/// `BINDINGS` is the one table the keyboard resolves against *and* the help
/// panel is generated from, so the keyboard and the help page cannot drift
/// apart. `keys` is display text for the help panel (`"h / ←"`); a test checks
/// that each binding's primary chord's label appears in it, so the displayed
/// key is at least the one that fires.
#[derive(Clone, Copy)]
struct Binding {
    action: Action,
    group: Group,
    /// The chord column of the help panel.
    keys: &'static str,
    /// The verb column of the help panel.
    doc: &'static str,
    /// Every documented way to press it.
    chords: &'static [Chord],
}

/// Every binding, grouped by the task it belongs to.
const BINDINGS: &[Binding] = &[
    // --- Rotate: the world's axes ---
    Binding {
        action: Action::Motion(Motion::YawLeft),
        group: Group::Rotate,
        keys: "h / ←",
        doc: "yaw left",
        chords: &[key(KeyCode::Char('h')), key(KeyCode::Left)],
    },
    Binding {
        action: Action::Motion(Motion::YawRight),
        group: Group::Rotate,
        keys: "l / →",
        doc: "yaw right",
        chords: &[key(KeyCode::Char('l')), key(KeyCode::Right)],
    },
    Binding {
        action: Action::Motion(Motion::PitchUp),
        group: Group::Rotate,
        keys: "k / ↑",
        doc: "pitch up",
        chords: &[key(KeyCode::Char('k')), key(KeyCode::Up)],
    },
    Binding {
        action: Action::Motion(Motion::PitchDown),
        group: Group::Rotate,
        keys: "j / ↓",
        doc: "pitch down",
        chords: &[key(KeyCode::Char('j')), key(KeyCode::Down)],
    },
    // `d` reads left and `f` reads right, the way hjkl reads spatially: on a
    // QWERTY home row `d` sits LEFT of `f`, so `d` rolls the image
    // anticlockwise (its left side dips) and `f` rolls it clockwise.
    Binding {
        action: Action::Motion(Motion::RollMinus),
        group: Group::Rotate,
        keys: "d",
        doc: "roll anticlockwise",
        chords: &[key(KeyCode::Char('d'))],
    },
    Binding {
        action: Action::Motion(Motion::RollPlus),
        group: Group::Rotate,
        keys: "f",
        doc: "roll clockwise",
        chords: &[key(KeyCode::Char('f'))],
    },
    // --- Rotate: the model's own axes ---
    Binding {
        action: Action::Motion(Motion::LocalYawLeft),
        group: Group::Local,
        keys: "Ctrl+h / Ctrl+←",
        doc: "yaw left",
        chords: &[ctrl(KeyCode::Char('h')), ctrl(KeyCode::Left)],
    },
    Binding {
        action: Action::Motion(Motion::LocalYawRight),
        group: Group::Local,
        keys: "Ctrl+l / Ctrl+→",
        doc: "yaw right",
        chords: &[ctrl(KeyCode::Char('l')), ctrl(KeyCode::Right)],
    },
    Binding {
        action: Action::Motion(Motion::LocalPitchUp),
        group: Group::Local,
        keys: "Ctrl+k / Ctrl+↑",
        doc: "pitch up",
        chords: &[ctrl(KeyCode::Char('k')), ctrl(KeyCode::Up)],
    },
    Binding {
        action: Action::Motion(Motion::LocalPitchDown),
        group: Group::Local,
        keys: "Ctrl+j / Ctrl+↓",
        doc: "pitch down",
        chords: &[ctrl(KeyCode::Char('j')), ctrl(KeyCode::Down)],
    },
    Binding {
        action: Action::Motion(Motion::LocalRollMinus),
        group: Group::Local,
        keys: "Ctrl+d",
        doc: "roll anticlockwise",
        chords: &[ctrl(KeyCode::Char('d'))],
    },
    Binding {
        action: Action::Motion(Motion::LocalRollPlus),
        group: Group::Local,
        keys: "Ctrl+f",
        doc: "roll clockwise",
        chords: &[ctrl(KeyCode::Char('f'))],
    },
    // --- Move: slide in the world's plane, and dolly along the sight line ---
    Binding {
        action: Action::Motion(Motion::MoveLeft),
        group: Group::Move,
        keys: "Shift+h / Shift+←",
        doc: "pan left",
        chords: &[shift(KeyCode::Char('H')), shift(KeyCode::Left)],
    },
    Binding {
        action: Action::Motion(Motion::MoveRight),
        group: Group::Move,
        keys: "Shift+l / Shift+→",
        doc: "pan right",
        chords: &[shift(KeyCode::Char('L')), shift(KeyCode::Right)],
    },
    Binding {
        action: Action::Motion(Motion::MoveUp),
        group: Group::Move,
        keys: "Shift+k / Shift+↑",
        doc: "pan up",
        chords: &[shift(KeyCode::Char('K')), shift(KeyCode::Up)],
    },
    Binding {
        action: Action::Motion(Motion::MoveDown),
        group: Group::Move,
        keys: "Shift+j / Shift+↓",
        doc: "pan down",
        chords: &[shift(KeyCode::Char('J')), shift(KeyCode::Down)],
    },
    // `+` is how several layouts spell the shifted `=`; it folds to `=`.
    // The plain glyph leads so the help and the strip show `+`, which is what
    // an operator actually types to zoom in.
    Binding {
        action: Action::Motion(Motion::MoveForward),
        group: Group::Move,
        keys: "+ / =",
        doc: "zoom in",
        chords: &[key(KeyCode::Char('+')), key(KeyCode::Char('='))],
    },
    Binding {
        action: Action::Motion(Motion::MoveBack),
        group: Group::Move,
        keys: "- / _",
        doc: "zoom out",
        chords: &[key(KeyCode::Char('-')), key(KeyCode::Char('_'))],
    },
    // --- View: where the model sits on screen ---
    Binding {
        action: Action::Reset,
        group: Group::View,
        keys: "r",
        doc: "reset the view",
        chords: &[key(KeyCode::Char('r'))],
    },
    Binding {
        action: Action::Spin,
        group: Group::View,
        keys: "Space",
        doc: "toggle spin",
        chords: &[key(KeyCode::Char(' '))],
    },
    // Tab is the one chord for the axes. A legacy terminal reports
    // Shift+Tab as BackTab, kitty reports Tab with SHIFT; BackTab is the
    // spelling both paths agree on.
    Binding {
        action: Action::Axes,
        group: Group::View,
        keys: "Tab",
        doc: "toggle axes",
        chords: &[key(KeyCode::Tab), shift(KeyCode::BackTab)],
    },
    // --- Session ---
    Binding {
        action: Action::Menu,
        group: Group::Session,
        keys: "Esc",
        doc: "open menu",
        chords: &[key(KeyCode::Esc)],
    },
    Binding {
        action: Action::Quit,
        group: Group::Session,
        keys: "q / Ctrl+c",
        doc: "quit",
        chords: &[
            key(KeyCode::Char('q')),
            // Raw mode turns Ctrl+C into a plain key event (no SIGINT), so it
            // is ours to bind. Ctrl+Shift+C stays unbound: that is a terminal
            // copy shortcut, not a quit.
            ctrl(KeyCode::Char('c')),
        ],
    },
];

/// Resolve a terminal key event to the key's identity and the action it fires.
///
/// A lookup over the one table: the identity that comes back is what a hold is
/// filed under, so a Release with different modifiers still finds the motion it
/// started. The table is tiny (a couple of dozen chords), so a linear scan is
/// the clearest implementation — there is no generated keymap to keep in step.
fn resolve_key_event(code: KeyCode, mods: KeyModifiers) -> Option<(KeyCode, Action)> {
    // A chord is a key plus at most Shift and Ctrl: Alt, Super, Hyper and Meta
    // bind nothing, whatever the key is.
    let allowed = KeyModifiers::SHIFT.union(KeyModifiers::CONTROL);
    if !mods.difference(allowed).is_empty() {
        return None;
    }
    let (base, shift) = canonical_key(code, mods);
    let control = mods.contains(KeyModifiers::CONTROL);
    BINDINGS
        .iter()
        .flat_map(|binding| {
            binding
                .chords
                .iter()
                .map(move |chord| (binding.action, chord))
        })
        .find(|(action, chord)| {
            let (chord_base, chord_shift) = canonical_key(chord.code, chord.mods);
            if chord_base != base || chord.mods.contains(KeyModifiers::CONTROL) != control {
                return false;
            }
            // Ctrl motion chords ignore Shift, matching the old Ctrl+Shift+h
            // alias. Ctrl+C quit stays strict: Ctrl+Shift+C is a terminal copy
            // shortcut, not a quit.
            if control && !matches!(*action, Action::Quit) {
                true
            } else {
                chord_shift == shift
            }
        })
        .map(|(action, _)| (base, action))
}

/// The conventional spelling of a chord: modifiers in `Ctrl+Shift+` order,
/// then the key. A shifted letter is the lowercase key (`Shift+h`), because the
/// chord names the key, not the glyph. Test-only: the help panel's display
/// text lives in [`Binding::keys`], and a test ties the two together.
#[cfg(test)]
impl Chord {
    fn label(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push_str("Ctrl+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push_str("Shift+");
        }
        match self.code {
            KeyCode::Char(' ') => out.push_str("Space"),
            KeyCode::Char(c) => {
                if self.mods.contains(KeyModifiers::SHIFT) && c.is_ascii_uppercase() {
                    out.push(c.to_ascii_lowercase());
                } else {
                    out.push(c);
                }
            }
            KeyCode::Esc => out.push_str("Esc"),
            KeyCode::Tab => out.push_str("Tab"),
            KeyCode::BackTab => out.push_str(if self.mods.contains(KeyModifiers::SHIFT) {
                "Tab"
            } else {
                "BackTab"
            }),
            KeyCode::Left => out.push_str("Left"),
            KeyCode::Right => out.push_str("Right"),
            KeyCode::Up => out.push_str("Up"),
            KeyCode::Down => out.push_str("Down"),
            other => out.push_str(&format!("{other:?}")),
        }
        out
    }
}

/// One frame of smooth continuous motion for a held key (model follows key).
fn continuous_step(view: &mut ViewState, m: Motion, speed: f64, dt: f64) {
    apply_motion_step(view, m, ROT_RATE * dt, speed * dt);
}

/// Apply one motion step: rotation around the world or the model's own axes,
/// pan in world X/Y, dolly along the view axis.
fn apply_motion_step(view: &mut ViewState, m: Motion, rot: f64, mv: f64) {
    match m {
        // World-frame rotation: yaw/pitch pre-multiply the model->world matrix
        // with a fixed world-axis rotation, so the axes are anchored to the
        // world and never follow the model. The keys read as the viewer sees
        // them: the model faces out of the screen while the sight line points
        // into it, so plain `h` sweeps the nose toward screen-left. Pitch has
        // no mirror (the model's up is the viewer's up), so `k`/`j` read the
        // same way.
        Motion::YawLeft => view.add_yaw(-rot),
        Motion::YawRight => view.add_yaw(rot),
        Motion::PitchUp => view.add_pitch(-rot),
        Motion::PitchDown => view.add_pitch(rot),
        // World-frame roll: the third member of the yaw/pitch family —
        // positive roll spins the image anticlockwise, so `d` (QWERTY, left of
        // `f`) dips the viewer's screen-left side and `f` dips the right.
        Motion::RollPlus => view.add_roll(-rot),
        Motion::RollMinus => view.add_roll(rot),
        // Local-frame rotation: the same step post-multiplied, so the axis
        // rides with the model. The direction convention is the same as the
        // world keys; only the axis frame changes.
        Motion::LocalYawLeft => view.add_yaw_local(rot),
        Motion::LocalYawRight => view.add_yaw_local(-rot),
        Motion::LocalPitchUp => view.add_pitch_local(-rot),
        Motion::LocalPitchDown => view.add_pitch_local(rot),
        Motion::LocalRollPlus => view.add_roll_local(rot),
        Motion::LocalRollMinus => view.add_roll_local(-rot),
        // Pan shifts the model in world X/Y at any orientation; the rotation
        // centre is the panned file origin (see view::project_point).
        Motion::MoveLeft => view.pan_x -= mv,
        Motion::MoveRight => view.pan_x += mv,
        Motion::MoveUp => view.pan_y += mv,
        Motion::MoveDown => view.pan_y -= mv,
        // Translation along the view axis (camera distance).
        Motion::MoveForward => view.add_dist_delta(-mv),
        Motion::MoveBack => view.add_dist_delta(mv),
    }
    // Angles are periodic; keep the displayed values in [-PI, PI].
    view.normalize();
}

/// How many leading bytes are probed to detect the file format.
const PROBE_BYTES: usize = 4096;

/// True when `path` is a FIFO (named pipe, e.g. bash's `<( cmd )` process substitution).
#[cfg(unix)]
fn is_fifo_path(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path)
        .map(|m| m.file_type().is_fifo())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_fifo_path(_path: &Path) -> bool {
    false
}

/// Verify a controlling terminal is available when stdin is a pipe.
#[cfg(unix)]
fn ensure_keyboard_terminal() -> Result<(), String> {
    if std::io::stdin().is_terminal() {
        return Ok(());
    }
    // stdin is not a terminal: keyboard must come from /dev/tty.
    std::fs::OpenOptions::new()
 .read(true)
 .write(true)
        .open("/dev/tty")
 .map(|_| ())
 .map_err(|e| {
 format!(
                "cannot open a terminal for keyboard input (/dev/tty): {e}\n'wireforge -' with stdin piped needs a controlling terminal to be interactive.\nUse a FIFO so the terminal stays free for keyboard:\n    wireforge <( cat model.wrfm )"
 )
 })
}

#[cfg(not(unix))]
fn ensure_keyboard_terminal() -> Result<(), String> {
    Ok(())
}

/// Probe a byte buffer: `.wrfm` magic passes, OBJ content gets a pointer
/// to the converter, everything else is unrecognized. The magic always
/// wins (obj-looking markers later in the head do not matter).
///
/// Only the first [`PROBE_BYTES`] bytes — the head — are examined: the
/// format is decided where a file decides it, so a 90 MB stdin buffer is
/// never scanned (or re-validated as UTF-8) just to classify it.
fn probe_bytes(buf: &[u8]) -> Result<(), String> {
    let buf = &buf[..PROBE_BYTES.min(buf.len())];
    // Lossy: a stray non-UTF-8 byte must never decide the format.
    let head = String::from_utf8_lossy(buf);
    // Strip a leading BOM like the wrfm parser does, so a BOM before the
    // magic line does not hide it.
    let head = head.strip_prefix('\u{feff}').unwrap_or(&head);

    // Magic first: the first line of a v2 wrfm file is `wrfm <version>`.
    // The magic is a short line, so it always fits within PROBE_BYTES.
    let first_line = head.lines().next().unwrap_or("");
    if first_line.split_whitespace().next() == Some("wrfm") {
        return Ok(());
    }

    if looks_like_obj(head) {
        return Err(
            "OBJ content: wireforge reads .wrfm only -- convert it first: \
             `wrfm convert <file> | wireforge -`"
                .to_string(),
        );
    }
    Err(
        "unrecognized file format: no wrfm magic line (`wrfm <version>`). \
         Got an OBJ file? Convert it first: `wrfm convert <file>`"
            .to_string(),
    )
}

/// True for the OBJ marker shapes the converter accepts: real markers
/// (`f`/`vt`/`vn`), or vertices without wrfm edges (a bare point cloud).
fn looks_like_obj(head: &str) -> bool {
    let mut has_vertex = false;
    let mut has_wrfm_edge = false;
    let mut has_obj_marker = false;
    for raw in head.lines() {
        let line = raw.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_whitespace().next() {
            Some("v") => has_vertex = true,
            Some("e") => has_wrfm_edge = true,
            Some("f") | Some("vt") | Some("vn") => has_obj_marker = true,
            _ => {}
        }
    }
    has_obj_marker || (has_vertex && !has_wrfm_edge)
}

/// Read `target_file` once and probe its head as `.wrfm`; errors are
/// path-prefixed. The returned text is what the parser sees — the path is
/// opened exactly once, so probe and parse can never disagree about bytes.
fn read_probed_file(target_file: &Path) -> Result<String, String> {
    let bytes = std::fs::read(target_file)
        .map_err(|e| format!("cannot read '{}': {e}", target_file.display()))?;
    probe_bytes(&bytes).map_err(|e| format!("'{}': {e}", target_file.display()))?;
    String::from_utf8(bytes)
        .map_err(|e| format!("'{}': not valid UTF-8: {e}", target_file.display()))
}

/// Parse the file that [`read_probed_file`] already read and checked.
fn load_model(target_file: &Path) -> Result<(Model, String), String> {
    let text = read_probed_file(target_file)?;
    let name = target_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown");
    let wrfm_data = WrfmModel::from_str(name, &text).map_err(|e| e.to_string())?;
    let model = Model {
        vertices: wrfm_data.vertices,
        edges: wrfm_data.edges,
    };
    Ok((model, wrfm_data.name))
}

/// Validate a stream (stdin / FIFO) as `.wrfm`, then parse it; `name` labels it.
fn load_model_from_text(name: &str, text: &str) -> Result<(Model, String), String> {
    probe_bytes(text.as_bytes())?;
    let wrfm_data = WrfmModel::from_str(name, text).map_err(|e| e.to_string())?;
    let model = Model {
        vertices: wrfm_data.vertices,
        edges: wrfm_data.edges,
    };
    Ok((model, wrfm_data.name))
}

// ---------------------------------------------------------------------------
// The bar and the overlays
// ---------------------------------------------------------------------------

/// One painted cell of a bar: char, foreground slot, background slot.
type Cell = (char, Ink, Ink);

/// Append `text` to `cells` in one ink. Bar text is single-width by
/// construction: ASCII, the powerline separators, `●`, `°`, `×` and `…` all
/// occupy one cell in the terminals this viewer targets.
fn put(cells: &mut Vec<Cell>, text: &str, fg: Ink, bg: Ink) {
    cells.extend(text.chars().map(|ch| (ch, fg, bg)));
}

/// Fill `cells` with blanks until it holds `width` columns.
fn pad_to(cells: &mut Vec<Cell>, width: usize, bg: Ink) {
    while cells.len() < width {
        cells.push((' ', Ink::Default, bg));
    }
}

/// Paint one row of cells into the retained screen.
fn paint_cells(screen: &mut render::Screen, row: usize, cells: &[Cell]) {
    for (x, &(ch, fg, bg)) in cells.iter().enumerate() {
        screen.set(x, row, ch, fg as u8, bg as u8);
    }
}

/// One metric of the camera readout: the name that labels it, the value it
/// shows, and the width of the column the value lives in.
///
/// The column is a constant, not the width of the value: the readout is a
/// row of fixed columns, so a value that grows — a minus sign, one more
/// digit — takes the room to its right and never moves the label beside it
/// or the ones after it.
struct Metric {
    label: &'static str,
    value: String,
    column: usize,
}

/// The width of an angle column: `-180°` is the widest angle the readout
/// shows, and it is five cells.
const ANGLE_COLUMN: usize = 5;
/// The width of the zoom column: `9999.99×`, the widest zoom a reader is
/// likely to reach.
const ZOOM_COLUMN: usize = 8;
/// The width of the pan column: `-123.45, 678.90`, a pan in model units.
const PAN_COLUMN: usize = 15;
/// The cells between two metric columns.
const METRIC_GAP: &str = "   ";

/// The camera readout, in the order it is shown and shed.
///
/// Angles are degrees and wrapped to (-180, 180]; zoom is a multiple of the
/// start-up fit distance, which is far easier to read than the raw camera
/// distance in model units; pan is in model units, where it is a real
/// displacement rather than a fraction of an unknown frame.
///
/// Every value sits in a column of its own, as wide as the widest value that
/// metric can show. The number starts right against the label that names it
/// (`yaw 12°`) and the rest of its column is the room it grows into, so the
/// labels hold their places while the numbers change.
fn metrics(app: &App) -> Vec<Metric> {
    fn degrees(rad: f64) -> String {
        let deg = rad.to_degrees();
        // `round` keeps -0.0 for a tiny negative angle; a viewer reading
        // "-0°" would wonder what changed.
        let deg = if deg.abs() < 0.5 { 0.0 } else { deg.round() };
        format!("{deg:.0}°")
    }
    // Pan is a model-space displacement; a tiny negative step should read as
    // 0.00, not as a mysterious -0.00.
    let coord = |value: f64| {
        if value.abs() < 0.005 { 0.0 } else { value }
    };
    vec![
        Metric {
            label: "yaw",
            value: degrees(app.view.yaw),
            column: ANGLE_COLUMN,
        },
        Metric {
            label: "pitch",
            value: degrees(app.view.pitch),
            column: ANGLE_COLUMN,
        },
        Metric {
            label: "roll",
            value: degrees(app.view.roll),
            column: ANGLE_COLUMN,
        },
        Metric {
            label: "zoom",
            value: format!("{:.2}×", app.fit_dist / app.view.dist),
            column: ZOOM_COLUMN,
        },
        Metric {
            label: "pan",
            value: format!("{:.2}, {:.2}", coord(app.view.pan_x), coord(app.view.pan_y)),
            column: PAN_COLUMN,
        },
    ]
}

/// Width of a metric row including the gaps between the columns.
///
/// The width is the sum of the columns, never the sum of the values: the
/// readout is as wide as its columns are, whatever the numbers say. That is
/// what keeps the row from jumping — the block does not move when a value
/// grows, and what fits on the row does not change under the reader's feet.
fn metrics_width(metrics: &[Metric]) -> usize {
    let rows: usize = metrics
        .iter()
        .map(|metric| metric.label.len() + 1 + metric.column)
        .sum();
    rows + metrics.len().saturating_sub(1) * METRIC_GAP.len()
}

/// Write a value into its column: the number is cut to `column` cells and
/// the cut is marked with an ellipsis, so a value that outgrows its column
/// says so instead of running into the next one. A value that fits leaves the
/// rest of the column empty — the room it grows into.
fn put_column(cells: &mut Vec<Cell>, value: &str, column: usize, fg: Ink, bg: Ink) {
    let width = value.chars().count();
    for ch in value.chars().take(column) {
        cells.push((ch, fg, bg));
    }
    if width <= column {
        for _ in width..column {
            cells.push((' ', fg, bg));
        }
        return;
    }
    // The value is wider than its column: the last cell becomes the mark
    // that says the number was cut.
    if column > 0
        && let Some((ch, _, _)) = cells.last_mut()
    {
        *ch = '…';
    }
}

/// Blank cells kept between the camera readout and the right edge of the bar.
/// The readout is right-aligned and its columns are fixed, so the block holds
/// its place; a few cells of air give the numbers somewhere to breathe.
const READOUT_MARGIN: usize = 3;

/// One lamp: one filled dot whose ink carries the state — lit in the accent,
/// unlit in the bar's own deep ink — then its name. The unlit dot is dark on
/// the lamps' block, so an off lamp reads as unlit rather than as missing.
fn push_lamp(cells: &mut Vec<Cell>, on: bool, label: &str) {
    put(
        cells,
        "●",
        if on { Ink::Accent } else { Ink::Surface },
        Ink::Lamp,
    );
    put(cells, " ", Ink::Foreground, Ink::Lamp);
    put(cells, label, Ink::Foreground, Ink::Lamp);
}

/// The state lamps as one block of the bar: a cell of padding around every
/// lamp, a [`SEAM`] between two of them, and the [`WEDGE`] that hands the
/// block's ground to the bar. `show_spin` / `show_axes` decide whether a lamp
/// is drawn at all; its state comes from the viewer.
fn lamps_cells(app: &App, show_spin: bool, show_axes: bool) -> Vec<Cell> {
    let mut cells = Vec::with_capacity(20);
    if !show_spin && !show_axes {
        return cells;
    }
    put(&mut cells, " ", Ink::Foreground, Ink::Lamp);
    if show_spin {
        push_lamp(&mut cells, app.auto_spin, "SPIN");
    }
    if show_axes {
        if show_spin {
            put(&mut cells, " ", Ink::Foreground, Ink::Lamp);
            cells.push((SEAM, Ink::Surface, Ink::Lamp));
        }
        put(&mut cells, " ", Ink::Foreground, Ink::Lamp);
        push_lamp(&mut cells, app.show_axes, "AXES");
    }
    put(&mut cells, " ", Ink::Foreground, Ink::Lamp);
    cells.push((WEDGE, Ink::Lamp, Ink::Surface));
    cells
}

/// The filled powerline wedge that closes a block of the bar (U+E0B0): the
/// block's own ink over the ground it hands off to, so the block tapers into a
/// full-height arrow — the separator an nvim lualine or a powerline zsh prompt
/// draws between two sections. It closes the name pill and the lamps alike.
const WEDGE: char = '\u{e0b0}';
/// The thin powerline seam between two items of one block (U+E0B1): its point
/// faces right, the way the bar reads, so one lamp's run ends and the next
/// begins without the block breaking.
const SEAM: char = '\u{e0b1}';

/// The bar under the model, laid out like a powerline statusline: the model's
/// name in an accent pill on the left, the state lamps in a lighter block beside
/// it, and the camera telemetry right-aligned on the bar's own ground. Every
/// block is closed by a [`WEDGE`] in its own ground, so the pill's colour
/// tapers into the lamps and the lamps into the bar — lualine's sections.
///
/// The row carries no key hints: `Esc` opens the menu, whose Key Bindings has
/// the complete list. On a narrow row the lamps shed first (AXES before SPIN),
/// then the readout sheds from its right end (pan first). The name may never
/// take more than half the row, and a cut name is marked with `…`. The readout
/// is a row of fixed columns, so a value that grows takes the room to its
/// right and the labels hold their places; the block stops [`READOUT_MARGIN`]
/// short of the right edge, where it has some air to breathe.
fn bar_cells(app: &App, width: usize) -> Vec<Cell> {
    let mut cells = Vec::with_capacity(width);
    if width < 3 {
        pad_to(&mut cells, width, Ink::Surface);
        return cells;
    }
    let named = app.strip_label.as_deref().unwrap_or(&app.name);
    let full = named.chars().count();
    // The name may never take more than half the row: the readout it shares
    // the row with is the reason the row exists.
    let cap = (width / 2).saturating_sub(3).max(1);
    let mut label: Vec<char> = named.chars().take(cap).collect();
    if full > cap
        && let Some(last) = label.last_mut()
    {
        *last = '…';
    }
    // The pill is a cell of padding, the name, a cell of padding, and the wedge
    // that closes it. A row too tight for the wedge keeps the name and stops
    // short of the arrow.
    let wedge = label.len() + 3 <= width;
    let pill_len = label.len() + usize::from(wedge) + 2;

    let mut metrics = metrics(app);
    // These are *visibility* flags: both lamps are drawn when they fit, and
    // each lamp's on/off state comes from the viewer (`push_lamp`).
    let mut show_axes = true;
    let mut show_spin = true;
    let fits = |lamps_len: usize, metrics: &[Metric]| {
        // The readout keeps its margin at the right edge on top of the gap
        // that separates it from the lamps.
        let gap = if metrics.is_empty() {
            0
        } else {
            2 + READOUT_MARGIN
        };
        pill_len + lamps_len + gap + metrics_width(metrics) <= width
    };
    loop {
        let lamps = lamps_cells(app, show_spin, show_axes);
        if fits(lamps.len(), &metrics) {
            break;
        }
        if show_axes {
            show_axes = false;
        } else if show_spin {
            show_spin = false;
        } else if !metrics.is_empty() {
            metrics.pop();
        } else {
            break;
        }
    }

    // The pill, closed by the wedge in the pill's own ink over the ground it
    // hands off to: the lamps' block when one is drawn, the bar otherwise.
    put(&mut cells, " ", Ink::OnAccent, Ink::Accent);
    for ch in &label {
        cells.push((*ch, Ink::OnAccent, Ink::Accent));
    }
    put(&mut cells, " ", Ink::OnAccent, Ink::Accent);
    if wedge {
        let ahead = if show_spin || show_axes {
            Ink::Lamp
        } else {
            Ink::Surface
        };
        cells.push((WEDGE, Ink::Accent, ahead));
    }

    cells.extend(lamps_cells(app, show_spin, show_axes));
    if !metrics.is_empty() {
        // Right-align the readout, leaving the lamps and the readout at least
        // two cells apart and the readout `READOUT_MARGIN` short of the right
        // edge (the fit test above is what guarantees both). Its columns are
        // fixed, so the block is always the same width and holds its place.
        let metrics_len = metrics_width(&metrics);
        pad_to(
            &mut cells,
            width - READOUT_MARGIN - metrics_len,
            Ink::Surface,
        );
        for (i, metric) in metrics.iter().enumerate() {
            if i > 0 {
                put(&mut cells, METRIC_GAP, Ink::Foreground, Ink::Surface);
            }
            // The label and the value share one ink: the readout is read as
            // one line of numbers, and a dim label beside a bright number
            // only competes with it.
            put(&mut cells, metric.label, Ink::Foreground, Ink::Surface);
            put(&mut cells, " ", Ink::Foreground, Ink::Surface);
            put_column(
                &mut cells,
                &metric.value,
                metric.column,
                Ink::Foreground,
                Ink::Surface,
            );
        }
    }
    pad_to(&mut cells, width, Ink::Surface);
    debug_assert_eq!(cells.len(), width, "the bar fills its row");
    cells
}

// ---------------------------------------------------------------------------
// The help panel
// ---------------------------------------------------------------------------

/// One column of the help panel, generated from [`BINDINGS`].
///
/// The chord gutter is measured from the data, so a longer key or a new row
/// cannot leave the verbs ragged. It is measured over the whole table, not
/// per column, so the verbs of the two columns start at the same offset and
/// the panel reads as one grid. A group heading is cut to its column's width,
/// which puts the rules of the panel on the same right edge as its rows.
/// Groups that are not in `groups` are skipped; the table order inside a group
/// is the order it was written in.
fn help_column(groups: &[Group]) -> Vec<Line<'static>> {
    let key_w = BINDINGS
        .iter()
        .map(|binding| binding.keys.chars().count())
        .max()
        .unwrap_or(0);
    let doc_w = BINDINGS
        .iter()
        .filter(|binding| groups.contains(&binding.group))
        .map(|binding| binding.doc.chars().count())
        .max()
        .unwrap_or(0);
    // The column is its widest row: the gutter, the gap and the longest verb.
    let col_w = key_w + 2 + doc_w;

    let mut lines = Vec::new();
    for &group in groups {
        let title = group.title();
        let rule = col_w.saturating_sub(title.chars().count() + 1);
        lines.push(Line::from(Span::styled(
            format!("{title} {}", "─".repeat(rule)),
            Style::default().fg(Ink::Accent.color()),
        )));
        for binding in BINDINGS.iter().filter(|binding| binding.group == group) {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<key_w$}", binding.keys),
                    Style::default().fg(Ink::AccentSoft.color()),
                ),
                Span::styled(
                    format!("  {}", binding.doc),
                    Style::default().fg(Ink::Foreground.color()),
                ),
            ]));
        }
        lines.push(Line::default());
    }
    lines.pop(); // no trailing gap after the last group
    lines
}

/// The complete key reference, as two columns of task groups. Everything here
/// comes from [`BINDINGS`]; there is no second copy to drift.
fn help_lines() -> Vec<Line<'static>> {
    let left = help_column(Group::COLUMNS[0]);
    let right = help_column(Group::COLUMNS[1]);
    let left_w = left.iter().map(Line::width).max().unwrap_or(0);

    let mut out = Vec::with_capacity(left.len().max(right.len()));
    for i in 0..left.len().max(right.len()) {
        let mut line = left.get(i).cloned().unwrap_or_default();
        // Pad by display width: the chords carry arrows and other glyphs whose
        // byte length is not their width on screen, and the right column must
        // start at the same offset on every row.
        let pad = left_w.saturating_sub(line.width()) + 2;
        line.spans.push(Span::raw(" ".repeat(pad)));
        if let Some(right_line) = right.get(i) {
            line.spans.extend(right_line.spans.iter().cloned());
        }
        out.push(line);
    }
    out
}

/// The Esc menu: Key Bindings, About, Exit. The cursor is the whole
/// interaction; Enter activates, Esc closes.
fn menu_lines(app: &App) -> Vec<Line<'static>> {
    let name_w = MENU_ITEMS.iter().map(|item| item.len()).max().unwrap_or(0);
    let footer = "j/k move   Enter select   Esc close";

    let mut lines = Vec::with_capacity(MENU_ITEMS.len() + 2);
    for (i, item) in MENU_ITEMS.iter().enumerate() {
        let is_selected = i == app.menu_selection;
        // The panel wears the canvas ground, so no row raises one of its own:
        // the cursor is the marker and the accent ink, and the glyph carries
        // the selection on its own.
        let name_fg = if is_selected {
            Ink::Accent.color()
        } else {
            Ink::Foreground.color()
        };
        lines.push(Line::from(vec![
            Span::styled(
                if is_selected { "▸ " } else { "  " },
                Style::default().fg(Ink::Accent.color()),
            ),
            Span::styled(format!("{item:<name_w$}"), Style::default().fg(name_fg)),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(Ink::Muted.color()),
    )));
    lines
}

/// The About panel. It is a menu item, so Esc/Enter goes back to the hub.
fn about_lines() -> Vec<Line<'static>> {
    let version = env!("CARGO_PKG_VERSION");
    vec![
        Line::from(Span::styled(
            "wireforge",
            Style::default().fg(Ink::Accent.color()),
        )),
        Line::from(Span::styled(
            format!("Version {version}"),
            Style::default().fg(Ink::Foreground.color()),
        )),
        Line::default(),
        Line::from(Span::styled(
            "A TUI viewer for .wrfm 3D wireframe models,",
            Style::default().fg(Ink::Foreground.color()),
        )),
        Line::from(Span::styled(
            "drawn as braille text.",
            Style::default().fg(Ink::Foreground.color()),
        )),
        Line::default(),
        Line::from(Span::styled(
            "https://github.com/lenitain/wireforge",
            Style::default().fg(Ink::AccentSoft.color()),
        )),
        Line::from(Span::styled(
            "MIT license",
            Style::default().fg(Ink::Muted.color()),
        )),
    ]
}

/// The lines and title of the overlay currently up, if any.
fn overlay_panel(app: &App) -> Option<(Vec<Line<'static>>, &'static str)> {
    match app.overlay {
        Overlay::None => None,
        Overlay::Menu => Some((menu_lines(app), " Menu (Esc to close) ")),
        Overlay::Help => Some((help_lines(), " Keys (Esc to close) ")),
        Overlay::About => Some((about_lines(), " About (Esc to close) ")),
    }
}

/// The panel's chrome — how much space the borders and padding eat around the
/// content. The natural panel wears a rounded border plus 2 chars of padding
/// on each side; chrome shrinks as the terminal narrows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PanelChrome {
    /// Borders + 2 chars of padding on each side (the default look).
    Full,
    /// Borders + 1 char of padding on each side.
    Padded,
    /// Borders only, no padding.
    Bare,
    /// No borders, no padding — content uses every column.
    Inline,
}

/// Pick the chrome that lets the content fit in `term_w`: start from `Full` and
/// shrink padding, then drop borders, until the content's widest row plus the
/// chrome fits.
fn panel_chrome(widest: u16, term_w: u16) -> PanelChrome {
    let fits = |chrome_w: u16| widest + chrome_w <= term_w;
    if fits(6) {
        PanelChrome::Full
    } else if fits(4) {
        PanelChrome::Padded
    } else if fits(2) {
        PanelChrome::Bare
    } else {
        PanelChrome::Inline
    }
}

impl PanelChrome {
    /// Cols the chrome eats horizontally (borders + padding, both sides).
    fn width(self) -> u16 {
        match self {
            PanelChrome::Full => 6,
            PanelChrome::Padded => 4,
            PanelChrome::Bare => 2,
            PanelChrome::Inline => 0,
        }
    }

    /// Rows the chrome eats vertically (borders only; padding is horizontal).
    fn height(self) -> u16 {
        match self {
            PanelChrome::Full | PanelChrome::Padded | PanelChrome::Bare => 2,
            PanelChrome::Inline => 0,
        }
    }

    /// Whether to draw the rounded border. Padding is rendered inside it.
    fn borders(self) -> Borders {
        match self {
            PanelChrome::Inline => Borders::NONE,
            _ => Borders::ALL,
        }
    }

    /// Chars of padding on each side. `0` means tight to the border (or the
    /// edge, when there is no border).
    fn padding(self) -> u16 {
        match self {
            PanelChrome::Full => 2,
            PanelChrome::Padded => 1,
            _ => 0,
        }
    }

    /// The total panel width for `widest` content inside `term_w`. The panel
    /// uses whatever horizontal space the chrome leaves, so the content can
    /// reach the right edge of the screen instead of clipping at the panel.
    fn panel_width(self, widest: u16, term_w: u16) -> u16 {
        (widest + self.width()).min(term_w)
    }
}

// Game-engine event loop: an input thread + channel, one scheduler owned
// by the loop, a two-mode main loop, and dirty-flag rendering.

/// All viewer state owned by the main loop.
struct App {
    current: Model,
    /// The model's geometric-mean length ([`view::model_extent`]), computed
    /// once because `current` is never mutated after `App::new`.
    ///
    /// It sizes the axes, so re-deriving it per frame would put an O(n)
    /// bounds scan on every frame. The bounding box only depends on the
    /// model, never on the view, so one scan at load is enough.
    extent: f64,
    /// The camera distance at start-up, which is the fit distance for the
    /// session. The telemetry reports zoom as a multiple of it: `2.00×` means
    /// the camera is twice as close as the framing the file opened with.
    fit_dist: f64,
    /// Translation speed in world units per second, set once at construction
    /// from the initial camera distance: the model's coordinate centre
    /// reaches the camera in 5 s at the initial pose, and the rate is pinned
    /// for the session (zoom in/out keeps this speed, by design).
    speed: f64,
    name: String,
    /// Title-bar label override: the FIFO path for a stream preview, whose
    /// model name is only the file stem; `None` shows the model name.
    strip_label: Option<String>,
    view: ViewState,
    /// Keys currently held down, keyed by their identity (the base key from
    /// `canonical_key`), so Release finds the right hold even when the
    /// modifiers changed since the Press.
    held: HashMap<KeyCode, Motion>,
    /// True once the terminal delivered a Release. From then on a key press
    /// can start a continuous hold; before that (a legacy terminal, or the very
    /// first key of a kitty session) a press is one finite step.
    release_seen: bool,
    auto_spin: bool,
    overlay: Overlay,
    /// The Esc menu's cursor, preserved across sub-panels and reopenings.
    menu_selection: usize,
    show_axes: bool,
    /// True when the screen must be repainted before the loop blocks again.
    dirty: bool,
}

impl App {
    fn new(current: Model, name: String, strip_label: Option<String>, vfov: f64) -> Self {
        // One extent for the whole session: it frames the model now and then
        // serves every later frame (see the field's docs).
        let extent = view::model_extent(&current);
        let mut view = ViewState {
            vfov,
            ..ViewState::default()
        };
        // Frame the model (an empty one lands at the default distance), so a
        // fresh App is always correctly framed — including the blank start-up
        // view, whose extent is undefined.
        view.fit_to_extent(extent);
        let fit_dist = view.dist;
        // Translation speed: 5 s for the model centre to reach the camera
        // from the initial pose. Pinned at start-up so the parameter set
        // driving motion is purely session-initial.
        let speed = view.dist / 5.0;
        App {
            current,
            extent,
            fit_dist,
            speed,
            name,
            strip_label,
            view,
            held: HashMap::new(),
            release_seen: false,
            auto_spin: false,
            overlay: Overlay::None,
            menu_selection: 0,
            show_axes: true,
            dirty: true,
        }
    }

    /// Translation speed in world units per second (see [`App::speed`]): the
    /// model's coordinate centre reaches the camera in 5 s, fixed at start-up.
    fn translation_speed(&self) -> f64 {
        self.speed
    }

    /// One finite motion step, for a tap or for a terminal that cannot report
    /// key-up. The step is small enough to be a nudge and large enough to be
    /// visible: a tap must never depend on how long the frame took.
    fn apply_tap(&mut self, motion: Motion) {
        let speed = self.translation_speed();
        let mv = match motion {
            Motion::MoveForward | Motion::MoveBack => speed * TAP_DOLLY,
            _ => speed * TAP_PAN,
        };
        apply_motion_step(&mut self.view, motion, TAP_ROT, mv);
        self.dirty = true;
    }

    /// Key-up: drop the hold by identity. The modifiers of the Release event
    /// are irrelevant — they may well differ from the Press (release Shift
    /// before `h` and the event no longer carries SHIFT).
    fn hold_key_up(&mut self, hold_id: KeyCode) {
        if self.held.remove(&hold_id).is_some() {
            self.dirty = true;
        }
    }

    /// Esc: the hub. It is modal — navigation moves its cursor and Enter
    /// activates an item — while q and Ctrl+C still fall through to the
    /// global quit.
    fn open_menu(&mut self) {
        self.held.clear();
        // Like a fresh menu: Esc then Enter always lands on the first item,
        // never on an Exit the previous visit happened to leave selected.
        self.menu_selection = MENU_KEY_BINDINGS;
        self.overlay = Overlay::Menu;
        self.dirty = true;
    }

    /// Move the menu cursor, wrapping at both ends.
    fn move_menu(&mut self, delta: isize) {
        let count = MENU_ITEMS.len() as isize;
        self.menu_selection = (self.menu_selection as isize + delta).rem_euclid(count) as usize;
        self.dirty = true;
    }

    /// Run the selected menu item. Returns true when the item is Exit.
    fn activate_menu(&mut self) -> bool {
        match self.menu_selection {
            MENU_KEY_BINDINGS => {
                self.open_help();
                false
            }
            MENU_ABOUT => {
                self.open_about();
                false
            }
            MENU_EXIT => true,
            _ => false,
        }
    }

    /// Keys while the menu is up. `None` means "not handled, let the global
    /// resolver see it" — that is how q and Ctrl+C still quit. `Some(quit)`
    /// reports a handled event, or an Exit selection.
    fn handle_menu_key(&mut self, key: &KeyEvent) -> Option<bool> {
        if key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return None;
        }
        let pressed = key.kind != KeyEventKind::Repeat;
        match key.code {
            KeyCode::Esc if pressed => {
                self.close_overlay();
                Some(false)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_menu(-1);
                Some(false)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_menu(1);
                Some(false)
            }
            KeyCode::Enter if pressed => Some(self.activate_menu()),
            // A modal menu swallows everything else, so h/j/k/l cannot move
            // the model behind it and no other key can stack another panel.
            // Esc and Enter repeats are swallowed too: holding either must not
            // close and re-open, or activate an item twice.
            _ => Some(false),
        }
    }

    /// Keys while Help or About is up. Both are modal; Esc or Enter closes
    /// the panel back to the menu, q and Ctrl+C fall through to the global
    /// quit, and everything else is swallowed.
    fn handle_panel_key(&mut self, key: &KeyEvent) -> Option<bool> {
        if key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Enter if key.kind != KeyEventKind::Repeat => {
                self.close_overlay();
                Some(false)
            }
            _ => Some(false),
        }
    }

    /// Close the current overlay and pick the right place to land. A panel
    /// opened from the menu goes back to the menu with its item selected;
    /// one opened directly goes back to the bare canvas.
    fn close_overlay(&mut self) {
        self.overlay = match self.overlay {
            Overlay::Help => {
                self.menu_selection = MENU_KEY_BINDINGS;
                Overlay::Menu
            }
            Overlay::About => {
                self.menu_selection = MENU_ABOUT;
                Overlay::Menu
            }
            Overlay::Menu | Overlay::None => Overlay::None,
        };
        self.dirty = true;
    }

    /// Open the key reference from the menu. It is modal: Esc/Enter goes back
    /// to the hub, and the model does not react behind the panel.
    fn open_help(&mut self) {
        self.held.clear();
        self.overlay = Overlay::Help;
        self.dirty = true;
    }

    /// Open the About panel. It is always reached through the menu, so it
    /// returns there on Esc/Enter.
    fn open_about(&mut self) {
        self.held.clear();
        self.overlay = Overlay::About;
        self.dirty = true;
    }

    /// Handle one terminal input event. Returns true when the loop must break (quit).
    fn handle_input(&mut self, ev: Event) -> bool {
        // Key and focus events are the only input handled: mouse capture is
        // OFF (native terminal drag-selection works), so the app never
        // receives mouse events; Event::Resize is handled by the loop (it
        // needs the Engine).
        // Focus loss: the key-up of a key held across an alt-tab reaches the
        // other window, so no Release ever arrives — drop the holds here.
        if let Event::FocusLost = ev {
            if !self.held.is_empty() {
                self.held.clear();
                self.dirty = true;
            }
            return false;
        }
        let Event::Key(key) = ev else {
            return false;
        };
        // A Release is direct evidence that the terminal reports key-up: from
        // here on a press can start a continuous hold. It is matched by
        // IDENTITY, because a Release may carry different modifiers than the
        // Press (release Shift before `h` and SHIFT is gone). A Release is
        // handled before the modal branches, so no overlay can swallow the
        // evidence that key-up works.
        if key.kind == KeyEventKind::Release {
            self.release_seen = true;
            let (hold_id, _) = canonical_key(key.code, key.modifiers);
            self.hold_key_up(hold_id);
            return false;
        }
        // The Esc menu is modal too; q/Ctrl+C fall through.
        if matches!(self.overlay, Overlay::Menu)
            && let Some(quit) = self.handle_menu_key(&key)
        {
            return quit;
        }
        // Help and About share the same modal close/quit rules.
        if matches!(self.overlay, Overlay::Help | Overlay::About)
            && let Some(quit) = self.handle_panel_key(&key)
        {
            return quit;
        }
        // One event -> one (identity, action). Unbound keys stop here.
        let Some((hold_id, action)) = resolve_key_event(key.code, key.modifiers) else {
            return false;
        };
        match action {
            // Quit applies to Press and Repeat alike (holding `q` quits now).
            Action::Quit => return true,
            Action::Motion(motion) => {
                if self.release_seen {
                    match key.kind {
                        // Continuous motion is clocked by the frame loop.
                        KeyEventKind::Repeat => {}
                        _ => {
                            self.apply_tap(motion);
                            self.held.insert(hold_id, motion);
                            self.dirty = true;
                        }
                    }
                } else {
                    // No key-up reporting: each press and each auto-repeat is
                    // one finite step. A tap can therefore never keep drifting
                    // after the terminal goes quiet, and silence ends it
                    // immediately rather than after a one-second timeout.
                    self.apply_tap(motion);
                }
            }
            // One-shot actions fire once per tap, never at the repeat rate.
            _ if key.kind == KeyEventKind::Repeat => {}
            Action::Menu => self.open_menu(),
            Action::Spin => {
                self.auto_spin = !self.auto_spin;
                self.dirty = true;
            }
            Action::Axes => {
                self.show_axes = !self.show_axes;
                self.dirty = true;
            }
            // Reset: the file's own framing (rotation, pan and distance).
            Action::Reset => {
                self.view.reset_with_extent(self.extent);
                self.dirty = true;
            }
        }
        false
    }

    /// Apply continuous motion to every held key each frame (dt-scaled).
    /// Release (kitty protocol) and FocusLost are the only stops: a terminal
    /// that reports key-up is trusted, and a terminal that does not never
    /// creates a hold in the first place.
    fn update_held(&mut self, dt: f64) {
        if self.held.is_empty() {
            return;
        }
        let speed = self.translation_speed();
        for motion in self.held.values() {
            continuous_step(&mut self.view, *motion, speed, dt);
        }
        self.dirty = true;
    }
}

/// L2 engine state: the retained-mode screen and the rasterizer.
struct Engine {
    screen: render::Screen,
    raster: render::Rasterizer,
    /// Scratch buffer for the overlay panel. `None` until a panel is opened
    /// (or after a resize), so a viewer that never opens one never allocates
    /// it.
    hud_buf: Option<Buffer>,
}

impl Engine {
    fn new(w: usize, h: usize) -> Self {
        let mut engine = Engine {
            screen: render::Screen::new(w, h),
            raster: render::Rasterizer::new(),
            hud_buf: None,
        };
        // No Resize event precedes the first frame, so the opening frame needs
        // a canvas already in place.
        engine.size_raster_to_canvas();
        engine
    }

    /// Point the rasterizer at the canvas of the screen's current size.
    ///
    /// The screen is the authority, so this is safe to call after any change
    /// to it: the rasterizer's grid always matches the region `compose_frame`
    /// renders through.
    fn size_raster_to_canvas(&mut self) {
        let (w, h) = self.screen.size();
        let (_, _, cw, ch) = model_canvas_rect(w, h);
        self.raster.resize(cw, ch);
    }

    /// Terminal resize: reallocate the screen, drop the HUD buffer (it was
    /// sized for the old screen) and carry the rasterizer's canvas along.
    /// The `Resize` event is the only thing that changes the canvas size, so
    /// this is the only place a frame's grid is ever allocated.
    fn resize(&mut self, w: usize, h: usize) {
        self.screen.resize(w, h);
        self.hud_buf = None;
        self.size_raster_to_canvas();
    }
}

/// The only crossterm event reader in the process (blocks on the tty).
fn spawn_input_thread(tx: Sender<Event>) {
    let _ = thread::Builder::new()
        .name("wireforge-input".into())
        .spawn(move || {
            while let Ok(ev) = event::read() {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
}

/// Fallback resize detection (timer-driven): resize and mark dirty when the terminal size no longer matches the screen.
fn check_resize(app: &mut App, engine: &mut Engine, timers: &mut TimerScheduler) {
    if let Ok((cols, rows)) = crossterm::terminal::size() {
        let (w, h) = engine.screen.size();
        if cols as usize != w || rows as usize != h {
            engine.resize(cols as usize, rows as usize);
            // A resize is a visual change: the dirty flag makes the idle
            // branch repaint right after this timer fire.
            app.dirty = true;
        }
    }
    timers.schedule(TimerId::ResizeCheck, Instant::now() + RESIZE_CHECK_INTERVAL);
}

/// Fire every timer due at `now` through its dedicated handler. Called from
/// both loop modes — and after each idle event, so a steady event stream can
/// never postpone a due timer.
fn fire_due_timers(app: &mut App, engine: &mut Engine, timers: &mut TimerScheduler, now: Instant) {
    for id in timers.fire_due(now) {
        match id {
            TimerId::ResizeCheck => check_resize(app, engine, timers),
        }
    }
}

/// Copy the rectangle `r` of the HUD buffer into the screen (char + fg + bg).
///
/// The rect, not the row, is the unit: the help and menu panels are centred
/// over the canvas, so only the columns they actually cover may overwrite the
/// model. The HUD is built with ordinary ratatui widgets and `Color` values;
/// this is where those colours become palette slots.
fn blit_hud_rect(screen: &mut render::Screen, hud: &Buffer, r: Rect) {
    let (w, h) = screen.size();
    let (w16, h16) = (w as u16, h as u16);
    let y_end = r.y.saturating_add(r.height).min(h16);
    let x_end = r.x.saturating_add(r.width).min(w16);
    for y in r.y..y_end {
        for x in r.x..x_end {
            let cell = &hud[(x, y)];
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            screen.set(
                x as usize,
                y as usize,
                ch,
                render::ink_idx(cell.fg),
                render::ink_idx(cell.bg),
            );
        }
    }
}

/// The model canvas on a screen of `w`x`h` cells, as `(x, y, w, h)`: the last
/// row is the bar, so the canvas is every row above it. Height 0 when the
/// terminal is too short to hold the bar.
///
/// The one place the canvas geometry is spelled out: the terminal Resize path
/// sizes the rasterizer with it and `compose_frame` renders with it, so the
/// grid the model is painted into and the region it is painted through cannot
/// drift apart.
fn model_canvas_rect(w: usize, h: usize) -> (usize, usize, usize, usize) {
    (0, 0, w, h.saturating_sub(1))
}

/// Compose one frame into the engine's retained screen: the model canvas, the
/// overlay panel over it (in its own rect only) and the bar. Presenting is
/// `render_frame`'s job; splitting the two is what
/// lets the frame tests read composition off `engine.screen`.
///
/// The rasterizer's canvas is NOT sized here: it follows the screen in
/// `Engine::resize`, so a frame costs no allocation.
fn compose_frame(app: &mut App, engine: &mut Engine) {
    let (w, h) = engine.screen.size();
    if w == 0 || h == 0 {
        return;
    }
    let w16 = w as u16;
    let h16 = h as u16;
    let (canvas_x, canvas_top, canvas_w, canvas_h) = model_canvas_rect(w, h);
    let canvas_top = canvas_top as u16;
    let canvas_h = canvas_h as u16;
    let footer_top: u16 = h16.saturating_sub(1);

    // The panel's rect on screen, when it is up. The model is rasterized
    // first and the panel's rows are blitted over exactly this rect, so the
    // panel is opaque where it covers and invisible everywhere else.
    let mut panel = None;
    if canvas_h > 0
        && let Some((lines, title)) = overlay_panel(app)
    {
        let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
        // Pick the chrome that lets the content fit: shrink padding, then
        // drop borders, until the panel's chrome-plus-content width is no
        // wider than the terminal. The rightmost cell of the overlay then
        // sits inside the screen instead of clipping at the panel edge.
        let chrome = panel_chrome(widest, w16);
        let panel_w = chrome.panel_width(widest, w16);
        let panel_h = (lines.len() as u16 + chrome.height()).min(canvas_h);
        let mut area = Rect {
            x: 0,
            y: canvas_top,
            width: panel_w,
            height: panel_h,
        };
        area.x = (w16.saturating_sub(area.width)) / 2;
        area.y = canvas_top + (canvas_h.saturating_sub(area.height)) / 2;
        let pad = chrome.padding();
        // The panel paints no ground of its own: its cells carry the
        // terminal's own background, the one the transparent canvas shows, so
        // the two read as one surface. Occlusion is the cells themselves —
        // every one of them, blanks included, overwrites the model.
        let block = Block::default()
            .borders(chrome.borders())
            .border_type(BorderType::Rounded)
            .padding(Padding::new(pad, pad, 0, 0))
            .style(Style::default().fg(Ink::Foreground.color()))
            .border_style(Style::default().fg(Ink::Border.color()))
            .title(Line::from(Span::styled(
                title,
                Style::default().fg(Ink::Foreground.color()),
            )));
        let inner = block.inner(area);

        let needs_realloc = engine
            .hud_buf
            .as_ref()
            .is_none_or(|b| b.area.width != w16 || b.area.height != h16);
        if needs_realloc {
            engine.hud_buf = Some(Buffer::empty(Rect::new(0, 0, w16, h16)));
        } else if let Some(buf) = engine.hud_buf.as_mut() {
            buf.reset();
        }
        let hud = engine.hud_buf.as_mut().unwrap();
        block.render(area, hud);
        for (i, line) in lines.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            Paragraph::new(line.clone())
                .render(Rect::new(inner.x, inner.y + i as u16, inner.width, 1), hud);
        }
        panel = Some((area, engine.hud_buf.as_ref().unwrap()));
    }

    // The canvas is transparent: the terminal's own background shows behind
    // the model.
    if canvas_h > 0 {
        // Model canvas: rasterize into the screen directly. It draws under
        // the panel too — the panel only claims the cells it covers.
        engine.raster.render(
            &app.current,
            &app.view,
            (canvas_x, canvas_top as usize, canvas_w, canvas_h as usize),
            app.show_axes,
            app.extent,
            &mut engine.screen,
        );
    }

    // The panel, if any, paints over the model in exactly its own rectangle.
    if let Some((area, hud)) = panel {
        blit_hud_rect(&mut engine.screen, hud, area);
    }

    // The bar always paints last, over whatever the canvas left behind.
    paint_cells(&mut engine.screen, footer_top as usize, &bar_cells(app, w));
}

/// Render the current state into the terminal: compose a frame, then present
/// it (packed-cell diff + one batched write).
fn render_frame(
    app: &mut App,
    engine: &mut Engine,
    stdout: &mut io::Stdout,
) -> Result<(), Box<dyn Error>> {
    compose_frame(app, engine);
    engine.screen.present(stdout)?;
    Ok(())
}

/// Undo everything `main` did to the terminal: the keyboard-enhancement flags
/// (they are sticky — without this the shell would keep receiving escape
/// codes for modified keys), focus reporting, cursor, alternate screen and
/// raw mode. Idempotent and a no-op unless raw mode was enabled, so the
/// guard, the panic hook and the normal exit path can all call it.
fn restore_terminal() {
    if !crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
        return;
    }
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        PopKeyboardEnhancementFlags,
        DisableFocusChange,
        Show,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// RAII terminal cleanup. `main` creates it right after `enable_raw_mode`,
/// so every later exit path (normal return, `?` error, panic) restores the
/// terminal instead of leaving a half-configured screen behind.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Restore first, then report the panic: the message would otherwise be
    // printed inside the alternate screen and the sticky flags would survive.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
    let args = Args::parse();
    // Validate `--vfov` BEFORE any terminal setup: a bad value must fail
    // fast on a sane screen, never inside raw mode. The valid range is
    // (0, MAX_VFOV]; 0 would divide by zero and > MAX_VFOV makes the
    // projection degenerate.
    if !(args.vfov > 0.0 && args.vfov <= MAX_VFOV) {
        eprintln!(
            "--vfov must be > 0 and <= {MAX_VFOV} radians, got {}",
            args.vfov
        );
        std::process::exit(1);
    }
    // FILE has no default value: what happens without it depends on stdin.
    //
    // * stdin is NOT a terminal: it is a stream (`cat m.wrfm | wireforge`),
    //   so read the model from there — the same thing `wireforge -` asks for.
    // * stdin IS a terminal: there is nothing to read and nothing to open,
    //   so start with an empty canvas. The viewer comes up blank and stays
    //   interactive (a model is only read at start-up; run wireforge with a
    //   path to view one).
    //
    // A terminal check rather than `None == blank` outright: that keeps
    // `wireforge < /dev/null` (a script, an editor, a null device) failing
    // loudly instead of parking an invisible TUI on a blank screen. Use `-`
    // for an explicit stdin read.
    let blank = args.file.is_none() && io::stdin().is_terminal();
    let target_file = if blank {
        // No FILE and no stream: nothing to load. `target_file` is only read
        // by the branches below, which this flag rules out.
        PathBuf::new()
    } else {
        // FILE, or `-` for the stream a bare invocation implies when stdin is
        // a pipe. A path is never guessed here.
        args.file.unwrap_or_else(|| PathBuf::from("-"))
    };

    // Regular files are probed through PROBE_BYTES and loaded from the path;
    // `-`/FIFO read the whole stream once, at start-up.
    let is_stdin = target_file == Path::new("-");
    let is_fifo = !is_stdin && !blank && is_fifo_path(&target_file);
    let is_stream = is_stdin || is_fifo;
    // Statusline label override: the FIFO's full path (its model name is only
    // the file stem). stdin and regular files show their model name.
    let strip_label: Option<String> = if is_fifo {
        Some(target_file.display().to_string())
    } else {
        None
    };

    // When stdin is a pipe the keyboard comes from the controlling terminal;
    // fail fast (before consuming the model) with a clear message if there
    // is none — never a cryptic ENXIO from raw-mode setup.
    if let Err(e) = ensure_keyboard_terminal() {
        eprintln!("{e}");
        std::process::exit(1);
    }

    let (current_model, model_name) = if blank {
        // No FILE on a terminal: nothing to read, nothing to open. The
        // viewer starts on an empty canvas — the bar's pill says
        // "no file" — and every key keeps working; there is no model to
        // rotate.
        (Model::default(), "no file".to_string())
    } else if is_stream {
        // Stream path: read all of stdin (or the FIFO) once, probe the
        // whole BUFFER (not the path) and parse it.
        let mut buf = String::new();
        if is_stdin {
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("cannot read stdin: {e}"))?;
        } else {
            std::fs::File::open(&target_file)
                .map_err(|e| format!("cannot open '{}': {e}", target_file.display()))?
                .read_to_string(&mut buf)
                .map_err(|e| format!("cannot read '{}': {e}", target_file.display()))?;
        }
        let name = if is_stdin {
            "stdin".to_string()
        } else {
            target_file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("fifo")
                .to_string()
        };
        let label = if is_stdin {
            "-".to_string()
        } else {
            target_file.display().to_string()
        };
        load_model_from_text(&name, &buf).unwrap_or_else(|e| {
            eprintln!("Failed to load '{label}': {e}");
            std::process::exit(1);
        })
    } else {
        load_model(&target_file).unwrap_or_else(|e| {
            eprintln!("Failed to load '{}': {}", target_file.display(), e);
            std::process::exit(1);
        })
    };

    // The event queue: one channel, one blocking consumer — the input thread
    // is the only crossterm reader, and it pushes events here.
    let (tx, rx) = mpsc::channel::<Event>();

    enable_raw_mode()?;
    // From here on every exit path (return value, `?` error or panic) must
    // hand back a usable terminal: the enhancement flags are sticky and would
    // otherwise leak into the shell. Dropping the guard runs `restore_terminal`.
    let _terminal = TerminalGuard;
    let mut stdout = io::stdout();
    // Mouse capture stays OFF (native drag-selection) and the cursor is
    // hidden for the session (the L2 direct-write present does not hide it).
    execute!(stdout, EnterAlternateScreen, Hide)?;
    // Enable the Kitty keyboard protocol for Press/Repeat/Release key events,
    // plus alternate key codes so a shifted chord arrives as the glyph the
    // active layout types (`Shift+0` -> `)`) rather than a layout-dependent
    // base key — that is what keeps `?` / `_` / `+` / `)` and the strict Shift
    // rules working on non-US layouts. Keyboard enhancement (kitty protocol)
    // is optional: terminals that don't support it (e.g. the legacy Windows
    // console API) run without it, and legacy terminals simply ignore the flag.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                | KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        )
    );
    // Focus events: the key-up of a key held across an alt-tab goes to the
    // other window, so the app drops its holds itself on FocusLost.
    let _ = execute!(stdout, EnableFocusChange);

    // Input thread: block in the kernel on the tty, push events through the
    // channel. Started after raw mode so the tty is in the expected state.
    spawn_input_thread(tx.clone());

    // L2 engine + viewer state.
    let (cols, rows) = crossterm::terminal::size()?;
    let mut app = App::new(current_model, model_name, strip_label, args.vfov);
    let mut engine = Engine::new(cols as usize, rows as usize);
    let mut timers = TimerScheduler::new();
    let mut last = Instant::now();

    // Initial frame: the previous screen is all-space, so the first present
    // writes every non-space cell (same first-frame semantics as before).
    render_frame(&mut app, &mut engine, &mut stdout)?;
    app.dirty = false;
    timers.schedule(TimerId::ResizeCheck, Instant::now() + RESIZE_CHECK_INTERVAL);

    'main: loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f64().min(0.1);
        last = now;

        // The two SDL-style loop modes: animating = drain +
        // update + render as fast as possible, no waiting; idle = block in
        // the channel until the earliest timer or an event.
        let animating = app.auto_spin || !app.held.is_empty();

        if animating {
            // Drain the event backlog (no waiting, no cap).
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    Event::Resize(cols, rows) => {
                        engine.resize(cols as usize, rows as usize);
                        app.dirty = true;
                    }
                    ev => {
                        if app.handle_input(ev) {
                            break 'main;
                        }
                    }
                }
            }
            // Timers can fire mid-animation too.
            fire_due_timers(&mut app, &mut engine, &mut timers, now);
            // One frame of smooth motion for every held key.
            app.update_held(dt);
            if app.auto_spin {
                // Space auto-spin: rotate the model around its own (local) Y
                // axis (view::ViewState::spin_local) — a globe turning in
                // place, however it is pitched/rolled. The sign keeps the
                // spin agreeing with plain ← (the viewer's left).
                app.view.spin_local(-SPIN_RATE * dt);
                app.view.normalize();
            }
            render_frame(&mut app, &mut engine, &mut stdout)?;
        } else {
            // Idle: block until the earliest pending timer or an event —
            // the thread is parked in the kernel (0% CPU).
            let wait = timers
                .earliest()
                .map(|d| d.saturating_duration_since(Instant::now()));
            let msg: Result<Event, mpsc::RecvTimeoutError> = match wait {
                Some(t) => rx.recv_timeout(t),
                None => match rx.recv() {
                    Ok(e) => Ok(e),
                    Err(_) => Err(mpsc::RecvTimeoutError::Disconnected),
                },
            };
            match msg {
                Ok(ev) => {
                    match ev {
                        Event::Resize(cols, rows) => {
                            engine.resize(cols as usize, rows as usize);
                            app.dirty = true;
                        }
                        ev => {
                            if app.handle_input(ev) {
                                break 'main;
                            }
                        }
                    }
                    // Due timers also advance while events keep arriving, so a
                    // steady stream cannot starve them until the queue drains.
                    fire_due_timers(&mut app, &mut engine, &mut timers, now);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // The kernel woke us exactly at the earliest deadline.
                    fire_due_timers(&mut app, &mut engine, &mut timers, now);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break 'main,
            }
            // Advance any continuous hold (kitty key-up reporting only).
            app.update_held(dt);
            // Redraw only when something actually changed (dirty-flag).
            if app.dirty {
                render_frame(&mut app, &mut engine, &mut stdout)?;
                app.dirty = false;
            }
        }
    }

    // The TerminalGuard created after enable_raw_mode restores the terminal
    // (flags, focus reporting, cursor, screen, raw mode) when this returns.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The speed constants are dyadic fractions (exact in binary).
    #[test]
    fn speed_constants_are_dyadic_and_exact() {
        assert_eq!(ROT_RATE * 128.0, 169.0);
        assert_eq!(SPIN_RATE * 256.0, 169.0);
    }

    /// Write `content` to a uniquely named temp file and return its path.
    fn temp_file(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wrfm-main-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    /// Write `content` to a uniquely named temp `.wrfm` file.
    fn temp_wrfm(name: &str, content: &str) -> PathBuf {
        temp_file(&format!("{name}.wrfm"), content)
    }

    // --- Content-first open detection ---

    #[test]
    fn load_model_accepts_valid_wrfm() {
        // A valid v2 file: `wrfm 2` magic, `vertices <N> edges <M>` counts
        // header, then `v` / `e` lines.
        let p = temp_wrfm(
            "valid",
            "wrfm 2\nvertices 2   edges 1\n\nv 0 0 0\nv 1 1 1\ne 0 1\n",
        );
        let (mode, name) = load_model(&p).expect("valid wrfm must load");
        let m = mode;
        assert_eq!(m.vertices.len(), 2);
        assert_eq!(m.edges.len(), 1);
        assert_eq!(
            name,
            p.file_stem().unwrap().to_str().unwrap(),
            "model name comes from the file stem"
        );
    }

    #[test]
    fn open_detection_txt_with_wrfm_content_opens() {
        // A `.txt` holding v2 wrfm content must open as wrfm — the extension
        // is only a hint, the content is authoritative.
        let p = temp_file(
            "model.txt",
            "wrfm 2\nvertices 2   edges 1\n\nv 0 0 0\nv 1 1 1\ne 0 1\n",
        );
        let (mode, name) = load_model(&p).expect("wrfm content in a .txt must open");
        let m = mode;
        assert_eq!(m.vertices.len(), 2);
        assert_eq!(m.edges.len(), 1);
        assert_eq!(name, "model");
    }

    #[test]
    fn open_detection_wrfm_with_garbage_is_unrecognized() {
        // Garbage is "unrecognized" (never an Ok empty model); a file
        // starting with `wrfm 2` is a parse (load) error instead.
        let p = temp_wrfm("garbage", "this is not a wireframe\nno markers here\n");
        let err = load_model(&p).expect_err("garbage must fail to load");
        assert!(
            err.contains("unrecognized"),
            "error should say 'unrecognized': {err}"
        );

        // Magic present, garbage after it: routed to the wrfm parser, which
        // must fail on the missing counts header — never "unrecognized".
        let p2 = temp_wrfm("magic-garbage", "wrfm 2\nthis is garbage after the magic\n");
        let err2 = load_model(&p2).expect_err("magic+garbage must fail to load");
        assert!(
            !err2.contains("unrecognized"),
            "a wrfm magic line must route to the wrfm parser: {err2}"
        );
    }

    #[test]
    fn open_detection_obj_content_points_at_the_converter() {
        // Real OBJ content: the probe points at `wrfm convert` and loading
        // fails with the same hint — never a silent empty model.
        let p = temp_file("cube.obj", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
        let err = read_probed_file(&p).unwrap_err();
        assert!(err.contains("wrfm convert"), "hint: {err}");
        let err = load_model(&p).expect_err("obj file must not load");
        assert!(err.contains("wrfm convert"), "error: {err}");
    }

    #[test]
    fn open_detection_vertex_only_points_at_the_converter() {
        // A file with only `v ` lines (no `e `) is an OBJ point cloud:
        // v-only is an obj marker, not a wrfm without the magic.
        let p = temp_wrfm("vertex-only", "v 0 0 0\nv 1 1 1\n");
        let err = read_probed_file(&p).unwrap_err();
        assert!(err.contains("wrfm convert"), "hint: {err}");
    }

    #[test]
    fn wrfm_style_without_magic_is_unrecognized() {
        // The v2 breaking change: a file with wrfm-style
        // `v`/`e` lines but NO `wrfm <version>` magic is NOT wrfm and NOT
        // obj — it is "unrecognized".
        let v_e = temp_wrfm("v-e", "v 0 0 0\nv 1 1 1\ne 0 1\n");
        assert!(
            read_probed_file(&v_e).is_err(),
            "v/e without magic must not probe as wrfm"
        );
        let err = load_model(&v_e).expect_err("v/e without magic must fail");
        assert!(err.contains("unrecognized"), "error: {err}");

        // Edge-only or marker-less content is likewise never wrfm.
        let edge_only = temp_wrfm("edge-only", "e 0 1\n");
        assert!(
            read_probed_file(&edge_only).is_err(),
            "an edge-only file is not wrfm by the detection contract"
        );
        let comment_only = temp_wrfm("comment-only", "# just a comment\n");
        assert!(read_probed_file(&comment_only).is_err());
    }

    #[test]
    fn wrfm_magic_beats_obj_markers() {
        // The magic test comes FIRST: a file starting with `wrfm 2` is
        // wrfm even if later lines look obj-ish (`f` is an unknown directive
        // that lenient parsing skips).
        let p = temp_wrfm(
            "magic-obj",
            "wrfm 2\nvertices 2   edges 1\n\nv 0 0 0\nv 1 0 0\ne 0 1\nf 1 2 3\n",
        );
        read_probed_file(&p).expect("magic wins");
        let (mode, _) = load_model(&p).expect("magic must win over obj markers");
        let m = mode;
        assert_eq!(m.vertices.len(), 2);
        assert_eq!(m.edges.len(), 1);
    }

    #[test]
    fn empty_file_is_unrecognized() {
        // An empty file has no first line -> no magic -> unrecognized
        // — never an Ok empty model.
        let p = temp_wrfm("empty", "");
        assert!(read_probed_file(&p).is_err());
        let err = load_model(&p).expect_err("empty file must fail to load");
        assert!(
            err.contains("unrecognized"),
            "error should say 'unrecognized': {err}"
        );
    }

    #[test]
    fn load_model_rejects_half_written_file() {
        // Truncated v2 .wrfm still carries magic + `v`/`e` markers: the
        // parser fails and the caller keeps the last good model.
        let p = temp_wrfm(
            "half",
            "wrfm 2\nvertices 2   edges 1\n\nv 0 0 0\nv 1 1\ne 0 1\n",
        );
        assert!(load_model(&p).is_err());
    }

    #[test]
    fn load_model_rejects_bad_edge() {
        // An edge line missing its second index must fail to parse.
        let p = temp_wrfm(
            "bad-edge",
            "wrfm 2\nvertices 2   edges 1\n\nv 0 0 0\nv 1 1 1\ne 0",
        );
        assert!(load_model(&p).is_err());
    }

    #[test]
    fn load_model_error_reports_line() {
        // Surface the parser's structured error (line number + offending
        // line); magic routes content to the wrfm parser.
        let p = temp_wrfm(
            "bad-num",
            "wrfm 2\nvertices 1   edges 1\n\nv 1.0 2.0 abc\ne 0 0\n",
        );
        let err = load_model(&p).expect_err("bad-num must fail to load");
        assert!(err.contains("line"), "error should mention the line: {err}");
        assert!(
            err.contains("v 1.0 2.0 abc"),
            "error should carry the offending source line: {err}"
        );
    }

    #[test]
    fn load_model_rejects_deleted_file() {
        let p = temp_wrfm("gone", "wrfm 2\nvertices 1   edges 1\n\nv 0 0 0\ne 0 0\n");
        fs::remove_file(&p).unwrap();
        assert!(load_model(&p).is_err(), "a deleted file must fail to load");
    }

    // --- the bar / telemetry ---

    fn bar_text(cells: &[Cell]) -> String {
        cells.iter().map(|(ch, _, _)| *ch).collect()
    }

    #[test]
    fn the_bar_names_the_model_and_reports_semantic_camera_values() {
        // The model's name leads the bar, and the readout speaks the
        // viewer's units: degrees, a zoom multiple of the start-up framing,
        // and pan in model units. A raw camera distance in model units is a
        // number nobody can act on.
        let app = App::new(
            Model::default(),
            "cube.wrfm".to_string(),
            None,
            DEFAULT_VFOV,
        );
        let text = bar_text(&bar_cells(&app, 100));
        assert!(text.starts_with(" cube.wrfm "), "pill leads: {text:?}");
        for label in ["yaw", "pitch", "roll", "zoom", "pan"] {
            assert!(text.contains(label), "{label} missing from {text:?}");
        }
        assert!(text.contains('°'), "angles are degrees: {text:?}");
        assert!(text.contains('×'), "zoom is a multiple: {text:?}");
        assert!(
            text.contains("1.00×"),
            "a fresh view is exactly the fit framing: {text:?}"
        );
        assert!(
            !text.contains("distance="),
            "the raw camera distance is not shown: {text:?}"
        );
    }

    #[test]
    fn bar_reports_angles_in_degrees() {
        let mut app = App::new(Model::default(), "cube".to_string(), None, DEFAULT_VFOV);
        app.view.yaw = std::f64::consts::PI;
        app.view.pitch = -std::f64::consts::FRAC_PI_2;
        app.view.roll = 0.5; // 28.6° -> 29°
        app.view.normalize();
        let text = bar_text(&bar_cells(&app, 100));
        assert!(text.contains("180°"), "text: {text:?}");
        assert!(text.contains("-90°"), "text: {text:?}");
        assert!(text.contains("29°"), "text: {text:?}");
    }

    #[test]
    fn bar_zoom_is_relative_to_the_start_fit() {
        let mut app = App::new(Model::default(), "cube".to_string(), None, DEFAULT_VFOV);
        app.view.dist = app.fit_dist / 2.0;
        let text = bar_text(&bar_cells(&app, 100));
        assert!(text.contains("2.00×"), "text: {text:?}");
    }

    #[test]
    fn bar_sheds_the_lamps_before_the_telemetry() {
        // The lamps are the first thing to go on a narrow row; the readout
        // outlives them and only then sheds from its right end, pan first. The
        // name keeps at most half the row, and a cut name is marked.
        let app = app_with("cube");
        // Roomy: both lamps and the whole readout fit.
        let text = bar_text(&bar_cells(&app, 140));
        assert!(text.contains("● SPIN"), "text: {text:?}");
        assert!(text.contains("● AXES"), "text: {text:?}");
        assert!(text.contains("pan"), "text: {text:?}");
        // Narrower: the lamps are gone, the readout untouched.
        let text = bar_text(&bar_cells(&app, 90));
        assert!(
            !text.contains("SPIN") && !text.contains("AXES"),
            "the lamps shed first: {text:?}"
        );
        assert!(
            text.contains("pan"),
            "the readout outlives the lamps: {text:?}"
        );

        // A long name and a narrow row: the readout sheds from its right end.
        let mut app = app_with("cube");
        app.strip_label = Some("/home/someone/very/deep/project/tree/preview.fifo".to_string());
        let text = bar_text(&bar_cells(&app, 70));
        assert!(text.contains("yaw"), "yaw survives longest: {text:?}");
        assert!(!text.contains("pan"), "pan sheds first: {text:?}");
        let tiny = bar_text(&bar_cells(&app, 12));
        assert!(tiny.contains('…'), "a cut name is marked: {tiny:?}");
    }

    // ---------- keymap: BINDINGS <-> resolve_key_event ----------

    #[test]
    fn keymap_resolves_the_documented_chords() {
        // Each row is an event as a terminal reports it, and what the viewer
        // must do with it. This is behaviour, not a second copy of the table:
        // the table is what actually resolves it, and the rows are the cases a
        // reader would expect.
        use KeyCode::*;
        let cases: &[(&str, KeyCode, KeyModifiers, Option<Action>)] = &[
            (
                "h rotates",
                Char('h'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::YawLeft)),
            ),
            (
                "l rotates",
                Char('l'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::YawRight)),
            ),
            (
                "k pitches up",
                Char('k'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::PitchUp)),
            ),
            (
                "j pitches down",
                Char('j'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::PitchDown)),
            ),
            (
                "d rolls",
                Char('d'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::RollMinus)),
            ),
            (
                "f rolls",
                Char('f'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::RollPlus)),
            ),
            (
                "Shift+h pans",
                Char('h'),
                KeyModifiers::SHIFT,
                Some(Action::Motion(Motion::MoveLeft)),
            ),
            (
                "Shift+H is the same chord",
                Char('H'),
                KeyModifiers::SHIFT,
                Some(Action::Motion(Motion::MoveLeft)),
            ),
            (
                "Ctrl+h yaws locally",
                Char('h'),
                KeyModifiers::CONTROL,
                Some(Action::Motion(Motion::LocalYawLeft)),
            ),
            (
                "Ctrl+Left is the same",
                Left,
                KeyModifiers::CONTROL,
                Some(Action::Motion(Motion::LocalYawLeft)),
            ),
            (
                "Ctrl+Shift+h is the same",
                Char('H'),
                KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
                Some(Action::Motion(Motion::LocalYawLeft)),
            ),
            (
                "Ctrl+l yaws the other way",
                Char('l'),
                KeyModifiers::CONTROL,
                Some(Action::Motion(Motion::LocalYawRight)),
            ),
            (
                "Ctrl+k pitches up",
                Char('k'),
                KeyModifiers::CONTROL,
                Some(Action::Motion(Motion::LocalPitchUp)),
            ),
            (
                "Ctrl+j pitches down",
                Char('j'),
                KeyModifiers::CONTROL,
                Some(Action::Motion(Motion::LocalPitchDown)),
            ),
            (
                "Ctrl+d rolls locally",
                Char('d'),
                KeyModifiers::CONTROL,
                Some(Action::Motion(Motion::LocalRollMinus)),
            ),
            (
                "Ctrl+f rolls the other way",
                Char('f'),
                KeyModifiers::CONTROL,
                Some(Action::Motion(Motion::LocalRollPlus)),
            ),
            (
                "+ zooms in",
                Char('+'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::MoveForward)),
            ),
            (
                "= is the same chord",
                Char('='),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::MoveForward)),
            ),
            (
                "- zooms out",
                Char('-'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::MoveBack)),
            ),
            (
                "_ is the same chord",
                Char('_'),
                KeyModifiers::NONE,
                Some(Action::Motion(Motion::MoveBack)),
            ),
            (
                "a is unbound; Tab toggles the axes",
                Char('a'),
                KeyModifiers::NONE,
                None,
            ),
            (
                "Tab toggles the axes",
                Tab,
                KeyModifiers::NONE,
                Some(Action::Axes),
            ),
            (
                "? is unbound; Key Bindings lives in the menu",
                Char('?'),
                KeyModifiers::NONE,
                None,
            ),
            (
                "Shift+/ is also unbound",
                Char('/'),
                KeyModifiers::SHIFT,
                None,
            ),
            (
                "r resets",
                Char('r'),
                KeyModifiers::NONE,
                Some(Action::Reset),
            ),
            (
                "Space spins",
                Char(' '),
                KeyModifiers::NONE,
                Some(Action::Spin),
            ),
            ("q quits", Char('q'), KeyModifiers::NONE, Some(Action::Quit)),
            (
                "Esc opens the menu",
                Esc,
                KeyModifiers::NONE,
                Some(Action::Menu),
            ),
            (
                "Ctrl+C quits",
                Char('c'),
                KeyModifiers::CONTROL,
                Some(Action::Quit),
            ),
            (
                "Ctrl+Shift+C is copy, not quit",
                Char('C'),
                KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
                None,
            ),
            ("Shift+q is not q", Char('q'), KeyModifiers::SHIFT, None),
            ("Shift+r is not r", Char('r'), KeyModifiers::SHIFT, None),
            ("Alt swallows the key", Char('h'), KeyModifiers::ALT, None),
            (
                "Alt swallows even Space",
                Char(' '),
                KeyModifiers::ALT,
                None,
            ),
            (
                "an unbound letter does nothing",
                Char('z'),
                KeyModifiers::NONE,
                None,
            ),
        ];
        for (what, code, mods, expected) in cases {
            assert_eq!(
                resolve_key_event(*code, *mods).map(|(_, action)| action),
                *expected,
                "{what}: {code:?}+{mods:?}"
            );
        }
    }

    #[test]
    fn bindings_cover_every_action_exactly_once() {
        // Two rows describing one action would make the overlay list it twice;
        // a row describing none would list a phantom.
        for b in BINDINGS {
            let hits = BINDINGS.iter().filter(|o| o.action == b.action).count();
            assert_eq!(hits, 1, "{:?} is documented {hits} times", b.action);
        }
    }

    #[test]
    fn chord_labels_follow_one_convention() {
        // A single spelling convention, checked mechanically: modifiers in
        // Ctrl-then-Shift order, no spaces, upper-case only when SHIFT is in
        // the chord. Anything else is a typo waiting to mislead someone.
        for b in BINDINGS {
            for c in b.chords {
                let l = c.label();
                assert!(!l.contains(' ') || l == "Space", "{l:?} has a stray space");
                assert!(
                    !l.contains("Shift+Ctrl"),
                    "{l}: modifier order is Ctrl+Shift+"
                );
                let ctrl_at = l.find("Ctrl+");
                let shift_at = l.find("Shift+");
                if let (Some(cs), Some(sc)) = (ctrl_at, shift_at) {
                    assert!(cs < sc, "{l}: Ctrl must come before Shift");
                }
                // The chord names the key, not the shifted glyph: after
                // `Shift+` a single letter is the lowercase form (`Shift+h`),
                // the same way `h` and `Shift+h` read. Named keys (`Left`,
                // `Tab`) have no glyph to case, so only single letters check.
                if let Some([c]) = l.strip_prefix("Shift+").and_then(|r| {
                    let mut it = r.chars();
                    match (it.next(), it.next()) {
                        (Some(c), None) => Some([c]),
                        _ => None,
                    }
                }) {
                    assert!(
                        c.is_lowercase() || !c.is_alphabetic(),
                        "{l}: Shift+ must be followed by the key's lowercase letter"
                    );
                }
            }
        }
    }

    // ---------- help panel layout ----------

    #[test]
    fn help_panel_is_generated_from_bindings() {
        // The page is no longer a hand-maintained second copy of the key
        // table: every binding's display keys and doc must appear on it, and
        // every group heading must be there.
        let text: String = help_lines()
            .iter()
            .map(|line| {
                let mut row: String = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect();
                row.push('\n');
                row
            })
            .collect();
        for binding in BINDINGS {
            assert!(
                text.contains(binding.keys),
                "{:?} keys missing from {text}",
                binding.doc
            );
            assert!(
                text.contains(binding.doc),
                "{:?} doc missing from {text}",
                binding.doc
            );
        }
        for group in [Group::Rotate, Group::Move, Group::View, Group::Session] {
            assert!(
                text.contains(group.title()),
                "{} heading missing",
                group.title()
            );
        }
    }

    #[test]
    fn help_keys_mention_the_primary_chord() {
        // The help column is display text, but its first key must be the chord
        // that actually fires; otherwise the page teaches a key that does
        // something else.
        for binding in BINDINGS {
            let primary = binding.chords[0].label();
            assert!(
                binding.keys.contains(&primary),
                "{}: help shows {:?}, primary chord is {primary:?}",
                binding.doc,
                binding.keys
            );
        }
    }

    #[test]
    fn help_panel_fits_eighty_columns() {
        // The old overlay was 24 lines of hand-aligned text that overflowed an
        // 80x24 terminal. The generated one must fit both common bounds.
        let lines = help_lines();
        let widest = lines.iter().map(Line::width).max().unwrap_or(0);
        assert!(widest <= 76, "help panel is {widest} cols wide (budget 76)");
        assert!(
            lines.len() <= 20,
            "help panel is {} lines tall (budget 20)",
            lines.len()
        );
    }

    #[test]
    fn panel_chrome_shrinks_with_terminal_width() {
        // The chrome (borders + padding) shrinks when the terminal narrows, so
        // the rightmost cell of a panel stays on screen rather than clipping at
        // the panel edge.
        let widest = help_lines().iter().map(Line::width).max().unwrap_or(0) as u16;
        assert_eq!(panel_chrome(widest, widest + 6), PanelChrome::Full);
        assert_eq!(panel_chrome(widest, widest + 5), PanelChrome::Padded);
        assert_eq!(panel_chrome(widest, widest + 4), PanelChrome::Padded);
        assert_eq!(panel_chrome(widest, widest + 3), PanelChrome::Bare);
        assert_eq!(panel_chrome(widest, widest + 2), PanelChrome::Bare);
        assert_eq!(panel_chrome(widest, widest + 1), PanelChrome::Inline);
    }

    #[test]
    fn panel_inner_never_exceeds_terminal() {
        // The whole point of the chrome dance: the inner area the lines are
        // rendered into must never spill past the right edge of the screen.
        let widest = help_lines().iter().map(Line::width).max().unwrap_or(0) as u16;
        for term_w in [80, 70, 65, 64, 63, 62, 61, 60, 59, 58, 50] {
            let chrome = panel_chrome(widest, term_w);
            let panel_w = chrome.panel_width(widest, term_w);
            assert!(
                panel_w <= term_w,
                "panel {panel_w} exceeds terminal {term_w}"
            );
        }
    }

    #[test]
    fn help_key_columns_are_aligned() {
        // Within each column the chord gutter is measured from the data, so
        // every doc starts at the same offset. The rows are generated; the
        // spacing under them is computed, and this is what keeps it that way.
        for groups in Group::COLUMNS {
            let column = help_column(groups);
            let widths: Vec<usize> = column
                .iter()
                .filter(|line| line.spans.len() >= 2)
                .map(|line| line.spans[0].content.chars().count())
                .collect();
            if let Some((&first, rest)) = widths.split_first() {
                assert!(
                    rest.iter().all(|w| *w == first),
                    "ragged chord column: {widths:?}"
                );
            }
        }
    }

    // --- Event-driven canvas sizing ---

    #[test]
    fn model_canvas_rect_leaves_room_for_the_bar() {
        // Rows 0..22 of a 24-row screen: the last row is the bar.
        assert_eq!(model_canvas_rect(80, 24), (0, 0, 80, 23));
        // Too short for the bar: the canvas collapses to nothing rather than
        // underflowing.
        assert_eq!(model_canvas_rect(80, 1), (0, 0, 80, 0));
        assert_eq!(model_canvas_rect(80, 0), (0, 0, 80, 0));
    }

    #[test]
    fn engine_new_sizes_the_raster_to_the_model_canvas() {
        // No Resize event precedes the first frame, so `Engine::new` must hand
        // the rasterizer a canvas itself — otherwise the opening frame
        // rasterizes into a 0x0 grid and draws nothing.
        let engine = Engine::new(80, 24);
        // Rows 0..22: the last row is the bar.
        assert_eq!(engine.raster.canvas_size(), (80, 23));
    }

    #[test]
    fn engine_resize_carries_the_raster_to_the_new_canvas() {
        // A terminal Resize event is the only thing that changes the canvas,
        // so `Engine::resize` is where the rasterizer follows it.
        let mut engine = Engine::new(80, 24);
        engine.resize(120, 40);
        assert_eq!(engine.screen.size(), (120, 40));
        assert_eq!(engine.raster.canvas_size(), (120, 39));
    }

    #[test]
    fn compose_frame_draws_inside_the_canvas_after_a_resize() {
        // The stride the rasterizer paints with and the region it is handed
        // must be the same canvas: a grid left at the old size paints outside
        // the new bounds (or panics) instead of into them.
        let mut app = App::new(Model::default(), "cube".to_string(), None, DEFAULT_VFOV);
        let mut engine = Engine::new(80, 24);
        compose_frame(&mut app, &mut engine);
        engine.resize(120, 40);
        compose_frame(&mut app, &mut engine);

        let (w, h) = engine.screen.size();
        let is_braille =
            |x: usize, y: usize| ('\u{2800}'..='\u{28ff}').contains(&engine.screen.symbol(x, y));
        let dots = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| is_braille(x, y))
            .count();
        assert!(dots > 0, "the model must still be drawn after a resize");
        for x in 0..w {
            assert!(
                !is_braille(x, h - 1),
                "the canvas must not paint over the bar: ({x},{})",
                h - 1
            );
        }
    }

    #[test]
    fn help_panel_occludes_only_the_cells_it_covers() {
        // Pressing `?` used to blank the canvas. The panel must instead claim
        // exactly its own rectangle on top of the model: whatever it covers is
        // occluded, and every cell outside it draws as if `?` were never
        // pressed — the two frames differ only inside the panel.
        let compose = |overlay: bool| {
            let mut app = App::new(Model::default(), "cube".to_string(), None, DEFAULT_VFOV);
            app.overlay = if overlay {
                Overlay::Help
            } else {
                Overlay::None
            };
            let mut engine = Engine::new(120, 40);
            compose_frame(&mut app, &mut engine);
            engine
        };
        let plain = compose(false);
        let over = compose(true);

        let is_braille = |e: &Engine, x: usize, y: usize| {
            ('\u{2800}'..='\u{28ff}').contains(&e.screen.symbol(x, y))
        };
        let braille = |e: &Engine| {
            let (w, h) = e.screen.size();
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| is_braille(e, x, y))
                .count()
        };
        assert!(braille(&plain) > 0, "the empty scene must draw its axes");

        // Find the panel by its rounded corners and measure it on screen,
        // rather than re-deriving the layout math the renderer just ran.
        let (w, h) = over.screen.size();
        let (px, py) = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .find(|&(x, y)| over.screen.symbol(x, y) == '╭')
            .expect("the help panel must be on screen");
        // The title sits on the top border, so the top edge is not a run of
        // `─`: measure to the closing corner instead.
        let mut pw = 0;
        while px + pw < w && over.screen.symbol(px + pw, py) != '╮' {
            pw += 1;
        }
        pw += 1; // the closing corner
        let mut ph = 0;
        while py + ph < h && over.screen.symbol(px, py + ph) != '╯' {
            ph += 1;
        }
        ph += 1; // the closing corner

        let mut covered = 0usize; // model cells the panel hides
        let mut leaked = 0usize; // model cells surviving under the panel
        let mut touched = 0usize; // cells outside the panel the overlay altered
        for y in 0..h {
            for x in 0..w {
                if x >= px && x < px + pw && y >= py && y < py + ph {
                    leaked += usize::from(is_braille(&over, x, y));
                    covered += usize::from(is_braille(&plain, x, y));
                } else if over.screen.cell(x, y) != plain.screen.cell(x, y) {
                    touched += 1;
                }
            }
        }
        assert!(covered > 0, "the panel must sit over part of the model");
        assert_eq!(leaked, 0, "the panel must occlude the model underneath");
        assert_eq!(
            touched, 0,
            "outside the panel the frame must be identical to the one without the overlay"
        );
    }

    // ---------- bars and overlays ----------

    #[test]
    fn the_readout_holds_its_columns_while_the_values_change() {
        // A value that grows — a minus sign, one more digit — takes the room
        // to its right: every label stays where it was, and the readout is
        // as wide as its columns are, so nothing on the row jumps. A value
        // that outgrows its column is cut, and the cut is marked.
        let label_at = |cells: &[Cell], label: &str| {
            let text = bar_text(cells);
            let byte = text
                .find(label)
                .unwrap_or_else(|| panic!("{label:?} is not on the bar: {text:?}"));
            text[..byte].chars().count()
        };
        let labels = ["yaw", "pitch", "roll", "zoom", "pan"];
        let mut app = app_with("cube");
        let before = bar_cells(&app, 140);
        let before_at: Vec<usize> = labels.iter().map(|l| label_at(&before, l)).collect();

        app.view.yaw = -90.0f64.to_radians();
        app.view.normalize();
        let after = bar_cells(&app, 140);
        let after_at: Vec<usize> = labels.iter().map(|l| label_at(&after, l)).collect();
        assert_eq!(
            before_at, after_at,
            "the labels must not move when a value grows"
        );
        assert!(
            bar_text(&after).contains("yaw -90°"),
            "the value grows in place: {:?}",
            bar_text(&after)
        );

        // A pan in model units can outgrow its column: the number is cut and
        // the cut says so.
        app.view.pan_x = 123456.0;
        app.view.pan_y = -654321.0;
        let text = bar_text(&bar_cells(&app, 140));
        assert!(text.contains('…'), "a cut value is marked: {text:?}");
    }

    fn app_with(name: &str) -> App {
        App::new(Model::default(), name.to_string(), None, DEFAULT_VFOV)
    }

    /// The ink of the dot in front of `label`, so a test can assert what a
    /// lamp is actually drawn in. The dot sits two cells before its name.
    fn lamp_ink(cells: &[Cell], label: &str) -> Ink {
        let text = bar_text(cells);
        let byte = text
            .find(label)
            .unwrap_or_else(|| panic!("{label:?} is not on the bar: {text:?}"));
        cells[text[..byte].chars().count() - 2].1
    }

    #[test]
    fn the_bar_fills_the_row_exactly() {
        // The bar is a full-width band: a cell short leaves a hole, a cell
        // long gets clipped. Absurd widths are included so the fit arithmetic
        // can never underflow.
        let app = app_with("wireforge.wrfm");
        for width in 0..=200usize {
            assert_eq!(bar_cells(&app, width).len(), width, "the bar at {width}");
        }
    }

    #[test]
    fn the_bar_paints_a_ground_across_the_whole_row() {
        // The bar is the one place Wireforge fills cells; a cell without a
        // ground would let the terminal show through mid-bar.
        let app = app_with("cube");
        for width in [1usize, 12, 40, 80, 200] {
            for (i, (ch, _, bg)) in bar_cells(&app, width).iter().enumerate() {
                assert_ne!(
                    *bg,
                    Ink::Default,
                    "cell {i} ({ch:?}) has no ground at width {width}"
                );
            }
        }
    }

    #[test]
    fn bar_leads_with_the_model_name_pill() {
        let app = app_with("cube");
        let cells = bar_cells(&app, 80);
        assert!(
            bar_text(&cells).starts_with(" cube "),
            "pill leads: {:?}",
            bar_text(&cells)
        );
        assert_eq!(cells[0].2, Ink::Accent, "pill ground");
        assert_eq!(cells[1].1, Ink::OnAccent, "pill text ink");
    }

    #[test]
    fn bar_prefers_the_stream_path_over_the_file_stem() {
        // A stream's model name is only the file stem ("stream"), so the FIFO
        // path is what tells two previews apart.
        let app = App::new(
            Model::default(),
            "stream".to_string(),
            Some("/tmp/stream.fifo".to_string()),
            DEFAULT_VFOV,
        );
        let text = bar_text(&bar_cells(&app, 100));
        assert!(text.starts_with(" /tmp/stream.fifo "), "pill: {text:?}");
    }

    #[test]
    fn bar_pill_never_takes_more_than_half_the_row() {
        let path = "/home/someone/very/deep/project/tree/preview.fifo";
        let mut app = app_with("cube");
        app.strip_label = Some(path.to_string());
        for width in [20usize, 40, 80] {
            let cells = bar_cells(&app, width);
            let pill = cells
                .iter()
                .take_while(|(_, _, bg)| *bg == Ink::Accent)
                .count();
            assert!(
                pill <= width / 2,
                "the pill takes {pill} of {width} cells: {:?}",
                bar_text(&cells)
            );
            assert!(
                bar_text(&cells).contains('…'),
                "a cut path is marked: {:?}",
                bar_text(&cells)
            );
        }
        // Roomy enough, and the path is shown whole and unmarked.
        let wide = bar_text(&bar_cells(&app, 200));
        assert!(
            wide.contains(path),
            "the whole path is shown when it fits: {wide:?}"
        );
        assert!(!wide.contains('…'), "nothing was cut: {wide:?}");
    }

    #[test]
    fn the_empty_state_is_named_on_the_bar() {
        let app = App::new(Model::default(), "no file".to_string(), None, DEFAULT_VFOV);
        let text = bar_text(&bar_cells(&app, 80));
        assert!(
            text.starts_with(" no file "),
            "the pill names the empty state: {text:?}"
        );
    }

    #[test]
    fn bar_lamps_carry_state_in_the_ink() {
        // One dot per lamp, and the dot's ink is the state: a lit lamp is the
        // accent, an unlit one is the bar's own deep ink — a dark dot on the
        // lamps' block, so it reads as unlit rather than as missing.
        let mut app = app_with("cube");
        app.auto_spin = false;
        app.show_axes = true;
        let cells = bar_cells(&app, 140);
        let text = bar_text(&cells);
        assert!(text.contains("● SPIN"), "spin is off: {text:?}");
        assert!(text.contains("● AXES"), "axes are on: {text:?}");
        assert_eq!(lamp_ink(&cells, "SPIN"), Ink::Surface);
        assert_eq!(lamp_ink(&cells, "AXES"), Ink::Accent);

        app.auto_spin = true;
        app.show_axes = false;
        let cells = bar_cells(&app, 140);
        let text = bar_text(&cells);
        assert!(text.contains("● SPIN"), "spin is on: {text:?}");
        assert!(text.contains("● AXES"), "axes are off: {text:?}");
        assert_eq!(lamp_ink(&cells, "SPIN"), Ink::Accent);
        assert_eq!(lamp_ink(&cells, "AXES"), Ink::Surface);
    }

    #[test]
    fn the_bar_carries_no_key_hints() {
        // The chords are not teased on the bar: Esc opens the menu, whose Key
        // Bindings has the full list. Nothing on the row may read as a chord.
        let app = app_with("cube");
        let text = bar_text(&bar_cells(&app, 200));
        for hint in ["hjkl", "Esc", "quit", "menu", "spin", "rotate", "Shift"] {
            assert!(
                !text.contains(hint),
                "{hint:?} leaked onto the bar: {text:?}"
            );
        }
    }

    #[test]
    fn the_bar_carries_no_glyph_but_the_powerline_separators() {
        // The blocks close in lualine's powerline wedge and seam (U+E0B0 /
        // U+E0B1) — the two glyphs that ask for a powerline font. Nothing else
        // on the bar may reach for a private-use glyph.
        let app = app_with("cube");
        let text = bar_text(&bar_cells(&app, 140));
        assert!(
            text.contains(WEDGE) && text.contains(SEAM),
            "the blocks close in their separators: {text:?}"
        );
        for ch in text.chars() {
            if ('\u{e000}'..='\u{f8ff}').contains(&ch) {
                assert!(
                    ch == WEDGE || ch == SEAM,
                    "private-use glyph {ch:?} leaked onto the bar: {text:?}"
                );
            }
        }
    }

    #[test]
    fn esc_opens_the_menu_and_esc_closes_it() {
        let mut app = app_with("cube");
        assert!(!app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
        assert_eq!(app.overlay, Overlay::Menu);
        assert!(!app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn esc_menu_navigates_wraps_and_exits() {
        let mut app = app_with("cube");
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.menu_selection, MENU_KEY_BINDINGS);

        // Down wraps at the bottom; Up wraps at the top.
        for expected in [MENU_ABOUT, MENU_EXIT, MENU_KEY_BINDINGS] {
            app.handle_input(Event::Key(KeyEvent::new(
                KeyCode::Char('j'),
                KeyModifiers::NONE,
            )));
            assert_eq!(app.menu_selection, expected);
        }
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.menu_selection, MENU_EXIT);

        // The Exit item returns "quit" from the input handler.
        assert!(app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        ))));
    }

    #[test]
    fn esc_menu_key_bindings_round_trips_through_the_menu() {
        let mut app = app_with("cube");
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        // Key Bindings is the first item.
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.overlay, Overlay::Help);
        // Esc from a panel reached through the menu goes back to the menu.
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.overlay, Overlay::Menu);
        assert_eq!(app.menu_selection, MENU_KEY_BINDINGS);
        // A second Esc closes the menu.
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn esc_menu_about_opens_and_returns() {
        let mut app = app_with("cube");
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.menu_selection, MENU_ABOUT);
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.overlay, Overlay::About);
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.overlay, Overlay::Menu);
        assert_eq!(app.menu_selection, MENU_ABOUT);
    }

    #[test]
    fn esc_menu_is_modal_but_q_still_quits() {
        let mut app = app_with("cube");
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        let yaw = app.view.yaw;
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.view.yaw, yaw, "the menu swallows motion keys");
        assert_eq!(app.overlay, Overlay::Menu);
        assert!(app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        ))));
    }

    #[test]
    fn menu_and_about_panels_render_their_content() {
        let mut app = app_with("cube");
        app.overlay = Overlay::Menu;
        app.menu_selection = MENU_ABOUT;
        let (lines, title) = overlay_panel(&app).expect("the menu is a panel");
        assert_eq!(title, " Menu (Esc to close) ");
        let text: String = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for item in MENU_ITEMS {
            assert!(text.contains(item), "{item:?} missing from {text}");
        }
        assert!(
            text.contains("▸ About"),
            "the cursor marks the selected item: {text}"
        );

        app.overlay = Overlay::About;
        let (lines, title) = overlay_panel(&app).expect("About is a panel");
        assert_eq!(title, " About (Esc to close) ");
        let text: String = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("wireforge"), "about: {text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "about: {text}");
        assert!(text.contains("MIT license"), "about: {text}");
    }

    #[test]
    fn help_is_modal_and_esc_returns_to_the_menu() {
        let mut app = app_with("cube");
        app.overlay = Overlay::Help;
        let yaw = app.view.yaw;
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.view.yaw, yaw, "help swallows motion keys");
        assert_eq!(app.overlay, Overlay::Help);

        // `?` is unbound and must not toggle the panel.
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.overlay, Overlay::Help);

        // Esc/Enter go back to the menu with Key Bindings selected.
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.overlay, Overlay::Menu);
        assert_eq!(app.menu_selection, MENU_KEY_BINDINGS);
    }

    #[test]
    fn alternate_key_glyphs_fold_into_the_same_chords() {
        use KeyCode::*;
        let bound = [
            (Char('H'), Char('h'), Action::Motion(Motion::MoveLeft)),
            (Char('_'), Char('-'), Action::Motion(Motion::MoveBack)),
            (Char('+'), Char('='), Action::Motion(Motion::MoveForward)),
        ];
        for (glyph, id, expected) in bound {
            assert_eq!(
                resolve_key_event(glyph, KeyModifiers::NONE),
                Some((id, expected)),
                "glyph {glyph:?} must fold into its listed chord"
            );
        }
        // Chords that are NOT listed stay silent in that shape as well:
        // `Shift+0` types `)` and binds nothing, `Shift+q` must not quit.
        assert_eq!(resolve_key_event(Char(')'), KeyModifiers::NONE), None);
        assert_eq!(resolve_key_event(Char('Q'), KeyModifiers::NONE), None);
    }
    #[test]
    fn app_focus_lost_drops_held_keys() {
        // The key-up of a key held across an alt-tab reaches the other
        // window, so no Release arrives: FocusLost is the only signal.
        let mut app = app_with("cube");
        app.release_seen = true;
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.held.len(), 1);
        app.dirty = false;
        assert!(!app.handle_input(Event::FocusLost));
        assert!(app.held.is_empty(), "focus loss must stop every hold");
        assert!(app.dirty, "the cleared hold must repaint");
    }

    /// The model is immutable once loaded, so its extent is measured once and
    /// reused: the axes and the auto-fit distance must all read that cached
    /// value instead of re-scanning every vertex per frame. The translation
    /// speed is also cached at construction, derived from the initial
    /// camera distance (`dist_initial / 5.0`).
    #[test]
    fn app_caches_the_model_extent_for_every_frame_use() {
        // A +-1 cube: the geometric mean of (2,2,2) is 2.
        let cube = || Model {
            vertices: vec![
                (1.0, 1.0, 1.0),
                (1.0, -1.0, -1.0),
                (-1.0, 1.0, -1.0),
                (-1.0, -1.0, 1.0),
            ],
            edges: vec![(0, 1), (0, 2), (0, 3), (1, 2), (2, 3), (3, 1)],
        };
        let mut app = App::new(cube(), "cube".to_string(), None, DEFAULT_VFOV);
        assert_eq!(app.extent, 2.0, "the extent is cached at construction");

        // The framing distance comes from that same cached extent, so it cannot
        // drift from what re-measuring the model would have produced.
        let expected = wrfm_raster::geometry::auto_dist_from_extent(app.extent, app.view.vfov);
        assert_eq!(
            app.view.dist, expected,
            "App::new must frame from the cached extent"
        );
        assert_eq!(
            app.fit_dist, expected,
            "the zoom reference is the same start-up framing"
        );

        // Translation speed: 5 s for the model centre to reach the camera
        // from the initial pose, pinned for the session.
        assert_eq!(app.translation_speed(), app.view.dist / 5.0);

        // Key `r` re-fits from the cache too, rather than a fresh bounds scan.
        app.view.dist = 1234.0;
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        assert_eq!(
            app.view.dist, expected,
            "reset must re-fit from the cached extent"
        );
    }

    #[test]
    fn app_handle_input_motion_press_starts_continuous() {
        let mut app = app_with("cube");
        app.release_seen = true;
        let yaw0 = app.view.yaw;
        // Press: one immediate step, then a hold that the frame clock drives.
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )));
        assert!(
            app.held.contains_key(&KeyCode::Right),
            "press must enter continuous state"
        );
        assert_ne!(app.view.yaw, yaw0, "a press gives immediate feedback");
        let yaw1 = app.view.yaw;
        // Auto-repeat: must not add a new entry.
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )));
        assert_eq!(
            app.held.len(),
            1,
            "auto-repeat must not accumulate held entries"
        );
        // Continuous motion is applied by update_held.
        app.update_held(0.016);
        assert_ne!(
            app.view.yaw, yaw1,
            "held key must rotate continuously immediately"
        );
    }

    /// The roll keys (`d` / `f`) roll the MODEL about the world Z axis — the
    /// same pre-multiplied world-frame rotation `hjkl` use — so they turn
    /// the frame exactly like any other rotation key. Pin the whole chain
    /// the operator sees: key event -> held motion -> composed frame.
    #[test]
    fn d_key_turns_the_composed_frame() {
        let frame = |app: &mut App| {
            let mut engine = Engine::new(60, 20);
            compose_frame(app, &mut engine);
            let (w, h) = engine.screen.size();
            let screen = &engine.screen;
            // The canvas only: rows 0..h-1. The last row is the bar, and the
            // bar prints the roll ANGLE — it changes whether or not the model
            // turns, so comparing it would let a dead renderer pass.
            (0..h - 1)
                .flat_map(move |y| (0..w).map(move |x| screen.symbol(x, y)))
                .collect::<Vec<_>>()
        };
        let mut app = App::new(
            Model {
                vertices: vec![
                    (1.0, 1.0, 1.0),
                    (1.0, -1.0, -1.0),
                    (-1.0, 1.0, -1.0),
                    (-1.0, -1.0, 1.0),
                ],
                edges: vec![(0, 1), (0, 2), (0, 3), (1, 2), (2, 3), (3, 1)],
            },
            "tetra".to_string(),
            None,
            DEFAULT_VFOV,
        );
        let before = frame(&mut app);
        assert!(
            before.iter().any(|c| ('\u{2800}'..='\u{28ff}').contains(c)),
            "the model must be on screen before its roll is checked"
        );
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        )));
        let after = frame(&mut app);
        assert_ne!(
            before, after,
            "plain d (view roll) must turn what is on screen"
        );
    }

    #[test]
    fn app_handle_input_toggles() {
        let mut app = app_with("cube");
        app.dirty = false;

        // `?` is unbound now: Key Bindings lives in the Esc menu, and the
        // key must not open a second copy of the reference.
        assert!(!app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE
        ))));
        assert_eq!(app.overlay, Overlay::None);
        assert!(!app.dirty, "an unbound key must not repaint");
        assert!(!app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::SHIFT
        ))));
        assert_eq!(app.overlay, Overlay::None);

        // Space toggles auto-spin.
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        )));
        assert!(app.auto_spin);
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        )));
        assert!(!app.auto_spin);

        // Tab toggles the axes; 'a' binds nothing.
        let before = app.show_axes;
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.show_axes, before);
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert_ne!(app.show_axes, before);

        // Esc opens the menu; a second Esc closes it. q quits.
        assert!(!app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
        assert_eq!(app.overlay, Overlay::Menu);
        assert!(!app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        ))));
    }

    #[test]
    fn legacy_press_is_one_finite_step() {
        // A terminal without key-up reporting never creates a hold: each press
        // and each auto-repeat is one step. A tap can therefore never drift
        // after the terminal goes quiet, which is what the old one-second
        // timeout used to allow.
        let mut app = app_with("cube");
        assert!(!app.release_seen, "a fresh app has not seen key-up yet");
        let yaw = app.view.yaw;
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )));
        assert!(app.held.is_empty(), "legacy presses must not leave a hold");
        assert_ne!(app.view.yaw, yaw, "the press still moves the model");
        let after = app.view.yaw;
        app.update_held(1.0);
        assert_eq!(app.view.yaw, after, "no hold means no drift");
    }

    #[test]
    fn app_key_release_removes_held_entry() {
        let mut app = app_with("cube");
        app.release_seen = true;
        let press = Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.handle_input(press);
        assert_eq!(app.held.len(), 1, "pressure holds the motion");
        let mut release = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_input(Event::Key(release));
        assert!(app.held.is_empty(), "release must stop motion immediately");
    }

    #[test]
    fn app_repeat_never_retriggers_a_toggle() {
        // The kitty protocol reports auto-repeat while a key is held; the
        // one-shot toggles must fire once per tap instead of on every repeat.
        let mut app = app_with("cube");
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        )));
        assert!(app.auto_spin, "press must toggle auto-spin");
        let mut repeat = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        app.handle_input(Event::Key(repeat));
        assert!(app.auto_spin, "auto-repeat must not re-trigger a toggle");

        // Esc opens the menu once; a held Esc must not toggle it open/closed.
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.overlay, Overlay::Menu);
        let mut repeat = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        app.handle_input(Event::Key(repeat));
        assert_eq!(
            app.overlay,
            Overlay::Menu,
            "a held Esc must not re-toggle the menu"
        );
        app.handle_input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.overlay, Overlay::None);

        // A repeat of a held motion key does not fork or restart the hold; the
        // frame clock keeps the single motion running.
        app.release_seen = true;
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )));
        let yaw = app.view.yaw;
        let mut repeat = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        app.handle_input(Event::Key(repeat));
        assert_eq!(app.held.len(), 1, "one key must not fork into two motions");
        app.update_held(0.016);
        assert_ne!(app.view.yaw, yaw, "the hold keeps moving after a repeat");
    }

    #[test]
    fn app_hold_survives_silence_until_release() {
        // Once the terminal reports key-up, Release (and FocusLost) end a
        // hold. Silence is not evidence of a released key, so there is no
        // timeout to expire a hold that is still down.
        let mut app = app_with("cube");
        app.release_seen = true;
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )));
        let yaw = app.view.yaw;
        for _ in 0..100 {
            app.update_held(0.05);
        }
        assert_eq!(app.held.len(), 1, "a hold must survive silence");
        assert_ne!(app.view.yaw, yaw, "and keep moving");
        let mut release = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_input(Event::Key(release));
        assert!(app.held.is_empty(), "release must stop the motion");
    }

    #[test]
    fn load_model_from_text_garbage_fails() {
        let err = load_model_from_text("stream", "garbage here\nno markers\n")
            .expect_err("garbage stream must fail");
        assert!(err.contains("unrecognized"), "error: {err}");
    }
    #[test]
    fn load_model_from_text_obj_errors_with_a_convert_hint() {
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        let err = load_model_from_text("cube", text).expect_err("obj stream must not load");
        assert!(err.contains("wrfm convert"), "error: {err}");
    } // ---------- event loop (input / holds) ----------

    #[test]
    fn load_model_from_text_wrfm_parses() {
        let (mode, name) = load_model_from_text(
            "stream",
            "wrfm 2\nvertices 2   edges 1\n\nv 0 0 0\nv 1 1 1\ne 0 1\n",
        )
        .expect("wrfm stream must load");
        let m = mode;
        assert_eq!(m.vertices.len(), 2);
        assert_eq!(m.edges.len(), 1);
        assert_eq!(name, "stream");
    }
    #[test]
    fn motion_keys_read_as_the_viewer_sees_them() {
        // The model faces out of the screen (+Z) while the sight line points
        // into it, so `h` sweeps the nose toward screen-left. Pitch has no
        // mirror (the model's up is the viewer's up); roll is the third member
        // of the same world-frame family.
        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::YawLeft, 0.3, 0.0);
        // The nose is the third column of the model->world matrix.
        assert!(v.rot[0][2] < 0.0, "h must sweep the nose screen-left");
        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::YawRight, 0.3, 0.0);
        assert!(v.rot[0][2] > 0.0, "l must sweep it screen-right");

        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::PitchUp, 0.3, 0.0);
        assert!(v.rot[1][2] > 0.0, "k must tip the nose up");
        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::PitchDown, 0.3, 0.0);
        assert!(v.rot[1][2] < 0.0, "j must tip it down");

        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::RollMinus, 0.3, 0.0);
        assert!(
            v.rot[1][0] > 0.0,
            "d rolls anticlockwise about the world Z axis"
        );
        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::RollPlus, 0.3, 0.0);
        assert!(v.rot[1][0] < 0.0, "f rolls clockwise");

        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::MoveLeft, 0.0, 0.5);
        assert!(v.pan_x < 0.0, "Shift+h pans screen-left");
        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::MoveUp, 0.0, 0.5);
        assert!(v.pan_y > 0.0, "Shift+k pans up");
        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::MoveForward, 0.0, 0.5);
        assert!(v.dist < ViewState::default().dist, "+ dollies nearer");
    }

    #[test]
    fn local_rotation_uses_the_models_own_axes() {
        // At the default view the sense mirrors the world keys; after a pitch
        // the difference is the axis frame. A local yaw must leave the model's
        // own Y axis fixed, while a world yaw swings that axis in the world.
        let model_y = |view: &ViewState| [view.rot[0][1], view.rot[1][1], view.rot[2][1]];

        let mut local = ViewState::default();
        local.add_pitch(std::f64::consts::FRAC_PI_2);
        let before = model_y(&local);
        apply_motion_step(&mut local, Motion::LocalYawLeft, 0.3, 0.0);
        let after = model_y(&local);
        for i in 0..3 {
            assert!(
                (after[i] - before[i]).abs() < 1e-12,
                "local yaw must leave the model Y axis fixed: {before:?} -> {after:?}"
            );
        }
        assert!(
            (local.yaw - 0.3).abs() < 1e-12,
            "local yaw accumulates the display angle"
        );

        let mut world = ViewState::default();
        world.add_pitch(std::f64::consts::FRAC_PI_2);
        let before = model_y(&world);
        apply_motion_step(&mut world, Motion::YawLeft, 0.3, 0.0);
        let after = model_y(&world);
        assert!(
            (0..3).any(|i| (after[i] - before[i]).abs() > 1e-6),
            "world yaw must move the model Y axis: {before:?} -> {after:?}"
        );

        // At the default view Ctrl+h / Ctrl+d read as the mirror of h / d.
        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::LocalYawLeft, 0.3, 0.0);
        assert!(v.rot[0][2] > 0.0, "Ctrl+h yaws to the model's own left");
        let mut v = ViewState::default();
        apply_motion_step(&mut v, Motion::LocalRollMinus, 0.3, 0.0);
        assert!(v.rot[1][0] < 0.0, "Ctrl+d rolls to the model's own left");
    }

    #[test]
    fn probe_bytes_garbage_is_unrecognized() {
        // A stream of garbage (no magic, no obj markers) is "unrecognized" —
        // never an empty model; a magic-less `v`/`e` stream is likewise not
        // wrfm (the v2 breaking change).
        let err = probe_bytes(b"this is not a wireframe\n").unwrap_err();
        assert!(err.contains("unrecognized"), "error: {err}");
        assert!(probe_bytes(b"v 0 0 0\nv 1 1 1\ne 0 1\n").is_err());
        assert!(probe_bytes(b"").is_err(), "empty stream is unrecognized");
    }
    /// OBJ content must point at the converter — wireforge itself reads
    /// .wrfm only (the ratty/OBJ rendering path is gone).
    #[test]
    fn probe_bytes_obj_content_points_at_the_converter() {
        let err = probe_bytes(b"v 0 0 0\nv 1 0 0\nf 1 2 3\n").unwrap_err();
        assert!(err.contains("wrfm convert"), "hint: {err}");
        // The v-without-e heuristic (a bare point cloud) routes there too.
        let err = probe_bytes(b"v 0 0 0\nv 1 1 1\n").unwrap_err();
        assert!(err.contains("wrfm convert"), "hint: {err}");
    }
    #[test]
    fn probe_bytes_wrfm_magic() {
        // The buffer probe (used by stdin/FIFO) is the same content-first
        // authority as the file probe: `wrfm <version>` first line -> wrfm.
        let buf = b"wrfm 2\nvertices 2   edges 1\n\nv 0 0 0\nv 1 1 1\ne 0 1\n";
        probe_bytes(buf).expect("wrfm magic probes as wrfm");
        // The magic wins even with obj-looking markers later.
        let buf2 = b"wrfm 2\nf 1 2 3\n";
        probe_bytes(buf2).expect("magic wins over obj markers");
    }

    /// Ctrl turns the model around its own axes; Ctrl+C quit stays strict so
    /// Ctrl+Shift+C remains a terminal copy shortcut. Alt swallows the key.
    #[test]
    fn resolve_key_event_ctrl_rotates_locally_and_alt_swallows() {
        use KeyCode::*;
        assert!(matches!(
            resolve_key_event(Char('c'), KeyModifiers::CONTROL),
            Some((_, Action::Quit))
        ));
        assert_eq!(
            resolve_key_event(Char('h'), KeyModifiers::CONTROL),
            Some((Char('h'), Action::Motion(Motion::LocalYawLeft)))
        );
        assert_eq!(
            resolve_key_event(Left, KeyModifiers::CONTROL),
            Some((Left, Action::Motion(Motion::LocalYawLeft)))
        );
        assert_eq!(
            resolve_key_event(Char('d'), KeyModifiers::CONTROL),
            Some((Char('d'), Action::Motion(Motion::LocalRollMinus)))
        );
        assert_eq!(
            resolve_key_event(Char('H'), KeyModifiers::CONTROL.union(KeyModifiers::SHIFT)),
            Some((Char('h'), Action::Motion(Motion::LocalYawLeft)))
        );
        assert_eq!(resolve_key_event(Char('q'), KeyModifiers::CONTROL), None);
        assert_eq!(
            resolve_key_event(Char('C'), KeyModifiers::CONTROL.union(KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(resolve_key_event(Char('h'), KeyModifiers::ALT), None);
        assert_eq!(resolve_key_event(Char(' '), KeyModifiers::ALT), None);
        // Shift+Tab arrives as BackTab on both paths and still toggles the axes.
        assert!(matches!(
            resolve_key_event(BackTab, KeyModifiers::SHIFT),
            Some((_, Action::Axes))
        ));
    }

    /// The same physical input can arrive in two shapes: the **base** key plus
    /// SHIFT (a path without alternate keys — still accepted) or the shifted
    /// **glyph** (what a legacy terminal sends, SHIFT synthesised for
    /// upper-case letters, and what the kitty path now sends for keys with an
    /// alternate, SHIFT cleared). Both must resolve alike — to the same action
    /// under the same identity when the chord is listed, or to nothing at all
    /// when it is not (`Shift+0` types `)` and `)` binds nothing).
    #[test]
    fn resolve_key_event_folds_both_terminal_encodings() {
        use KeyCode::*;
        // Each row is (base+SHIFT form, shifted-glyph form, expected or None).
        let cases = [
            // Listed shifted chords: they bind, in either shape.
            (
                (Char('h'), KeyModifiers::SHIFT),
                (Char('H'), KeyModifiers::SHIFT),
                Some(Action::Motion(Motion::MoveLeft)),
            ),
            (
                (Char('/'), KeyModifiers::SHIFT),
                (Char('?'), KeyModifiers::NONE),
                None,
            ),
            (
                (Char('-'), KeyModifiers::SHIFT),
                (Char('_'), KeyModifiers::NONE),
                Some(Action::Motion(Motion::MoveBack)),
            ),
            (
                (Char('='), KeyModifiers::SHIFT),
                (Char('+'), KeyModifiers::NONE),
                Some(Action::Motion(Motion::MoveForward)),
            ),
            // Shifted chords that are NOT listed: nothing happens, in either shape.
            (
                (Char('0'), KeyModifiers::SHIFT),
                (Char(')'), KeyModifiers::NONE),
                None,
            ),
            (
                (Char('q'), KeyModifiers::SHIFT),
                (Char('Q'), KeyModifiers::SHIFT),
                None,
            ),
            (
                (Char('r'), KeyModifiers::SHIFT),
                (Char('R'), KeyModifiers::SHIFT),
                None,
            ),
        ];
        for ((base_code, base_mods), (glyph_code, glyph_mods), expected) in cases {
            let label = format!("{base_code:?}+{base_mods:?} vs {glyph_code:?}+{glyph_mods:?}");
            let base = resolve_key_event(base_code, base_mods);
            let glyph = resolve_key_event(glyph_code, glyph_mods);
            assert_eq!(
                base.as_ref().map(|(_, action)| *action),
                expected,
                "base+SHIFT: {label}"
            );
            assert_eq!(
                glyph.as_ref().map(|(_, action)| *action),
                expected,
                "glyph: {label}"
            );
            if let (Some((base_id, _)), Some((glyph_id, _))) = (&base, &glyph) {
                assert_eq!(base_id, glyph_id, "both must hold the same key: {label}");
            }
        }
    }
    /// Every plain key must still bind to its documented action after the
    /// modifier rules were simplified.
    #[test]
    fn resolve_key_event_keeps_every_unshifted_key_bound() {
        use KeyCode::*;
        let documented = [
            (Char('q'), Action::Quit),
            (Esc, Action::Menu),
            (Char(' '), Action::Spin),
            (Tab, Action::Axes),
            (Char('r'), Action::Reset),
            (Char('d'), Action::Motion(Motion::RollMinus)),
            (Char('f'), Action::Motion(Motion::RollPlus)),
            (Char('h'), Action::Motion(Motion::YawLeft)),
            (Char('='), Action::Motion(Motion::MoveForward)),
            (Char('-'), Action::Motion(Motion::MoveBack)),
            (Left, Action::Motion(Motion::YawLeft)),
        ];
        for (code, expected) in documented {
            let (_, action) = resolve_key_event(code, KeyModifiers::NONE)
                .unwrap_or_else(|| panic!("{code:?} must stay bound"));
            assert_eq!(action, expected, "{code:?}");
        }
    }

    /// Plain `c` and `e` bind nothing; `Ctrl+c` is the quit chord, and
    /// `Ctrl+e` is not a substitute for it.
    #[test]
    fn unbound_letters_stay_unbound() {
        use KeyCode::*;
        for (code, mods) in [
            (Char('c'), KeyModifiers::NONE),
            (Char('e'), KeyModifiers::NONE),
            (Char('e'), KeyModifiers::CONTROL),
            (Char('y'), KeyModifiers::NONE),
        ] {
            assert!(
                resolve_key_event(code, mods).is_none(),
                "{code:?}+{mods:?} must bind nothing"
            );
        }
        assert!(matches!(
            resolve_key_event(Char('c'), KeyModifiers::CONTROL),
            Some((_, Action::Quit))
        ));
    }

    /// Regression: `held` used to be keyed by Motion, so a modifier change
    /// between Press and Release made key-up resolve to a *different* motion —
    /// the original one kept running, and with key-up reporting there is no
    /// timeout left to stop it.
    #[test]
    fn shift_change_mid_hold_cannot_leave_a_hold_running() {
        let mut app = app_with("cube");
        app.release_seen = true;
        // Press `h` (rotate), then Shift: the Repeat now spells "move", but it
        // is still the same key, so there is exactly one hold — and a Repeat
        // never re-samples the motion it started with.
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::NONE,
        )));
        let mut shifted = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::SHIFT);
        shifted.kind = KeyEventKind::Repeat;
        app.handle_input(Event::Key(shifted));
        assert_eq!(app.held.len(), 1, "one key must not fork into two motions");
        assert_eq!(app.held[&KeyCode::Char('h')], Motion::YawLeft);
        // Release the key while Shift is still down: key-up carries SHIFT and
        // must still find the hold.
        let mut release = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::SHIFT);
        release.kind = KeyEventKind::Release;
        app.handle_input(Event::Key(release));
        assert!(app.held.is_empty(), "shifted key-up must stop the hold");

        // The other order: press Shift+h (move), release Shift first, and the
        // key-up arrives WITHOUT SHIFT. Matching on the motion here is what
        // used to leave the model panning forever.
        app.handle_input(Event::Key(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::SHIFT,
        )));
        assert_eq!(app.held[&KeyCode::Char('h')], Motion::MoveLeft);
        let mut release = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_input(Event::Key(release));
        assert!(app.held.is_empty(), "unshifted key-up must stop the hold");
    }
}
