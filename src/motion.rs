//! Motion vocabulary and the per-frame math that drives it.
//!
//! Everything here is pure: a [`Motion`] plus a step size in, a [`ViewState`]
//! mutation out. Keyboards, event timing and terminal capabilities live in
//! [`crate::input`].

use std::time::Duration;

use crate::view::ViewState;

/// Smooth continuous rotation rate (radians per second) while a key is held.
pub(crate) const ROT_RATE: f64 = 169.0 / 128.0;

/// Auto-spin yaw rate (radians per second): 169/256 = 0.66015625, a dyadic
/// (exact-in-binary) constant.
pub(crate) const SPIN_RATE: f64 = 169.0 / 256.0;

/// A tap's rotation: 3 degrees, enough to feel like an answer without making
/// fine alignment a chore.
pub(crate) const TAP_ROT: f64 = std::f64::consts::PI / 60.0;
/// A tap's pan, as a fraction of the session's translation speed.
pub(crate) const TAP_PAN: f64 = 1.0 / 20.0;
/// A tap's dolly, as a fraction of the session's translation speed. Zoom
/// wants to feel more immediate than a nudge of the model.
pub(crate) const TAP_DOLLY: f64 = 1.0 / 8.0;

/// How long a hold stays in the slow gear before switching to the fast one.
pub(crate) const HOLD_FAST_AFTER: Duration = Duration::from_millis(200);

/// The fast gear's fixed multiplier. It is a step, not the start of a ramp.
pub(crate) const HOLD_FAST_SCALE: f64 = 2.0;

/// One continuous degree of freedom: rotation (yaw / pitch / roll) and
/// translation (pan / dolly). Rotation comes in two frames: plain keys turn
/// around the fixed world axes (turntable), Ctrl keys around the model's own
/// current axes (trackball). The frame picks the axis, never the direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Motion {
    YawLeft,
    YawRight,
    PitchUp,
    PitchDown,
    RollPlus,
    RollMinus,
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

/// The hold gear for a key that has been down for `held_for`.
///
/// Two gears, selected by age alone: 1x before [`HOLD_FAST_AFTER`], then the
/// fixed [`HOLD_FAST_SCALE`] for as long as the key stays down.
pub(crate) fn hold_speed_scale(held_for: Duration) -> f64 {
    if held_for >= HOLD_FAST_AFTER {
        HOLD_FAST_SCALE
    } else {
        1.0
    }
}

/// One finite tap step. It must not depend on frame time, so a short press
/// always moves the view by the same amount.
pub(crate) fn tap_step(view: &mut ViewState, motion: Motion, speed: f64) {
    let mv = match motion {
        Motion::MoveForward | Motion::MoveBack => speed * TAP_DOLLY,
        _ => speed * TAP_PAN,
    };
    apply_motion_step(view, motion, TAP_ROT, mv);
}

/// One frame of smooth motion for a held key, scaled by its gear.
pub(crate) fn continuous_step(
    view: &mut ViewState,
    motion: Motion,
    speed: f64,
    dt: f64,
    scale: f64,
) {
    apply_motion_step(view, motion, ROT_RATE * dt * scale, speed * dt * scale);
}

/// Apply one motion step: rotation around the world or the model's own axes,
/// pan in world X/Y, dolly along the view axis.
pub(crate) fn apply_motion_step(view: &mut ViewState, motion: Motion, rot: f64, mv: f64) {
    match motion {
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
        // World-frame roll: positive roll spins the image anticlockwise, so
        // `d` (QWERTY, left of `f`) dips the viewer's screen-left side and `f`
        // dips the right.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_constants_are_dyadic_and_exact() {
        assert_eq!(ROT_RATE * 128.0, 169.0);
        assert_eq!(SPIN_RATE * 256.0, 169.0);
    }

    #[test]
    fn hold_speed_scale_is_a_step_at_200ms() {
        assert_eq!(hold_speed_scale(Duration::ZERO), 1.0);
        assert_eq!(hold_speed_scale(Duration::from_millis(199)), 1.0);
        assert_eq!(hold_speed_scale(Duration::from_millis(200)), 2.0);
        assert_eq!(hold_speed_scale(Duration::from_secs(10)), 2.0);
        // There is no third speed: every age at or past the boundary is the
        // same value, and every age below it is the slow gear.
        for ms in 0..1000 {
            let scale = hold_speed_scale(Duration::from_millis(ms));
            assert_eq!(scale, if ms >= 200 { 2.0 } else { 1.0 }, "{ms}ms");
        }
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
}
