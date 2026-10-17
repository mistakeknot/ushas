//! Acquiring drawables and the one present sequence measured to work.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_metal::{MTLCommandBuffer, MTLDrawable};

use super::PresentSink;

/// A drawable acquired from the layer, owned until presented.
pub struct AcquiredDrawable {
    drawable: Retained<ProtocolObject<dyn MTLDrawable>>,
    /// `id<MTLTexture>` owned by the drawable. Borrowed, not retained here.
    texture: NonNull<AnyObject>,
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

        let drawable: Retained<ProtocolObject<dyn MTLDrawable>> = Retained::retain(raw.cast())?;

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

        // Retain the sources so they survive until the handler runs: wgpu may
        // drop the staging textures on a resize while this frame's completion
        // is still pending. The guard releases them again when the block is
        // destroyed, which is after the handler has run and Metal has let go
        // of its copy. Until 0.5 the retain had no matching release, so every
        // staging texture ever created outlived the process, and a resize or
        // mode change leaked two full-resolution BGRA textures of GPU memory.
        let _: *mut AnyObject = msg_send![interp_tex.cast::<AnyObject>(), retain];
        let _: *mut AnyObject = msg_send![real_tex.cast::<AnyObject>(), retain];
        let staging = RetainedPair(interp_tex as usize, real_tex as usize);

        let handler = completion_handler(
            staging,
            layer.as_ptr() as usize,
            queue.as_ptr() as usize,
            Arc::clone(sink),
            single,
            refresh_interval,
        );
        // `addCompletedHandler:` copies the block, so Metal holds its own
        // reference until the handler has run; ours can go at the end of this
        // scope, exactly as `gpu_timing::add_gpu_timing_handler` does. Until
        // 0.5 this reference was `mem::forget`-ed instead, which is the leak
        // the comment next to it said it was avoiding: one block per frame,
        // each holding an `Arc` to the sink, for the life of the process.
        cmd_buf.addCompletedHandler(&*handler as *const _ as *mut _);
    }
}

/// The block Metal calls when the graph's command buffer completes.
type CompletionBlock = RcBlock<dyn Fn(NonNull<ProtocolObject<dyn MTLCommandBuffer>>)>;

/// The completion block that presents both frames, owning `staging` until
/// the block itself is destroyed.
///
/// `staging` must be captured as a whole, and the way it is read inside the
/// closure decides that. Rust 2021 closures capture the places they use, not
/// the variables: `let RetainedPair(a, b) = staging;` uses only the two `Copy`
/// fields, so a `move` closure copies those two addresses and leaves the guard
/// behind in the caller's frame, where it drops, and releases both textures,
/// before the handler has run. That is the first shape this fix had. A method
/// call borrows `&staging`, which names the whole guard, so the guard moves
/// into the block and drops with it. `retention_tests` pins this with a real
/// block and real Objective-C retain counts.
fn completion_handler(
    staging: RetainedPair,
    layer_addr: usize,
    queue_addr: usize,
    sink: Arc<PresentSink>,
    single: bool,
    refresh_interval: f64,
) -> CompletionBlock {
    RcBlock::new(
        move |_done: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| unsafe {
            // Reaching here proves the graph's command buffer completed —
            // the middle of the encoded/committed/callbacks triad that
            // separates "never ran" from "ran but was skipped".
            sink.push_committed();
            let (interp_addr, real_addr) = staging.addresses();
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
    )
}

/// Two retained `MTLTexture` addresses, released when the completion block
/// that captured them is destroyed.
struct RetainedPair(usize, usize);

impl RetainedPair {
    /// Both addresses, through a borrow of the whole guard: see
    /// [`completion_handler`] for why the closure must read them this way.
    fn addresses(&self) -> (usize, usize) {
        (self.0, self.1)
    }
}

impl Drop for RetainedPair {
    fn drop(&mut self) {
        // SAFETY: both addresses were retained by `present_pair_deferred`
        // before this pair was created, and nothing else releases them.
        unsafe {
            let _: () = msg_send![self.0 as *mut AnyObject, release];
            let _: () = msg_send![self.1 as *mut AnyObject, release];
        }
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use objc2_foundation::NSObject;

    unsafe fn retain_count(addr: usize) -> usize {
        unsafe { msg_send![addr as *mut AnyObject, retainCount] }
    }

    /// The guard must live inside the block, not in the caller's frame. One
    /// object stands in for both textures, retained twice as
    /// `present_pair_deferred` retains each texture once: three references
    /// while the block lives, one once it is destroyed. The destructuring
    /// capture this fix first shipped with read one here, right after the
    /// block was built.
    #[test]
    fn the_completion_block_owns_the_retained_textures_until_it_is_destroyed() {
        let object = NSObject::new();
        let addr = &*object as *const NSObject as usize;
        unsafe {
            let _: *mut AnyObject = msg_send![addr as *mut AnyObject, retain];
            let _: *mut AnyObject = msg_send![addr as *mut AnyObject, retain];
            assert_eq!(retain_count(addr), 3);
        }

        let block = completion_handler(
            RetainedPair(addr, addr),
            usize::MAX,
            usize::MAX,
            PresentSink::new(),
            false,
            1.0 / 60.0,
        );
        unsafe {
            assert_eq!(
                retain_count(addr),
                3,
                "building the block must not release the textures"
            );
        }
        // Metal's copy of the block is a second reference to the same heap
        // block; dropping ours first, as the registration path does, must not
        // release anything either.
        let metal_copy = block.clone();
        drop(block);
        unsafe {
            assert_eq!(
                retain_count(addr),
                3,
                "the caller's reference is not the last"
            );
        }
        drop(metal_copy);
        unsafe {
            assert_eq!(
                retain_count(addr),
                1,
                "destroying the last reference releases both textures"
            );
        }
    }
}
