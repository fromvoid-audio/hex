use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};

fn limits() -> NativeInstrumentRuntimeLimits {
    NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 259,
        max_active_occurrences: 32,
        max_tail_frames: 44_100 * 600,
    }
}
fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 32,
        handle_rows: 32,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
fn summary(pair: &RuntimePair) -> crate::AudioRootStorageSummary {
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger.root(pair, AudioStorageRole::Caller).unwrap();
    pair.visit_storage(&mut ledger).unwrap();
    let before = ledger.summary().unwrap();
    pair.visit_storage(&mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, before.data_bytes);
    assert_eq!(
        ledger.summary().unwrap().data_allocations,
        before.data_allocations
    );
    before
}

#[test]
fn concrete_route_has_two_actual_bodies_and_generic_same_factory_remains_unknown() {
    let Some((_bank, owner, _editor)) = crate::shared_analytic::sampler_tests::retained_fixture()
    else {
        return;
    };
    let factory = PreparedTrackRuntimeFactory::Sampler(owner.sampler_factory_on_control().unwrap());
    let pair = RuntimePair::from_factory(&factory, limits()).unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert!(pair.visit_storage_with_gate(&mut ledger, false).is_err());
    let PreparedTrackRuntimeFactory::Sampler(concrete) = &factory else {
        panic!("sampler")
    };
    let generic = RuntimePair::new(concrete.as_ref(), limits()).unwrap();
    assert!(generic.visit_storage(&mut ledger).is_err());
    if !native_box_layout_known() {
        assert!(pair.visit_storage(&mut ledger).is_err());
        return;
    }
    let boxes = pair.runtime_box_observations();
    assert!(!boxes[0].same_allocation(&boxes[1]));
    assert!(!boxes[0].same_owning_field(&boxes[1]));
    assert!(boxes[0].same_owning_field(&pair.runtime_box_observations()[0]));
    let expected = hex_devices::sampler::PreparedSamplerRuntime::runtime_box_layout_on_control();
    assert_eq!(boxes[0].pointee_layout(), expected);
    let data = summary(&pair);
    assert_eq!(data.data_allocations, 2); // inline parent is not a heap allocation
    assert_eq!(
        data.data_bytes,
        std::mem::size_of_val(&pair) + 2 * expected.size()
    );
    let mut exact = reservation();
    exact.max_retained_bytes = data.retained_bytes;
    exact.max_construction_peak_bytes = data.construction_peak_bytes;
    let mut ledger = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    ledger.root(&pair, AudioStorageRole::Caller).unwrap();
    pair.visit_storage(&mut ledger).unwrap();
    drop(ledger);
    exact.max_retained_bytes -= 1;
    let mut ledger = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    ledger.root(&pair, AudioStorageRole::Caller).unwrap();
    pair.visit_storage(&mut ledger).unwrap();
    assert!(ledger.summary().is_err());
    // Independent whole-body oracle: an omitted entire Box has no shared
    // counts that could expose it. Expected actual body count/bytes must do so.
    let mut omitted = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    omitted.root(&pair, AudioStorageRole::Caller).unwrap();
    omitted
        .bytes(
            crate::audio_storage::Owner::exclusive(&pair.pair),
            crate::audio_storage::Location("sampler-runtime-box", 0),
            expected.size(),
        )
        .unwrap();
    assert_ne!(omitted.summary().unwrap().data_bytes, data.data_bytes);
    assert_ne!(
        omitted.summary().unwrap().data_allocations,
        data.data_allocations
    );
}

struct Misleading(hex_devices::sampler::PreparedSamplerRuntime);
impl PreparedMutableInstrumentRuntime for Misleading {
    fn runtime_version(&self) -> u32 {
        self.0.runtime_version()
    }
    fn declared_twin_bytes(&self) -> usize {
        self.0.declared_twin_bytes()
    }
    fn max_events_per_block(&self) -> usize {
        self.0.max_events_per_block()
    }
    fn max_active_occurrences(&self) -> usize {
        self.0.max_active_occurrences()
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.0.max_release_tail_frames()
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::MutableInstrumentRuntime> {
        self.0.create_runtime()
    }
    // A wrapper can deliberately pretend that downcasting proves origin.
    fn as_any(&self) -> &dyn std::any::Any {
        &self.0
    }
}
#[test]
fn opaque_wrapper_with_misleading_downcast_cannot_upgrade_constructor_route() {
    let Some((_bank, owner, _editor)) = crate::shared_analytic::sampler_tests::retained_fixture()
    else {
        return;
    };
    let concrete = owner.sampler_factory_on_control().unwrap();
    let wrapper = Misleading(concrete.as_ref().clone());
    assert!(
        wrapper
            .as_any()
            .is::<hex_devices::sampler::PreparedSamplerRuntime>()
    );
    let factory = PreparedTrackRuntimeFactory::Opaque(Box::new(wrapper));
    let pair = RuntimePair::from_factory(&factory, limits()).unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert!(pair.visit_storage(&mut ledger).is_err());
}

#[test]
fn history_transactions_reset_and_move_preserve_route_and_two_body_allocations() {
    let Some((_bank, owner, _editor)) = crate::shared_analytic::sampler_tests::retained_fixture()
    else {
        return;
    };
    let factory = PreparedTrackRuntimeFactory::Sampler(owner.sampler_factory_on_control().unwrap());
    let mut pair = RuntimePair::from_factory(&factory, limits()).unwrap();
    let mut checkpoint = RuntimePair::from_factory(&factory, limits()).unwrap();
    if !native_box_layout_known() {
        return;
    }
    let expected = summary(&pair).data_bytes;
    let on = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 127,
    }];
    let mut speculative = [[0.0; 2]; 1];
    pair.begin_block();
    pair.process_block_with_clock(&on, &mut speculative, None, owner.resources())
        .unwrap();
    pair.rollback_block();
    assert_eq!(summary(&pair).data_bytes, expected);
    let mut committed = [[0.0; 2]; 1];
    pair.begin_block();
    pair.process_block_with_clock(&on, &mut committed, None, owner.resources())
        .unwrap();
    pair.commit_block();
    assert_eq!(speculative, committed);
    assert_eq!(committed[0], crate::native_device_resources::tests::PCM[0]);
    assert_eq!(summary(&pair).data_bytes, expected);
    checkpoint.inherit_from(&pair).unwrap();
    let moved_tail = pair;
    let mut moved_tail = moved_tail;
    let mut tail_pcm = [[0.0; 2]; 3];
    let mut checkpoint_pcm = [[0.0; 2]; 3];
    moved_tail.begin_block();
    checkpoint.begin_block();
    moved_tail
        .process_block_with_clock(&[], &mut tail_pcm, None, owner.resources())
        .unwrap();
    checkpoint
        .process_block_with_clock(&[], &mut checkpoint_pcm, None, owner.resources())
        .unwrap();
    moved_tail.commit_block();
    checkpoint.commit_block();
    assert_eq!(tail_pcm, checkpoint_pcm);
    assert_eq!(&tail_pcm, &crate::native_device_resources::tests::PCM[1..]);
    moved_tail.reset();
    assert_eq!(summary(&moved_tail).data_bytes, expected);
    assert_eq!(summary(&checkpoint).data_bytes, expected);
    let a = moved_tail.runtime_box_observations();
    let b = checkpoint.runtime_box_observations();
    for old in &a {
        for new in &b {
            assert!(!old.same_allocation(new));
        }
    }
}

#[path = "../../../build_support/runtime_box_gate.rs"]
mod runtime_box_gate;
use crate::pcm_arc::build_gate_tests as pcm_gate;
#[test]
fn runtime_box_family_is_separate_and_unsupported_native_builds_refuse() {
    assert_ne!(runtime_box_gate::FAMILY, "arc-audiodata-retained-vec-v1");
    assert_ne!(
        runtime_box_gate::FAMILY,
        "canonical-midi-sized-arc-new-make-mut-v1"
    );
    for target in ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"] {
        let metadata = format!(
            "release: 1.94.0\ncommit-hash: {}\nhost: {target}\n",
            pcm_gate::COMMIT
        );
        for (installed, unsupported) in [(true, false), (false, false), (true, true)] {
            assert_eq!(
                runtime_box_gate::decide(pcm_gate::Observation {
                    verbose: &metadata,
                    host: target,
                    target,
                    ordinary_install: installed,
                    unsupported_inputs: unsupported,
                })
                .is_ok(),
                installed && !unsupported
            );
        }
    }
}
