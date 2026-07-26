//! Finding `wgpu`'s `CAMetalLayer` and building the one we present on.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

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
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
        "CGSize",
        &[objc2::Encoding::Double, objc2::Encoding::Double],
    );
}
unsafe impl objc2::Encode for CGPoint {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
        "CGPoint",
        &[objc2::Encoding::Double, objc2::Encoding::Double],
    );
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
            if hosted {
                "its own NSView"
            } else {
                "a bare sublayer (fallback)"
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_pointers_are_rejected_rather_than_dereferenced() {
        // SAFETY: passing null is exactly the case under test.
        assert!(unsafe { find_metal_layer(std::ptr::null_mut()) }.is_none());
    }
}
