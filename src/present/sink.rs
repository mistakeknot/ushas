//! Presentation telemetry: the ring of `MTLDrawable.presentedTime` samples
//! and the counters that separate a frame that never ran from one that ran
//! and was skipped.

use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::runtime::ProtocolObject;
use objc2_metal::MTLDrawable;

/// How many recent presentation samples to retain.
const RING_CAPACITY: usize = 480;

/// Thread-safe ring of drawable presentation samples.
///
/// Written from Metal's presentation callback thread, read from the Bevy side.
/// Mirrors [`crate::gpu_timing::GpuTimingSink`]'s discipline: `try_lock` only,
/// no allocation in the callback, no panics across the ObjC boundary.
#[derive(Debug, Default)]
pub struct PresentSink {
    inner: Mutex<Ring>,
    /// The presented-handler block, created once and reused for every drawable.
    ///
    /// A per-frame `RcBlock` does not survive long enough: the presented
    /// callback fires *after* the command buffer completes, so any lifetime
    /// tied to that buffer has already ended. The block only captures this
    /// sink, so one instance serves every drawable — created once, leaked
    /// deliberately, and handed to Metal by raw pointer.
    handler_block: std::sync::OnceLock<usize>,
}

#[derive(Debug, Default)]
struct Ring {
    /// `presentedTime` of each interpolated frame that actually reached the
    /// display, in CoreAnimation's timebase.
    presented: Vec<f64>,
    next: usize,
    /// Interpolated frames skipped because no drawable was available.
    dropped: u64,
    /// Presents actually encoded onto a command buffer. Distinguishes "the
    /// path never ran" from "it ran but nothing reached the display".
    encoded: u64,
    /// Presentation callbacks received, regardless of whether the frame was
    /// actually shown. Metal reports `presentedTime == 0` for a frame it
    /// skipped, so callbacks-without-times is the signature of a frame that
    /// was superseded before it reached the panel.
    callbacks: u64,
    /// Completions of the command buffer the present was encoded onto. Proves
    /// whether that buffer reaches the GPU at all.
    committed: u64,
    /// Presentation callbacks that arrived with a non-increasing timestamp —
    /// i.e. a frame reached the display out of order.
    inversions: u64,
    /// Most recent presentation time seen, for inversion detection.
    last_presented: f64,
}

impl PresentSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Ring {
                presented: Vec::with_capacity(RING_CAPACITY),
                ..Default::default()
            }),
            handler_block: std::sync::OnceLock::new(),
        })
    }

    /// Record one presented interpolated frame. Called from Metal's callback
    /// thread; drops the sample rather than blocking under contention.
    fn push_presented(&self, t: f64) {
        let Ok(mut ring) = self.inner.try_lock() else {
            return;
        };
        // A presentation timestamp that does not advance means this frame
        // reached the display no later than the one before it.
        if ring.last_presented > 0.0 && t <= ring.last_presented {
            ring.inversions += 1;
        }
        ring.last_presented = t;

        if ring.presented.len() < RING_CAPACITY {
            ring.presented.push(t);
        } else {
            let i = ring.next;
            ring.presented[i] = t;
            ring.next = (i + 1) % RING_CAPACITY;
        }
    }

    /// Note an interpolated frame that never got a drawable.
    pub fn push_dropped(&self) {
        if let Ok(mut ring) = self.inner.try_lock() {
            ring.dropped += 1;
        }
    }

    pub(crate) fn push_committed(&self) {
        if let Ok(mut ring) = self.inner.try_lock() {
            ring.committed += 1;
        }
    }

    /// Note a presentation callback, shown or skipped.
    fn push_callback(&self) {
        if let Ok(mut ring) = self.inner.try_lock() {
            ring.callbacks += 1;
        }
    }

    /// Note a present that was encoded onto a command buffer.
    pub fn push_encoded(&self) {
        if let Ok(mut ring) = self.inner.try_lock() {
            ring.encoded += 1;
        }
    }

    /// Raw counters, readable even when there are too few samples for
    /// [`Self::stats`]. `(encoded, dropped, presented)` — the three numbers
    /// that separate "never ran" from "no drawable" from "encoded but never
    /// displayed".
    pub fn counts(&self) -> (u64, u64, usize, u64, u64) {
        self.inner
            .lock()
            .map(|r| {
                (
                    r.encoded,
                    r.dropped,
                    r.presented.len(),
                    r.callbacks,
                    r.committed,
                )
            })
            .unwrap_or((0, 0, 0, 0, 0))
    }

    /// Raw pointer to the shared presented-handler block, creating it on first
    /// use. Leaked on purpose — it must outlive every drawable it is attached
    /// to, and there is exactly one per sink.
    pub(crate) fn presented_handler_block(self: &Arc<Self>) -> usize {
        *self.handler_block.get_or_init(|| {
            let sink = Arc::clone(self);
            let block = RcBlock::new(move |presented: NonNull<ProtocolObject<dyn MTLDrawable>>| {
                sink.push_callback();
                // SAFETY: Metal hands us a live drawable for the call.
                let t = unsafe { presented.as_ref() }.presentedTime();
                if t.is_finite() && t > 0.0 {
                    sink.push_presented(t);
                }
            });
            RcBlock::into_raw(block) as usize
        })
    }

    /// Last presentation time seen, or 0.0 if none yet.
    pub fn last_presented(&self) -> f64 {
        self.inner.lock().map(|r| r.last_presented).unwrap_or(0.0)
    }

    /// Discard accumulated samples — used to drop warmup frames before a
    /// measurement window.
    pub fn reset(&self) {
        if let Ok(mut ring) = self.inner.lock() {
            ring.presented.clear();
            ring.next = 0;
            ring.dropped = 0;
            ring.encoded = 0;
            ring.callbacks = 0;
            ring.committed = 0;
            ring.inversions = 0;
            // `last_presented` deliberately survives: it anchors inversion
            // detection across the reset boundary.
        }
    }

    /// Summarise the interpolated-frame presentation record.
    ///
    /// Returns `None` until at least two frames have been presented, since
    /// every statistic here is defined over *intervals*.
    pub fn stats(&self) -> Option<PresentStats> {
        let ring = self.inner.lock().ok()?;
        let (dropped, inversions) = (ring.dropped, ring.inversions);

        // The ring is written circularly, so sort into presentation order
        // before differencing.
        let mut times = ring.presented.clone();
        drop(ring);
        if times.len() < 2 {
            return None;
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let mut intervals: Vec<f32> = times
            .windows(2)
            .map(|w| ((w[1] - w[0]) * 1000.0) as f32)
            .filter(|d| d.is_finite() && *d > 0.0)
            .collect();
        if intervals.is_empty() {
            return None;
        }

        let n = intervals.len();
        let mean = intervals.iter().sum::<f32>() / n as f32;
        // Spread of the interval distribution: what judder actually is.
        let variance = intervals.iter().map(|d| (d - mean).powi(2)).sum::<f32>() / n as f32;

        intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p = |q: f32| intervals[((q * (n as f32 - 1.0)).round() as usize).min(n - 1)];

        Some(PresentStats {
            count: n + 1,
            interp_fps: if mean > 0.0 { 1000.0 / mean } else { 0.0 },
            mean_interval_ms: mean,
            p50_interval_ms: p(0.50),
            p99_interval_ms: p(0.99),
            judder_ms: variance.sqrt(),
            dropped,
            inversions,
        })
    }
}

/// Summary of how the interpolated frames actually reached the display.
///
/// Every field is derived from `MTLDrawable.presentedTime` — the time the
/// compositor reports for a frame that was really shown — not from render-loop
/// timing, which cannot see drops or compositor decisions.
#[derive(Debug, Clone, Copy)]
pub struct PresentStats {
    /// Interpolated frames presented in the sample window.
    pub count: usize,
    /// Rate of *interpolated* frames reaching the display. Total presented
    /// rate is this plus the real-frame rate, since the two alternate 1:1.
    pub interp_fps: f32,
    pub mean_interval_ms: f32,
    pub p50_interval_ms: f32,
    pub p99_interval_ms: f32,
    /// Standard deviation of the presentation interval. Even pacing drives
    /// this toward zero; visible judder shows up here before anywhere else.
    pub judder_ms: f32,
    /// Interpolated frames skipped for want of a drawable.
    pub dropped: u64,
    /// Frames whose presentation timestamp did not advance — out-of-order
    /// display. Structurally should be zero; measured rather than assumed.
    pub inversions: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_need_two_samples() {
        let sink = PresentSink::new();
        assert!(sink.stats().is_none());
        sink.push_presented(1.0);
        assert!(sink.stats().is_none(), "one sample defines no interval");
        sink.push_presented(1.008);
        assert!(sink.stats().is_some());
    }

    #[test]
    fn interval_stats_track_a_steady_120hz_cadence() {
        let sink = PresentSink::new();
        // Interpolated frames alternate with real ones, so a 120Hz display
        // shows them every ~16.67ms.
        for i in 0..60 {
            sink.push_presented(1.0 + i as f64 * (1.0 / 60.0));
        }
        let s = sink.stats().expect("stats");
        assert!((s.mean_interval_ms - 16.667).abs() < 0.1, "{s:?}");
        assert!((s.interp_fps - 60.0).abs() < 0.5, "{s:?}");
        assert!(
            s.judder_ms < 0.01,
            "steady cadence should show no judder: {s:?}"
        );
        assert_eq!(s.inversions, 0);
    }

    #[test]
    fn non_advancing_timestamps_count_as_inversions() {
        let sink = PresentSink::new();
        sink.push_presented(2.0);
        sink.push_presented(1.5); // went backwards
        sink.push_presented(1.5); // did not advance
        let s = sink.stats().expect("stats");
        assert_eq!(s.inversions, 2, "{s:?}");
    }

    #[test]
    fn dropped_frames_are_counted_without_disturbing_intervals() {
        let sink = PresentSink::new();
        sink.push_presented(1.0);
        sink.push_presented(1.016);
        sink.push_dropped();
        sink.push_dropped();
        let s = sink.stats().expect("stats");
        assert_eq!(s.dropped, 2);
        assert_eq!(s.count, 2);
    }

    #[test]
    fn reset_clears_samples_but_keeps_the_scheduling_anchor() {
        let sink = PresentSink::new();
        sink.push_presented(1.0);
        sink.push_presented(1.016);
        sink.push_dropped();
        sink.reset();
        assert!(sink.stats().is_none());
        assert_eq!(
            sink.last_presented(),
            1.016,
            "anchor must survive so scheduling and inversion detection stay continuous"
        );
    }

    #[test]
    fn judder_shows_up_as_interval_spread() {
        let steady = PresentSink::new();
        let jittery = PresentSink::new();
        let mut t_s = 1.0;
        let mut t_j = 1.0;
        for i in 0..40 {
            t_s += 1.0 / 60.0;
            // Alternating long/short frames: same mean, visible judder.
            t_j += if i % 2 == 0 { 1.0 / 40.0 } else { 1.0 / 120.0 };
            steady.push_presented(t_s);
            jittery.push_presented(t_j);
        }
        let a = steady.stats().expect("steady");
        let b = jittery.stats().expect("jittery");
        assert!(a.judder_ms < 0.01, "{a:?}");
        assert!(
            b.judder_ms > 5.0,
            "alternating cadence should read as judder: {b:?}"
        );
    }
}
