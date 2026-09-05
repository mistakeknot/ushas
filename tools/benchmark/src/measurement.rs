//! Cohort completion contract: GPU completion is fenced once at each boundary.
//! No per-frame GPU callback, wait, hardware timestamp, or presentation claim.

// BEGIN PURE CONTRACT
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameToken {
    pub epoch: u64,
    pub tick: u32,
    pub frame: u64,
    pub view: u64,
    pub output: [u32; 2],
    pub content: [u32; 2],
    pub scale_bits: u32,
    pub mode: u8,
    pub started_ns: u64,
}

#[derive(Clone, Debug)]
pub struct Proof {
    pub frame: u64,
    pub view: u64,
    pub output: [u32; 2],
    pub content: [u32; 2],
    pub scale_bits: u32,
    pub mode: u8,
    pub output_ready: bool,
    pub target_valid: bool,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Cohort {
    pub epoch: u64,
    pub expected: u32,
    pub opening_ns: u64,
    pub frames: Vec<(FrameToken, Proof)>,
    pub errors: Vec<String>,
    pub closing_ns: Option<u64>,
}

impl Cohort {
    pub fn new(epoch: u64, expected: u32, opening_ns: u64) -> Self {
        let mut result = Self {
            epoch,
            expected,
            opening_ns,
            frames: Vec::new(),
            errors: Vec::new(),
            closing_ns: None,
        };
        if epoch == 0 || expected == 0 || expected > 65_536 {
            result.fail("invalid cohort bounds");
        }
        result
    }

    pub fn fail(&mut self, error: impl Into<String>) {
        if self.errors.len() < 32 {
            self.errors.push(error.into());
        }
    }

    pub fn record(&mut self, token: FrameToken, proof: Proof) {
        if self.frames.len() >= 65_536 {
            self.fail("frame retention exhausted");
            return;
        }
        if self.closing_ns.is_some() {
            self.fail("frame after cohort closure");
        }
        if token.epoch != self.epoch
            || token.tick as usize != self.frames.len()
            || token.tick >= self.expected
        {
            self.fail("noncontiguous tick or wrong configuration epoch");
        }
        if token.started_ns < self.opening_ns {
            self.fail("tick started before opening completion");
        }
        if let Some((previous, _)) = self.frames.last() {
            if token.frame <= previous.frame
                || token.started_ns < previous.started_ns
                || token.view != previous.view
                || token.output != previous.output
                || token.content != previous.content
                || token.mode != previous.mode
                || token.scale_bits != previous.scale_bits
            {
                self.fail("frame identity, target, clock or configuration changed inside cohort");
            }
        }
        if token.frame != proof.frame
            || token.view != proof.view
            || token.output != proof.output
            || token.content != proof.content
            || token.scale_bits != proof.scale_bits
            || token.mode != proof.mode
            || !proof.output_ready
            || !proof.target_valid
        {
            self.fail(format!(
                "unqualified frame {}: {}",
                token.frame, proof.reason
            ));
        }
        self.frames.push((token, proof));
    }

    pub fn close(&mut self, observed_ns: u64) {
        if self.closing_ns.replace(observed_ns).is_some() {
            self.fail("duplicate closing fence");
        }
        if self.frames.len() != self.expected as usize {
            self.fail("closing fence with incomplete cohort membership");
        }
        if self
            .frames
            .last()
            .is_none_or(|(token, _)| observed_ns <= token.started_ns)
        {
            self.fail("invalid closing timestamp");
        }
    }

    pub fn seconds(&self) -> Option<f64> {
        let first = self.frames.first()?.0.started_ns;
        self.closing_ns?
            .checked_sub(first)
            .filter(|n| *n > 0)
            .map(|n| n as f64 / 1e9)
    }

    pub fn fps(&self) -> Option<f64> {
        if !self.errors.is_empty() || self.frames.len() != self.expected as usize {
            return None;
        }
        self.seconds().map(|s| self.expected as f64 / s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair(tick: u32) -> (FrameToken, Proof) {
        let token = FrameToken {
            epoch: 1,
            tick,
            frame: u64::from(tick) + 10,
            view: 7,
            output: [2560, 1440],
            content: [1280, 720],
            scale_bits: 0.5f32.to_bits(),
            mode: 1,
            started_ns: 100 + u64::from(tick) * 1_000_000_000,
        };
        let proof = Proof {
            frame: token.frame,
            view: 7,
            output: token.output,
            content: token.content,
            scale_bits: token.scale_bits,
            mode: 1,
            output_ready: true,
            target_valid: true,
            reason: String::new(),
        };
        (token, proof)
    }
    #[test]
    fn pipelined_cohort_needs_only_closing_fence() {
        let mut c = Cohort::new(1, 2, 50);
        let (t, p) = pair(0);
        c.record(t, p);
        let (t, p) = pair(1);
        c.record(t, p);
        assert_eq!(c.fps(), None);
        c.close(2_000_000_100);
        assert_eq!(c.fps(), Some(1.0));
    }
    #[test]
    fn missing_duplicate_and_wrong_epoch_ticks_fail() {
        for case in 0..3 {
            let mut c = Cohort::new(1, 2, 50);
            let (t, p) = pair(0);
            c.record(t, p);
            let (mut t, p) = pair(if case == 0 { 0 } else { 1 });
            if case == 1 {
                t.epoch = 2;
            }
            if case < 2 {
                c.record(t, p);
            }
            c.close(3_000_000_100);
            assert_eq!(c.fps(), None);
        }
    }
    #[test]
    fn stale_proof_wrong_target_and_changed_configuration_fail() {
        for case in 0..5 {
            let mut c = Cohort::new(1, 1, 50);
            let (t, mut p) = pair(0);
            match case {
                0 => p.frame -= 1,
                1 => p.view += 1,
                2 => p.output = [1280, 720],
                3 => p.output_ready = false,
                _ => p.target_valid = false,
            }
            c.record(t, p);
            c.close(1000);
            assert_eq!(c.fps(), None);
        }
        let mut c = Cohort::new(1, 2, 50);
        let (t, p) = pair(0);
        c.record(t, p);
        let (mut t, mut p) = pair(1);
        t.scale_bits = 1.0f32.to_bits();
        p.scale_bits = t.scale_bits;
        c.record(t, p);
        c.close(3_000_000_100);
        assert_eq!(c.fps(), None);
    }
    #[test]
    fn timeout_cancellation_and_bad_boundary_cannot_score() {
        for case in 0..3 {
            let mut c = Cohort::new(1, 1, if case == 2 { 101 } else { 50 });
            let (t, p) = pair(0);
            c.record(t, p);
            if case < 2 {
                c.fail(if case == 0 { "timeout" } else { "cancelled" });
            }
            c.close(1000);
            assert_eq!(c.fps(), None);
        }
    }
    #[test]
    fn duplicate_closure_or_late_frame_invalidates_score() {
        let mut c = Cohort::new(1, 1, 50);
        let (t, p) = pair(0);
        c.record(t, p);
        c.close(1000);
        assert!(c.fps().is_some());
        c.close(1001);
        assert_eq!(c.fps(), None);
    }
}
// END PURE CONTRACT
