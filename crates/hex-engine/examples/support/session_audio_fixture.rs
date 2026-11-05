//! Deterministic original PCM fixture and independent direct-frame oracles.
use hex_engine::{Audio, Frame};
use hex_project::{
    AssetId, AssetSnapshot, BeatTime, ClipFade, ClipId, ClipSnapshot, Meter, MeterPoint, Project,
    SceneId, SceneSnapshot, SessionAudioClipSnapshot, SessionCellContent,
    SessionCellContentSnapshot, SessionCellSnapshot, SessionSnapshot, Snapshot, Tempo, TempoPoint,
    TrackContentSnapshot, TrackId, TrackSnapshot,
};
use std::{collections::BTreeMap, error::Error, path::PathBuf};

pub const RATE: u32 = 48_000;
pub const SOURCE_FRAMES: usize = 65_536;
pub const WINDOW_A: (u64, u64) = (1_024, 5_120);
pub const WINDOW_B: (u64, u64) = (8_192, 12_288);
pub const ARRANGEMENT_WINDOW: (u64, u64) = (16_384, 24_576);
pub const SCENE_A: u64 = 1;
pub const SCENE_B: u64 = 2;
pub const STOP_SCENE: u64 = 3;

pub struct Fixture {
    pub project: Project,
    pub media: BTreeMap<AssetId, Audio>,
    pub tracks: Vec<TrackId>,
    pub scene_a_clips: Vec<ClipId>,
    pub scene_b_clips: Vec<ClipId>,
    pub scene_a: SceneId,
    pub scene_b: SceneId,
    pub stop_scene: SceneId,
}

pub fn fixture(distinct_assets: bool) -> Result<Fixture, Box<dyn Error>> {
    let asset_count = if distinct_assets { 100 } else { 1 };
    let mut next_id = 4_u64;
    let mut assets = Vec::with_capacity(asset_count);
    let mut pcm = Vec::with_capacity(asset_count);
    for index in 0..asset_count {
        let id = next_id;
        next_id += 1;
        assets.push(AssetSnapshot {
            original_source: None,
            id,
            path: PathBuf::from(format!("memory/session-audio-{index:03}.wav")),
            source_sample_rate: RATE,
            source_frames: SOURCE_FRAMES as u64,
            playback_frames: SOURCE_FRAMES as u64,
            conversion_version: 0,
        });
        let frames = (0..SOURCE_FRAMES)
            .map(|frame| original_frame(frame, if distinct_assets { index } else { 0 }))
            .collect();
        pcm.push(Audio::new(RATE, frames)?);
    }

    let mut tracks = Vec::with_capacity(100);
    let mut snapshot_tracks = Vec::with_capacity(100);
    let mut cells = Vec::with_capacity(300);
    let mut scene_a_clips = Vec::with_capacity(100);
    let mut scene_b_clips = Vec::with_capacity(100);
    for index in 0..100_usize {
        let track_id = next_id;
        next_id += 1;
        let arrangement_clip = next_id;
        next_id += 1;
        let scene_a_clip = next_id;
        next_id += 1;
        let scene_b_clip = next_id;
        next_id += 1;
        let asset = if distinct_assets {
            u64::try_from(index)? + 4
        } else {
            4_u64
        };
        let arrangement_duration = 16_384;
        snapshot_tracks.push(TrackSnapshot {
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: format!("audio {index:03}"),
            gain: 0.0625 + (index % 4) as f32 * 0.015625,
            balance: (index as i32 % 5 - 2) as f32 * 0.125,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: arrangement_clip,
                asset,
                start: 0,
                duration: arrangement_duration,
                source_offset: ARRANGEMENT_WINDOW.0,
                source_length: ARRANGEMENT_WINDOW.1 - ARRANGEMENT_WINDOW.0,
                looping: true,
                source_phase: 0,
                fade: ClipFade::neutral(arrangement_duration)?,
            }]),
            sends: Vec::new(),
        });
        cells.push(SessionCellSnapshot {
            track: track_id,
            scene: SCENE_A,
            content: SessionCellContentSnapshot::Audio(SessionAudioClipSnapshot {
                loop_range: None,
                active: true,
                flavor: hex_project::ClipFlavor::default(),
                gain: hex_project::ClipGain::UNITY,
                id: scene_a_clip,
                name: format!("one shot a {index:03}"),
                asset,
                source_start: WINDOW_A.0,
                source_end: WINDOW_A.1,
            }),
        });
        cells.push(SessionCellSnapshot {
            track: track_id,
            scene: SCENE_B,
            content: SessionCellContentSnapshot::Audio(SessionAudioClipSnapshot {
                loop_range: None,
                active: true,
                flavor: hex_project::ClipFlavor::default(),
                gain: hex_project::ClipGain::UNITY,
                id: scene_b_clip,
                name: format!("one shot b {index:03}"),
                asset,
                source_start: WINDOW_B.0,
                source_end: WINDOW_B.1,
            }),
        });
        cells.push(SessionCellSnapshot {
            track: track_id,
            scene: STOP_SCENE,
            content: SessionCellContentSnapshot::Stop,
        });
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
        assets,
        tracks: snapshot_tracks,
        session: SessionSnapshot {
            scenes: vec![
                SceneSnapshot {
                    id: SCENE_A,
                    name: "source a".into(),
                },
                SceneSnapshot {
                    id: SCENE_B,
                    name: "source b".into(),
                },
                SceneSnapshot {
                    id: STOP_SCENE,
                    name: "stop all".into(),
                },
            ],
            cells,
        },
        loop_region: None,
        master_gain: 0.5,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })?;
    let media = project
        .assets()
        .map(|asset| asset.id())
        .zip(pcm)
        .collect::<BTreeMap<_, _>>();
    tracks.extend(project.tracks().iter().map(|track| track.id()));
    for cell in project.session_cells() {
        if let SessionCellContent::Audio(clip) = cell.content() {
            if cell.scene().value() == SCENE_A {
                scene_a_clips.push(clip.id());
            } else if cell.scene().value() == SCENE_B {
                scene_b_clips.push(clip.id());
            }
        }
    }
    let scene_a = project
        .scenes()
        .iter()
        .find(|scene| scene.id().value() == SCENE_A)
        .ok_or("missing scene a")?
        .id();
    let scene_b = project
        .scenes()
        .iter()
        .find(|scene| scene.id().value() == SCENE_B)
        .ok_or("missing scene b")?
        .id();
    let stop_scene = project
        .scenes()
        .iter()
        .find(|scene| scene.id().value() == STOP_SCENE)
        .ok_or("missing stop scene")?
        .id();
    Ok(Fixture {
        project,
        media,
        tracks,
        scene_a_clips,
        scene_b_clips,
        scene_a,
        scene_b,
        stop_scene,
    })
}

/// An original, deterministic stereo signal made only from exact binary fractions.
pub fn original_frame(frame: usize, asset_index: usize) -> Frame {
    let left = ((frame.wrapping_add(asset_index * 3) % 17) as i32 - 8) as f32 / 16.0;
    let right = ((frame.wrapping_mul(3).wrapping_add(asset_index) % 19) as i32 - 9) as f32 / 16.0;
    [left, right]
}

pub fn unique_pcm_summary(media: &BTreeMap<AssetId, Audio>) -> (usize, usize, usize) {
    let mut unique = BTreeMap::<usize, usize>::new();
    for audio in media.values() {
        unique
            .entry(audio.frames().as_ptr() as usize)
            .or_insert(audio.frames().len());
    }
    let unique_assets = unique.len();
    let unique_frames = unique.values().sum::<usize>();
    let bytes = unique_frames * std::mem::size_of::<Frame>();
    (unique_assets, unique_frames, bytes)
}

/// Independent scalar Session oracle for a scene launched at frame zero.
/// `elapsed_since_launch` is source elapsed, not an arbitrary transport anchor.
pub fn finite_session_oracle(
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    scene: SceneId,
    elapsed_since_launch: u64,
    frames: usize,
) -> Result<Vec<Frame>, Box<dyn Error>> {
    let mut output = vec![[0.0; 2]; frames];
    for (offset, mixed) in output.iter_mut().enumerate() {
        let elapsed = elapsed_since_launch
            .checked_add(u64::try_from(offset)?)
            .ok_or("source elapsed frame overflow")?;
        for track in project.tracks() {
            if track.muted() {
                continue;
            }
            let Some(SessionCellContent::Audio(clip)) = project.session_cell(track.id(), scene)
            else {
                continue;
            };
            let source_frame = clip
                .source_start()
                .checked_add(elapsed)
                .ok_or("source frame overflow")?;
            if source_frame >= clip.source_end() {
                continue;
            }
            let audio = media.get(&clip.asset()).ok_or("missing oracle PCM asset")?;
            let source = *audio
                .frames()
                .get(usize::try_from(source_frame)?)
                .ok_or("oracle source frame is outside PCM")?;
            let channel_gain = [
                track.gain() * (1.0 - track.balance().max(0.0)),
                track.gain() * (1.0 + track.balance().min(0.0)),
            ];
            mixed[0] += source[0] * channel_gain[0];
            mixed[1] += source[1] * channel_gain[1];
        }
        mixed[0] *= project.master_gain();
        mixed[1] *= project.master_gain();
    }
    Ok(output)
}

/// Independent scalar oracle for the fixture's neutral-fade, phase-zero companions.
pub fn finite_arrangement_oracle(
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    arrangement_start: u64,
    frames: usize,
) -> Result<Vec<Frame>, Box<dyn Error>> {
    let mut output = vec![[0.0; 2]; frames];
    for (offset, mixed) in output.iter_mut().enumerate() {
        let timeline_frame = arrangement_start
            .checked_add(u64::try_from(offset)?)
            .ok_or("arrangement frame overflow")?;
        for track in project.tracks() {
            if track.muted() {
                continue;
            }
            for clip in track.clips() {
                let Some(elapsed) = timeline_frame.checked_sub(clip.start()) else {
                    continue;
                };
                if elapsed >= clip.duration() {
                    continue;
                }
                let source_elapsed = if clip.looping() {
                    elapsed % clip.source_length()
                } else if elapsed < clip.source_length() {
                    elapsed
                } else {
                    continue;
                };
                let source_frame = clip
                    .source_offset()
                    .checked_add(source_elapsed)
                    .ok_or("arrangement source frame overflow")?;
                let audio = media.get(&clip.asset()).ok_or("missing oracle PCM asset")?;
                let source = *audio
                    .frames()
                    .get(usize::try_from(source_frame)?)
                    .ok_or("arrangement source frame is outside PCM")?;
                let channel_gain = [
                    track.gain() * (1.0 - track.balance().max(0.0)),
                    track.gain() * (1.0 + track.balance().min(0.0)),
                ];
                mixed[0] += source[0] * channel_gain[0];
                mixed[1] += source[1] * channel_gain[1];
            }
        }
        mixed[0] *= project.master_gain();
        mixed[1] *= project.master_gain();
    }
    Ok(output)
}

pub fn render_partitions(
    playback: &mut hex_engine::playback::Playback,
    partitions: &[usize],
) -> Result<Vec<Frame>, Box<dyn Error>> {
    let total = partitions.iter().sum();
    let mut output = Vec::with_capacity(total);
    for &count in partitions {
        let start = output.len();
        output.resize(start + count, [0.0; 2]);
        playback.render(&mut output[start..start + count])?;
    }
    Ok(output)
}
