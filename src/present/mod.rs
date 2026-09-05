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
//! present a second drawable. So this module reaches past `wgpu` to
//! CoreAnimation directly.
//!
//! # Shape of the solution
//!
//! A `CAMetalLayer` of our own, on its own `NSView`, stacked above the one
//! `wgpu` renders into. Both frames are presented from it; Bevy's swapchain
//! image still carries the real frame and is simply covered.
//!
//! Three properties of that layer are load-bearing, and each was a separate
//! silent failure before it was set (see `shadow-work-shb1`):
//!
//! - `framebufferOnly = false`, so a drawable is a legal blit destination.
//!   `wgpu`'s own layer sets this true, which is why a drawable taken from
//!   *its* layer has to be drawn into rather than copied into.
//! - `pixelFormat` pinned to `BGRA8Unorm_sRGB`. `CAMetalLayer` supports BGRA
//!   channel order only; give it the view's RGBA format and CoreAnimation
//!   accepts every present and then skips it, with no error anywhere.
//! - `displaySyncEnabled = true` and `maximumDrawableCount = 3`.
//!
//! # Per-frame sequence
//!
//! ```text
//! in the render node          each frame is drawn into a BGRA staging texture
//!                             by a fullscreen blit pass (a blit *copy* cannot
//!                             convert channel order, a render pass can)
//!
//! on graph cmdbuf completion  acquire two drawables fresh
//!                             blit staging -> drawable, on OUR queue
//!                             present interp untimed
//!                             present real  +1 refresh interval
//!                             commit
//! ```
//!
//! The presents deliberately do **not** happen inside the node. Encoding them
//! on the graph's own command buffer yields zero accepted presents: a drawable
//! acquired mid-graph has been recycled by the time that buffer commits, and
//! the present is discarded silently. Moving acquire-copy-present-commit into
//! the graph buffer's completion handler — where the graph's work is finished,
//! so the staging textures are final and ordering is free — took accepted
//! presents from 0/758 to 757/758.
//!
//! Interpolated frame first: it depicts the moment *between* the previous real
//! frame and this one. The real frame is held back one refresh interval with
//! `presentDrawable:afterMinimumDuration:`, because two untimed presents issued
//! microseconds apart collapse onto the same vsync and CoreAnimation keeps only
//! the later one. The cost is one display interval of latency on the real
//! frame — the standard frame-generation trade, and unavoidable, since the
//! interpolated frame cannot be built until the frame after it exists.
//!
//! # Status: presents accepted, display unverified
//!
//! Measured, with both arms through the same layer and the same telemetry so
//! only the present count differs:
//!
//! ```text
//! baseline (single present)   403 presents   403 callbacks   26.9 fps render
//! dual present                802 presents   801 callbacks   26.7 fps render
//! ```
//!
//! That is 1.99x the accepted-present rate at an unchanged render rate.
//!
//! What is **not** established is that any of it reaches the panel.
//! `MTLDrawable.presentedTime` never populates on the development machine —
//! not for this crate, and not for a minimal, maximally visible Metal window
//! either (`crates/sw-renderer/scripts/present-probe-visible.swift` reports
//! `encoded=881 callbacks=881 presented=0`). Presented frame rate is therefore
//! unmeasurable there by any implementation, correct or not, and the
//! accepted-present rate above is a proxy for it, not a substitute.
//!
//! [`PresentSink`] records real `presentedTime` values and reports presented
//! rate, interval spread (judder), ordering inversions and drops, so the
//! measurement needs only hardware where that signal works. See
//! `shadow-work-au59`.
//!
//! # Drawable budget
//!
//! Our layer allows three drawables and we take two per frame, so
//! `nextDrawable` can legitimately return nil under pressure. That is counted
//! as a drop and skipped — never waited on, because blocking here would stall
//! the render thread behind the display.

pub mod drawable;
pub mod layer;
pub mod resource;
pub mod sink;

pub use drawable::{acquire_drawable, present_pair_deferred, AcquiredDrawable};
pub use layer::{create_owned_layer, find_metal_layer};
pub use resource::{capture_metal_layer, MetalFxDualPresent};
pub use sink::{PresentSink, PresentStats};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> i32;
}

/// Whether CoreGraphics reports the main display as awake.
///
/// This does not test session lock, application visibility, or panel delivery.
/// A sleeping or unavailable display can prevent rendering or leave presentation
/// telemetry absent. Preserve that environmental failure instead of treating
/// missing callbacks as a fast run or a renderer defect. An awake result still
/// needs separate surface, image, and presentation checks.
pub fn display_awake() -> bool {
    // SAFETY: both are pure CoreGraphics queries over a valid display ID.
    unsafe { CGDisplayIsAsleep(CGMainDisplayID()) == 0 }
}
