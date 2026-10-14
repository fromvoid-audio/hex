//! Bounded verified WAV worker and borrowed project-rate page cache.
// Boxed entries keep the 4096-frame payload at a stable address while ownership
// moves through the bounded worker, ready and return queues.
#![allow(clippy::vec_box)]

mod borrowed_staging;
mod construction;
mod construction_report;
mod construction_transport;
mod domain_escrow;
mod domain_types;
mod endpoint;
mod footprint;
mod page_endpoint;
mod readiness;
mod readiness_manifest;
mod revision;
mod runtime;
mod shutdown_report;
mod staging;
mod types;
mod worker;

pub use borrowed_staging::{BorrowedCompactCopy, BorrowedCopyFailure, BorrowedSourcePath};
pub use construction::{
    ConstructionCapacityReport, ConstructionCleanup, ConstructionRefusal, ConstructionState,
    FinishedStreamInput, RecoveredStreamInput, StreamConstructionRequest,
};
pub use construction_report::WorkerConstructionObservation;
pub use construction_transport::TransportSetupObservation;
pub use domain_escrow::{
    CallbackDomainBatch, DomainAdmission, DomainAdmissionFailure, MediaCleanupObserver,
    MediaDomainReaper, MediaReturnFailure, RejectedMediaReturn, ReturnedMediaDomain,
    TerminalBatchFailure, TerminalBatchSender, domain_escrow, reusable_domain_escrow,
};
pub use domain_types::{
    DomainCleanupProgress, DomainEscrowFootprint, DomainId, DomainIds, DomainPoll, DomainRefusal,
    DomainReturnStatus, DomainSetupFailure, MediaCleanupPhase, MediaDomainInput,
};
pub use endpoint::EndpointStats;
pub use footprint::{
    AllocationCapacity, CatalogAllocationFootprint, ConstructionAllocationFootprint,
    EndpointAllocationFootprint, StreamAllocationFootprint, TransportAllocationFootprint,
};
pub use page_endpoint::{PageEndpointMismatch, VerifiedPageEndpoint};
pub use readiness::{
    InstalledProtectedReceipt, IntentStatus, PageReadiness, PageRequestStatus, ProtectionRole,
    ReadinessAllocationFootprint, ReadinessProgress, ReadinessSetupFailure, RequestProgress,
};
pub use readiness_manifest::{
    ManifestRefusal, PageExtent, ReadinessEpochs, ReadinessRefusal, ReadinessStamp,
    WantedPageManifest,
};
pub use revision::{
    OwnedStreamRevisionMismatch, StreamRevisionMismatch, VerifiedOwnedStreamRevision,
    VerifiedStreamRevision,
};
pub use runtime::{
    CatalogLease, PagePoolReport, ShutdownError, StreamBusy, StreamCallbackPages, StreamControl,
    StreamPairMismatch, StreamRuntime,
};
pub use shutdown_report::{ReportedShutdown, WorkerJoinObservation};
pub use staging::{
    CompactCopyError, CompactCopyReport, CompactCopyReservations, CompactSourceInput,
    InputAllocationFootprint,
};
pub use types::{
    CatalogFootprint, PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind,
    StreamRuntimeError,
};

// Typed control inputs; audio does not depend directly on media or decode WAVs.
pub use hex_media::{VerifiedWavReaderLimits, WavReaderLimits};

pub use domain_escrow::{
    CatalogCustodyAwaiter, CatalogCustodyCapacity, CatalogCustodyError, CatalogCustodyReceipt,
    CatalogCustodyRefusal, CatalogCustodyReservation, VerifiedCatalogCustodyRefusal,
};
