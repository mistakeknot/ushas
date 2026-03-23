//! Halton(2,3) jitter sequence for MetalFX temporal upscaling.
//!
//! Updates the `TemporalJitter` component each frame with the next sample
//! from an 8-element Halton(2,3) sequence, matching Bevy's TAA convention.

use bevy::render::camera::TemporalJitter;
use bevy::prelude::*;

/// Halton(2,3) sequence, 8 samples, centered at 0 (subtract 0.5).
/// Identical to Bevy's built-in TAA sequence.
const HALTON_SEQUENCE: [Vec2; 8] = [
    Vec2::new(0.0, 0.0),
    Vec2::new(0.0, -0.16666666),
    Vec2::new(-0.25, 0.16666669),
    Vec2::new(0.25, -0.3888889),
    Vec2::new(-0.375, -0.055555552),
    Vec2::new(0.125, 0.2777778),
    Vec2::new(-0.125, -0.2777778),
    Vec2::new(0.375, 0.055555582),
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
