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

use crate::PerViewFoveationView;
use alvr_common::{
    ViewParams,
    glam::{Quat, Vec3},
};
use alvr_packets::FaceData;
use alvr_session::{FoveatedEncodingConfig, PerViewFoveationConfig};
use std::time::{Duration, Instant};

/// Dead band on raw gaze angles before the normalisation step. ~2° matches
/// the scoping doc's recommendation and natural saccade granularity.
const DEFAULT_DEAD_BAND_RAD: f32 = 0.035;

/// Extract a symmetric half-FOV (radians) from a `ViewParams` with potentially
/// asymmetric `Fov { left, right, up, down }`. Uses the larger of each axis's
/// two magnitudes so the gaze can reach the full extent of the view on either
/// side without saturating prematurely. Returns `[half_fov_x, half_fov_y]`.
fn symmetric_half_fov(params: &ViewParams) -> [f32; 2] {
    let fov = params.fov;
    [
        fov.left.abs().max(fov.right.abs()),
        fov.up.abs().max(fov.down.abs()),
    ]
}

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

/// Rate-limited producer of [`crate::ServerCoreEvent::PerViewFoveation`].
/// Lives inside the tracking loop and consumes raw `FaceData.eyes_*` samples,
/// applies [`gaze_to_center_shift`], and decides whether enough time has
/// elapsed since the last emit to publish another update.
///
/// State here is intentionally minimal — just the last-emit timestamp. The
/// emitter does not do any time-series smoothing of the gaze itself; the
/// `gaze_to_center_shift` dead band suppresses micro-jitter, and any further
/// filtering (one-Euro, EMA, etc.) is a follow-up if real eye-tracking data
/// turns out to be too jittery in practice.
pub struct PerViewFoveationEmitter {
    last_emit: Option<Instant>,
}

impl Default for PerViewFoveationEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl PerViewFoveationEmitter {
    pub fn new() -> Self {
        Self { last_emit: None }
    }

    /// Decide whether an emit is warranted this tick. Returns `Some(views)`
    /// when the rate-limit window has elapsed *and* the client supplied a
    /// gaze sample for the resolved frame; `None` otherwise.
    ///
    /// View ordering: index 0 = left, index 1 = right (matches
    /// `LocalViewParams` and the bridge's `AlvrOxrFoveationView` cache).
    /// `view_params` carries the client's negotiated per-view FOV so the
    /// gaze → normalised-offset projection uses the right denominator per
    /// side; pass `[ViewParams::DUMMY; 2]` before the client has reported its
    /// view config (the DUMMY values produce a sensible ±1 rad half-FOV).
    ///
    /// Gaze source preference matches `face::FaceTrackingSink::send_tracking`:
    /// per-eye `eyes_social` when both sides are present, otherwise the
    /// shared `eyes_combined` quaternion applied to both views.
    pub fn maybe_compute(
        &mut self,
        face: &FaceData,
        static_config: &FoveatedEncodingConfig,
        per_view_config: &PerViewFoveationConfig,
        view_params: &[ViewParams; 2],
        now: Instant,
    ) -> Option<[PerViewFoveationView; 2]> {
        let min_interval = Duration::from_secs_f32(
            // Guard against a degenerate / hostile rate config; 0.1 Hz is the
            // floor (one update every 10s — generous enough that an angry
            // session.json still spits out updates rather than going silent).
            1.0 / per_view_config.update_rate_hz.max(0.1),
        );
        if let Some(last) = self.last_emit
            && now.duration_since(last) < min_interval
        {
            return None;
        }

        let gazes: [Option<Quat>; 2] = if let [Some(left), Some(right)] = face.eyes_social {
            [Some(left), Some(right)]
        } else if let Some(combined) = face.eyes_combined {
            [Some(combined), Some(combined)]
        } else {
            return None;
        };

        let static_view = PerViewFoveationView {
            center_size: [static_config.center_size_x, static_config.center_size_y],
            center_shift: [static_config.center_shift_x, static_config.center_shift_y],
            edge_ratio: [static_config.edge_ratio_x, static_config.edge_ratio_y],
        };

        let half_fovs = [
            symmetric_half_fov(&view_params[0]),
            symmetric_half_fov(&view_params[1]),
        ];

        let view_for = |gaze: Quat, half_fov: [f32; 2]| PerViewFoveationView {
            center_size: static_view.center_size,
            center_shift: gaze_to_center_shift(
                gaze,
                half_fov[0],
                half_fov[1],
                DEFAULT_DEAD_BAND_RAD,
                per_view_config.max_offset_normalized,
            ),
            edge_ratio: static_view.edge_ratio,
        };

        let views = [
            gazes[0]
                .map(|g| view_for(g, half_fovs[0]))
                .unwrap_or(static_view),
            gazes[1]
                .map(|g| view_for(g, half_fovs[1]))
                .unwrap_or(static_view),
        ];

        self.last_emit = Some(now);
        Some(views)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alvr_common::glam::EulerRot;
    use alvr_session::settings_schema::Switch;

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

    // ----- PerViewFoveationEmitter tests ---------------------------------

    fn default_static_config() -> FoveatedEncodingConfig {
        FoveatedEncodingConfig {
            force_enable: false,
            center_size_x: 0.45,
            center_size_y: 0.4,
            center_shift_x: 0.4,
            center_shift_y: 0.1,
            edge_ratio_x: 4.0,
            edge_ratio_y: 5.0,
            per_view_eye_tracked: Switch::Disabled,
        }
    }

    fn per_view_cfg(update_rate_hz: f32) -> PerViewFoveationConfig {
        PerViewFoveationConfig {
            update_rate_hz,
            max_offset_normalized: 0.25,
            confidence_floor: 0.5,
        }
    }

    fn face_with_combined(q: Quat) -> FaceData {
        FaceData {
            eyes_combined: Some(q),
            eyes_social: [None, None],
            face_expressions: None,
        }
    }

    fn face_with_social(left: Quat, right: Quat) -> FaceData {
        FaceData {
            eyes_combined: None,
            eyes_social: [Some(left), Some(right)],
            face_expressions: None,
        }
    }

    #[test]
    fn emitter_skips_when_no_gaze() {
        let mut emitter = PerViewFoveationEmitter::new();
        let face = FaceData {
            eyes_combined: None,
            eyes_social: [None, None],
            face_expressions: None,
        };
        let out =
            emitter.maybe_compute(
                &face,
                &default_static_config(),
                &per_view_cfg(10.0),
                &[ViewParams::DUMMY; 2],
                Instant::now(),
            );
        assert!(out.is_none(), "no gaze → no event");
    }

    #[test]
    fn emitter_uses_combined_when_social_absent() {
        let mut emitter = PerViewFoveationEmitter::new();
        let face = face_with_combined(gaze_quat(0.2, 0.0));
        let views = emitter
            .maybe_compute(
                &face,
                &default_static_config(),
                &per_view_cfg(10.0),
                &[ViewParams::DUMMY; 2],
                Instant::now(),
            )
            .expect("combined gaze → event");
        // Both views share the same gaze → identical center_shift.
        assert_eq!(views[0].center_shift, views[1].center_shift);
        assert!(views[0].center_shift[0] > 0.0, "right gaze → positive shift");
    }

    #[test]
    fn emitter_prefers_social_when_both_present() {
        let mut emitter = PerViewFoveationEmitter::new();
        // Left eye looks slightly left, right eye slightly right (divergent
        // gaze, e.g. focusing on something close).
        let face = face_with_social(gaze_quat(-0.1, 0.0), gaze_quat(0.1, 0.0));
        let views = emitter
            .maybe_compute(
                &face,
                &default_static_config(),
                &per_view_cfg(10.0),
                &[ViewParams::DUMMY; 2],
                Instant::now(),
            )
            .expect("social gaze → event");
        assert!(
            views[0].center_shift[0] < 0.0 && views[1].center_shift[0] > 0.0,
            "per-view shifts must diverge with eyes_social: got {:?}",
            views.map(|v| v.center_shift)
        );
    }

    #[test]
    fn emitter_rate_limits() {
        let mut emitter = PerViewFoveationEmitter::new();
        let face = face_with_combined(gaze_quat(0.2, 0.0));
        let cfg = per_view_cfg(10.0); // 100 ms interval
        let t0 = Instant::now();

        assert!(
            emitter
                .maybe_compute(
                    &face,
                    &default_static_config(),
                    &cfg,
                    &[ViewParams::DUMMY; 2],
                    t0,
                )
                .is_some(),
            "first call should emit"
        );
        // 50 ms later — inside the rate-limit window.
        assert!(
            emitter
                .maybe_compute(
                    &face,
                    &default_static_config(),
                    &cfg,
                    &[ViewParams::DUMMY; 2],
                    t0 + Duration::from_millis(50),
                )
                .is_none(),
            "second call within window must be suppressed"
        );
        // 150 ms later — past the 100 ms window.
        assert!(
            emitter
                .maybe_compute(
                    &face,
                    &default_static_config(),
                    &cfg,
                    &[ViewParams::DUMMY; 2],
                    t0 + Duration::from_millis(150),
                )
                .is_some(),
            "third call past window emits again"
        );
    }

    #[test]
    fn emitter_honours_per_side_half_fov() {
        // A view with a *smaller* half-FOV maps the same gaze yaw to a
        // *larger* normalised offset — that's the whole point of plumbing
        // real FOV. Pre-clamp at max_offset=1.0 so we can read the
        // proportionality directly.
        let mut emitter = PerViewFoveationEmitter::new();
        let face = face_with_social(gaze_quat(0.3, 0.0), gaze_quat(0.3, 0.0));
        let mut narrow = ViewParams::DUMMY;
        narrow.fov = alvr_common::Fov {
            left: -0.5,
            right: 0.5,
            up: 0.5,
            down: -0.5,
        };
        let mut wide = ViewParams::DUMMY;
        wide.fov = alvr_common::Fov {
            left: -1.5,
            right: 1.5,
            up: 1.5,
            down: -1.5,
        };
        let views = emitter
            .maybe_compute(
                &face,
                &default_static_config(),
                &PerViewFoveationConfig {
                    update_rate_hz: 10.0,
                    max_offset_normalized: 1.0,
                    confidence_floor: 0.5,
                },
                &[narrow, wide],
                Instant::now(),
            )
            .expect("event");
        assert!(
            views[0].center_shift[0] > views[1].center_shift[0],
            "narrower view (half_fov=0.5) → larger normalised offset than wider view (half_fov=1.5); got {:?}",
            views.map(|v| v.center_shift[0]),
        );
    }

    #[test]
    fn emitter_propagates_static_center_size_and_edge_ratio() {
        let mut emitter = PerViewFoveationEmitter::new();
        let face = face_with_combined(Quat::IDENTITY);
        let views = emitter
            .maybe_compute(
                &face,
                &default_static_config(),
                &per_view_cfg(10.0),
                &[ViewParams::DUMMY; 2],
                Instant::now(),
            )
            .expect("event with identity gaze");
        let cfg = default_static_config();
        // Identity gaze → zero center_shift, but center_size and edge_ratio
        // must be passed through from the static config unchanged.
        for view in &views {
            assert_eq!(view.center_size, [cfg.center_size_x, cfg.center_size_y]);
            assert_eq!(view.edge_ratio, [cfg.edge_ratio_x, cfg.edge_ratio_y]);
            assert!(view.center_shift[0].abs() < 1e-6 && view.center_shift[1].abs() < 1e-6);
        }
    }
}
