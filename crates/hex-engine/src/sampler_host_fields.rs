//! Audited internal sampler integration, NOT hostile-consumer unforgeability.
//! Producers expose actual borrowed fields. Engine computes layout/traversal;
//! body observations certify footprint only, never unseen children or admission.
mod physical;
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
    FamilyCompiledFields, FamilyCompiledRecordFields, FamilyFields, FamilyRequestFields,
    FamilyRequestPayload, FamilyRequestRecords, FamilySnapshotFields,
};

mod worker;
pub use worker::SamplerWorkerFields;
