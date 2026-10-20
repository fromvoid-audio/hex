//! Audited internal sampler integration, NOT hostile-consumer unforgeability.
//! Producers expose actual borrowed fields. Engine computes layout/traversal;
//! body observations certify footprint only, never unseen children or admission.
mod physical;
pub use crate::audio_storage::OriginalSamplerPendingModelFields;
pub use physical::{BodyRef, SharedMetadataRef, VecRef};
mod file_slots;
pub use file_slots::{
    FileJobFields, FileSlotsFields, PublicationFields, StagedFileFields, WavWriterFields,
};
pub(crate) use physical::{BodyLayout, SharedLayout};

#[cfg(test)]
mod tests;

mod captured_source;
pub use captured_source::{CapturedSourceFields, CapturedSourcePayload};

mod waveform;
pub use waveform::{WaveformFields, WaveformLevelsRef, WaveformPayload, WaveformSamples};

mod snapshot_view;
pub use snapshot_view::{SnapshotViewFields, SnapshotViewPayload};

mod family;
pub use family::{
    ExportResultOutcomeFields, ExportResultPayloadFields, FamilyCompiledFields,
    FamilyCompiledRecordFields, FamilyEventMetadataFields, FamilyExportResultFields, FamilyFields,
    FamilyRequestFields, FamilyRequestPayload, FamilyRequestRecords, FamilySnapshotFields,
};

mod worker;
pub use worker::{
    DocumentFields, DocumentTokenFields, JournalFields, SamplerWorkerFields,
    WorkerCurrentOwnerFields, WorkerInlineGestureFields, WorkerLegacyInputFields,
};
mod event;
pub use event::{
    EventBatchFailureFields, EventMetadataFields, EventReasonFields, TuningLibraryMetadataPayload,
    TuningSlotMetadataFields,
};

mod native;
pub use native::{
    NativeFields, NativePreparationFields, NativeRuntimeFields, NativeTransferredRuntimeFields,
};

pub use crate::audio_storage::native_state::{
    NativeStateStorageLease, NativeStateStorageRegistration,
};

pub use crate::audio_storage::native_state::{
    NativeClaimBirthFields, NativeClaimStorageLease, NativeClaimStorageRegistration,
};

pub use crate::audio_storage::native_carrier::{
    NativeCarrierStorageLease, NativeCarrierStorageRegistration,
};
pub use crate::audio_storage::native_fixed_ring::{
    NativeFixedRingFields, NativeFixedRingLease, NativeFixedRingRegistration,
};

pub use crate::audio_storage::native_atomic_signal::{
    NativeAtomicSignal, NativeAtomicSignalRegistration,
};

pub use crate::audio_storage::native_resident::{
    NativeResidentFieldLease, NativeResidentFields, NativeResidentRegistration,
};
pub use crate::audio_storage::native_status::{
    NativeStatusFieldLease, NativeStatusFields, NativeStatusRegistration,
};

pub use crate::audio_storage::native_root_fields::{
    NativeRootFieldLease, NativeRootFieldRegistration,
};

mod resident;
pub(crate) use resident::visit_resident as visit_original_sampler_resident;
pub(crate) use resident::visit_resident_editor as visit_original_sampler_resident_editor;
pub use resident::{
    OriginalSamplerPendingSourceFields, OriginalSamplerResidentFields, ResidentAssetFields,
    ResidentClipWaveformFields, ResidentOriginalFields,
};

mod original_driver;
mod pending_session;
pub(crate) use original_driver::OriginalSamplerConstructionHost;
pub use original_driver::OriginalSamplerNativeHost;
pub use original_driver::construct_original_sampler_native_with_host_on_control;
pub use pending_session::{
    OriginalSamplerPendingSession, construct_pending_original_sampler_native_on_control,
};

mod spectrum;
pub use spectrum::{
    NativeSpectrumCoreFields, NativeSpectrumProjectCoreFields, SpectrumFloatBoxRef,
};

pub use crate::audio_storage::native_spectrum::{
    NativeSpectrumCaptureFields, NativeSpectrumCaptureLease, NativeSpectrumCapturedRegistration,
    NativeSpectrumJoinLease, NativeSpectrumPacketFields, NativeSpectrumPacketLease,
    NativeSpectrumPacketRegistration,
};

pub use crate::audio_storage::native_spectrum::{
    NativeSpectrumResultFields, NativeSpectrumResultHome, NativeSpectrumResultLease,
    NativeSpectrumResultRegistration,
};

mod family_inventory;
pub use family_inventory::{
    SamplerFamilyGuardFields, SamplerFamilyInventoryFields, SamplerOuterFamilyFields,
    SamplerOuterFamilySlotFields,
};
