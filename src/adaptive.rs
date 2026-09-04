//! Deterministic, time-based policy for adaptive render resolution.
//!
//! This module does not measure the GPU or infer GPU pressure from app cadence.
//! Its adapter must supply fresh, validated **frame-cost** measurements. A GPU
//! timestamp around the MetalFX pass alone is not a suitable input.

use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
/// Frame budget, quality floor, and time-based controller policy.
pub struct AdaptiveConfig {
    /// Intended rendered frames per second; independent of observation cadence.
    pub target_fps: f64,
    /// Lowest acceptable render scale. Only ladder rungs at or above it are used.
    pub minimum_scale: f32,
    /// Exponential smoothing time constant, in wall-clock time.
    pub smoothing_time: Duration,
    /// Continuous smoothed overload required before one downward step.
    pub over_budget_for: Duration,
    /// Continuous smoothed headroom required before one upward step.
    pub headroom_for: Duration,
    /// Fresh, ready observations required after a transition before decisions.
    pub settling_time: Duration,
    /// Maximum measurement age and gap between consecutive usable samples.
    pub max_sample_age: Duration,
    /// Overload threshold as a multiple of `1000 / target_fps` milliseconds.
    pub over_budget_ratio: f64,
    /// Headroom threshold as a fraction of the same budget; must be below one.
    pub headroom_ratio: f64,
    /// Required fractional GPU-cost reduction after each downward step.
    pub minimum_downshift_benefit: f64,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            target_fps: 60.0,
            minimum_scale: 0.5,
            smoothing_time: Duration::from_millis(250),
            over_budget_for: Duration::from_millis(500),
            headroom_for: Duration::from_secs(2),
            settling_time: Duration::from_millis(250),
            max_sample_age: Duration::from_millis(250),
            over_budget_ratio: 1.05,
            headroom_ratio: 0.75,
            minimum_downshift_benefit: 0.08,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaptiveConfigError {
    InvalidTarget,
    InvalidMinimumScale,
    InvalidLadder,
    NoAllowedScale,
    InvalidStartingScale,
    InvalidTiming,
    InvalidThresholds,
}

impl std::fmt::Display for AdaptiveConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidTarget => "target FPS must give a positive finite frame budget",
            Self::InvalidMinimumScale => "minimum scale must be finite and in (0, 1]",
            Self::InvalidLadder => {
                "scale ladder must be nonempty, strictly ascending, and in (0, 1]"
            }
            Self::NoAllowedScale => "scale ladder has no rung at or above the quality floor",
            Self::InvalidStartingScale => "starting scale must be finite and in (0, 1]",
            Self::InvalidTiming => "smoothing, evidence, and sample-age intervals must be nonzero",
            Self::InvalidThresholds => {
                "overload ratio must be >= 1; headroom and benefit fractions must be in (0, 1)"
            }
        })
    }
}

impl std::error::Error for AdaptiveConfigError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaptiveSampleValidity {
    /// The adapter has validated this signal's frame coverage and GPU semantics.
    ValidatedFrameCost,
    Unavailable,
    UnvalidatedScope,
}

#[derive(Clone, Copy, Debug)]
pub struct AdaptiveObservation {
    /// Monotonic time at observation delivery, relative to an arbitrary origin.
    pub now: Duration,
    /// Monotonically increasing camera/mode generation, not a render-scale ID.
    /// Older generations are rejected; a newer generation clears learned costs.
    pub epoch: u64,
    /// Strictly increasing within an epoch; repeated callbacks are not evidence.
    pub frame_id: u64,
    /// Monotonic measurement time, using the same clock origin as `now`.
    pub sampled_at: Duration,
    /// Scale that produced this measurement, not the latest requested scale.
    pub sampled_scale: f32,
    /// GPU frame cost in milliseconds; missing is not zero cost.
    pub gpu_ms: Option<f64>,
    pub validity: AdaptiveSampleValidity,
    /// Whether the requested effect is currently producing usable output.
    pub effect_ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaptiveReason {
    Settling,
    Unavailable,
    UnvalidatedScope,
    InvalidSample,
    StaleSample,
    OutOfOrderSample,
    ScaleMismatch,
    EffectPending,
    WithinBudget,
    Headroom,
    OverBudget,
    BudgetInfeasible,
    NoMeasuredBenefit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveDecision {
    pub scale: f32,
    pub changed: bool,
    pub reason: AdaptiveReason,
    /// Smoothed validated frame cost, or `None` after evidence invalidation.
    pub gpu_ms: Option<f64>,
    pub budget_ms: f64,
}

#[derive(Debug)]
pub struct AdaptiveController {
    config: AdaptiveConfig,
    ladder: Vec<f32>,
    index: usize,
    floor: usize,
    epoch: Option<u64>,
    last_now: Option<Duration>,
    last_sample: Option<(u64, Duration)>,
    last_valid_at: Option<Duration>,
    ready_since: Option<Duration>,
    smoothed_ms: Option<f64>,
    overload_for: Duration,
    headroom_for: Duration,
    trial_for: Duration,
    downward_trial: Option<DownwardTrial>,
    no_benefit_cost: Option<f64>,
    comparisons: Vec<Option<RungComparison>>,
}

#[derive(Clone, Copy, Debug)]
struct DownwardTrial {
    previous_index: usize,
    previous_ms: f64,
}

#[derive(Clone, Copy, Debug)]
struct RungComparison {
    upper_ms: f64,
    lower_ms: f64,
}

impl AdaptiveController {
    /// Validate the policy and snap the starting scale to the nearest allowed
    /// ladder rung. Equidistant rungs favor the higher quality.
    pub fn new(
        config: AdaptiveConfig,
        ladder: Vec<f32>,
        starting_scale: f32,
    ) -> Result<Self, AdaptiveConfigError> {
        if ladder.is_empty()
            || ladder
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0 || *v > 1.0)
            || ladder.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(AdaptiveConfigError::InvalidLadder);
        }
        if !starting_scale.is_finite() || starting_scale <= 0.0 || starting_scale > 1.0 {
            return Err(AdaptiveConfigError::InvalidStartingScale);
        }
        let floor = Self::validate_config(&config, &ladder)?;
        let index = (floor..ladder.len())
            .rev()
            .min_by(|a, b| {
                (ladder[*a] - starting_scale)
                    .abs()
                    .total_cmp(&(ladder[*b] - starting_scale).abs())
            })
            .expect("validated ladder is nonempty");
        let comparisons = vec![None; ladder.len()];
        Ok(Self {
            config,
            ladder,
            index,
            floor,
            comparisons,
            epoch: None,
            last_now: None,
            last_sample: None,
            last_valid_at: None,
            ready_since: None,
            smoothed_ms: None,
            overload_for: Duration::ZERO,
            headroom_for: Duration::ZERO,
            trial_for: Duration::ZERO,
            downward_trial: None,
            no_benefit_cost: None,
        })
    }

    pub fn current_scale(&self) -> f32 {
        self.ladder[self.index]
    }

    /// Apply a new target or quality policy atomically. Invalid settings leave
    /// the controller unchanged. Raising the floor immediately raises the scale
    /// to the first allowed rung; changing policy clears all prior evidence.
    pub fn update_config(&mut self, config: AdaptiveConfig) -> Result<(), AdaptiveConfigError> {
        let floor = Self::validate_config(&config, &self.ladder)?;
        if self.config == config {
            return Ok(());
        }
        self.floor = floor;
        self.index = self.index.max(floor);
        self.config = config;
        self.reset();
        Ok(())
    }

    /// Start a fresh measurement period after a pause or workload change.
    /// Preserve sample identity so delayed/duplicate callbacks remain rejected.
    /// Keep this separate from normal scaler rebuilds: `effect_ready = false`
    /// pauses evidence while preserving the pending benefit comparison.
    pub fn reset(&mut self) {
        self.clear_evidence();
        self.downward_trial = None;
        self.no_benefit_cost = None;
        self.comparisons.fill(None);
    }

    /// Consume one delivery. Every decision depends on elapsed measurement time,
    /// never on a fixed number of frames. Invalid/missing samples cannot lower
    /// quality. A downshift that fails to reduce measured cost is reversed; a
    /// 25% workload-cost change or explicit reset allows that trial again.
    pub fn observe(&mut self, observation: AdaptiveObservation) -> AdaptiveDecision {
        let before = self.index;
        if self.last_now.is_some_and(|last| observation.now < last)
            || self.epoch.is_some_and(|epoch| observation.epoch < epoch)
        {
            return self.decision(before, AdaptiveReason::OutOfOrderSample);
        }
        self.last_now = Some(observation.now);
        if self.epoch != Some(observation.epoch) {
            self.reset();
            self.epoch = Some(observation.epoch);
            self.last_sample = None;
        }
        let rejected = if !observation.effect_ready {
            Some(AdaptiveReason::EffectPending)
        } else if observation.validity == AdaptiveSampleValidity::Unavailable
            || observation.gpu_ms.is_none()
        {
            Some(AdaptiveReason::Unavailable)
        } else if observation.validity == AdaptiveSampleValidity::UnvalidatedScope {
            Some(AdaptiveReason::UnvalidatedScope)
        } else if observation.sampled_at > observation.now
            || !observation
                .gpu_ms
                .is_some_and(|ms| ms.is_finite() && ms > 0.0)
            || !observation.sampled_scale.is_finite()
        {
            Some(AdaptiveReason::InvalidSample)
        } else if observation.now - observation.sampled_at > self.config.max_sample_age {
            Some(AdaptiveReason::StaleSample)
        } else if (observation.sampled_scale - self.current_scale()).abs() > 1e-4 {
            Some(AdaptiveReason::ScaleMismatch)
        } else {
            None
        };
        if let Some(reason) = rejected {
            self.clear_evidence();
            return self.decision(before, reason);
        }
        if self.last_sample.is_some_and(|(frame, at)| {
            observation.frame_id <= frame || observation.sampled_at <= at
        }) {
            return self.decision(before, AdaptiveReason::OutOfOrderSample);
        }
        let mut dt = self
            .last_valid_at
            .map(|at| observation.sampled_at - at)
            .unwrap_or_default();
        if dt > self.config.max_sample_age {
            // A pause is not sustained overload, even when the first resumed
            // measurement is fresh and carries a larger frame ID.
            self.clear_evidence();
            dt = Duration::ZERO;
        }
        self.last_sample = Some((observation.frame_id, observation.sampled_at));
        self.last_valid_at = Some(observation.sampled_at);
        let ms = observation.gpu_ms.expect("validated above");
        let alpha = 1.0 - (-dt.as_secs_f64() / self.config.smoothing_time.as_secs_f64()).exp();
        let smoothed = self
            .smoothed_ms
            .map(|old| old + alpha * (ms - old))
            .unwrap_or(ms);
        self.smoothed_ms = Some(smoothed);
        let ready_since = *self.ready_since.get_or_insert(observation.sampled_at);
        if observation.sampled_at - ready_since < self.config.settling_time {
            return self.decision(before, AdaptiveReason::Settling);
        }

        let budget = 1000.0 / self.config.target_fps;
        if let Some(trial) = self.downward_trial {
            self.trial_for = self.trial_for.saturating_add(dt);
            if self.trial_for < self.config.over_budget_for {
                return self.decision(before, AdaptiveReason::Settling);
            }
            let benefit = (trial.previous_ms - smoothed) / trial.previous_ms;
            self.downward_trial = None;
            if benefit < self.config.minimum_downshift_benefit {
                self.index = trial.previous_index;
                self.no_benefit_cost = Some(trial.previous_ms);
                self.clear_evidence();
                return self.decision(before, AdaptiveReason::NoMeasuredBenefit);
            }
            // At this lower rung, remember the measured cost of the adjacent
            // higher rung. Only retry it after enough measured headroom develops.
            self.comparisons[self.index] = Some(RungComparison {
                upper_ms: trial.previous_ms,
                lower_ms: smoothed,
            });
            self.overload_for = Duration::ZERO;
            self.headroom_for = Duration::ZERO;
        }

        if let Some(reference) = self.no_benefit_cost {
            if (smoothed - reference).abs() / reference < 0.25 {
                return self.decision(before, AdaptiveReason::NoMeasuredBenefit);
            }
            self.no_benefit_cost = None;
        }
        if smoothed > budget * self.config.over_budget_ratio {
            self.headroom_for = Duration::ZERO;
            self.overload_for = self.overload_for.saturating_add(dt);
            if self.overload_for < self.config.over_budget_for {
                return self.decision(before, AdaptiveReason::OverBudget);
            }
            if self.index == self.floor {
                return self.decision(before, AdaptiveReason::BudgetInfeasible);
            }
            self.downward_trial = Some(DownwardTrial {
                previous_index: self.index,
                previous_ms: smoothed,
            });
            self.index -= 1;
            self.clear_evidence();
            return self.decision(before, AdaptiveReason::OverBudget);
        }
        self.overload_for = Duration::ZERO;
        if smoothed < budget * self.config.headroom_ratio && self.index + 1 < self.ladder.len() {
            let upper_estimate = self.comparisons[self.index].map(|comparison| {
                if comparison.lower_ms > 0.0 {
                    comparison.upper_ms * (smoothed / comparison.lower_ms)
                } else {
                    comparison.upper_ms
                }
            });
            // The lower rung must have sustained headroom, but the higher rung
            // only needs to meet the budget. Applying the headroom fraction a
            // second time would strand quality after a workload improvement.
            if upper_estimate.is_none_or(|ms| ms <= budget) {
                self.headroom_for = self.headroom_for.saturating_add(dt);
                if self.headroom_for >= self.config.headroom_for {
                    self.index += 1;
                    self.clear_evidence();
                    return self.decision(before, AdaptiveReason::Headroom);
                }
            } else {
                self.headroom_for = Duration::ZERO;
            }
        } else {
            self.headroom_for = Duration::ZERO;
        }
        self.decision(before, AdaptiveReason::WithinBudget)
    }

    fn clear_evidence(&mut self) {
        self.last_valid_at = None;
        self.ready_since = None;
        self.smoothed_ms = None;
        self.overload_for = Duration::ZERO;
        self.headroom_for = Duration::ZERO;
        self.trial_for = Duration::ZERO;
    }

    fn decision(&self, before: usize, reason: AdaptiveReason) -> AdaptiveDecision {
        AdaptiveDecision {
            scale: self.current_scale(),
            changed: before != self.index,
            reason,
            gpu_ms: self.smoothed_ms,
            budget_ms: 1000.0 / self.config.target_fps,
        }
    }

    fn validate_config(
        config: &AdaptiveConfig,
        ladder: &[f32],
    ) -> Result<usize, AdaptiveConfigError> {
        let budget = 1000.0 / config.target_fps;
        if !config.target_fps.is_finite()
            || config.target_fps <= 0.0
            || !budget.is_finite()
            || budget <= 0.0
        {
            return Err(AdaptiveConfigError::InvalidTarget);
        }
        if !config.minimum_scale.is_finite()
            || config.minimum_scale <= 0.0
            || config.minimum_scale > 1.0
        {
            return Err(AdaptiveConfigError::InvalidMinimumScale);
        }
        if config.smoothing_time.is_zero()
            || config.over_budget_for.is_zero()
            || config.headroom_for.is_zero()
            || config.max_sample_age.is_zero()
        {
            return Err(AdaptiveConfigError::InvalidTiming);
        }
        if !config.over_budget_ratio.is_finite()
            || config.over_budget_ratio < 1.0
            || !config.headroom_ratio.is_finite()
            || !(0.0..1.0).contains(&config.headroom_ratio)
            || config.headroom_ratio == 0.0
            || !config.minimum_downshift_benefit.is_finite()
            || !(0.0..1.0).contains(&config.minimum_downshift_benefit)
            || config.minimum_downshift_benefit == 0.0
            || !(budget * config.over_budget_ratio).is_finite()
        {
            return Err(AdaptiveConfigError::InvalidThresholds);
        }
        ladder
            .iter()
            .position(|scale| *scale >= config.minimum_scale)
            .ok_or(AdaptiveConfigError::NoAllowedScale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(fps: f64, minimum: f32, start: f32) -> AdaptiveController {
        AdaptiveController::new(
            AdaptiveConfig {
                target_fps: fps,
                minimum_scale: minimum,
                ..Default::default()
            },
            vec![0.5, 0.67, 0.77, 1.0],
            start,
        )
        .unwrap()
    }

    fn sample(seconds: f64, frame: u64, scale: f32, gpu_ms: f64) -> AdaptiveObservation {
        AdaptiveObservation {
            now: Duration::from_secs_f64(seconds),
            epoch: 0,
            frame_id: frame,
            sampled_at: Duration::from_secs_f64(seconds),
            sampled_scale: scale,
            gpu_ms: Some(gpu_ms),
            validity: AdaptiveSampleValidity::ValidatedFrameCost,
            effect_ready: true,
        }
    }

    fn trajectory(
        controller: &mut AdaptiveController,
        hz: u32,
        start: f64,
        seconds: f64,
        cost: impl Fn(f32, f64) -> f64,
    ) -> Vec<AdaptiveDecision> {
        (1..=(seconds * hz as f64) as u64)
            .map(|tick| {
                let at = start + tick as f64 / hz as f64;
                let scale = controller.current_scale();
                controller.observe(sample(
                    at,
                    (at * 1_000_000.0).round() as u64,
                    scale,
                    cost(scale, at),
                ))
            })
            .collect()
    }

    #[test]
    fn validates_configuration_and_quality_floor() {
        for fps in [0.0, -60.0, f64::NAN, f64::INFINITY] {
            assert!(controller_config(fps, 0.5, vec![0.5, 1.0]).is_err());
        }
        for ladder in [
            vec![],
            vec![1.0, 0.5],
            vec![0.5, 0.5],
            vec![0.0, 1.0],
            vec![0.5, f32::NAN],
        ] {
            assert!(controller_config(60.0, 0.5, ladder).is_err());
        }
        assert!(controller_config(60.0, 0.8, vec![0.5, 0.67]).is_err());
        assert_eq!(controller(60.0, 0.75, 0.5).current_scale(), 0.77);
    }

    fn controller_config(
        fps: f64,
        minimum: f32,
        ladder: Vec<f32>,
    ) -> Result<AdaptiveController, AdaptiveConfigError> {
        AdaptiveController::new(
            AdaptiveConfig {
                target_fps: fps,
                minimum_scale: minimum,
                ..Default::default()
            },
            ladder,
            1.0,
        )
    }

    #[test]
    fn sustained_gpu_pressure_converges_at_60_and_120_hz() {
        for hz in [60, 120] {
            let mut c = controller(hz as f64, 0.5, 1.0);
            let budget = 1000.0 / hz as f64;
            let decisions = trajectory(&mut c, hz, 0.0, 15.0, |scale, _| {
                budget * 1.8 * f64::from(scale).powi(2)
            });
            assert!(decisions
                .iter()
                .any(|d| d.reason == AdaptiveReason::OverBudget && d.changed));
            assert_eq!(c.current_scale(), 0.67);
            assert!(decisions
                .iter()
                .rev()
                .take(hz as usize * 5)
                .all(|d| !d.changed));
        }
    }

    #[test]
    fn decision_intervals_are_seconds_not_frame_counts() {
        let first_change = |hz| {
            let mut c = controller(60.0, 0.5, 1.0);
            trajectory(&mut c, hz, 0.0, 3.0, |_, _| 30.0)
                .iter()
                .position(|d| d.changed)
                .unwrap() as f64
                / hz as f64
        };
        assert!((first_change(60) - first_change(120)).abs() < 0.04);
    }

    #[test]
    fn overload_during_evidence_window_is_not_reported_within_budget() {
        let mut c = controller(60.0, 0.5, 1.0);
        let decisions = trajectory(&mut c, 60, 0.0, 0.6, |_, _| 30.0);
        assert_eq!(decisions.last().unwrap().reason, AdaptiveReason::OverBudget);
        assert!(decisions.iter().all(|d| !d.changed));
    }

    #[test]
    fn scaler_rebuild_preserves_the_downshift_benefit_check() {
        let mut c = controller(60.0, 0.5, 1.0);
        trajectory(&mut c, 60, 0.0, 1.0, |_, _| 30.0);
        assert_eq!(c.current_scale(), 0.77);
        for frame in 1..100 {
            let mut input = sample(1.0 + frame as f64 / 60.0, 1_000_000 + frame, 0.77, 30.0);
            input.effect_ready = false;
            assert_eq!(c.observe(input).reason, AdaptiveReason::EffectPending);
        }
        let decisions = trajectory(&mut c, 60, 3.0, 5.0, |_, _| 30.0);
        assert!(decisions.iter().all(|d| d.scale >= 0.77));
        assert_eq!(c.current_scale(), 1.0);
        assert_eq!(
            decisions.last().unwrap().reason,
            AdaptiveReason::NoMeasuredBenefit
        );
    }

    #[test]
    fn lower_gpu_workload_reopens_a_previously_failed_higher_rung() {
        let mut c = controller(60.0, 0.5, 0.77);
        trajectory(&mut c, 60, 0.0, 10.0, |scale, _| {
            if scale == 1.0 {
                22.0
            } else {
                10.0
            }
        });
        assert_eq!(c.current_scale(), 0.77);
        trajectory(&mut c, 60, 10.0, 10.0, |scale, _| {
            if scale == 1.0 {
                11.0
            } else {
                5.0
            }
        });
        assert_eq!(c.current_scale(), 1.0);
    }

    #[test]
    fn workload_recovery_uses_a_higher_rung_that_now_meets_budget() {
        let mut c = controller_config(60.0, 0.5, vec![0.5, 1.0]).unwrap();
        trajectory(&mut c, 60, 0.0, 6.0, |scale, _| 24.0 * f64::from(scale));
        assert_eq!(c.current_scale(), 0.5);
        trajectory(&mut c, 60, 6.0, 24.0, |scale, _| 15.0 * f64::from(scale));
        assert_eq!(c.current_scale(), 1.0);
    }

    #[test]
    fn slow_cpu_cadence_with_gpu_headroom_never_lowers_quality() {
        let mut c = controller(60.0, 0.5, 1.0);
        let decisions = trajectory(&mut c, 20, 0.0, 12.0, |_, _| 4.0);
        assert!(decisions.iter().all(|d| d.scale == 1.0));
    }

    #[test]
    fn brief_spike_and_near_budget_noise_do_not_change_scale() {
        let mut c = controller(60.0, 0.5, 1.0);
        let decisions = trajectory(&mut c, 120, 0.0, 12.0, |_, at| {
            if (4.0..4.05).contains(&at) {
                45.0
            } else {
                16.0 + (at * 17.0).sin() * 0.4
            }
        });
        assert!(decisions.iter().all(|d| !d.changed));
    }

    #[test]
    fn headroom_can_raise_quality_but_does_not_repeat_failed_upward_trials() {
        let mut c = controller(60.0, 0.5, 0.5);
        let decisions = trajectory(&mut c, 60, 0.0, 30.0, |scale, _| {
            if scale == 1.0 {
                22.0
            } else {
                10.0
            }
        });
        assert_eq!(c.current_scale(), 0.77);
        assert!(decisions
            .iter()
            .any(|d| d.reason == AdaptiveReason::Headroom && d.changed));
        assert!(decisions.iter().rev().take(60 * 10).all(|d| !d.changed));
    }

    #[test]
    fn missing_unvalidated_and_invalid_samples_hold_quality() {
        for (validity, gpu_ms, reason) in [
            (
                AdaptiveSampleValidity::Unavailable,
                None,
                AdaptiveReason::Unavailable,
            ),
            (
                AdaptiveSampleValidity::UnvalidatedScope,
                Some(40.0),
                AdaptiveReason::UnvalidatedScope,
            ),
            (
                AdaptiveSampleValidity::ValidatedFrameCost,
                Some(f64::NAN),
                AdaptiveReason::InvalidSample,
            ),
            (
                AdaptiveSampleValidity::ValidatedFrameCost,
                Some(-1.0),
                AdaptiveReason::InvalidSample,
            ),
            (
                AdaptiveSampleValidity::ValidatedFrameCost,
                Some(0.0),
                AdaptiveReason::InvalidSample,
            ),
            (
                AdaptiveSampleValidity::ValidatedFrameCost,
                Some(f64::INFINITY),
                AdaptiveReason::InvalidSample,
            ),
        ] {
            let mut c = controller(60.0, 0.5, 1.0);
            for tick in 1..600 {
                let mut input = sample(tick as f64 / 60.0, tick, 1.0, 40.0);
                input.validity = validity;
                input.gpu_ms = gpu_ms;
                let d = c.observe(input);
                assert_eq!(d.reason, reason);
                assert!(!d.changed);
            }
        }
    }

    #[test]
    fn stale_future_duplicate_and_mismatched_samples_cannot_accumulate_evidence() {
        let mut c = controller(60.0, 0.5, 1.0);
        let initial = sample(1.0, 10, 1.0, 40.0);
        c.observe(initial);
        let mut duplicate = initial;
        duplicate.now = Duration::from_secs_f64(1.1);
        assert_eq!(
            c.observe(duplicate).reason,
            AdaptiveReason::OutOfOrderSample
        );
        let mut stale = sample(2.0, 11, 1.0, 40.0);
        stale.sampled_at = Duration::from_secs(1);
        assert_eq!(c.observe(stale).reason, AdaptiveReason::StaleSample);
        let mut future = sample(3.0, 12, 1.0, 40.0);
        future.sampled_at = Duration::from_secs(4);
        assert_eq!(c.observe(future).reason, AdaptiveReason::InvalidSample);
        assert_eq!(
            c.observe(sample(4.0, 13, 0.5, 40.0)).reason,
            AdaptiveReason::ScaleMismatch
        );
        let mut older_epoch = sample(5.0, 20, 1.0, 40.0);
        older_epoch.epoch = 2;
        c.observe(older_epoch);
        older_epoch.epoch = 1;
        assert_eq!(
            c.observe(older_epoch).reason,
            AdaptiveReason::OutOfOrderSample
        );
        assert_eq!(c.current_scale(), 1.0);
    }

    #[test]
    fn pause_and_pending_effect_clear_old_overload_evidence() {
        let mut c = controller(60.0, 0.5, 1.0);
        trajectory(&mut c, 60, 0.0, 0.6, |_, _| 30.0);
        let mut input = sample(0.61, 610000, 1.0, 30.0);
        input.effect_ready = false;
        assert_eq!(c.observe(input).reason, AdaptiveReason::EffectPending);
        assert!(trajectory(&mut c, 60, 20.0, 0.5, |_, _| 30.0)
            .iter()
            .all(|d| !d.changed));
        c.reset();
        assert!(trajectory(&mut c, 60, 21.0, 0.5, |_, _| 30.0)
            .iter()
            .all(|d| !d.changed));
    }

    #[test]
    fn ineffective_downshift_restores_quality_and_stops_descent() {
        let mut c = controller(60.0, 0.5, 1.0);
        let decisions = trajectory(&mut c, 60, 0.0, 15.0, |_, _| 30.0);
        assert!(decisions
            .iter()
            .any(|d| d.reason == AdaptiveReason::NoMeasuredBenefit && d.changed));
        assert!(decisions.iter().all(|d| d.scale >= 0.77));
        assert_eq!(c.current_scale(), 1.0);
        assert!(decisions.iter().rev().take(60 * 5).all(|d| !d.changed));
    }

    #[test]
    fn reports_infeasible_at_quality_floor_and_never_crosses_it() {
        let mut c = controller(120.0, 0.75, 1.0);
        let decisions = trajectory(&mut c, 120, 0.0, 12.0, |scale, _| {
            30.0 * f64::from(scale).powi(2)
        });
        assert!(decisions.iter().all(|d| d.scale >= 0.75));
        assert_eq!(c.current_scale(), 0.77);
        assert_eq!(
            decisions.last().unwrap().reason,
            AdaptiveReason::BudgetInfeasible
        );
    }

    #[test]
    fn target_and_floor_changes_clear_evidence_and_apply_new_policy() {
        let mut c = controller(60.0, 0.5, 1.0);
        trajectory(&mut c, 60, 0.0, 4.0, |scale, _| {
            14.0 * f64::from(scale).powi(2)
        });
        assert_eq!(c.current_scale(), 1.0);
        c.update_config(AdaptiveConfig {
            target_fps: 120.0,
            ..Default::default()
        })
        .unwrap();
        trajectory(&mut c, 120, 4.0, 12.0, |scale, _| {
            14.0 * f64::from(scale).powi(2)
        });
        assert_eq!(c.current_scale(), 0.77);
        c.update_config(AdaptiveConfig {
            minimum_scale: 0.9,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c.current_scale(), 1.0);
        let previous = c.current_scale();
        assert!(c
            .update_config(AdaptiveConfig {
                minimum_scale: 1.1,
                ..Default::default()
            })
            .is_err());
        assert_eq!(c.current_scale(), previous);
    }
}
