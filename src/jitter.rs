//! Halton(2,3) jitter sequence for MetalFX temporal upscaling.
//!
//! Updates the `TemporalJitter` component each frame with the next sample
//! from a 32-element Halton(2,3) sequence, matching Bevy's TAA convention.
//!
//! A longer sequence increases the sub-pixel sampling density the temporal
//! accumulator can integrate over, improving reconstruction quality (and
//! letting render scale drop further for the same perceived sharpness). The
//! first 8 samples are identical to Bevy's built-in 8-phase TAA sequence.

use bevy::prelude::*;
use bevy::render::camera::TemporalJitter;

/// Convert Bevy's perspective jitter into MetalFX's input-pixel coordinates.
///
/// Bevy adds `(2*x/width, -2*y/height)` to the projection's Z column.
/// Perspective division by `-view_z` negates both offsets; converting NDC to
/// Metal's downward-Y viewport negates Y once more. The resulting screen
/// displacement is `(-x, -y)`, which is what Apple's temporal sample supplies.
/// See `docs/research/temporal-quality.md` for the source and derivation.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn metalfx_jitter_offset(jitter: Option<&TemporalJitter>) -> Vec2 {
    jitter.map(|j| -j.offset).unwrap_or(Vec2::ZERO)
}

/// Halton(2,3) sequence, 32 samples, centered at 0 (subtract 0.5).
/// Samples are the Halton points for indices 1..=32 — the leading 8 match
/// Bevy's built-in TAA sequence exactly.
const HALTON_SEQUENCE: [Vec2; 32] = [
    Vec2::new(0.0, -0.16666667),
    Vec2::new(-0.25, 0.16666667),
    Vec2::new(0.25, -0.388_888_9),
    Vec2::new(-0.375, -0.055555556),
    Vec2::new(0.125, 0.277_777_8),
    Vec2::new(-0.125, -0.277_777_8),
    Vec2::new(0.375, 0.055555556),
    Vec2::new(-0.4375, 0.388_888_9),
    Vec2::new(0.0625, -0.46296296),
    Vec2::new(-0.1875, -0.12962963),
    Vec2::new(0.3125, 0.2037037),
    Vec2::new(-0.3125, -0.35185185),
    Vec2::new(0.1875, -0.018518519),
    Vec2::new(-0.0625, 0.314_814_8),
    Vec2::new(0.4375, -0.24074074),
    Vec2::new(-0.46875, 0.092_592_59),
    Vec2::new(0.03125, 0.42592593),
    Vec2::new(-0.21875, -0.42592593),
    Vec2::new(0.28125, -0.092_592_59),
    Vec2::new(-0.34375, 0.24074074),
    Vec2::new(0.15625, -0.314_814_8),
    Vec2::new(-0.09375, 0.018518519),
    Vec2::new(0.40625, 0.35185185),
    Vec2::new(-0.40625, -0.2037037),
    Vec2::new(0.09375, 0.12962963),
    Vec2::new(-0.15625, 0.46296296),
    Vec2::new(0.34375, -0.48765432),
    Vec2::new(-0.28125, -0.15432099),
    Vec2::new(0.21875, 0.17901235),
    Vec2::new(-0.03125, -0.37654321),
    Vec2::new(0.46875, -0.043209877),
    Vec2::new(-0.484375, 0.29012346),
];

/// Update jitter offset each frame using the Halton(2,3) sequence.
pub fn update_jitter(mut frame_count: Local<u32>, mut query: Query<&mut TemporalJitter>) {
    let idx = (*frame_count as usize) % HALTON_SEQUENCE.len();
    let offset = HALTON_SEQUENCE[idx];
    *frame_count = frame_count.wrapping_add(1);

    for mut jitter in query.iter_mut() {
        jitter.offset = offset;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metalfx_jitter_matches_bevy_perspective_screen_displacement() {
        // Apple's sample sends the projected displacement, measured in input
        // pixels with Y pointing down, as MetalFX's jitterOffsetX/Y. Exercise
        // Bevy's actual projection mutation, perspective divide and viewport
        // conversion instead of asserting another copy of the sign formula.
        for size in [Vec2::new(640.0, 360.0), Vec2::new(960.0, 540.0)] {
            let projection =
                Mat4::perspective_infinite_reverse_rh(60.0_f32.to_radians(), size.x / size.y, 0.1);
            for offset in HALTON_SEQUENCE {
                let jitter = TemporalJitter { offset };
                let mut jittered = projection;
                jitter.jitter_projection(&mut jittered, size);
                for point in [Vec3::new(0.0, 0.0, -1.0), Vec3::new(1.0, -0.5, -7.0)] {
                    let to_pixel = |projection: Mat4| {
                        let ndc = projection.project_point3(point);
                        (ndc.truncate() * Vec2::new(0.5, -0.5) + Vec2::splat(0.5)) * size
                    };
                    let displacement = to_pixel(jittered) - to_pixel(projection);
                    let supplied = metalfx_jitter_offset(Some(&jitter));
                    assert!(
                        supplied.abs_diff_eq(displacement, 0.0001),
                        "offset={offset:?}, supplied={supplied:?}, screen={displacement:?}"
                    );
                }
            }
        }
        assert_eq!(metalfx_jitter_offset(None), Vec2::ZERO);
    }

    #[test]
    fn sequence_is_32_phase() {
        assert_eq!(HALTON_SEQUENCE.len(), 32);
    }

    #[test]
    fn samples_are_centered_and_in_range() {
        // Halton(2,3) centered on 0 lands every sample within [-0.5, 0.5],
        // matching Bevy's `TemporalJitter.offset` contract.
        for s in HALTON_SEQUENCE {
            assert!(s.x >= -0.5 && s.x <= 0.5, "x out of range: {}", s.x);
            assert!(s.y >= -0.5 && s.y <= 0.5, "y out of range: {}", s.y);
        }
    }

    #[test]
    fn first_samples_match_bevy_taa() {
        // The leading samples must match Bevy's built-in 8-phase TAA sequence
        // so behaviour is unchanged for the first frames.
        assert!((HALTON_SEQUENCE[0].x - 0.0).abs() < 1e-5);
        assert!((HALTON_SEQUENCE[0].y - (-0.16666667)).abs() < 1e-5);
        assert!((HALTON_SEQUENCE[1].x - (-0.25)).abs() < 1e-5);
        assert!((HALTON_SEQUENCE[2].x - 0.25).abs() < 1e-5);
    }
}
