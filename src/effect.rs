//! Per-view observations of what MetalFX actually encoded.
//!
//! Clone [`MetalFxEffectStatus`] into the main and render worlds. The render
//! path publishes a new observation at each decision, including pending and
//! fallback decisions, and readers supply the frame they want to inspect.
//! A previous frame's successful output never counts as current output.
//!
//! This is CPU-side evidence of encoded commands. Neither [`MetalFxEffectState::Encoded`]
//! nor [`MetalFxEffectState::OutputWritten`] proves GPU completion, successful
//! presentation, or that an image reached the display.

use crate::MetalFxMode;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How far this view's MetalFX work progressed in the observed frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetalFxEffectState {
    /// MetalFX was explicitly disabled.
    Disabled,
    /// The requested effect cannot run on this platform or configuration.
    Unavailable,
    /// No render-path observation exists for the requested frame and view.
    NoRender,
    /// Required scaler or pipeline preparation has not finished.
    Pending,
    /// Preparation or encoding failed; the reason identifies the known cause.
    Failed,
    /// The MetalFX pass was encoded, but its output was not yet copied to the view.
    Encoded,
    /// Commands to copy MetalFX output to the view target were encoded.
    /// This does not mean those commands have completed on the GPU.
    OutputWritten,
}

/// A diagnosed reason for a fallback, delay, or failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetalFxEffectReason {
    /// Background scaler creation has not completed.
    ScalerPending,
    /// Scaler creation has exceeded the diagnostic wait threshold; still waiting.
    ScalerCreationSlow,
    /// Scaler creation returned no usable scaler.
    ScalerCreationFailed,
    /// A required texture format is unsupported.
    UnsupportedFormat,
    /// Depth or motion-vector inputs needed for temporal work are absent.
    MissingPrepass,
    /// A raw Metal device, texture, or command-buffer handle was unavailable.
    MetalHandleUnavailable,
    /// The pipeline that copies output to the view is still compiling.
    BlitPipelinePending,
    /// Compilation of the pipeline that copies output to the view failed.
    BlitPipelineFailed,
    /// This operating system cannot run MetalFX.
    UnsupportedPlatform,
    /// The MetalFX framework is unavailable at runtime.
    FrameworkUnavailable,
    /// The requested mode is disabled.
    ModeDisabled,
    /// The requested mode was not compiled into this build.
    FeatureUnavailable,
    /// No eligible render view ran in this frame.
    NoRenderView,
    /// Encoding was skipped but no more specific diagnosis is available.
    EncodeSkipped,
    /// The view has no output target or output format.
    MissingOutput,
    /// A required texture or content dimension is invalid.
    InvalidDimensions,
    /// The renderer has one scaler/history cache and cannot isolate multiple views.
    MultipleViewsUnsupported,
    /// A subrectangle or offset viewport is not supported by the effect's copy path.
    UnsupportedViewport,
}

/// One view's render-path decision, captured before publishing it.
///
/// Construct a new observation for every decision; cloning an old observation
/// preserves its ordering identity and timestamp. Frame IDs must increase
/// monotonically for the lifetime of the status resource. Sizes are physical
/// pixels; `[0, 0]` means the corresponding size has not been observed.
#[derive(Debug, Clone, PartialEq)]
pub struct MetalFxEffectObservation {
    /// Application frame ID, extended across any native counter wraparound.
    pub frame_id: u64,
    /// Stable view identity, normally Bevy's entity bits including generation.
    pub view_id: u64,
    /// Mode requested by the application.
    pub requested_mode: MetalFxMode,
    /// Mode selected by the render path; this alone does not prove encoding.
    pub effective_mode: MetalFxMode,
    /// Render scale requested for this frame.
    pub requested_scale: f32,
    /// Observed input content dimensions, in physical pixels.
    pub content_size: [u32; 2],
    /// Observed output dimensions, in physical pixels.
    pub output_size: [u32; 2],
    /// Stage actually observed, independent of the configured mode.
    pub state: MetalFxEffectState,
    /// Known reason for the observed state or mode fallback.
    pub reason: Option<MetalFxEffectReason>,
    /// Process-local monotonic time of the decision, rather than publication.
    pub observed_at: Instant,
    // Break equal clock timestamps without allowing delayed publications to
    // undo a later decision from the same frame. Clones keep this identity.
    sequence: u64,
}

impl MetalFxEffectObservation {
    /// Capture a render-path decision with a fresh timestamp and ordering ID.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        frame_id: u64,
        view_id: u64,
        requested_mode: MetalFxMode,
        effective_mode: MetalFxMode,
        requested_scale: f32,
        content_size: [u32; 2],
        output_size: [u32; 2],
        state: MetalFxEffectState,
        reason: Option<MetalFxEffectReason>,
    ) -> Self {
        static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        Self {
            frame_id,
            view_id,
            requested_mode,
            effective_mode,
            requested_scale,
            content_size,
            output_size,
            state,
            reason,
            observed_at: Instant::now(),
            sequence: NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        }
    }

    fn order(&self) -> (u64, Instant, u64) {
        (self.frame_id, self.observed_at, self.sequence)
    }
}

/// Frame relationship between a retained observation and a reader's frame.
/// Wall-clock freshness is checked separately by [`MetalFxEffectSnapshot::is_fresh`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetalFxEffectFreshness {
    /// The observation belongs to the requested frame.
    Current,
    /// The view last published in an earlier frame.
    Stale { age_frames: u64 },
    /// The reader requested an earlier frame than the retained observation.
    Future { ahead_frames: u64 },
    /// No observation is retained for this view, including after eviction.
    NeverObserved,
}

/// A view's observation interpreted relative to a specific frame.
#[derive(Debug, Clone)]
pub struct MetalFxEffectSnapshot {
    /// Frame requested by the reader.
    pub frame_id: u64,
    /// View requested by the reader.
    pub view_id: u64,
    /// Explicit relationship to the retained observation's frame.
    pub freshness: MetalFxEffectFreshness,
    /// Retained evidence for diagnostics, which may belong to an older frame.
    /// Use [`Self::state`] for strict current-frame status or [`Self::is_fresh`]
    /// before deliberately accepting a bounded render-pipeline delay.
    pub last_observation: Option<MetalFxEffectObservation>,
}

impl MetalFxEffectSnapshot {
    /// The current frame's state, or `NoRender` if no current observation exists.
    /// This checks frame identity; use [`Self::is_fresh`] to also bound wall age.
    pub fn state(&self) -> MetalFxEffectState {
        match (&self.freshness, &self.last_observation) {
            (MetalFxEffectFreshness::Current, Some(observation)) => observation.state,
            _ => MetalFxEffectState::NoRender,
        }
    }

    /// Number of elapsed frames, or `None` for missing/future observations.
    pub fn age_frames(&self) -> Option<u64> {
        match self.freshness {
            MetalFxEffectFreshness::Current => Some(0),
            MetalFxEffectFreshness::Stale { age_frames } => Some(age_frames),
            _ => None,
        }
    }

    /// Wall time since observation, evaluated when this method is called.
    /// `None` means there is no observation or its timestamp is in the future.
    pub fn wall_age(&self) -> Option<Duration> {
        Instant::now().checked_duration_since(self.last_observation.as_ref()?.observed_at)
    }

    /// Accept an observation only within both caller-chosen freshness bounds.
    /// A fresh observation can still report pending, unavailable, or failed;
    /// inspect its state before treating the effect as active.
    pub fn is_fresh(&self, max_age_frames: u64, max_wall_age: Duration) -> bool {
        self.age_frames().is_some_and(|age| age <= max_age_frames)
            && self.wall_age().is_some_and(|age| age <= max_wall_age)
    }

    fn from_observation(
        view_id: u64,
        frame_id: u64,
        last_observation: Option<MetalFxEffectObservation>,
    ) -> Self {
        let freshness = match &last_observation {
            None => MetalFxEffectFreshness::NeverObserved,
            Some(observation) if observation.frame_id < frame_id => MetalFxEffectFreshness::Stale {
                age_frames: frame_id - observation.frame_id,
            },
            Some(observation) if observation.frame_id > frame_id => {
                MetalFxEffectFreshness::Future {
                    ahead_frames: observation.frame_id - frame_id,
                }
            }
            Some(_) => MetalFxEffectFreshness::Current,
        };
        Self {
            frame_id,
            view_id,
            freshness,
            last_observation,
        }
    }
}

/// Shared, bounded registry of the latest observation for each render view.
/// Clones share storage across Bevy worlds. No GPU callback is required.
#[derive(bevy::prelude::Resource, Debug, Clone, Default)]
pub struct MetalFxEffectStatus {
    observations: Arc<Mutex<BTreeMap<u64, MetalFxEffectObservation>>>,
}

impl MetalFxEffectStatus {
    /// Maximum retained views; each view retains exactly one observation.
    pub const MAX_VIEWS: usize = 64;

    /// Publish a decision, returning `false` if a later decision already exists.
    /// Decisions are ordered by frame, observation time, then creation identity.
    /// At capacity, a new view replaces the oldest observation only if newer.
    pub fn publish(&self, observation: MetalFxEffectObservation) -> bool {
        let mut observations = self.observations.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = observations.get(&observation.view_id) {
            if observation.order() <= previous.order() {
                return false;
            }
        } else if observations.len() >= Self::MAX_VIEWS {
            let (&oldest_view, oldest) = observations
                .iter()
                .min_by_key(|(_, observation)| observation.order())
                .expect("a full registry is nonempty");
            if observation.order() <= oldest.order() {
                return false;
            }
            observations.remove(&oldest_view);
        }
        observations.insert(observation.view_id, observation);
        true
    }

    /// Read one view; an absent or evicted view explicitly reports `NoRender`.
    pub fn snapshot(&self, view_id: u64, current_frame: u64) -> MetalFxEffectSnapshot {
        let observations = self.observations.lock().unwrap_or_else(|e| e.into_inner());
        MetalFxEffectSnapshot::from_observation(
            view_id,
            current_frame,
            observations.get(&view_id).cloned(),
        )
    }

    /// Read all retained views in stable view-ID order, including stale views.
    pub fn snapshots(&self, current_frame: u64) -> Vec<MetalFxEffectSnapshot> {
        let observations = self.observations.lock().unwrap_or_else(|e| e.into_inner());
        observations
            .values()
            .map(|observation| {
                MetalFxEffectSnapshot::from_observation(
                    observation.view_id,
                    current_frame,
                    Some(observation.clone()),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(frame: u64, view: u64, state: MetalFxEffectState) -> MetalFxEffectObservation {
        MetalFxEffectObservation::new(
            frame,
            view,
            MetalFxMode::Temporal,
            MetalFxMode::Temporal,
            0.5,
            [960, 540],
            [1920, 1080],
            state,
            None,
        )
    }

    #[test]
    fn pending_encoded_and_output_written_are_distinct_observations() {
        let status = MetalFxEffectStatus::default();
        for state in [
            MetalFxEffectState::Pending,
            MetalFxEffectState::Encoded,
            MetalFxEffectState::OutputWritten,
        ] {
            let mut next = observation(1, 7, state);
            next.requested_mode = MetalFxMode::FrameInterpolation;
            next.reason = Some(MetalFxEffectReason::FeatureUnavailable);
            assert!(status.publish(next.clone()));
            let snapshot = status.snapshot(7, 1);
            assert_eq!(snapshot.state(), state);
            assert_eq!(snapshot.last_observation, Some(next));
        }
    }

    #[test]
    fn new_frame_fallback_replaces_previous_output() {
        let status = MetalFxEffectStatus::default();
        assert!(status.publish(observation(1, 7, MetalFxEffectState::OutputWritten)));
        for (index, state, reason) in [
            (
                2,
                MetalFxEffectState::Pending,
                MetalFxEffectReason::ScalerPending,
            ),
            (
                3,
                MetalFxEffectState::Failed,
                MetalFxEffectReason::ScalerCreationFailed,
            ),
            (
                4,
                MetalFxEffectState::Disabled,
                MetalFxEffectReason::ModeDisabled,
            ),
            (
                5,
                MetalFxEffectState::Unavailable,
                MetalFxEffectReason::UnsupportedPlatform,
            ),
            (
                6,
                MetalFxEffectState::NoRender,
                MetalFxEffectReason::NoRenderView,
            ),
        ] {
            let mut next = observation(index, 7, state);
            next.effective_mode = MetalFxMode::Disabled;
            next.reason = Some(reason);
            assert!(status.publish(next));
            let snapshot = status.snapshot(7, index);
            assert_eq!(snapshot.state(), state);
            let latest = snapshot.last_observation.unwrap();
            assert_eq!(latest.reason, Some(reason));
            assert_eq!(latest.effective_mode, MetalFxMode::Disabled);
        }
    }

    #[test]
    fn missing_and_stale_views_do_not_report_active_output() {
        let status = MetalFxEffectStatus::default();
        let missing = status.snapshot(7, 10);
        assert_eq!(missing.frame_id, 10);
        assert_eq!(missing.view_id, 7);
        assert_eq!(missing.freshness, MetalFxEffectFreshness::NeverObserved);
        assert_eq!(missing.state(), MetalFxEffectState::NoRender);
        assert_eq!(missing.age_frames(), None);
        assert_eq!(missing.wall_age(), None);
        assert!(!missing.is_fresh(1, Duration::from_secs(1)));

        assert!(status.publish(observation(8, 7, MetalFxEffectState::OutputWritten)));
        let stale = status.snapshot(7, 10);
        assert_eq!(
            stale.freshness,
            MetalFxEffectFreshness::Stale { age_frames: 2 }
        );
        assert_eq!(stale.age_frames(), Some(2));
        assert_eq!(stale.state(), MetalFxEffectState::NoRender);
        assert_eq!(
            stale.last_observation.unwrap().state,
            MetalFxEffectState::OutputWritten
        );
    }

    #[test]
    fn readers_accept_only_explicit_frame_lag_and_wall_age_bounds() {
        let status = MetalFxEffectStatus::default();
        assert!(status.publish(observation(8, 7, MetalFxEffectState::OutputWritten)));
        let current = status.snapshot(7, 8);
        assert_eq!(current.age_frames(), Some(0));
        assert!(current.is_fresh(0, Duration::from_secs(10)));
        let lagged = status.snapshot(7, 9);
        assert!(!lagged.is_fresh(0, Duration::from_secs(10)));
        assert!(lagged.is_fresh(1, Duration::from_secs(10)));

        let mut old = observation(10, 7, MetalFxEffectState::OutputWritten);
        old.observed_at = Instant::now() - Duration::from_secs(10);
        assert!(status.publish(old));
        let stalled = status.snapshot(7, 10);
        assert_eq!(stalled.freshness, MetalFxEffectFreshness::Current);
        assert!(!stalled.is_fresh(1, Duration::from_secs(1)));
        assert!(stalled.wall_age().unwrap() >= Duration::from_secs(10));
    }

    #[test]
    fn future_observations_are_not_fresh_for_an_earlier_frame() {
        let status = MetalFxEffectStatus::default();
        assert!(status.publish(observation(10, 7, MetalFxEffectState::Encoded)));
        let future = status.snapshot(7, 9);
        assert_eq!(
            future.freshness,
            MetalFxEffectFreshness::Future { ahead_frames: 1 }
        );
        assert_eq!(future.age_frames(), None);
        assert_eq!(future.state(), MetalFxEffectState::NoRender);
        assert!(!future.is_fresh(u64::MAX, Duration::MAX));
    }

    #[test]
    fn delayed_old_frames_cannot_replace_newer_observations() {
        let status = MetalFxEffectStatus::default();
        assert!(status.publish(observation(10, 7, MetalFxEffectState::OutputWritten)));
        assert!(!status.publish(observation(9, 7, MetalFxEffectState::Pending)));
        assert_eq!(
            status.snapshot(7, 10).state(),
            MetalFxEffectState::OutputWritten
        );
    }

    #[test]
    fn delayed_events_within_a_frame_cannot_replace_later_events() {
        let status = MetalFxEffectStatus::default();
        let mut earlier = observation(10, 7, MetalFxEffectState::Pending);
        earlier.observed_at = Instant::now() - Duration::from_secs(1);
        assert!(status.publish(observation(10, 7, MetalFxEffectState::OutputWritten)));
        assert!(!status.publish(earlier));
        assert_eq!(
            status.snapshot(7, 10).state(),
            MetalFxEffectState::OutputWritten
        );
    }

    #[test]
    fn events_with_equal_clock_timestamps_keep_creation_order() {
        let status = MetalFxEffectStatus::default();
        let earlier = observation(10, 7, MetalFxEffectState::Pending);
        let mut later = observation(10, 7, MetalFxEffectState::OutputWritten);
        later.observed_at = earlier.observed_at;
        assert!(status.publish(later));
        assert!(!status.publish(earlier));
        assert_eq!(
            status.snapshot(7, 10).state(),
            MetalFxEffectState::OutputWritten
        );
    }

    #[test]
    fn resource_clones_share_observations_without_mixing_views() {
        let main = MetalFxEffectStatus::default();
        let render = main.clone();
        assert!(render.publish(observation(10, 9, MetalFxEffectState::OutputWritten)));
        assert!(render.publish(observation(10, 7, MetalFxEffectState::Failed)));
        assert_eq!(
            main.snapshot(9, 10).state(),
            MetalFxEffectState::OutputWritten
        );
        assert_eq!(main.snapshot(7, 10).state(), MetalFxEffectState::Failed);
        let snapshots = main.snapshots(10);
        assert_eq!(
            snapshots.iter().map(|s| s.view_id).collect::<Vec<_>>(),
            vec![7, 9]
        );
    }

    #[test]
    fn retained_views_are_bounded_and_old_publications_cannot_evict_newer_views() {
        let status = MetalFxEffectStatus::default();
        for view in 0..=MetalFxEffectStatus::MAX_VIEWS as u64 {
            assert!(status.publish(observation(view + 1, view, MetalFxEffectState::Encoded)));
        }
        assert_eq!(status.snapshots(100).len(), MetalFxEffectStatus::MAX_VIEWS);
        assert_eq!(
            status.snapshot(0, 100).freshness,
            MetalFxEffectFreshness::NeverObserved
        );
        assert!(!status.publish(observation(1, 0, MetalFxEffectState::OutputWritten)));
        assert_eq!(status.snapshots(100).len(), MetalFxEffectStatus::MAX_VIEWS);
        assert!(status.publish(observation(100, 1, MetalFxEffectState::OutputWritten)));
        assert_eq!(status.snapshots(100).len(), MetalFxEffectStatus::MAX_VIEWS);
        assert_eq!(
            status.snapshot(1, 100).state(),
            MetalFxEffectState::OutputWritten
        );
    }
}
