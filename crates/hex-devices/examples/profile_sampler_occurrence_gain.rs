//! Mutable Sampler host microbenchmark; no physical device deadline claim.
//!
//! Run `cargo run --release -p hex-devices --example profile_sampler_occurrence_gain`.
//! Constructs a looping mono source, then measures 32 held root-note voices
//! through the actual prepared twin, preflight, render and commit boundary.
//! All resources, gains, output and timing storage are allocated before timing.
use hex_devices::sampler::{
    Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape, Settings,
};
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    NativeInstrumentEvent, NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, TuningTable,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot};
use std::{error::Error, hint::black_box, time::Instant};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;
const VOICES: usize = 32;
const WARMUP: usize = 100;
const TRIALS: usize = 1_000;

fn main() -> Result<(), Box<dyn Error>> {
    let pcm: Vec<_> = (0..RATE)
        .map(|i| {
            ((f64::from(i) * 440.0 * std::f64::consts::TAU / f64::from(RATE)).sin() * 0.003) as f32
        })
        .collect();
    let view = DeviceResourceView {
        slot: DeviceAssetSlot::new(1)?,
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1)?,
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: RATE,
        pcm: DeviceResourcePcm::Mono(&pcm),
    };
    let views = [view];
    let resources = BorrowedDeviceResources(&views);
    let region = Region {
        start: 0,
        end: pcm.len(),
    };
    let mut settings = Settings::new(region);
    settings.mode = Mode::Classic {
        loop_region: Some(region),
    };
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(pcm.len(), RATE, SampleChannels::Mono)?,
        RATE,
        settings,
    )?
    .bind_source_on_control(view)?
    .prepare_runtime_on_control(VOICES, 64, &TuningTable::default())?;
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 64,
        max_active_occurrences: VOICES,
        max_tail_frames: u64::from(RATE) * 600,
    };
    let events: Vec<_> = (1..=VOICES as u64)
        .map(|occurrence_id| NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id,
            pitch: 60,
            velocity: 127,
        })
        .collect();
    let gains: Vec<_> = (1..=VOICES as u64)
        .map(|id| NativeInstrumentOccurrenceGain::new(id, 0.25, 0.75, BLOCK as u64, 0))
        .collect::<Result<_, _>>()?;
    println!(
        "rate={RATE} block={BLOCK} voices={VOICES} pitch=60 loop=48000 source=mono warmup={WARMUP} trials={TRIALS} timer=Instant includes=preflight-render-commit"
    );
    for automated in [false, true] {
        let mut pair = NativeInstrumentRuntimePair::new(&prepared, limits)?;
        let mut out = [[0.0_f32; 2]; BLOCK];
        pair.begin_block();
        pair.process_block(&events, &mut out, &resources)?;
        pair.commit_block();
        let mut ids = [0_u64; VOICES];
        if pair.active_occurrence_ids(&mut ids)? != VOICES {
            return Err("expected 32 held voices".into());
        }
        let mut durations = vec![0_u128; TRIALS];
        let mut checksum = 0.0_f64;
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for block in 0..WARMUP + TRIALS {
            out.fill([0.0; 2]);
            let start = Instant::now();
            pair.begin_block();
            if automated {
                pair.process_block_with_clock_and_occurrence_gains(
                    &[],
                    &mut out,
                    None,
                    &gains,
                    &resources,
                )?;
            } else {
                pair.process_block(&[], &mut out, &resources)?;
            }
            pair.commit_block();
            let elapsed = start.elapsed().as_nanos();
            black_box(&out);
            checksum += f64::from(out[block % BLOCK][0]);
            for frame in out {
                for sample in frame {
                    for byte in sample.to_bits().to_le_bytes() {
                        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
                    }
                }
            }
            if block >= WARMUP {
                durations[block - WARMUP] = elapsed;
            }
        }
        durations.sort_unstable();
        println!(
            "gain_ramps={} min_ns={} p50_ns={} p95_ns={} p99_ns={} max_ns={} checksum={checksum:.9} hash={hash:016x}",
            if automated { VOICES } else { 0 },
            durations[0],
            durations[TRIALS / 2],
            durations[TRIALS * 95 / 100],
            durations[TRIALS * 99 / 100],
            durations[TRIALS - 1]
        );
    }
    Ok(())
}
