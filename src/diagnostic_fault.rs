//! Explicit, default-off scaler-creation faults for bounded diagnostic fixtures.
//!
//! These controls simulate a missing creation result or delayed completion at
//! the creation boundary. They do not provoke a Metal driver failure or explain
//! a historical crash. Normal builds omit this module and its render hooks.

use bevy::prelude::Resource;
use bevy::render::extract_resource::ExtractResource;
#[cfg(any(target_os = "macos", test))]
use std::sync::mpsc::{self, Receiver, Sender};

/// A diagnostic replacement for starting a new scaler-creation attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScalerCreationFault {
    /// Use normal MetalFX creation.
    #[default]
    Off,
    /// Complete an attempt with no scaler, using the ordinary failure path.
    ReturnNone,
    /// Keep the attempt pending until the fixture clears or changes this fault.
    /// No driver thread is started or blocked by the injected attempt.
    HoldPending,
}

/// A frozen fault identity carried into one extracted render frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScalerFaultSnapshot {
    /// Changes whenever the selected fault changes, including its release.
    pub generation: u64,
    /// Fault selected for this generation.
    pub fault: ScalerCreationFault,
}

/// Diagnostic-only control; leave absent or at its default in normal use.
///
/// Extraction copies the generation and fault. Updating the main resource
/// cannot mutate an already-extracted frame's decision.
#[derive(Resource, ExtractResource, Debug, Clone, Default)]
pub struct MetalFxDiagnosticFault {
    current: ScalerFaultSnapshot,
}

impl MetalFxDiagnosticFault {
    /// Select a fault. Selecting the same value again preserves its generation.
    pub fn set(&mut self, fault: ScalerCreationFault) {
        if fault != self.current.fault {
            self.current.generation = self
                .current
                .generation
                .checked_add(1)
                .expect("diagnostic fault generation exhausted");
            self.current.fault = fault;
        }
    }

    /// Resume normal creation in a new generation if a fault was selected.
    pub fn clear(&mut self) {
        self.set(ScalerCreationFault::Off);
    }

    /// Return the immutable identity used by the render creation/cache path.
    pub fn snapshot(&self) -> ScalerFaultSnapshot {
        self.current
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) type InjectedReceiver<T> = (Receiver<Option<T>>, Option<Sender<Option<T>>>);

/// The optional sender keeps a held attempt connected; it is dropped with the
/// old pending generation. Synthetic completion still uses the normal receiver.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn injected_receiver<T>(fault: ScalerCreationFault) -> Option<InjectedReceiver<T>> {
    match fault {
        ScalerCreationFault::Off => None,
        ScalerCreationFault::ReturnNone => {
            let (sender, receiver) = mpsc::channel();
            sender.send(None).expect("new receiver is alive");
            Some((receiver, None))
        }
        ScalerCreationFault::HoldPending => {
            let (sender, receiver) = mpsc::channel();
            Some((receiver, Some(sender)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::TryRecvError;

    #[test]
    fn default_fault_does_not_replace_real_creation() {
        let control = MetalFxDiagnosticFault::default();
        assert_eq!(control.snapshot(), ScalerFaultSnapshot::default());
        assert!(injected_receiver::<u32>(control.snapshot().fault).is_none());
    }

    #[test]
    fn release_changes_generation_without_mutating_extracted_identity() {
        let mut control = MetalFxDiagnosticFault::default();
        control.set(ScalerCreationFault::HoldPending);
        let extracted = control.clone();
        let held = extracted.snapshot();
        assert_eq!(held.fault, ScalerCreationFault::HoldPending);
        assert_eq!(held.generation, 1);
        control.set(ScalerCreationFault::HoldPending);
        assert_eq!(control.snapshot(), held);
        control.clear();
        assert_eq!(control.snapshot().fault, ScalerCreationFault::Off);
        assert_eq!(control.snapshot().generation, 2);
        assert_eq!(extracted.snapshot(), held);
        control.clear();
        assert_eq!(control.snapshot().generation, 2);
    }

    #[test]
    fn injected_failure_is_a_real_empty_creation_result() {
        let (receiver, keeper) = injected_receiver::<u32>(ScalerCreationFault::ReturnNone)
            .expect("the fixture must replace this creation attempt");
        assert!(keeper.is_none());
        assert_eq!(receiver.try_recv(), Ok(None));
    }

    #[test]
    fn held_attempt_stays_pending_until_its_owner_releases_it() {
        let (receiver, keeper) = injected_receiver::<u32>(ScalerCreationFault::HoldPending)
            .expect("the fixture must hold this creation attempt");
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
        drop(keeper);
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
    }
}
