//! GPU timing capture for Phase 0 bound-ness benchmarking.
//!
//! Attaches an `addCompletedHandler:` to the MetalFX command buffer and reads
//! `GPUEndTime - GPUStartTime` to get the *actual GPU active time* for that
//! command buffer — the primary signal for distinguishing GPU-bound from
//! CPU/sim/present-bound frames on Apple silicon (Codex plan review: total
//! frame time is vsync-pinned to ProMotion refresh buckets and cannot
//! discriminate bound-ness on its own).
//!
//! ## Metric caveat (Codex review item C)
//!
//! `GPUStartTime/GPUEndTime` measure GPU active time for *this one command
//! buffer*. It captures the MetalFX upscale pass and everything else wgpu
//! encoded onto the same buffer, but **excludes** any GPU work wgpu split
//! onto other command buffers / submissions in the same frame. Treat
//! `gpu_ms` as "GPU cost of the render+upscale command buffer", and verify
//! against a Metal System Trace once that the frame really is one relevant
//! command buffer before reading the verdict as total-frame GPU cost.
//!
//! ## Safety model (Codex review items A/B/D)
//!
//! - The raw command-buffer pointer is **borrowed**, never retained/owned and
//!   never stored. We only read timestamps via the buffer handed to the block.
//! - `addCompletedHandler:` is observational; we never commit/enqueue/wait or
//!   mutate lifecycle. Multiple handlers are permitted.
//! - The completion block runs later on a Metal-owned thread. It captures only
//!   an `Arc<GpuTimingSink>`, uses `try_lock`, allocates nothing, logs nothing,
//!   and cannot panic across the ObjC block boundary.

use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::runtime::ProtocolObject;
use objc2_metal::MTLCommandBuffer;

/// How many recent GPU-elapsed samples to retain.
const RING_CAPACITY: usize = 240;

/// Thread-safe ring of recent GPU-elapsed-time samples (milliseconds).
///
/// Written from the Metal completion-handler thread, read from the Bevy/debug
/// thread. The mutex is held only for a single push or a snapshot copy.
#[derive(Debug, Default)]
pub struct GpuTimingSink {
    inner: Mutex<Ring>,
}

#[derive(Debug, Default)]
struct Ring {
    samples: Vec<f32>,
    next: usize,
}

impl GpuTimingSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Ring {
                samples: Vec::with_capacity(RING_CAPACITY),
                next: 0,
            }),
        })
    }

    /// Push one GPU-elapsed sample (ms). Called from the Metal completion thread.
    /// Uses `try_lock` and never blocks the GPU callback; a dropped sample under
    /// contention is acceptable for a statistical harness.
    fn push(&self, gpu_ms: f32) {
        if let Ok(mut ring) = self.inner.try_lock() {
            if ring.samples.len() < RING_CAPACITY {
                ring.samples.push(gpu_ms);
            } else {
                let i = ring.next;
                ring.samples[i] = gpu_ms;
                ring.next = (i + 1) % RING_CAPACITY;
            }
        }
    }

    /// Snapshot the current samples, **oldest first**, for percentile
    /// computation on the reader side.
    ///
    /// Chronological order is load-bearing once the ring has wrapped. The
    /// backing `Vec` is circular: after saturation the oldest sample sits at
    /// `next`, not at index 0, so returning it raw hands the caller a sequence
    /// with a discontinuity at an arbitrary offset. Percentiles survive that
    /// (they sort anyway), but anything that slices by position — "the samples
    /// after warmup ended", the only way to separate a measurement window from
    /// pipeline compilation — silently reads the wrong elements.
    pub fn snapshot(&self) -> Vec<f32> {
        self.inner
            .lock()
            .map(|r| {
                if r.samples.len() < RING_CAPACITY {
                    r.samples.clone()
                } else {
                    let mut v = Vec::with_capacity(r.samples.len());
                    v.extend_from_slice(&r.samples[r.next..]);
                    v.extend_from_slice(&r.samples[..r.next]);
                    v
                }
            })
            .unwrap_or_default()
    }

    /// Mean / p50 / p99 of the current samples in ms. Returns `None` if empty.
    pub fn stats(&self) -> Option<GpuTimingStats> {
        let mut s = self.snapshot();
        if s.is_empty() {
            return None;
        }
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = s.len();
        let mean = s.iter().sum::<f32>() / n as f32;
        let p = |q: f32| s[((q * (n as f32 - 1.0)).round() as usize).min(n - 1)];
        Some(GpuTimingStats {
            count: n,
            mean_ms: mean,
            p50_ms: p(0.50),
            p99_ms: p(0.99),
        })
    }
}

/// Summary statistics over the GPU-elapsed ring (milliseconds).
#[derive(Debug, Clone, Copy)]
pub struct GpuTimingStats {
    pub count: usize,
    pub mean_ms: f32,
    pub p50_ms: f32,
    pub p99_ms: f32,
}

/// Attach a completion handler to a *borrowed* MetalFX command buffer that
/// records its GPU active time (ms) into `sink`.
///
/// Call this once per frame, before wgpu commits the buffer, from inside the
/// same `as_hal_mut` closure that already holds the raw command-buffer pointer.
///
/// # Safety
/// `cmd_buf_ptr` must be a valid, non-null `id<MTLCommandBuffer>` borrowed from
/// wgpu-hal's encoder for the current frame. The pointer is not retained or
/// stored; only `addCompletedHandler:` is called on it.
pub unsafe fn add_gpu_timing_handler(cmd_buf_ptr: *mut std::ffi::c_void, sink: Arc<GpuTimingSink>) {
    if cmd_buf_ptr.is_null() {
        return;
    }
    // Borrow the raw id as an objc2 MTLCommandBuffer protocol object. Same
    // underlying ObjC id as wgpu's metal-0.32 handle; we do NOT take ownership.
    let cmd_buf: &ProtocolObject<dyn MTLCommandBuffer> =
        unsafe { &*(cmd_buf_ptr as *const ProtocolObject<dyn MTLCommandBuffer>) };

    // The completion block captures only the Arc<sink>. It runs later on a
    // Metal-owned thread: read timestamps from the buffer handed to the block,
    // validate finiteness, push, and return — no allocation, no panic.
    let handler = RcBlock::new(
        move |finished: std::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            // SAFETY: Metal hands us a valid command buffer for the duration of the call.
            let fb = unsafe { finished.as_ref() };
            let start = fb.GPUStartTime();
            let end = fb.GPUEndTime();
            let elapsed = end - start;
            if start > 0.0 && elapsed.is_finite() && elapsed > 0.0 {
                sink.push((elapsed * 1000.0) as f32);
            }
        },
    );

    // `addCompletedHandler:` copies the block, so passing a borrowed pointer to
    // our RcBlock is sound; Metal retains its own copy for the callback.
    cmd_buf.addCompletedHandler(&*handler as *const _ as *mut _);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ring that reports its contents in storage order is fine for percentiles
    /// and wrong for anything positional. The consumer that matters — "the
    /// samples taken after warmup ended" — is positional, so wrapping must not
    /// rotate the sequence out from under it.
    #[test]
    fn snapshot_reports_samples_oldest_first_after_wrapping() {
        let sink = GpuTimingSink::new();
        // Overfill by a quarter so the write head ends mid-buffer, which is the
        // case a plain `.clone()` of the backing Vec gets wrong.
        let total = RING_CAPACITY + RING_CAPACITY / 4;
        for i in 0..total {
            sink.push(i as f32);
        }

        let got = sink.snapshot();
        assert_eq!(got.len(), RING_CAPACITY, "the ring must stay at capacity");

        let expected: Vec<f32> = ((total - RING_CAPACITY)..total).map(|i| i as f32).collect();
        assert_eq!(
            got, expected,
            "snapshot must be oldest-first: the survivors are the last \
             {RING_CAPACITY} pushed, in push order"
        );
    }

    /// Below capacity the ring is a plain append, and must stay one.
    #[test]
    fn snapshot_is_push_order_before_wrapping() {
        let sink = GpuTimingSink::new();
        for i in 0..8 {
            sink.push(i as f32);
        }
        assert_eq!(
            sink.snapshot(),
            vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]
        );
    }
}
