use super::*;
use crate::{LaunchQuantization, PlaybackAction, SessionMidiOrigin, SessionPlaybackCommand};

pub(super) fn looping_source(
    notes: &[(BeatTime, BeatTime, u8, u8)],
    voices: u8,
) -> (
    Editor,
    hex_project::AssetId,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    let (project, asset) = fixture();
    let track = project.tracks()[0].id();
    let device = project.tracks()[0].instrument().unwrap();
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    state.voices = voices;
    state.mode = hex_devices::sampler::state::AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: hex_devices::sampler::Region { start: 0, end: 4 },
    };
    state.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 2.0 / 16000.0,
    };
    let native = state
        .to_native()
        .unwrap()
        .with_asset_references(device.state().asset_references().to_vec())
        .unwrap();
    let device_id = device.id();
    let mut editor = Editor::new(project, 32).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device_id,
            state: native,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "cue".into() })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "cue".into(),
            source_length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    for &(start, duration, pitch, velocity) in notes {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration,
                pitch,
                channel: 0,
                velocity,
            })
            .unwrap();
    }
    (editor, asset, track, clip)
}
pub(super) fn source() -> crate::OwnedAudioSource {
    crate::OwnedAudioSource::try_resident(
        8000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]],
    )
    .ok()
    .unwrap()
}

#[test]
fn current_sampler_cue_same_clip_reserves_old_release_and_fresh_incarnation_with_literal_pcm() {
    // The outgoing incarnation's release starts at the cue frame.
    let release = release_frames(2.0 / 16000.0) as usize;
    let mut results = Vec::new();
    for held in [false, true] {
        let (editor, asset, track, clip) = looping_source(
            &[
                (BeatTime::ZERO, BeatTime::from_quarters(2).unwrap(), 60, 127),
                (BeatTime::ONE, BeatTime::ONE, 72, 37),
            ],
            16,
        );
        let saved = editor.project().snapshot();
        let mut cell = prepare(editor.project(), vec![(asset, source())]);
        let owner = cell.owner_on_control().unwrap();
        owner
            .test_apply_action(PlaybackAction::Session(
                SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        if held {
            owner
                .test_apply_action(PlaybackAction::Transport(crate::PlaybackCommand::Play))
                .unwrap();
            owner.test_render(&mut [[0.0; 2]; 3]).unwrap();
            assert_eq!(
                owner
                    .inner
                    .runtime
                    .as_ref()
                    .unwrap()
                    .playback
                    .test_current_sampler_route_proofs(0),
                Some((1, 1, 0))
            );
        }
        let playback = &owner.inner.runtime.as_ref().unwrap().playback;
        let before = owner.test_transport().unwrap();
        let generation = playback.session_route_state_generation();
        owner
            .test_apply_action(PlaybackAction::Session(
                SessionPlaybackCommand::CueMidiClip {
                    track,
                    clip,
                    source_beat: BeatTime::ONE,
                    expected_route_generation: generation,
                },
            ))
            .unwrap();
        assert_eq!(
            owner.test_transport().unwrap(),
            before,
            "cue preserves play/pause and both clocks"
        );
        let status = owner
            .inner
            .runtime
            .as_ref()
            .unwrap()
            .playback
            .session_route_status(0)
            .unwrap();
        assert_eq!(
            status.active_origin.unwrap().elapsed_at_anchor,
            BeatTime::ONE
        );
        if !held {
            assert_eq!(
                status.active_origin,
                Some(SessionMidiOrigin::new(BeatTime::ZERO, BeatTime::ONE))
            );
            owner
                .test_apply_action(PlaybackAction::Transport(crate::PlaybackCommand::Play))
                .unwrap();
        }
        let mut pcm = vec![[0.0; 2]; release + 6];
        owner.test_render(&mut pcm[..1]).unwrap();
        assert_eq!(
            owner
                .inner
                .runtime
                .as_ref()
                .unwrap()
                .playback
                .test_current_sampler_route_proofs(0),
            Some((2, 1, usize::from(held)))
        );
        owner.test_render(&mut pcm[1..]).unwrap();
        assert_eq!(
            owner
                .inner
                .runtime
                .as_ref()
                .unwrap()
                .playback
                .test_current_sampler_route_proofs(0),
            Some((2, 1, 0))
        );
        assert!(pcm[release..].iter().flatten().any(|v| v.abs() > 1e-5));
        let generation = owner
            .inner
            .runtime
            .as_ref()
            .unwrap()
            .playback
            .session_route_state_generation();
        owner
            .test_apply_action(PlaybackAction::Session(
                SessionPlaybackCommand::CueMidiClip {
                    track,
                    clip,
                    source_beat: BeatTime::ONE,
                    expected_route_generation: generation,
                },
            ))
            .unwrap();
        owner.test_render(&mut [[0.0; 2]; 1]).unwrap();
        assert_eq!(
            owner
                .inner
                .runtime
                .as_ref()
                .unwrap()
                .playback
                .test_current_sampler_route_proofs(0),
            Some((3, 1, 1)),
            "same NoteId is a fresh incoming incarnation, while old proof receives one release"
        );
        assert_eq!(editor.project().snapshot(), saved);
        results.push(pcm);
        cell.cleanup_on_control().unwrap();
    }
    assert_eq!(
        results[0][release..],
        results[1][release..],
        "after bounded outgoing release, same literal cued sampler PCM and exact velocity37"
    );
}

#[test]
fn current_sampler_cue_steals_survivor_releases_for_a_full_incoming_voice_set() {
    // 32 voices: 16 held survivors plus 16 incoming exceeded the old
    // held+releasing reservation. Survivors all release at the cut, and the
    // stealing sampler reuses their slots, so only the incoming 16 need room.
    let mut notes = Vec::new();
    for pitch in 60..76 {
        notes.push((BeatTime::ZERO, BeatTime::ONE, pitch, 93));
    }
    for pitch in 76..92 {
        notes.push((BeatTime::ONE, BeatTime::ONE, pitch, 37));
    }
    let (editor, asset, track, clip) = looping_source(&notes, 32);
    let mut cell = prepare(editor.project(), vec![(asset, source())]);
    let owner = cell.owner_on_control().unwrap();
    owner
        .test_apply_action(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    owner.test_render(&mut [[0.0; 2]; 3]).unwrap();
    let playback = &owner.inner.runtime.as_ref().unwrap().playback;
    let generation = playback.session_route_state_generation();
    assert_eq!(
        playback.test_current_sampler_route_proofs(0),
        Some((1, 16, 0))
    );
    owner
        .test_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::CueMidiClip {
                track,
                clip,
                source_beat: BeatTime::ONE,
                expected_route_generation: generation,
            },
        ))
        .unwrap();
    let mut pcm = [[0.0; 2]; 64];
    owner.test_render(&mut pcm[..1]).unwrap();
    assert_eq!(
        owner
            .inner
            .runtime
            .as_ref()
            .unwrap()
            .playback
            .test_current_sampler_route_proofs(0),
        Some((2, 16, 16)),
        "16 incoming voices are held while the 16 released proofs keep their deadlines"
    );
    owner.test_render(&mut pcm[1..]).unwrap();
    assert!(pcm.iter().flatten().all(|v| v.is_finite()));
    assert!(pcm[1..].iter().flatten().any(|v| v.abs() > 1e-5));
    cell.cleanup_on_control().unwrap();
}

#[test]
fn activation_inactive_current_sampler_keeps_cell_without_source_events() {
    let (mut editor, asset, _, clip) = looping_source(
        &[(BeatTime::ZERO, BeatTime::from_quarters(2).unwrap(), 60, 127)],
        16,
    );
    let arrangement = editor.project().tracks()[0].midi_clips()[0].id();
    editor
        .execute(Command::SetClipActive {
            clip: arrangement,
            active: false,
        })
        .unwrap();
    editor
        .execute(Command::SetClipActive {
            clip,
            active: false,
        })
        .unwrap();
    let mut cell = prepare(editor.project(), vec![(asset, source())]);
    let owner = cell.owner_on_control().unwrap();
    let revision = owner
        .inner
        .runtime
        .as_ref()
        .unwrap()
        .playback
        .prepared_revision()
        .unwrap();
    let prepared = revision.session().midi_clip(clip).unwrap();
    assert!(!prepared.active);
    assert_eq!(prepared.source_template().max_query_edges(), 0);
    assert_eq!(prepared.work.active_voices, 0);
    assert_eq!(prepared.work.query_onsets, 0);
    owner
        .test_apply_action(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    let mut pcm = [[9.0; 2]; 16];
    owner.test_render(&mut pcm).unwrap();
    assert_eq!(pcm, [[0.0; 2]; 16]);
    assert_eq!(
        owner
            .inner
            .runtime
            .as_ref()
            .unwrap()
            .playback
            .test_current_sampler_route_proofs(0),
        Some((1, 0, 0))
    );
}
