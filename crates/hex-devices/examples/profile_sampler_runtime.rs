//! Offline registered mutable sampler pair timings, including transactional preflight.
//! Setup, PCM oracles, validation and hashing are outside timings. No native host
//! or audio-device deadline is measured. The harness uses public pre-cursor APIs.
use hex_devices::sampler::{
    Region,
    state::{AuthoredMode, SamplerState},
};
use hex_lattice::{
    BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity, DeviceResourcePcm,
    DeviceResourceView, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, PreparedMutableInstrumentRuntime, StereoFrame, TuningTable,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use std::{error::Error, hint::black_box, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const RATE: u32 = 48_000;
const ORACLE_FRAMES: usize = RATE as usize * 2;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

struct Options {
    voices: usize,
    block: usize,
    warmup: usize,
    measured: usize,
}
fn options() -> Result<Option<Options>> {
    let mut options = Options {
        voices: 16,
        block: 128,
        warmup: 100,
        measured: 2_000,
    };
    for argument in std::env::args().skip(1) {
        if argument == "--help" {
            println!(
                "usage: profile_sampler_runtime [--voices=1|16|32] [--block=128|512] [--warmup-blocks=n] [--measured-blocks=n]"
            );
            return Ok(None);
        } else if let Some(value) = argument.strip_prefix("--voices=") {
            options.voices = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--block=") {
            options.block = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--warmup-blocks=") {
            options.warmup = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--measured-blocks=") {
            options.measured = value.parse()?;
        } else {
            return Err(format!("unknown argument: {argument}").into());
        }
    }
    if ![1, 16, 32].contains(&options.voices)
        || ![128, 512].contains(&options.block)
        || options.warmup > 1_000_000
        || !(1..=1_000_000).contains(&options.measured)
    {
        return Err("workload exceeds voices/block/count bounds".into());
    }
    options
        .warmup
        .checked_add(options.measured)
        .and_then(|blocks| blocks.checked_mul(options.block))
        .ok_or("workload frame count overflow")?;
    Ok(Some(options))
}
fn limits(voices: usize) -> NativeInstrumentRuntimeLimits {
    NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 259,
        max_active_occurrences: voices,
        max_tail_frames: u64::from(RATE) * 60,
    }
}
fn events(voices: usize) -> Result<Vec<NativeInstrumentEvent>> {
    (0..voices)
        .map(|index| {
            Ok(NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: u64::try_from(index.checked_add(1).ok_or("occurrence overflow")?)?,
                pitch: 48_u8
                    .checked_add(u8::try_from(index)?)
                    .ok_or("pitch overflow")?,
                velocity: 100,
            })
        })
        .collect()
}
fn render(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    output: &mut [StereoFrame],
    resources: &BorrowedDeviceResources<'_>,
) -> Result<()> {
    pair.begin_block();
    pair.process_block(events, output, resources)?;
    pair.commit_block();
    Ok(())
}
fn validate_pcm(pcm: &[StereoFrame]) -> Result<()> {
    let mut audible = false;
    for sample in pcm.iter().flatten() {
        if !sample.is_finite() {
            return Err("non-finite rendered pcm".into());
        }
        audible |= *sample != 0.0;
    }
    if !audible {
        return Err("rendered workload is silent".into());
    }
    Ok(())
}
fn hash(checksum: &mut u64, frames: &[StereoFrame]) {
    for sample in frames.iter().flatten() {
        *checksum = (*checksum ^ u64::from(sample.to_bits())).wrapping_mul(0x0000_0100_0000_01b3);
    }
}
fn oracle(
    factory: &dyn PreparedMutableInstrumentRuntime,
    options: &Options,
    events: &[NativeInstrumentEvent],
    resources: &BorrowedDeviceResources<'_>,
) -> Result<()> {
    let mut whole = NativeInstrumentRuntimePair::new(factory, limits(options.voices))?;
    let mut split = NativeInstrumentRuntimePair::new(factory, limits(options.voices))?;
    let mut expected = vec![[0.0; 2]; ORACLE_FRAMES];
    let mut actual = vec![[0.0; 2]; ORACLE_FRAMES];
    render(&mut whole, events, &mut expected, resources)?;
    let partitions = [1, 63, 257, 128];
    let mut offset = 0;
    let mut part = 0;
    while offset < actual.len() {
        let width = partitions[part % partitions.len()].min(actual.len() - offset);
        let end = offset.checked_add(width).ok_or("oracle offset overflow")?;
        render(
            &mut split,
            if offset == 0 { events } else { &[] },
            &mut actual[offset..end],
            resources,
        )?;
        offset = end;
        part += 1;
    }
    validate_pcm(&expected)?;
    validate_pcm(&actual)?;
    if expected != actual {
        return Err("mutable sampler pcm differs across block partitions".into());
    }
    let mut checksum = FNV_OFFSET;
    hash(&mut checksum, &expected);
    println!(
        "partition_pcm_check=pass frames={ORACLE_FRAMES} partitions=whole,1/63/257/128 checksum={checksum:016x}"
    );
    Ok(())
}
fn percentile(sorted: &[u128], percent: usize) -> Result<u128> {
    let rank = sorted
        .len()
        .checked_mul(percent)
        .ok_or("percentile overflow")?
        .div_ceil(100)
        .max(1);
    sorted
        .get(rank - 1)
        .copied()
        .ok_or_else(|| "missing percentile sample".into())
}
fn main() -> Result<()> {
    let Some(options) = options()? else {
        return Ok(());
    };
    let pcm: Vec<StereoFrame> = (0..RATE)
        .map(|frame| {
            [
                ((frame * 73 + 19) % 4093) as f32 / 4096.0 - 0.5,
                ((frame * 151 + 23) % 8191) as f32 / 8192.0 - 0.5,
            ]
        })
        .collect();
    let slot = DeviceAssetSlot::new(1)?;
    let asset = DeviceAssetKey::new(1)?;
    let views = [DeviceResourceView {
        slot,
        identity: DeviceResourceIdentity {
            asset,
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: RATE,
        pcm: DeviceResourcePcm::Stereo(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);
    let mut authored = SamplerState::for_source_frames(pcm.len())?;
    authored.mode = AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: Region {
            start: 137,
            end: pcm.len() - 97,
        },
    };
    authored.detune_cents = 7.0;
    authored.voices = u8::try_from(options.voices)?;
    let state = authored
        .to_native()?
        .with_asset_references(vec![DeviceAssetReference { slot, asset }])?;
    let registry = hex_devices::native_instrument_registry()?;
    let tuning = TuningTable::default();
    let prepared = registry.prepare_instrument_with_runtime_limits(
        &state,
        &DevicePreparationContext {
            sample_rate: RATE,
            tuning: &tuning,
            resources: &resources,
        },
        limits(options.voices),
    )?;
    let factory = prepared
        .prepare_mutable_runtime()?
        .ok_or("registered sampler has no mutable runtime")?;
    let onsets = events(options.voices)?;
    oracle(factory.as_ref(), &options, &onsets, &resources)?;
    let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits(options.voices))?;
    println!(
        "profile={} provider=registered_mutable_sampler sample_rate={RATE} channels=2 voices={} block_frames={} warmup_blocks={} measured_blocks={} pcm_frames={} loop=137..{} detune_cents=7 declared_pair_bytes={} actual_pair_bytes={:?} onset_frames=1 scope=offline_pair_begin_preflight_process_commit",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        options.voices,
        options.block,
        options.warmup,
        options.measured,
        pcm.len(),
        pcm.len() - 97,
        pair.declared_pair_bytes(),
        pair.fixed_storage_pair_bytes()
    );
    render(&mut pair, &onsets, &mut [[0.0; 2]; 1], &resources)?;
    let mut output = vec![[0.0; 2]; options.block];
    for _ in 0..options.warmup {
        output.fill([0.0; 2]);
        render(&mut pair, &[], &mut output, &resources)?;
    }
    let mut timings = vec![0_u128; options.measured];
    let mut checksum = FNV_OFFSET;
    for duration in &mut timings {
        output.fill([0.0; 2]);
        let started = Instant::now();
        pair.begin_block();
        pair.process_block(&[], black_box(&mut output), &resources)?;
        pair.commit_block();
        *duration = started.elapsed().as_nanos();
        validate_pcm(&output)?;
        hash(&mut checksum, black_box(&output));
    }
    let mut identities = [0; 32];
    if pair.active_occurrence_ids(&mut identities)? != options.voices {
        return Err("held voice count changed".into());
    }
    timings.sort_unstable();
    let maximum = timings.last().copied().ok_or("missing timing sample")?;
    println!(
        "steady_pair: n={} p50_ns={} p95_ns={} p99_ns={} max_ns={} checksum={checksum:016x}",
        timings.len(),
        percentile(&timings, 50)?,
        percentile(&timings, 95)?,
        percentile(&timings, 99)?,
        maximum
    );
    Ok(())
}
