"""Audited sampler-host producer boundary, not Rust hostile-client sealing.

These public internal borrowed projections are only constructed/implemented in
reviewed owning modules. Engine, rather than a producer callback, traverses the
finite schema. This check preserves the independent Cargo dependency checks.
"""
import re


EXACT = {
    'crates/hex-audio/src/candidate_custody.rs',
    'crates/hex-audio/src/candidate_custody/eligible/pages.rs',
    'crates/hex-audio/src/candidate_custody/eligible/native_birth_tests.rs',
    'crates/hex-audio/src/candidate_custody/eligible/publication/tests.rs',
    'crates/hex-audio/src/candidate_custody/eligible/publication.rs',
    'crates/hex-session/src/service/worker_roots.rs',
    'crates/hex-project/src/time_range.rs',
    'crates/hex-engine/src/lib.rs',
    'crates/hex-engine/src/midi_preview.rs',
    'crates/hex-engine/src/gain_envelope.rs',
    'crates/hex-session/src/snapshot_exports/storage.rs',
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
    'crates/hex-session/src/service/main_control_file/tests.rs',
}
# Whole subtrees are restricted to the new Engine schema itself, not arbitrary
# modules in an approved crate. New production producers require explicit review.
PREFIXES = ('crates/hex-engine/src/sampler_host_fields/',)
NAMES = re.compile(r'\b(?:borrow_sampler_worker_fields_on_control|track_buffer_on_control|FamilySnapshotFields|FamilyCompiledRecordFields|FamilyCompiledFields|FamilyRequestPayload|FamilyRequestFields|FamilyRequestRecords|borrow_sampler_family_fields_on_control|borrow_sampler_family_requests_on_control|borrow_sampler_family_compiled_on_control|SnapshotViewFields|SnapshotViewPayload|CapturedSourcePayload|borrow_sampler_snapshot_children_fields_on_control|borrow_sampler_snapshot_view_fields_on_control|WaveformFields|WaveformPayload|WaveformLevelsRef|WaveformSamples|borrow_sampler_waveform_fields_on_control|sampler_host_fields|BodyRef|VecRef|SharedMetadataRef|SamplerWorkerFields|CapturedSourceFields|FamilyFields|FileSlotsFields|NativeFields|sampler_fields_on_control|borrow_sampler_file_slots_fields_on_control|borrow_sampler_captured_source_fields_on_control)\b')


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
    if not NAMES.search(source):
        return []
    if path in EXACT or path.startswith(PREFIXES):
        return []
    return [f'{path}: sampler-host projections require a declared audited owning module']


def repository_violations(root):
    from custody_mutex_policy import repository_violations as custody_mutex_violations
    result = custody_mutex_violations(root)
    for path in sorted((root / 'crates').rglob('*.rs')):
        result.extend(violations(path.relative_to(root).as_posix(), path.read_text()))
    return result
