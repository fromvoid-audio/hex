//! Profile canonical edit and control-side compilation for a dense arrangement.
//! This excludes fixture construction, PCM decoding, GPUI, queues, and audio devices.
//! `--return-feed` adds two silent Return buses and a feed to exercise routing
//! admission with the same clip workload/output oracle; this does not benchmark
//! audible Return mixing.
use hex_engine::{Frame, Renderer, compile};
use hex_project::{
    AssetSnapshot, ClipSnapshot, Command, Created, Editor, IDENTITY_CONVERSION, Project, Snapshot,
    TrackSnapshot,
};
use std::{collections::BTreeMap, error::Error, hint::black_box, path::PathBuf, time::Instant};

const TRACKS: usize = 100;
const CLIPS_PER_TRACK: usize = 100;
const BLOCK_FRAMES: usize = 128;
const SOURCE_FRAMES: usize = 64;
const WARMUP: usize = 10;
const ITERATIONS: usize = 200;
const RATE: u32 = 48_000;
const ARRANGEMENT_FRAMES: u64 = 500_000;

fn fixture() -> Result<Project, Box<dyn Error>> {
    let mut clips_total = 0_u64;
    let tracks = (0..TRACKS)
        .map(|track_index| {
            let id = 2 + track_index as u64;
            let clips = (0..CLIPS_PER_TRACK)
                .map(|clip_index| -> Result<ClipSnapshot, hex_project::Invalid> {
                    let clip_id = 102 + clips_total;
                    clips_total += 1;
                    let active = clip_index == 0;
                    let start = if active {
                        0
                    } else {
                        // Canonical Arrangement clips cannot overlap on a track.
                        // Keep sparse clips after the sustained loop, spaced
                        // beyond their maximum 64-frame duration.
                        ARRANGEMENT_FRAMES + clip_index as u64 * 128
                    };
                    let duration = if active {
                        ARRANGEMENT_FRAMES
                    } else {
                        match clip_index % 3 {
                            0 => 16,
                            1 => 32,
                            _ => 64,
                        }
                    };
                    Ok(ClipSnapshot {
                        active: true,
                        name: "clip".into(),
                        flavor: hex_project::ClipFlavor::default(),
                        edit: None,
                        gain: hex_project::ClipGain::UNITY,
                        id: clip_id,
                        asset: 1,
                        start,
                        duration,
                        source_offset: 0,
                        source_length: SOURCE_FRAMES as u64,
                        looping: active || (track_index + clip_index) % 3 == 0,
                        source_phase: 0,
                        fade: hex_project::ClipFade::neutral(duration)?,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, hex_project::Invalid>(TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id,
                name: format!("track {track_index:03}"),
                gain: 0.5 + (track_index % 5) as f32 * 0.1,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: hex_project::TrackContentSnapshot::Audio(clips),
                sends: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, hex_project::Invalid>>()?;
    let snapshot = Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: RATE,
        sample_rate_mode: Default::default(),
        next_id: 102 + clips_total,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: 1,
            path: PathBuf::from("profile-source.wav"),
            source_sample_rate: RATE,
            source_frames: SOURCE_FRAMES as u64,
            playback_frames: SOURCE_FRAMES as u64,
            conversion_version: IDENTITY_CONVERSION,
        }],
        tracks,
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![hex_project::TempoPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Tempo::DEFAULT,
        )],
        meter_points: vec![hex_project::MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
        return_tracks: Vec::new(),
    };
    Ok(Project::from_snapshot(snapshot)?)
}

fn source() -> Vec<Frame> {
    (0..SOURCE_FRAMES)
        .map(|frame| {
            let left = ((frame * 11) % 97) as f32 / 128.0 - 0.25;
            let right = ((frame * 19) % 89) as f32 / 128.0 - 0.2;
            [left, right]
        })
        .collect()
}

fn digest(frames: &[Frame]) -> u64 {
    frames.iter().fold(0xcbf2_9ce4_8422_2325, |hash, frame| {
        let hash = (hash ^ u64::from(frame[0].to_bits())).wrapping_mul(0x100_0000_01b3);
        (hash ^ u64::from(frame[1].to_bits())).wrapping_mul(0x100_0000_01b3)
    })
}

fn reference_render(output: &mut [Frame]) {
    let source = source();
    output.fill([0.0; 2]);
    for (frame_index, frame) in output.iter_mut().enumerate() {
        for track_index in 0..TRACKS {
            let gain = 0.5 + (track_index % 5) as f32 * 0.1;
            let sample = source[frame_index % SOURCE_FRAMES];
            frame[0] += sample[0] * gain;
            frame[1] += sample[1] * gain;
        }
    }
}

fn report(label: &str, samples: &mut [u128]) {
    samples.sort_unstable();
    let percentile = |p: usize| samples[(samples.len() - 1) * p / 100] as f64 / 1_000.0;
    println!(
        "{label}: median={:.2}µs p95={:.2}µs p99={:.2}µs max={:.2}µs",
        percentile(50),
        percentile(95),
        percentile(99),
        samples[samples.len() - 1] as f64 / 1_000.0,
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let routed = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--return-feed" => true,
        _ => return Err("usage: profile_compile [--return-feed]".into()),
    };
    let project = fixture()?;
    let mut editor = Editor::new(project, 32)?;
    if routed {
        let mut returns = Vec::new();
        for name in ["first", "second"] {
            let Created::ReturnTrack(id) =
                editor.execute(Command::AddReturnTrack { name: name.into() })?
            else {
                return Err("expected fixture return".into());
            };
            returns.push(id);
        }
        editor.execute(Command::SetReturnFeed {
            source: returns[0],
            destination: returns[1],
            gain: 0.5,
        })?;
    }
    let media_id = editor
        .project()
        .assets()
        .next()
        .ok_or("fixture asset missing")?
        .id();
    let mut media = BTreeMap::new();
    media.insert(media_id, hex_engine::Audio::new(RATE, source())?);
    let track = editor
        .project()
        .tracks()
        .first()
        .ok_or("fixture track missing")?
        .id();
    let clip = editor
        .project()
        .tracks()
        .first()
        .and_then(|item| item.clips().first())
        .ok_or("fixture clip missing")?
        .id();

    let run_mutations = |editor: &mut Editor, iteration: usize| -> Result<(), Box<dyn Error>> {
        let gain = if iteration.is_multiple_of(2) {
            0.75
        } else {
            0.5
        };
        let start = if iteration.is_multiple_of(2) { 1 } else { 0 };
        editor.execute(Command::Gain { track, gain })?;
        editor.execute(Command::MoveClip { clip, track, start })?;
        Ok(())
    };
    for iteration in 0..WARMUP {
        run_mutations(&mut editor, iteration)?;
        black_box(compile::project(editor.project(), &media)?);
    }

    let mut edit_times = Vec::with_capacity(ITERATIONS);
    let mut compile_times = Vec::with_capacity(ITERATIONS);
    for iteration in 0..ITERATIONS {
        let started = Instant::now();
        run_mutations(&mut editor, iteration + WARMUP)?;
        black_box(editor.project());
        edit_times.push(started.elapsed().as_nanos());

        let started = Instant::now();
        let compiled = compile::project(editor.project(), &media)?;
        black_box(&compiled);
        compile_times.push(started.elapsed().as_nanos());
        if iteration + 1 == ITERATIONS {
            let mut renderer = Renderer::new(&compiled);
            renderer.play();
            let mut actual = vec![[0.0; 2]; BLOCK_FRAMES];
            let mut expected = vec![[0.0; 2]; BLOCK_FRAMES];
            renderer.render(&mut actual)?;
            reference_render(&mut expected);
            if actual != expected {
                return Err("compiled render differs from known fixture".into());
            }
            println!("compiled render digest: {:016x}", digest(&actual));
        }
    }

    println!(
        "compile profile: {TRACKS} tracks × {CLIPS_PER_TRACK} clips (100 active sustained, 9,900 sparse), one shared {SOURCE_FRAMES}-frame stereo asset at {RATE} hz, history capacity 32; {WARMUP} warmup + {ITERATIONS} measured; each edit sample is one gain change plus one clip move; fixture construction and media generation excluded; device/GPUI/queue timing excluded; {} {}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    println!("return feed mode: {routed}; returns receive no source signal");
    report("canonical edits", &mut edit_times);
    report("control-side compile", &mut compile_times);
    Ok(())
}
