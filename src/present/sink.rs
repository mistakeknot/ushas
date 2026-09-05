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
    /// Retained positive `presentedTime` reports in CoreAnimation's timebase.
    ///
    /// One handler serves both real and interpolated drawables on the owned
    /// layer. Samples can be lost under lock contention; frame kind and source
    /// identity are not retained.
    ///
    /// It is a ring: the most recent `RING_CAPACITY` samples and no more, which
    /// is the window the interval statistics are computed over. Anything that
    /// needs a count of frames displayed must read `displayed`, not this
    /// length.
    presented: Vec<f64>,
    next: usize,
    /// Retained positive presentation reports since reset, cumulative.
    ///
    /// `presented.len()` saturates at `RING_CAPACITY`, so past 480 presents it
    /// reads 480 for the rest of the run while `encoded` and `callbacks` keep
    /// climbing. Reported side by side with those, that reads as presentation
    /// having stopped, which is why this counter exists.
    ///
    /// Incremented under the same `try_lock` as every other counter here: a
    /// sample dropped for contention is not counted, exactly as for
    /// `callbacks`.
    displayed: u64,
    /// Presents skipped because no drawable was available.
    dropped: u64,
    /// Presents actually encoded onto a command buffer. Distinguishes "the
    /// path never ran" from "it ran but nothing reached the display".
    encoded: u64,
    /// Retained presentation callbacks, regardless of their timestamp.
    /// Zero `presentedTime` means not yet presented or dropped. This counter
    /// and the positive-timestamp update can lose samples independently.
    callbacks: u64,
    /// Retained completion callbacks for the presentation command buffer.
    /// Completion alone does not establish successful work or presentation.
    committed: u64,
    /// Non-increasing positive timestamps in callback lock-acquisition order.
    /// Without frame identities this cannot establish content ordering.
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

    /// Record one presented frame — any frame the owned layer displayed, real
    /// or interpolated. Called from Metal's callback thread; drops the sample
    /// rather than blocking under contention.
    fn push_presented(&self, t: f64) {
        let Ok(mut ring) = self.inner.try_lock() else {
            return;
        };
        ring.displayed += 1;
        // Compare callback observations, not intended frame/content order.
        // Equal times and reordered callback delivery can both increment this.
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

    /// Note a present that could not be issued — no drawable free, or no
    /// command buffer. Either frame of a pair can hit this, so it counts
    /// skipped presents, not skipped interpolated frames.
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
    /// [`Self::stats`]: `(encoded, dropped, displayed, callbacks, committed)`.
    ///
    /// All five are cumulative since the last [`Self::reset`] — `displayed`
    /// included, which is why it is a counter and not `presented.len()`. That
    /// length is the ring's occupancy, capped at `RING_CAPACITY`, so past 480
    /// presents it reads 480 for the rest of the run while the other four keep
    /// climbing; reported alongside them it looks exactly like presentation
    /// having stopped.
    ///
    /// Together they separate "the path never ran" (`encoded` zero) from "no
    /// drawable was free" (`dropped`) from "encoded and never displayed"
    /// (`encoded` high, `displayed` zero).
    pub fn counts(&self) -> (u64, u64, u64, u64, u64) {
        self.inner
            .lock()
            .map(|r| (r.encoded, r.dropped, r.displayed, r.callbacks, r.committed))
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
            ring.displayed = 0;
            ring.dropped = 0;
            ring.encoded = 0;
            ring.callbacks = 0;
            ring.committed = 0;
            ring.inversions = 0;
            // `last_presented` deliberately survives: it anchors inversion
            // detection across the reset boundary.
        }
    }

    /// Summarise retained presentation reports for both frame kinds.
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
            presented_fps: if mean > 0.0 { 1000.0 / mean } else { 0.0 },
            mean_interval_ms: mean,
            p50_interval_ms: p(0.50),
            p99_interval_ms: p(0.99),
            judder_ms: variance.sqrt(),
            dropped,
            inversions,
        })
    }
}

/// Summary of retained presentation reports and counters for the owned layer.
///
/// Rate and interval fields derive from positive `MTLDrawable.presentedTime`
/// reports, not render-loop timing. These are Metal's reported onscreen times,
/// not independent panel, content-order, or input-latency measurements. Lock
/// contention can lose telemetry, and callbacks carry no measurement epoch.
///
/// One presented-handler serves every drawable this crate presents, so these
/// cover the real frames as well as the synthesised ones and there is no
/// per-kind breakdown here. Adding a render rate to `presented_fps` to
/// manufacture one counts the real frames twice.
#[derive(Debug, Clone, Copy)]
pub struct PresentStats {
    /// Number of positive intervals used by the summary, plus one.
    /// Duplicate or nonfinite intervals are excluded.
    pub count: usize,
    /// Rate at which frames reached the display, over the most recent 480
    /// samples: the *total* through the owned layer, real and interpolated
    /// together. Nothing needs adding to it.
    pub presented_fps: f32,
    pub mean_interval_ms: f32,
    pub p50_interval_ms: f32,
    pub p99_interval_ms: f32,
    /// Standard deviation of retained positive presentation intervals.
    /// This is a pacing diagnostic, not a complete visual judder assessment.
    pub judder_ms: f32,
    /// Presents skipped for want of a drawable — either frame of a pair.
    pub dropped: u64,
    /// Non-increasing positive timestamps in callback lock-acquisition order.
    /// This includes equal timestamps and does not establish content order.
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
        assert!((s.presented_fps - 60.0).abs() < 0.5, "{s:?}");
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
    fn displayed_counts_every_frame_while_the_ring_saturates() {
        let sink = PresentSink::new();
        let n = RING_CAPACITY + 120;
        for i in 0..n {
            sink.push_presented(1.0 + i as f64 / 120.0);
        }
        let (_, _, displayed, _, _) = sink.counts();
        assert_eq!(
            displayed, n as u64,
            "displayed must count every frame, not saturate with the ring"
        );
        assert_eq!(
            sink.inner.lock().unwrap().presented.len(),
            RING_CAPACITY,
            "the ring itself is still capped — that is why the counter exists"
        );
    }

    #[test]
    fn reset_clears_the_displayed_counter() {
        let sink = PresentSink::new();
        sink.push_presented(1.0);
        sink.push_presented(1.016);
        sink.reset();
        let (_, _, displayed, _, _) = sink.counts();
        assert_eq!(displayed, 0);
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
