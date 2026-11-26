//! Focused deterministic tests for bounded next-scene Session follow actions (#3448).

use super::*;
use crate::{Audio, Frame, PreparedRevision, SineTable};
use hex_project::{BeatTime, Command, Created, Editor, Project, SceneFollowAction, Tempo};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;

struct FollowFixture {
    editor: Editor,
    media: BTreeMap<hex_project::AssetId, Audio>,
    track: hex_project::TrackId,
    scenes: [SceneId; 3],
    clips: [ClipId; 3],
    signals: [Frame; 3],
}

fn three_scene_audio_project(
    tempo_bpm_micros: u32,
    tempo_change_at_bar_2_micros: Option<u32>,
) -> FollowFixture {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    if tempo_bpm_micros != 120_000_000 {
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(tempo_bpm_micros).unwrap(),
            })
            .unwrap();
    }
    if let Some(bpm2) = tempo_change_at_bar_2_micros {
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(4).unwrap(),
                tempo: Tempo::new(bpm2).unwrap(),
            })
            .unwrap();
    }
    let signals: [Frame; 3] = [[0.25, 0.25], [0.5, -0.5], [-0.75, 0.125]];
    let clip_frames = RATE as u64 * 4;
    let Created::Imported {
        track, asset: a0, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "t1".into(),
            path: "a0.wav".into(),
            sample_rate: RATE,
            frames: clip_frames,
        })
        .unwrap()
    else {
        panic!("import");
    };
    let arr_clip = editor.project().track(track).unwrap().clips()[0].id();
    editor.execute(Command::RemoveClip(arr_clip)).unwrap();

    let Created::Asset(a1) = editor
        .execute(Command::AddAsset {
            path: "a1.wav".into(),
            sample_rate: RATE,
            frames: clip_frames,
        })
        .unwrap()
    else {
        panic!("a1");
    };
    let Created::Asset(a2) = editor
        .execute(Command::AddAsset {
            path: "a2.wav".into(),
            sample_rate: RATE,
            frames: clip_frames,
        })
        .unwrap()
    else {
        panic!("a2");
    };

    let mut scenes = Vec::new();
    for name in ["s1", "s2", "s3"] {
        let Created::Scene(id) = editor
            .execute(Command::AddScene { name: name.into() })
            .unwrap()
        else {
            panic!("scene");
        };
        scenes.push(id);
    }
    let scenes: [SceneId; 3] = [scenes[0], scenes[1], scenes[2]];
    let assets = [a0, a1, a2];
    let mut clips = Vec::new();
    for i in 0..3 {
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene: scenes[i],
                name: format!("c{i}"),
                asset: assets[i],
                source_start: 0,
                source_end: clip_frames,
            })
            .unwrap()
        else {
            panic!("clip");
        };
        clips.push(clip);
    }
    let clips: [ClipId; 3] = [clips[0], clips[1], clips[2]];

    // Configure s1 -> s2 after 1 bar, s2 -> s3 after 1 bar, s3 -> end-of-grid (stops cleanly).
    for &scene in &scenes {
        editor
            .execute(Command::SetSceneFollowAction {
                scene,
                action: SceneFollowAction::NextScene { after_bars: 1 },
            })
            .unwrap();
    }

    let media = BTreeMap::from([
        (
            a0,
            Audio::new(RATE, vec![signals[0]; clip_frames as usize]).unwrap(),
        ),
        (
            a1,
            Audio::new(RATE, vec![signals[1]; clip_frames as usize]).unwrap(),
        ),
        (
            a2,
            Audio::new(RATE, vec![signals[2]; clip_frames as usize]).unwrap(),
        ),
    ]);
    FollowFixture {
        editor,
        media,
        track,
        scenes,
        clips,
        signals,
    }
}

#[test]
fn next_scene_follow_chains_at_exact_bar_boundaries_across_irregular_blocks_and_tempo_changes() {
    // 120 BPM for bar 1 (4 quarters = 2.0s = 96,000 frames), then 240 BPM for bar 2 (4 quarters = 1.0s = 48,000 frames).
    let FollowFixture {
        editor,
        media,
        track,
        scenes,
        clips,
        signals,
    } = three_scene_audio_project(120_000_000, Some(240_000_000));
    let table = Arc::new(SineTable::new());
    let prepared =
        Arc::new(PreparedRevision::prepare(editor.project(), &media, table, None).unwrap());
    let mut playback = SessionPlayback::new(prepared).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene: scenes[0],
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();

    // Armed follow: s1 -> s2 at frame 96,000 (1 bar @ 120 BPM).
    assert_eq!(
        playback.pending_scene_follow(),
        Some((scenes[0], scenes[1], 96_000))
    );

    let total_frames = 96_000 + 48_000 + 1_024;
    let mut rendered = vec![[0.0; 2]; total_frames];
    let block_sizes = [1, 7, 13, 64, 113, 256, 509];
    let mut pos = 0;
    let mut step = 0;
    while pos < total_frames {
        let len = block_sizes[step % block_sizes.len()].min(total_frames - pos);
        playback.render(&mut rendered[pos..pos + len]).unwrap();
        pos += len;
        step += 1;
    }

    // Bar 1 plays s1, bar 2 switches cleanly to s2, bar 3 switches cleanly to s3, and s3 ends grid with no further follow.
    assert_eq!(rendered[1_000], signals[0]);
    assert_eq!(rendered[95_999], signals[0]);
    assert_eq!(rendered[96_000 + 500], signals[1]);
    assert_eq!(rendered[96_000 + 48_000 + 500], signals[2]);
    assert_eq!(playback.pending_scene_follow(), None);
    let status = playback.route_status(0).unwrap();
    assert_eq!(status.track, track);
    assert_eq!(
        status.owner,
        SessionPlaybackOwner::Session { clip: clips[2] }
    );
}

#[test]
fn stop_seek_manual_launch_and_deleted_target_cancel_pending_scene_follow() {
    let FollowFixture {
        mut editor,
        media,
        scenes,
        clips,
        ..
    } = three_scene_audio_project(120_000_000, None);
    let table = Arc::new(SineTable::new());
    let prepared = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
    );

    // 1. Explicit Stop cancels follow action.
    let mut pb = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    pb.apply(SessionPlaybackCommand::LaunchScene {
        scene: scenes[0],
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    assert!(pb.pending_scene_follow().is_some());
    pb.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(pb.pending_scene_follow(), None);

    // 2. Seek cancels follow action.
    pb.apply(SessionPlaybackCommand::LaunchScene {
        scene: scenes[0],
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    assert!(pb.pending_scene_follow().is_some());
    pb.apply(SessionPlaybackCommand::Seek(4_096)).unwrap();
    assert_eq!(pb.pending_scene_follow(), None);

    // 3. Manual LaunchClip cancels follow action.
    pb.apply(SessionPlaybackCommand::LaunchScene {
        scene: scenes[0],
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    assert!(pb.pending_scene_follow().is_some());
    pb.apply(SessionPlaybackCommand::LaunchClip {
        clip: clips[2],
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    assert_eq!(pb.pending_scene_follow(), None);

    // 4. Deleting the target scene on live revision handoff clears the pending follow cleanly.
    let mut old = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    old.apply(SessionPlaybackCommand::LaunchScene {
        scene: scenes[0],
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut buf = [[0.0; 2]; 64];
    old.render(&mut buf).unwrap();
    assert_eq!(
        old.pending_scene_follow(),
        Some((scenes[0], scenes[1], 96_000))
    );

    editor.execute(Command::RemoveScene(scenes[1])).unwrap();
    let next_rev =
        Arc::new(PreparedRevision::prepare(editor.project(), &media, table, None).unwrap());
    let mut next = SessionPlayback::new(next_rev).unwrap();
    next.try_inherit_state_live_from(&mut old).unwrap();
    assert_eq!(next.pending_scene_follow(), None);
}
