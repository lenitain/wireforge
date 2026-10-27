//! The motion-input state machine.
//!
//! One terminal key owns one hold. Presses become a finite tap plus, on a
//! key-up terminal, a frame-clocked hold; a terminal that cannot report key-up
//! stays event-clocked and gets gap-scaled repeats. The hold's age selects the
//! speed gear in [`crate::motion`].

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyEventKind};

use crate::{
    motion::{Motion, continuous_step, hold_speed_scale, tap_step},
    view::ViewState,
};

/// How long a hold may go without any key event before the lost-Release
/// watchdog drops it. Terminals that report key-up normally send auto-repeat
/// events while a key is down, so a live hold refreshes this well within the
/// window.
pub(crate) const HOLD_WATCHDOG: Duration = Duration::from_secs(10);

/// The largest gap between two motion-key events still treated as one
/// auto-repeat on a terminal that cannot report key-up. A longer gap is a new
/// press (or the first auto-repeat after the initial repeat delay), so it gets
/// the finite tap step instead of a large time-scaled jump.
pub(crate) const LEGACY_REPEAT_GAP: Duration = Duration::from_millis(200);

/// Whether the terminal can report a key release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyUpMode {
    /// No evidence yet: presses are finite, event-clocked steps.
    Unknown,
    /// The start-up probe or an observed Release proved key-up support.
    Reported,
}

/// A key that is down and moving the view until its Release.
struct Hold {
    motion: Motion,
    started_at: Instant,
}

/// The event clock for a terminal without key-up reporting.
struct LegacyRepeat {
    /// The motion sampled at the press that began this chain.
    motion: Motion,
    /// When this press chain began; a promoted hold inherits it.
    started_at: Instant,
    /// The previous event, for gap-scaled auto-repeat.
    last_at: Instant,
}

/// Motion-key state across the whole session.
pub(crate) struct MotionInput {
    key_up: KeyUpMode,
    holds: HashMap<KeyCode, Hold>,
    legacy: HashMap<KeyCode, LegacyRepeat>,
    last_key_event_at: Instant,
}

impl MotionInput {
    pub(crate) fn new(reports_key_up: bool) -> Self {
        Self {
            key_up: if reports_key_up {
                KeyUpMode::Reported
            } else {
                KeyUpMode::Unknown
            },
            holds: HashMap::new(),
            legacy: HashMap::new(),
            last_key_event_at: Instant::now(),
        }
    }

    /// Record that the terminal reports key-up. The start-up probe and an
    /// observed Release both call this; the mode only ever moves forward.
    pub(crate) fn set_key_up(&mut self, reports_key_up: bool) {
        if reports_key_up {
            self.key_up = KeyUpMode::Reported;
        }
    }

    /// Note any key event. The watchdog only measures keyboard silence, so
    /// unbound keys and UI keys refresh it too.
    pub(crate) fn note_key(&mut self, at: Instant) {
        self.last_key_event_at = at;
    }

    /// Handle one motion key press or repeat. Returns true when the view
    /// changed or a hold was created.
    pub(crate) fn on_motion(
        &mut self,
        hold_id: KeyCode,
        motion: Motion,
        kind: KeyEventKind,
        at: Instant,
        view: &mut ViewState,
        speed: f64,
    ) -> bool {
        if kind == KeyEventKind::Release {
            return false;
        }
        match self.key_up {
            KeyUpMode::Reported => self.on_reported(hold_id, motion, kind, at, view, speed),
            KeyUpMode::Unknown => self.on_legacy(hold_id, motion, at, view, speed),
        }
    }

    /// Handle a Release by identity and record that the terminal reports
    /// key-up. The modifiers may differ from the Press, which is why the caller
    /// resolves the physical key first.
    pub(crate) fn on_release(&mut self, hold_id: KeyCode) -> bool {
        self.key_up = KeyUpMode::Reported;
        let had_hold = self.holds.remove(&hold_id).is_some();
        let had_repeat = self.legacy.remove(&hold_id).is_some();
        had_hold || had_repeat
    }

    /// Advance every live hold by one frame and drop a hold whose Release was
    /// lost. Returns true when a hold was advanced or dropped.
    pub(crate) fn advance(
        &mut self,
        now: Instant,
        dt: f64,
        view: &mut ViewState,
        speed: f64,
    ) -> bool {
        if self.holds.is_empty() {
            return false;
        }
        if now.saturating_duration_since(self.last_key_event_at) > HOLD_WATCHDOG {
            return self.clear();
        }
        for hold in self.holds.values() {
            let scale = hold_speed_scale(now.saturating_duration_since(hold.started_at));
            continuous_step(view, hold.motion, speed, dt, scale);
        }
        true
    }

    /// Drop every hold and legacy chain. FocusLost and the modal overlays use
    /// this; the key-up capability survives because it is a terminal property.
    pub(crate) fn clear(&mut self) -> bool {
        if self.holds.is_empty() && self.legacy.is_empty() {
            return false;
        }
        self.holds.clear();
        self.legacy.clear();
        true
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.holds.is_empty()
    }

    fn on_reported(
        &mut self,
        hold_id: KeyCode,
        motion: Motion,
        kind: KeyEventKind,
        at: Instant,
        view: &mut ViewState,
        speed: f64,
    ) -> bool {
        match kind {
            // A Repeat for an existing hold never re-samples its motion; the
            // frame clock owns it. If the key was already down before key-up
            // was known, promote it using its original press time so the gear
            // clock does not restart.
            KeyEventKind::Repeat => {
                if self.holds.contains_key(&hold_id) {
                    return false;
                }
                let (motion, started_at) = self
                    .legacy
                    .remove(&hold_id)
                    .map_or((motion, at), |repeat| (repeat.motion, repeat.started_at));
                self.holds.insert(hold_id, Hold { motion, started_at });
                true
            }
            // Press. A partial terminal can send another Press without a
            // Release; the existing hold owns the motion and its start time.
            _ => {
                if self.holds.contains_key(&hold_id) {
                    return false;
                }
                self.legacy.remove(&hold_id);
                tap_step(view, motion, speed);
                self.holds.insert(
                    hold_id,
                    Hold {
                        motion,
                        started_at: at,
                    },
                );
                true
            }
        }
    }

    fn on_legacy(
        &mut self,
        hold_id: KeyCode,
        motion: Motion,
        at: Instant,
        view: &mut ViewState,
        speed: f64,
    ) -> bool {
        let gap = self
            .legacy
            .get(&hold_id)
            .map(|repeat| at.saturating_duration_since(repeat.last_at));
        if let Some(gap) = gap.filter(|gap| *gap <= LEGACY_REPEAT_GAP) {
            continuous_step(view, motion, speed, gap.as_secs_f64(), 1.0);
            if let Some(repeat) = self.legacy.get_mut(&hold_id) {
                repeat.last_at = at;
            }
            return true;
        }
        // A new press, or the first repeat after the initial delay: one finite
        // tap. Resetting the chain keeps a later promotion from inheriting an
        // age that belongs to an earlier press.
        tap_step(view, motion, speed);
        self.legacy.insert(
            hold_id,
            LegacyRepeat {
                motion,
                started_at: at,
                last_at: at,
            },
        );
        true
    }

    #[cfg(test)]
    pub(crate) fn reports_key_up(&self) -> bool {
        self.key_up == KeyUpMode::Reported
    }

    #[cfg(test)]
    pub(crate) fn hold_count(&self) -> usize {
        self.holds.len()
    }

    #[cfg(test)]
    pub(crate) fn held_motion(&self, hold_id: KeyCode) -> Option<Motion> {
        self.holds.get(&hold_id).map(|hold| hold.motion)
    }

    #[cfg(test)]
    pub(crate) fn has_legacy(&self, hold_id: KeyCode) -> bool {
        self.legacy.contains_key(&hold_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{ROT_RATE, TAP_PAN};
    use crossterm::event::KeyCode::*;

    fn step(
        input: &mut MotionInput,
        view: &mut ViewState,
        hold_id: KeyCode,
        motion: Motion,
        kind: KeyEventKind,
        at: Instant,
    ) -> bool {
        input.note_key(at);
        input.on_motion(hold_id, motion, kind, at, view, 1.0)
    }

    #[test]
    fn reported_press_holds_and_release_stops_it() {
        let at = Instant::now();
        let mut input = MotionInput::new(true);
        let mut view = ViewState::default();
        assert!(step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        ));
        assert!(input.is_active());
        let yaw = view.yaw;
        assert!(input.advance(at, 0.01, &mut view, 1.0));
        assert!(view.yaw < yaw, "the hold must move the view");

        assert!(input.on_release(Char('h')));
        assert!(!input.is_active());
        let yaw = view.yaw;
        assert!(!input.advance(at, 0.01, &mut view, 1.0));
        assert_eq!(view.yaw, yaw, "a released key must stop immediately");
    }

    #[test]
    fn unknown_press_is_a_finite_step_without_drift() {
        let at = Instant::now();
        let mut input = MotionInput::new(false);
        let mut view = ViewState::default();
        assert!(step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        ));
        assert!(!input.is_active(), "legacy presses never create a hold");
        let yaw = view.yaw;
        assert!(!input.advance(at + Duration::from_secs(1), 1.0, &mut view, 1.0));
        assert_eq!(view.yaw, yaw, "no hold means no drift");
    }

    #[test]
    fn legacy_repeat_scales_by_gap_after_a_tap() {
        let at = Instant::now();
        let mut input = MotionInput::new(false);
        let mut view = ViewState::default();
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::MoveLeft,
            KeyEventKind::Press,
            at,
        );
        let after_press = view.pan_x;

        // The first repeat after the initial delay is still a tap; scaling it
        // by the whole delay would jump.
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::MoveLeft,
            KeyEventKind::Repeat,
            at + Duration::from_millis(600),
        );
        let after_first_repeat = view.pan_x;
        assert!((after_press - after_first_repeat - TAP_PAN).abs() < 1e-12);

        // The next repeat arrives 40 ms later: one continuous frame.
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::MoveLeft,
            KeyEventKind::Repeat,
            at + Duration::from_millis(640),
        );
        let step = after_first_repeat - view.pan_x;
        assert!((step - 0.040).abs() < 1e-12, "repeat follows its gap");
        assert!(!input.is_active());
    }

    #[test]
    fn repeat_promotes_a_legacy_press_with_its_original_age() {
        let at = Instant::now();
        let mut input = MotionInput::new(false);
        let mut view = ViewState::default();
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        );
        assert!(!input.is_active());

        // A Release from another key teaches key-up while `h` is still down.
        input.note_key(at + Duration::from_millis(10));
        assert!(!input.on_release(Char('z')));
        assert!(step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::MoveLeft,
            KeyEventKind::Repeat,
            at + Duration::from_millis(250),
        ));
        assert_eq!(
            input.held_motion(Char('h')),
            Some(Motion::YawLeft),
            "the promoted hold keeps the motion sampled at the press"
        );

        // The promoted hold inherits the original press time, so at 250 ms it
        // is already in the fast gear.
        let yaw = view.yaw;
        input.advance(at + Duration::from_millis(250), 0.01, &mut view, 1.0);
        assert!((yaw - view.yaw - 2.0 * ROT_RATE * 0.01).abs() < 1e-12);
    }

    #[test]
    fn duplicate_press_cannot_restart_or_replace_a_hold() {
        let at = Instant::now();
        let mut input = MotionInput::new(true);
        let mut view = ViewState::default();
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        );
        let yaw = view.yaw;
        assert!(!step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::MoveLeft,
            KeyEventKind::Press,
            at + Duration::from_millis(50),
        ));
        assert_eq!(view.yaw, yaw, "a duplicate press must not tap again");
        assert_eq!(input.held_motion(Char('h')), Some(Motion::YawLeft));
        assert_eq!(input.hold_count(), 1);
    }

    #[test]
    fn a_repeat_never_resamples_the_held_motion() {
        let at = Instant::now();
        let mut input = MotionInput::new(true);
        let mut view = ViewState::default();
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        );
        let pan = view.pan_x;
        let yaw = view.yaw;
        assert!(!step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::MoveLeft,
            KeyEventKind::Repeat,
            at + Duration::from_millis(10),
        ));
        assert_eq!(view.pan_x, pan, "the repeat must not pan");
        assert_eq!(view.yaw, yaw, "the repeat must not tap");
        assert_eq!(input.held_motion(Char('h')), Some(Motion::YawLeft));
    }

    #[test]
    fn hold_gear_is_a_step_from_the_press_time() {
        fn frame_step(age_ms: u64) -> f64 {
            let at = Instant::now();
            let mut input = MotionInput::new(true);
            let mut view = ViewState::default();
            step(
                &mut input,
                &mut view,
                Char('h'),
                Motion::YawLeft,
                KeyEventKind::Press,
                at,
            );
            let yaw = view.yaw;
            input.advance(at + Duration::from_millis(age_ms), 0.01, &mut view, 1.0);
            yaw - view.yaw
        }

        let slow = ROT_RATE * 0.01;
        assert!((frame_step(0) - slow).abs() < 1e-12);
        assert!((frame_step(199) - slow).abs() < 1e-12);
        assert!((frame_step(200) - 2.0 * slow).abs() < 1e-12);
        assert!((frame_step(10_000) - 2.0 * slow).abs() < 1e-12);
        // Same dt, same key: the only two speeds are one and two. The 100 ms
        // and 500 ms frames sit on opposite sides of the step.
        assert!((frame_step(500) / frame_step(100) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn watchdog_drops_a_hold_but_a_key_event_refreshes_it() {
        let at = Instant::now();
        let mut input = MotionInput::new(true);
        let mut view = ViewState::default();
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        );
        input.note_key(at + HOLD_WATCHDOG - Duration::from_millis(1));
        assert!(input.advance(at + HOLD_WATCHDOG, 0.016, &mut view, 1.0));
        assert!(input.is_active(), "a recent key event keeps the hold alive");

        assert!(input.advance(at + HOLD_WATCHDOG + HOLD_WATCHDOG, 0.016, &mut view, 1.0));
        assert!(
            !input.is_active(),
            "silence past the watchdog drops the hold"
        );
    }

    #[test]
    fn clear_drops_holds_but_keeps_key_up_capability() {
        let at = Instant::now();
        let mut input = MotionInput::new(true);
        let mut view = ViewState::default();
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        );
        assert!(input.clear());
        assert!(!input.is_active());
        assert!(step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at + Duration::from_millis(1),
        ));
        assert!(input.is_active(), "clear keeps the terminal capability");
    }

    #[test]
    fn a_release_teaches_key_up_for_the_next_press() {
        let at = Instant::now();
        let mut input = MotionInput::new(false);
        let mut view = ViewState::default();
        assert!(!input.on_release(Char('z')));
        assert!(step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        ));
        assert!(input.is_active());
    }

    #[test]
    fn hold_clock_uses_the_hold_that_was_just_pressed() {
        // Two keys have independent gears: one pressed earlier is fast while
        // a newer one is still slow.
        let at = Instant::now();
        let mut input = MotionInput::new(true);
        let mut view = ViewState::default();
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        );
        step(
            &mut input,
            &mut view,
            Char('l'),
            Motion::YawRight,
            KeyEventKind::Press,
            at + Duration::from_millis(150),
        );
        let before = view.yaw;
        input.advance(at + Duration::from_millis(250), 0.01, &mut view, 1.0);
        // h is fast (2 units left) and l is slow (1 unit right): the net is
        // one slow unit to the left, not a cancellation.
        assert!((before - view.yaw - ROT_RATE * 0.01).abs() < 1e-12);
    }
}
