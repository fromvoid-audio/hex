//! # Purpose
//! Own verified WAV and sealed scalar source workers, fixed typed page pools and borrowed project-rate page-cache
//! endpoints.
//!
//! # Dependencies
//! Uses hex-engine/hex-project contracts, hex-media readers/conversion, hex-warp's
//! bounded pitch kernel, hex-storage closed automation readers and rtrb ownership queues.
//!
//! # Does not own
//! Does not edit projects, own CPAL or decode/hash on callback. Only the joining control owner
//! wakes/joins workers; page endpoints are destroyed on control after callback quiescence.
// Boxed entries keep the 4096-frame payload at a stable address while ownership
// moves through the bounded worker, ready and return queues.
#![allow(clippy::vec_box)]

mod footprint;
pub use footprint::{AllocationData, EndpointData, StreamDataFootprint, TransportData, WorkerData};
mod endpoint;
mod runtime;
#[cfg(feature = "test-support")]
pub mod test_support;
mod types;
mod worker;

pub use endpoint::EndpointStats;
pub use hex_engine::{StreamPageExtent, StreamResourceExtent};
pub use runtime::{
    AudioStreamStorageError, AudioStreamStorageLedger, AudioStreamStorageReservation,
    AudioStreamStorageSummary, CatalogLease, ConstructorPageLoan, ConstructorStreamDomain,
    CurrentQuantumRefusal, CurrentQuantumReservation, OpaqueStreamCost, OpaqueStreamHandleGroup,
    PagePoolReport, PendingStreamDomain, PreparedRolePromotion, PreparedStreamDisposal,
    ProtectedStreamPages, ProtectionRole, ReadinessArtifact, ReadinessProgress, ReadinessRefusal,
    ReadinessStamp, ShutdownError, StreamBusy, StreamCallbackPages, StreamControl,
    StreamDisposalError, StreamDisposalPhase, StreamDomainConstructorSummary,
    StreamDomainSealRefusal, StreamHandleLocation, StreamOwningHandleObligation,
    StreamPairMismatch, StreamPreparationAllowance, StreamPreparationBound,
    StreamReadinessReservation, StreamRuntime, StreamSourceDomainObligation,
};
pub use types::{
    CatalogFootprint, PageKey, ResourcePageKey, ScalarSharedData, ScalarSourceDefinition,
    SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind, StreamRuntimeError,
};
