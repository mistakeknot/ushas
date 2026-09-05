//! Temporal reset requests persist until their encoded frame acknowledges them.

use bevy::prelude::Resource;
use bevy::render::extract_resource::ExtractResource;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
struct ResetState {
    requested: AtomicU64,
    acknowledged: AtomicU64,
}

/// Ask MetalFX to discard temporal history on the next successful temporal encode.
///
/// Call [`Self::request`] through the main world's `ResMut<MetalFxHistoryReset>`
/// after a camera cut, teleport, or scene load. Requests survive inactive views,
/// missing prepasses, and scaler preparation. Encoding a reset acknowledges
/// only the request generation that frame observed; it cannot clear a later cut.
/// This acknowledgement establishes command encoding, not GPU completion.
///
/// The first frame of a new scaler resets automatically. Spatial mode does not
/// consume temporal requests. Repeated requests before a frame can be coalesced
/// into one reset; requesting every frame suppresses temporal accumulation.
#[derive(Resource, Clone, Debug, Default)]
pub struct MetalFxHistoryReset {
    state: Arc<ResetState>,
    /// Main-world clones see current requests; extraction freezes frame identity.
    extracted_generation: Option<u64>,
}

impl MetalFxHistoryReset {
    /// Keep a reset pending until a temporal encode acknowledges this request.
    pub fn request(&mut self) {
        self.state
            .requested
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .expect("MetalFX history reset generation exhausted");
    }

    /// Whether this resource's current or extracted request is unacknowledged.
    pub fn is_requested(&self) -> bool {
        self.generation() > self.state.acknowledged.load(Ordering::Acquire)
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn pending_request(&self) -> Option<HistoryResetRequest> {
        let generation = self.generation();
        (generation > self.state.acknowledged.load(Ordering::Acquire)).then(|| {
            HistoryResetRequest {
                state: self.state.clone(),
                generation,
            }
        })
    }

    fn generation(&self) -> u64 {
        self.extracted_generation
            .unwrap_or_else(|| self.state.requested.load(Ordering::Acquire))
    }
}

impl ExtractResource for MetalFxHistoryReset {
    type Source = Self;

    fn extract_resource(source: &Self) -> Self {
        Self {
            state: source.state.clone(),
            extracted_generation: Some(source.generation()),
        }
    }
}

/// A captured request. Dropping it without encoding leaves the request pending.
#[cfg(any(target_os = "macos", test))]
pub(crate) struct HistoryResetRequest {
    state: Arc<ResetState>,
    generation: u64,
}

#[cfg(any(target_os = "macos", test))]
impl HistoryResetRequest {
    /// Call only after an actual temporal/interpolation reset encode succeeded.
    pub(crate) fn acknowledge(self) {
        self.state
            .acknowledged
            .fetch_max(self.generation, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_requested_reset() {
        let main = MetalFxHistoryReset::default();
        assert!(!main.is_requested());
        assert!(main.pending_request().is_none());
    }

    #[test]
    fn inactive_or_unready_frames_cannot_consume_the_request() {
        let mut main = MetalFxHistoryReset::default();
        main.request();
        for _ in 0..8 {
            let render = MetalFxHistoryReset::extract_resource(&main);
            assert!(render.is_requested());
            // No eligible view, missing prepass, or pending scaler: no encode acknowledgement.
            assert!(render.pending_request().is_some());
        }
        assert!(main.is_requested());
    }

    #[test]
    fn successful_encode_consumes_once_across_main_and_render_worlds() {
        let mut main = MetalFxHistoryReset::default();
        let observer = main.clone();
        main.request();
        let render = MetalFxHistoryReset::extract_resource(&main);
        render
            .pending_request()
            .expect("requested reset")
            .acknowledge();
        assert!(!main.is_requested());
        assert!(!observer.is_requested());
        assert!(!render.is_requested());
        assert!(MetalFxHistoryReset::extract_resource(&main)
            .pending_request()
            .is_none());
    }

    #[test]
    fn extraction_does_not_see_a_request_from_a_later_main_frame() {
        let mut main = MetalFxHistoryReset::default();
        let render = MetalFxHistoryReset::extract_resource(&main);
        main.request();
        assert!(main.is_requested());
        assert!(!render.is_requested());
        assert!(MetalFxHistoryReset::extract_resource(&main).is_requested());
    }

    #[test]
    fn old_encode_acknowledgement_cannot_consume_a_newer_request() {
        let mut main = MetalFxHistoryReset::default();
        main.request();
        let old_render = MetalFxHistoryReset::extract_resource(&main);
        let old_token = old_render.pending_request().expect("first request");
        main.request();
        old_token.acknowledge();
        assert!(!old_render.is_requested());
        assert!(
            main.is_requested(),
            "the newer cut must still reach its own frame"
        );
        MetalFxHistoryReset::extract_resource(&main)
            .pending_request()
            .expect("second request")
            .acknowledge();
        assert!(!main.is_requested());
    }

    #[test]
    fn dropped_encode_token_leaves_the_request_pending() {
        let mut main = MetalFxHistoryReset::default();
        main.request();
        let render = MetalFxHistoryReset::extract_resource(&main);
        {
            let _failed_encode = render.pending_request().expect("requested reset");
        }
        assert!(main.is_requested());
        MetalFxHistoryReset::extract_resource(&main)
            .pending_request()
            .expect("retry still owes reset")
            .acknowledge();
        assert!(!main.is_requested());
    }

    #[test]
    fn one_encoded_reset_can_coalesce_requests_known_before_extraction() {
        let mut main = MetalFxHistoryReset::default();
        main.request();
        main.request();
        MetalFxHistoryReset::extract_resource(&main)
            .pending_request()
            .expect("coalesced request")
            .acknowledge();
        assert!(!main.is_requested());
    }

    #[test]
    fn acknowledgements_arriving_out_of_order_do_not_reopen_consumed_requests() {
        let mut main = MetalFxHistoryReset::default();
        main.request();
        let first = MetalFxHistoryReset::extract_resource(&main)
            .pending_request()
            .expect("first request");
        main.request();
        let second = MetalFxHistoryReset::extract_resource(&main)
            .pending_request()
            .expect("second request");
        second.acknowledge();
        first.acknowledge();
        assert!(!main.is_requested());
        main.request();
        assert!(main.is_requested());
    }
}
