//! Offline scene-cell scaling probe; direct actions, no callback queue or device.
use hex_engine::{
    Audio, Frame, LaunchQuantization, PlaybackAction, PreparedRevision, SessionPlaybackCommand,
    SineTable, playback::Playback,
};
use hex_project::{
    AssetSnapshot, BeatTime, Meter, MeterPoint, Project, SceneId, SceneSnapshot,
    SessionAudioClipSnapshot, SessionCellContentSnapshot, SessionCellSnapshot, SessionSnapshot,
    Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use std::{
    collections::BTreeMap, error::Error, hint::black_box, path::PathBuf, sync::Arc, time::Instant,
};

const RATE: u32 = 48_000;
const TRACKS: usize = 256;
const BURST: usize = 32;
const SAMPLES: usize = 1_000;
const WARMUPS: usize = 20;

fn main() -> Result<(), Box<dyn Error>> {
    for scenes in [2, 256] {
        profile(scenes)?;
    }
    println!(
        "offline direct actions only; fixture/preparation/render/oracle/drops excluded; 32 actions models the native admission bound but does not exercise its queue; no physical deadline claim"
    );
    Ok(())
}

fn fixture(
    scene_count: usize,
) -> Result<(Arc<PreparedRevision>, SceneId, SceneId), Box<dyn Error>> {
    let mut next_id = 2_u64;
    let mut scenes = Vec::with_capacity(scene_count);
    for index in 0..scene_count {
        scenes.push(SceneSnapshot {
            id: next_id,
            name: format!("scene {index}"),
        });
        next_id += 1;
    }
    let mut tracks = Vec::with_capacity(TRACKS);
    let mut cells = Vec::with_capacity(TRACKS * scene_count);
    // Store cells in track-major order so the selected scene is not contiguous.
    for index in 0..TRACKS {
        let track = next_id;
        next_id += 1;
        tracks.push(TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track,
            name: format!("audio {index}"),
            gain: 1.0 / 512.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(Vec::new()),
            sends: Vec::new(),
        });
        for (scene_index, scene) in scenes.iter().enumerate() {
            let content = if scene_index == 0 {
                let id = next_id;
                next_id += 1;
                SessionCellContentSnapshot::Audio(SessionAudioClipSnapshot {
                    loop_range: None,
                    active: true,
                    flavor: hex_project::ClipFlavor::default(),
                    gain: hex_project::ClipGain::UNITY,
                    id,
                    name: format!("source {index}"),
                    asset: 1,
                    source_start: 0,
                    source_end: 32,
                })
            } else {
                SessionCellContentSnapshot::Stop
            };
            cells.push(SessionCellSnapshot {
                track,
                scene: scene.id,
                content,
            });
        }
    }
    let project = Project::from_legacy_snapshot(Snapshot {
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
        next_id,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: 1,
            path: PathBuf::from("memory/grid.wav"),
            source_sample_rate: RATE,
            source_frames: 32,
            playback_frames: 32,
            conversion_version: 0,
        }],
        tracks,
        session: SessionSnapshot { scenes, cells },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })?;
    let asset = project.assets().next().ok_or("missing fixture asset")?.id();
    let source = (0..32)
        .map(|index| [index as f32 / 64.0, -(index as f32) / 64.0])
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(RATE, source)?)]);
    let first = project.scenes().first().ok_or("missing first scene")?.id();
    let last = project.scenes().last().ok_or("missing last scene")?.id();
    let revision = Arc::new(PreparedRevision::prepare(
        &project,
        &media,
        Arc::new(SineTable::new()),
        None,
    )?);
    Ok((revision, first, last))
}

fn launch(playback: &mut Playback, scene: SceneId) -> Result<(), Box<dyn Error>> {
    playback
        .try_apply_action(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    Ok(())
}

fn verify(playback: &mut Playback, audio: bool) -> Result<(), Box<dyn Error>> {
    let mut output = [[0.0; 2]; 7];
    playback.render(&mut output)?;
    for (index, actual) in output.iter().enumerate() {
        let expected: Frame = if audio {
            [index as f32 / 128.0, -(index as f32) / 128.0]
        } else {
            [0.0; 2]
        };
        if *actual != expected {
            return Err(
                format!("scene PCM mismatch at {index}: {actual:?} != {expected:?}").into(),
            );
        }
    }
    if playback.session_route_count() != TRACKS {
        return Err("route count mismatch".into());
    }
    Ok(())
}

fn profile(scenes: usize) -> Result<(), Box<dyn Error>> {
    let (revision, audio, stop) = fixture(scenes)?;
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    // Independent exact oracle confirms both selected scenes, at nonzero anchors.
    launch(&mut playback, audio)?;
    verify(&mut playback, true)?;
    launch(&mut playback, stop)?;
    verify(&mut playback, false)?;
    launch(&mut playback, audio)?;
    verify(&mut playback, true)?;
    println!(
        "grid: tracks={TRACKS} scenes={scenes} cells={} audio_clips={TRACKS} rate={RATE} burst={BURST} arch={} os={} host_metadata=external_run_metadata oracle=passed",
        revision.session().cells().len(),
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    for width in [1, BURST] {
        let mut times = Vec::with_capacity(SAMPLES);
        let mut output = [[0.0; 2]; 7];
        for sample in 0..(SAMPLES + WARMUPS) {
            let started = Instant::now();
            for index in 0..width {
                let scene = if (sample + index) % 2 == 0 {
                    audio
                } else {
                    stop
                };
                launch(&mut playback, scene)?;
            }
            let nanos = started.elapsed().as_nanos();
            if sample >= WARMUPS {
                times.push(nanos);
            }
            playback.render(&mut output)?;
            black_box(&output);
        }
        times.sort_unstable();
        let percentile =
            |percent: usize| times[(times.len() * percent).div_ceil(100) - 1] as f64 / 1_000.0;
        let exceeds = times
            .iter()
            .filter(|nanos| **nanos * u128::from(RATE) > 128 * 1_000_000_000)
            .count();
        println!(
            "scene_actions: width={width} samples={SAMPLES} warmups={WARMUPS} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} nominal_128_frame_period_exceed={exceeds}/{SAMPLES}",
            percentile(50),
            percentile(95),
            percentile(99),
            *times.last().ok_or("empty timing series")? as f64 / 1_000.0
        );
    }
    Ok(())
}
