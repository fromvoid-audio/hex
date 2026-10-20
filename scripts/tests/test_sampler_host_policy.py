"""Finite source-boundary sensitivity; no compiler/admission claim."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    'sampler_host_policy', Path(__file__).resolve().parents[1] / 'sampler_host_policy.py')
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)
violations = policy.violations


class SamplerHostPolicyTests(unittest.TestCase):
    def test_ordered_source_query_keeps_actual_binding_and_all_owning_fields(self):
        root = Path(__file__).resolve().parents[2]
        path = 'crates/hex-session/src/service/comparison_audio.rs'
        source = (root / path).read_text()
        self.assertEqual(violations(path, source), [])
        for step in ['source: SharedCanonicalAudioSource', 'previous.revision.checked_add(1)',
                     'source.matches_session_sources(session)', 'source: source.clone()']:
            self.assertTrue(violations(path, source.replace(step, 'omitted()')), step)
        self.assertTrue(violations('crates/hex-ui/src/foreign.rs', 'ComparisonAudioBinding::advance()'))
        path = 'crates/hex-engine/src/sampler_host_fields/snapshot_view.rs'
        source = (root / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source.replace('payload.comparison_source()', 'None')))
        path = 'crates/hex-session/src/snapshot_exports/transport.rs'
        source = (root / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source.replace('self.route != previous.route', 'false')))

    def test_sampler_copy_controls_do_not_adopt_state_or_open_registry(self):
        path = 'crates/hex-engine/src/native_device_view/sampler_parameters.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        for step in ['DeviceStateRead::from_native(state)?', 'descriptor.validate(current)?',
                     'semantics.validate(*descriptor)?']:
            self.assertTrue(violations(path, source.replace(step, 'omitted()')), step)
        for forbidden in ['native_instrument_registry(', 'state.clone()', 'Vec::new()']:
            self.assertTrue(violations(path, source.replace('#[cfg(test)]', forbidden + '\n#[cfg(test)]', 1)))
        self.assertTrue(violations('crates/hex-ui/src/foreign.rs', 'sampler_parameters_on_control(&owned_state)'))
        fixture = 'crates/hex-ui/src/editor/device_panel/sampler_ui_tests.rs'
        self.assertEqual(violations(fixture, (Path(__file__).resolve().parents[2] / fixture).read_text()), [])
        mount = (Path(__file__).resolve().parents[2] / 'crates/hex-ui/src/editor/device_panel.rs').read_text()
        self.assertIn('#[cfg(all(test, feature = "ui-test-support"))]\n#[path = "device_panel/sampler_ui_tests.rs"]\nmod sampler_ui_tests;', mount)
        self.assertEqual(violations('crates/hex-ui/src/editor/device_panel.rs', mount), [])
        self.assertTrue(violations('crates/hex-ui/src/editor/device_panel.rs', mount.replace(
            '#[cfg(all(test, feature = "ui-test-support"))]\n#[path = "device_panel/sampler_ui_tests.rs"]',
            '#[path = "device_panel/sampler_ui_tests.rs"]')))

    def test_initial_startup_purpose_is_actual_once_only_and_not_edit_authority(self):
        root = Path(__file__).resolve().parents[2]
        path = 'crates/hex-session/src/service/worker_startup.rs'
        source = (root / path).read_text()
        self.assertEqual(violations(path, source), [])
        for step in ['*self.phase != InitialSnapshotPhase::Fresh',
                     'self.last_source.is_some()', 'self.cancellation.is_cancelled()',
                     '*self.phase = InitialSnapshotPhase::Entered;']:
            self.assertTrue(violations(path, source.replace(step, 'omitted()')), step)
        self.assertTrue(violations(path, source.replace('#[cfg(test)]\nmod tests {', 'mod tests {')))
        session = 'crates/hex-session/src/resource_plan/original_sampler.rs'
        current = (root / session).read_text()
        self.assertEqual(violations(session, current), [])
        self.assertTrue(violations(session, current.replace(
            'source.purpose != Some(PendingSourcePurpose::Edit)', 'false')))
        for foreign in ['crates/hex-ui/src/foreign.rs', 'crates/hex-session/src/foreign.rs']:
            self.assertTrue(violations(foreign, 'WorkerInitialSessionView'))
            self.assertTrue(violations(foreign, 'x.original_sampler_initial_writers_on_control()'))

    def test_source_event_queue_is_fixed_and_reserves_fanout_before_move(self):
        path = 'crates/hex-session/src/snapshot_exports/source_queue.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        for required in ['self.collect_releases_on_control()?',
                         'self.derive_read(&entry.canonical)?',
                         'self.source_event_head()? != Some(head)']:
            self.assertTrue(violations(path, source.replace(required, 'omitted()')))
        for forbidden in ['Arc::new(owned)', 'Vec::new()', 'mpsc::channel()']:
            self.assertTrue(violations(path, source.replace('#[cfg(test)]', forbidden + '\n#[cfg(test)]', 1)))
        self.assertTrue(violations('crates/hex-ui/src/foreign.rs',
            'SnapshotSourceEventReceiver::new_unqualified_on_control(route, ops, reads)'))

    def test_event_constructor_uses_same_actual_family_writer_without_birth_on_acquisition(self):
        path = 'crates/hex-session/src/snapshot_exports/family.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        consumer = 'crates/hex-session/src/snapshot_exports/event/tests.rs'
        self.assertEqual(violations(consumer,
            (Path(__file__).resolve().parents[2] / consumer).read_text()), [])
        for required in ['self.validate_producer_on_control(key)?',
                         'self.records.try_write()', 'records.event_destination_on_control()?']:
            self.assertTrue(violations(path, source.replace(required, 'omitted')))
        self.assertTrue(violations(path, source.replace(
            'let destination = records.event_destination_on_control()?;',
            'let destination = records.event_destination_on_control()?; owner.clone();')))
        for foreign in ['crates/hex-ui/src/foreign.rs', 'crates/hex-session/src/service.rs']:
            self.assertTrue(violations(foreign, 'SnapshotEventConstruction {}'))
            self.assertTrue(violations(foreign, 'x.event_destination_on_control()'))

    def test_inline_gestures_and_actual_legacy_input_presence_have_exact_producers(self):
        root = Path(__file__).resolve().parents[2]
        for leaf, checked in [('device_gesture.rs', ['inline_copy(snapshot);', 'inline_copy(kind);']),
                              ('tuning_gesture.rs', ['inline_copy(active);', 'inline_copy(initial);'])]:
            path = 'crates/hex-session/src/' + leaf
            source = (root / path).read_text()
            self.assertEqual(violations(path, source), [])
            for required in checked + ['inline_copy(last_key);', 'BodyRef::of(self)']:
                self.assertTrue(violations(path, source.replace(required, 'omitted')))
            self.assertTrue(violations(path, source + '\nfn arbitrary() { NativeFields; }'))
        path = 'crates/hex-session/src/service/worker_roots.rs'
        source = (root / path).read_text()
        self.assertEqual(violations(path, source), [])
        for field in ['clipboard', 'keyboard', 'keyboard_hint', 'audition', 'expression']:
            self.assertTrue(violations(path, source.replace(
                f'{field}: {field}.as_ref().map(BodyRef::of)', f'{field}: None')))
        for path in ['crates/hex-ui/src/foreign.rs', 'crates/hex-session/src/service.rs']:
            self.assertTrue(violations(path, 'WorkerLegacyInputFields {}'))
            self.assertTrue(violations(path, 'foreign.inline_worker_gesture_frame_on_control()'))

    def test_pending_session_producer_keeps_both_prefix_passes_and_actual_source_field(self):
        path = 'crates/hex-engine/src/sampler_host_fields/pending_session.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        for required in ['worker.require_current_constructor_owner()?',
                         'actual_source.owner.body().same(current_source.owner.body())',
                         'arithmetic_cost_for_limits(',
                         'size_of::<crate::audio_transform::OriginalSamplerTransaction',
                         'ledger.validate_selected_shared_handles_on_control()?',
                         'ledger.metadata_capacity_bytes()',
                         'session.source_and_writer_on_control()?']:
            self.assertTrue(violations(path, source.replace(required, 'omitted')))
        for foreign in ['crates/hex-ui/src/arbitrary.rs', 'crates/hex-session/src/service.rs',
                        'crates/hex-audio/src/prepared.rs']:
            self.assertTrue(violations(foreign, 'x.stage_and_capture_on_control()?'))
            self.assertTrue(violations(foreign, 'impl OriginalSamplerPendingSession for Arbitrary {}'))
            self.assertTrue(violations(foreign, 'T::from_pending_original_on_control()'))

    def test_pending_model_reader_uses_actual_guard_and_keeps_unknown_children(self):
        path = 'crates/hex-engine/src/audio_storage/original_sampler_pending.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        for omitted in ['pending.undo_replacement()', 'pending.retired_redo()',
                        'pending.request()', 'ledger.project_native_states(project)?',
                        '_ => return Err(AudioStorageError::UnknownStorage)']:
            self.assertTrue(violations(path, source.replace(omitted, 'omitted')))
        self.assertTrue(violations('crates/hex-session/src/service.rs',
            'ledger.borrow_original_sampler_pending_model_fields_on_control(transition)'))

    def test_closed_driver_and_nominal_reexport_have_exact_callers(self):
        name = 'construct_original_sampler_native_with_host_on_control'
        for path in [
            'crates/hex-engine/src/sampler_host_fields.rs',
            'crates/hex-engine/src/sampler_host_fields/original_driver.rs',
            'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
        ]:
            if path.endswith('original_driver.rs') or path.endswith('native_host.rs'):
                source = (Path(__file__).resolve().parents[2] / path).read_text()
            else:
                source = name
            self.assertEqual(violations(path, source), [])
        for path in ['crates/hex-ui/src/arbitrary.rs',
                     'crates/hex-session/src/service.rs',
                     'crates/hex-audio/src/prepared.rs']:
            self.assertTrue(violations(path, name))
        reexport = '    sampler_fields_on_control as sampler_captured_source_fields_on_control,\n'
        self.assertEqual(violations('crates/hex-session/src/service.rs', reexport), [])
        self.assertTrue(violations('crates/hex-session/src/service.rs',
                                  reexport + 'BodyRef::of(&foreign)'))

    def test_closed_construction_host_keeps_all_real_operands(self):
        root = Path(__file__).resolve().parents[2]
        path = 'crates/hex-engine/src/sampler_host_fields/original_driver.rs'
        actual = (root / path).read_text()
        self.assertEqual(violations(path, actual), [])
        for step in ['self.source.payload.project()', 'self.files.borrowed_frame()',
                     'super::worker::visit_worker(self.worker, ledger)?',
                     'super::captured_source::visit_captured_source(self.source, ledger)?',
                     'super::file_slots::visit_file_slots(self.files, ledger)?',
                     'super::family_inventory::visit_family_inventory(self.families, ledger)?',
                     'super::native::visit_native_for_original_sampler']:
            self.assertTrue(violations(path, actual.replace(step, 'omitted()')))
        for path in ['crates/hex-engine/src/foreign.rs', 'crates/hex-session/src/foreign.rs', 'crates/hex-audio/src/foreign.rs']:
            self.assertTrue(violations(path, 'OriginalSamplerConstructionHost'))
        native = 'crates/hex-engine/src/sampler_host_fields/native.rs'
        actual = (root / native).read_text()
        self.assertEqual(violations(native, actual), [])
        self.assertTrue(violations(native, actual + '\nimpl OriginalSamplerNativeHost for Foreign {}'))

    def test_owned_selected_cell_requires_real_destination_and_native_fresh_graph(self):
        root = Path(__file__).resolve().parents[2]
        for path, steps in {
            'crates/hex-engine/src/audio_candidate/preparation/native_fields.rs': ['.visit_transferred_host_shell(ledger, owner)?', 'owner.visit_unpublished_native_host_fields(ledger)'],
            'crates/hex-engine/src/audio_candidate/draft.rs': ['*phase != Phase::Transferred', '*identity != Some(owner.identity())'],
            'crates/hex-engine/src/audio_candidate/storage.rs': ['inner.visit_fresh_storage(ledger)', 'runtime.playback.is_main_control_runtime()'],
        }.items():
            actual = (root / path).read_text()
            self.assertEqual(violations(path, actual), [])
            for step in steps:
                self.assertTrue(violations(path, actual.replace(step, 'omitted()')))
        for path in ['crates/hex-audio/src/foreign.rs', 'crates/hex-session/src/foreign.rs']:
            for name in ['visit_unpublished_native_host_fields', 'visit_transferred_host_shell']:
                self.assertTrue(violations(path, name))

    def test_complete_family_traversal_keeps_actual_owners_and_private_leaves(self):
        root = Path(__file__).resolve().parents[2]
        path = 'crates/hex-engine/src/sampler_host_fields/family_inventory.rs'
        actual = (root / path).read_text()
        self.assertEqual(violations(path, actual), [])
        for step in ['job.matches_actual_payload_body(family.owner)',
                     'job.visit_outer_fields_on_control(ledger)?',
                     'slot.cancellation.visit_family_field_on_control(ledger)?',
                     'super::family::visit_family(family.records, ledger)?',
                     'super::family::visit_export_result(family.retiring_export, ledger)?',
                     'inner_index != inner_guards.len()']:
            self.assertTrue(violations(path, actual.replace(step, 'omitted()')))
        for path, trait in [
                ('crates/hex-audio/src/lifecycle/retained_families/inventory.rs', 'SamplerOuterFamilyFields'),
                ('crates/hex-desktop/src/snapshot_transport/inventory.rs', 'SamplerFamilyInventoryFields')]:
            actual = (root / path).read_text()
            self.assertEqual(violations(path, actual), [])
            self.assertTrue(violations(path, actual + '\nimpl ' + trait + ' for Foreign {}'))
        for path in ['crates/hex-audio/src/foreign.rs', 'crates/hex-session/src/foreign.rs', 'crates/hex-desktop/src/foreign.rs']:
            for name in ['SamplerFamilyInventoryFields', 'SamplerOuterFamilyFields', 'matches_actual_payload_body', 'visit_family_field_on_control']:
                self.assertTrue(violations(path, name))

    def test_registered_native_payload_birth_keeps_all_owning_destinations(self):
        root = Path(__file__).resolve().parents[2]
        required = {
            'crates/hex-audio/src/native_callback_owner/registered.rs': ['.matches_fresh_registered_payload_on_control(root)'],
            'crates/hex-audio/src/sample_preview.rs': ['pending.is_none()', 'active.is_none()'],
            'crates/hex-audio/src/prepared/endpoints.rs': ['sample_preview.matches_fresh_registered_payload_on_control(root)', '*command_ticket == 0'],
            'crates/hex-audio/src/prepared.rs': ['held_control_return.is_none()', 'latest_fresh_runtime_cancellation.is_none()', 'domain_pages.iter().all(Option::is_none)', 'deferred_eligible.is_none()'],
        }
        for path, steps in required.items():
            actual = (root / path).read_text()
            self.assertEqual(violations(path, actual), [])
            for step in steps:
                self.assertTrue(violations(path, actual.replace(step, 'omitted()')), (path, step))

    def test_occupied_runtime_requires_actual_state_and_retained_graph(self):
        root = Path(__file__).resolve().parents[2]
        path = 'crates/hex-engine/src/sampler_host_fields/native.rs'
        actual = (root / path).read_text()
        self.assertEqual(violations(path, actual), [])
        for step in ['transferred.state.is_live_on_control()',
                     'transferred.state.visit_fields(ledger)?',
                     'transferred.anchors.visit_transferred_graph_fields(ledger)?']:
            self.assertTrue(violations(path, actual.replace(step, 'omitted()')))
        for path in ['crates/hex-session/src/foreign.rs',
                     'crates/hex-audio/src/prepared.rs',
                     'crates/hex-engine/src/foreign.rs']:
            self.assertTrue(violations(path, 'NativeTransferredRuntimeFields'))

    def test_result_owner_and_ui_fixture_are_finite_and_nonclone(self):
        root = Path(__file__).resolve().parents[2]
        path = 'crates/hex-audio/src/spectrum_worker/project/result.rs'
        actual = (root / path).read_text()
        self.assertEqual(violations(path, actual), [])
        self.assertTrue(violations(path, actual.replace('drop(payload)', 'drop(None::<()>)')))
        self.assertTrue(violations(path, actual + '\n#[derive(Clone)] struct Fake;'))
        self.assertTrue(violations(path, actual + '\npub frames: Box<[u8; 8]>'))
        self.assertTrue(violations(path, actual.replace('#[cfg(feature = "allocation-probe")]', '')))
        self.assertTrue(violations(path, actual.replace('#[cfg(all(test, feature = "allocation-probe"))]', '')))
        self.assertTrue(violations(path, actual + '\nSharedMetadataRef::of(&foreign)'))
        for other in ['crates/hex-ui/src/foreign.rs', 'crates/hex-session/src/foreign.rs', 'crates/hex-audio/src/prepared.rs']:
            self.assertTrue(violations(other, 'NativeSpectrumResultLease'))
            self.assertTrue(violations(other, 'unqualified_for_ui_test'))
        path = 'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs'
        actual = (root / path).read_text()
        self.assertTrue(violations(path, actual.replace('&self.bank.spectrum_results', '&self.bank.spectrum_results[..1]')))

    def test_float_box_schema_has_exact_analyzer_producers_without_other_host_authority(self):
        for path in ['crates/hex-audio/src/spectrum.rs', 'crates/hex-audio/src/spectrum_worker/native.rs']:
            source = (Path(__file__).resolve().parents[2] / path).read_text()
            self.assertEqual(violations(path, source), [])
            for other in ['NativeFields', 'FamilyFields', 'SharedMetadataRef', 'OriginalSamplerNativeHost']:
                self.assertTrue(violations(path, source + '\nuse hex_engine::sampler_host_fields::{' + other + ', NativeSpectrumCoreFields};'))
        path = 'crates/hex-audio/src/spectrum_worker/native.rs'
        actual = (Path(__file__).resolve().parents[2] / path).read_text()
        for expression in ['SharedMetadataRef::of(&arbitrary)',
                           'SharedMetadataRef::constructor_bytes::<Foreign>()',
                           'hex_engine::sampler_host_fields::SharedMetadataRef::constructor_bytes::<AtomicBool>()']:
            self.assertTrue(violations(path, actual + '\n' + expression))
        for path in ['crates/hex-ui/src/foreign.rs', 'crates/hex-audio/src/prepared.rs', 'crates/hex-engine/src/foreign.rs']:
            self.assertTrue(violations(path, 'SpectrumFloatBoxRef'))

    def test_captured_spectrum_authority_is_exact_and_join_and_drop_order_is_mandatory(self):
        root = Path(__file__).resolve().parents[2]
        worker_path = 'crates/hex-audio/src/spectrum_worker.rs'
        worker = (root / worker_path).read_text()
        self.assertEqual(violations(worker_path, worker), [])
        for original, changed in [
                ('drop(payload)', 'drop(None::<()>)'),
                ('ReaperPhase::Complete || self.join.is_some()', 'ReaperPhase::Ready || self.join.is_some()'),
                ('drop(self.captured_join.take())', 'drop(None::<()>)'),
                ('match join.join()', 'match Ok::<(), Box<dyn std::any::Any + Send>>(())')]:
            self.assertTrue(violations(worker_path, worker.replace(original, changed)))
        self.assertTrue(violations(worker_path, worker.replace('captured.record_capture_failure();', 'captured.payload.as_ref().expect("new runtime panic");')))
        self.assertTrue(violations(worker_path, worker.replace('drop(payload);', 'drop(payload); self.lease.as_ref().expect("destructor panic");')))
        path = 'crates/hex-audio/src/lifecycle.rs'
        actual = (root / path).read_text()
        self.assertEqual(violations(path, actual), [])
        self.assertTrue(violations(path, actual.replace('.finalize_captured_frame_on_control(native)', '.omitted(native)')))
        for path in ['crates/hex-audio/src/random.rs', 'crates/hex-session/src/resource_plan/original_sampler.rs',
                     'crates/hex-engine/src/sampler_host_fields/foreign.rs']:
            for name in ['NativeSpectrumJoinLease', 'pair_actual_capture_on_control', 'finalize_actual_join_on_control', 'spawn_worker_with_parent']:
                self.assertTrue(violations(path, name))
        self.assertTrue(violations('crates/hex-audio/src/spectrum_worker/native.rs', 'finalize_actual_join_on_control'))
        self.assertTrue(violations('crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs', 'x.finish_spectrum_capture_on_control()'))

    def test_spectrum_endpoint_fields_are_checked_before_carrier_bodies(self):
        root = Path(__file__).resolve().parents[2]
        for path, required in [
            ('crates/hex-audio/src/prepared.rs', 'root.matches_spectrum_capture(self.spectrum_capture.as_ref())'),
            ('crates/hex-audio/src/prepared/endpoints.rs', 'root.matches_spectrum_reader(self.spectrum_reader.as_ref())')]:
            source = (root / path).read_text()
            self.assertEqual(violations(path, source), [])
            self.assertTrue(violations(path, source.replace(required, 'true')))
        self.assertTrue(violations('crates/hex-audio/src/spectrum_worker/native.rs', 'pair_actual_fresh_ring_on_control'))

    def test_safe_stop_access_does_not_spread_into_other_owners_or_callbacks(self):
        for name in ['try_load_stop_on_control', 'try_request_stop_on_control', 'try_value_identity_on_control']:
            self.assertEqual(violations('crates/hex-audio/src/spectrum_worker/stop.rs', name), [])
            for path in ['crates/hex-ui/src/foreign.rs', 'crates/hex-audio/src/prepared.rs',
                         'crates/hex-audio/src/spectrum_worker.rs', 'crates/hex-session/src/service.rs']:
                self.assertTrue(violations(path, name))

    def test_native_selected_operations_have_one_actual_private_facade(self):
        path = 'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source + '\nimpl OriginalSamplerNativeHost for Foreign {}'))
        for other in ['crates/hex-session/src/resource_plan/original_sampler.rs',
                      'crates/hex-audio/src/prepared.rs',
                      'crates/hex-engine/src/foreign.rs']:
            self.assertTrue(violations(other, 'OriginalSamplerNativeHost'))
        self.assertTrue(violations('crates/hex-engine/src/sampler_host_fields/original_driver.rs',
                                   'impl OriginalSamplerNativeHost for Foreign {}'))

    def test_private_constructor_stages_never_become_external_approvals(self):
        for name in ['inspect_original_sampler_revision_with_writers_on_control',
                     'construct_original_sampler_revision_with_writers_on_control']:
            for path in ['crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
                         'crates/hex-session/src/resource_plan/original_sampler.rs',
                         'crates/hex-engine/src/foreign.rs']:
                self.assertTrue(violations(path, name))
            self.assertEqual(violations('crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile/complete.rs', name), [])

    def test_candidate_constructor_stages_remain_engine_internal(self):
        for name in ['OriginalSamplerConstruction',
                     'inspect_original_sampler_with_writers_on_control',
                     'construct_original_sampler_with_writers_on_control',
                     'original_sampler_compiler_reservation',
                     'finish_original_sampler_runtime_on_control']:
            self.assertEqual(violations(
                'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs', name), [])
            for path in ['crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs',
                         'crates/hex-session/src/resource_plan/original_sampler.rs',
                         'crates/hex-engine/src/foreign.rs']:
                self.assertTrue(violations(path, name))

    def test_constructor_seal_inspection_stays_inside_existing_engine_custody(self):
        storage = 'crates/hex-engine/src/audio_candidate/storage.rs'
        actual = (Path(__file__).resolve().parents[2] / storage).read_text()
        for name in ['OriginalSamplerSealInspection',
                     'inspect_sampler_constructor_with_inventory_on_control',
                     'install_sampler_constructor_inspection_on_control']:
            self.assertEqual(violations(storage, actual + '\n' + name), [])
            for path in ['crates/hex-engine/src/foreign.rs',
                         'crates/hex-session/src/resource_plan/original_sampler.rs',
                         'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs']:
                self.assertTrue(violations(path, name))

    def test_original_sampler_writers_have_exact_producer_and_consumer_modules(self):
        for name in ['OriginalSamplerTransaction', 'OriginalSamplerSessionLoan',
                     'original_sampler_session_loan_on_control',
                     'original_sampler_transaction_on_control',
                     'original_sampler_writer_on_control']:
            self.assertTrue(violations('crates/hex-session/src/arbitrary.rs', name))
            self.assertTrue(violations('crates/hex-audio/src/prepared.rs', name))
            self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs', name))
        path = 'crates/hex-session/src/resource_plan/original_sampler.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source + '\nfn extra() { context.original_sampler_transaction_on_control(); }'))
        self.assertTrue(violations(path, source + '\nimpl OriginalSamplerTransaction {}'))

    def test_original_sampler_resident_fields_have_one_actual_session_producer(self):
        for path in ['crates/hex-ui/src/random.rs', 'crates/hex-session/src/random.rs',
                     'crates/hex-audio/src/prepared.rs']:
            self.assertTrue(violations(path, 'OriginalSamplerResidentFields'))
        path = 'crates/hex-session/src/resource_plan/original_sampler.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source + '\nimpl OriginalSamplerResidentFields for Foreign {}'))
        self.assertTrue(violations('crates/hex-engine/src/sampler_host_fields/resident.rs',
                                   'impl OriginalSamplerResidentFields for Foreign {}'))

    def test_closed_original_constructor_readers_do_not_enable_other_engine_modules(self):
        for path in [
            'crates/hex-engine/src/audio_candidate/preparation/original_resources.rs',
            'crates/hex-engine/src/audio_candidate/preparation_operations.rs',
            'crates/hex-engine/src/audio_candidate/storage.rs',
            'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile/complete.rs',
            'crates/hex-engine/src/audio_preparation/arrangement_clips/sampler_compile.rs',
        ]:
            actual = (Path(__file__).resolve().parents[2] / path).read_text()
            self.assertEqual(violations(path, actual + '\nhex_engine::sampler_host_fields::OriginalSamplerResidentFields'), [])
            self.assertTrue(violations(path, 'impl OriginalSamplerResidentFields for Foreign {}'))
        self.assertTrue(violations('crates/hex-engine/src/foreign.rs', 'hex_engine::sampler_host_fields::OriginalSamplerResidentFields'))

    def test_real_service_closed_consumer_is_reader_only_in_exact_test_leaf(self):
        path = 'crates/hex-desktop/src/native_audio/tests/closed_original.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source + '\nimpl OriginalSamplerResidentFields for Foreign {}'))
        self.assertTrue(violations(path, source + '\nimpl OriginalSamplerTransaction {}'))
        self.assertTrue(violations('crates/hex-desktop/src/native_audio.rs', source))
        self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs', source))

    def test_staged_editor_allocating_entry_has_only_the_closed_prefix_producer(self):
        root = Path(__file__).resolve().parents[2]
        path = 'crates/hex-project/src/history/original_sampler.rs'
        source = (root / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source.replace('pub fn stage_original_sampler_after_prefix_on_control', 'pub fn unchecked_stage')))
        self.assertTrue(violations(path, source.replace('.try_borrow_mut()', '.borrow_mut()')))
        for foreign in ['crates/hex-ui/src/arbitrary.rs',
                        'crates/hex-session/src/service.rs',
                        'crates/hex-engine/src/sampler_host_fields/original_driver.rs']:
            self.assertTrue(violations(foreign, 'x.stage_original_sampler_after_prefix_on_control();'))
        path = 'crates/hex-engine/src/audio_storage/model_buffers.rs'
        source = (root / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source.replace('.require_no_original_sampler_pending_on_control()', '.omitted_pending_check()')))

    def test_event_metadata_projection_has_one_owning_session_adapter(self):
        for name in ['EventMetadataFields', 'EventReasonFields', 'EventBatchFailureFields',
                     'TuningLibraryMetadataPayload', 'TuningSlotMetadataFields',
                     'event_metadata_fields_on_control']:
            for path in ['crates/hex-ui/src/arbitrary.rs',
                         'crates/hex-session/src/snapshot_exports/storage.rs',
                         'crates/hex-audio/src/prepared.rs']:
                self.assertTrue(violations(path, name))
            self.assertEqual(violations('crates/hex-session/src/service/event_fields.rs', name), [])

    def test_actual_file_owner_is_allowed(self):
        self.assertEqual(violations(
            'crates/hex-session/src/service/main_control_file.rs',
            "impl<'a> hex_engine::sampler_host_fields::FileSlotsFields<'a> for Reserved<'a> {}"), [])

    def test_foreign_module_in_same_crate_is_refused(self):
        self.assertTrue(violations('crates/hex-session/src/random.rs',
                                   'use hex_engine::sampler_host_fields as fields;'))

    def test_foreign_crate_and_aliased_import_are_refused(self):
        for text in ['use hex_engine::sampler_host_fields::{BodyRef as X};',
                     'let body = hex_engine::sampler_host_fields::BodyRef::of(&x);',
                     'impl FileSlotsFields for Wrong {}']:
            self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs', text))

    def test_indirect_producer_accessor_callsite_is_checked(self):
        self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs',
                                   'let fields = staged.sampler_fields_on_control()?;'))

    def test_export_result_children_require_exact_audited_producer(self):
        for name in ['FamilyExportResultFields', 'ExportResultPayloadFields',
                     'ExportResultOutcomeFields',
                     'borrow_sampler_export_result_fields_on_control']:
            self.assertTrue(violations('crates/hex-session/src/arbitrary.rs', name))
            self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs', name))
            self.assertEqual(violations(
                'crates/hex-session/src/snapshot_exports/native_export_results.rs', name), [])

    def test_journal_storage_borrow_requires_audited_actual_owner(self):
        for name in ['JournalFields', 'journal_storage_fields_on_control']:
            self.assertTrue(violations('crates/hex-storage/src/arbitrary.rs', name))
            self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs', name))
        self.assertEqual(violations('crates/hex-storage/src/journal.rs',
                                    'journal_storage_fields_on_control'), [])

    def test_file_token_observation_and_document_bridge_are_exact_audited_sources(self):
        for name in ['FileTokenStorageObservation', 'file_token_storage_on_control',
                     'borrowed_file_token_arc_field_on_control', 'DocumentTokenFields',
                     'borrow_sampler_document_fields_on_control']:
            self.assertTrue(violations('crates/hex-storage/src/arbitrary.rs', name))
            self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs', name))
        root = Path(__file__).resolve().parents[2]
        for path in ['crates/hex-storage/src/files.rs', 'crates/hex-storage/src/replacement.rs']:
            source = (root / path).read_text()
            self.assertEqual(violations(path, source), [])
            self.assertTrue(violations(path, source + '\nfn bad() { FileToken(arbitrary); }'))
        self.assertTrue(violations('crates/hex-session/src/arbitrary.rs', 'FileToken(arbitrary)'))
        self.assertTrue(violations('crates/hex-audio/src/prepared.rs',
                                   'token.file_token_storage_on_control()'))
        path = 'crates/hex-session/src/service/worker_roots.rs'
        source = (root / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source.replace(
            'let observation = token.file_token_storage_on_control();',
            'let observation = foreign.file_token_storage_on_control();', 1)))

    def test_carrier_producers_are_exact_four_in_owning_module(self):
        path = 'crates/hex-audio/src/native_callback_owner/registered.rs'
        body = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, body), [])
        self.assertTrue(violations(path, body + '\nimpl CarrierPayload for Other {}'))
        self.assertTrue(violations(path, body.replace('impl CarrierPayload for ClientEndpoints', 'impl CarrierPayload for Other')))
        self.assertTrue(violations('crates/hex-audio/src/foreign.rs', 'impl CarrierPayload for Other {}'))

    def test_root_field_producers_require_audited_owning_module(self):
        for name in ['NativeRootFieldRegistration', 'NativeRootFieldLease',
                     'borrow_sampler_native_root_fields_on_control']:
            self.assertTrue(violations('crates/hex-audio/src/foreign.rs', name))
            self.assertEqual(violations('crates/hex-audio/src/candidate_custody/root_fields.rs', name), [])

    def test_actual_both_input_check_precedes_all_carrier_births(self):
        path = 'crates/hex-audio/src/native_callback_owner.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source.replace('!callback.matches_fresh_registered_payload_on_control(&root)', 'false')))
        self.assertTrue(violations(path, source.replace('!control.matches_fresh_registered_payload_on_control(&root)', 'false')))

    def test_worker_resident_and_native_status_roles_are_required(self):
        path = 'crates/hex-audio/src/native_callback_owner/registered.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source.replace('self.matches_fresh_registered_payload_on_control(root)', 'true')))
        owning_path = 'crates/hex-audio/src/prepared.rs'
        owning = (Path(__file__).resolve().parents[2] / owning_path).read_text()
        self.assertTrue(violations(owning_path, owning.replace('self.matches_constructor_worker_fields_on_control(root)', 'true')))
        self.assertTrue(violations(path, source.replace('status_owner::NATIVE_ERROR', 'status_owner::NATIVE_DATA')))

    def test_other_architecture_source_is_unchanged(self):
        self.assertEqual(violations('crates/hex-ui/src/arbitrary.rs',
                                    'use hex_engine::Frame;'), [])

    def test_spectrum_static_quotes_do_not_authorize_borrowed_producers(self):
        path = 'crates/hex-audio/src/spectrum_worker/native.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source + '\nimpl NativeFields for Arbitrary {}'))
        self.assertTrue(violations(path, source + '\nSharedMetadataRef::of(&arbitrary)'))
        self.assertTrue(violations(path, source + '\nNativeAtomicSignal::borrow(&arbitrary)'))
        self.assertTrue(violations(path, source + '\nsize_of::<[hex_engine::sampler_host_fields::NativeAtomicSignal; 2]>()'))
        self.assertTrue(violations(path, source.replace(
            'size_of::<[hex_engine::sampler_host_fields::NativeAtomicSignal; 2]>()',
            'size_of::<[hex_engine::sampler_host_fields::NativeAtomicSignal; 3]>()')))
        self.assertTrue(violations('crates/hex-ui/src/random.rs',
                                   'SharedMetadataRef::constructor_bytes::<T>()'))

    def test_project_ring_factory_has_only_actual_owner_and_parent_routes(self):
        path = 'crates/hex-audio/src/spectrum_worker/project.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source + '\nSharedMetadataRef::of(&arbitrary)'))
        self.assertTrue(violations(path, source + '\nimpl NativeFields for Arbitrary {}'))
        for path in ['crates/hex-session/src/random.rs',
                     'crates/hex-audio/src/prepared.rs',
                     'crates/hex-audio/src/spectrum_worker.rs']:
            self.assertTrue(violations(path, 'x.spectrum_project_ring_on_control(0, 32)'))

    def test_packet_custody_has_only_actual_constructor_and_parent_producers(self):
        path = 'crates/hex-audio/src/spectrum_worker/project/packet.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source + '\nSharedMetadataRef::of(&foreign)'))
        self.assertTrue(violations(path, source.replace('#[cfg(all(test, feature = "allocation-probe"))]\n#[path = "packet/carrier_binding_tests.rs"]', '#[path = "packet/carrier_binding_tests.rs"]')))
        self.assertTrue(violations('crates/hex-audio/src/spectrum_worker.rs', 'NativeSpectrumPacketLease'))

        self.assertTrue(violations(path, source.replace('drop(self.payload.take())', '/* omitted actual destructor */')))
        host = 'crates/hex-audio/src/candidate_custody/eligible/publication/native_host.rs'
        host_source = (Path(__file__).resolve().parents[2] / host).read_text()
        self.assertEqual(violations(host, host_source), [])
        self.assertTrue(violations(host, host_source.replace('&self.bank.spectrum_packets', '&self.bank.spectrum_packets[..32]')))

        for path in ['crates/hex-ui/src/random.rs', 'crates/hex-session/src/random.rs',
                     'crates/hex-audio/src/prepared.rs']:
            self.assertTrue(violations(path, 'NativeSpectrumPacketRegistration::pair_actual_packet_on_control(fields)'))

    def test_final_native_completion_stays_after_actual_parent_join(self):
        path = 'crates/hex-audio/src/lifecycle.rs'
        source = (Path(__file__).resolve().parents[2] / path).read_text()
        self.assertEqual(violations(path, source), [])
        self.assertTrue(violations(path, source.replace('advance_shutdown_on_control()', 'request_stop()')))
        self.assertTrue(violations(path, source.replace('preflight_native_worker_birth(', 'omitted_preflight(')))
        self.assertTrue(violations(path, source + '\nfn bypass() { x.finish_root_after_native_fields_on_control(); }'))
        self.assertTrue(violations('crates/hex-audio/src/random.rs',
                                   'x.finish_root_after_native_fields_on_control()'))


if __name__ == '__main__':
    unittest.main()
