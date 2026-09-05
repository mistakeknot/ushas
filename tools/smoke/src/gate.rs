//! Arm identity and distinct-frame readiness, independent of loop frequency.
use bevy_metalfx::{MetalFxEffectObservation, MetalFxEffectState, MetalFxMode};

pub fn mode(name: &str) -> MetalFxMode {
    match name {
        "spatial" => MetalFxMode::Spatial,
        "temporal" => MetalFxMode::Temporal,
        "interpolate" => MetalFxMode::FrameInterpolation,
        _ => MetalFxMode::Disabled,
    }
}

pub fn arm_matches(
    o: &MetalFxEffectObservation,
    requested: MetalFxMode,
    scale: f32,
    output: [u32; 2],
) -> bool {
    let content = output.map(|v| (v as f32 * scale).round().max(1.0) as u32);
    o.requested_mode == requested
        && o.effective_mode == requested
        && o.output_size == output
        && o.content_size == content
        && (o.requested_scale - scale).abs() < 0.0001
        && o.state
            == if requested == MetalFxMode::Disabled {
                MetalFxEffectState::Disabled
            } else {
                MetalFxEffectState::OutputWritten
            }
}

#[derive(Default)]
pub struct Readiness {
    pub count: usize,
    last: Option<u64>,
}

impl Readiness {
    pub fn observe(&mut self, ready: bool, frame: Option<u64>) {
        if !ready {
            self.count = 0;
            self.last = None;
            return;
        }
        if let Some(frame) = frame {
            if self.last.is_none_or(|previous| frame > previous) {
                self.count += 1;
                self.last = Some(frame);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observation() -> MetalFxEffectObservation {
        MetalFxEffectObservation::new(
            1,
            1,
            MetalFxMode::Temporal,
            MetalFxMode::Temporal,
            0.5,
            [640, 360],
            [1280, 720],
            MetalFxEffectState::OutputWritten,
            None,
        )
    }
    #[test]
    fn rejects_fallback_and_incorrect_dimensions() {
        let mut o = observation();
        assert!(arm_matches(&o, MetalFxMode::Temporal, 0.5, [1280, 720]));
        o.effective_mode = MetalFxMode::Spatial;
        assert!(!arm_matches(&o, MetalFxMode::Temporal, 0.5, [1280, 720]));
        o = observation();
        o.content_size = [1280, 720];
        assert!(!arm_matches(&o, MetalFxMode::Temporal, 0.5, [1280, 720]));
        o = observation();
        o.output_size = [1920, 1080];
        assert!(!arm_matches(&o, MetalFxMode::Temporal, 0.5, [1280, 720]));
    }
    #[test]
    fn readiness_counts_rendered_frames_not_repeated_main_loops() {
        let mut r = Readiness::default();
        for _ in 0..100 {
            r.observe(true, Some(1));
        }
        assert_eq!(r.count, 1);
        r.observe(true, Some(2));
        assert_eq!(r.count, 2);
        r.observe(false, Some(3));
        assert_eq!(r.count, 0);
        r.observe(true, Some(4));
        assert_eq!(r.count, 1);
    }
}
