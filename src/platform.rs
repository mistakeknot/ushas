//! macOS-specific MetalFX implementation.
//!
//! This module contains all platform-gated code that depends on objc2-metal-fx,
//! objc2-metal, and the MetalFX.framework.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2_metal::MTLDevice;
use objc2_metal::MTLPixelFormat;
use objc2_metal_fx::{MTLFXSpatialScaler, MTLFXSpatialScalerDescriptor};

// Link MetalFX.framework. The is_available() check gates all usage at runtime.
// On macOS < 13, this framework doesn't exist — the binary still links because
// MetalFX symbols are only called through objc_msgSend (ObjC runtime dispatch),
// not through direct C symbol linkage, so there are no unresolved symbols at load time.
// The #[link] directive tells the linker to search for the framework but MetalFX
// classes are looked up at runtime via AnyClass::get(), not at link time.
#[link(name = "MetalFX", kind = "framework")]
extern "C" {}

/// Runtime check for MetalFX availability.
/// Checks whether the MTLFXSpatialScalerDescriptor class exists in the runtime.
pub(crate) fn is_available_impl() -> bool {
    AnyClass::get(c"MTLFXSpatialScalerDescriptor").is_some()
}

/// Attempt to create a spatial scaler for the given Metal device.
///
/// Returns `None` if the device/format combination is unsupported,
/// or if MetalFX is not available on this system.
///
/// This is the Phase 1 proof-of-concept: can we create a MetalFX scaler
/// from `objc2-metal-fx` bindings?
#[allow(dead_code)]
pub fn try_create_spatial_scaler(
    device: &ProtocolObject<dyn MTLDevice>,
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
) -> Option<Retained<ProtocolObject<dyn MTLFXSpatialScaler>>> {
    if !is_available_impl() {
        return None;
    }

    // Safety: MTLFXSpatialScalerDescriptor::new() is safe — it creates a new
    // autoreleased descriptor that we immediately retain.
    let descriptor = unsafe { MTLFXSpatialScalerDescriptor::new() };

    // Safety: all setters write to ObjC properties. Values are validated
    // by newSpatialScalerWithDevice which returns nil on invalid config.
    unsafe {
        descriptor.setInputWidth(input_width);
        descriptor.setInputHeight(input_height);
        descriptor.setOutputWidth(output_width);
        descriptor.setOutputHeight(output_height);

        // Color format: BGRA8Unorm (standard Metal surface format on macOS)
        descriptor.setColorTextureFormat(MTLPixelFormat::BGRA8Unorm);
        // Output format: same as color
        descriptor.setOutputTextureFormat(MTLPixelFormat::BGRA8Unorm);
    }

    // Note: Spatial scaler does NOT take a depth texture — that is temporal-only.

    // Create the scaler. Returns None if the device doesn't support MetalFX
    // or if the format/dimension combination is invalid.
    unsafe { descriptor.newSpatialScalerWithDevice(device) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metalfx_availability() {
        let available = is_available_impl();
        println!("MetalFX available: {available}");
        // This test passes on both macOS 13+ (true) and older (false).
        // It should never panic.
    }
}
