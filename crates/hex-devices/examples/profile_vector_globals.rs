//! Optimized headless Vector runtime-pair profile. This is not physical callback,
//! listening, allocation instrumentation, or numerical Live compatibility proof.
use hex_devices::operator_state::{OPERATOR_METADATA, OPERATOR_STATE_VERSION, parameter};
use hex_lattice::{
    NativeInstrumentDefinition, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair,
};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterValue};
use std::{
    error::Error,
    fs,
    hint::black_box,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
const RATE: u32 = 48_000;
const FRAMES: usize = 128;
const WARMUPS: usize = 100;
const MEASURED: usize = 2000;
const TRIALS: usize = 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_variants_use_current_codec_and_explicit_processing_modes() {
        for variant in [Variant::Legacy16, Variant::Operator16, Variant::Operator32] {
            let state = state(variant).unwrap();
            assert_eq!(state.state_version(), OPERATOR_STATE_VERSION);
            assert_eq!(
                OPERATOR_METADATA
                    .read_parameter(&state, parameter::GLOBAL_MODE)
                    .unwrap(),
                ParameterValue::Choice(u32::from(!matches!(variant, Variant::Legacy16)))
            );
            let factory = OPERATOR_METADATA
                .prepare_instrument(
                    &state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: RATE,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    },
                )
                .unwrap()
                .prepare_mutable_runtime()
                .unwrap()
                .unwrap();
            assert_eq!(
                factory.max_active_occurrences(),
                if matches!(variant, Variant::Legacy16) {
                    16
                } else {
                    64
                }
            );
        }
    }
}
#[derive(Clone, Copy)]
enum Variant {
    Legacy16,
    Operator16,
    Operator32,
}
impl Variant {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "legacy16" => Ok(Self::Legacy16),
            "operator16" => Ok(Self::Operator16),
            "operator32" => Ok(Self::Operator32),
            _ => Err("expected legacy16, operator16, or operator32".into()),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Legacy16 => "legacy16",
            Self::Operator16 => "operator16",
            Self::Operator32 => "operator32",
        }
    }
    fn voices(self) -> usize {
        if matches!(self, Self::Operator32) {
            32
        } else {
            16
        }
    }
}
fn state(variant: Variant) -> Result<NativeDeviceState, Box<dyn Error>> {
    let mut state = OPERATOR_METADATA.default_state()?;
    let mut patches = vec![
        (parameter::ALGORITHM, ParameterValue::Choice(1)),
        (
            parameter::VOICES,
            ParameterValue::Choice(if variant.voices() == 32 { 14 } else { 11 }),
        ),
        (parameter::RETRIGGER, ParameterValue::Bool(false)),
        (parameter::LFO_ENABLED, ParameterValue::Bool(true)),
        (parameter::LFO_SYNC, ParameterValue::Bool(false)),
        (parameter::LFO_RATE, ParameterValue::Float(1.0)),
        (parameter::LFO_AMOUNT, ParameterValue::Float(20.0)),
        (parameter::feedback(4), ParameterValue::Float(0.25)),
        (ParameterId::from_static(20001), ParameterValue::Bool(true)),
    ];
    for operator in 1..=4 {
        patches.push((
            parameter::lfo_destination(operator),
            ParameterValue::Bool(true),
        ));
        if operator > 1 {
            patches.push((
                parameter::oscillator_level(operator),
                ParameterValue::Float(0.35),
            ));
        }
    }
    for (id, value) in patches {
        state = OPERATOR_METADATA.patch_parameter(&state, id, value)?;
    }
    // Other global parameter edits opt in to global processing; set the mode last.
    Ok(OPERATOR_METADATA.patch_parameter(
        &state,
        parameter::GLOBAL_MODE,
        ParameterValue::Choice(u32::from(!matches!(variant, Variant::Legacy16))),
    )?)
}
fn percentile(samples: &[Duration], percent: usize) -> Result<f64, Box<dyn Error>> {
    let index = (samples.len() * percent).div_ceil(100).saturating_sub(1);
    Ok(samples
        .get(index)
        .ok_or("empty timing samples")?
        .as_secs_f64()
        * 1e6)
}
fn run(variant: Variant, path: &Path) -> Result<(), Box<dyn Error>> {
    let state = state(variant)?;
    fs::create_dir_all(path)?;
    fs::write(
        path.join(format!(
            "{}-state-v{}.bin",
            variant.label(),
            state.state_version()
        )),
        state.payload(),
    )?;
    let prepared = OPERATOR_METADATA.prepare_instrument(
        &state,
        &hex_lattice::DevicePreparationContext {
            sample_rate: RATE,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        },
    )?;
    let factory = prepared
        .prepare_mutable_runtime()?
        .ok_or("fixture did not select a mutable runtime")?;
    let mut samples = Vec::new();
    samples.try_reserve_exact(TRIALS * MEASURED)?;
    let events: Vec<_> = (0..variant.voices())
        .map(|i| NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: i as u64 + 1,
            pitch: 48 + (i % 16) as u8,
            velocity: 100,
        })
        .collect();
    println!(
        "variant={} voices={} state_version={} payload_bytes={} runtime_version={} twin_bytes={} declared_occurrences={} declared_events={} max_tail_frames={} latency={:?}",
        variant.label(),
        variant.voices(),
        state.state_version(),
        state.payload().len(),
        factory.runtime_version(),
        factory.declared_twin_bytes(),
        factory.max_active_occurrences(),
        factory.max_events_per_block(),
        factory.max_release_tail_frames(),
        factory.algorithmic_latency_frames()
    );
    let mut checksums = [0.0; TRIALS];
    for (trial, checksum) in checksums.iter_mut().enumerate() {
        let mut pair = NativeInstrumentRuntimePair::new(
            factory.as_ref(),
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 259,
                max_active_occurrences: 64,
                max_tail_frames: u64::from(RATE) * 600,
            },
        )?;
        let mut output = [[0.0; 2]; FRAMES];
        pair.begin_block();
        pair.process_block(&events, &mut output, &hex_lattice::EMPTY_DEVICE_RESOURCES)?;
        pair.commit_block();
        for _ in 0..WARMUPS {
            output.fill([0.0; 2]);
            pair.begin_block();
            pair.process_block(
                &[],
                black_box(&mut output),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )?;
            pair.commit_block();
            black_box(&output);
        }
        for _ in 0..MEASURED {
            output.fill([0.0; 2]);
            let started = Instant::now();
            pair.begin_block();
            // process_block performs its ordinary generic/event/clock preflight.
            pair.process_block(
                &[],
                black_box(&mut output),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )?;
            pair.commit_block();
            let elapsed = started.elapsed();
            samples.push(elapsed);
            // Validation and checksum are deliberately outside the timed region.
            for frame in black_box(&output) {
                for sample in frame {
                    if !sample.is_finite() {
                        return Err("nonfinite output".into());
                    }
                    *checksum += f64::from(*sample);
                }
            }
        }
        if !checksum.is_finite() {
            return Err("nonfinite checksum".into());
        }
        let begin = trial * MEASURED;
        let mut sorted = samples[begin..begin + MEASURED].to_vec();
        sorted.sort_unstable();
        println!(
            "trial={} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} checksum={:.12e}",
            trial + 1,
            percentile(&sorted, 50)?,
            percentile(&sorted, 95)?,
            percentile(&sorted, 99)?,
            percentile(&sorted, 100)?,
            checksum
        );
    }
    fs::create_dir_all(path)?;
    let mut raw = fs::File::create(path.join(format!("{}-samples.csv", variant.label())))?;
    writeln!(raw, "trial,block,nanoseconds")?;
    for (index, duration) in samples.iter().enumerate() {
        writeln!(
            raw,
            "{},{},{}",
            index / MEASURED + 1,
            index % MEASURED,
            duration.as_nanos()
        )?;
    }
    samples.sort_unstable();
    println!(
        "aggregate={} samples={} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} p99_deadline_percent={:.3} finite_checksum={:.12e}",
        variant.label(),
        samples.len(),
        percentile(&samples, 50)?,
        percentile(&samples, 95)?,
        percentile(&samples, 99)?,
        percentile(&samples, 100)?,
        percentile(&samples, 99)? / (FRAMES as f64 / f64::from(RATE) * 1e6) * 100.0,
        checksums.iter().sum::<f64>()
    );
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let argument = std::env::args()
        .nth(1)
        .ok_or("usage: profile_vector_globals legacy16|operator16|operator32 [output-directory]")?;
    let variant = Variant::parse(&argument)?;
    let directory = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "artifacts/profiles/vector-globals".to_owned());
    println!(
        "vector_globals_profile codec_version={OPERATOR_STATE_VERSION} state_fixture=current_codec_with_explicit_mode os={} arch={} hardware={} commit={} source_sha256={} rate={RATE} frames={FRAMES} warmups={WARMUPS} measured={MEASURED} trials={TRIALS} optimization=release percentiles=nearest_rank outlier_removal=none",
        std::env::consts::OS,
        std::env::consts::ARCH,
        option_env!("HEX_PROFILE_HARDWARE").unwrap_or("unrecorded"),
        option_env!("HEX_PROFILE_GIT_REVISION").unwrap_or("unrecorded"),
        option_env!("HEX_PROFILE_SOURCE_SHA256").unwrap_or("unrecorded")
    );
    println!(
        "protocol=begin_block/process_block_with_internal_preflight/commit_block; initialization,note_admission,buffer_clear,checksum,storage,timing_sample_append are outside timing; legacy and operator use the same authored sine/serial/FM/filter/LFO parameters; operator mode uses2x oscillator/FM and carrier filter evaluation with8th-order Butterworth decimation, held output-rate envelopes/LFO, and4096-frame finite drain; no Live numerical parity or physical callback/deadline claim; allocator inactivity is source inspection only, not instrumentation"
    );
    run(variant, Path::new(&directory))
}
