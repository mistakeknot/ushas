//! Bevy wiring: the extracted resource and the main-world system that finds
//! the layer once the window exists.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use super::layer::{create_owned_layer, find_metal_layer};
use super::PresentSink;

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
