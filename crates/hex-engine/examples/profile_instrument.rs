//! Release CPU benchmark for shared original MIDI instrument rendering.
//! This measures the offline renderer, not an audio device, UI, or audible acceptance.
use hex_engine::{
    compile,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice::PreparedAnalyticNote;
use hex_project::{
    BeatTime, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, Project, Snapshot, Tempo,
    TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use std::{error::Error, hint::black_box, time::Instant};

const RATE: u32 = 48_000;
const TRACKS: usize = 100;
const NOTES_PER_TRACK: usize = 16;
const WARMUP_BLOCKS: usize = 100;
const MEASURED_BLOCKS: usize = 3_000;

fn fixture() -> Result<Project, hex_project::Invalid> {
    let mut next_id = 1_u64;
    let mut tracks = Vec::with_capacity(TRACKS);
    let two_quarters = BeatTime::from_ratio(2, 1)?.raw();
    for track_index in 0..TRACKS {
        let track_id = next_id;
        next_id += 1;
        let instrument_id = next_id;
        next_id += 1;
        let clip_id = next_id;
        next_id += 1;
        let notes = (0..NOTES_PER_TRACK)
            .map(|note_index| {
                let id = next_id;
                next_id += 1;
                MidiNoteSnapshot {
                    id,
                    start_raw: ((note_index * 4) as u64) << 32,
                    duration_raw: two_quarters,
                    pitch: 48 + ((track_index + note_index) % 36) as u8,
                    channel: 0,
                    velocity: 100,
                }
            })
            .collect();
        tracks.push(TrackSnapshot {
            id: track_id,
            name: format!("instrument {track_index:03}"),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: Some(hex_project::DeviceSnapshot {
                id: instrument_id,
                bypassed: false,
                state: hex_project::default_instrument_state()?,
            }),
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                id: clip_id,
                start_raw: 0,
                length_raw: 64_u64 << 32,
                source_length_raw: 64_u64 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 64_u64 << 32,
                looping: false,
                notes,
            }]),
        });
    }
    Project::from_snapshot(Snapshot {
        sample_rate: RATE,
        next_id,
        assets: Vec::new(),
        tracks,
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
    })
}

fn measure(project: &hex_engine::Project, block_size: usize) -> Result<(), hex_engine::Invalid> {
    let mut playback = Playback::new(project.clone());
    playback.apply(PlaybackCommand::Play);
    let mut output = vec![[0.0; 2]; block_size];
    for _ in 0..WARMUP_BLOCKS {
        playback.render(black_box(&mut output))?;
    }

    let mut timings = Vec::with_capacity(MEASURED_BLOCKS);
    let started = Instant::now();
    for _ in 0..MEASURED_BLOCKS {
        let block_started = Instant::now();
        playback.render(black_box(&mut output))?;
        timings.push(block_started.elapsed().as_nanos());
        black_box(&output);
    }
    let elapsed = started.elapsed();
    timings.sort_unstable();
    let total_frames = block_size * MEASURED_BLOCKS;
    let deadline_us = block_size as f64 / f64::from(RATE) * 1_000_000.0;
    println!(
        "block={block_size:3} frames total={total_frames} elapsed={:.3}s max_block={:.1}us audio_deadline={deadline_us:.1}us max/deadline={:.2}x",
        elapsed.as_secs_f64(),
        timings[timings.len() - 1] as f64 / 1_000.0,
        timings[timings.len() - 1] as f64 / (deadline_us * 1_000.0),
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let editable = fixture()?;
    let project = compile::project(&editable, &Default::default())?;
    let sampler_references = project.midi_note_sampler_reference_count()?;
    let old_sampler_bytes = sampler_references
        .checked_mul(std::mem::size_of::<PreparedAnalyticNote>())
        .ok_or("old sampler byte estimate overflow")?;
    let indexed_bytes = project
        .midi_note_bank_storage_bytes()?
        .checked_add(
            sampler_references
                .checked_mul(std::mem::size_of::<u16>())
                .ok_or("indexed sampler reference byte count overflow")?,
        )
        .ok_or("indexed sampler byte estimate overflow")?;
    println!(
        "note storage estimate: refs={} indexed_bytes={} previous_inline_sampler_bytes={} (bank entries include keys and owner overhead)",
        sampler_references, indexed_bytes, old_sampler_bytes
    );
    for block_size in [128, 512] {
        measure(&project, block_size)?;
    }
    Ok(())
}
