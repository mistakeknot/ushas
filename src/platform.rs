//! macOS-specific MetalFX implementation.
//!
//! Uses `objc2-metal-fx` for MetalFX framework bindings.
//!
//! ## ObjC Runtime Interop
//!
//! wgpu-hal uses the `metal` crate v0.32 (built on `objc` v0.2 runtime),
//! while `objc2-metal-fx` uses `objc2` v0.6. Both wrap the same underlying
//! ObjC `id` pointers. The `interop` module provides unsafe bridge functions
//! to convert raw pointers between the two runtime families.

use std::ffi::c_void;

use bevy::render::render_resource::TextureUsages;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureUsage};
#[cfg(feature = "frame-interpolation")]
use objc2_metal_fx::{
    MTLFXFrameInterpolatableScaler, MTLFXFrameInterpolator, MTLFXFrameInterpolatorBase,
    MTLFXFrameInterpolatorDescriptor,
};
use objc2_metal_fx::{MTLFXSpatialScaler, MTLFXSpatialScalerBase, MTLFXSpatialScalerDescriptor};
#[cfg(feature = "temporal")]
use objc2_metal_fx::{MTLFXTemporalScaler, MTLFXTemporalScalerBase, MTLFXTemporalScalerDescriptor};

// Link MetalFX.framework. MetalFX symbols are called through objc_msgSend
// (ObjC runtime dispatch), not direct C linkage, so no unresolved symbols.
#[link(name = "MetalFX", kind = "framework")]
extern "C" {}

/// MetalFX states texture requirements as `MTLTextureUsage`; the textures this
/// crate can offer are described in wgpu's terms. Read is `TEXTURE_BINDING`,
/// write is `STORAGE_BINDING`, render target is `RENDER_ATTACHMENT`. Bits
/// MetalFX never asks for (`pixelFormatView`) have no wgpu counterpart here
/// and are dropped.
pub(crate) fn wgpu_usage_from_mtl(usage: MTLTextureUsage) -> TextureUsages {
    let mut out = TextureUsages::empty();
    if usage.contains(MTLTextureUsage::ShaderRead) {
        out |= TextureUsages::TEXTURE_BINDING;
    }
    if usage.contains(MTLTextureUsage::ShaderWrite) {
        out |= TextureUsages::STORAGE_BINDING;
    }
    if usage.contains(MTLTextureUsage::RenderTarget) {
        out |= TextureUsages::RENDER_ATTACHMENT;
    }
    out
}

/// Runtime check for MetalFX availability.
pub(crate) fn is_available_impl() -> bool {
    AnyClass::get(c"MTLFXSpatialScalerDescriptor").is_some()
}

/// Attempt to create a spatial scaler for the given Metal device.
///
/// Returns `None` if the device/format combination is unsupported,
/// or if MetalFX is not available on this system.
///
/// # Safety
/// `device_ptr` must be a valid `id<MTLDevice>` pointer from wgpu-hal's
/// `raw_device().lock().as_ptr()`.
pub(crate) unsafe fn try_create_spatial_scaler_from_raw(
    device_ptr: *mut c_void,
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    color_format: MTLPixelFormat,
    output_format: MTLPixelFormat,
) -> Option<Retained<ProtocolObject<dyn MTLFXSpatialScaler>>> {
    if !is_available_impl() {
        return None;
    }

    if device_ptr.is_null() {
        return None;
    }
    // Safety: cast raw id<MTLDevice> pointer to objc2's ProtocolObject.
    // Both runtime families wrap the same ObjC id pointer.
    let device: &ProtocolObject<dyn MTLDevice> =
        unsafe { &*(device_ptr as *const ProtocolObject<dyn MTLDevice>) };

    let descriptor = unsafe { MTLFXSpatialScalerDescriptor::new() };

    unsafe {
        descriptor.setInputWidth(input_width);
        descriptor.setInputHeight(input_height);
        descriptor.setOutputWidth(output_width);
        descriptor.setOutputHeight(output_height);
        descriptor.setColorTextureFormat(color_format);
        descriptor.setOutputTextureFormat(output_format);
    }

    // Spatial scaler does NOT take a depth texture — that is temporal-only.
    unsafe { descriptor.newSpatialScalerWithDevice(device) }
}

/// Set textures and encode a spatial upscale pass.
///
/// # Safety
/// - `scaler` must be a valid MTLFXSpatialScaler.
/// - `color_ptr`, `output_ptr`, `cmd_buf_ptr` must be valid Metal objects
///   from wgpu-hal's `raw_handle()` / `raw_command_buffer()`.
/// - No Metal render/compute encoder may be active on the command buffer.
pub(crate) unsafe fn encode_spatial_upscale(
    scaler: &ProtocolObject<dyn MTLFXSpatialScaler>,
    color_ptr: *mut c_void,
    output_ptr: *mut c_void,
    cmd_buf_ptr: *mut c_void,
    input_content_width: usize,
    input_content_height: usize,
) {
    // Safety: cast raw ObjC pointers to objc2 protocol references.
    // Both runtime families wrap the same ObjC id pointer layout.
    let color: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(color_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let output: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(output_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let cmd_buf: &ProtocolObject<dyn MTLCommandBuffer> =
        unsafe { &*(cmd_buf_ptr as *const ProtocolObject<dyn MTLCommandBuffer>) };

    unsafe {
        // Set per-frame textures
        scaler.setColorTexture(Some(color));
        scaler.setOutputTexture(Some(output));

        // Set actual rendered content dimensions (may differ from texture dimensions)
        scaler.setInputContentWidth(input_content_width);
        scaler.setInputContentHeight(input_content_height);

        // Encode the upscale operation into the command buffer
        scaler.encodeToCommandBuffer(cmd_buf);
    }
}

/// Attempt to create a temporal scaler for the given Metal device.
///
/// Returns `None` if the device/format combination is unsupported,
/// or if MetalFX is not available on this system.
///
/// # Safety
/// `device_ptr` must be a valid `id<MTLDevice>` pointer from wgpu-hal's
/// `raw_device().lock().as_ptr()`.
#[cfg(feature = "temporal")]
pub(crate) unsafe fn try_create_temporal_scaler_from_raw(
    device_ptr: *mut c_void,
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    color_format: MTLPixelFormat,
    output_format: MTLPixelFormat,
    depth_format: MTLPixelFormat,
    motion_format: MTLPixelFormat,
    dynamic_res: Option<(f32, f32)>,
) -> Option<Retained<ProtocolObject<dyn MTLFXTemporalScaler>>> {
    if !is_available_impl() {
        return None;
    }
    if device_ptr.is_null() {
        return None;
    }
    let device: &ProtocolObject<dyn MTLDevice> =
        unsafe { &*(device_ptr as *const ProtocolObject<dyn MTLDevice>) };

    let descriptor = unsafe { MTLFXTemporalScalerDescriptor::new() };

    unsafe {
        descriptor.setInputWidth(input_width);
        descriptor.setInputHeight(input_height);
        descriptor.setOutputWidth(output_width);
        descriptor.setOutputHeight(output_height);
        descriptor.setColorTextureFormat(color_format);
        descriptor.setOutputTextureFormat(output_format);
        descriptor.setDepthTextureFormat(depth_format);
        descriptor.setMotionTextureFormat(motion_format);
        descriptor.setAutoExposureEnabled(true);

        // True dynamic resolution: when enabled, the scaler accepts a *range*
        // of input scales without recreation. The descriptor's input size (set
        // above to the output size by the caller) is the maximum; the per-frame
        // `setInputContentWidth/Height` in `encode_temporal_upscale` then selects
        // the actual content size each frame, letting an adaptive governor flex
        // render scale with zero scaler rebuilds. Apple requires the input/output
        // aspect ratio to stay constant (our scaling is uniform, so this holds).
        //
        // `dynamic_res` is given as *render-scale fractions* (e.g. 0.5..=0.75 of
        // native). MetalFX's InputContentMin/MaxScale are *upscale ratios*
        // (output/input, always ≥ 1.0), so convert: a 0.5 render scale is a 2.0
        // upscale, a 0.75 render scale is a ~1.33 upscale. Min and max swap under
        // the reciprocal (smaller render fraction → larger upscale ratio).
        if let Some((min_render_scale, max_render_scale)) = dynamic_res {
            let metalfx_min_scale = 1.0 / max_render_scale;
            let metalfx_max_scale = 1.0 / min_render_scale;
            descriptor.setInputContentPropertiesEnabled(true);
            descriptor.setInputContentMinScale(metalfx_min_scale);
            descriptor.setInputContentMaxScale(metalfx_max_scale);
        }
    }

    unsafe { descriptor.newTemporalScalerWithDevice(device) }
}

/// The band of input-content scales the temporal scaler supports on this
/// device, stated the way MetalFX states it: as upscale ratios, `output /
/// input`, so `(1.0, 3.0)` means "from native down to one third of the output
/// resolution in each dimension".
///
/// Returns `None` when temporal scaling is unsupported on the device or when
/// the query itself is missing — it arrived with dynamic resolution in macOS
/// 14, and on 13 the class exists but the selector does not, so the call is
/// guarded rather than trusted. The pair is validated: both finite, minimum at
/// least `1.0` (a ratio below that is not an upscale), maximum not below the
/// minimum.
///
/// # Safety
/// `device_ptr` must be a valid `id<MTLDevice>` pointer from wgpu-hal's
/// `raw_device()`.
#[cfg(feature = "temporal")]
pub(crate) unsafe fn temporal_upscale_ratio_band_from_raw(
    device_ptr: *mut c_void,
) -> Option<(f32, f32)> {
    use objc2::ClassType;

    if !is_available_impl() {
        return None;
    }
    let device: &ProtocolObject<dyn MTLDevice> =
        unsafe { &*(device_ptr as *const ProtocolObject<dyn MTLDevice>) };

    // These are class methods, so the question goes to the metaclass:
    // `responds_to` on the class itself answers for *instance* methods and
    // says no, which quietly turns every device into a pre-M5 one.
    let metaclass = MTLFXTemporalScalerDescriptor::class().metaclass();
    if !metaclass.responds_to(objc2::sel!(supportedInputContentMinScaleForDevice:))
        || !metaclass.responds_to(objc2::sel!(supportedInputContentMaxScaleForDevice:))
    {
        return None;
    }
    if !unsafe { MTLFXTemporalScalerDescriptor::supportsDevice(device) } {
        return None;
    }
    let min =
        unsafe { MTLFXTemporalScalerDescriptor::supportedInputContentMinScaleForDevice(device) };
    let max =
        unsafe { MTLFXTemporalScalerDescriptor::supportedInputContentMaxScaleForDevice(device) };
    if !min.is_finite() || !max.is_finite() || min < 1.0 || max < min {
        return None;
    }
    Some((min, max))
}

/// Set textures and encode a temporal upscale pass.
///
/// # Safety
/// - All pointers must be valid Metal objects from wgpu-hal's raw handles.
/// - No Metal render/compute encoder may be active on the command buffer.
#[cfg(feature = "temporal")]
pub(crate) unsafe fn encode_temporal_upscale(
    scaler: &ProtocolObject<dyn MTLFXTemporalScaler>,
    color_ptr: *mut c_void,
    depth_ptr: *mut c_void,
    motion_ptr: *mut c_void,
    output_ptr: *mut c_void,
    cmd_buf_ptr: *mut c_void,
    input_content_width: usize,
    input_content_height: usize,
    jitter_offset_x: f32,
    jitter_offset_y: f32,
    motion_vector_scale_x: f32,
    motion_vector_scale_y: f32,
    reset: bool,
) {
    if color_ptr.is_null()
        || depth_ptr.is_null()
        || motion_ptr.is_null()
        || output_ptr.is_null()
        || cmd_buf_ptr.is_null()
    {
        log::error!("encode_temporal_upscale: received null pointer");
        return;
    }

    let color: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(color_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let depth: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(depth_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let motion: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(motion_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let output: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(output_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let cmd_buf: &ProtocolObject<dyn MTLCommandBuffer> =
        unsafe { &*(cmd_buf_ptr as *const ProtocolObject<dyn MTLCommandBuffer>) };

    unsafe {
        scaler.setColorTexture(Some(color));
        scaler.setDepthTexture(Some(depth));
        scaler.setMotionTexture(Some(motion));
        scaler.setOutputTexture(Some(output));

        scaler.setInputContentWidth(input_content_width);
        scaler.setInputContentHeight(input_content_height);

        scaler.setJitterOffsetX(jitter_offset_x);
        scaler.setJitterOffsetY(jitter_offset_y);

        scaler.setMotionVectorScaleX(motion_vector_scale_x);
        scaler.setMotionVectorScaleY(motion_vector_scale_y);

        // Bevy uses infinite reversed-Z: near=1.0, far=0.0.
        scaler.setDepthReversed(true);

        scaler.setReset(reset);

        scaler.encodeToCommandBuffer(cmd_buf);
    }
}

/// An owning, `Send` handle to the Metal device for background scaler creation.
///
/// The previous design passed `&**hal_dev.raw_device() as *mut c_void` -- a
/// BORROW of wgpu's device -- into `std::thread::spawn`, with the caller's
/// safety note claiming "the pointer does not outlive this scope". For the
/// synchronous spatial path that was true. For the temporal and
/// frame-interpolation paths it was false by construction: a detached thread
/// outlives the scope that spawned it, and nothing retained the object. The
/// process survived only because wgpu happened to keep its own reference alive.
///
/// All four SIGSEGVs recorded in shadow-work-ejgo faulted on this thread, inside
/// `newTemporalScalerWithDevice:` (EXC_BAD_ACCESS, KERN_INVALID_ADDRESS at 0x18).
/// That crash is unreproduced, so this is not a demonstrated fix for it -- but a
/// detached thread dereferencing an unretained Objective-C object is a defect on
/// its own terms, and it is exactly the shape that produces that fault.
///
/// `Retained` makes the lifetime a guarantee instead of a coincidence: the object
/// is retained before the spawn and released when the thread's copy drops.
#[cfg(feature = "temporal")]
pub(crate) struct SendDevice(pub(crate) Retained<ProtocolObject<dyn MTLDevice>>);

// SAFETY: `MTLDevice` is documented thread-safe, and `Retained` keeps the object
// alive for as long as this wrapper exists -- which is the whole point of it.
#[cfg(feature = "temporal")]
unsafe impl Send for SendDevice {}

/// Spawn a background thread to create a temporal scaler (avoids blocking the render thread).
///
/// Takes an owned [`SendDevice`] rather than a raw pointer: the thread is
/// detached, so it must keep the device alive itself.
#[cfg(feature = "temporal")]
pub(crate) fn spawn_temporal_scaler_thread(
    device: SendDevice,
    iw: usize,
    ih: usize,
    ow: usize,
    oh: usize,
    color_fmt_raw: usize,
    dynamic_res: Option<(f32, f32)>,
    tx: std::sync::mpsc::Sender<Option<super::node::SendScaler>>,
) {
    std::thread::spawn(move || {
        // MTLPixelFormat is a #[repr(transparent)] newtype over NSUInteger,
        // so we can construct it directly from the raw discriminant instead
        // of transmuting (which would be UB for out-of-range values).
        let cfmt = MTLPixelFormat(color_fmt_raw as objc2_foundation::NSUInteger);
        // Borrow from the retained handle the thread owns, so the object cannot
        // go away underneath this call.
        let ptr = &*device.0 as *const ProtocolObject<dyn MTLDevice> as *mut c_void;
        log::info!("MetalFX: background thread starting temporal scaler creation");
        let scaler = unsafe {
            try_create_temporal_scaler_from_raw(
                ptr,
                iw,
                ih,
                ow,
                oh,
                cfmt,
                cfmt,
                MTLPixelFormat::Depth32Float,
                MTLPixelFormat::RG16Float,
                dynamic_res,
            )
        };
        log::info!(
            "MetalFX: background thread done, scaler={}",
            scaler.is_some()
        );
        let _ = tx.send(scaler.map(super::node::SendScaler::Temporal));
    });
}

/// Check if frame interpolation is supported on this device (macOS 26+).
///
/// # Safety
/// `device_ptr` must be a valid `id<MTLDevice>` pointer.
#[allow(dead_code)] // Reserved for future use (runtime capability check).
#[cfg(feature = "frame-interpolation")]
pub(crate) unsafe fn is_frame_interpolation_supported(device_ptr: *mut c_void) -> bool {
    if device_ptr.is_null() {
        return false;
    }
    let device: &ProtocolObject<dyn MTLDevice> =
        unsafe { &*(device_ptr as *const ProtocolObject<dyn MTLDevice>) };
    unsafe { MTLFXFrameInterpolatorDescriptor::supportsDevice(device) }
}

/// Attempt to create a frame interpolator for the given Metal device.
///
/// Returns `None` if the device doesn't support frame interpolation (macOS < 26).
///
/// # Safety
/// `device_ptr` must be a valid `id<MTLDevice>` pointer.
/// `input_width`/`input_height` describe the **motion and depth** textures (the
/// low-res render size); `output_width`/`output_height` describe the **color**
/// textures *and* the output. Both `colorTexture` and `prevColorTexture` must
/// therefore be allocated at output size — the interpolator sits *after* the
/// upscaler in the pipeline, not in place of it. Getting this wrong trips a
/// MetalFX debug-layer assertion: "Color texture width mismatch from
/// descriptor".
///
/// `scaler` is the upscaler whose output feeds `colorTexture`. Attaching it
/// lets the interpolator reuse the scaler's internal history instead of
/// re-deriving it.
#[cfg(feature = "frame-interpolation")]
pub(crate) unsafe fn try_create_frame_interpolator_from_raw(
    device_ptr: *mut c_void,
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    color_format: MTLPixelFormat,
    output_format: MTLPixelFormat,
    depth_format: MTLPixelFormat,
    motion_format: MTLPixelFormat,
    scaler: Option<&ProtocolObject<dyn MTLFXFrameInterpolatableScaler>>,
) -> Option<Retained<ProtocolObject<dyn MTLFXFrameInterpolator>>> {
    if device_ptr.is_null() {
        return None;
    }
    let device: &ProtocolObject<dyn MTLDevice> =
        unsafe { &*(device_ptr as *const ProtocolObject<dyn MTLDevice>) };

    if !unsafe { MTLFXFrameInterpolatorDescriptor::supportsDevice(device) } {
        log::warn!(
            "MetalFX: frame interpolation not supported on this device (requires macOS 26+)"
        );
        return None;
    }

    let descriptor = unsafe { MTLFXFrameInterpolatorDescriptor::new() };

    unsafe {
        descriptor.setInputWidth(input_width);
        descriptor.setInputHeight(input_height);
        descriptor.setOutputWidth(output_width);
        descriptor.setOutputHeight(output_height);
        descriptor.setColorTextureFormat(color_format);
        descriptor.setOutputTextureFormat(output_format);
        descriptor.setDepthTextureFormat(depth_format);
        descriptor.setMotionTextureFormat(motion_format);
        if let Some(scaler) = scaler {
            descriptor.setScaler(Some(scaler));
        }
    }

    unsafe { descriptor.newFrameInterpolatorWithDevice(device) }
}

/// Set textures and encode a frame interpolation pass.
///
/// # Safety
/// All pointers must be valid Metal objects. No encoder may be active on the command buffer.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "frame-interpolation")]
pub(crate) unsafe fn encode_frame_interpolation(
    interpolator: &ProtocolObject<dyn MTLFXFrameInterpolator>,
    color_ptr: *mut c_void,
    prev_color_ptr: *mut c_void,
    depth_ptr: *mut c_void,
    motion_ptr: *mut c_void,
    output_ptr: *mut c_void,
    cmd_buf_ptr: *mut c_void,
    jitter_offset_x: f32,
    jitter_offset_y: f32,
    motion_vector_scale_x: f32,
    motion_vector_scale_y: f32,
    delta_time: f32,
    field_of_view: f32,
    aspect_ratio: f32,
    near_plane: f32,
    far_plane: f32,
    reset_history: bool,
) {
    if color_ptr.is_null()
        || prev_color_ptr.is_null()
        || depth_ptr.is_null()
        || motion_ptr.is_null()
        || output_ptr.is_null()
        || cmd_buf_ptr.is_null()
    {
        log::error!("encode_frame_interpolation: received null pointer");
        return;
    }

    let color: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(color_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let prev_color: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(prev_color_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let depth: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(depth_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let motion: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(motion_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let output: &ProtocolObject<dyn MTLTexture> =
        unsafe { &*(output_ptr as *const ProtocolObject<dyn MTLTexture>) };
    let cmd_buf: &ProtocolObject<dyn MTLCommandBuffer> =
        unsafe { &*(cmd_buf_ptr as *const ProtocolObject<dyn MTLCommandBuffer>) };

    unsafe {
        interpolator.setColorTexture(Some(color));
        interpolator.setPrevColorTexture(Some(prev_color));
        interpolator.setDepthTexture(Some(depth));
        interpolator.setMotionTexture(Some(motion));
        interpolator.setOutputTexture(Some(output));

        interpolator.setJitterOffsetX(jitter_offset_x);
        interpolator.setJitterOffsetY(jitter_offset_y);

        interpolator.setMotionVectorScaleX(motion_vector_scale_x);
        interpolator.setMotionVectorScaleY(motion_vector_scale_y);

        interpolator.setDeltaTime(delta_time);
        interpolator.setFieldOfView(field_of_view);
        interpolator.setAspectRatio(aspect_ratio);
        interpolator.setNearPlane(near_plane);
        interpolator.setFarPlane(far_plane);

        // Bevy renders with an infinite *reverse-Z* projection: the near plane
        // maps to depth 1.0 and the far plane to 0.0. MetalFX's
        // `isDepthReversed` means "zero represents the farthest distance",
        // which matches. It already defaults to true, but state it explicitly
        // so the assumption is visible at the call site rather than inherited.
        interpolator.setDepthReversed(true);

        interpolator.setShouldResetHistory(reset_history);

        interpolator.encodeToCommandBuffer(cmd_buf);
    }
}

/// Spawn a background thread to create a frame interpolator.
///
/// Takes an owned [`SendDevice`] for the same reason as the temporal spawn: a
/// detached thread must keep the device alive itself.
#[cfg(feature = "frame-interpolation")]
pub(crate) fn spawn_frame_interpolator_thread(
    device: SendDevice,
    iw: usize,
    ih: usize,
    ow: usize,
    oh: usize,
    color_fmt_raw: usize,
    tx: std::sync::mpsc::Sender<Option<super::node::SendScaler>>,
) {
    std::thread::spawn(move || {
        // MTLPixelFormat is a #[repr(transparent)] newtype over NSUInteger,
        // so we can construct it directly from the raw discriminant instead
        // of transmuting (which would be UB for out-of-range values).
        let cfmt = MTLPixelFormat(color_fmt_raw as objc2_foundation::NSUInteger);
        // Borrow from the retained handle the thread owns.
        let ptr = &*device.0 as *const ProtocolObject<dyn MTLDevice> as *mut c_void;
        log::info!("MetalFX: background thread starting frame interpolator creation");

        // Frame interpolation is a two-stage pipeline: the temporal scaler
        // upscales the low-res render to output size, then the interpolator
        // synthesises an intermediate frame from consecutive *upscaled* frames.
        // Both objects are built here so the render thread never blocks on
        // MetalFX's (multi-second) pipeline compilation.
        let scaler = unsafe {
            try_create_temporal_scaler_from_raw(
                ptr,
                iw,
                ih,
                ow,
                oh,
                cfmt,
                cfmt,
                MTLPixelFormat::Depth32Float,
                MTLPixelFormat::RG16Float,
                None,
            )
        };
        let Some(scaler) = scaler else {
            log::warn!("MetalFX: frame interpolation needs a temporal scaler, but creation failed");
            let _ = tx.send(None);
            return;
        };

        let interpolator = unsafe {
            try_create_frame_interpolator_from_raw(
                ptr,
                iw,
                ih,
                ow,
                oh,
                // Color textures live at *output* size (see the fn's doc).
                cfmt,
                cfmt,
                MTLPixelFormat::Depth32Float,
                MTLPixelFormat::RG16Float,
                Some(ProtocolObject::from_ref(&*scaler)),
            )
        };
        log::info!(
            "MetalFX: background thread done, interpolator={}",
            interpolator.is_some()
        );
        let _ =
            tx.send(
                interpolator.map(|interpolator| super::node::SendScaler::FrameInterpolator {
                    scaler,
                    interpolator,
                }),
            );
    });
}

/// Map a wgpu TextureFormat to the corresponding MTLPixelFormat.
/// Returns None for formats that MetalFX doesn't support.
pub(crate) fn wgpu_format_to_mtl(
    format: bevy::render::render_resource::TextureFormat,
) -> Option<MTLPixelFormat> {
    use bevy::render::render_resource::TextureFormat as WF;
    match format {
        WF::Bgra8Unorm => Some(MTLPixelFormat::BGRA8Unorm),
        WF::Bgra8UnormSrgb => Some(MTLPixelFormat::BGRA8Unorm_sRGB),
        WF::Rgba16Float => Some(MTLPixelFormat::RGBA16Float),
        WF::Rgba8Unorm => Some(MTLPixelFormat::RGBA8Unorm),
        WF::Rgba8UnormSrgb => Some(MTLPixelFormat::RGBA8Unorm_sRGB),
        WF::Depth32Float => Some(MTLPixelFormat::Depth32Float),
        WF::Rg16Float => Some(MTLPixelFormat::RG16Float),
        _ => {
            log::warn!("Unsupported wgpu TextureFormat for MetalFX: {format:?}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metalfx_availability() {
        let available = is_available_impl();
        println!("MetalFX available: {available}");
    }

    #[test]
    fn test_format_mapping() {
        use bevy::render::render_resource::TextureFormat as WF;

        assert_eq!(
            wgpu_format_to_mtl(WF::Bgra8Unorm),
            Some(MTLPixelFormat::BGRA8Unorm)
        );
        assert_eq!(
            wgpu_format_to_mtl(WF::Rgba16Float),
            Some(MTLPixelFormat::RGBA16Float)
        );
        assert_eq!(
            wgpu_format_to_mtl(WF::Depth32Float),
            Some(MTLPixelFormat::Depth32Float)
        );
        assert!(wgpu_format_to_mtl(WF::R8Unorm).is_none());
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    /// What Bevy allocates for a camera's main texture unless told otherwise.
    const BEVY_DEFAULT_MAIN: TextureUsages = TextureUsages::RENDER_ATTACHMENT
        .union(TextureUsages::TEXTURE_BINDING)
        .union(TextureUsages::COPY_SRC);

    #[test]
    fn usage_bits_map_one_to_one() {
        let all = MTLTextureUsage::ShaderRead
            | MTLTextureUsage::ShaderWrite
            | MTLTextureUsage::RenderTarget;
        assert_eq!(
            wgpu_usage_from_mtl(all),
            TextureUsages::TEXTURE_BINDING
                | TextureUsages::STORAGE_BINDING
                | TextureUsages::RENDER_ATTACHMENT
        );
        assert_eq!(
            wgpu_usage_from_mtl(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget),
            TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT
        );
        assert!(wgpu_usage_from_mtl(MTLTextureUsage::empty()).is_empty());
    }

    /// The reason the plugin adds `STORAGE_BINDING` to the camera in temporal
    /// mode and leaves spatial alone, pinned against the bits measured on an
    /// M5 Max: spatial output wants read|renderTarget, temporal adds write.
    #[test]
    fn bevys_default_main_texture_satisfies_spatial_but_not_temporal() {
        let spatial =
            wgpu_usage_from_mtl(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
        let temporal = wgpu_usage_from_mtl(
            MTLTextureUsage::ShaderRead
                | MTLTextureUsage::ShaderWrite
                | MTLTextureUsage::RenderTarget,
        );
        assert!(BEVY_DEFAULT_MAIN.contains(spatial));
        assert!(!BEVY_DEFAULT_MAIN.contains(temporal));
        assert!((BEVY_DEFAULT_MAIN | TextureUsages::STORAGE_BINDING).contains(temporal));
    }

    /// Ask a real spatial scaler. Whatever it requires must be satisfiable by
    /// Bevy's default main texture, or the direct-write path would silently
    /// fall back to a blit on every machine.
    #[test]
    fn a_real_spatial_scaler_fits_bevys_default_main_texture() {
        let Some(device) = objc2_metal::MTLCreateSystemDefaultDevice() else {
            eprintln!("no Metal device; skipping");
            return;
        };
        let device_ptr = &*device as *const ProtocolObject<dyn MTLDevice> as *mut c_void;
        let scaler = unsafe {
            try_create_spatial_scaler_from_raw(
                device_ptr,
                640,
                360,
                1280,
                720,
                MTLPixelFormat::RGBA8Unorm_sRGB,
                MTLPixelFormat::RGBA8Unorm_sRGB,
            )
        }
        .expect("a spatial scaler on this device");
        let required = wgpu_usage_from_mtl(unsafe { scaler.outputTextureUsage() });
        assert!(
            BEVY_DEFAULT_MAIN.contains(required),
            "spatial requires {required:?}"
        );
    }
}

#[cfg(all(test, feature = "temporal"))]
mod device_band_tests {
    use super::*;

    /// Ask the real device. This runs on whatever Mac executes the suite and
    /// pins two things: that the query reaches the class method at all — the
    /// first version asked the class instead of the metaclass, got "does not
    /// respond", and reported every device as pre-M5 without an error — and
    /// that the answer has the shape the crate relies on: a floor of exactly
    /// native and a ceiling of at least the half-resolution every supported
    /// device provides.
    #[test]
    fn the_device_reports_its_temporal_band() {
        let Some(device) = objc2_metal::MTLCreateSystemDefaultDevice() else {
            eprintln!("no Metal device; skipping");
            return;
        };
        let device_ptr = &*device as *const ProtocolObject<dyn MTLDevice> as *mut c_void;
        let band = unsafe { temporal_upscale_ratio_band_from_raw(device_ptr) };
        let (min, max) = band.expect("a Mac with MetalFX temporal support reports a band");
        assert_eq!(min, 1.0, "the floor is native");
        assert!(
            max >= 2.0,
            "every supported device reconstructs from at least one half: {max}"
        );
        eprintln!("device temporal upscale ratios: {min}..={max}");
    }
}
