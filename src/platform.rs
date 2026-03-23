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

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLPixelFormat, MTLTexture};
use objc2_metal_fx::{
    MTLFXFrameInterpolator, MTLFXFrameInterpolatorBase, MTLFXFrameInterpolatorDescriptor,
    MTLFXSpatialScaler, MTLFXSpatialScalerBase, MTLFXSpatialScalerDescriptor,
    MTLFXTemporalScaler, MTLFXTemporalScalerBase, MTLFXTemporalScalerDescriptor,
};

// Link MetalFX.framework. MetalFX symbols are called through objc_msgSend
// (ObjC runtime dispatch), not direct C linkage, so no unresolved symbols.
#[link(name = "MetalFX", kind = "framework")]
extern "C" {}

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
    }

    unsafe { descriptor.newTemporalScalerWithDevice(device) }
}

/// Set textures and encode a temporal upscale pass.
///
/// # Safety
/// - All pointers must be valid Metal objects from wgpu-hal's raw handles.
/// - No Metal render/compute encoder may be active on the command buffer.
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
    if color_ptr.is_null() || depth_ptr.is_null() || motion_ptr.is_null()
        || output_ptr.is_null() || cmd_buf_ptr.is_null()
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

/// Spawn a background thread to create a temporal scaler (avoids blocking the render thread).
///
/// # Safety
/// `device_ptr` must be a valid `id<MTLDevice>` pointer that outlives the thread.
pub(crate) unsafe fn spawn_temporal_scaler_thread(
    device_ptr: *mut c_void,
    iw: usize, ih: usize, ow: usize, oh: usize,
    color_fmt_raw: usize,
    tx: std::sync::mpsc::Sender<Option<super::node::SendScaler>>,
) {
    // Wrapper to make raw pointer Send-able for thread transfer.
    struct SendablePtr(usize); // Store as usize to avoid *mut c_void !Send
    unsafe impl Send for SendablePtr {}

    let dev = SendablePtr(device_ptr as usize);

    std::thread::spawn(move || {
        let cfmt: MTLPixelFormat = unsafe { std::mem::transmute(color_fmt_raw) };
        let ptr = dev.0 as *mut c_void;
        log::info!("MetalFX: background thread starting temporal scaler creation");
        let scaler = unsafe {
            try_create_temporal_scaler_from_raw(
                ptr,
                iw, ih, ow, oh,
                cfmt, cfmt,
                MTLPixelFormat::Depth32Float,
                MTLPixelFormat::RG16Float,
            )
        };
        log::info!("MetalFX: background thread done, scaler={}", scaler.is_some());
        let _ = tx.send(scaler.map(super::node::SendScaler::Temporal));
    });
}

/// Check if frame interpolation is supported on this device (macOS 26+).
///
/// # Safety
/// `device_ptr` must be a valid `id<MTLDevice>` pointer.
#[allow(dead_code)] // Reserved for future use (runtime capability check).
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
) -> Option<Retained<ProtocolObject<dyn MTLFXFrameInterpolator>>> {
    if device_ptr.is_null() {
        return None;
    }
    let device: &ProtocolObject<dyn MTLDevice> =
        unsafe { &*(device_ptr as *const ProtocolObject<dyn MTLDevice>) };

    if !unsafe { MTLFXFrameInterpolatorDescriptor::supportsDevice(device) } {
        log::warn!("MetalFX: frame interpolation not supported on this device (requires macOS 26+)");
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
    }

    unsafe { descriptor.newFrameInterpolatorWithDevice(device) }
}

/// Set textures and encode a frame interpolation pass.
///
/// # Safety
/// All pointers must be valid Metal objects. No encoder may be active on the command buffer.
#[allow(clippy::too_many_arguments)]
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
    if color_ptr.is_null() || prev_color_ptr.is_null() || depth_ptr.is_null()
        || motion_ptr.is_null() || output_ptr.is_null() || cmd_buf_ptr.is_null()
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

        interpolator.setShouldResetHistory(reset_history);

        interpolator.encodeToCommandBuffer(cmd_buf);
    }
}

/// Spawn a background thread to create a frame interpolator.
///
/// # Safety
/// `device_ptr` must be a valid `id<MTLDevice>` pointer that outlives the thread.
pub(crate) unsafe fn spawn_frame_interpolator_thread(
    device_ptr: *mut c_void,
    iw: usize, ih: usize, ow: usize, oh: usize,
    color_fmt_raw: usize,
    tx: std::sync::mpsc::Sender<Option<super::node::SendScaler>>,
) {
    struct SendablePtr(usize);
    unsafe impl Send for SendablePtr {}

    let dev = SendablePtr(device_ptr as usize);

    std::thread::spawn(move || {
        let cfmt: MTLPixelFormat = unsafe { std::mem::transmute(color_fmt_raw) };
        let ptr = dev.0 as *mut c_void;
        log::info!("MetalFX: background thread starting frame interpolator creation");
        let interpolator = unsafe {
            try_create_frame_interpolator_from_raw(
                ptr,
                iw, ih, ow, oh,
                cfmt, cfmt,
                MTLPixelFormat::Depth32Float,
                MTLPixelFormat::RG16Float,
            )
        };
        log::info!("MetalFX: background thread done, interpolator={}", interpolator.is_some());
        let _ = tx.send(interpolator.map(super::node::SendScaler::FrameInterpolator));
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
