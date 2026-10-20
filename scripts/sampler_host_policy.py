"""Audited sampler-host producer boundary, not Rust hostile-client sealing.

These public internal borrowed projections are only constructed/implemented in
reviewed owning modules. Engine, rather than a producer callback, traverses the
finite schema. This check preserves the independent Cargo dependency checks.
"""
import re


EXACT = {
    'crates/hex-session/src/resource_plan.rs',
    'crates/hex-session/src/device_gesture.rs',
    'crates/hex-session/src/tuning_gesture.rs',
    'crates/hex-engine/src/audio_storage/original_sampler_pending.rs',
    'crates/hex-desktop/src/native_audio/tests/closed_original.rs',
    'crates/hex-audio/src/prepared/worker_publication.rs',
    'crates/hex-engine/src/retained_control_job.rs',
    'crates/hex-engine/src/retained_control_job/cancellation.rs',
    'crates/hex-audio/src/lifecycle/retained_families/inventory.rs',
    'crates/hex-session/src/snapshot_exports/family.rs',
    'crates/hex-desktop/src/snapshot_transport/inventory.rs',
    'crates/hex-desktop/src/snapshot_transport/inventory/tests.rs',
    'crates/hex-engine/src/audio_storage/native_spectrum/result.rs',
    'crates/hex-audio/src/spectrum_worker/project/result.rs',
    # Existing closed original-Sampler constructor readers; no new producer.
    'crates/hex-engine/src/native_device_resources/producer.rs',
    'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs',
    'crates/hex-engine/src/audio_candidate/preparation_operations.rs',
    'crates/hex-engine/src/audio_candidate/storage.rs',
    'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile/complete.rs',
    'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile.rs',
    'crates/hex-session/src/service/event_fields.rs',
    'crates/hex-session/src/resource_plan/original_sampler.rs',
    'crates/hex-session/src/resource_plan/original_sampler/tests.rs',
    'crates/hex-storage/src/files.rs',
    'crates/hex-storage/src/lib.rs',
    'crates/hex-engine/src/audio_storage/document_token.rs',
    'crates/hex-storage/src/journal.rs',
    'crates/hex-engine/src/audio_storage/native_root_fields.rs',
    'crates/hex-audio/src/candidate_custody/root_fields.rs',
    'crates/hex-engine/src/audio_storage/native_status.rs',
    'crates/hex-engine/src/audio_storage/native_resident.rs',
    'crates/hex-audio/src/prepared/resident_forwarding.rs',
    'crates/hex-audio/src/native_callback_owner/registered.rs',
    'crates/hex-audio/src/prepared/resident_forwarding/tests.rs',
    'crates/hex-audio/src/prepared/status_owner.rs',
    'crates/hex-audio/src/prepared/status_owner/tests.rs',
    'crates/hex-audio/src/prepared/endpoints.rs',
    'crates/hex-audio/src/gain_recording/tests.rs',
    'crates/hex-engine/src/audio_storage/native_atomic_signal.rs',
    'crates/hex-engine/src/audio_storage/native_atomic_signal/tests.rs',
    'crates/hex-audio/src/native_callback_owner/atomic_signal.rs',
    'crates/hex-audio/src/sample_preview.rs',
    'crates/hex-audio/src/gain_recording.rs',
    'crates/hex-audio/src/spectrum_target.rs',
    'crates/hex-audio/src/processor_audition.rs',
    'crates/hex-engine/src/audio_storage/native_carrier.rs',
    'crates/hex-engine/src/audio_storage/native_fixed_ring.rs',
    'crates/hex-audio/src/native_callback_owner/ring_storage/transport.rs',
    'crates/hex-audio/src/prepared.rs',
    'crates/hex-audio/src/native_callback_owner.rs',
    'crates/hex-audio/src/native_callback_owner/storage_tests.rs',
    'crates/hex-audio/src/native_callback_owner/ring_storage/registered_tests.rs',
    'crates/hex-engine/src/audio_candidate/claimed.rs',
    'crates/hex-engine/src/audio_candidate/groups.rs',  # actual constructor archive owns the wrapper counterpart
    'crates/hex-engine/src/audio_storage/native_state.rs',
    'crates/hex-engine/src/audio_storage/native_spectrum.rs',
    'crates/hex-engine/src/audio_storage/native_spectrum/packet.rs',
    'crates/hex-engine/src/audio_storage/native_spectrum/tests.rs',
    'crates/hex-audio/src/spectrum_worker.rs',
    'crates/hex-audio/src/spectrum_worker/stop.rs',
    'crates/hex-audio/src/spectrum_worker/project.rs',
    'crates/hex-audio/src/spectrum_worker/project/packet.rs',
    'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
    'crates/hex-audio/src/candidate_custody/eligible/publication/native_host/tests.rs',
    'crates/hex-audio/src/candidate_custody.rs',
    'crates/hex-audio/src/candidate_custody/eligible/pages.rs',
    'crates/hex-audio/src/candidate_custody/eligible.rs',
    'crates/hex-audio/src/prepared/factory.rs',
    'crates/hex-audio/src/prepared/factory/tests.rs',
    'crates/hex-audio/src/candidate_custody/eligible/native_birth_tests.rs',
    'crates/hex-audio/src/candidate_custody/eligible/publication/tests.rs',
    'crates/hex-audio/src/candidate_custody/eligible/publication.rs',
    'crates/hex-audio/src/candidate_custody/eligible/publication/disposal.rs',
    'crates/hex-session/src/service/worker_roots.rs',
    'crates/hex-session/src/service/worker_startup.rs',  # exact test-only schema consumer checked below
    'crates/hex-project/src/time_range.rs',
    'crates/hex-engine/src/lib.rs',
    'crates/hex-engine/src/midi_preview.rs',
    'crates/hex-engine/src/gain_envelope.rs',
    'crates/hex-session/src/snapshot_exports/storage.rs',
    'crates/hex-session/src/snapshot_exports/native_export_results.rs',
    'crates/hex-session/src/snapshot_exports/native_transport.rs',
    'crates/hex-session/src/snapshot_exports/native_transport/tests.rs',
    'crates/hex-session/src/snapshot_exports/native_requests.rs',
    'crates/hex-media/src/waveform.rs',
    'crates/hex-media/src/waveform/shared_storage_tests.rs',
    'crates/hex-engine/src/audio_storage.rs',
    'crates/hex-engine/src/sampler_host_fields.rs',
    'crates/hex-export/src/staged_file.rs',
    'crates/hex-export/src/publication.rs',
    'crates/hex-export/src/buffered_file.rs',
    'crates/hex-session/src/service/main_control_file.rs',
    'crates/hex-session/src/service/canonical_audio_source.rs',
    'crates/hex-session/src/service/comparison_audio.rs',
    'crates/hex-session/src/snapshot_exports/event.rs',
    'crates/hex-session/src/snapshot_exports/source_event.rs',
    'crates/hex-session/src/snapshot_exports/source_event/tests.rs',
    'crates/hex-session/src/service/main_control_file/tests.rs',
}
# Whole subtrees are restricted to the new Engine schema itself, not arbitrary
# modules in an approved crate. New production producers require explicit review.
PREFIXES = ('crates/hex-engine/src/sampler_host_fields/',)
NAMES = re.compile(r'\b(?:EventMetadataFields|EventReasonFields|EventBatchFailureFields|TuningLibraryMetadataPayload|TuningSlotMetadataFields|event_metadata_fields_on_control|matches_constructor_worker_fields_on_control|matches_actual_field_role_on_control|matches_constructor_channel_fields_on_control|matches_live_endpoint_on_control|NativeRootFieldRegistration|NativeRootFieldLease|borrow_sampler_native_root_fields_on_control|ResidentArchive|ResidentHandle|NativeResidentFields|NativeResidentRegistration|NativeResidentFieldLease|StatusArchive|StatusHandle|NativeStatusRegistration|NativeStatusFieldLease|NativeStatusFields|atomic_signal|NativeAtomicSignal|NativeAtomicSignalRegistration|registered_prepared_ring|NativePreparedProducer|NativePreparedConsumer|native_prepared_ring|NativeFixedRingFields|NativeFixedRingLease|NativeFixedRingRegistration|NativeCarrierStorageRegistration|NativeCarrierStorageLease|borrow_sampler_worker_fields_on_control|track_buffer_on_control|FamilySnapshotFields|FamilyCompiledRecordFields|FamilyCompiledFields|FamilyRequestPayload|FamilyRequestFields|FamilyRequestRecords|borrow_sampler_family_fields_on_control|borrow_sampler_family_requests_on_control|borrow_sampler_family_compiled_on_control|SnapshotViewFields|SnapshotViewPayload|CapturedSourcePayload|borrow_sampler_snapshot_children_fields_on_control|borrow_sampler_snapshot_view_fields_on_control|WaveformFields|WaveformPayload|WaveformLevelsRef|WaveformSamples|borrow_sampler_waveform_fields_on_control|sampler_host_fields|BodyRef|VecRef|SharedMetadataRef|SamplerWorkerFields|DocumentFields|DocumentTokenFields|FileTokenStorageObservation|file_token_storage_on_control|borrowed_file_token_arc_field_on_control|document_storage_fields_on_control|document_file_token_storage_layout_supported|borrow_sampler_document_fields_on_control|JournalFields|journal_storage_fields_on_control|CapturedSourceFields|FamilyFields|FamilyExportResultFields|ExportResultPayloadFields|ExportResultOutcomeFields|borrow_sampler_export_result_fields_on_control|FileSlotsFields|NativeFields|sampler_fields_on_control|borrow_sampler_file_slots_fields_on_control|borrow_sampler_captured_source_fields_on_control)\b')


def violations(path: str, source: str) -> list[str]:
    if 'ComparisonAudioBinding::advance' in source and path not in {
            'crates/hex-session/src/service.rs',
            'crates/hex-session/src/service/comparison_audio.rs'}:
        return [f'{path}: ordered source binding has one owning worker constructor']
    if path == 'crates/hex-session/src/service/comparison_audio.rs':
        required = ['source: SharedCanonicalAudioSource', 'pub(super) fn advance(',
                    'previous.revision.checked_add(1)', 'source.matches_session_sources(session)',
                    'previous.audio_authoring_revision(source.context())',
                    'source.shares_owner_with(&self.source)', 'source: source.clone()',
                    'canonical_audio_source::sampler_fields_on_control(source)']
        if any(step not in source for step in required):
            return [f'{path}: ordered binding retains actual source and validates private producer chain']
        if re.search(r'Arc::|unsafe|transmute|source_bindings\(', source):
            return [f'{path}: no raw source adoption or temporary binding owner']
    if path == 'crates/hex-engine/src/sampler_host_fields/snapshot_view.rs':
        if '[payload.source(), payload.comparison_source()]' not in source:
            return [f'{path}: both actual source owning fields are mandatory']
    if path == 'crates/hex-session/src/snapshot_exports/transport.rs':
        if 'pub fn follows_source(' in source and any(step not in source for step in [
                'self.route != previous.route', 'std::ptr::eq(self.operations, previous.operations)',
                '(self.operations.follows_source)(self.route, current, previous, generation)']):
            return [f'{path}: ordered source query uses same actual route and adapter']
    if 'sampler_parameters_on_control' in source and path not in {
            'crates/hex-engine/src/lib.rs',
            'crates/hex-engine/src/native_device_view.rs',
            'crates/hex-engine/src/native_device_view/sampler_parameters.rs',
            'crates/hex-session/src/snapshot_exports.rs'}:
        return [f'{path}: Sampler controls are Copy facts through the concrete codec and nominal snapshot query']
    if path == 'crates/hex-engine/src/native_device_view/sampler_parameters.rs':
        production = source.split('#[cfg(test)]\nmod tests {', 1)[0]
        for forbidden in ['.clone(', 'Arc::', 'Vec::', 'to_native(', 'migrate_state(',
                          'default_state(', 'descriptors_for_state(', 'native_instrument_registry(']:
            if forbidden in production:
                return [f'{path}: Sampler display query cannot construct model/state/registry owners']
        for required in ['DeviceStateRead::from_native(state)?', 'METADATA.descriptors()',
                         'METADATA.read_parameter(state, descriptor.id)?',
                         'descriptor.validate(current)?', 'semantics.validate(*descriptor)?',
                         'METADATA.parameter_enabled(state, descriptor.id)?']:
            if required not in production:
                return [f'{path}: Sampler Copy facts must come from the actual validated borrowed codec']
    initial_owners = {
        'crates/hex-session/src/service.rs',
        'crates/hex-session/src/service/audio_sink.rs',
        'crates/hex-session/src/service/worker_startup.rs',
        'crates/hex-session/src/resource_plan/original_sampler.rs',
    }
    if re.search(r'\b(?:WorkerInitialSessionView|InitialSnapshotPhase|original_sampler_initial_writers_on_control|resume_original_sampler_initial_writers_on_control|original_sampler_initial_session_loan_on_control|resume_original_sampler_initial_session_loan_on_control|initialize_session_with_startup_roots)\b', source) and path not in initial_owners:
        return [f'{path}: initial Session purpose has only the actual startup/Session/native integration owners']
    if path == 'crates/hex-session/src/service/worker_startup.rs':
        production = source.split('#[cfg(test)]\nmod tests {', 1)[0]
        if NAMES.search(production):
            return [f'{path}: startup phase does not grant generic host projection authority']
        compact = re.sub(r'\s+', '', production)
        for step in ['*self.phase!=InitialSnapshotPhase::Fresh',
                     'self.last_source.is_some()', 'self.cancellation.is_cancelled()',
                     'self.startup.session.as_ref()',
                     'session.original_sampler_initial_writers_on_control()?',
                     '*self.phase=InitialSnapshotPhase::Entered;',
                     'Err(hex_engine::AudioStorageError::UnknownStorage)']:
            if step not in compact:
                return [f'{path}: initial phase requires actual same Session/cancellation/once-only state and unresolved transports']
        # Nested focused controls use existing projections; none is a new
        # production implementation or constructor. Missing cfg(test) leaves
        # their names in production and is rejected by the check above.
        source = production
    if path == 'crates/hex-session/src/resource_plan/original_sampler.rs':
        compact = re.sub(r'\s+', '', source)
        if 'source.purpose!=Some(PendingSourcePurpose::Edit)' not in compact or \
           'self.pending_source.purpose!=Some(PendingSourcePurpose::Edit)' not in compact:
            return [f'{path}: initial pair cannot use allocating edit stage or Editor commit']
    if re.search(r'\.(?:resume_)?original_sampler_initial_writers_on_control\s*\(', source) and path != 'crates/hex-session/src/service/worker_startup.rs':
        return [f'{path}: only actual first-view startup may acquire Initial Session purpose']
    if path == 'crates/hex-session/src/snapshot_exports/source_queue.rs':
        production = source.split('#[cfg(test)]\nmod tests {', 1)[0]
        if any(token in production for token in ['try_event_construction_on_control(',
                'SharedMetadata::new', 'Arc::', 'Vec::', 'mpsc::', '.clone()']):
            return [f'{path}: Event queue only moves actual reserved reads in the existing fixed family body']
        for required in ['self.collect_releases_on_control()?',
                         'self.derive_read(&entry.canonical)?',
                         'self.source_event_head()? != Some(head)',
                         'self.validate_source_events_on_control()?']:
            if required not in production:
                return [f'{path}: Event head preserves actual identity, ACK collection and pre-move fanout']
    if re.search(r'\bSnapshotSourceEventReceiver::new_unqualified_on_control\s*\(', source) and path not in {'crates/hex-desktop/src/snapshot_transport.rs', 'crates/hex-session/src/snapshot_exports/transport/source_tests.rs'}:
        return [f'{path}: source Event receiver uses the single audited registered Desktop adapter']
    event_construction_owners = {
        'crates/hex-session/src/snapshot_exports.rs',
        'crates/hex-session/src/snapshot_exports/family.rs',
        'crates/hex-session/src/snapshot_exports/event.rs',
        'crates/hex-session/src/snapshot_exports/event/tests.rs',
        'crates/hex-session/src/snapshot_exports/source_event/tests.rs',
        'crates/hex-session/src/snapshot_exports/source_queue.rs',
        'crates/hex-session/src/snapshot_exports/transport/source_tests.rs',
    }
    if re.search(r'\b(?:SnapshotEventConstruction|EventDestination|try_event_construction_on_control|event_destination_on_control|insert_event_at_destination_on_control)\b', source) and path not in event_construction_owners:
        return [f'{path}: Event construction stays under the existing owning family writer']
    if path == 'crates/hex-session/src/snapshot_exports/family.rs':
        start = source.find('pub fn try_event_construction_on_control(')
        end = source.find('\n    }', start)
        acquire = source[start:end]
        if any(x not in acquire for x in ['self.validate_producer_on_control(key)?',
                'self.records.try_write()', 'self.abandoned.load(Ordering::Acquire)',
                'records.event_destination_on_control()?']):
            return [f'{path}: Event writer acquisition must validate actual producer, cancellation and destination']
        if any(x in acquire for x in ['.write()', '.clone(', '.take(', '::new(', 'try_reserve']):
            return [f'{path}: Event writer acquisition cannot construct or consume an owner']
    gesture_owners = {'crates/hex-session/src/device_gesture.rs',
                      'crates/hex-session/src/tuning_gesture.rs'}
    if 'inline_worker_gesture_frame_on_control' in source:
        if path not in gesture_owners | {'crates/hex-session/src/service/worker_roots.rs'}:
            return [f'{path}: inline gesture frames belong only to their actual private owners']
        if path in gesture_owners:
            start = source.find('pub(crate) fn inline_worker_gesture_frame_on_control(')
            end = source.find('\n    }', start)
            producer = re.sub(r'\s+', '', source[start:end])
            required = ['fninline_copy<T:Copy>(_:&T){}',
                        'hex_engine::sampler_host_fields::BodyRef::of(self)']
            required += (['letSelf{active,last_key}=self;',
                          'ifletSome(Active{snapshot,kind})=active{',
                          'inline_copy(snapshot);', 'inline_copy(kind);', 'inline_copy(last_key);']
                         if path.endswith('/device_gesture.rs') else
                         ['letSelf{active,initial,last_key,}=self;',
                          'inline_copy(active);', 'inline_copy(initial);', 'inline_copy(last_key);'])
            if any(x not in producer for x in required) or any(x in producer for x in ['.clone(', '.take(', '..']):
                return [f'{path}: gesture footprint requires every actual private inline Copy field']
    if path in gesture_owners:
        # This leaf exception adds ONLY one readonly actual self-body borrow.
        if any(match.group() not in {'sampler_host_fields', 'BodyRef'} for match in NAMES.finditer(source)):
            return [f'{path}: gesture frame access does not enable unrelated owning host schemas']
    if re.search(r'\b(?:WorkerInlineGestureFields|WorkerLegacyInputFields)\b', source) and path not in {
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/sampler_host_fields/worker.rs',
            'crates/hex-session/src/service/worker_roots.rs'}:
        return [f'{path}: worker gesture/input records have one actual dispatch producer']
    if path == 'crates/hex-session/src/service/worker_roots.rs':
        for required in ['clipboard: clipboard.as_ref().map(BodyRef::of)',
                         'keyboard: keyboard.as_ref().map(BodyRef::of)',
                         'keyboard_hint: keyboard_hint.as_ref().map(BodyRef::of)',
                         'audition: audition.as_ref().map(BodyRef::of)',
                         'expression: expression.as_ref().map(BodyRef::of)',
                         'device: device_gesture.inline_worker_gesture_frame_on_control()',
                         'tuning: tuning_gesture.inline_worker_gesture_frame_on_control()']:
            if re.sub(r'\s+', '', required) not in re.sub(r'\s+', '', source):
                return [f'{path}: actual private gestures and every legacy owning input remain mandatory']
    staged_model = 'crates/hex-project/src/history/original_sampler.rs'
    staged_tests = 'crates/hex-project/src/history/original_sampler/tests.rs'
    staged_names = re.search(r'\b(?:OriginalSamplerEditorTransition|OriginalSamplerPending|OriginalSamplerTransitionRequest|OriginalSamplerTransitionResult|original_sampler_transition_on_control|commit_original_sampler_transition_on_control|clear_original_sampler_transition_on_control|stage_original_sampler_after_prefix_on_control)\b', source)
    staged_readers = {
        staged_model, staged_tests, 'crates/hex-project/src/history.rs',
        'crates/hex-project/src/lib.rs',
        'crates/hex-session/src/resource_plan/original_sampler.rs',
        'crates/hex-session/src/resource_plan/tests.rs',
        'crates/hex-session/src/resource_plan/original_sampler/tests.rs',
        'crates/hex-session/src/service/worker_roots.rs',
        'crates/hex-engine/src/audio_storage/original_sampler_pending.rs',
        'crates/hex-engine/src/audio_storage/original_sampler_pending/tests.rs',
        'crates/hex-session/src/service/worker_startup.rs',  # cfg(test) only, checked above
    }
    if staged_names and path not in staged_readers:
        return [f'{path}: staged Editor candidates have one owning model and audited Session producer']
    if 'borrow_original_sampler_pending_model_fields_on_control' in source:
        pending_readers = {
            'crates/hex-engine/src/audio_storage/original_sampler_pending.rs',
            'crates/hex-engine/src/audio_storage/original_sampler_pending/tests.rs',
        }
        if path not in pending_readers:
            return [f'{path}: pending model reader needs the actual held Editor guard; production prefix is not connected']
        if path.endswith('/original_sampler_pending.rs'):
            for required in ['held.storage()', 'held.editor()',
                             'pending.inspectable()', 'pending.undo_replacement()',
                             'pending.redo_replacement()', 'pending.retired_redo()',
                             '.retained_projects_on_control()',
                             'pending.request()', 'ledger.project_exclusive_buffers(project)?',
                             'ledger.project_midi_sources(project)?',
                             'ledger.project_native_states(project)?',
                             '_ => return Err(AudioStorageError::UnknownStorage)']:
                if required not in source:
                    return [f'{path}: actual pending descendants and unknown command refusal remain mandatory']
    # Allocation-producing staging has ONE audited closed prefix caller. The
    # caller must still refuse unqualified actual worker/startup owners.
    if path == staged_model:
        compact = re.sub(r'\s+', '', source)
        required = ['pubfnstage_original_sampler_after_prefix_on_control(',
                    'fnstage(',
                    'pubfnreturn_unprepared_request_on_control(',
                    'original_sampler_pending.try_borrow_mut()',
                    'self.phase=Phase::Cleaning;', 'self.phase=Phase::Failed;']
        if any(item not in compact for item in required) or re.search(r'\.borrow(?:_mut)?\s*\(', source):
            return [f'{path}: pending guard is single-try and staging has only its closed prefix producer']
    if re.search(r'\.stage_original_sampler_after_prefix_on_control\s*\(', source) and path not in {staged_tests, 'crates/hex-session/src/resource_plan/original_sampler.rs'}:
        return [f'{path}: production Editor stage requires the actual closed Root prefix first']
    pending_session_paths = {
        'crates/hex-engine/src/sampler_host_fields.rs',
        'crates/hex-engine/src/sampler_host_fields/pending_session.rs',
        'crates/hex-session/src/resource_plan/original_sampler.rs',
        'crates/hex-session/src/resource_plan/original_sampler/tests.rs',
        'crates/hex-session/src/service/worker_roots.rs',
        'crates/hex-session/src/service/worker_startup.rs',  # cfg(test) only, checked above
        'crates/hex-audio/src/prepared/worker_publication.rs',
        'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
    }
    if re.search(r'\b(?:OriginalSamplerPendingSession|construct_pending_original_sampler_native_on_control|stage_and_capture_on_control|source_and_writer_on_control)\b', source):
        if path not in pending_session_paths:
            return [f'{path}: pending Session producer has one closed Audio/Engine consumer']
        if re.search(r'impl[^{}]*OriginalSamplerPendingSession[^{}]*for', source, re.S) and path != 'crates/hex-session/src/resource_plan/original_sampler.rs':
            return [f'{path}: only the actual Session loan implements the pending producer']
        if re.search(r'\.stage_and_capture_on_control\s*\(', source) and path != 'crates/hex-engine/src/sampler_host_fields/pending_session.rs':
            return [f'{path}: allocating producer is called only after actual closed prefix preflight']
    if path == 'crates/hex-engine/src/sampler_host_fields/pending_session.rs':
        required = ['worker.require_current_constructor_owner()?',
                    'actual_source.owner.body().same(current_source.owner.body())',
                    'revision.checked_add(1)', 'metadata_cost(session, current_source, reservation)?',
                    'preflight_prefix(', 'arithmetic_cost_for_limits(',
                    'size_of::<crate::audio_transform::OriginalSamplerTransaction',
                    'ledger.validate_selected_shared_handles_on_control()?',
                    'ledger.metadata_capacity_bytes()', 'session.stage_and_capture_on_control()?',
                    'session.source_and_writer_on_control()?', 'construct_staged_with_fresh_original(']
        if any(x not in source for x in required):
            return [f'{path}: staged constructor keeps current parent/field/counter binding and both prefix passes']
        if re.sub(r'\s+', '', source).index('session.stage_and_capture_on_control()?') < re.sub(r'\s+', '', source).index('preflight_prefix(&host'):
            return [f'{path}: prefix inspection must finish before the first allocating stage']
    if 'from_pending_original_on_control' in source and path not in {
            'crates/hex-session/src/service/canonical_audio_source.rs',
            'crates/hex-session/src/resource_plan/original_sampler.rs',
            'crates/hex-session/src/resource_plan/original_sampler/tests.rs'}:
        return [f'{path}: pending source construction belongs only to the owning Session producer']
    if re.search(r'\b(?:borrow_original_sampler_pending_model_projection_on_control|OriginalSamplerPendingModelFields)\b', source):
        projection_paths = {
            'crates/hex-engine/src/audio_storage.rs',
            'crates/hex-engine/src/audio_storage/original_sampler_pending.rs',
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/sampler_host_fields/resident.rs',
            'crates/hex-session/src/resource_plan.rs',
            'crates/hex-session/src/resource_plan/original_sampler.rs',
        }
        if path not in projection_paths:
            return [f'{path}: pending model fields originate only in the same held guard']
    legacy_pending_checks = {
        'crates/hex-engine/src/audio_storage/models.rs': 3,
        'crates/hex-engine/src/audio_storage/canonical_state.rs': 1,
        'crates/hex-engine/src/audio_storage/model_buffers.rs': 1,
    }
    if path in legacy_pending_checks and source.count('.require_no_original_sampler_pending_on_control()') != legacy_pending_checks[path]:
        return [f'{path}: subset Editor readers cannot silently omit an actual pending owner']
    if path == 'crates/hex-session/src/service.rs':
        # ONE nominal re-export of the existing owning producer, not permission
        # for Service to construct arbitrary host fields or implement schemas.
        source = source.replace(
            '    sampler_fields_on_control as sampler_captured_source_fields_on_control,\n',
            '', 1)
    owned_host_paths = {
        'crates/hex-engine/src/audio_candidate/preparation/native_fields.rs',
        'crates/hex-engine/src/audio_candidate/draft.rs',
        'crates/hex-engine/src/audio_candidate/storage.rs',
    }
    if re.search(r'\b(?:visit_unpublished_native_host_fields|visit_transferred_host_shell)\b', source) and path not in owned_host_paths:
        return [f'{path}: unpublished owned parent traversal stays inside actual Engine custodians']
    if path == 'crates/hex-engine/src/audio_candidate/preparation/native_fields.rs':
        if '.visit_transferred_host_shell(ledger, owner)?' not in source or 'owner.visit_unpublished_native_host_fields(ledger)' not in source:
            return [f'{path}: owned preparation requires both actual transferred shell and owned descendants']
    if path == 'crates/hex-engine/src/audio_candidate/draft.rs':
        for step in ['*phase != Phase::Transferred', '*identity != Some(owner.identity())', '*purpose != ConstructorPurpose::NativeFresh']:
            if step not in source:
                return [f'{path}: transferred shell must match actual destination owner and purpose']
    construction_host_paths = {
        'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs',
        'crates/hex-engine/src/audio_candidate/storage.rs',
        'crates/hex-engine/src/sampler_host_fields.rs',
        'crates/hex-engine/src/sampler_host_fields/original_driver.rs',
        'crates/hex-engine/src/sampler_host_fields/pending_session.rs',
        'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile.rs',
        'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile/complete.rs',
    }
    if re.search(r'\bOriginalSamplerConstructionHost\b', source) and path not in construction_host_paths:
        return [f'{path}: complete constructor host composition stays inside the closed Engine driver']
    if path == 'crates/hex-engine/src/sampler_host_fields/original_driver.rs':
        for step in ['std::ptr::eq(target, self.source.payload.project())',
                     'selected.require_original_sampler_constructor_custody(preparation)?',
                     'self.files.borrowed_frame()',
                     'super::worker::visit_worker(self.worker, ledger)?',
                     'super::captured_source::visit_captured_source(self.source, ledger)?',
                     'super::file_slots::visit_file_slots(self.files, ledger)?',
                     'super::family_inventory::visit_family_inventory(self.families, ledger)?',
                     'super::native::visit_native_for_original_sampler']:
            if step not in source:
                return [f'{path}: actual host composition must keep every named mandatory operand']
    if path == 'crates/hex-engine/src/audio_candidate/storage.rs':
        start = source.find('fn require_unpublished_native_host_class')
        end = source.find('/// Inspect and seal', start)
        body = source[start:end]
        if any(step not in body for step in ['inner.phase != AudioCandidateDisposalPhase::Owned', 'runtime.playback.is_main_control_runtime()', 'inner.validate_fresh_origins()?', 'self.require_unpublished_native_host_class()?', 'inner.visit_fresh_storage(ledger)']):
            return [f'{path}: unpublished owning visitor must preserve original NativeFresh phase/origins/runtime class']
    family_owners = {
        'crates/hex-session/src/snapshot_exports/source_event/tests.rs',
        # Test-only consumer of the actual existing family construction loan;
        # literal SamplerFamilyGuardFields producers remain restricted below.
        'crates/hex-session/src/snapshot_exports/event/tests.rs',
        'crates/hex-engine/src/sampler_host_fields/pending_session.rs',
        'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
        'crates/hex-audio/src/prepared/worker_publication.rs',
        'crates/hex-engine/src/sampler_host_fields/original_driver.rs',
        'crates/hex-engine/src/sampler_host_fields.rs',
        'crates/hex-engine/src/sampler_host_fields/family_inventory.rs',
        'crates/hex-audio/src/lifecycle/retained_families/inventory.rs',
        'crates/hex-session/src/snapshot_exports/family.rs',
        'crates/hex-desktop/src/snapshot_transport/inventory.rs',
        'crates/hex-desktop/src/snapshot_transport/inventory/tests.rs',
    }
    if re.search(r'\b(?:SamplerOuterFamilyFields|SamplerOuterFamilySlotFields|SamplerFamilyGuardFields|SamplerFamilyInventoryFields|borrow_sampler_family_inventory_fields_on_control|sampler_family_fields_on_control)\b', source) and path not in family_owners:
        return [f'{path}: whole-family projections have exact actual outer/inner/closed adapter owners']
    implementations = re.findall(r'impl\s+(?:[\w:]+::)?(SamplerOuterFamilyFields|SamplerFamilyInventoryFields)\s+for\s+([\w:]+)', source)
    permitted = {
        'crates/hex-audio/src/lifecycle/retained_families/inventory.rs': [('SamplerOuterFamilyFields', 'RetainedFamilyInventory')],
        'crates/hex-desktop/src/snapshot_transport/inventory.rs': [('SamplerFamilyInventoryFields', 'SnapshotFamilyInventory')],
        'crates/hex-desktop/src/snapshot_transport/inventory/tests.rs': [('SamplerFamilyInventoryFields', 'AlteredFamilies')],
    }
    if implementations and implementations != permitted.get(path):
        return [f'{path}: family inventory has one exact owning producer per boundary']
    if re.search(r'\bSamplerFamilyGuardFields\s*\{', source) and path not in {'crates/hex-engine/src/sampler_host_fields/family_inventory.rs', 'crates/hex-session/src/snapshot_exports/family.rs'}:
        return [f'{path}: actual inner family records are produced only beside their held owning guard']
    if re.search(r'\bSamplerOuterFamilySlotFields\s*\{', source) and path not in {'crates/hex-engine/src/sampler_host_fields/family_inventory.rs', 'crates/hex-audio/src/lifecycle/retained_families/inventory.rs'}:
        return [f'{path}: actual outer fields are produced only from held slot guards']
    private_family = {'crates/hex-engine/src/retained_control_job.rs', 'crates/hex-engine/src/retained_control_job/cancellation.rs', 'crates/hex-engine/src/sampler_host_fields/family_inventory.rs'}
    if re.search(r'\b(?:matches_actual_payload_body|visit_outer_fields_on_control|visit_family_field_on_control)\b', source) and path not in private_family:
        return [f'{path}: direct family traversal remains private to Engine owning fields']
    if path == 'crates/hex-engine/src/sampler_host_fields/family_inventory.rs':
        required = ['job.matches_actual_payload_body(family.owner)', 'job.visit_outer_fields_on_control(ledger)?', 'slot.cancellation.visit_family_field_on_control(ledger)?', 'super::family::visit_family(family.records, ledger)?', 'super::family::visit_export_result(family.retiring_export, ledger)?', 'inner_index != inner_guards.len()', 'slots.len() != outer_guards.len()']
        if any(step not in source for step in required):
            return [f'{path}: every actual slot/job/inner/retirement descendant is mandatory']
    if path == 'crates/hex-desktop/src/snapshot_transport/inventory.rs' and 'ledger.borrow_sampler_family_inventory_fields_on_control(self)' not in source:
        return [f'{path}: production inventory uses Engine mandatory finite traversal']
    if re.search(r'\bNativeTransferredRuntimeFields\b', source) and path not in {
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/sampler_host_fields/native.rs',
            'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs'}:
        return [f'{path}: transferred runtime fields belong to guarded actual native slots']
    if path == 'crates/hex-engine/src/sampler_host_fields/native.rs':
        required = ['transferred.state.is_live_on_control()',
                    'transferred.state.visit_fields(ledger)?',
                    'transferred.anchors.visit_transferred_graph_fields(ledger)?']
        if any(step not in source for step in required):
            return [f'{path}: transferred State footprint and actual retained graph are both mandatory']
    result_owners = {
        'crates/hex-engine/src/audio_storage/native_spectrum/result.rs',
        'crates/hex-engine/src/audio_storage/native_spectrum.rs',
        'crates/hex-engine/src/sampler_host_fields.rs',
        'crates/hex-engine/src/sampler_host_fields/native.rs',
        'crates/hex-audio/src/spectrum_worker/project/result.rs',
        'crates/hex-audio/src/spectrum_worker/project.rs',
        'crates/hex-audio/src/spectrum_worker.rs',
        'crates/hex-audio/src/candidate_custody.rs',
        'crates/hex-audio/src/candidate_custody/eligible.rs',
        'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
        'crates/hex-audio/src/native_callback_owner.rs',
    }
    if re.search(r'\b(?:NativeSpectrumResultFields|NativeSpectrumResultRegistration|NativeSpectrumResultLease|NativeSpectrumResultHome|pair_spectrum_result_on_control|pair_actual_result_on_control)\b', source) and path not in result_owners:
        return [f'{path}: result buffers have exact constructor/Root/schema owners only']
    if re.search(r'\b(?:begin_actual_result_drop_on_control|complete_actual_result_drop_on_control|record_actual_result_failure_on_control)\b', source) and path not in {
            'crates/hex-engine/src/audio_storage/native_spectrum/result.rs',
            'crates/hex-audio/src/spectrum_worker/project/result.rs'}:
        return [f'{path}: result destruction belongs to the actual affine buffer owner']
    if 'unqualified_for_ui_test' in source:
        if path not in {'crates/hex-ui/src/editor/project_spectrum/tests.rs', 'crates/hex-audio/src/spectrum_worker/project/result.rs'}:
            return [f'{path}: unqualified drawing fixture is not a production producer']
        if path.endswith('/result.rs') and not re.search(r'#\[cfg\(feature = "allocation-probe"\)\]\s*#\[doc\(hidden\)\]\s*pub fn unqualified_for_ui_test', source):
            return [f'{path}: unqualified fixture requires the explicit test feature']
    if path == 'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs' and not re.search(r'fn spectrum_results\([^}]+\{\s*&self\.bank\.spectrum_results\s*\}', source):
        return [f'{path}: result inventory must borrow the whole actual Root array']
    if path == 'crates/hex-audio/src/spectrum_worker/project/result.rs':
        if any(name not in {'sampler_host_fields', 'BodyRef'} for name in NAMES.findall(source)):
            return [f'{path}: result constructor does not authorize unrelated host schemas']
        start = source.find('impl Drop for PublishedProjectSpectrum')
        drop = source[start:source.find('mod tests {', start) if 'mod tests {' in source[start:] else None]
        steps = ['begin_actual_result_drop_on_control(fields)', 'let payload = self.frames.take()', 'drop(payload)', 'complete_actual_result_drop_on_control()']
        hook = '#[cfg(all(test, feature = "allocation-probe"))]\n        if self.panic_on_drop {\n            std::panic::panic_any(31995u64);\n        }'
        checked_drop = drop.replace(hook, '', 1)
        if re.search(r'panic_any|panic_on_drop|panic!', checked_drop):
            return [f'{path}: destructor fault injection must remain exactly test-only']
        positions = [drop.find(step) for step in steps]
        if any(i < 0 for i in positions) or positions != sorted(positions) or re.search(r'\.(?:expect|unwrap)\s*\(', drop):
            return [f'{path}: actual result destruction requires pending/take/normal completion without panic']
        if re.search(r'#\[derive\([^]]*Clone', source) or re.search(r'pub frames\s*:', source):
            return [f'{path}: affine result has no Clone or public owning frame field']
    captured_symbols = re.search(r'\b(?:NativeSpectrumCaptureFields|NativeSpectrumCapturedRegistration|NativeSpectrumCaptureLease|NativeSpectrumJoinLease|pair_actual_capture_on_control|begin_actual_buffers_drop_on_control|complete_actual_buffers_drop_on_control|finalize_actual_join_on_control|finalize_failed_spawn_on_control|pair_spectrum_capture_on_control|finish_spectrum_capture_on_control|abort_spectrum_capture_on_control|finalize_captured_frame_on_control|spawn_worker_with_parent|record_actual_capture_failure_on_control)\b', source)
    if captured_symbols or path in {'crates/hex-audio/src/spectrum_worker.rs', 'crates/hex-audio/src/lifecycle.rs'}:
        owners = {
            'crates/hex-engine/src/audio_storage/native_spectrum.rs',
            'crates/hex-engine/src/audio_storage/native_spectrum/tests.rs',
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/sampler_host_fields/native.rs',
            'crates/hex-audio/src/spectrum_worker.rs',
            'crates/hex-audio/src/spectrum_worker/native.rs',
            'crates/hex-audio/src/spectrum_worker/project/packet/carrier_binding_tests.rs',
            'crates/hex-audio/src/candidate_custody.rs',
            'crates/hex-audio/src/candidate_custody/eligible.rs',
            'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
            'crates/hex-audio/src/native_callback_owner.rs',
            'crates/hex-audio/src/lifecycle.rs',
        }
        if path not in owners:
            return [f'{path}: captured analyzer has exact existing Root/Core/Reaper owners only']
        forwarders = {
            'pair_spectrum_capture_on_control': {'crates/hex-audio/src/candidate_custody.rs', 'crates/hex-audio/src/native_callback_owner.rs', 'crates/hex-audio/src/spectrum_worker.rs'},
            'finish_spectrum_capture_on_control': {'crates/hex-audio/src/candidate_custody.rs', 'crates/hex-audio/src/native_callback_owner.rs', 'crates/hex-audio/src/spectrum_worker.rs'},
            'abort_spectrum_capture_on_control': {'crates/hex-audio/src/candidate_custody.rs', 'crates/hex-audio/src/native_callback_owner.rs', 'crates/hex-audio/src/spectrum_worker.rs'},
            'finalize_captured_frame_on_control': {'crates/hex-audio/src/spectrum_worker.rs', 'crates/hex-audio/src/spectrum_worker/native.rs', 'crates/hex-audio/src/lifecycle.rs', 'crates/hex-audio/src/spectrum_worker/project/packet/carrier_binding_tests.rs'},
            'spawn_worker_with_parent': {'crates/hex-audio/src/spectrum_worker.rs', 'crates/hex-audio/src/spectrum_worker/native.rs', 'crates/hex-audio/src/lifecycle.rs', 'crates/hex-audio/src/spectrum_worker/project/packet/carrier_binding_tests.rs'},
        }
        for name, permitted in forwarders.items():
            if name in source and path not in permitted:
                return [f'{path}: captured spectrum operation {name} has exact owning callsites']
        for method in ['pair_actual_capture_on_control', 'finalize_actual_join_on_control', 'finalize_failed_spawn_on_control']:
            if method in source and path not in {
                    'crates/hex-engine/src/audio_storage/native_spectrum.rs',
                    'crates/hex-engine/src/audio_storage/native_spectrum/tests.rs',
                    'crates/hex-audio/src/candidate_custody/eligible.rs'}:
                return [f'{path}: captured analyzer metadata authority belongs to actual NativeBank']
        for method in ['begin_actual_buffers_drop_on_control', 'complete_actual_buffers_drop_on_control', 'record_actual_capture_failure_on_control']:
            if method in source and path not in {
                    'crates/hex-engine/src/audio_storage/native_spectrum.rs',
                    'crates/hex-engine/src/audio_storage/native_spectrum/tests.rs',
                    'crates/hex-audio/src/spectrum_worker.rs'}:
                return [f'{path}: only actual captured payload may destroy its buffers']
        if path == 'crates/hex-audio/src/spectrum_worker.rs':
            compact = re.sub(r'\s+', '', source)
            joined = ['self.phase=ReaperPhase::Pending;', 'self.join.take()', 'matchjoin.join()', 'self.phase=ReaperPhase::Complete;']
            joined_positions = [compact.find(item) for item in joined]
            if any(pos < 0 for pos in joined_positions) or joined_positions != sorted(joined_positions):
                return [f'{path}: actual consumed join and Pending must precede normal completion']
            drop_start = source.find('impl Drop for CapturedSpectrumCore')
            drop_end = source.find('struct StreamContext', drop_start)
            destructor = re.sub(r'\s+', '', source[drop_start:drop_end])
            loop_start = source.find('fn worker_loop(')
            loop_end = source.find('struct CapturedSpectrumCore', loop_start)
            if re.search(r'\.(?:expect|unwrap)\s*\(', source[drop_start:drop_end] + source[loop_start:loop_end]):
                return [f'{path}: captured worker/Drop must preserve failed custody without new runtime panic']
            steps = ['begin_actual_buffers_drop_on_control(fields)', 'self.payload.take()', 'drop(payload)', 'drop(self.project.take())', 'complete_actual_buffers_drop_on_control()']
            positions = [destructor.find(step) for step in steps]
            if any(pos < 0 for pos in positions) or positions != sorted(positions):
                return [f'{path}: actual captured payload requires Pending, normal take/drop, then buffer completion']
            if (compact.count('parent.finish_spectrum_capture_on_control(association)') != 1
                    or 'self.phase!=ReaperPhase::Complete||self.join.is_some()||self.first_panic.is_some()' not in compact
                    or 'drop(self.captured_join.take())' not in compact):
                return [f'{path}: captured frame finalization requires same actual normal joined Reaper']
        if path == 'crates/hex-audio/src/lifecycle.rs':
            if 'stamp,native_mode,entry,parent.native.as_ref()' not in re.sub(r'\s+', '', source):
                return [f'{path}: analyzer birth must borrow its actual existing retirement parent']
            start = source.find('fn advance_worker_then_final_native(')
            end = source.find('// One matched retirement bundle', start)
            driver = re.sub(r'\s+', '', source[start:end])
            if (source.count('finalize_captured_frame_on_control(') != 1
                    or driver.find('reaper.advance_shutdown_on_control()') < 0
                    or driver.find('reaper.finalize_captured_frame_on_control(native)') < driver.find('reaper.advance_shutdown_on_control()')
                    or driver.find('reaper.finalize_captured_frame_on_control(native)') > driver.find('native.finish_root_after_native_fields_on_control()')):
                return [f'{path}: same parent actual join must precede frame and final Root completion']
    if 'pair_actual_fresh_ring_on_control' in source and path not in {
            'crates/hex-engine/src/audio_storage/native_fixed_ring.rs',
            'crates/hex-audio/src/native_callback_owner/ring_storage/transport.rs',
            'crates/hex-audio/src/native_callback_owner/ring_storage/registered_tests.rs'}:
        return [f'{path}: actual ring pairing belongs only to existing pinned constructor']
    if path == 'crates/hex-audio/src/native_callback_owner/registered.rs':
        if source.count('.matches_fresh_registered_payload_on_control(root)') != 3:
            return [f'{path}: each actual callback/client/worker carrier requires its fresh owning payload inspection']
    fresh_paths = {
        'crates/hex-audio/src/sample_preview.rs': ['pending.is_none()', 'active.is_none()', '!*closing'],
        'crates/hex-audio/src/prepared/endpoints.rs': ['sample_preview.matches_fresh_registered_payload_on_control(root)', '*command_ticket == 0', '!status.client_exposed.load(Acquire)'],
        'crates/hex-audio/src/prepared.rs': ['self.matches_constructor_worker_fields_on_control(root)', 'sample_preview.matches_fresh_registered_payload_on_control(root)', 'held_control_return.is_none()', 'latest_fresh_runtime_cancellation.is_none()', 'active.is_none()', 'deferred.is_none()', 'deferred_eligible.is_none()', 'domain_pages.iter().all(Option::is_none)', 'held_retirement.is_none()'],
    }
    if path in fresh_paths and any(required not in source for required in fresh_paths[path]):
        return [f'{path}: fresh carrier birth must inspect actual owning destinations before escape']
    if path == 'crates/hex-audio/src/prepared.rs':
        if 'root.matches_spectrum_capture(self.spectrum_capture.as_ref())' not in source:
            return [f'{path}: actual Callback spectrum field requires same constructor record']
    if path == 'crates/hex-audio/src/prepared/endpoints.rs':
        if 'root.matches_spectrum_reader(self.spectrum_reader.as_ref())' not in source:
            return [f'{path}: actual Client spectrum reader requires same constructor record']
    if 'spectrum_project_ring_on_control' in source and path not in {
            'crates/hex-audio/src/spectrum_worker/project.rs',
            'crates/hex-audio/src/candidate_custody.rs',
            'crates/hex-audio/src/native_callback_owner.rs'}:
        return [f'{path}: Project ring construction belongs only to actual constructor/parent forwarding']
    if path == 'crates/hex-audio/src/spectrum_worker/project.rs' and any(
            name not in {'sampler_host_fields', 'NativeFixedRingRegistration',
                         'NativePreparedProducer', 'NativePreparedConsumer', 'native_prepared_ring', 'NativeSpectrumPacketRegistration', 'BodyRef'}
            for name in NAMES.findall(source)):
        return [f'{path}: Project ring fields do not authorize other host storage producers']
    if re.search(r'\b(?:NativeSpectrumPacketFields|NativeSpectrumPacketLease|NativeSpectrumPacketRegistration|pair_spectrum_packet_on_control|pair_actual_packet_on_control)\b', source) and path not in {
            'crates/hex-audio/src/spectrum_worker.rs',
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/sampler_host_fields/native.rs',
            'crates/hex-engine/src/audio_storage/native_spectrum.rs',
            'crates/hex-engine/src/audio_storage/native_spectrum/packet.rs',
            'crates/hex-audio/src/spectrum_worker/project.rs',
            'crates/hex-audio/src/spectrum_worker/project/packet.rs',
            'crates/hex-audio/src/candidate_custody.rs',
            'crates/hex-audio/src/candidate_custody/eligible.rs',
            'crates/hex-audio/src/native_callback_owner.rs',
            'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs'}:
        return [f'{path}: packet custody belongs only to its actual constructor/Root/schema']
    if path == 'crates/hex-audio/src/spectrum_worker/project/packet.rs' and any(
            name not in {'sampler_host_fields', 'BodyRef'} for name in NAMES.findall(source)):
        return [f'{path}: actual packet constructor does not authorize unrelated host producers']
    if re.search(r'\b(?:begin_actual_packet_drop_on_control|complete_actual_packet_drop_on_control|record_actual_packet_failure_on_control)\b', source) and path not in {
            'crates/hex-engine/src/audio_storage/native_spectrum/packet.rs',
            'crates/hex-audio/src/spectrum_worker/project/packet.rs'}:
        return [f'{path}: packet destruction belongs only to the actual affine owner']
    if path == 'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs':
        if not re.search(r'fn spectrum_packets\([^}]+\{\s*&self\.bank\.spectrum_packets\s*\}', source):
            return [f'{path}: packet inventory must borrow the whole actual Root array']
    if path == 'crates/hex-audio/src/spectrum_worker/project/packet.rs':
        start = source.find('impl Drop for PacketOwner'); end = source.find('#[cfg(', start)
        drop = source[start:end if end >= 0 else None]
        first = drop.find('begin_actual_packet_drop_on_control')
        take = drop.find('drop(self.payload.take())')
        last = drop.find('complete_actual_packet_drop_on_control')
        if first < 0 or not first < take < last or re.search(r'\.(?:expect|unwrap)\s*\(', drop):
            return [f'{path}: actual packet destruction requires pending-before-take and normal-complete afterward without new panic']
    if path == 'crates/hex-audio/src/spectrum_worker.rs' and re.search(r'\b(?:NativeSpectrumPacketFields|NativeSpectrumPacketLease|pair_spectrum_packet_on_control|pair_actual_packet_on_control)\b', source):
        return [f'{path}: worker adapter may only borrow the actual packet-record slice']
    if path == 'crates/hex-audio/src/spectrum_worker/project/packet.rs':
        required = '#[cfg(all(test, feature = "allocation-probe"))]\n#[path = "packet/carrier_binding_tests.rs"]\nmod carrier_binding_tests;'
        if required not in source:
            return [f'{path}: packet carrier test finalization is test-only']
    stop_ops = re.search(r'\b(?:try_load_stop_on_control|try_request_stop_on_control|try_value_identity_on_control)\b', source)
    if stop_ops and path not in {'crates/hex-engine/src/audio_storage/native_atomic_signal.rs',
                                'crates/hex-engine/src/audio_storage/native_atomic_signal/tests.rs',
                                'crates/hex-audio/src/spectrum_worker/stop.rs'}:
        return [f'{path}: safe stop-field operations belong only to actual atomic owner/stop adapter']
    spectrum_names = re.search(r'\b(?:SpectrumFloatBoxRef|NativeSpectrumCoreFields|NativeSpectrumProjectCoreFields|native_buffer_fields|native_core_fields|borrow_sampler_spectrum_core_fields_on_control)\b', source)
    spectrum_owned_module = False
    if spectrum_names:
        allowed_spectrum = {
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/sampler_host_fields/spectrum.rs',
            'crates/hex-audio/src/spectrum.rs',
            'crates/hex-audio/src/spectrum_worker.rs',
            'crates/hex-engine/src/audio_storage/native_spectrum.rs',
            'crates/hex-engine/src/audio_storage/native_spectrum/tests.rs',
            'crates/hex-audio/src/spectrum_worker/native.rs',
            'crates/hex-audio/src/spectrum_worker/project.rs',
        }
        if path not in allowed_spectrum:
            return [f'{path}: actual analyzer float fields require the exact owning constructor modules']
        spectrum_owned_module = path in {
            'crates/hex-audio/src/spectrum.rs', 'crates/hex-audio/src/spectrum_worker.rs', 'crates/hex-audio/src/spectrum_worker/native.rs'}
        spectrum_checked = source
        if path == 'crates/hex-audio/src/spectrum_worker/native.rs':
            # ONLY the two existing static constructor quotes, not borrowed metadata producers.
            for quote in [
                'hex_engine::sampler_host_fields::SharedMetadataRef::constructor_bytes::<RingBuffer<T>>()',
                'hex_engine::sampler_host_fields::SharedMetadataRef::constructor_bytes::<AtomicBool>()',
            ]:
                spectrum_checked = spectrum_checked.replace(quote, '', 1)
            # One concrete stack-array size quote does not authorize borrowing
            # or constructing a signal field in this cost-only module.
            spectrum_checked = spectrum_checked.replace(
                'size_of::<[hex_engine::sampler_host_fields::NativeAtomicSignal; 2]>()', '', 1)
        if spectrum_owned_module and any(match.group() not in {'sampler_host_fields', 'BodyRef', 'NativePreparedProducer', 'NativePreparedConsumer', 'native_prepared_ring', 'NativeFixedRingRegistration', 'NativeAtomicSignalRegistration'} for match in NAMES.finditer(spectrum_checked)):
            return [f'{path}: analyzer float-field permission does not enable unrelated host schemas']
    closed_driver = re.search(r'\bconstruct_original_sampler_native_with_host_on_control\b', source)
    if closed_driver and path not in {
        'crates/hex-engine/src/sampler_host_fields.rs',
        'crates/hex-engine/src/sampler_host_fields/original_driver.rs',
        'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
    }:
        return [f'{path}: the closed native constructor has one private Audio consumer']
    native_driver = re.search(r'\b(?:OriginalSamplerNativeHost|selected_preparation_on_control|selected_operations_on_control)\b', source)
    if native_driver:
        permitted = {
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/sampler_host_fields/original_driver.rs',
            'crates/hex-engine/src/sampler_host_fields/pending_session.rs',
            'crates/hex-engine/src/sampler_host_fields/native.rs',
            'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs',
            'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
            'crates/hex-audio/src/candidate_custody/eligible/publication/native_host/tests.rs',
        }
        if path not in permitted:
            return [f'{path}: selected-parent driver operations belong only to closed Engine/private Audio']
        implementations = re.findall(r'\bimpl(?:<[^>]*>)?\s+(?:[\w:]+::)?OriginalSamplerNativeHost\s+for\s+(\w+)', source)
        if implementations and (path != 'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs' or implementations != ['NativePreparationHost']):
            return [f'{path}: only the actual private guarded Audio facade implements the driver']
    private_driver_names = re.search(r'\b(?:inspect_original_sampler_revision_with_writers_on_control|construct_original_sampler_revision_with_writers_on_control)\b', source)
    if private_driver_names and path not in {
            'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile/complete.rs',
            'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs'}:
        return [f'{path}: private original-Sampler inspect/construct stages cannot escape the closed Engine driver']
    candidate_stage_names = re.search(r'\b(?:OriginalSamplerConstruction|inspect_original_sampler_with_writers_on_control|construct_original_sampler_with_writers_on_control|original_sampler_compiler_reservation|finish_original_sampler_runtime_on_control)\b', source)
    if candidate_stage_names and path not in {
            'crates/hex-engine/src/audio_preparation.rs',
            'crates/hex-engine/src/audio_preparation/arrangement_clips.rs',
            'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs',
            'crates/hex-engine/src/audio_candidate/preparation/original_resources/tests/runtime.rs',
            'crates/hex-engine/src/sampler_host_fields/original_driver.rs',
            'crates/hex-engine/src/audio_candidate/preparation.rs',
            'crates/hex-engine/src/audio_candidate/preparation_operations.rs',
    }:
        return [f'{path}: private candidate construction stages cannot become an external plan/approval API']
    seal_stage_names = re.search(r'\b(?:OriginalSamplerSealInspection|inspect_sampler_constructor_with_inventory_on_control|install_sampler_constructor_inspection_on_control)\b', source)
    if seal_stage_names and path not in {
            'crates/hex-engine/src/audio_candidate/preparation_operations.rs',
            'crates/hex-engine/src/audio_candidate/storage.rs',
            'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs'}:
        return [f'{path}: private sampler seal inspection cannot escape the closed Engine constructor']
    resident_names = re.search(r'\b(?:OriginalSamplerResidentFields|OriginalSamplerResidentView|ResidentAssetFields|ResidentClipWaveformFields|ResidentOriginalFields|resident_fields_and_writer_on_control|visit_original_sampler_resident)\b', source)
    if resident_names:
        resident_paths = {
            'crates/hex-desktop/src/native_audio/tests/closed_original.rs',
            'crates/hex-audio/src/prepared/worker_publication.rs',
            'crates/hex-engine/src/native_device_resources/producer.rs',
            'crates/hex-session/src/lib.rs', 'crates/hex-session/src/resource_plan.rs',
            'crates/hex-session/src/resource_plan/original_sampler.rs',
            'crates/hex-session/src/resource_plan/tests.rs',
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/audio_candidate/storage.rs',
            'crates/hex-engine/src/audio_candidate/preparation_operations.rs',
            'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs',
            'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile.rs',
            'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile/complete.rs',
            'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
        }
        if path not in resident_paths and not path.startswith('crates/hex-engine/src/sampler_host_fields/'):
            return [f'{path}: original-Sampler resident fields require the audited owning Session view']
        impls = re.findall(r'\bimpl(?:<[^>]*>)?\s+(?:[\w:]+::)?OriginalSamplerResidentFields\s+for\s+(\w+)', source)
        if impls and (path != 'crates/hex-session/src/resource_plan/original_sampler.rs' or impls != ['OriginalSamplerResidentView']):
            return [f'{path}: exactly one actual Session producer implements resident fields']
    lending_names = re.search(r'\b(?:OriginalSamplerTransaction|OriginalSamplerSessionLoan|original_sampler_transaction_on_control|original_sampler_session_loan_on_control|original_sampler_writer_on_control|compile_original_sampler_lent_on_control)\b', source)
    if lending_names:
        lending_paths = {
            'crates/hex-desktop/src/native_audio/tests/closed_original.rs',
            'crates/hex-audio/src/prepared/worker_publication.rs',
            'crates/hex-session/src/service/worker_roots.rs',
            'crates/hex-engine/src/sampler_host_fields/original_driver.rs',
            'crates/hex-engine/src/sampler_host_fields/pending_session.rs',
            'crates/hex-engine/src/audio_transform/mod.rs',
            'crates/hex-engine/src/audio_transform/original_sampler.rs',
            'crates/hex-engine/src/audio_candidate/preparation_operations.rs',
            'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs',
            'crates/hex-engine/src/audio_candidate/preparation/original_resources/tests/runtime.rs',
            'crates/hex-engine/src/audio_candidate/storage.rs',
            'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile/complete.rs',
            'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile.rs',
            'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile/tests/captured_target.rs',
            'crates/hex-session/src/lib.rs',
            'crates/hex-session/src/resource_plan.rs',
            'crates/hex-session/src/service/worker_startup.rs',  # borrower only; no transaction producer
            'crates/hex-session/src/resource_plan/original_sampler.rs',
            'crates/hex-session/src/resource_plan/original_sampler/tests.rs',
            'crates/hex-session/src/resource_plan/tests.rs',
            'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
        }
        if path not in lending_paths:
            return [f'{path}: original-Sampler writers have one Session producer and private Audio consumer']
        if re.search(r'impl(?:<[^>]*>)?\s+.*\bOriginalSamplerTransaction\b', source) and path != 'crates/hex-engine/src/audio_transform/original_sampler.rs':
            return [f'{path}: only Engine implements the original-Sampler transaction']
        if path == 'crates/hex-session/src/resource_plan/original_sampler.rs':
            if source.count('.original_sampler_transaction_on_control(') != 1 or source.count('pub fn original_sampler_session_loan_on_control(') != 1:
                return [f'{path}: original-Sampler acquisition must remain the single audited Session producer']
    event_projection = re.search(r'\b(?:EventMetadataFields|EventReasonFields|EventBatchFailureFields|TuningLibraryMetadataPayload|TuningSlotMetadataFields)\b', source)
    if event_projection and not (path.startswith('crates/hex-engine/src/sampler_host_fields/') or path in {
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-session/src/service/event_fields.rs',
            'crates/hex-session/src/snapshot_exports/source_event.rs'}):
        return [f'{path}: Event metadata projection belongs to the audited owning adapter and Engine schema']
    if 'event_metadata_fields_on_control' in source and path not in {
            'crates/hex-session/src/service/event_fields.rs',
            'crates/hex-session/src/service/worker_roots.rs'}:
        return [f'{path}: Event metadata accessor belongs to the actual Event/worker borrow']
    token_accessors = re.search(r'\b(?:FileTokenStorageObservation|file_token_storage_on_control|borrowed_file_token_arc_field_on_control|document_storage_fields_on_control)\b', source)
    if token_accessors and path not in {
            'crates/hex-storage/src/files.rs', 'crates/hex-storage/src/lib.rs',
            'crates/hex-session/src/service/worker_roots.rs'}:
        return [f'{path}: FileToken borrowed access belongs only to Storage and ONE Session document producer']
    if path == 'crates/hex-session/src/service/worker_roots.rs':
        start = source.find('fn document_storage_fields_on_control(')
        end = source.find('\n}\n', start)
        bridge = re.sub(r'\s+', '', source[start:end]) if start >= 0 and end > start else ''
        required = ['letSelf{path,token}=self;',
                    'letobservation=token.file_token_storage_on_control();',
                    'body:BodyRef::of(self),', 'body:BodyRef::of(token),',
                    'contents:observation.borrowed_file_token_arc_field_on_control(),']
        if any(item not in bridge for item in required) or any(item in bridge for item in ['.clone(', '.take(', '.as_ptr(', 'Arc::']):
            return [f'{path}: actual immutable DocumentHandle/token fields must feed the audited bridge without owning conversion']
    # Closed Storage token constructors. Read-only observation adds no owning
    # getter/constructor; new ingress needs an explicit constructor audit.
    constructors = re.findall(r'\bFileToken\s*\(', source)
    if constructors and path not in {
            'crates/hex-storage/src/files.rs',
            'crates/hex-storage/src/replacement.rs'}:
        return [f'{path}: FileToken constructors/destructuring belong only to Storage owners']
    if path == 'crates/hex-storage/src/files.rs':
        compact = re.sub(r'\s+', '', source)
        required = ['pubstructFileToken(pub(super)Arc<[u8]>);',
                    'FileToken(bytes.into())', 'FileToken(Arc::from(bytes))',
                    'letFileToken(owner)=self.token;']
        if len(constructors) != 4 or any(value not in compact for value in required):
            return [f'{path}: sealed FileToken definition/two births/borrowed-field match changed']
    if path == 'crates/hex-storage/src/replacement.rs':
        compact = re.sub(r'\s+', '', source)
        if len(constructors) != 2 or any(value not in compact for value in [
                'FileToken(Arc::from(new_bytes))', 'FileToken(Arc::from(payload))']):
            return [f'{path}: sealed replacement/recovery FileToken births changed']
    final = 'finish_root_after_native_fields_on_control'
    if final in source:
        if path == 'crates/hex-audio/src/lifecycle.rs':
            begin = source.find('fn construct_prepared_channel_on_control(')
            stop = source.find('\n    pub fn new(', begin)
            constructor = re.sub(r'\s+', '', source[begin:stop])
            required = ['::preflight_native_worker_birth(', '::channel_with_factory_reservation(',
                        'Self::begin_prepared_custody(', 'parent.construct_spectrum_on_control(']
            positions = [constructor.find(item) for item in required]
            if any(position < 0 for position in positions) or positions != sorted(positions):
                return [f'{path}: combined fixed preflight/Root/parent must precede analyzer birth']
            start = source.find('fn advance_worker_then_final_native(')
            end = source.find('// One matched retirement bundle', start)
            driver = source[start:end]
            compact = re.sub(r'\s+', '', driver)
            if (source.count(final) != 1 or driver.count(final) != 1
                    or compact.find('reaper.advance_shutdown_on_control()') < 0
                    or compact.find('reaper.advance_shutdown_on_control()') > compact.find(final)):
                return [f'{path}: final native Root completion requires the owning parent actual Reaper join']
        elif path != 'crates/hex-audio/src/native_callback_owner.rs':
            return [f'{path}: final native Root completion belongs only to the lifecycle owning parent']
    if path == 'crates/hex-audio/src/spectrum_worker/native.rs':
        # Static fresh constructor quotes ONLY; this does not add another
        # borrowed producer/adoption implementation to the audited schema.
        names = set(NAMES.findall(spectrum_checked))
        if (names - {'sampler_host_fields', 'BodyRef', 'NativePreparedProducer', 'NativePreparedConsumer', 'NativeFixedRingRegistration', 'NativeAtomicSignalRegistration'}
                or re.findall(r'SharedMetadataRef\s*::\s*(\w+)', source) != ['constructor_bytes', 'constructor_bytes']):
            return [f'{path}: spectrum may only use its two fresh typed constructor layout quotes']
        return []
    if path == 'crates/hex-audio/src/native_callback_owner.rs':
        start = source.find('pub(crate) fn reserve_native_worker(')
        end = source.find('fn reserve_carriers(', start)
        worker = source[start:end] if start >= 0 and end > start else ''
        required = ['registered::validate_birth_limits(&root)?;',
                    '!control.matches_fresh_registered_payload_on_control(&root)',
                    '!callback.matches_fresh_registered_payload_on_control(&root)',
                    'control.reserve_worker_endpoints(reader)?']
        positions = [worker.find(text) for text in required]
        if any(position < 0 for position in positions) or positions != sorted(positions):
            return [f'{path}: both actual prepared queue inputs must be checked before ANY carrier birth']
    carrier_module = 'crates/hex-audio/src/native_callback_owner/registered.rs'
    if re.search(r'\bCarrierPayload\b', source):
        if path != carrier_module:
            return [f'{path}: carrier construction trait belongs only to its four concrete producers']
        types = re.findall(r'impl CarrierPayload for (\w+)', source)
        if types != ['NativeCallbackOwner', 'ErrorCallbackOwner', 'ClientEndpoints', 'PublicationControl']:
            return [f'{path}: carrier construction requires exactly four concrete original types']
        required = ['self.matches_fresh_registered_payload_on_control(root)',
                    'root.matches_status_field(&self.status, crate::prepared::status_owner::NATIVE_DATA)',
                    'root.matches_status_field(&self.status, crate::prepared::status_owner::NATIVE_ERROR)']
        if any(text not in source for text in required):
            return [f'{path}: actual worker Resident and native Status field roles must be checked before carrier birth']
    if not NAMES.search(source):
        return []
    if path in EXACT or path.startswith(PREFIXES) or spectrum_owned_module:
        return []
    return [f'{path}: sampler-host projections require a declared audited owning module']


def repository_violations(root):
    from custody_mutex_policy import repository_violations as custody_mutex_violations
    result = custody_mutex_violations(root)
    for path in sorted((root / 'crates').rglob('*.rs')):
        result.extend(violations(path.relative_to(root).as_posix(), path.read_text()))
    return result
