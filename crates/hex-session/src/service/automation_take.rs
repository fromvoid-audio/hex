//! Session-facing completed capture queries delegate to the closed storage source.
//!
//! Storage owns semantic clock/file interpretation and bounded sequential spans so
//! the existing stream worker can consume the same producer without depending on
//! Session or accepting arbitrary page-producing closures. Session still owns live
//! source/output/transport admission and eventual canonical adoption. These APIs
//! perform control-side I/O; they grant no callback read or native resource lease.
pub use hex_storage::automation_draft::{
    AutomationSharedData, CompletedAutomationTake, MAX_PREVIEW_QUERIES, MAX_SCALAR_RANGE_FRAMES,
    MAX_SPANS_PER_STEP, PreparedAutomationTake, PreparedAutomationTakeDataLayout,
    ProjectedAutomationSpan, ProjectedSpanCursor, SpanReadProgress, SpanReadState,
};

mod adoption;
pub use adoption::{
    prepare_device_take_adoption_on_control, prepare_group_take_adoption_on_control,
};

#[cfg(test)]
use hex_project::{LoopRegion, Project};
#[cfg(test)]
use hex_storage::automation_draft::{
    AppliedEvent, CompletedDraft, DraftClock, DraftProvenance, DraftReadLimits, DraftTarget,
    MAX_APPEND_EVENTS,
};
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
mod tests;
