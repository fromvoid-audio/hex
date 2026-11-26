//! Core Operator (Vector) parameter automation and sample-rate modulation target catalog.
//!
//! Targets are listed in strictly ascending `ParameterId` order as required by
//! `hex-lattice`'s `InstrumentParameterRuntime` and `hex-engine`'s instrument
//! automation compiler.
use super::*;
use hex_lattice_contract::{
    ParameterAccess, ParameterAutomation, ParameterChangeImpact, ParameterModulation,
    ParameterNormalization, ParameterSemantics, ParameterSmoothing, ParameterStepping,
};

pub const FILTER_FREQUENCY: ParameterId =
    ParameterId::from_static(crate::operator_filter::FIRST + 4);
pub const FILTER_RESONANCE: ParameterId =
    ParameterId::from_static(crate::operator_filter::FIRST + 5);
pub const FILTER_MORPH: ParameterId = ParameterId::from_static(crate::operator_filter::FIRST + 6);

pub(super) const TARGET_COUNT: usize = 33;
pub(super) const PAN_IDX: usize = 14;
pub(super) const FINE_OFFSET_BASE_IDX: usize = 15;
pub(super) const TIME_IDX: usize = 19;
pub(super) const TONE_IDX: usize = 20;
pub(super) const VOLUME_IDX: usize = 21;
pub(super) const TRANSPOSE_IDX: usize = 22;
pub(super) const SPREAD_IDX: usize = 23;
pub(super) const GLIDE_TIME_IDX: usize = 24;
pub(super) const PITCH_ENV_AMOUNT_IDX: usize = 25;
pub(super) const FILTER_ENV_AMOUNT_IDX: usize = 26;
pub(super) const SHAPER_DRIVE_IDX: usize = 27;
pub(super) const SHAPER_DRY_WET_IDX: usize = 28;
pub(super) const FILTER_DRIVE_IDX: usize = 29;
pub(super) const FILTER_FREQ_IDX: usize = 30;
pub(super) const FILTER_RES_IDX: usize = 31;
pub(super) const FILTER_MORPH_IDX: usize = 32;

pub(super) const TARGET_IDS: [ParameterId; TARGET_COUNT] = [
    parameter::oscillator_ratio(1), // 1
    parameter::oscillator_level(1), // 2
    parameter::oscillator_ratio(2), // 4
    parameter::oscillator_level(2), // 5
    parameter::oscillator_ratio(3), // 7
    parameter::oscillator_level(3), // 8
    parameter::oscillator_ratio(4), // 10
    parameter::oscillator_level(4), // 11
    parameter::feedback(1),         // 78
    parameter::feedback(2),         // 79
    parameter::feedback(3),         // 80
    parameter::feedback(4),         // 81
    parameter::LFO_RATE,            // 85
    parameter::LFO_AMOUNT,          // 87
    parameter::PAN,                 // 605
    parameter::fine_offset(1),      // 609
    parameter::fine_offset(2),      // 610
    parameter::fine_offset(3),      // 611
    parameter::fine_offset(4),      // 612
    parameter::TIME,                // 617
    parameter::TONE,                // 618
    parameter::VOLUME,              // 619
    parameter::TRANSPOSE,           // 640
    parameter::SPREAD,              // 641
    parameter::GLIDE_TIME,          // 643
    parameter::PITCH_ENV_AMOUNT,    // 651
    parameter::FILTER_ENV_AMOUNT,   // 671
    parameter::SHAPER_DRIVE,        // 688
    parameter::SHAPER_DRY_WET,      // 689
    parameter::FILTER_DRIVE,        // 690
    FILTER_FREQUENCY,               // 20_005
    FILTER_RESONANCE,               // 20_006
    FILTER_MORPH,                   // 20_007
];

const fn find_descriptor(id: ParameterId) -> ParameterDescriptor {
    let mut i = 0;
    while i < PARAMETER_COUNT {
        if PARAMETERS[i].id.value() == id.value() {
            return PARAMETERS[i];
        }
        i += 1;
    }
    panic!("operator automation target missing from descriptor table")
}

const fn build_targets() -> [ParameterDescriptor; TARGET_COUNT] {
    let mut out = [find_descriptor(TARGET_IDS[0]); TARGET_COUNT];
    let mut i = 0;
    while i < TARGET_COUNT {
        out[i] = find_descriptor(TARGET_IDS[i]);
        if i > 0 && out[i].id.value() <= out[i - 1].id.value() {
            panic!("operator automation targets must be strictly ascending");
        }
        i += 1;
    }
    out
}

pub(super) static TARGETS: [ParameterDescriptor; TARGET_COUNT] = build_targets();

pub(super) const fn target_index(id: ParameterId) -> Option<usize> {
    let mut i = 0;
    while i < TARGET_COUNT {
        if TARGET_IDS[i].value() == id.value() {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub(super) fn parameter_semantics(id: ParameterId) -> Option<ParameterSemantics> {
    target_index(id)?;
    let normalization =
        if id == parameter::LFO_RATE || id == parameter::GLIDE_TIME || id == FILTER_FREQUENCY {
            ParameterNormalization::PositiveLogarithmic
        } else {
            ParameterNormalization::Linear
        };
    Some(ParameterSemantics {
        access: ParameterAccess::ReadWrite,
        normalization,
        stepping: ParameterStepping::Continuous,
        automation: ParameterAutomation::DeviceArrangementEnvelope,
        modulation: ParameterModulation::SampleRate,
        smoothing: ParameterSmoothing::None,
        change_impact: ParameterChangeImpact::ValueOnly,
    })
}

pub(super) fn base_target_values(settings: &OperatorSettings) -> [f32; TARGET_COUNT] {
    let read_f32 = |val: Option<ParameterValue>| match val {
        Some(ParameterValue::Float(v)) => v,
        _ => 0.0,
    };
    [
        settings.fields[0],
        settings.fields[1],
        settings.fields[3],
        settings.fields[4],
        settings.fields[6],
        settings.fields[7],
        settings.fields[9],
        settings.fields[10],
        settings.feedback[0],
        settings.feedback[1],
        settings.feedback[2],
        settings.feedback[3],
        settings.lfo.rate() as f32,
        (settings.lfo.amount() * 100.0) as f32,
        settings.global.pan,
        settings.fine.values()[0],
        settings.fine.values()[1],
        settings.fine.values()[2],
        settings.fine.values()[3],
        settings.performance.time,
        settings.performance.tone,
        settings.performance.volume,
        settings.pitch_perf.transpose,
        settings.pitch_perf.spread,
        settings.pitch_perf.glide_time,
        settings.pitch_env.amount(),
        settings.filter_env.amount(),
        settings.filter_shaper.shaper_drive,
        settings.filter_shaper.shaper_dry_wet,
        settings.filter_shaper.filter_drive,
        read_f32(settings.filter.read(FILTER_FREQUENCY)),
        read_f32(settings.filter.read(FILTER_RESONANCE)),
        read_f32(settings.filter.read(FILTER_MORPH)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{
        DevicePreparationContext, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
        NativeInstrumentRuntimePair, NativeParameterAutomation,
    };

    struct SingleParam {
        id: [ParameterId; 1],
        values: Vec<f32>,
    }
    impl NativeParameterAutomation for SingleParam {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.id
        }
        fn value_at(&self, parameter: ParameterId, offset: usize) -> Option<ParameterValue> {
            (parameter == self.id[0])
                .then(|| self.values.get(offset).copied().map(ParameterValue::Float))
                .flatten()
        }
    }

    #[test]
    fn operator_core_targets_expose_sample_rate_modulation_and_drive_audio() {
        let mut state = OPERATOR_METADATA.default_state().unwrap();
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                ParameterId::from_static(20_001),
                ParameterValue::Bool(true),
            )
            .unwrap();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::LFO_ENABLED, ParameterValue::Bool(true))
            .unwrap();
        // Parallel algorithm (choice 11) makes op1 a carrier with no incoming
        // modulation so feedback(1) is enabled alongside level/pitch/filter.
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::ALGORITHM, ParameterValue::Choice(11))
            .unwrap();
        for &id in &TARGET_IDS {
            let sem = OPERATOR_METADATA
                .parameter_semantics_for_state(&state, id)
                .unwrap()
                .expect("semantics present");
            assert_eq!(
                sem.automation,
                ParameterAutomation::DeviceArrangementEnvelope
            );
            assert_eq!(sem.modulation, ParameterModulation::SampleRate);
        }

        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let prepared = OPERATOR_METADATA
            .prepare_instrument(&state, &context)
            .unwrap();
        let factory = prepared
            .prepare_mutable_runtime()
            .unwrap()
            .expect("consolidated mutable runtime");
        assert_eq!(
            factory.parameter_automation_descriptors().len(),
            TARGET_COUNT
        );

        let limits = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 64,
            max_tail_frames: 48_000 * 600,
        };
        let render_with = |id: ParameterId, value: f32| {
            let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
            let mut out = [[0.0_f32; 2]; 128];
            let events = [NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 127,
            }];
            let auto = SingleParam {
                id: [id],
                values: vec![value; 128],
            };
            pair.begin_block();
            pair.process_block_with_clock_and_automation(
                &events,
                &mut out,
                None,
                &[],
                &auto,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
            pair.commit_block();
            let mut targets = [None; TARGET_COUNT];
            pair.copy_committed_parameter_targets(&TARGET_IDS, &mut targets)
                .unwrap();
            (out, targets)
        };

        let (loud, loud_targets) = render_with(parameter::oscillator_level(1), 1.0);
        let (quiet, quiet_targets) = render_with(parameter::oscillator_level(1), 0.1);
        assert_eq!(
            loud_targets[target_index(parameter::oscillator_level(1)).unwrap()],
            Some(ParameterValue::Float(1.0))
        );
        assert_eq!(
            quiet_targets[target_index(parameter::oscillator_level(1)).unwrap()],
            Some(ParameterValue::Float(0.1))
        );
        let peak = |buf: &[[f32; 2]]| {
            buf.iter()
                .flatten()
                .map(|s| s.abs())
                .fold(0.0_f32, f32::max)
        };
        assert!(peak(&loud) > peak(&quiet) * 5.0);

        let (bright, _) = render_with(FILTER_FREQUENCY, 18_000.0);
        let (dark, _) = render_with(FILTER_FREQUENCY, 80.0);
        assert_ne!(bright, dark);

        let (base_pitch, _) = render_with(parameter::fine_offset(1), 0.0);
        let (detuned, _) = render_with(parameter::fine_offset(1), 250.0);
        assert_ne!(base_pitch, detuned);

        let (no_fb, _) = render_with(parameter::feedback(1), 0.0);
        let (hi_fb, _) = render_with(parameter::feedback(1), 0.8);
        assert_ne!(no_fb, hi_fb);
    }

    #[test]
    fn host_filter_automation_composes_with_per_voice_filter_modulation() {
        let build_state = |velocity_filter_amount: f32| {
            let mut state = OPERATOR_METADATA.default_state().unwrap();
            // Parallel algorithm with op 1 feedback produces rich harmonics for the filter.
            state = OPERATOR_METADATA
                .patch_parameter(&state, parameter::ALGORITHM, ParameterValue::Choice(11))
                .unwrap();
            state = OPERATOR_METADATA
                .patch_parameter(&state, parameter::feedback(1), ParameterValue::Float(0.9))
                .unwrap();
            state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    ParameterId::from_static(20_001),
                    ParameterValue::Bool(true),
                )
                .unwrap();
            // Velocity destination A (620) = choice 5 ("filter frequency"), amount A (621).
            state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    ParameterId::from_static(620),
                    ParameterValue::Choice(5),
                )
                .unwrap();
            state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    ParameterId::from_static(621),
                    ParameterValue::Float(velocity_filter_amount),
                )
                .unwrap();
            state
        };

        let render = |state: &NativeDeviceState| {
            let tuning = hex_lattice::TuningTable::default();
            let context = DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &tuning,
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            };
            let prepared = OPERATOR_METADATA
                .prepare_instrument(state, &context)
                .unwrap();
            let factory = prepared
                .prepare_mutable_runtime()
                .unwrap()
                .expect("consolidated mutable runtime");
            let limits = NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 259,
                max_active_occurrences: 64,
                max_tail_frames: 48_000 * 600,
            };
            let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
            let mut out = [[0.0_f32; 2]; 256];
            let events = [NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 72,
                velocity: 127,
            }];
            let auto = SingleParam {
                id: [FILTER_FREQUENCY],
                values: vec![200.0; 256],
            };
            pair.begin_block();
            pair.process_block_with_clock_and_automation(
                &events,
                &mut out,
                None,
                &[],
                &auto,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
            pair.commit_block();
            out
        };

        let without_vel_mod = render(&build_state(0.0));
        let with_vel_mod = render(&build_state(100.0));
        assert_ne!(
            without_vel_mod, with_vel_mod,
            "host FILTER_FREQUENCY automation must compose with per-voice velocity filter modulation"
        );
        let energy = |buf: &[[f32; 2]]| buf.iter().map(|f| f[0] * f[0]).sum::<f32>();
        assert!(
            energy(&with_vel_mod) > energy(&without_vel_mod) * 1.2,
            "positive velocity->cutoff modulation must open the automated 200 Hz filter ({} vs {})",
            energy(&with_vel_mod),
            energy(&without_vel_mod)
        );
    }

    #[test]
    fn mpe_timbre_boosts_modulators_without_inflating_pure_carriers() {
        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let limits = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 64,
            max_tail_frames: 48_000 * 600,
        };
        let render = |state: &NativeDeviceState, timbre: u8| {
            let prepared = OPERATOR_METADATA
                .prepare_instrument(state, &context)
                .unwrap();
            let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
            let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
            let mut out = [[0.0_f32; 2]; 128];
            let events = [
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteTimbre {
                    frame_offset: 0,
                    occurrence_id: std::num::NonZeroU64::new(1).unwrap(),
                    value: timbre,
                },
            ];
            pair.begin_block();
            pair.process_block(&events, &mut out, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .unwrap();
            pair.commit_block();
            out
        };

        // Algorithm 11 (all carriers): timbre must not inflate raw carrier gain.
        let parallel = OPERATOR_METADATA
            .patch_parameter(
                &OPERATOR_METADATA.default_state().unwrap(),
                parameter::ALGORITHM,
                ParameterValue::Choice(11),
            )
            .unwrap();
        assert_eq!(render(&parallel, 0), render(&parallel, 127));

        // Algorithm 0 (serial, op 2 modulates op 1): timbre must boost modulator depth.
        let serial = OPERATOR_METADATA
            .patch_parameter(
                &OPERATOR_METADATA.default_state().unwrap(),
                parameter::oscillator_level(2),
                ParameterValue::Float(1.0),
            )
            .unwrap();
        assert_ne!(render(&serial, 0), render(&serial, 127));
    }
}
