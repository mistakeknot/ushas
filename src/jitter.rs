//! Halton(2,3) jitter sequence for MetalFX temporal upscaling.
//!
//! Updates the `TemporalJitter` component each frame with the next sample
//! from a 32-element Halton(2,3) sequence, matching Bevy's TAA convention.
//!
//! A longer sequence increases the sub-pixel sampling density the temporal
//! accumulator can integrate over, improving reconstruction quality (and
//! letting render scale drop further for the same perceived sharpness). The
//! first 8 samples are identical to Bevy's built-in 8-phase TAA sequence.

use bevy::render::camera::TemporalJitter;
use bevy::prelude::*;

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
pub fn update_jitter(
    mut frame_count: Local<u32>,
    mut query: Query<&mut TemporalJitter>,
) {
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
