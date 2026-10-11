//! Design-stage storage probe for Pulse2342. No device registration or DSP.
//!
//! These concrete types reserve every fixed field of a compact candidate,
//! including source-boundary f64 coefficients and frozen v6 canonical bytes.
//! They do not implement preparation, adoption, event handling or sampling.
use hex_devices::SineTable;
use std::mem::{align_of, offset_of, size_of};
use std::sync::{Arc, atomic::AtomicU64};

const OSCILLATORS: usize = 4;
const VOICES: usize = 16;
const EVENTS: usize = 256;
const V6_BYTES: usize = 364;
const PAIR_LIMIT: usize = 8_192;

#[repr(C)]
#[derive(Clone, Copy)]
struct Phase {
    fraction: f64,
    cycle: u64,
}

/// Prepared phase steps remain per note; integrated phases and f64 recurrence
/// memories remain per voice. Velocity is stored in a separate fixed array to
/// avoid seven padding bytes per voice. Zero occurrence ID denotes an empty
/// slot. Gate u64::MAX is the existing LFO's ungated sentinel, not a new law.
#[repr(C)]
#[derive(Clone, Copy)]
struct Voice {
    steps: [u64; OSCILLATORS],
    phases: [u64; OSCILLATORS],
    previous_outputs: [f64; OSCILLATORS],
    age: u64,
    gate: u64,
    occurrence_id: u64,
    lfo: Phase,
}

/// Cache only frame counts and original f64 timing coefficients. Initial,
/// peak and sustain are exactly reconstructible from canonical f32 words;
/// duplicating them in every prepared envelope is unnecessary for this layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct EnvelopeTiming {
    frames: [u64; 3],
    coefficients: [f64; 3],
}

/// Target-independent source boundary for the existing supported scalar ramp.
/// Exact target scalar values live in canonical_v6, not a duplicate array.
#[repr(C)]
#[derive(Clone, Copy)]
struct ScalarBoundary {
    levels: [f64; OSCILLATORS],
    carriers: [f64; OSCILLATORS],
    feedback: [f64; OSCILLATORS],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CompactRuntime<const H: usize> {
    // Preserve exact authored bits, including signed zero and inactive LFO.
    canonical_v6: [u8; V6_BYTES],
    // Target authoring values, f32 throughout. This is a layout alternative,
    // not a selected persistence format or confirmed Live amplitude domain.
    target_partials: [[f32; H]; OSCILLATORS],
    // Reserve a range/normalization/authoring-mode word per oscillator.
    target_settings: [[u32; 3]; OSCILLATORS],
    // Prepared target normalization is applied before interpolation.
    target_normalization: [f64; OSCILLATORS],
    // Preserve the actual interpolated boundary in f64 without requantization.
    source_partials: [[f64; H]; OSCILLATORS],
    // All four existing shapes can contribute to an interrupted transition.
    // Keeping only one old shape would fail repeated builtin/user retargets.
    source_builtin_weights: [[f64; 4]; OSCILLATORS],
    source_scalars: ScalarBoundary,
    envelopes: [EnvelopeTiming; OSCILLATORS],
    phase_origins: [u64; OSCILLATORS],
    // Topological order is prepared on control; no callback graph search.
    routing_order: [u8; OSCILLATORS],
    // Borrowed process-wide immutable assets only, never per-edit owners.
    sine: Option<&'static SineTable>,
    builtin_bank: Option<&'static [f32]>,
    sample_rate: u32,
    release_frames: u64,
    source_configuration_id: u64,
    target_configuration_id: u64,
    transition_frames: u32,
    transition_elapsed: u32,
    device_lfo: Phase,
    voices: [Voice; VOICES],
    velocities: [u8; VOICES],
}

/// A source-independent target proof must not retain a source graph/buffer.
#[repr(C)]
#[derive(Clone, Copy)]
struct TargetProof {
    configuration_id: u64,
    sample_rate: u32,
    transition_frames: u32,
    runtime_version: u32,
    reserved: u32,
}

/// Alternative requiring a NEW host accounting/lifetime contract. The current
/// lattice factory counts owned data per twin and cannot admit this by declaring
/// only an Arc handle. Report the complete unique allocation below instead.
#[repr(C)]
struct DeviceWideLaw<const H: usize> {
    canonical_v6: [u8; V6_BYTES],
    target_partials: [[f32; H]; OSCILLATORS],
    target_settings: [[u32; 3]; OSCILLATORS],
    target_normalization: [f64; OSCILLATORS],
    envelopes: [EnvelopeTiming; OSCILLATORS],
    phase_origins: [u64; OSCILLATORS],
    routing_order: [u8; OSCILLATORS],
    sine: Option<&'static SineTable>,
    builtin_bank: Option<&'static [f32]>,
    sample_rate: u32,
    transition_frames: u32,
    release_frames: u64,
    target_configuration_id: u64,
    // A real safe late-fill representation, not an UnsafeCell/raw pointer.
    // No implementation of the required freeze/admission protocol exists here.
    source_partials: [[AtomicU64; H]; OSCILLATORS],
    source_builtin_weights: [[AtomicU64; 4]; OSCILLATORS],
    source_scalars: [[AtomicU64; OSCILLATORS]; 3],
    source_configuration_id: AtomicU64,
    frozen: AtomicU64,
}

/// Hypothesis: reconstruct original quantized base steps from canonical tuning,
/// pitch and rate using precisely the old arithmetic. This changes CPU work,
/// not stored phase/recurrence precision; equivalence is NOT established here.
#[repr(C)]
#[derive(Clone, Copy)]
struct RecomputedStepVoice {
    phases: [u64; OSCILLATORS],
    previous_outputs: [f64; OSCILLATORS],
    age: u64,
    gate: u64,
    occurrence_id: u64,
    lfo: Phase,
}

#[repr(C)]
struct DeviceWideTwin<const H: usize> {
    law: Arc<DeviceWideLaw<H>>,
    voices: [RecomputedStepVoice; VOICES],
    pitches: [u8; VOICES],
    velocities: [u8; VOICES],
    device_lfo: Phase,
    transition_elapsed: u32,
}

#[repr(C)]
struct DeviceWideCachedStepTwin<const H: usize> {
    law: Arc<DeviceWideLaw<H>>,
    voices: [Voice; VOICES],
    velocities: [u8; VOICES],
    device_lfo: Phase,
    transition_elapsed: u32,
}

/// Count BOTH mutable twins, one unique shared payload, Arc's strong/weak words,
/// and a retained graph-owner handle. Allocator bookkeeping is excluded as in
/// the existing contract. Host/ABI/lifetime implementation remains unproven.
fn device_wide_bytes<const H: usize>() -> usize {
    2 * size_of::<DeviceWideTwin<H>>()
        + size_of::<DeviceWideLaw<H>>()
        + 2 * size_of::<std::sync::atomic::AtomicUsize>()
        + size_of::<Arc<DeviceWideLaw<H>>>()
}

fn inline_field_bytes<const H: usize>() -> usize {
    V6_BYTES
        + size_of::<[[f32; H]; OSCILLATORS]>()
        + size_of::<[[u32; 3]; OSCILLATORS]>()
        + size_of::<[f64; OSCILLATORS]>()
        + size_of::<[[f64; H]; OSCILLATORS]>()
        + size_of::<[[f64; 4]; OSCILLATORS]>()
        + size_of::<ScalarBoundary>()
        + size_of::<[EnvelopeTiming; OSCILLATORS]>()
        + size_of::<[u64; OSCILLATORS]>()
        + size_of::<[u8; OSCILLATORS]>()
        + size_of::<Option<&'static SineTable>>()
        + size_of::<Option<&'static [f32]>>()
        + size_of::<u32>() // sample rate
        + size_of::<u64>() // release frames
        + 2 * size_of::<u64>() // source/target identity
        + 2 * size_of::<u32>() // transition duration/clock
        + size_of::<Phase>()
        + size_of::<[Voice; VOICES]>()
        + VOICES
}

fn report<const H: usize>() {
    let twin = size_of::<CompactRuntime<H>>();
    println!(
        "partials={H} twin={twin} pair={} alignment={} pair_limit={PAIR_LIMIT} fits={}",
        twin * 2,
        align_of::<CompactRuntime<H>>(),
        twin * 2 <= PAIR_LIMIT
    );
    println!(
        "  inline_fields={} padding={} separately_owned_buffers=0",
        inline_field_bytes::<H>(),
        twin - inline_field_bytes::<H>()
    );
    println!(
        "  canonical_v6={V6_BYTES} target_partials={} source_partials={} builtin_boundary={} voice_history={} velocities={VOICES} owned_heap=0 proof={}",
        size_of::<[[f32; H]; OSCILLATORS]>(),
        size_of::<[[f64; H]; OSCILLATORS]>(),
        size_of::<[[f64; 4]; OSCILLATORS]>(),
        size_of::<[Voice; VOICES]>(),
        size_of::<TargetProof>()
    );
    println!(
        "  source_offset={} voices_offset={} velocities_offset={} event_limit={EVENTS} events_borrowed=true",
        offset_of!(CompactRuntime<H>, source_partials),
        offset_of!(CompactRuntime<H>, voices),
        offset_of!(CompactRuntime<H>, velocities)
    );
}

fn main() {
    println!(
        "design-stage storage only: arch={} pointer_bits={} voices={VOICES} v6_bytes={V6_BYTES}",
        std::env::consts::ARCH,
        usize::BITS
    );
    report::<16>();
    report::<32>();
    report::<64>();
    println!(
        "devicewide_alternative partials=64 mutable_twin={} unique_shared={} arc_words={} graph_handle={} aggregate={} limit={PAIR_LIMIT} fits={} current_trait_compatible=false",
        size_of::<DeviceWideTwin<64>>(),
        size_of::<DeviceWideLaw<64>>(),
        2 * size_of::<std::sync::atomic::AtomicUsize>(),
        size_of::<Arc<DeviceWideLaw<64>>>(),
        device_wide_bytes::<64>(),
        device_wide_bytes::<64>() <= PAIR_LIMIT
    );
    println!(
        "devicewide_cached_steps aggregate={} fits=false",
        device_wide_bytes::<64>()
            + 2 * (size_of::<DeviceWideCachedStepTwin<64>>() - size_of::<DeviceWideTwin<64>>())
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::needs_drop;

    #[test]
    fn all_owned_storage_is_inline_copyable_and_accounted_in_each_twin() {
        fn copyable<T: Copy>() {}
        copyable::<CompactRuntime<64>>();
        assert!(!needs_drop::<CompactRuntime<64>>());
        assert!(!needs_drop::<TargetProof>());
        assert_eq!(size_of::<[Voice; VOICES]>(), size_of::<Voice>() * 16);
        assert_eq!(size_of::<[[f64; 64]; 4]>(), 2_048);
        assert_eq!(size_of::<[[f32; 64]; 4]>(), 1_024);
        // The final velocity array's end is within the declared twin, including
        // tail padding. No separately owned Vec/Box/Arc can be hidden by Copy.
        assert!(
            offset_of!(CompactRuntime<64>, velocities) + VOICES <= size_of::<CompactRuntime<64>>()
        );
        assert_eq!(V6_BYTES, 320 + 11 * 4);
    }

    #[test]
    fn capacity_and_full_coefficient_trajectories_are_not_reduced_to_fit() {
        assert_eq!(VOICES, 16);
        assert_eq!(EVENTS, 256);
        assert_eq!(size_of::<Phase>(), 16);
        assert_eq!(size_of::<EnvelopeTiming>(), 48);
        // Borrowed event batches are the existing host contract: no device
        // event array is included or needed. These checks concern owned bytes.
        if usize::BITS == 64 {
            assert_eq!(size_of::<CompactRuntime<16>>(), 3_944);
            assert_eq!(align_of::<CompactRuntime<16>>(), 8);
            assert_eq!(inline_field_bytes::<16>(), 3_932);
            assert_eq!(size_of::<TargetProof>(), 24);
            assert!(size_of::<CompactRuntime<16>>() * 2 <= PAIR_LIMIT);
            assert!(size_of::<CompactRuntime<32>>() * 2 > PAIR_LIMIT);
            assert!(size_of::<CompactRuntime<64>>() * 2 > PAIR_LIMIT);
        }
        assert_eq!(
            size_of::<CompactRuntime<64>>() - size_of::<CompactRuntime<32>>(),
            4 * 32 * (size_of::<f32>() + size_of::<f64>())
        );
    }

    #[test]
    fn source_has_room_for_every_builtin_and_exact_f64_boundary() {
        assert_eq!(size_of::<ScalarBoundary>(), 12 * size_of::<f64>());
        assert_eq!(size_of::<[[f64; 4]; 4]>(), 128);
        // Even before envelope/clock/shape metadata, this representation's
        // 64-partial lower bound exceeds one twin. This is not a proof that
        // every possible representation is impossible.
        let lower_bound = size_of::<[Voice; VOICES]>()
            + V6_BYTES
            + size_of::<[[f32; 64]; OSCILLATORS]>()
            + size_of::<[[f64; 64]; OSCILLATORS]>();
        assert_eq!(lower_bound, 5_612);
        assert!(lower_bound * 2 > PAIR_LIMIT);
    }

    #[test]
    fn device_wide_alternative_counts_unique_owned_data_and_both_mutable_histories() {
        assert_eq!(size_of::<AtomicU64>(), size_of::<f64>());
        assert_eq!(size_of::<RecomputedStepVoice>(), 104);
        assert_eq!(size_of::<[RecomputedStepVoice; VOICES]>(), 1_664);
        // Copy alone is insufficient for the Arc-owning alternative. Its shared
        // allocation is explicitly included, never hidden behind pointer size.
        assert!(needs_drop::<DeviceWideTwin<64>>());
        assert!(device_wide_bytes::<64>() > 2 * size_of::<DeviceWideTwin<64>>());
        assert!(size_of::<DeviceWideLaw<64>>() >= V6_BYTES + 1_024 + 2_048 + 128 + 96);
        if usize::BITS == 64 {
            assert_eq!(size_of::<DeviceWideLaw<64>>(), 4_040);
            assert_eq!(device_wide_bytes::<64>(), 7_520);
            assert!(device_wide_bytes::<64>() <= PAIR_LIMIT);
            assert_eq!(
                device_wide_bytes::<64>()
                    + 2 * (size_of::<DeviceWideCachedStepTwin<64>>()
                        - size_of::<DeviceWideTwin<64>>()),
                8_512
            );
            // Under the CURRENT per-twin-owned-buffer contract it still fails.
            assert!(
                2 * (size_of::<DeviceWideTwin<64>>() + size_of::<DeviceWideLaw<64>>()) > PAIR_LIMIT
            );
        }
    }

    #[test]
    fn recomputed_steps_match_real_prepared_payload_including_nyquist_refusal() {
        use hex_devices::operator_state::{OperatorMetadata, parameter};
        use hex_lattice::NativeInstrumentDefinition;
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

        // Source equivalence check for the storage alternative, not a new
        // admitted callback implementation. Retain the operation order: Hz,
        // ratio/fixed product, divide by rate, multiply 2^64, then round.
        let metadata = OperatorMetadata;
        let ratios = [0.007_812_5_f32, 0.99, 1.0, 128.0];
        let mut state = metadata.default_state().unwrap();
        for (i, ratio) in ratios.iter().enumerate() {
            state = metadata
                .patch_parameter(
                    &state,
                    parameter::oscillator_ratio(i as u32 + 1),
                    ParameterValue::Float(*ratio),
                )
                .unwrap();
        }
        let mut accepted = 0;
        let mut refused = 0;
        for fixed in [false, true] {
            let mut law = state.clone();
            if fixed {
                law = metadata
                    .patch_parameter(&law, parameter::fixed(4), ParameterValue::Bool(true))
                    .unwrap();
                law = metadata
                    .patch_parameter(
                        &law,
                        parameter::fixed_frequency(4),
                        ParameterValue::Float(20_000.0),
                    )
                    .unwrap();
            }
            for rate in [8_000, 44_100, 48_000, 192_000] {
                let instrument = metadata.prepare_instrument(&law, rate).unwrap();
                let context = instrument
                    .prepare_shared_analytic_context()
                    .unwrap()
                    .unwrap();
                for pitch in 0_u8..=127 {
                    let hz = 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0);
                    let frequencies: [f64; 4] = std::array::from_fn(|i| {
                        if fixed && i == 3 {
                            20_000.0
                        } else {
                            hz * f64::from(ratios[i])
                        }
                    });
                    let valid = frequencies.iter().all(|hz| *hz < f64::from(rate) * 0.5);
                    let prepared = context.prepare_note(pitch, 97);
                    assert_eq!(prepared.is_ok(), valid, "rate={rate} pitch={pitch}");
                    if let Ok(note) = prepared {
                        accepted += 1;
                        for (i, frequency) in frequencies.iter().enumerate() {
                            let step = (frequency / f64::from(rate) * 18_446_744_073_709_551_616.0)
                                .round() as u64;
                            assert_eq!(note.payload()[i], step);
                        }
                    } else {
                        refused += 1;
                    }
                }
            }
        }
        assert!(accepted > 0 && refused > 0);
    }
}
