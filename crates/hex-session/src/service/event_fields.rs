//! Borrow only ACTUAL Event metadata. Other Event owners are never declared
//! absent or qualified by this finite child projection.
use super::*;
use hex_engine::sampler_host_fields::{
    BodyRef, EventBatchFailureFields, EventMetadataFields, EventReasonFields,
    TuningLibraryMetadataPayload, TuningSlotMetadataFields, VecRef,
};

impl Event {
    pub(super) fn event_metadata_fields_on_control(&self) -> EventMetadataFields<'_> {
        let Self {
            state: _,
            snapshot_reads: _,
            playback: _,
            status,
            error,
            audio_batch_failure,
            request_complete: _,
            computer_keyboard_complete: _,
            tuning_audition_complete: _,
            live_expression_complete: _,
            audio_batch_complete: _,
            save_complete: _,
            open_complete: _,
            project_replaced: _,
            stopped_playback_restore_complete: _,
            created: _,
            duplicate_complete: _,
            restore_complete: _,
            device_preview: _,
            gesture_ack,
            gesture_protocol_ack: _,
            gesture_request: _,
            active_device_gesture: _,
            tuning_preview: _,
            tuning_gesture_ack,
            tuning_gesture_request: _,
            active_tuning_gesture: _,
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
                .map(|value| value as &dyn TuningLibraryMetadataPayload),
        }
    }
}

struct LibraryParts<'a> {
    slots: &'a Vec<hex_storage::tuning_library::Slot>,
    loaded_name: &'a Option<String>,
    error: &'a Option<String>,
}
fn library_parts(result: &TuningLibraryResult) -> LibraryParts<'_> {
    let TuningLibraryResult {
        operation_id: _,
        slots,
        selected_slot: _,
        loaded_name,
        error,
    } = result;
    LibraryParts {
        slots,
        loaded_name,
        error,
    }
}
impl TuningLibraryMetadataPayload for TuningLibraryResult {
    fn body(&self) -> BodyRef<'_> {
        BodyRef::of(self)
    }
    fn slots(&self) -> VecRef<'_> {
        VecRef::of(library_parts(self).slots)
    }
    fn slot(
        &self,
        index: usize,
    ) -> Result<TuningSlotMetadataFields<'_>, hex_engine::AudioStorageError> {
        let slot = library_parts(self)
            .slots
            .get(index)
            .ok_or(hex_engine::AudioStorageError::SourceMismatch)?;
        let hex_storage::tuning_library::Slot {
            number: _,
            name,
            error,
        } = slot;
        Ok(TuningSlotMetadataFields {
            body: BodyRef::of(slot),
            name: name.as_ref(),
            error: error.as_ref(),
        })
    }
    fn loaded_name(&self) -> Option<&String> {
        library_parts(self).loaded_name.as_ref()
    }
    fn error(&self) -> Option<&String> {
        library_parts(self).error.as_ref()
    }
}
