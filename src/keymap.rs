//! The one binding table, and the resolver that turns terminal key events into
//! actions. The help panel renders the same table, so the two cannot drift.

use crate::motion::Motion;
use crossterm::event::{KeyCode, KeyModifiers};

/// What one key press does. A `Motion` key is continuous while held on a
/// terminal that reports key-up; a terminal that cannot report it gets one
/// finite step per press and auto-repeat. Every other action fires once per
/// press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
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
pub(crate) enum Group {
    Rotate,
    Local,
    Move,
    View,
    Session,
}

impl Group {
    /// The help panel's columns, top to bottom. World and model rotation lead
    /// the left column; the camera and session groups fill the right.
    pub(crate) const COLUMNS: [&[Group]; 2] = [
        &[Group::Rotate, Group::Local],
        &[Group::Move, Group::View, Group::Session],
    ];

    pub(crate) fn title(self) -> &'static str {
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
pub(crate) const fn canonical_key(code: KeyCode, mods: KeyModifiers) -> (KeyCode, bool) {
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
pub(crate) struct Chord {
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
pub(crate) struct Binding {
    pub(crate) action: Action,
    pub(crate) group: Group,
    /// The chord column of the help panel.
    pub(crate) keys: &'static str,
    /// The verb column of the help panel.
    pub(crate) doc: &'static str,
    /// Every documented way to press it.
    pub(crate) chords: &'static [Chord],
}

/// Every binding, grouped by the task it belongs to.
pub(crate) const BINDINGS: &[Binding] = &[
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
pub(crate) fn resolve_key_event(code: KeyCode, mods: KeyModifiers) -> Option<(KeyCode, Action)> {
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
    pub(crate) fn label(&self) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
