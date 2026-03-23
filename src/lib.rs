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

#[cfg(target_os = "macos")]
mod jitter;

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

/// Main-world resource holding the render scale for resolution override systems.
#[derive(bevy::prelude::Resource, Clone, Copy)]
pub struct MetalFxRenderScale(pub f32);

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

        // Main-world: insert render scale resource and resolution override systems.
        app.insert_resource(MetalFxRenderScale(self.render_scale));
        app.insert_resource(MetalFxModeResource(self.mode));
        app.add_systems(
            bevy::app::PostStartup,
            apply_resolution_override,
        );
        app.add_systems(bevy::app::Update, update_resolution_on_resize);

        // Temporal + FrameInterpolation modes: add prepass components and jitter system.
        if self.mode == MetalFxMode::Temporal || self.mode == MetalFxMode::FrameInterpolation {
            app.add_systems(bevy::app::PostStartup, setup_temporal_camera);
            app.add_systems(bevy::app::Update, jitter::update_jitter);
        }

        #[cfg(target_os = "macos")]
        {
            use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
            use bevy::render::render_graph::{RenderGraphExt, ViewNodeRunner};
            use bevy::render::RenderApp;

            if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
                render_app
                    .insert_resource(node::MetalFxConfig {
                        render_scale: self.render_scale,
                        mode: self.mode,
                    })
                    .add_render_graph_node::<ViewNodeRunner<MetalFxUpscaleNode>>(
                        Core3d,
                        MetalFxLabel,
                    )
                    // Run MetalFX after Bevy's UpscalingNode — we overwrite
                    // out_texture with the ML-upscaled result via Metal blit.
                    .add_render_graph_edges(
                        Core3d,
                        (Node3d::Upscaling, MetalFxLabel),
                    );
            }
        }
    }
}

use bevy::camera::MainPassResolutionOverride;
use bevy::core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass};
use bevy::render::camera::TemporalJitter;
use bevy::prelude::*;

/// Main-world resource holding the MetalFX mode.
#[derive(Resource, Clone, Copy)]
pub struct MetalFxModeResource(pub MetalFxMode);

/// Insert `MainPassResolutionOverride` on all Camera3d entities at startup.
fn apply_resolution_override(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<MainPassResolutionOverride>)>,
    windows: Query<&Window>,
    scale: Res<MetalFxRenderScale>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let w = window.physical_width();
    let h = window.physical_height();
    if w == 0 || h == 0 {
        return;
    }
    let override_w = (w as f32 * scale.0).round() as u32;
    let override_h = (h as f32 * scale.0).round() as u32;

    for entity in cameras.iter() {
        log::info!(
            "MetalFX: setting MainPassResolutionOverride {override_w}x{override_h} \
             (window {w}x{h}, scale {})",
            scale.0
        );
        commands
            .entity(entity)
            .insert(MainPassResolutionOverride(UVec2::new(override_w, override_h)));
    }
}

/// Update resolution override when the window size changes.
fn update_resolution_on_resize(
    mut cameras: Query<&mut MainPassResolutionOverride, With<Camera3d>>,
    windows: Query<&Window, Changed<Window>>,
    scale: Res<MetalFxRenderScale>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let w = window.physical_width();
    let h = window.physical_height();
    if w == 0 || h == 0 {
        return;
    }
    let override_w = (w as f32 * scale.0).round() as u32;
    let override_h = (h as f32 * scale.0).round() as u32;

    for mut res_override in cameras.iter_mut() {
        log::info!(
            "MetalFX: resize -> MainPassResolutionOverride {override_w}x{override_h}"
        );
        res_override.0 = UVec2::new(override_w, override_h);
    }
}

/// Insert prepass components and jitter on Camera3d for temporal mode.
fn setup_temporal_camera(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<MotionVectorPrepass>)>,
) {
    for entity in cameras.iter() {
        log::info!("MetalFX temporal: adding MotionVectorPrepass + DepthPrepass + TemporalJitter");
        commands.entity(entity).insert((
            MotionVectorPrepass,
            DepthPrepass,
            TemporalJitter::default(),
        ));
    }
}

/// Render graph label for the MetalFX upscale node.
#[derive(Debug, Hash, PartialEq, Eq, Clone, bevy::render::render_graph::RenderLabel)]
pub struct MetalFxLabel;
