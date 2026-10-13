//! Bounded verified WAV worker and borrowed project-rate page cache.
// Boxed entries keep the 4096-frame payload at a stable address while ownership
// moves through the bounded worker, ready and return queues.
#![allow(clippy::vec_box)]

mod endpoint;
mod page_endpoint;
mod revision;
mod runtime;
mod types;
mod worker;

pub use endpoint::EndpointStats;
pub use page_endpoint::{PageEndpointMismatch, VerifiedPageEndpoint};
pub use revision::{StreamRevisionMismatch, VerifiedStreamRevision};
pub use runtime::{
    CatalogLease, PagePoolReport, ShutdownError, StreamBusy, StreamCallbackPages, StreamControl,
    StreamPairMismatch, StreamRuntime,
};
pub use types::{
    CatalogFootprint, PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind,
    StreamRuntimeError,
};
