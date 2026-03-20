//! Bevy plugin for Apple MetalFX upscaling and frame interpolation.
//!
//! Uses `objc2-metal-fx` for MetalFX framework bindings and integrates
//! as a render graph node replacing Bevy's built-in upscaling.
//!
//! ## Supported Modes
//! - **Spatial**: Single-frame ML upscaling (macOS 13+)
//! - **Temporal**: Multi-frame temporal upscaling with motion vectors (macOS 13+)
//! - **FrameInterpolation**: Generate intermediate frames (macOS 26+, Metal 4)

#[cfg(target_os = "macos")]
mod platform;

#[cfg(target_os = "macos")]
mod node;

#[cfg(target_os = "macos")]
pub use platform::*;

#[cfg(target_os = "macos")]
pub use node::{MetalFxConfig, MetalFxUpscaleNode};

/// Check whether MetalFX is available on this system at runtime.
///
/// Returns `false` on non-macOS platforms or when the MetalFX framework
/// is not present (macOS < 13).
pub fn is_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        platform::is_available_impl()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// MetalFX operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetalFxMode {
    /// Single-frame spatial upscaling. Needs only color input.
    /// Available on macOS 13+ with Apple Silicon.
    #[default]
    Spatial,
    /// Temporal upscaling with motion vectors + jitter.
    /// Better quality than spatial but requires MotionVectorPrepass.
    Temporal,
    /// Frame interpolation — generates intermediate frames between rendered frames.
    /// Requires macOS 26+ (Metal 4). Adds +1 frame of input latency.
    FrameInterpolation,
    /// Bypass MetalFX — render at full res with Bevy's default upscaling.
    /// Useful for A/B benchmarking.
    Disabled,
}

/// Configuration for the MetalFX plugin.
pub struct MetalFxPlugin {
    /// Render scale factor (0.25–1.0). Default 0.5 = half-res render.
    pub render_scale: f32,
    /// Which MetalFX mode to use.
    pub mode: MetalFxMode,
}

impl Default for MetalFxPlugin {
    fn default() -> Self {
        Self {
            render_scale: 0.5,
            mode: MetalFxMode::Spatial,
        }
    }
}

impl bevy::app::Plugin for MetalFxPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        assert!(
            (0.1..=1.0).contains(&self.render_scale),
            "MetalFxPlugin: render_scale must be in [0.1, 1.0], got {}",
            self.render_scale
        );

        if !is_available() {
            log::warn!("MetalFX is not available on this system — plugin disabled");
            return;
        }

        if self.mode == MetalFxMode::Disabled {
            log::info!("MetalFX mode is Disabled — bypassing");
            return;
        }

        log::info!(
            "MetalFX plugin initialized: mode={:?}, render_scale={}",
            self.mode,
            self.render_scale
        );

        #[cfg(target_os = "macos")]
        {
            use bevy::render::RenderApp;

            // Insert config resource into the render world.
            if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
                render_app.insert_resource(node::MetalFxConfig {
                    render_scale: self.render_scale,
                });

                // TODO Phase 2b: Add render graph node once command buffer
                // synchronization with wgpu's internal encoder state is solved.
                // The current encode_spatial_upscale call conflicts with wgpu's
                // active command encoder, causing GPU hangs. Need to either:
                // 1. Flush wgpu's encoder before MetalFX encode, or
                // 2. Use a separate command buffer for MetalFX
            }
        }
    }
}

/// Render graph label for the MetalFX upscale node.
#[derive(Debug, Hash, PartialEq, Eq, Clone, bevy::render::render_graph::RenderLabel)]
pub struct MetalFxLabel;
