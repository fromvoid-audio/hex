//! Registered immutable Event metadata in the EXISTING snapshot family.
//! No runtime, compiled graph, model, meter or input owner escapes a read.
use super::SnapshotRead;
use crate::service::{AudioBatchMemberFailure, DevicePreviewIdentity, TuningPreviewIdentity};
use crate::{
    device_gesture::{GestureAck, GestureSnapshot},
    tuning_gesture::{TuningGestureAck, TuningGestureSnapshot, TuningLibraryResult},
};

/// Scalar response facts only. Source and backend publication association are
/// separate actual retained fields, never inferred from these completion bits.
#[derive(Clone, Copy, Default)]
pub struct SourceEventFacts {
    pub request_complete: bool,
    pub computer_keyboard_complete: Option<u64>,
    pub save_complete: bool,
    pub open_complete: bool,
    pub project_replaced: bool,
    pub stopped_playback_restore_complete: bool,
    pub created: Option<hex_project::Created>,
    pub duplicate_complete: bool,
    pub restore_complete: Option<u64>,
    pub gesture_protocol_ack: bool,
    pub gesture_request: bool,
    pub active_device_gesture: Option<GestureSnapshot>,
    pub tuning_gesture_request: bool,
    pub active_tuning_gesture: Option<TuningGestureSnapshot>,
    pub device_preview: Option<DevicePreviewIdentity>,
    pub tuning_preview: Option<TuningPreviewIdentity>,
}

/// Owned metadata is constructed by the worker, quoted before its fresh paired
/// allocation, then moved into the actual snapshot record. UI can borrow only
/// the nominal queries below. No constructor adopts a legacy Event/ViewState.
pub struct SourceEventMetadata {
    pub(crate) facts: SourceEventFacts,
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) audio_batch_failure: Option<AudioBatchMemberFailure>,
    pub(crate) gesture_ack: Option<GestureAck>,
    pub(crate) tuning_gesture_ack: Option<TuningGestureAck>,
    pub(crate) tuning_library: Option<TuningLibraryResult>,
}
impl SourceEventMetadata {
    pub(super) fn fields(&self) -> hex_engine::sampler_host_fields::EventMetadataFields<'_> {
        use hex_engine::sampler_host_fields::{
            BodyRef, EventBatchFailureFields, EventMetadataFields, EventReasonFields,
            TuningLibraryMetadataPayload,
        };
        let Self {
            facts: _,
            status,
            error,
            audio_batch_failure,
            gesture_ack,
            tuning_gesture_ack,
            tuning_library,
        } = self;
        EventMetadataFields {
            body: BodyRef::of(self),
            status: status.as_ref(),
            error: error.as_ref(),
            batch_failure: audio_batch_failure.as_ref().map(|failure| {
                let AudioBatchMemberFailure {
                    member: _,
                    path,
                    explanation,
                } = failure;
                EventBatchFailureFields {
                    body: BodyRef::of(failure),
                    path,
                    explanation,
                }
            }),
            device_ack: gesture_ack.as_ref().map(|ack| {
                let GestureAck {
                    key: _,
                    phase: _,
                    sequence: _,
                    accepted: _,
                    reason,
                } = ack;
                EventReasonFields {
                    body: BodyRef::of(ack),
                    reason: reason.as_ref(),
                }
            }),
            tuning_ack: tuning_gesture_ack.as_ref().map(|ack| {
                let TuningGestureAck {
                    key: _,
                    phase: _,
                    sequence: _,
                    accepted: _,
                    reason,
                } = ack;
                EventReasonFields {
                    body: BodyRef::of(ack),
                    reason: reason.as_ref(),
                }
            }),
            tuning_library: tuning_library
                .as_ref()
                .map(|library| library as &dyn TuningLibraryMetadataPayload),
        }
    }
}

#[derive(Clone, Copy)]
pub struct SourceEventRead<'a> {
    pub(super) metadata: &'a SourceEventMetadata,
}
impl<'a> SourceEventRead<'a> {
    pub fn facts(self) -> SourceEventFacts {
        self.metadata.facts
    }
    pub fn status(self) -> Option<&'a str> {
        self.metadata.status.as_deref()
    }
    pub fn error(self) -> Option<&'a str> {
        self.metadata.error.as_deref()
    }
    pub fn audio_batch_failure(self) -> Option<SourceBatchFailureRead<'a>> {
        self.metadata
            .audio_batch_failure
            .as_ref()
            .map(|failure| SourceBatchFailureRead { failure })
    }
    pub fn gesture_ack(self) -> Option<SourceGestureAckRead<'a>> {
        self.metadata
            .gesture_ack
            .as_ref()
            .map(|ack| SourceGestureAckRead { ack })
    }
    pub fn tuning_gesture_ack(self) -> Option<SourceTuningGestureAckRead<'a>> {
        self.metadata
            .tuning_gesture_ack
            .as_ref()
            .map(|ack| SourceTuningGestureAckRead { ack })
    }
    pub fn tuning_library(self) -> Option<SourceTuningLibraryRead<'a>> {
        self.metadata
            .tuning_library
            .as_ref()
            .map(|library| SourceTuningLibraryRead { library })
    }
}
#[derive(Clone, Copy)]
pub struct SourceBatchFailureRead<'a> {
    failure: &'a AudioBatchMemberFailure,
}
impl<'a> SourceBatchFailureRead<'a> {
    pub fn member(self) -> usize {
        self.failure.member
    }
    pub fn path(self) -> &'a std::path::Path {
        &self.failure.path
    }
    pub fn explanation(self) -> &'a str {
        &self.failure.explanation
    }
}
#[derive(Clone, Copy)]
pub struct SourceGestureAckRead<'a> {
    ack: &'a GestureAck,
}
impl<'a> SourceGestureAckRead<'a> {
    pub fn key(self) -> crate::device_gesture::GestureKey {
        self.ack.key
    }
    pub fn phase(self) -> crate::device_gesture::GesturePhase {
        self.ack.phase
    }
    pub fn sequence(self) -> u64 {
        self.ack.sequence
    }
    pub fn accepted(self) -> bool {
        self.ack.accepted
    }
    pub fn reason(self) -> Option<&'a str> {
        self.ack.reason.as_deref()
    }
}
#[derive(Clone, Copy)]
pub struct SourceTuningGestureAckRead<'a> {
    ack: &'a TuningGestureAck,
}
impl<'a> SourceTuningGestureAckRead<'a> {
    pub fn key(self) -> crate::tuning_gesture::TuningGestureKey {
        self.ack.key
    }
    pub fn phase(self) -> crate::tuning_gesture::TuningGesturePhase {
        self.ack.phase
    }
    pub fn sequence(self) -> u64 {
        self.ack.sequence
    }
    pub fn accepted(self) -> bool {
        self.ack.accepted
    }
    pub fn reason(self) -> Option<&'a str> {
        self.ack.reason.as_deref()
    }
}
#[derive(Clone, Copy)]
pub struct SourceTuningLibraryRead<'a> {
    library: &'a TuningLibraryResult,
}
impl<'a> SourceTuningLibraryRead<'a> {
    pub fn operation_id(self) -> u64 {
        self.library.operation_id
    }
    pub fn selected_slot(self) -> Option<usize> {
        self.library.selected_slot
    }
    pub fn loaded_name(self) -> Option<&'a str> {
        self.library.loaded_name.as_deref()
    }
    pub fn error(self) -> Option<&'a str> {
        self.library.error.as_deref()
    }
    pub fn slots(self) -> impl ExactSizeIterator<Item = SourceTuningSlotRead<'a>> + 'a {
        self.library
            .slots
            .iter()
            .map(|slot| SourceTuningSlotRead { slot })
    }
}
#[derive(Clone, Copy)]
pub struct SourceTuningSlotRead<'a> {
    slot: &'a hex_storage::tuning_library::Slot,
}
impl<'a> SourceTuningSlotRead<'a> {
    pub fn number(self) -> usize {
        self.slot.number
    }
    pub fn name(self) -> Option<&'a str> {
        self.slot.name.as_deref()
    }
    pub fn error(self) -> Option<&'a str> {
        self.slot.error.as_deref()
    }
}
impl<'a> SnapshotRead<'a> {
    /// Event text/ACK/library facts stay protected by this exact record read.
    /// Materialize bounded UI display data, then end the read before callbacks.
    /// ```compile_fail
    /// fn escape(cap: &hex_session::snapshot_exports::SnapshotReadCapability) {
    ///     let _event = cap.read_with(|read| read.event_metadata());
    /// }
    /// ```
    pub fn event_metadata(self) -> Option<SourceEventRead<'a>> {
        self.event.map(|metadata| SourceEventRead { metadata })
    }
    pub fn canonical_source_context(self) -> Option<hex_engine::AudioSourceContext> {
        self.snapshot
            .canonical_audio_source
            .as_ref()
            .map(|source| source.context())
    }
    pub fn audio_authoring_revision(self, context: hex_engine::AudioSourceContext) -> Option<u64> {
        self.snapshot.audio_authoring_revision(context)
    }
    pub fn audio_mode(self) -> Option<crate::DocumentAudioMode> {
        self.snapshot
            .canonical_audio_source
            .as_ref()
            .map(|source| source.audio_mode())
    }
}

#[cfg(test)]
mod tests;

/// Copy observations of the inherited UI receive law. These facts convey no
/// read lifetime, native admission, publication or storage authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceEventSourceRelation {
    comparison_present: bool,
    follows: bool,
    same_source_duplicate: bool,
}
impl SourceEventSourceRelation {
    pub fn comparison_present(self) -> bool {
        self.comparison_present
    }
    pub fn follows(self) -> bool {
        self.follows
    }
    pub fn same_source_duplicate(self) -> bool {
        self.same_source_duplicate
    }
    pub(super) fn between(
        previous: &crate::service::ViewState,
        current: &crate::service::ViewState,
        generation: u64,
    ) -> Self {
        Self {
            comparison_present: current.comparison_audio.is_some(),
            follows: current
                .comparison_audio
                .as_ref()
                .is_some_and(|binding| binding.follows(previous, current, generation)),
            same_source_duplicate: previous.revision == current.revision
                && previous.project == current.project
                && previous
                    .canonical_audio_source
                    .as_ref()
                    .zip(current.canonical_audio_source.as_ref())
                    .is_some_and(|(old, source)| {
                        old.shares_owner_with(source)
                            && source.context().project_generation == generation
                            && current.audio_authoring_revision(source.context())
                                == Some(current.revision)
                    }),
        }
    }
}
