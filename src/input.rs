//! The motion-input state machine.
//!
//! One terminal key owns one hold. A terminal that reports key-up (the Kitty
//! keyboard protocol) sends Press, Repeat and Release: the hold starts on the
//! Press and lasts until the Release. A terminal that cannot report key-up has
//! no hold: every event it sends is one finite step. The hold's age selects the
//! speed gear in [`crate::motion`].
//!
//! Silence is not evidence that a key was released, so a hold is never timed
//! out. Release, FocusLost, and the modal overlays are the only stops.

use std::{collections::HashMap, time::Instant};

use crossterm::event::{KeyCode, KeyEventKind};

use crate::{
    motion::{Motion, continuous_step, hold_speed_scale, tap_step},
    view::ViewState,
};

/// A key that is down and moving the view until its Release.
struct Hold {
    motion: Motion,
    started_at: Instant,
}

/// Motion-key state for the session.
pub(crate) struct MotionInput {
    /// True once the terminal proved it reports key-up: the start-up probe or
    /// an observed Release. Without that proof there is no hold to own.
    reports_key_up: bool,
    holds: HashMap<KeyCode, Hold>,
}

impl MotionInput {
    pub(crate) fn new(reports_key_up: bool) -> Self {
        Self {
            reports_key_up,
            holds: HashMap::new(),
        }
    }

    /// Record that the terminal reports key-up. The start-up probe and an
    /// observed Release both call this; the capability only ever moves forward.
    pub(crate) fn set_key_up(&mut self, reports_key_up: bool) {
        self.reports_key_up |= reports_key_up;
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
        if !self.reports_key_up {
            // No key-up: the terminal can only say "this key fired", so every
            // event is one finite step. Its own repeat setting is the cadence.
            tap_step(view, motion, speed);
            return true;
        }
        match kind {
            // A Repeat never re-samples a live hold; the frame clock owns it.
            // With no hold, the Press was missed (or key-up was learned after
            // the Press), so adopt the Repeat as the start of the hold.
            KeyEventKind::Repeat => {
                if self.holds.contains_key(&hold_id) {
                    return false;
                }
                self.holds.insert(
                    hold_id,
                    Hold {
                        motion,
                        started_at: at,
                    },
                );
                true
            }
            // A Press always answers: it taps immediately and (re)starts the
            // hold. A second Press with no Release in between means the
            // terminal lost a Release; re-syncing here is better than
            // swallowing the key.
            _ => {
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

    /// Handle a Release by identity and record that the terminal reports
    /// key-up. The modifiers may differ from the Press, which is why the caller
    /// resolves the physical key first.
    pub(crate) fn on_release(&mut self, hold_id: KeyCode) -> bool {
        self.set_key_up(true);
        self.holds.remove(&hold_id).is_some()
    }

    /// Advance every live hold by one frame. Returns true when a hold was
    /// advanced.
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
        for hold in self.holds.values() {
            let scale = hold_speed_scale(now.saturating_duration_since(hold.started_at));
            continuous_step(view, hold.motion, speed, dt, scale);
        }
        true
    }

    /// Drop every hold. FocusLost and the modal overlays use this; the key-up
    /// capability survives because it is a terminal property.
    pub(crate) fn clear(&mut self) -> bool {
        if self.holds.is_empty() {
            return false;
        }
        self.holds.clear();
        true
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.holds.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn reports_key_up(&self) -> bool {
        self.reports_key_up
    }

    #[cfg(test)]
    pub(crate) fn hold_count(&self) -> usize {
        self.holds.len()
    }

    #[cfg(test)]
    pub(crate) fn held_motion(&self, hold_id: KeyCode) -> Option<Motion> {
        self.holds.get(&hold_id).map(|hold| hold.motion)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{ROT_RATE, TAP_ROT};
    use crossterm::event::KeyCode::*;
    use std::time::Duration;

    fn step(
        input: &mut MotionInput,
        view: &mut ViewState,
        hold_id: KeyCode,
        motion: Motion,
        kind: KeyEventKind,
        at: Instant,
    ) -> bool {
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
    fn legacy_every_event_is_one_finite_step() {
        let at = Instant::now();
        let mut input = MotionInput::new(false);
        let mut view = ViewState::default();
        let yaw = view.yaw;
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at,
        );
        assert!((yaw - view.yaw - TAP_ROT).abs() < 1e-12);

        // A repeat is another event, so it is another finite step: the
        // terminal's repeat setting decides the cadence.
        let yaw = view.yaw;
        step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Repeat,
            at + Duration::from_millis(40),
        );
        assert!((yaw - view.yaw - TAP_ROT).abs() < 1e-12);
        assert!(!input.is_active(), "no key-up means no hold");
    }

    #[test]
    fn a_release_teaches_key_up_and_a_repeat_starts_the_hold() {
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

        // A Release from another key proves the terminal reports key-up. The
        // held key's next Repeat starts the hold instead of remaining a tap.
        assert!(!input.on_release(Char('z')));
        assert!(input.reports_key_up());
        let yaw = view.yaw;
        assert!(step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Repeat,
            at + Duration::from_millis(40),
        ));
        assert_eq!(view.yaw, yaw, "the Repeat only starts the hold");
        assert_eq!(input.held_motion(Char('h')), Some(Motion::YawLeft));
        input.advance(at + Duration::from_millis(40), 0.01, &mut view, 1.0);
        assert!(view.yaw < yaw);
    }

    #[test]
    fn duplicate_press_retaps_and_restarts_the_hold() {
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
        assert!(step(
            &mut input,
            &mut view,
            Char('h'),
            Motion::YawLeft,
            KeyEventKind::Press,
            at + Duration::from_millis(50),
        ));
        assert!(
            (yaw - view.yaw - TAP_ROT).abs() < 1e-12,
            "a duplicate press must answer with a tap"
        );
        assert_eq!(input.hold_count(), 1, "it must not fork the hold");
        assert_eq!(input.held_motion(Char('h')), Some(Motion::YawLeft));
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
    fn a_hold_survives_silence_until_release() {
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
        // Silence is not a Release: a minute of frames must keep the hold.
        input.advance(at + Duration::from_secs(60), 0.016, &mut view, 1.0);
        assert!(input.is_active(), "silence must not drop a hold");
        assert!(view.yaw < yaw, "the hold keeps moving");
        assert!(input.on_release(Char('h')));
        assert!(!input.is_active());
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
