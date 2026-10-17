//! Count Bevy's pipeline-cache errors so a shader that fails to compile fails
//! the run.
//!
//! The image checks cannot catch this on their own: a material that never
//! compiles draws nothing, and the rest of the scene still satisfies them.
//! That is how the load plane's `#import` shader passed every run on Bevy
//! 0.20 while rendering nothing. Bevy reports the failure exactly once, as an
//! error event from its pipeline cache, so the fixture listens for those.

use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::app::App;
use bevy::log::tracing::{Event, Level, Subscriber};
use bevy::log::tracing_subscriber::layer::Context;
use bevy::log::tracing_subscriber::Layer;
use bevy::log::BoxedLayer;

/// Errors Bevy's pipeline cache has logged since startup.
static ERRORS: AtomicUsize = AtomicUsize::new(0);

/// The log target of `bevy_render`'s pipeline cache, which reports shader
/// module and pipeline creation failures.
const TARGET: &str = "bevy_render::render_resource::pipeline_cache";

/// How many pipeline-cache errors Bevy has logged so far.
pub fn count() -> usize {
    ERRORS.load(Ordering::Relaxed)
}

struct PipelineErrorLayer;

impl<S: Subscriber> Layer<S> for PipelineErrorLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() == Level::ERROR && meta.target().starts_with(TARGET) {
            ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The `LogPlugin::custom_layer` hook that installs the counter.
pub fn layer(_app: &mut App) -> Option<BoxedLayer> {
    Some(Box::new(PipelineErrorLayer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::log::tracing::subscriber::with_default;
    use bevy::log::tracing_subscriber::layer::SubscriberExt;
    use bevy::log::tracing_subscriber::registry;

    #[test]
    fn counts_only_pipeline_cache_errors() {
        let before = count();
        let subscriber = registry().with(PipelineErrorLayer);
        with_default(subscriber, || {
            bevy::log::error!(target: "bevy_render::render_resource::pipeline_cache", "failed to create shader module");
            bevy::log::warn!(target: "bevy_render::render_resource::pipeline_cache", "a warning is not a failure");
            bevy::log::error!(target: "bevy_winit::state", "an error elsewhere is not a pipeline failure");
        });
        assert_eq!(count() - before, 1);
    }
}
