//! Actual private Session owning field; raw flags never acquire provenance.
use hex_engine::{CancellationQuery, RetainedControlCancellation};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[derive(Clone)]
pub(super) enum SessionCancellation {
    Unqualified(Arc<AtomicBool>),
    Retained(RetainedControlCancellation),
}
impl CancellationQuery for SessionCancellation {
    fn is_cancelled(&self) -> bool {
        match self {
            Self::Unqualified(flag) => flag.load(Ordering::Acquire),
            Self::Retained(worker) => worker.is_cancelled(),
        }
    }
}
impl SessionCancellation {
    pub(super) fn transform(&self) -> hex_engine::audio_transform::AudioTransformPreparation {
        match self {
            Self::Unqualified(flag) => hex_engine::audio_transform::AudioTransformPreparation::new(flag.clone()),
            Self::Retained(worker) => hex_engine::audio_transform::AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone()),
        }
    }
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        match self {
            Self::Unqualified(_) => Err(hex_engine::AudioStorageError::UnknownStorage),
            Self::Retained(worker) => worker.visit_storage_on_control(ledger),
        }
    }
}
