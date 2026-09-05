//! CPU-only gate copied into the frozen consumer, never into its live worktree.
use std::time::Duration;

pub struct ImageEvidence {
    pub pixel_count: usize,
    pub opaque_pixels: usize,
    pub visible_pixels: usize,
    pub sampled_colors: usize,
    pub valid: bool,
}

pub fn analyze_image(
    width: u32,
    height: u32,
    pixels: impl Iterator<Item = [u8; 4]>,
) -> ImageEvidence {
    let mut colors = std::collections::HashSet::new();
    let mut opaque = 0;
    let mut visible = 0;
    let mut count = 0;
    for (index, pixel) in pixels.enumerate() {
        count += 1;
        opaque += usize::from(pixel[3] == 255);
        visible += usize::from(pixel[..3].iter().any(|channel| *channel > 16));
        if index % 16 == 0 {
            colors.insert([pixel[0], pixel[1], pixel[2]]);
        }
    }
    ImageEvidence {
        pixel_count: count,
        opaque_pixels: opaque,
        visible_pixels: visible,
        sampled_colors: colors.len(),
        valid: width == 1600
            && height == 900
            && count == 1_440_000
            && opaque == count
            && colors.len() >= 64
            && visible >= 14_400,
    }
}

#[derive(Default)]
pub struct Readiness {
    pub distinct_ready: u64,
    pub started: bool,
    last_frame: Option<u64>,
}

impl Readiness {
    pub fn advance(&mut self, elapsed: Duration, observation: Option<u64>, ready: bool) -> bool {
        if !ready || observation.is_none() {
            if !self.started {
                self.distinct_ready = 0;
            }
            return false;
        }
        let frame = observation.unwrap();
        if self.last_frame.is_some_and(|last| frame <= last) {
            return false;
        }
        self.last_frame = Some(frame);
        self.distinct_ready += 1;
        self.started |= self.distinct_ready >= 20 && elapsed >= Duration::from_secs(3);
        self.started
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompletionAction {
    Wait,
    CaptureWarmup,
    BeginMeasure,
    BeginDrain,
    CaptureFinal,
    Finish,
}

#[derive(Default)]
enum CompletionPhase {
    #[default]
    Waiting,
    WarmupImage,
    AdoptingMeasure,
    Measuring(Duration),
    AdoptingDrain,
    FinalImage,
    Finished,
}

#[derive(Default)]
pub struct CompletionGate {
    phase: CompletionPhase,
}

impl CompletionGate {
    /// The timer starts after the render world has drained/adopted Measure.
    /// An unready measured frame stays in the ledger; it cannot pause the timer.
    pub fn advance(
        &mut self,
        elapsed: Duration,
        adopted_epoch: Option<u64>,
        ready: bool,
        warmup_image_valid: bool,
        final_image_valid: bool,
    ) -> CompletionAction {
        use CompletionAction as Action;
        use CompletionPhase as Phase;
        match self.phase {
            Phase::Waiting if ready && adopted_epoch == Some(1) => {
                self.phase = Phase::WarmupImage;
                Action::CaptureWarmup
            }
            Phase::WarmupImage if ready && warmup_image_valid => {
                self.phase = Phase::AdoptingMeasure;
                Action::BeginMeasure
            }
            Phase::AdoptingMeasure if adopted_epoch == Some(2) => {
                self.phase = Phase::Measuring(elapsed);
                Action::Wait
            }
            Phase::Measuring(start) if elapsed.saturating_sub(start) >= Duration::from_secs(6) => {
                self.phase = Phase::AdoptingDrain;
                Action::BeginDrain
            }
            Phase::AdoptingDrain if adopted_epoch == Some(3) => {
                self.phase = Phase::FinalImage;
                Action::CaptureFinal
            }
            Phase::FinalImage if final_image_valid => {
                self.phase = Phase::Finished;
                Action::Finish
            }
            _ => Action::Wait,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_measurement_waits_for_image_and_epoch_then_drains_before_final_capture() {
        let mut gate = CompletionGate::default();
        assert_eq!(
            gate.advance(Duration::ZERO, None, true, false, false),
            CompletionAction::Wait
        );
        assert_eq!(
            gate.advance(Duration::ZERO, Some(1), false, false, false),
            CompletionAction::Wait
        );
        assert_eq!(
            gate.advance(Duration::from_secs(3), Some(1), true, false, false),
            CompletionAction::CaptureWarmup
        );
        assert_eq!(
            gate.advance(Duration::from_secs(4), Some(1), true, false, false),
            CompletionAction::Wait
        );
        assert_eq!(
            gate.advance(Duration::from_secs(4), Some(1), true, true, false),
            CompletionAction::BeginMeasure
        );
        assert_eq!(
            gate.advance(Duration::from_secs(20), Some(1), true, true, false),
            CompletionAction::Wait
        );
        assert_eq!(
            gate.advance(Duration::from_secs(21), Some(2), true, true, false),
            CompletionAction::Wait
        );
        assert_eq!(
            gate.advance(Duration::from_secs(26), Some(2), true, true, false),
            CompletionAction::Wait
        );
        // Losing readiness cannot hide bad measured frames or postpone the drain.
        assert_eq!(
            gate.advance(Duration::from_secs(27), Some(2), false, true, false),
            CompletionAction::BeginDrain
        );
        assert_eq!(
            gate.advance(Duration::from_secs(28), Some(2), true, true, false),
            CompletionAction::Wait
        );
        assert_eq!(
            gate.advance(Duration::from_secs(29), Some(3), true, true, false),
            CompletionAction::CaptureFinal
        );
        assert_eq!(
            gate.advance(Duration::from_secs(30), Some(3), true, true, false),
            CompletionAction::Wait
        );
        assert_eq!(
            gate.advance(Duration::from_secs(31), Some(3), true, true, true),
            CompletionAction::Finish
        );
        assert_eq!(
            gate.advance(Duration::from_secs(32), Some(3), true, true, true),
            CompletionAction::Wait
        );
    }

    #[test]
    fn varied_rgb_with_zero_alpha_is_not_a_rendered_image() {
        let pixels = |alpha| {
            (0..1_440_000usize).map(move |i| [(i % 251) as u8, ((i / 251) % 251) as u8, 80, alpha])
        };
        let opaque = analyze_image(1600, 900, pixels(255));
        assert!(opaque.valid);
        assert_eq!(opaque.opaque_pixels, 1_440_000);
        let transparent = analyze_image(1600, 900, pixels(0));
        assert!(transparent.sampled_colors >= 64);
        assert!(transparent.visible_pixels >= 14_400);
        assert_eq!(transparent.opaque_pixels, 0);
        assert!(!transparent.valid);
        assert!(!analyze_image(800, 450, pixels(255)).valid);
    }

    #[test]
    fn twenty_distinct_ready_observations_and_wall_warmup_are_both_required() {
        let mut gate = Readiness::default();
        for frame in 0..19 {
            assert!(!gate.advance(Duration::from_secs(4), Some(frame), true));
        }
        assert!(!gate.advance(Duration::from_secs(4), Some(18), true));
        assert_eq!(gate.distinct_ready, 19);
        assert!(gate.advance(Duration::from_secs(4), Some(19), true));
        let mut early = Readiness::default();
        for frame in 0..40 {
            assert!(!early.advance(Duration::from_secs(1), Some(frame), true));
        }
        assert!(early.advance(Duration::from_secs(3), Some(40), true));
    }

    #[test]
    fn missing_failed_or_repeated_evidence_cannot_advance_the_script() {
        let mut gate = Readiness::default();
        for frame in 0..20 {
            gate.advance(Duration::from_secs(4), Some(frame), true);
        }
        assert!(!gate.advance(Duration::from_secs(4), Some(19), true));
        assert!(!gate.advance(Duration::from_secs(4), Some(18), true));
        assert!(!gate.advance(Duration::from_secs(4), None, false));
        assert!(!gate.advance(Duration::from_secs(4), Some(20), false));
        assert!(gate.advance(Duration::from_secs(4), Some(21), true));
    }

    #[test]
    fn preparation_failure_breaks_the_consecutive_readiness_streak() {
        let mut gate = Readiness::default();
        for frame in 0..19 {
            gate.advance(Duration::from_secs(4), Some(frame), true);
        }
        gate.advance(Duration::from_secs(4), Some(19), false);
        assert_eq!(gate.distinct_ready, 0);
        assert!(!gate.advance(Duration::from_secs(4), Some(20), true));
    }
}
