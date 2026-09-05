//! Native window and externally driven OS lifecycle observations, never power requests.

// BEGIN PURE CONTRACT
const MAX_EVENTS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    MinimizeRequested {
        window: u64,
        minimized: bool,
    },
    WindowOccluded {
        window: u64,
        occluded: bool,
    },
    MinimizedObserved {
        window: u64,
        minimized: Option<bool>,
    },
    WillSleep,
    DidWake,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub sequence: u64,
    pub unix_ms: u64,
    pub process_elapsed_ms: u64,
    pub frame: Option<u64>,
    pub kind: EventKind,
}

#[derive(Debug, Default)]
pub struct EventLedger {
    events: Vec<Event>,
    dropped: usize,
}

impl EventLedger {
    pub fn cursor(&self) -> u64 {
        self.events.last().map_or(0, |event| event.sequence)
    }

    pub fn record(
        &mut self,
        kind: EventKind,
        unix_ms: u64,
        process_elapsed_ms: u64,
        frame: Option<u64>,
    ) {
        if self.events.len() == MAX_EVENTS {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.events.push(Event {
            sequence: self.cursor() + 1,
            unix_ms,
            process_elapsed_ms,
            frame,
            kind,
        });
    }

    pub fn window_state_since(&self, cursor: u64, window: u64, minimized: bool) -> bool {
        if self.dropped != 0 {
            return false;
        }
        let mut occluded = None;
        let mut native_minimized = None;
        for event in self.events.iter().filter(|event| event.sequence > cursor) {
            match event.kind {
                EventKind::WindowOccluded {
                    window: id,
                    occluded: value,
                } if id == window => {
                    occluded = Some(value);
                }
                EventKind::MinimizedObserved {
                    window: id,
                    minimized: value,
                } if id == window => {
                    native_minimized = value;
                }
                _ => {}
            }
        }
        occluded == Some(minimized) && native_minimized == Some(minimized)
    }

    pub fn sleep_cycle_since(&self, cursor: u64) -> Option<(u64, u64)> {
        if self.dropped != 0 {
            return None;
        }
        let mut sleep = None;
        let mut cycle = None;
        for event in self.events.iter().filter(|event| event.sequence > cursor) {
            match event.kind {
                EventKind::WillSleep => {
                    sleep = Some(event.sequence);
                    cycle = None;
                }
                EventKind::DidWake => {
                    if let Some(start) = sleep.take() {
                        cycle = Some((start, event.sequence));
                    }
                }
                _ => {}
            }
        }
        cycle
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    fn push(ledger: &mut EventLedger, kind: EventKind) {
        let sequence = ledger.cursor() + 1;
        ledger.record(kind, sequence * 10, sequence * 5, Some(sequence));
    }

    fn observed(ledger: &mut EventLedger, window: u64, state: bool) {
        push(
            ledger,
            EventKind::WindowOccluded {
                window,
                occluded: state,
            },
        );
        push(
            ledger,
            EventKind::MinimizedObserved {
                window,
                minimized: Some(state),
            },
        );
    }

    #[test]
    fn requests_and_unknown_or_wrong_window_observations_do_not_prove_minimize() {
        let mut ledger = EventLedger::default();
        push(
            &mut ledger,
            EventKind::MinimizeRequested {
                window: 7,
                minimized: true,
            },
        );
        let cursor = ledger.cursor();
        assert!(!ledger.window_state_since(cursor, 7, true));
        observed(&mut ledger, 8, true);
        push(
            &mut ledger,
            EventKind::WindowOccluded {
                window: 7,
                occluded: true,
            },
        );
        push(
            &mut ledger,
            EventKind::MinimizedObserved {
                window: 7,
                minimized: None,
            },
        );
        assert!(!ledger.window_state_since(cursor, 7, true));
        push(
            &mut ledger,
            EventKind::MinimizedObserved {
                window: 7,
                minimized: Some(true),
            },
        );
        assert!(ledger.window_state_since(cursor, 7, true));
    }

    #[test]
    fn both_actual_transitions_must_follow_each_request_and_latest_state_wins() {
        let mut ledger = EventLedger::default();
        observed(&mut ledger, 7, true);
        let prior = ledger.cursor();
        assert!(!ledger.window_state_since(prior, 7, true));
        push(
            &mut ledger,
            EventKind::MinimizeRequested {
                window: 7,
                minimized: false,
            },
        );
        let restore = ledger.cursor();
        assert!(!ledger.window_state_since(restore, 7, false));
        push(
            &mut ledger,
            EventKind::MinimizedObserved {
                window: 7,
                minimized: Some(false),
            },
        );
        assert!(!ledger.window_state_since(restore, 7, false));
        push(
            &mut ledger,
            EventKind::WindowOccluded {
                window: 7,
                occluded: false,
            },
        );
        assert!(ledger.window_state_since(restore, 7, false));
        push(
            &mut ledger,
            EventKind::WindowOccluded {
                window: 7,
                occluded: true,
            },
        );
        assert!(!ledger.window_state_since(restore, 7, false));
        push(
            &mut ledger,
            EventKind::WindowOccluded {
                window: 7,
                occluded: false,
            },
        );
        push(
            &mut ledger,
            EventKind::MinimizedObserved {
                window: 7,
                minimized: None,
            },
        );
        assert!(!ledger.window_state_since(restore, 7, false));
    }

    #[test]
    fn sleep_requires_an_ordered_new_native_pair_and_no_later_sleep() {
        let mut ledger = EventLedger::default();
        push(&mut ledger, EventKind::WillSleep);
        push(&mut ledger, EventKind::DidWake);
        let arm = ledger.cursor();
        assert_eq!(ledger.sleep_cycle_since(arm), None);
        push(&mut ledger, EventKind::DidWake);
        assert_eq!(ledger.sleep_cycle_since(arm), None);
        push(&mut ledger, EventKind::WillSleep);
        let sleep = ledger.cursor();
        assert_eq!(ledger.sleep_cycle_since(arm), None);
        push(&mut ledger, EventKind::DidWake);
        assert_eq!(
            ledger.sleep_cycle_since(arm),
            Some((sleep, ledger.cursor()))
        );
        push(&mut ledger, EventKind::WillSleep);
        assert_eq!(ledger.sleep_cycle_since(arm), None);
    }

    #[test]
    fn overflow_invalidates_even_an_earlier_successful_cycle() {
        let mut ledger = EventLedger::default();
        observed(&mut ledger, 7, false);
        push(&mut ledger, EventKind::WillSleep);
        push(&mut ledger, EventKind::DidWake);
        assert!(ledger.window_state_since(0, 7, false));
        assert!(ledger.sleep_cycle_since(0).is_some());
        for _ in 0..MAX_EVENTS {
            push(&mut ledger, EventKind::DidWake);
        }
        assert_eq!(ledger.events.len(), MAX_EVENTS);
        assert!(ledger.dropped > 0);
        assert!(!ledger.window_state_since(0, 7, false));
        assert_eq!(ledger.sleep_cycle_since(0), None);
    }
}
// END PURE CONTRACT

use bevy::{
    ecs::system::NonSendMarker,
    prelude::*,
    window::{PrimaryWindow, WindowOccluded},
};
use bevy_metalfx::MetalFxObservationFrame;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Shared {
    ledger: Mutex<EventLedger>,
    started: Instant,
    wall_started: SystemTime,
}

impl Shared {
    fn record(&self, kind: EventKind, frame: Option<u64>) -> Option<u64> {
        let mut ledger = self.ledger.lock().ok()?;
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis()
            .try_into()
            .ok()?;
        let process_ms = self.started.elapsed().as_millis().try_into().ok()?;
        ledger.record(kind, unix_ms, process_ms, frame);
        (ledger.dropped == 0).then(|| ledger.cursor())
    }
}

/// Installed only for the optional native-window lifecycle exercises.
/// Native sleep events are observations. This type never requests OS sleep or wake.
#[derive(Resource)]
pub struct WindowLifecycle {
    shared: Arc<Shared>,
    native_sleep_available: bool,
    native_sleep_error: Option<String>,
}

impl WindowLifecycle {
    pub fn cursor(&self) -> Option<u64> {
        let ledger = self.shared.ledger.lock().ok()?;
        (ledger.dropped == 0).then(|| ledger.cursor())
    }

    /// Log a request independently from the OS's subsequent observations.
    /// The caller must issue `Window::set_minimized` for its own test window.
    pub fn record_minimize_request(
        &self,
        window: Entity,
        minimized: bool,
        frame: u64,
    ) -> Option<u64> {
        self.shared.record(
            EventKind::MinimizeRequested {
                window: window.to_bits(),
                minimized,
            },
            Some(frame),
        )
    }

    pub fn window_state_since(&self, cursor: u64, window: Entity, minimized: bool) -> bool {
        self.shared
            .ledger
            .lock()
            .is_ok_and(|ledger| ledger.window_state_since(cursor, window.to_bits(), minimized))
    }

    pub fn native_sleep_available(&self) -> bool {
        self.native_sleep_available
    }

    pub fn sleep_cycle_since(&self, cursor: u64) -> Option<(u64, u64)> {
        if !self.native_sleep_available {
            return None;
        }
        self.shared.ledger.lock().ok()?.sleep_cycle_since(cursor)
    }

    /// SystemTime includes elapsed sleep; an Instant timeout may not on macOS.
    /// A clock rollback is unavailable evidence and must fail the caller's gate.
    pub fn wall_elapsed(&self) -> Option<Duration> {
        self.shared.wall_started.elapsed().ok()
    }

    pub fn report(&self) -> Value {
        let (events, dropped, poisoned) = match self.shared.ledger.lock() {
            Ok(ledger) => (ledger.events.clone(), ledger.dropped, false),
            Err(error) => {
                let ledger = error.into_inner();
                (ledger.events.clone(), ledger.dropped, true)
            }
        };
        let events: Vec<Value> = events.into_iter().map(|event| {
            let details = match event.kind {
                EventKind::MinimizeRequested { window, minimized } => json!({"kind":"minimize_requested", "window_id":window,"minimized":minimized}),
                EventKind::WindowOccluded { window, occluded } => json!({"kind":"window_occluded","window_id":window,"occluded":occluded}),
                EventKind::MinimizedObserved { window, minimized } => json!({"kind":"native_minimized_observed","window_id":window,"minimized":minimized}),
                EventKind::WillSleep => json!({"kind":"workspace_will_sleep"}),
                EventKind::DidWake => json!({"kind":"workspace_did_wake"}),
            };
            json!({"sequence":event.sequence,"unix_ms":event.unix_ms,"process_elapsed_ms":event.process_elapsed_ms,"main_frame":event.frame,"event":details})
        }).collect();
        json!({"scope":"WindowOccluded messages and native winit minimized-state transitions; NSWorkspace system sleep/wake notifications. Requests are separate. No screen-sleep, session-unlock, continuous visibility, panel delivery, or GPU completion inference.",
            "clock":"unix_ms is SystemTime UTC; process_elapsed_ms is Instant and may exclude system sleep on macOS; sequence establishes callback/collector order",
            "native_sleep_available":self.native_sleep_available,"native_sleep_error":self.native_sleep_error,
            "wall_elapsed_seconds":self.wall_elapsed().map(|elapsed| elapsed.as_secs_f64()),
            "max_events":MAX_EVENTS,"dropped_events":dropped,"poisoned":poisoned,"events":events})
    }
}

pub struct WindowLifecyclePlugin;

impl Plugin for WindowLifecyclePlugin {
    fn build(&self, app: &mut App) {
        let shared = Arc::new(Shared {
            ledger: Mutex::new(EventLedger::default()),
            started: Instant::now(),
            wall_started: SystemTime::now(),
        });
        #[cfg(target_os = "macos")]
        let result = native::Subscriptions::install(shared.clone());
        #[cfg(target_os = "macos")]
        let (native_sleep_available, native_sleep_error) = match result {
            Ok(subscriptions) => {
                app.world_mut().insert_non_send(subscriptions);
                (true, None)
            }
            Err(error) => (false, Some(error)),
        };
        #[cfg(not(target_os = "macos"))]
        let (native_sleep_available, native_sleep_error) = (
            false,
            Some("NSWorkspace sleep/wake notifications require macOS".into()),
        );
        app.insert_resource(WindowLifecycle {
            shared,
            native_sleep_available,
            native_sleep_error,
        })
        .add_systems(PreUpdate, collect_window_events);
    }
}

// Bevy 0.19 stores WinitWindows in thread-local storage. NonSendMarker forces
// this collector onto the main thread, where querying the native window is valid.
fn collect_window_events(
    _main_thread: NonSendMarker,
    lifecycle: Res<WindowLifecycle>,
    frame: Res<MetalFxObservationFrame>,
    mut messages: MessageReader<WindowOccluded>,
    windows: Query<Entity, With<PrimaryWindow>>,
    mut last_minimized: Local<BTreeMap<u64, Option<bool>>>,
) {
    for message in messages.read() {
        lifecycle.shared.record(
            EventKind::WindowOccluded {
                window: message.window.to_bits(),
                occluded: message.occluded,
            },
            Some(frame.0),
        );
    }
    for window in &windows {
        let minimized = bevy::winit::WINIT_WINDOWS.with_borrow(|native| {
            native
                .get_window(window)
                .and_then(|native| native.is_minimized())
        });
        if last_minimized.get(&window.to_bits()) != Some(&minimized) {
            last_minimized.insert(window.to_bits(), minimized);
            lifecycle.shared.record(
                EventKind::MinimizedObserved {
                    window: window.to_bits(),
                    minimized,
                },
                Some(frame.0),
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::{EventKind, Shared};
    use block2::RcBlock;
    use objc2::{rc::Retained, runtime::ProtocolObject, MainThreadMarker};
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
    use std::{ptr::NonNull, sync::Arc};

    // Non-send resource: registration and removal happen on the app's main thread.
    pub(super) struct Subscriptions {
        center: Retained<NSNotificationCenter>,
        tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    }

    impl Subscriptions {
        pub(super) fn install(shared: Arc<Shared>) -> Result<Self, String> {
            let _main =
                MainThreadMarker::new().ok_or("NSWorkspace observer needs the main thread")?;
            let center = NSWorkspace::sharedWorkspace().notificationCenter();
            let mut tokens = Vec::new();
            // SAFETY: constants are valid NSNotificationName objects. Each block
            // captures only Arc<Shared> (Send+Sync), ignores the notification's
            // object, and uses no Bevy/world or main-thread-only state. With no
            // operation queue the callback may run on the notification thread.
            unsafe {
                for (name, kind) in [
                    (NSWorkspaceWillSleepNotification, EventKind::WillSleep),
                    (NSWorkspaceDidWakeNotification, EventKind::DidWake),
                ] {
                    let shared = shared.clone();
                    let callback = RcBlock::new(move |_notification: NonNull<NSNotification>| {
                        shared.record(kind, None);
                    });
                    tokens.push(center.addObserverForName_object_queue_usingBlock(
                        Some(name),
                        None,
                        None,
                        &callback,
                    ));
                }
            }
            Ok(Self { center, tokens })
        }
    }

    impl Drop for Subscriptions {
        fn drop(&mut self) {
            for token in &self.tokens {
                // SAFETY: each token is the retained observer returned by this
                // notification center and is removed exactly once before drop.
                unsafe {
                    self.center.removeObserver(token.as_ref());
                }
            }
        }
    }
}
