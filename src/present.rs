//! Display-timed dual presentation for MetalFX frame interpolation — macOS only.
//!
//! # Why this module exists
//!
//! Frame interpolation only buys frame rate if the synthesised frame is
//! *displayed*. A Bevy render graph presents its swapchain exactly once per
//! `App::update()`, so an interpolated frame computed inside a render node has
//! nowhere to go — it is correct, and invisible.
//!
//! Presenting twice per update cannot be done through `wgpu`: its only
//! presentation entry point is `SurfaceTexture::present()`, which bottoms out
//! in `[MTLCommandBuffer presentDrawable:]` with no time argument and no way to
//! present a second drawable. So this module reaches past `wgpu` to the
//! `CAMetalLayer` itself, acquires its own drawable, and presents it on the
//! render graph's own command buffer.
//!
//! # Status: implemented, not yet validated
//!
//! Whether this actually raises the displayed frame rate is **unverified**.
//! Every measurement so far was taken with the macOS session locked and the
//! display asleep — a state in which the compositor presents nothing to a
//! panel, so `presentedTime` stays 0 and presented-handlers never fire for
//! *any* drawable, including Bevy's own. The observed "0 frames displayed"
//! is a property of that environment, not of this code.
//!
//! Established regardless: Metal accepts every present, the debug layer is
//! clean, the command buffer carrying the presents commits and completes every
//! frame, and drawable acquisition never fails. Unestablished: that a frame
//! reaches the display, the presented rate, and the ordering behaviour.
//!
//! Validating it needs an unlocked session with the display awake. The
//! instrumentation is already here — [`PresentSink`] reports presented rate,
//! interval spread, ordering inversions and drops from real `presentedTime`
//! values.
//!
//! # Frame sequence it aims for
//!
//! Interpolation synthesises the frame *between* the previous real frame and
//! the current one, so the interpolated frame must be shown first. Bevy
//! presents its swapchain image untimed at the end of the graph, which always
//! lands on the earlier of two vsyncs — so the interpolated frame is the one
//! handed to Bevy, and the real frame is the one presented here, held back by
//! one refresh:
//!
//! ```text
//! update N
//! ├─ interp(N-1, N)  → Bevy's swapchain image, untimed   (vsync k)
//! └─ real N          → our drawable, +1 refresh          (vsync k+1)
//! ```
//!
//! The reverse ordering was tried first and cannot work: two untimed presents
//! issued microseconds apart collapse onto the same vsync, and delaying ours to
//! separate them would display the interpolated frame *after* the real frame it
//! was built from.
//!
//! The cost is one display interval of latency on the real frame — the standard
//! frame-generation trade, and unavoidable: the interpolated frame cannot be
//! built until the frame after it exists.
//!
//! # Why the interpolated frame is drawn with a render pass, not a blit
//!
//! `wgpu` configures the layer with `framebufferOnly = true` (it derives that
//! from the surface being used only as a colour target). A framebuffer-only
//! drawable texture is **not a legal blit destination** — `copyFromTexture:`
//! into it fails. So the interpolated texture is drawn into the drawable with
//! the same `BlitPipeline` render pass the node already uses for its normal
//! swapchain path, targeting a `wgpu::Texture` wrapped around the drawable's
//! raw `MTLTexture`.
//!
//! # Drawable budget
//!
//! `wgpu` sets `maximumDrawableCount = maximum_frame_latency + 1` (3 by
//! default) and holds one drawable per frame itself. Taking a second one leaves
//! a single spare, so `nextDrawable` can legitimately return nil under
//! pressure. That is treated as a dropped interpolated frame, counted, and
//! skipped — never waited on, because blocking here would stall the render
//! thread behind the display.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{class, msg_send};
use objc2_metal::{MTLCommandBuffer, MTLDrawable};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> i32;
}

/// Whether the main display is awake enough for presentation to mean anything.
///
/// This exists because presentation telemetry is silently meaningless on a
/// sleeping or locked machine: the compositor sends nothing to a panel, so
/// `MTLDrawable.presentedTime` stays 0 and presented-handlers never fire for
/// *any* drawable — the engine's own included. Every configuration then measures
/// identically zero, which reads exactly like a code defect and is not one.
///
/// Uniform null results across independent mechanisms are the tell. Check this
/// before believing any of them.
pub fn display_awake() -> bool {
    // SAFETY: both are pure CoreGraphics queries over a valid display ID.
    unsafe { CGDisplayIsAsleep(CGMainDisplayID()) == 0 }
}

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

    fn push_committed(&self) {
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
            .map(|r| (r.encoded, r.dropped, r.presented.len(), r.callbacks, r.committed))
            .unwrap_or((0, 0, 0, 0, 0))
    }

    /// Raw pointer to the shared presented-handler block, creating it on first
    /// use. Leaked on purpose — it must outlive every drawable it is attached
    /// to, and there is exactly one per sink.
    fn presented_handler_block(self: &Arc<Self>) -> usize {
        *self.handler_block.get_or_init(|| {
            let sink = Arc::clone(self);
            let block = RcBlock::new(
                move |presented: NonNull<ProtocolObject<dyn MTLDrawable>>| {
                    sink.push_callback();
                    // SAFETY: Metal hands us a live drawable for the call.
                    let t = unsafe { presented.as_ref() }.presentedTime();
                    if t.is_finite() && t > 0.0 {
                        sink.push_presented(t);
                    }
                },
            );
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

/// A drawable acquired from the layer, owned until presented.
pub struct AcquiredDrawable {
    drawable: Retained<ProtocolObject<dyn MTLDrawable>>,
    /// `id<MTLTexture>` owned by the drawable. Borrowed, not retained here.
    texture: NonNull<AnyObject>,
}

/// Locate the `CAMetalLayer` that `wgpu` renders into for this `NSView`.
///
/// `wgpu` only adopts the view's root layer when it is already a
/// `CAMetalLayer` (an `MTKView`, or a custom `layerClass`). For an ordinary
/// winit window it installs a **sublayer** instead, so the root layer is a
/// plain `CALayer` and the Metal layer is one level down. Both shapes are
/// handled here; anything else means `wgpu` changed its surface strategy and
/// dual presentation must stay off rather than guess.
///
/// # Safety
/// `ns_view` must be a valid `NSView`. Must be called on the main thread —
/// `-[NSView layer]` is not thread-safe.
pub unsafe fn find_metal_layer(ns_view: *mut c_void) -> Option<NonNull<c_void>> {
    if ns_view.is_null() {
        return None;
    }
    unsafe {
        let view: *mut AnyObject = ns_view.cast();
        let root: *mut AnyObject = msg_send![view, layer];
        if root.is_null() {
            return None;
        }

        let metal_class = class!(CAMetalLayer);
        let root_is_metal: bool = msg_send![root, isKindOfClass: metal_class];
        if root_is_metal {
            return NonNull::new(root.cast());
        }

        let sublayers: *mut AnyObject = msg_send![root, sublayers];
        if sublayers.is_null() {
            return None;
        }
        let count: usize = msg_send![sublayers, count];
        for i in 0..count {
            let layer: *mut AnyObject = msg_send![sublayers, objectAtIndex: i];
            if layer.is_null() {
                continue;
            }
            let is_metal: bool = msg_send![layer, isKindOfClass: metal_class];
            if is_metal {
                log_layer_identity(layer, count);
                return NonNull::new(layer.cast());
            }
        }
        None
    }
}

/// Log what the located layer actually is, once.
///
/// Layer identity is the failure mode worth catching early here: acquiring
/// drawables from a `CAMetalLayer` that is not the one being composited
/// succeeds silently and displays nothing.
unsafe fn log_layer_identity(layer: *mut AnyObject, sublayer_count: usize) {
    unsafe {
        let size: CGSize = msg_send![layer, drawableSize];
        let pixel_format: usize = msg_send![layer, pixelFormat];
        let device: *mut AnyObject = msg_send![layer, device];
        let max_drawables: usize = msg_send![layer, maximumDrawableCount];
        let framebuffer_only: bool = msg_send![layer, framebufferOnly];
        let display_sync: bool = msg_send![layer, displaySyncEnabled];
        log::info!(
            "MetalFX dual presentation: layer {:p} — drawableSize {}x{}, pixelFormat {}, \
             maxDrawables {}, framebufferOnly {}, displaySync {}, device {}, \
             {} sublayer(s) on the root",
            layer,
            size.width,
            size.height,
            pixel_format,
            max_drawables,
            framebuffer_only,
            display_sync,
            if device.is_null() { "none" } else { "set" },
            sublayer_count,
        );
    }
}

// CoreGraphics geometry types. `msg_send!` needs each struct's ObjC type
// encoding to select the right return/argument ABI.
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

unsafe impl objc2::Encode for CGSize {
    const ENCODING: objc2::Encoding =
        objc2::Encoding::Struct("CGSize", &[objc2::Encoding::Double, objc2::Encoding::Double]);
}
unsafe impl objc2::Encode for CGPoint {
    const ENCODING: objc2::Encoding =
        objc2::Encoding::Struct("CGPoint", &[objc2::Encoding::Double, objc2::Encoding::Double]);
}
unsafe impl objc2::Encode for CGRect {
    const ENCODING: objc2::Encoding =
        objc2::Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}

/// Build a `CAMetalLayer` of our own, stacked directly above the one `wgpu`
/// renders into.
///
/// Sharing `wgpu`'s layer is awkward by construction: it acquires that layer's
/// drawable in Bevy's `prepare_windows`, before the render graph runs, and holds
/// it for the whole frame, so anything a render node acquires is the newer of
/// two outstanding drawables on a queue it does not control.
///
/// Owning a layer sidesteps the question: its drawable queue is ours, so both
/// the interpolated and the real frame can be presented and paced. `wgpu` keeps
/// presenting its own layer underneath, hidden behind this one.
///
/// Note this is a design argument, not a measured one — presentation has not yet
/// been validated on an unlocked display (see the module docs).
///
/// Mirrors the reference layer's geometry, scale, pixel format and device so the
/// two are visually interchangeable.
///
/// # Safety
/// `reference` must be a valid `CAMetalLayer` already in the view hierarchy.
/// Must be called on the main thread — it mutates the layer tree.
pub unsafe fn create_owned_layer(
    reference: NonNull<c_void>,
    ns_view: *mut c_void,
) -> Option<(NonNull<c_void>, NonNull<c_void>)> {
    unsafe {
        let reference: *mut AnyObject = reference.as_ptr().cast();

        let superlayer: *mut AnyObject = msg_send![reference, superlayer];
        if superlayer.is_null() {
            log::warn!("MetalFX dual presentation: wgpu's layer has no superlayer to attach to");
            return None;
        }
        let device: *mut AnyObject = msg_send![reference, device];
        if device.is_null() {
            log::warn!("MetalFX dual presentation: wgpu's layer has no MTLDevice yet");
            return None;
        }

        let pixel_format: usize = msg_send![reference, pixelFormat];
        let drawable_size: CGSize = msg_send![reference, drawableSize];
        let scale: f64 = msg_send![reference, contentsScale];

        // Geometry comes from the superlayer's bounds, not the reference
        // layer's frame. wgpu's observer layer leaves its own frame at zero and
        // tracks the superlayer's bounds through an observer, so copying its
        // frame yields a zero-sized layer — which is never composited, and a
        // layer that is never composited never has its presents take effect.
        let ref_frame: CGRect = msg_send![reference, frame];
        let super_bounds: CGRect = msg_send![superlayer, bounds];
        let frame = if ref_frame.size.width > 0.0 && ref_frame.size.height > 0.0 {
            ref_frame
        } else {
            super_bounds
        };
        log::info!(
            "MetalFX dual presentation: geometry — wgpu layer frame {}x{}, \
             superlayer bounds {}x{}, using {}x{}",
            ref_frame.size.width,
            ref_frame.size.height,
            super_bounds.size.width,
            super_bounds.size.height,
            frame.size.width,
            frame.size.height,
        );

        let cls = class!(CAMetalLayer);
        let layer: *mut AnyObject = msg_send![cls, layer];
        if layer.is_null() {
            return None;
        }

        // Layer-tree mutations must land in a committed CATransaction, or
        // CoreAnimation may never give the layer a display association — and a
        // layer with no display association still vends drawables while
        // rejecting every present against them, which is exactly the observed
        // failure (nextDrawable succeeds, presentation callbacks never fire).
        let transaction = class!(CATransaction);
        let _: () = msg_send![transaction, begin];

        let _: () = msg_send![layer, setDevice: device];
        let _: () = msg_send![layer, setPixelFormat: pixel_format];
        let _: () = msg_send![layer, setDrawableSize: drawable_size];
        let _: () = msg_send![layer, setContentsScale: scale];
        let _: () = msg_send![layer, setFrame: frame];
        // Display-paced: presentation must be tied to the refresh, or the two
        // frames collapse into one interval instead of occupying two.
        let _: () = msg_send![layer, setDisplaySyncEnabled: true];
        // Triple buffering — this layer carries two presents per update.
        let _: () = msg_send![layer, setMaximumDrawableCount: 3usize];
        // NOT framebuffer-only: we own this layer, and allowing its drawables
        // to be blit destinations lets the whole present sequence happen on one
        // command buffer we control — acquire, copy, present, commit — which is
        // the only shape measured to actually work (see examples/present_repro.rs).
        let _: () = msg_send![layer, setFramebufferOnly: false];
        let _: () = msg_send![layer, setOpaque: true];

        // Host the layer on an NSView of our own, rather than adding it as a
        // bare sibling sublayer.
        //
        // This is the last structural difference from the known-good control,
        // which is view-backed. A bare sublayer still vends drawables while
        // rejecting every present against them, which is exactly the observed
        // failure; a view-backed layer is what AppKit actually wires up for
        // display.
        let mut hosted = false;
        if !ns_view.is_null() {
            let parent: *mut AnyObject = ns_view.cast();
            let view_alloc: *mut AnyObject = msg_send![class!(NSView), alloc];
            let sub_view: *mut AnyObject = msg_send![view_alloc, initWithFrame: frame];
            if !sub_view.is_null() {
                let _: () = msg_send![sub_view, setWantsLayer: true];
                let _: () = msg_send![sub_view, setLayer: layer];
                let _: () = msg_send![parent, addSubview: sub_view];
                hosted = true;
            }
        }
        if !hosted {
            let _: () = msg_send![superlayer, addSublayer: layer];
        }
        log::info!(
            "MetalFX dual presentation: layer hosted on {}",
            if hosted { "its own NSView" } else { "a bare sublayer (fallback)" }
        );

        let _: () = msg_send![transaction, commit];
        // Force the transaction out now rather than at the end of the current
        // run-loop turn, so the layer is live before the first present.
        let _: () = msg_send![transaction, flush];
        // Retained by its superlayer; keep our own reference for the process
        // lifetime so the pointer stays valid.
        let _: *mut AnyObject = msg_send![layer, retain];

        log::info!(
            "MetalFX dual presentation: owned CAMetalLayer created ({}x{}, format {}, scale {}) \
             above wgpu's — presentation is no longer shared",
            drawable_size.width,
            drawable_size.height,
            pixel_format,
            scale,
        );
        // A command queue of our own. Presenting on wgpu's graph command buffer
        // is the remaining suspect for presents that never take effect, and the
        // known-good control uses its own queue and buffer.
        let queue: *mut AnyObject = msg_send![device, newCommandQueue];
        let queue = NonNull::new(queue.cast::<c_void>())?;

        Some((NonNull::new(layer.cast())?, queue))
    }
}

/// Acquire a drawable for our own present.
///
/// Returns `None` when the layer has no drawable free. That is an ordinary,
/// expected outcome — `wgpu` holds one of the layer's drawables for the whole
/// frame — and the caller should skip interpolated presentation for this frame
/// rather than wait.
///
/// # Safety
/// `layer` must be a valid `CAMetalLayer`.
pub unsafe fn acquire_drawable(layer: NonNull<c_void>) -> Option<AcquiredDrawable> {
    unsafe {
        let layer_obj: *mut AnyObject = layer.as_ptr().cast();
        // `nextDrawable` returns an autoreleased object; retain it so it
        // survives to the end of the frame, past any autorelease pool drain.
        let raw: *mut AnyObject = msg_send![layer_obj, nextDrawable];
        if raw.is_null() {
            return None;
        }
        let texture: *mut AnyObject = msg_send![raw, texture];
        let texture = NonNull::new(texture)?;

        let drawable: Retained<ProtocolObject<dyn MTLDrawable>> =
            Retained::retain(raw.cast())?;

        // Validate the cast once, with the single cheapest MTLDrawable call.
        // If the ProtocolObject cast is wrong this throws, and a throwing
        // selector is indistinguishable at a distance from a present that was
        // accepted and skipped — the ambiguity that cost this investigation
        // the most time.
        static CHECKED: std::sync::Once = std::sync::Once::new();
        CHECKED.call_once(|| {
            let id = drawable.drawableID();
            log::info!("MetalFX dual presentation: drawable cast OK — drawableID {id}");
        });

        Some(AcquiredDrawable { drawable, texture })
    }
}

// ---------------------------------------------------------------------------
// Bevy wiring
// ---------------------------------------------------------------------------

/// Dual-presentation state, mirrored from the main world into the render world.
///
/// The `CAMetalLayer` can only be looked up on the main thread, but the extra
/// present has to be encoded from the render world — hence a main-world system
/// that finds the layer once and an `ExtractResource` that carries the pointer
/// across.
#[derive(bevy::prelude::Resource, Clone, bevy::render::extract_resource::ExtractResource)]
pub struct MetalFxDualPresent {
    /// `CAMetalLayer` pointer as a `usize` so the resource stays `Send`. Zero
    /// until the surface exists — the window is created after the plugin.
    layer: usize,
    /// `MTLCommandQueue` of our own, for presents that must not ride on wgpu's
    /// command buffer. Zero until the layer is created.
    queue: usize,
    /// Presentation telemetry, shared with whoever wants to read it.
    pub sink: Arc<PresentSink>,
    /// Master switch. Off means the node behaves exactly as before: one
    /// present per update, interpolated frame computed but not shown.
    pub enabled: bool,
    /// Assumed display refresh interval, in seconds.
    ///
    /// The real frame is held back by this much so it lands on the vsync after
    /// the interpolated one instead of collapsing into the same one.
    pub refresh_interval: f64,
    /// Present only the real frame, skipping the interpolated one.
    ///
    /// The measurement control. It routes the baseline through the exact same
    /// present path — same layer, same instrument — so an A/B differs only in
    /// the number of frames presented. Without it the baseline has no
    /// presentation telemetry at all and its rate has to be inferred, which is
    /// not a sound comparison.
    pub single_present: bool,
}

impl Default for MetalFxDualPresent {
    fn default() -> Self {
        Self {
            layer: 0,
            queue: 0,
            sink: PresentSink::new(),
            // Off by default: on this Bevy/wgpu stack the second present is
            // never displayed (see the module docs), and enabling it would
            // hand Bevy's swapchain the interpolated frame while the real one
            // goes nowhere. Opt in only to investigate.
            enabled: false,
            // ProMotion. Wrong only for non-120Hz panels, where the two
            // frames are spaced by the wrong interval but still both present.
            refresh_interval: 1.0 / 120.0,
            single_present: false,
        }
    }
}

impl MetalFxDualPresent {
    /// Configure dual presentation.
    ///
    /// A constructor rather than a struct literal because the layer pointer is
    /// private — it is discovered at runtime, never supplied by the caller.
    pub fn new(sink: Arc<PresentSink>, enabled: bool) -> Self {
        Self {
            sink,
            enabled,
            ..Default::default()
        }
    }

    /// Present only the real frame — the instrumented baseline.
    pub fn with_single_present(mut self, single: bool) -> Self {
        self.single_present = single;
        self
    }

    /// Override the assumed display refresh interval (default: 120Hz).
    pub fn with_refresh_interval(mut self, seconds: f64) -> Self {
        self.refresh_interval = seconds;
        self
    }

    /// The layer to present on, once one has been found.
    pub fn layer(&self) -> Option<NonNull<c_void>> {
        if !self.enabled {
            return None;
        }
        NonNull::new(self.layer as *mut c_void)
    }

    /// Our own command queue, once the layer exists.
    pub fn queue(&self) -> Option<NonNull<c_void>> {
        NonNull::new(self.queue as *mut c_void)
    }

    /// Whether a layer has been located yet.
    pub fn has_layer(&self) -> bool {
        self.layer != 0
    }
}

/// Main-world system: find the `CAMetalLayer` `wgpu` is rendering into.
///
/// Runs every frame until it succeeds rather than once at startup, because the
/// window — and therefore the surface and its layer — is created after plugin
/// build. Once found, the pointer is stable for the window's lifetime.
pub fn capture_metal_layer(
    mut state: bevy::prelude::ResMut<MetalFxDualPresent>,
    windows: bevy::prelude::Query<&bevy::window::RawHandleWrapper>,
) {
    if state.layer != 0 || !state.enabled {
        return;
    }
    let Ok(handle) = windows.single() else {
        return;
    };
    let raw_window_handle::RawWindowHandle::AppKit(appkit) = handle.get_window_handle() else {
        return;
    };

    // SAFETY: Bevy's `Update` schedule runs on the main thread, which is where
    // `-[NSView layer]` must be read, and the handle is a live `NSView`.
    let found = unsafe { find_metal_layer(appkit.ns_view.as_ptr()) };

    match found {
        Some(wgpu_layer) => {
            // Do not present on wgpu's layer — it owns that layer's drawable
            // queue for the whole frame and a second drawable is never
            // displayed. Stack our own layer above it instead.
            //
            // SAFETY: main thread (Bevy's `Update`), and `wgpu_layer` is live.
            let Some((owned, queue)) =
                (unsafe { create_owned_layer(wgpu_layer, appkit.ns_view.as_ptr()) })
            else {
                return;
            };
            state.layer = owned.as_ptr() as usize;
            state.queue = queue.as_ptr() as usize;
            log::info!("MetalFX dual presentation: active on an owned layer");
        }
        None => {
            // Not fatal, and not worth a per-frame log: without a layer the
            // node simply keeps its single-present behaviour.
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                log::warn!(
                    "MetalFX dual presentation: no CAMetalLayer found on the window's NSView; \
                     interpolated frames will be computed but not presented"
                );
            });
        }
    }
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
        assert!(s.judder_ms < 0.01, "steady cadence should show no judder: {s:?}");
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
        assert!(b.judder_ms > 5.0, "alternating cadence should read as judder: {b:?}");
    }

    #[test]
    fn null_pointers_are_rejected_rather_than_dereferenced() {
        // SAFETY: passing null is exactly the case under test.
        assert!(unsafe { find_metal_layer(std::ptr::null_mut()) }.is_none());
    }
}

/// Present the interpolated and real frames from the render graph command
/// buffer's *completion* handler, on a command buffer of our own.
///
/// This is the only sequence measured to work. `examples/present_repro.rs`
/// establishes the shape: acquire the drawable, write it, present it, and commit
/// — all on one buffer we own, committed immediately. Every variant that
/// acquired a drawable mid-graph and then either rode wgpu's command buffer or
/// deferred the commit measured zero presentation callbacks, because by the time
/// the commit happened the drawable had been recycled.
///
/// Running at completion time also removes the need for any synchronisation: the
/// graph's work is finished, so `interp_tex` and `real_tex` hold final data, and
/// the drawables are acquired fresh rather than held across the frame.
///
/// # Safety
/// `cmd_buf_ptr` must be the live, uncommitted graph command buffer; `layer` and
/// `queue` ours; the textures live `id<MTLTexture>` at the layer's drawable size.
#[allow(clippy::too_many_arguments)]
pub unsafe fn present_pair_deferred(
    cmd_buf_ptr: *mut c_void,
    layer: NonNull<c_void>,
    queue: NonNull<c_void>,
    interp_tex: *mut c_void,
    real_tex: *mut c_void,
    refresh_interval: f64,
    sink: &Arc<PresentSink>,
    single: bool,
) {
    if cmd_buf_ptr.is_null() || interp_tex.is_null() || real_tex.is_null() {
        return;
    }
    unsafe {
        let cmd_buf: &ProtocolObject<dyn MTLCommandBuffer> =
            &*(cmd_buf_ptr as *const ProtocolObject<dyn MTLCommandBuffer>);

        // The layer is pinned to BGRA8Unorm_sRGB (81) and the sources are BGRA
        // staging textures, so the blit copy is format-identical. Taking the
        // format from the source instead would adopt the view's RGBA order,
        // which CAMetalLayer does not support — it accepts such presents and
        // then silently skips them.
        static FORMAT_SET: std::sync::Once = std::sync::Once::new();
        FORMAT_SET.call_once(|| {
            let layer_obj: *mut AnyObject = layer.as_ptr().cast();
            let _: () = msg_send![layer_obj, setPixelFormat: 81usize];
            log::info!("MetalFX dual presentation: owned layer pinned to BGRA8Unorm_sRGB");
        });

        // Retain the sources so they survive until the handler runs.
        let _: *mut AnyObject = msg_send![interp_tex.cast::<AnyObject>(), retain];
        let _: *mut AnyObject = msg_send![real_tex.cast::<AnyObject>(), retain];

        let layer_addr = layer.as_ptr() as usize;
        let queue_addr = queue.as_ptr() as usize;
        let interp_addr = interp_tex as usize;
        let real_addr = real_tex as usize;
        let sink = Arc::clone(sink);

        let handler = RcBlock::new(
            move |_done: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                // Reaching here proves the graph's command buffer completed —
                // the middle of the encoded/committed/callbacks triad that
                // separates "never ran" from "ran but was skipped".
                sink.push_committed();
                let layer_obj = layer_addr as *mut AnyObject;

                // In single-present mode only the real frame is shown; the
                // interpolated one is still computed, so the GPU cost is
                // identical and the A/B isolates presentation alone.
                let interp_drawable = if single {
                    None
                } else {
                    match acquire_drawable(NonNull::new_unchecked(layer_addr as *mut c_void)) {
                        Some(d) => Some(d),
                        None => {
                            sink.push_dropped();
                            return;
                        }
                    }
                };
                let Some(real_drawable) =
                    acquire_drawable(NonNull::new_unchecked(layer_addr as *mut c_void))
                else {
                    sink.push_dropped();
                    return;
                };
                let _ = layer_obj;

                let queue_obj = queue_addr as *mut AnyObject;
                let cb: *mut AnyObject = msg_send![queue_obj, commandBuffer];
                if cb.is_null() {
                    sink.push_dropped();
                    return;
                }
                let blit: *mut AnyObject = msg_send![cb, blitCommandEncoder];
                if blit.is_null() {
                    sink.push_dropped();
                    return;
                }
                if let Some(d) = &interp_drawable {
                    let _: () = msg_send![
                        blit,
                        copyFromTexture: interp_addr as *mut AnyObject,
                        toTexture: d.texture.as_ptr()
                    ];
                }
                let _: () = msg_send![
                    blit,
                    copyFromTexture: real_addr as *mut AnyObject,
                    toTexture: real_drawable.texture.as_ptr()
                ];
                let _: () = msg_send![blit, endEncoding];

                let block_ptr = sink.presented_handler_block() as *mut _;
                if let Some(d) = &interp_drawable {
                    d.drawable.addPresentedHandler(block_ptr);
                }
                real_drawable.drawable.addPresentedHandler(block_ptr);

                let cb_obj: &ProtocolObject<dyn MTLCommandBuffer> =
                    &*(cb as *const ProtocolObject<dyn MTLCommandBuffer>);
                // Interpolated frame first — it depicts the earlier moment — and
                // the real frame one refresh later, so the two occupy
                // consecutive intervals instead of collapsing into one.
                if let Some(d) = &interp_drawable {
                    cb_obj.presentDrawable(&d.drawable);
                    cb_obj.presentDrawable_afterMinimumDuration(
                        &real_drawable.drawable,
                        refresh_interval,
                    );
                    sink.push_encoded();
                } else {
                    cb_obj.presentDrawable(&real_drawable.drawable);
                }
                cb_obj.commit();
                sink.push_encoded();
            },
        );
        cmd_buf.addCompletedHandler(&*handler as *const _ as *mut _);
        // The handler must outlive this call; Metal copies it, but the block is
        // cheap and leaking one per frame would not be, so hold it via the
        // completion registration only.
        core::mem::forget(handler);
    }
}
