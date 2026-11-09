//! Release CPU benchmark for shared original MIDI instrument rendering.
//! This measures the offline renderer, not an audio device, UI, or audible acceptance.
#[cfg(feature = "instrument-test-support")]
#[path = "profile_instrument/fm4_backend.rs"]
mod fm4_backend;

use hex_engine::{
    PreparedRevision, compile,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{
    BeatTime, DeviceSnapshot, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, Project,
    Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const RATE: u32 = 48_000;
const TRACKS: usize = 100;
const NOTES_PER_TRACK: usize = 16;
const PREVIOUS_INLINE_SAMPLER_BYTES: usize = 40;
const WARMUP_BLOCKS: usize = 100;
const MEASURED_BLOCKS: usize = 3_000;

#[derive(Clone, Copy)]
struct Workload {
    instrument_position: usize,
    fm4: bool,
    operator: bool,
    four_stage_fm4: bool,
    dense_sustain: bool,
    one_note: bool,
}

fn fixture(workload: Workload) -> Result<Project, Box<dyn Error>> {
    let mut next_id = 1_u64;
    let mut tracks = Vec::with_capacity(TRACKS);
    let two_quarters = BeatTime::from_ratio(2, 1)?.raw();
    let note_count = if workload.one_note {
        1
    } else {
        NOTES_PER_TRACK
    };
    let note_duration = if workload.dense_sustain {
        BeatTime::from_ratio(120, 1)?.raw()
    } else {
        two_quarters
    };
    let clip_length = if workload.dense_sustain {
        BeatTime::from_ratio(128, 1)?.raw()
    } else {
        64_u64 << 32
    };
    for track_index in 0..TRACKS {
        let track_id = next_id;
        next_id += 1;
        let instrument_id = next_id;
        next_id += 1;
        let prefix_id = next_id;
        next_id += 1;
        let suffix_id = next_id;
        next_id += 1;
        let clip_id = next_id;
        next_id += 1;
        let utility_state =
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))?;
        let utility_state = hex_engine::patch_native_device_parameter(
            &utility_state,
            ParameterId::from_static(1),
            ParameterValue::Float(0.999),
        )?;
        let notes = (0..note_count)
            .map(|note_index| {
                let id = next_id;
                next_id += 1;
                MidiNoteSnapshot {
                    id,
                    active: true,
                    start_raw: if workload.dense_sustain {
                        0
                    } else {
                        ((note_index * 4) as u64) << 32
                    },
                    duration_raw: note_duration,
                    pitch: 48 + ((track_index + note_index) % 36) as u8,
                    channel: 0,
                    velocity: 100,
                }
            })
            .collect();
        tracks.push(TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: format!("instrument {track_index:03}"),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: vec![
                DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: prefix_id,
                    bypassed: false,
                    state: utility_state.clone(),
                },
                DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: suffix_id,
                    bypassed: false,
                    state: utility_state,
                },
            ],
            instrument: Some(hex_project::DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: instrument_id,
                bypassed: false,
                state: if workload.operator {
                    hex_engine::default_native_device_state_for_class(
                        hex_engine::NativeDeviceClass::Instrument,
                        DeviceTypeId::from_static("hex.instrument.operator"),
                    )?
                } else if workload.fm4 {
                    let mut state = hex_engine::default_native_device_state_for_class(
                        hex_engine::NativeDeviceClass::Instrument,
                        DeviceTypeId::from_static("hex.instrument.fm4"),
                    )?;
                    if workload.four_stage_fm4 {
                        for (parameter, value) in [
                            (14, 1.0), // operator 1 modulates operator 2
                            (19, 1.0), // operator 2 modulates operator 3
                            (24, 1.0), // operator 3 modulates operator 4
                            (29, 0.0), // carrier 1 is silent
                            (32, 1.0), // operator 4 is the audible carrier
                        ] {
                            state = hex_engine::patch_native_device_parameter_for_class(
                                hex_engine::NativeDeviceClass::Instrument,
                                &state,
                                ParameterId::from_static(parameter),
                                ParameterValue::Float(value),
                            )?;
                        }
                    }
                    state
                } else {
                    hex_project::default_instrument_state()?
                },
            }),
            instrument_position: Some(workload.instrument_position),
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: clip_id,
                start_raw: 0,
                length_raw: clip_length,
                source_length_raw: clip_length,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: clip_length,
                looping: false,
                notes,
            }]),
            sends: Vec::new(),
        });
    }
    Ok(Project::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: RATE,
        sample_rate_mode: Default::default(),
        next_id,
        assets: Vec::new(),
        tracks,
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })?)
}

fn measure(
    project: &hex_engine::Project,
    prepared: Option<&Arc<PreparedRevision>>,
    force_session: bool,
    block_size: usize,
    warmup_blocks: usize,
    measured_blocks: usize,
    pcm_directory: Option<&std::path::Path>,
) -> Result<(), Box<dyn Error>> {
    let construction_started = Instant::now();
    let mut playback = match prepared {
        Some(revision) if force_session => {
            Playback::try_from_prepared_with_session(Arc::clone(revision))?
        }
        Some(revision) => Playback::from_prepared(Arc::clone(revision)),
        None => Playback::new(project.clone()),
    };
    println!(
        "playback_construction_elapsed={:.6}s",
        construction_started.elapsed().as_secs_f64()
    );
    playback.apply(PlaybackCommand::Play);
    let mut output = vec![[0.0; 2]; block_size];
    let sample_count = block_size
        .checked_mul(measured_blocks)
        .ok_or("profile sample count overflow")?;
    let mut complete_run = Vec::with_capacity(sample_count);
    for _ in 0..warmup_blocks {
        playback.render(black_box(&mut output))?;
    }

    let mut timings = Vec::with_capacity(measured_blocks);
    let started = Instant::now();
    for _ in 0..measured_blocks {
        let block_started = Instant::now();
        playback.render(black_box(&mut output))?;
        timings.push(block_started.elapsed().as_nanos());
        complete_run.extend_from_slice(&output);
        black_box(&output);
    }
    let render_elapsed_nanos = timings.iter().sum::<u128>();
    let whole_run_digest = complete_run
        .iter()
        .flatten()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, sample| {
            (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0000_0100_0000_01b3)
        });
    let elapsed_including_collection_and_hash = started.elapsed();
    timings.sort_unstable();
    let final_block_digest = output
        .iter()
        .flatten()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, sample| {
            (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0000_0100_0000_01b3)
        });
    let total_frames = block_size * measured_blocks;
    let deadline_us = block_size as f64 / f64::from(RATE) * 1_000_000.0;
    println!(
        "block={block_size:3} frames total={total_frames} render_sum={:.6}s elapsed_with_collection_and_hash={:.6}s max_block={:.1}us audio_deadline={deadline_us:.1}us max/deadline={:.2}x final_block_digest={final_block_digest:016x} whole_run_digest={whole_run_digest:016x}",
        render_elapsed_nanos as f64 / 1_000_000_000.0,
        elapsed_including_collection_and_hash.as_secs_f64(),
        timings[timings.len() - 1] as f64 / 1_000.0,
        timings[timings.len() - 1] as f64 / (deadline_us * 1_000.0),
    );
    if let Some(directory) = pcm_directory {
        use std::io::Write;
        std::fs::create_dir_all(directory)?;
        let mut file = std::io::BufWriter::new(std::fs::File::create(
            directory.join(format!("block-{block_size}.f32")),
        )?);
        for sample in complete_run.iter().flatten() {
            file.write_all(&sample.to_le_bytes())?;
        }
        file.flush()?;
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    let pcm_directory = arguments
        .iter()
        .find_map(|argument| argument.strip_prefix("--pcm-directory="))
        .map(std::path::Path::new);
    let prepared_runtime = arguments
        .iter()
        .any(|argument| argument == "--prepared-runtime");
    let forced_session_runtime = arguments
        .iter()
        .any(|argument| argument == "--session-runtime");
    let backend = arguments
        .iter()
        .find_map(|argument| argument.strip_prefix("--fm4-backend="));
    if backend.is_some_and(|backend| !matches!(backend, "inline" | "mutable")) {
        return Err("--fm4-backend requires inline or mutable".into());
    }
    if backend.is_some()
        && !(prepared_runtime
            && forced_session_runtime
            && arguments.iter().any(|argument| argument == "--fm4"))
    {
        return Err("--fm4-backend requires --fm4 --prepared-runtime --session-runtime".into());
    }
    #[cfg(not(feature = "instrument-test-support"))]
    if backend.is_some() {
        return Err("--fm4-backend requires instrument-test-support".into());
    }
    if let Some(backend) = backend {
        println!("fm4_backend={backend}");
    }
    let instrument_position = if arguments.iter().any(|argument| argument == "--gap-end") {
        2
    } else if arguments.iter().any(|argument| argument == "--ordered-gap") {
        1
    } else {
        0
    };
    let workload = Workload {
        instrument_position,
        fm4: arguments.iter().any(|argument| argument == "--fm4"),
        operator: arguments.iter().any(|argument| argument == "--operator"),
        four_stage_fm4: arguments
            .iter()
            .any(|argument| argument == "--four-stage-fm4"),
        dense_sustain: arguments
            .iter()
            .any(|argument| argument == "--dense-sustain"),
        one_note: arguments.iter().any(|argument| argument == "--one-note"),
    };
    if workload.four_stage_fm4 && !workload.fm4 {
        return Err("--four-stage-fm4 requires --fm4".into());
    }
    if workload.operator && workload.fm4 {
        return Err("--operator cannot be combined with --fm4 or --four-stage-fm4".into());
    }
    let short_profile = arguments.iter().any(|argument| argument == "--short");
    let warmup_blocks = parse_profile_count(
        &arguments,
        "--warmup-blocks=",
        if short_profile { 10 } else { WARMUP_BLOCKS },
    )?;
    let measured_blocks = parse_profile_count(
        &arguments,
        "--measured-blocks=",
        if short_profile { 100 } else { MEASURED_BLOCKS },
    )?;
    println!("instrument position: {instrument_position}");
    println!(
        "instrument={} four_stage_fm4={} notes_per_track={} dense_sustain={} warmup_blocks={} measured_blocks={}",
        if workload.operator {
            "vector"
        } else if workload.fm4 {
            "fm4"
        } else {
            "sine"
        },
        workload.four_stage_fm4,
        if workload.one_note {
            1
        } else {
            NOTES_PER_TRACK
        },
        workload.dense_sustain,
        warmup_blocks,
        measured_blocks,
    );
    let editable = fixture(workload)?;
    let project = compile::project(&editable, &Default::default())?;
    let prepare_started = Instant::now();
    let prepared = prepared_runtime
        .then(|| {
            #[cfg(feature = "instrument-test-support")]
            if let Some(backend) = backend {
                return fm4_backend::prepare(&editable, backend == "mutable")
                    .map(Arc::new)
                    .map_err(|error| error.to_string());
            }
            PreparedRevision::prepare(
                &editable,
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .map(Arc::new)
            .map_err(|error| error.to_string())
        })
        .transpose()?;
    if let Some(revision) = &prepared {
        let arrangement_bank_bytes = revision.arrangement().midi_note_bank_storage_bytes()?;
        let session_bank_bytes = revision.session().midi_note_bank_storage_bytes();
        let bank_bytes = arrangement_bank_bytes
            .checked_add(session_bank_bytes)
            .ok_or("prepared bank byte count overflow")?;
        let references = project
            .midi_note_sampler_reference_count()?
            .checked_add(revision.session().midi_note_bank_reference_count())
            .ok_or("prepared bank reference count overflow")?;
        let indexed_bytes = bank_bytes
            .checked_add(
                references
                    .checked_mul(std::mem::size_of::<u16>())
                    .ok_or("prepared bank index byte count overflow")?,
            )
            .ok_or("prepared bank component byte count overflow")?;
        println!(
            "prepared_revision_construction_elapsed={:.6}s bank_component_bytes={} note_references={} indexed_bank_component_bytes={}",
            prepare_started.elapsed().as_secs_f64(),
            bank_bytes,
            references,
            indexed_bytes
        );
    }
    println!(
        "runtime: {}",
        if forced_session_runtime {
            "explicit SessionPlayback::new wrapper"
        } else if prepared_runtime {
            "ordinary prepared Playback::from_prepared"
        } else {
            "legacy Playback::new"
        }
    );
    let sampler_references = project.midi_note_sampler_reference_count()?;
    let old_sampler_bytes = sampler_references
        .checked_mul(PREVIOUS_INLINE_SAMPLER_BYTES)
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
        measure(
            &project,
            prepared.as_ref(),
            forced_session_runtime,
            block_size,
            warmup_blocks,
            measured_blocks,
            pcm_directory,
        )?;
    }
    Ok(())
}

fn parse_profile_count(
    arguments: &[String],
    prefix: &str,
    default: usize,
) -> Result<usize, Box<dyn Error>> {
    let Some(argument) = arguments
        .iter()
        .find(|argument| argument.starts_with(prefix))
    else {
        return Ok(default);
    };
    let count = argument[prefix.len()..].parse::<usize>()?;
    if count == 0 || count > 10_000 {
        return Err(format!("{prefix} value must be between 1 and 10000").into());
    }
    Ok(count)
}
