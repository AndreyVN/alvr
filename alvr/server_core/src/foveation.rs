//! Eye-tracking → foveation-centre math (Phase 7 per-view foveation, Slice 5).
//!
//! Stateless conversion from a head-space gaze quaternion to image-space
//! `center_shift` parameters matching
//! `alvr_session::FoveatedEncodingConfig::center_shift_{x,y}`. The integration
//! into the connection-loop worker that emits
//! `ServerCoreEvent::PerViewFoveation` lives elsewhere; this module is
//! deliberately a pure computation so it can be unit-tested without spinning
//! up tracking or sockets.
//!
//! No time-series smoothing here — callers that want to suppress saccade
//! microjitter should drive a low-pass filter (one-Euro, Kalman, EMA, etc.)
//! upstream of `gaze_to_center_shift`.

use alvr_common::glam::{Quat, Vec3};

/// Convert a head-space gaze quaternion into image-space foveation
/// `[center_shift_x, center_shift_y]` for a single view.
///
/// - `gaze` is the eye's orientation in head space (OpenXR convention: +Y up,
///   −Z forward). Identity = looking straight ahead → returns `[0.0, 0.0]`.
/// - `half_fov_x_rad` / `half_fov_y_rad` are the view's half-FOVs in radians.
///   These are the symmetric magnitudes; for an asymmetric FOV pass the
///   larger of `(fov.left, fov.right)` / `(fov.up, fov.down)`.
/// - `dead_band_rad` zeroes out gaze angles below the threshold (suppresses
///   micro-jitter — ~0.035 rad ≈ 2° is the value the scoping doc suggests).
/// - `max_offset_normalized` is the maximum image-space offset (matches
///   `PerViewFoveationConfig::max_offset_normalized` in `alvr_session`).
///
/// The output sign matches `FoveatedEncodingConfig::center_shift_x` / `_y`:
/// positive X = gaze right of view centre, positive Y = gaze above view centre.
pub fn gaze_to_center_shift(
    gaze: Quat,
    half_fov_x_rad: f32,
    half_fov_y_rad: f32,
    dead_band_rad: f32,
    max_offset_normalized: f32,
) -> [f32; 2] {
    // Direction the eye is looking, in head space. Quat::IDENTITY * NEG_Z = NEG_Z
    // (straight forward).
    let direction: Vec3 = gaze * Vec3::NEG_Z;

    // Yaw = horizontal angle, pitch = vertical angle.
    //
    // For a +Y-up, −Z-forward frame:
    //   yaw   = atan2(direction.x, -direction.z)   (positive when looking right)
    //   pitch = asin(direction.y)                  (positive when looking up)
    //
    // The atan2 form stays well-behaved across the full hemisphere; a naive
    // `asin(direction.x)` would lose monotonicity past ±90°.
    let yaw = direction.x.atan2(-direction.z);
    let pitch = direction.y.clamp(-1.0, 1.0).asin();

    // Dead band: tiny angles get zeroed before normalisation. Applied
    // independently per axis so a purely-horizontal saccade still moves the
    // centre even if pitch is in the dead zone.
    let yaw = apply_dead_band(yaw, dead_band_rad);
    let pitch = apply_dead_band(pitch, dead_band_rad);

    // Normalise to image space and clamp. We divide by the half-FOV: gaze at
    // the edge of the view (yaw == half_fov_x) maps to ±1.0 before clamp;
    // gaze at the centre maps to 0. The clamp then bounds the centre to
    // [-max_offset_normalized, max_offset_normalized] so we never drag the
    // foveation inset off the rendered region.
    let normalised_x = if half_fov_x_rad > 0.0 {
        (yaw / half_fov_x_rad).clamp(-max_offset_normalized, max_offset_normalized)
    } else {
        0.0
    };
    let normalised_y = if half_fov_y_rad > 0.0 {
        (pitch / half_fov_y_rad).clamp(-max_offset_normalized, max_offset_normalized)
    } else {
        0.0
    };

    [normalised_x, normalised_y]
}

/// Zero out `value` when `|value| < threshold`. Otherwise return `value`
/// unchanged (no smoothing past the threshold — that's the caller's job).
fn apply_dead_band(value: f32, threshold: f32) -> f32 {
    if value.abs() < threshold { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alvr_common::glam::EulerRot;

    const HALF_FOV: f32 = 0.9; // ~51° — typical headset half-FOV, matches FB Touch
    const DEAD_BAND: f32 = 0.035; // ~2°
    const MAX_OFFSET: f32 = 0.25;

    /// Build a gaze quaternion from `yaw_right` / `pitch_up` in radians, using
    /// the same convention as the extractor: positive `yaw_right` means the
    /// gaze direction is to the right of forward (+X side), positive
    /// `pitch_up` means above forward (+Y side). Glam's right-hand-rule Y
    /// rotation goes the *other* way (positive Euler yaw is left), so the
    /// helper passes `-yaw_right` to the Euler builder.
    fn gaze_quat(yaw_right: f32, pitch_up: f32) -> Quat {
        Quat::from_euler(EulerRot::YXZ, -yaw_right, pitch_up, 0.0)
    }

    #[test]
    fn identity_gaze_centred() {
        let shift = gaze_to_center_shift(Quat::IDENTITY, HALF_FOV, HALF_FOV, DEAD_BAND, MAX_OFFSET);
        assert!(shift[0].abs() < 1e-6, "x centred, got {}", shift[0]);
        assert!(shift[1].abs() < 1e-6, "y centred, got {}", shift[1]);
    }

    #[test]
    fn dead_band_suppresses_microjitter() {
        // Gaze 1° to the right — below the 2° dead band.
        let q = gaze_quat(0.5_f32.to_radians(), 0.0);
        let shift = gaze_to_center_shift(q, HALF_FOV, HALF_FOV, DEAD_BAND, MAX_OFFSET);
        assert_eq!(shift, [0.0, 0.0], "microjitter must be suppressed");
    }

    #[test]
    fn large_right_gaze_clamped_to_max_offset() {
        // Gaze well past the half-FOV to the right — must clamp to +max_offset.
        let q = gaze_quat(HALF_FOV * 2.0, 0.0);
        let shift = gaze_to_center_shift(q, HALF_FOV, HALF_FOV, DEAD_BAND, MAX_OFFSET);
        assert!(
            (shift[0] - MAX_OFFSET).abs() < 1e-6,
            "right gaze clamped to +max_offset, got {}",
            shift[0]
        );
        assert!(shift[1].abs() < 1e-6, "pitch unchanged, got {}", shift[1]);
    }

    #[test]
    fn gaze_down_clamped_to_negative_max_offset() {
        // Gaze well below the view centre.
        let q = gaze_quat(0.0, -HALF_FOV * 2.0);
        let shift = gaze_to_center_shift(q, HALF_FOV, HALF_FOV, DEAD_BAND, MAX_OFFSET);
        assert!(
            (shift[1] - (-MAX_OFFSET)).abs() < 1e-6,
            "pitch clamped to -max_offset, got {}",
            shift[1]
        );
    }

    #[test]
    fn half_fov_maps_to_one_pre_clamp() {
        // With a max_offset of 1.0 (no clamping in this range), a gaze at
        // exactly half_fov to the right should map to +1.0.
        let q = gaze_quat(HALF_FOV, 0.0);
        let shift = gaze_to_center_shift(q, HALF_FOV, HALF_FOV, DEAD_BAND, 1.0);
        assert!(
            (shift[0] - 1.0).abs() < 1e-5,
            "half_fov_x → 1.0, got {}",
            shift[0]
        );
    }

    #[test]
    fn degenerate_zero_fov_returns_centre() {
        // A misconfigured zero half-FOV must not divide by zero.
        let q = gaze_quat(0.5, 0.5);
        let shift = gaze_to_center_shift(q, 0.0, 0.0, DEAD_BAND, MAX_OFFSET);
        assert_eq!(shift, [0.0, 0.0]);
    }

    #[test]
    fn axes_are_independent() {
        // A pure-yaw saccade must not produce any pitch component (and vice
        // versa). Catches sign-mixing or axis-swap bugs in the quat → angle
        // extraction.
        let q = gaze_quat(0.2, 0.0); // gaze 0.2 rad right
        let shift = gaze_to_center_shift(q, HALF_FOV, HALF_FOV, DEAD_BAND, MAX_OFFSET);
        assert!(shift[0] > 0.0, "right gaze → positive center_shift_x");
        assert!(shift[1].abs() < 1e-6, "no pitch → no y shift");

        let q = gaze_quat(0.0, 0.2); // gaze 0.2 rad up
        let shift = gaze_to_center_shift(q, HALF_FOV, HALF_FOV, DEAD_BAND, MAX_OFFSET);
        assert!(shift[1] > 0.0, "up gaze → positive center_shift_y");
        assert!(shift[0].abs() < 1e-6, "no yaw → no x shift");
    }

    #[test]
    fn left_gaze_produces_negative_x_shift() {
        // Cements the sign convention end-to-end: when the user looks left,
        // the foveation inset moves to the left half of the image.
        let q = gaze_quat(-0.3, 0.0);
        let shift = gaze_to_center_shift(q, HALF_FOV, HALF_FOV, DEAD_BAND, MAX_OFFSET);
        assert!(shift[0] < 0.0, "left gaze → negative center_shift_x");
    }
}
