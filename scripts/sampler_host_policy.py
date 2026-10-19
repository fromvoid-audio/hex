"""Audited sampler-host producer boundary, not Rust hostile-client sealing.

These public internal borrowed projections are only constructed/implemented in
reviewed owning modules. Engine, rather than a producer callback, traverses the
finite schema. This check preserves the independent Cargo dependency checks.
"""
import re

EXACT = {
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
    'crates/hex-session/src/service/main_control_file/tests.rs',
}
# Whole subtrees are restricted to the new Engine schema itself, not arbitrary
# modules in an approved crate. New production producers require explicit review.
PREFIXES = ('crates/hex-engine/src/sampler_host_fields/',)
NAMES = re.compile(r'\b(?:borrow_sampler_worker_fields_on_control|track_buffer_on_control|FamilySnapshotFields|FamilyCompiledRecordFields|FamilyCompiledFields|FamilyRequestPayload|FamilyRequestFields|FamilyRequestRecords|borrow_sampler_family_fields_on_control|borrow_sampler_family_requests_on_control|borrow_sampler_family_compiled_on_control|SnapshotViewFields|SnapshotViewPayload|CapturedSourcePayload|borrow_sampler_snapshot_children_fields_on_control|borrow_sampler_snapshot_view_fields_on_control|WaveformFields|WaveformPayload|WaveformLevelsRef|WaveformSamples|borrow_sampler_waveform_fields_on_control|sampler_host_fields|BodyRef|VecRef|SharedMetadataRef|SamplerWorkerFields|CapturedSourceFields|FamilyFields|FileSlotsFields|NativeFields|sampler_fields_on_control|borrow_sampler_file_slots_fields_on_control|borrow_sampler_captured_source_fields_on_control)\b')


def violations(path: str, source: str) -> list[str]:
    if not NAMES.search(source):
        return []
    if path in EXACT or path.startswith(PREFIXES):
        return []
    return [f'{path}: sampler-host projections require a declared audited owning module']


def repository_violations(root):
    result = []
    for path in sorted((root / 'crates').rglob('*.rs')):
        result.extend(violations(path.relative_to(root).as_posix(), path.read_text()))
    return result
