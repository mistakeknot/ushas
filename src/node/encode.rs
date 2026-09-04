//! The MetalFX encode itself — one arm per mode.
//!
//! # The snatch-lock rule
//!
//! Every raw `MTLTexture` pointer is extracted *before* `as_hal_mut` is called
//! on the encoder. wgpu guards raw-handle access with an internal "snatch
//! lock", and taking `as_hal` on a texture while `as_hal_mut` is live on the
//! encoder (or the reverse) is a recursive lock — a panic, not an error. The
//! pointer extraction below therefore happens in its own scopes, and the
//! encode closures use only the `*mut c_void` values captured beforehand.

use std::ffi::c_void;

use bevy::prelude::*;
use bevy::render::camera::TemporalJitter;
// Ungated: every mode encodes MetalFX through a dedicated raw command encoder,
// not just frame interpolation. Kept as its own `use` so it does not inherit
// the `#[cfg]` on the import below.
use bevy::render::render_resource::CommandEncoderDescriptor;
#[cfg(feature = "frame-interpolation")]
use bevy::render::render_resource::{Extent3d, TextureDescriptor, TextureDimension, TextureUsages};
use bevy::render::renderer::{RenderContext, RenderDevice};

#[cfg(feature = "frame-interpolation")]
use super::MetalFxFrameTiming;
#[cfg(feature = "frame-interpolation")]
use super::PRESENT_FORMAT;
use super::{CachedState, MetalFxUpscaleNode, SendScaler};
use crate::gpu_timing::add_gpu_timing_handler;
use crate::platform::encode_spatial_upscale;
#[cfg(feature = "temporal")]
use crate::platform::encode_temporal_upscale;
use crate::GpuTimingDiag;

fn with_command_buffer<T>(buffer: Option<T>, encode: impl FnOnce(T)) -> bool {
    let Some(buffer) = buffer else { return false };
    encode(buffer);
    true
}

#[cfg(test)]
mod tests {
    use super::with_command_buffer;

    #[test]
    fn missing_command_buffer_does_not_report_encoding() {
        assert!(!with_command_buffer(None::<()>, |_| panic!(
            "no buffer to encode"
        )));
    }

    #[test]
    fn command_buffer_reports_encoding_only_after_the_callback_runs() {
        let mut encoded = false;
        assert!(with_command_buffer(Some(42), |buffer| {
            assert_eq!(buffer, 42);
            encoded = true;
        }));
        assert!(encoded);
    }
}

impl MetalFxUpscaleNode {
    /// Encode the MetalFX pass for whichever scaler is cached.
    ///
    /// Returns `false` if a raw Metal handle could not be obtained, in which
    /// case the frame is skipped rather than encoded half-way.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(not(feature = "frame-interpolation"), allow(unused_variables))]
    pub(super) fn encode_metalfx(
        &self,
        world: &World,
        device: &RenderDevice,
        render_context: &mut RenderContext,
        state: &mut CachedState,
        is_temporal_like: bool,
        temporal_jitter: Option<&TemporalJitter>,
        projection: Option<&Projection>,
        main_format: bevy::render::render_resource::TextureFormat,
        content_w: u32,
        content_h: u32,
        input_w: u32,
        input_h: u32,
        output_w: u32,
        output_h: u32,
    ) -> bool {
        // --- Phase B: MetalFX encode ---
        // CRITICAL: Extract ALL raw texture pointers in isolated scopes BEFORE
        // calling encoder.as_hal_mut(). wgpu uses a "snatch lock" internally;
        // calling as_hal() on textures while as_hal_mut() is active (or vice
        // versa) causes a recursive lock panic.

        let input_tex_ptr = {
            // SAFETY: the texture is owned by CachedState and alive for the whole
            // frame, and no `as_hal_mut` is live on the encoder at this point — the
            // guard drops at the end of this scope, before the encode below.
            let Some(hal) = (unsafe { state.input_texture.as_hal::<wgpu_hal::metal::Api>() })
            else {
                log::error!("MetalFxUpscaleNode: no Metal HAL for input texture");
                return false;
            };
            (hal.raw_handle() as *const _) as *mut c_void
        };

        let out_tex_ptr = {
            // SAFETY: the texture is owned by CachedState and alive for the whole
            // frame, and no `as_hal_mut` is live on the encoder at this point — the
            // guard drops at the end of this scope, before the encode below.
            let Some(hal) = (unsafe { state.output_texture.as_hal::<wgpu_hal::metal::Api>() })
            else {
                log::error!("MetalFxUpscaleNode: no Metal HAL for output texture");
                return false;
            };
            (hal.raw_handle() as *const _) as *mut c_void
        };

        // A reset is owed on the very first frame — there is no history yet — and
        // whenever a consumer declares a discontinuity the renderer cannot see
        // (camera cut, teleport, scene load). Both mean the same thing to
        // MetalFX, so they collapse into one flag here.
        //
        // The request clears itself in the main world at the top of the next
        // frame, so this reads true for exactly one rendered frame.
        let history_reset_requested = world
            .get_resource::<crate::MetalFxHistoryReset>()
            .is_some_and(crate::MetalFxHistoryReset::is_requested);
        let is_first_frame = state.frame_count == 0 || history_reset_requested;
        // Log the honoured request, not the requested one. The main world can
        // say `request()` and still have the flag never arrive — extraction
        // runs after the whole main schedule, so a mistimed clear would drop it
        // silently and the only visible symptom would be ghosting nobody
        // connects to the flag. This line is what distinguishes "the reset did
        // not help" from "the reset never reached the scaler".
        if history_reset_requested {
            log::info!(
                "MetalFxUpscaleNode: honouring MetalFxHistoryReset — dropping temporal history \
                 for frame {}",
                state.frame_count
            );
        }

        // Extract temporal texture pointers (content-sized depth + motion).
        let temporal_ptrs = if is_temporal_like {
            let content_depth = state.content_depth_texture.as_ref().unwrap();
            let content_motion = state.content_motion_texture.as_ref().unwrap();

            let depth_ptr = {
                // SAFETY: the texture is owned by CachedState and alive for the whole
                // frame, and no `as_hal_mut` is live on the encoder at this point — the
                // guard drops at the end of this scope, before the encode below.
                let Some(hal) = (unsafe { content_depth.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for content depth texture");
                    return false;
                };
                (hal.raw_handle() as *const _) as *mut c_void
            };

            let motion_ptr = {
                // SAFETY: the texture is owned by CachedState and alive for the whole
                // frame, and no `as_hal_mut` is live on the encoder at this point — the
                // guard drops at the end of this scope, before the encode below.
                let Some(hal) = (unsafe { content_motion.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for content motion texture");
                    return false;
                };
                (hal.raw_handle() as *const _) as *mut c_void
            };

            Some((depth_ptr, motion_ptr))
        } else {
            None
        };

        // For frame interpolation, extract prev color ptr (must be before as_hal_mut).
        // Frame interpolation needs two extra full-resolution textures: the
        // previous upscaled frame (history) and somewhere to put the synthesised
        // frame. Both sit at *output* size — the interpolator consumes upscaled
        // color, so `inputWidth/Height` on its descriptor describe only the
        // depth/motion textures. Sizing these to the input instead trips
        // MetalFX's "Color texture width mismatch from descriptor" assertion.
        // Annotated because the only arm producing `Some` is gated out of a
        // spatial-only build, leaving nothing to infer the payload from.
        let interp_ptrs: Option<(*mut c_void, *mut c_void)> = match &state.scaler {
            #[cfg(feature = "frame-interpolation")]
            SendScaler::FrameInterpolator { .. } => {
                if state.prev_color_texture.is_none() {
                    state.prev_color_texture = Some(device.create_texture(&TextureDescriptor {
                        label: Some("metalfx_prev_color"),
                        size: Extent3d {
                            width: output_w,
                            height: output_h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: main_format,
                        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                        view_formats: &[],
                    }));
                }
                if state.interp_output_texture.is_none() {
                    let tex = device.create_texture(&TextureDescriptor {
                        label: Some("metalfx_interp_output"),
                        size: Extent3d {
                            width: output_w,
                            height: output_h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: main_format,
                        usage: TextureUsages::TEXTURE_BINDING
                            | TextureUsages::STORAGE_BINDING
                            | TextureUsages::COPY_SRC,
                        view_formats: &[],
                    });
                    state.interp_output_view = Some(tex.create_view(
                        &bevy::render::render_resource::TextureViewDescriptor::default(),
                    ));
                    state.interp_output_texture = Some(tex);

                    let stage = |label: &'static str| {
                        let t = device.create_texture(&TextureDescriptor {
                            label: Some(label),
                            size: Extent3d {
                                width: output_w,
                                height: output_h,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: PRESENT_FORMAT,
                            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
                            view_formats: &[],
                        });
                        let v = t.create_view(
                            &bevy::render::render_resource::TextureViewDescriptor::default(),
                        );
                        (t, v)
                    };
                    let (t, v) = stage("metalfx_interp_bgra");
                    state.interp_bgra = Some(t);
                    state.interp_bgra_view = Some(v);
                    let (t, v) = stage("metalfx_real_bgra");
                    state.real_bgra = Some(t);
                    state.real_bgra_view = Some(v);
                }

                let prev_tex = state.prev_color_texture.as_ref().unwrap();
                // SAFETY: the texture is owned by CachedState and alive for the whole
                // frame, and no `as_hal_mut` is live on the encoder at this point — the
                // guard drops at the end of this scope, before the encode below.
                let Some(hal) = (unsafe { prev_tex.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for prev color texture");
                    return false;
                };
                let prev_ptr = (hal.raw_handle() as *const _) as *mut c_void;
                drop(hal);

                let interp_tex = state.interp_output_texture.as_ref().unwrap();
                // SAFETY: the texture is owned by CachedState and alive for the whole
                // frame, and no `as_hal_mut` is live on the encoder at this point — the
                // guard drops at the end of this scope, before the encode below.
                let Some(hal) = (unsafe { interp_tex.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for interp output texture");
                    return false;
                };
                let interp_ptr = (hal.raw_handle() as *const _) as *mut c_void;

                Some((prev_ptr, interp_ptr))
            }
            _ => None,
        };

        // GPU-timing sink (Phase 0 bound-ness bench): clone the Arc before
        // borrowing the encoder, so the completion handler can capture it.
        let timing_sink = world.get_resource::<GpuTimingDiag>().map(|d| d.0.clone());

        // Now safe to acquire as_hal_mut — all texture guards dropped.
        //
        // The raw encoding gets a command encoder of its own, because wgpu 29
        // refuses to let one encoder carry both kinds of work: the first wgpu
        // call marks an encoder `Wgpu`, the first `as_hal_mut` marks it `Raw`,
        // and whichever runs second panics with "Mixing the wgpu encoding API
        // with the raw encoding API is not permitted".
        //
        // This pass unavoidably does both. Phase B0 copies the content region
        // with `copy_texture_to_texture`, and the temporal path resolves depth
        // and motion through render passes — all on the context's encoder —
        // before MetalFX encodes against the raw `MTLCommandBuffer`. wgpu 27
        // tracked no such state and allowed the mixing, which is exactly why
        // this survived a compile-time audit of every `as_hal` call site and
        // only surfaced when the pass first ran on hardware.
        //
        // Ordering is preserved rather than assumed: `add_command_buffer` at
        // the end of this function flushes the context's open encoder and
        // queues it *ahead* of this one, so the copies and resolves still
        // execute before the MetalFX work that reads their output.
        let mut raw_encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("metalfx_raw_encode"),
        });
        let encoder = &mut raw_encoder;

        let encoded = match &state.scaler {
            SendScaler::Spatial(scaler) => {
                unsafe {
                    // SAFETY: every raw pointer above was taken in a scope that has already
                    // closed, so no texture guard is live. wgpu's snatch lock is not
                    // reentrant — overlapping the two is a panic, not a recoverable error.
                    encoder.as_hal_mut::<wgpu_hal::metal::Api, _, bool>(|hal_encoder| {
                        with_command_buffer(
                            hal_encoder.and_then(|enc| enc.raw_command_buffer()),
                            |cmd_buf| {
                                let cmd_buf_ptr = cmd_buf as *const _ as *mut c_void;

                                encode_spatial_upscale(
                                    scaler,
                                    input_tex_ptr,
                                    out_tex_ptr,
                                    cmd_buf_ptr,
                                    content_w as usize,
                                    content_h as usize,
                                );

                                if let Some(sink) = timing_sink.clone() {
                                    // Borrowed cmd buffer, registered pre-commit (Codex review A/B/D).
                                    add_gpu_timing_handler(cmd_buf_ptr, sink);
                                }
                            },
                        )
                    })
                }
            }
            #[cfg(feature = "frame-interpolation")]
            SendScaler::FrameInterpolator {
                scaler,
                interpolator,
            } => {
                let (depth_ptr, motion_ptr) = temporal_ptrs.unwrap();
                let (prev_color_ptr, interp_out_ptr) = interp_ptrs.unwrap();
                // Bevy's `TemporalJitter.offset` is a pixel offset in [-0.5, 0.5]
                // whose Y is flipped when it enters clip space (see
                // `TemporalJitter::jitter_projection`: `offset * vec2(2, -2)`).
                // MetalFX's `jitterOffsetX/Y` wants the pixel offset that returns
                // the sample to the reference frame — the same X, but Y negated
                // to match MetalFX's (un-flipped) pixel space.
                let jitter = temporal_jitter
                    .map(|j| Vec2::new(j.offset.x, -j.offset.y))
                    .unwrap_or(Vec2::ZERO);
                let motion_scale_x = -(input_w as f32);
                let motion_scale_y = -(input_h as f32);

                // Camera params, read from the render world rather than guessed.
                //
                // `MTLFXFrameInterpolator` uses these to unproject depth when it
                // synthesises the intermediate frame, so wrong values distort
                // the motion field rather than failing loudly.
                //
                // Unit trap: MetalFX documents `fieldOfView` as the *vertical*
                // FOV in DEGREES; Bevy's `PerspectiveProjection::fov` is
                // vertical FOV in RADIANS. Passing it through unconverted is a
                // silent ~57x error.
                let (field_of_view, aspect_ratio, near_plane, far_plane) = match projection {
                    Some(Projection::Perspective(p)) => {
                        (p.fov.to_degrees(), p.aspect_ratio, p.near, p.far)
                    }
                    // Orthographic/custom projections have no meaningful FOV.
                    // MetalFX frame interpolation assumes a perspective frustum,
                    // so fall back to a neutral one and let the caller know.
                    _ => {
                        // Once, not per-frame — this runs in the render loop.
                        static WARNED: std::sync::Once = std::sync::Once::new();
                        WARNED.call_once(|| {
                            log::warn!(
                                "MetalFxUpscaleNode: frame interpolation expects a perspective \
                         Projection on the view; falling back to 45deg/0.1/1000"
                            );
                        });
                        (
                            45.0_f32,
                            output_w as f32 / output_h as f32,
                            0.1_f32,
                            1000.0_f32,
                        )
                    }
                };

                // Real inter-frame interval, mirrored from the main world's
                // `Time` (the render world has none). Defaults to 60Hz if the
                // resource is missing, which only happens when the plugin was
                // built in a non-interpolation mode.
                let delta_time = world
                    .get_resource::<MetalFxFrameTiming>()
                    .map_or(1.0 / 60.0, |t| t.delta_seconds);

                unsafe {
                    // SAFETY: every raw pointer above was taken in a scope that has already
                    // closed, so no texture guard is live. wgpu's snatch lock is not
                    // reentrant — overlapping the two is a panic, not a recoverable error.
                    encoder.as_hal_mut::<wgpu_hal::metal::Api, _, bool>(|hal_encoder| {
                        with_command_buffer(
                            hal_encoder.and_then(|enc| enc.raw_command_buffer()),
                            |cmd_buf| {
                                let cmd_buf_ptr = cmd_buf as *const _ as *mut c_void;

                                // Stage 1 — upscale the low-res render to output size.
                                // This is the *real* frame, and it is what gets
                                // presented; it also becomes next frame's history.
                                encode_temporal_upscale(
                                    scaler,
                                    input_tex_ptr,
                                    depth_ptr,
                                    motion_ptr,
                                    out_tex_ptr,
                                    cmd_buf_ptr,
                                    content_w as usize,
                                    content_h as usize,
                                    jitter.x,
                                    jitter.y,
                                    motion_scale_x,
                                    motion_scale_y,
                                    is_first_frame,
                                );

                                // Stage 2 — synthesise the intermediate frame between
                                // the previous upscaled frame and this one. Both color
                                // inputs are full-res; depth/motion stay content-sized.
                                crate::platform::encode_frame_interpolation(
                                    interpolator,
                                    out_tex_ptr,
                                    prev_color_ptr,
                                    depth_ptr,
                                    motion_ptr,
                                    interp_out_ptr,
                                    cmd_buf_ptr,
                                    jitter.x,
                                    jitter.y,
                                    motion_scale_x,
                                    motion_scale_y,
                                    delta_time,
                                    field_of_view,
                                    aspect_ratio,
                                    near_plane,
                                    far_plane,
                                    is_first_frame,
                                );

                                if let Some(sink) = timing_sink.clone() {
                                    add_gpu_timing_handler(cmd_buf_ptr, sink);
                                }
                            },
                        )
                    })
                }

                // Snapshot this frame's upscaled color into the history buffer.
                //
                // MetalFX's contract: whenever `shouldResetHistory` is false,
                // `prevColorTexture` must contain the data that was in
                // `colorTexture` during the *previous* `encodeToCommandBuffer:`.
                // Without this copy the history buffer stayed uninitialised for
                // the life of the process, so every frame after the first was
                // interpolated against garbage (6zit.8).
                //
                // Ordering still holds, but it is now carried by command-buffer
                // submission order rather than encode order within one buffer:
                // this copy is a wgpu operation and cannot share the raw encoder
                // above, so it is issued after `add_command_buffer` below, and
                // Metal executes command buffers on a queue in commit order.
                // See the snapshot block at the end of this function.
            }
            #[cfg(feature = "temporal")]
            SendScaler::Temporal(scaler) => {
                let (depth_ptr, motion_ptr) = temporal_ptrs.unwrap();
                // Negate Y to convert Bevy's clip-space jitter convention to
                // MetalFX's pixel-space one — see the FrameInterpolator branch.
                let jitter = temporal_jitter
                    .map(|j| Vec2::new(j.offset.x, -j.offset.y))
                    .unwrap_or(Vec2::ZERO);

                let motion_scale_x = -(input_w as f32);
                let motion_scale_y = -(input_h as f32);

                unsafe {
                    // SAFETY: every raw pointer above was taken in a scope that has already
                    // closed, so no texture guard is live. wgpu's snatch lock is not
                    // reentrant — overlapping the two is a panic, not a recoverable error.
                    encoder.as_hal_mut::<wgpu_hal::metal::Api, _, bool>(|hal_encoder| {
                        with_command_buffer(
                            hal_encoder.and_then(|enc| enc.raw_command_buffer()),
                            |cmd_buf| {
                                let cmd_buf_ptr = cmd_buf as *const _ as *mut c_void;

                                encode_temporal_upscale(
                                    scaler,
                                    input_tex_ptr,
                                    depth_ptr,
                                    motion_ptr,
                                    out_tex_ptr,
                                    cmd_buf_ptr,
                                    content_w as usize,
                                    content_h as usize,
                                    jitter.x,
                                    jitter.y,
                                    motion_scale_x,
                                    motion_scale_y,
                                    is_first_frame,
                                );

                                if let Some(sink) = timing_sink.clone() {
                                    add_gpu_timing_handler(cmd_buf_ptr, sink);
                                }
                            },
                        )
                    })
                }
            }
        };
        if !encoded {
            return false;
        }

        // Queue the raw-encoded buffer. This flushes the context's still-open
        // wgpu encoder first, so the Phase B0 copy and the depth/motion
        // resolves are submitted ahead of the MetalFX work that consumes them,
        // and the Phase C blit — which opens a fresh encoder after this — is
        // submitted after it.
        render_context.add_command_buffer(raw_encoder.finish());
        state.frame_count = state.frame_count.saturating_add(1);

        // Snapshot this frame's upscaled color into the history buffer.
        //
        // MetalFX's contract: whenever `shouldResetHistory` is false,
        // `prevColorTexture` must contain the data that was in `colorTexture`
        // during the *previous* `encodeToCommandBuffer:`. Without this copy the
        // history buffer stays uninitialised for the life of the process and
        // every frame after the first is interpolated against garbage (6zit.8).
        //
        // It must land *after* the interpolation pass, which is why it sits
        // here rather than beside it: this is a wgpu copy and the interpolation
        // is raw Metal, and wgpu 29 will not let one encoder carry both. The
        // copy therefore goes on a fresh context encoder, queued after the raw
        // buffer, and Metal runs a queue's command buffers in commit order.
        #[cfg(feature = "frame-interpolation")]
        if matches!(state.scaler, SendScaler::FrameInterpolator { .. }) {
            let prev_tex = state.prev_color_texture.as_ref().unwrap();
            render_context.command_encoder().copy_texture_to_texture(
                state.output_texture.as_image_copy(),
                prev_tex.as_image_copy(),
                Extent3d {
                    width: output_w,
                    height: output_h,
                    depth_or_array_layers: 1,
                },
            );
        }

        true
    }
}
