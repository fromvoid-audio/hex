//! Private actual-EQ layout counterexample, not an admitted processor.
#[path = "../../hex-audio/tests/eq8_oversampling_study/kernel.rs"]
#[allow(dead_code)] // Same authoritative private kernel; this target measures only layout.
mod kernel;
use hex_devices::eq8::{EQ8_DEFINITION, Eq8Runtime, PreparedEq8};
use hex_lattice::NativeDeviceDefinition;
use hex_lattice_contract::NativeDeviceMetadata;
use std::mem::{align_of, size_of};
#[repr(C)]
struct ProposedEqTwin {
    eq: Eq8Runtime,
    fir: kernel::History,
}
#[test]
fn actual_eq_twin_storage_and_complete_shared_ownership_exceeded_original_cap() {
    let state = EQ8_DEFINITION.default_state().unwrap();
    let prepared = EQ8_DEFINITION
        .prepare_processor(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(prepared.declared_runtime_bytes(), size_of::<Eq8Runtime>());
    // The historical 7,536-byte runtime predated the fixed inline bank
    // inventory; each runtime now owns that inventory rather than a shared target.
    assert_eq!(size_of::<Eq8Runtime>(), 22_272);
    assert_eq!(
        (size_of::<kernel::History>(), align_of::<kernel::History>()),
        (632, 8)
    );
    assert_eq!(size_of::<ProposedEqTwin>(), 22_904);
    // Historical 4 KiB refusal evidence remains true; the current 256 KiB
    // per-twin policy does not itself admit this private prototype.
    assert!(size_of::<ProposedEqTwin>() > 4_096);
    assert!(size_of::<ProposedEqTwin>() <= hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES);
    let runtime = prepared.create_runtime();
    let eq = runtime.as_any().downcast_ref::<Eq8Runtime>().unwrap();
    let a = Box::new(ProposedEqTwin {
        eq: eq.clone(),
        fir: kernel::History::new(96000).unwrap(),
    });
    let b = Box::new(ProposedEqTwin {
        eq: eq.clone(),
        fir: kernel::History::new(96000).unwrap(),
    });
    assert_eq!(size_of_val(&*a) + size_of_val(&*b), 45_808);
    assert_eq!(size_of_val(&a) + size_of_val(&b), 16);
    assert_eq!(kernel::weights_bytes(), 128);
    assert_eq!(size_of_val(&*prepared), size_of::<PreparedEq8>());
    assert_eq!(size_of_val(&prepared), 16); // Actual boxed trait-object prepared handle.
    assert_eq!(
        size_of::<Box<dyn hex_lattice::NativeProcessorRuntime>>(),
        16
    );
    // The prepared owner holds its own inline banks, with no separate target Arc.
    let prepared_arc_charge = size_of::<PreparedEq8>()
        + 2 * size_of::<usize>()
        + 2 * align_of::<PreparedEq8>().max(align_of::<usize>());
    assert_eq!(
        EQ8_DEFINITION.source_effect_factory_bytes(),
        Some(prepared_arc_charge)
    );
    eprintln!(
        "actual Eq={} FIR={} twin={} excess={} twin_payloads={} twin_box_handles={} static_FIR={} prepared_payload={} prepared_arc_charge={}",
        size_of::<Eq8Runtime>(),
        size_of::<kernel::History>(),
        size_of::<ProposedEqTwin>(),
        size_of::<ProposedEqTwin>() - 4096,
        size_of_val(&*a) + size_of_val(&*b),
        16,
        kernel::weights_bytes(),
        size_of::<PreparedEq8>(),
        prepared_arc_charge
    );
}

struct OvercapProbe(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl hex_lattice::PreparedNativeProcessor for OvercapProbe {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES + 1
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::NativeProcessorRuntime> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("overcap prototype factory must not run")
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(16)
    }
}
#[test]
fn real_chain_admission_refuses_current_overcap_payload_before_any_twin_factory() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorInstance, ProcessorChainOwner, StereoMatrix,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let state = EQ8_DEFINITION.default_state().unwrap();
    let instance = PreparedProcessorInstance {
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: state.type_id(),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48000,
        meter: Default::default(),
        configuration: Some(Arc::new(state)),
        definition: Some(Arc::new(OvercapProbe(calls.clone()))),
    };
    assert!(matches!(
        NativeRuntimeChain::new(vec![instance]),
        Err(hex_lattice_contract::DeviceContractError::RuntimeBudgetExceeded)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
