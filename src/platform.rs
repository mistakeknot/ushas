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
use objc2_metal_fx::{MTLFXSpatialScaler, MTLFXSpatialScalerBase, MTLFXSpatialScalerDescriptor};

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
pub unsafe fn try_create_spatial_scaler_from_raw(
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
pub unsafe fn encode_spatial_upscale(
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

/// Map a wgpu TextureFormat to the corresponding MTLPixelFormat.
/// Returns None for formats that MetalFX doesn't support.
pub fn wgpu_format_to_mtl(
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
