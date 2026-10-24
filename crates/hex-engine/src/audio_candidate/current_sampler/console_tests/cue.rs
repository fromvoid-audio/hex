use super::*;
use crate::{LaunchQuantization, PlaybackAction, SessionMidiOrigin, SessionPlaybackCommand};

fn looping_source(
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
fn source() -> crate::OwnedAudioSource {
    crate::OwnedAudioSource::try_resident(
        8000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]],
    )
    .ok()
    .unwrap()
}

#[test]
fn current_sampler_cue_same_clip_reserves_old_release_and_fresh_incarnation_with_literal_pcm() {
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
        let mut pcm = [[0.0; 2]; 8];
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
        assert!(pcm[2..].iter().flatten().any(|v| v.abs() > 1e-5));
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
        results[0][2..],
        results[1][2..],
        "after bounded outgoing release, same literal cued sampler PCM and exact velocity37"
    );
}

#[test]
fn current_sampler_cue_release_reservation_refusal_preserves_incarnation_proofs_and_next_pcm() {
    let mut notes = Vec::new();
    for pitch in 60..76 {
        notes.push((BeatTime::ZERO, BeatTime::ONE, pitch, 93));
    }
    for pitch in 76..92 {
        notes.push((BeatTime::ONE, BeatTime::ONE, pitch, 37));
    }
    let (editor, asset, track, clip) = looping_source(&notes, 32);
    let mut tested = prepare(editor.project(), vec![(asset, source())]);
    let mut oracle = prepare(editor.project(), vec![(asset, source())]);
    for cell in [&mut tested, &mut oracle] {
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
    }
    let owner = tested.owner_on_control().unwrap();
    let playback = &owner.inner.runtime.as_ref().unwrap().playback;
    let before = playback.session_route_status(0).unwrap();
    let generation = playback.session_route_state_generation();
    let proofs = playback.test_current_sampler_route_proofs(0);
    assert_eq!(proofs, Some((1, 16, 0)));
    assert_eq!(
        owner
            .inner
            .runtime
            .as_mut()
            .unwrap()
            .apply_action(PlaybackAction::Session(
                SessionPlaybackCommand::CueMidiClip {
                    track,
                    clip,
                    source_beat: BeatTime::ONE,
                    expected_route_generation: generation
                }
            )),
        Err(crate::PlaybackActionError::Session(
            crate::SessionPlaybackError::ReleaseBankFull
        ))
    );
    let playback = &owner.inner.runtime.as_ref().unwrap().playback;
    assert_eq!(playback.session_route_status(0).unwrap(), before);
    assert_eq!(playback.session_route_state_generation(), generation);
    assert_eq!(playback.test_current_sampler_route_proofs(0), proofs);
    let mut actual = [[0.0; 2]; 64];
    let mut expected = actual;
    owner.test_render(&mut actual).unwrap();
    oracle
        .owner_on_control()
        .unwrap()
        .test_render(&mut expected)
        .unwrap();
    assert_eq!(
        actual, expected,
        "failed incoming+survivor reservation cannot cut old deadlines or mutate DSP"
    );
    tested.cleanup_on_control().unwrap();
    oracle.cleanup_on_control().unwrap();
}
