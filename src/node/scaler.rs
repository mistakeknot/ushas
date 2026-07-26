//! Scaler lifecycle: creation, background creation, and the textures whose
//! size is tied to the scaler's.
//!
//! Split out of `run` because it is the one phase that can decline to produce
//! anything — a temporal scaler takes seconds to compile, so it is built on a
//! background thread and polled here, and until it arrives the node falls
//! through to Bevy's own upscaling. Every path that cannot proceed returns
//! `false` and the frame is skipped.

use std::ffi::c_void;

use bevy::render::render_resource::{
    Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::render::renderer::RenderDevice;
use foreign_types::ForeignType;
use objc2_metal::MTLPixelFormat;

#[cfg(feature = "temporal")]
use super::PendingScaler;
use super::{CachedState, MetalFxUpscaleNode, SendScaler};
use crate::platform::try_create_spatial_scaler_from_raw;
use crate::MetalFxMode;

/// The six dimensions Phase A juggles, bundled so the signature stays readable.
///
/// `scaler_*` are what the scaler is *created* at and `input_*` what this frame
/// actually rendered at. Under dynamic resolution they differ: the scaler is
/// built at output size and the live content region flexes inside it.
#[derive(Clone, Copy)]
pub(super) struct ScalerDims {
    pub scaler_input_w: u32,
    pub scaler_input_h: u32,
    pub input_w: u32,
    pub input_h: u32,
    pub output_w: u32,
    pub output_h: u32,
}

impl MetalFxUpscaleNode {
    /// Ensure `cached` holds a scaler usable at `dims`.
    ///
    /// Returns `false` when this frame has nothing to do — creation is still in
    /// flight, creation failed, or the mode is not built into this binary.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ensure_scaler(
        &self,
        device: &RenderDevice,
        cached: &mut Option<CachedState>,
        dims: ScalerDims,
        mode: MetalFxMode,
        main_format: TextureFormat,
        color_mtl_fmt: MTLPixelFormat,
        dynamic_res_range: Option<(f32, f32)>,
    ) -> bool {
        let ScalerDims {
            scaler_input_w,
            scaler_input_h,
            input_w,
            input_h,
            output_w,
            output_h,
        } = dims;

        // Recreate only when the *scaler* input dimensions change (i.e. a window
        // resize, or the dynamic-res max). Per-frame render-scale changes move
        // `input_w/h` but not `scaler_input_w/h`, so under dynamic resolution they
        // no longer force a rebuild — the scaler flexes via setInputContentWidth.
        let needs_recreate = cached.as_ref().is_none_or(|c| {
            c.input_w != scaler_input_w
                || c.input_h != scaler_input_h
                || c.output_w != output_w
                || c.output_h != output_h
        });

        if needs_recreate {
            // Check if a background scaler creation is pending.
            let mut pending = self.pending.lock().unwrap();
            if let Some(p) = pending.as_ref() {
                // Check if dimensions match what we need (scaler-creation dims).
                if p.input_w == scaler_input_w
                    && p.input_h == scaler_input_h
                    && p.output_w == output_w
                    && p.output_h == output_h
                {
                    // Try to receive the scaler (non-blocking).
                    match p.receiver.try_recv() {
                        Ok(Some(scaler)) => {
                            log::info!(
                        "MetalFxUpscaleNode: background scaler ready {input_w}x{input_h} -> {output_w}x{output_h}"
                    );
                            *pending = None;

                            // Sized to the scaler's max input so the same textures
                            // serve every frame under dynamic resolution (the
                            // per-frame content copy fills only the top-left region).
                            let input_texture = device.create_texture(&TextureDescriptor {
                                label: Some("metalfx_input"),
                                size: Extent3d {
                                    width: scaler_input_w,
                                    height: scaler_input_h,
                                    depth_or_array_layers: 1,
                                },
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: TextureDimension::D2,
                                format: main_format,
                                usage: TextureUsages::COPY_DST | TextureUsages::TEXTURE_BINDING,
                                view_formats: &[],
                            });

                            // Content-sized depth texture for temporal mode.
                            // Depth32Float — matches scaler's setDepthTextureFormat.
                            // Written via depth resolve render pass (@builtin(frag_depth)).
                            let (content_depth_texture, content_depth_view) =
                                if scaler.is_temporal_like() {
                                    let tex = device.create_texture(&TextureDescriptor {
                            label: Some("metalfx_content_depth"),
                            size: Extent3d {
                                width: scaler_input_w,
                                height: scaler_input_h,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: bevy::render::render_resource::TextureFormat::Depth32Float,
                            usage: TextureUsages::RENDER_ATTACHMENT
                                | TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        });
                                    let view = tex.create_view(
                            &bevy::render::render_resource::TextureViewDescriptor::default(),
                        );
                                    (Some(tex), Some(view))
                                } else {
                                    (None, None)
                                };

                            // Content-sized motion vector texture for temporal mode.
                            // Written via motion resolve render pass (Bevy's prepass
                            // textures lack COPY_SRC, so copy_texture_to_texture fails).
                            let (content_motion_texture, content_motion_view) =
                                if scaler.is_temporal_like() {
                                    let tex = device.create_texture(&TextureDescriptor {
                                        label: Some("metalfx_content_motion"),
                                        size: Extent3d {
                                            width: scaler_input_w,
                                            height: scaler_input_h,
                                            depth_or_array_layers: 1,
                                        },
                                        mip_level_count: 1,
                                        sample_count: 1,
                                        dimension: TextureDimension::D2,
                                        format:
                                            bevy::render::render_resource::TextureFormat::Rg16Float,
                                        usage: TextureUsages::RENDER_ATTACHMENT
                                            | TextureUsages::TEXTURE_BINDING,
                                        view_formats: &[],
                                    });
                                    let view = tex.create_view(
                            &bevy::render::render_resource::TextureViewDescriptor::default(),
                        );
                                    (Some(tex), Some(view))
                                } else {
                                    (None, None)
                                };

                            let output_texture = device.create_texture(&TextureDescriptor {
                                label: Some("metalfx_output"),
                                size: Extent3d {
                                    width: output_w,
                                    height: output_h,
                                    depth_or_array_layers: 1,
                                },
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: TextureDimension::D2,
                                format: main_format,
                                // COPY_SRC: frame interpolation snapshots the upscaled
                                // frame into `metalfx_prev_color` as history.
                                usage: TextureUsages::RENDER_ATTACHMENT
                                    | TextureUsages::TEXTURE_BINDING
                                    | TextureUsages::STORAGE_BINDING
                                    | TextureUsages::COPY_SRC,
                                view_formats: &[],
                            });
                            let output_view = output_texture.create_view(
                                &bevy::render::render_resource::TextureViewDescriptor::default(),
                            );

                            *self.cached_bind_group.lock().unwrap() = None;
                            *self.cached_pipeline.lock().unwrap() = None;
                            *self.depth_resolve_bind_group.lock().unwrap() = None;
                            *self.motion_resolve_bind_group.lock().unwrap() = None;

                            *cached = Some(CachedState {
                                scaler,
                                input_texture,
                                output_texture,
                                output_view,
                                #[cfg(feature = "frame-interpolation")]
                                prev_color_texture: None,
                                #[cfg(feature = "frame-interpolation")]
                                interp_output_texture: None,
                                #[cfg(feature = "frame-interpolation")]
                                interp_output_view: None,
                                #[cfg(feature = "frame-interpolation")]
                                interp_bgra: None,
                                #[cfg(feature = "frame-interpolation")]
                                interp_bgra_view: None,
                                #[cfg(feature = "frame-interpolation")]
                                real_bgra: None,
                                #[cfg(feature = "frame-interpolation")]
                                real_bgra_view: None,
                                content_depth_texture,
                                content_depth_view,
                                content_motion_texture,
                                content_motion_view,
                                input_w: scaler_input_w,
                                input_h: scaler_input_h,
                                output_w,
                                output_h,
                                frame_count: 0,
                            });
                        }
                        Ok(None) => {
                            log::warn!("MetalFxUpscaleNode: background scaler creation failed");
                            *pending = None;
                            return false;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            // Still creating — skip this frame.
                            return false;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            log::error!("MetalFxUpscaleNode: background thread panicked");
                            *pending = None;
                            return false;
                        }
                    }
                } else {
                    // Dimensions changed: discard the pending creation AND the
                    // stale cached scaler so the block below starts a fresh one.
                    *pending = None;
                    *cached = None;
                }
            }

            // Start a new creation only when we have neither a usable scaler nor
            // one in flight. Guarding on `cached.is_none()` is essential: the
            // receive branch above sets `*cached = Some(..)` and `*pending = None`
            // in the same frame, so a `pending.is_none()`-only guard would fall
            // straight through here and immediately discard the scaler we just
            // received — rebuilding forever (6zit.12). The dimensions-changed
            // path nulls `cached` above so a genuine resize still recreates.
            if cached.is_none() && pending.is_none() {
                log::info!(
            "MetalFxUpscaleNode: creating {:?} scaler {scaler_input_w}x{scaler_input_h} -> {output_w}x{output_h} (dynamic_res={dynamic_res_range:?}, cur_input={input_w}x{input_h})",
            mode
        );

                let wgpu_dev = device.wgpu_device();
                let Some(hal_dev) = (unsafe { wgpu_dev.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL device");
                    return false;
                };
                let device_ptr = {
                    let dev_lock = hal_dev.raw_device().lock();
                    dev_lock.as_ptr() as *mut c_void
                };

                match mode {
                    MetalFxMode::Spatial => {
                        // Spatial is fast — create synchronously.
                        let scaler = unsafe {
                            try_create_spatial_scaler_from_raw(
                                device_ptr,
                                input_w as usize,
                                input_h as usize,
                                output_w as usize,
                                output_h as usize,
                                color_mtl_fmt,
                                color_mtl_fmt,
                            )
                        };
                        let Some(scaler) = scaler else {
                            log::error!("MetalFxUpscaleNode: failed to create spatial scaler");
                            return false;
                        };

                        // Content-sized input texture for MetalFX (GPU-copied from main_texture).
                        let input_texture = device.create_texture(&TextureDescriptor {
                            label: Some("metalfx_input"),
                            size: Extent3d {
                                width: input_w,
                                height: input_h,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: main_format,
                            usage: TextureUsages::COPY_DST
                                | TextureUsages::TEXTURE_BINDING
                                | TextureUsages::STORAGE_BINDING,
                            view_formats: &[],
                        });

                        let output_texture = device.create_texture(&TextureDescriptor {
                            label: Some("metalfx_output"),
                            size: Extent3d {
                                width: output_w,
                                height: output_h,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: main_format,
                            // COPY_SRC: see the sibling `metalfx_output` above.
                            usage: TextureUsages::RENDER_ATTACHMENT
                                | TextureUsages::TEXTURE_BINDING
                                | TextureUsages::STORAGE_BINDING
                                | TextureUsages::COPY_SRC,
                            view_formats: &[],
                        });
                        let output_view = output_texture.create_view(
                            &bevy::render::render_resource::TextureViewDescriptor::default(),
                        );

                        *self.cached_bind_group.lock().unwrap() = None;
                        *self.cached_pipeline.lock().unwrap() = None;

                        *cached = Some(CachedState {
                            scaler: SendScaler::Spatial(scaler),
                            input_texture,
                            output_texture,
                            output_view,
                            #[cfg(feature = "frame-interpolation")]
                            prev_color_texture: None,
                            #[cfg(feature = "frame-interpolation")]
                            interp_output_texture: None,
                            #[cfg(feature = "frame-interpolation")]
                            interp_output_view: None,
                            #[cfg(feature = "frame-interpolation")]
                            interp_bgra: None,
                            #[cfg(feature = "frame-interpolation")]
                            interp_bgra_view: None,
                            #[cfg(feature = "frame-interpolation")]
                            real_bgra: None,
                            #[cfg(feature = "frame-interpolation")]
                            real_bgra_view: None,
                            content_depth_texture: None,
                            content_depth_view: None,
                            content_motion_texture: None,
                            content_motion_view: None,
                            input_w,
                            input_h,
                            output_w,
                            output_h,
                            frame_count: 0,
                        });
                    }
                    #[cfg(feature = "temporal")]
                    MetalFxMode::Temporal | MetalFxMode::FrameInterpolation => {
                        // Temporal + FrameInterpolation are slow — create on background thread.
                        let (tx, rx) = std::sync::mpsc::channel();

                        // MTLPixelFormat is a #[repr(transparent)] newtype over
                        // NSUInteger (= usize on 64-bit macOS); read its
                        // discriminant via the field rather than transmuting.
                        let color_fmt_raw: usize = color_mtl_fmt.0;
                        match mode {
                            MetalFxMode::Temporal => unsafe {
                                // Create temporal scaler at the max input dimensions
                                // (== current dims when dynamic res is off). Depth and
                                // motion vectors are resolved to content-sized textures
                                // before being passed to MetalFX. `dynamic_res_range`, if
                                // set, enables true dynamic resolution so scale changes
                                // flex without rebuilding this scaler.
                                crate::platform::spawn_temporal_scaler_thread(
                                    device_ptr,
                                    scaler_input_w as usize,
                                    scaler_input_h as usize,
                                    output_w as usize,
                                    output_h as usize,
                                    color_fmt_raw,
                                    dynamic_res_range,
                                    tx,
                                );
                            },
                            #[cfg(feature = "frame-interpolation")]
                            MetalFxMode::FrameInterpolation => unsafe {
                                crate::platform::spawn_frame_interpolator_thread(
                                    device_ptr,
                                    scaler_input_w as usize,
                                    scaler_input_h as usize,
                                    output_w as usize,
                                    output_h as usize,
                                    color_fmt_raw,
                                    tx,
                                );
                            },
                            _ => unreachable!(),
                        }

                        *pending = Some(PendingScaler {
                            receiver: rx,
                            input_w: scaler_input_w,
                            input_h: scaler_input_h,
                            output_w,
                            output_h,
                        });

                        return false;
                    }
                    _ => {
                        log::warn!("MetalFxUpscaleNode: unsupported mode {:?}", mode);
                        return false;
                    }
                }
            }

            // Still no cached scaler after all attempts — skip this frame.
            if cached.is_none() {
                return false;
            }
        }

        true
    }
}
