//! Exercise the production-default consumer, with only native output creation
//! substituted. Session transactions, prospective admission, publication and callback stay real.
use super::*;
mod native_automation;
mod native_source_effect;
use hex_engine::{
    PreparedPlaybackObservation,
    native_device_resources::{DeviceResourceAdmission, PreparedDeviceResources},
    playback::Playback,
};
use hex_session::service::{
    ResidentPublicationAcceptance, ResidentPublicationMetadata, ResidentPublicationPolicy,
    ResidentPublicationRefusal,
};
use std::sync::atomic::AtomicU8;

// At 48 kHz the arrangement loop tapers its final 128 frames. Compare every
// sample against that envelope independently of publication scheduling.
fn assert_loop_pcm(h: &ResidentHarness, level: f32) -> [hex_engine::Frame; 64] {
    let start = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    let pcm = h.render();
    for (offset, frame) in pcm.iter().enumerate() {
        let position = (start + offset as u64) % 8192;
        let gain = if position >= 8192 - 128 {
            (8191 - position) as f32 / 127.0
        } else {
            1.0
        };
        let expected = level * gain;
        assert!(
            frame.iter().all(|sample| (*sample - expected).abs() < 1e-5),
            "loop frame {position}: {frame:?}, expected {expected}"
        );
    }
    pcm
}

// Native fixture parameters are uninhabited on targets without the host; the
// portable harness can still use its ordinary constructor with None.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
type PluginFixture = (
    hex_audio::plugin_effects::Hosting,
    Option<hex_audio::plugin_effects::EffectPresentation>,
    Option<hex_diagnostics::DiagnosticsHandle>,
);
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
enum PluginFixture {}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
type PluginFacts = crate::native_audio::plugins::FactsGate;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
enum PluginFacts {}

#[test]
fn held_single_midi_move_reschedules_the_note_before_commit() {
    use hex_project::{BeatTime, ClipGroupPosition, Command, Created};
    use hex_session::service::{ArrangementMoveAudition, ArrangementMoveMode};
    let _native = native_fixture();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddTrack {
        name: "midi".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    let (applied, _) = h.applied(applied);
    h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Instrument,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.instrument.sine")),
        before: None,
    });
    let (applied, _) = h.applied(applied);
    let beat = |q| BeatTime::from_quarters(q).unwrap();
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: beat(4),
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let (applied, _) = h.applied(applied);
    let before = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: beat(1),
        duration: beat(1),
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    let (applied, metadata) = h.applied(applied);
    let original = before.state.project.snapshot();
    let target = ArrangementMoveAudition {
        context: metadata.context,
        gesture: 11,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: hex_session::service::ArrangementMoveDestination::Existing(track),
        position: ClipGroupPosition::Midi(beat(1)),
        mode: ArrangementMoveMode::Move,
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert!(h.service.audition_arrangement_move(Some(target.clone())));
    let (preview, preview_metadata) = h.applied(applied);
    assert_eq!(preview_metadata.clip_preview, Some(target.identity()));
    assert_eq!(
        preview_metadata.source_origin,
        hex_session::service::ResidentSourceOrigin::ClipMovePreview
    );
    let position = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
    };
    for _ in 0..800 {
        if position(&h) >= 24_128 {
            break;
        }
        assert!(h.render().iter().all(|frame| frame[0].abs() < 1e-5));
    }
    assert!((24_128..48_000).contains(&position(&h)));
    assert!(h.render().iter().all(|frame| frame[0].abs() < 1e-5));
    for _ in 0..800 {
        if position(&h) >= 48_128 {
            break;
        }
        h.render();
    }
    assert!(position(&h) >= 48_128);
    assert!(h.render().iter().any(|frame| frame[0].abs() > 0.005));
    assert!(h.service.try_event().is_err());
    h.service.finish_arrangement_move_audition();
    let committed = h.run(Request::MoveExistingClips {
        project_generation: target.context.project_generation,
        revision: target.context.editor_revision,
        command: Command::MoveMidiClipWithOverwrite {
            clip,
            track,
            start: beat(1),
        },
    });
    assert!(committed.error.is_none());
    h.service.retire_arrangement_move_audition();
    let (committed_audio, _) = h.applied(preview);
    assert_eq!(h.run(Request::Undo).state.project.snapshot(), original);
    h.applied(committed_audio);
}

#[test]
fn held_single_audio_move_uses_single_commit_and_restores_on_cancel() {
    use hex_project::{ClipGroupPosition, Command};
    use hex_session::service::{ArrangementMoveAudition, ArrangementMoveMode};
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 96_000)));
    let (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let looped = h.run(Request::Edit(Command::SetLoopRegion {
        start: 0,
        end: 8192,
        enabled: true,
    }));
    let (applied, metadata) = h.applied(applied);
    let original = looped.state.project.snapshot();
    let target = ArrangementMoveAudition {
        context: metadata.context,
        gesture: 11,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: hex_session::service::ArrangementMoveDestination::Existing(track),
        position: ClipGroupPosition::Audio(48_000),
        mode: ArrangementMoveMode::Move,
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_loop_pcm(&h, 0.25);
    assert!(h.service.audition_arrangement_move(Some(target.clone())));
    let (preview, preview_metadata) = h.applied(applied);
    assert_eq!(preview_metadata.clip_preview, Some(target.identity()));
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    // A refused newer move must leave the previously admitted preview and the
    // canonical project untouched; the FIFO request remains serviceable.
    let (entered, resume, _) = h.probe_next_native_publication();
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    assert!(
        h.service
            .audition_arrangement_move(Some(ArrangementMoveAudition {
                sequence: 2,
                position: ClipGroupPosition::Audio(24_000),
                ..target.clone()
            }))
    );
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    resume.send(()).unwrap();
    let unchanged = h.run(Request::QueryDeviceAutomationCloseSafety { request_id: 1 });
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    assert_eq!(unchanged.state.project.snapshot(), original);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        preview
    );
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    assert!(!h.service.audition_arrangement_move(None));
    let (restored, restored_metadata) = h.applied(preview);
    assert!(restored_metadata.clip_preview.is_none());
    assert_loop_pcm(&h, 0.25);
    assert!(h.service.audition_arrangement_move(Some(target.clone())));
    let (preview, _) = h.applied(restored);
    h.service.finish_arrangement_move_audition();
    let committed = h.run(Request::MoveExistingClips {
        project_generation: target.context.project_generation,
        revision: target.context.editor_revision,
        command: Command::MoveClipWithOverwrite {
            clip,
            track,
            start: 48_000,
        },
    });
    assert!(committed.error.is_none());
    h.service.retire_arrangement_move_audition();
    let (committed_audio, committed_metadata) = h.applied(preview);
    assert!(committed_metadata.clip_preview.is_none());
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    assert_eq!(h.run(Request::Undo).state.project.snapshot(), original);
    h.applied(committed_audio);
    assert_loop_pcm(&h, 0.25);
}

#[test]
fn held_audio_move_to_new_track_is_transient_and_commits_once() {
    use hex_project::{ClipGroupPosition, Command};
    use hex_session::service::{
        ArrangementMoveAudition, ArrangementMoveDestination, ArrangementMoveMode,
        ClipMovePreviewDestination, NewTrackMoveDestination,
    };
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 96_000)));
    let (applied, _) = h.applied(initial);
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let looped = h.run(Request::Edit(Command::SetLoopRegion {
        start: 0,
        end: 8192,
        enabled: true,
    }));
    let (applied, metadata) = h.applied(applied);
    let original = looped.state.project.snapshot();
    let destination = NewTrackMoveDestination::new(Arc::from("track 2"), Some(track), 0.5).unwrap();
    let mut target = ArrangementMoveAudition {
        context: metadata.context,
        gesture: 25,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: ArrangementMoveDestination::NewTrack(destination),
        position: ClipGroupPosition::Audio(48_000),
        mode: ArrangementMoveMode::Move,
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_loop_pcm(&h, 0.25);
    assert!(h.service.audition_arrangement_move(Some(target.clone())));
    let (preview, preview_metadata) = h.applied(applied);
    assert_eq!(preview_metadata.clip_preview, Some(target.identity()));
    assert_eq!(
        target.identity().destination,
        ClipMovePreviewDestination::NewTrack {
            before_track: Some(track)
        }
    );
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    assert!(h.service.try_event().is_err());
    assert!(!h.service.audition_arrangement_move(None));
    let (restored, _) = h.applied(preview);
    assert_loop_pcm(&h, 0.25);
    target.sequence = 2;
    target.position = ClipGroupPosition::Audio(0);
    assert!(h.service.audition_arrangement_move(Some(target.clone())));
    let (preview, _) = h.applied(restored);
    assert_loop_pcm(&h, 0.125);
    h.service.finish_arrangement_move_audition();
    let committed = h.run(Request::MoveExistingClips {
        project_generation: target.context.project_generation,
        revision: target.context.editor_revision,
        command: Command::MoveClipToNewTrack {
            clip,
            name: "track 2".into(),
            before_track: Some(track),
            start: 0,
            initial_gain: Some(0.5),
        },
    });
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert_eq!(committed.state.revision, looped.state.revision + 1);
    assert_eq!(committed.state.project.tracks().len(), 2);
    assert_eq!(committed.state.project.tracks()[0].gain(), 0.5);
    h.service.retire_arrangement_move_audition();
    let (committed_audio, _) = h.applied(preview);
    assert_loop_pcm(&h, 0.125);
    let mut expected_undo = original;
    expected_undo.next_id = committed.state.project.snapshot().next_id;
    assert_eq!(h.run(Request::Undo).state.project.snapshot(), expected_undo);
    h.applied(committed_audio);
    assert_loop_pcm(&h, 0.25);
    for _ in 0..128 {
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
            == 8128
        {
            break;
        }
        h.render();
    }
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire),
        8128
    );
    let seam = assert_loop_pcm(&h, 0.25);
    assert_eq!(seam[63], [0.0; 2]);
    assert_loop_pcm(&h, 0.25);
}

#[test]
fn held_group_move_auditions_both_clips_and_commits_once() {
    use hex_project::{ClipGroupPosition, Command};
    use hex_session::service::{ArrangementMoveAudition, ArrangementMoveMode};

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let source = files.wav(48_000, 96_000);
    let first = h.run(Request::Import(source.clone()));
    let a = first.state.project.tracks()[0].clips()[0].id();
    let (applied, _) = h.applied(applied);
    let second = h.run(Request::Import(source));
    let (applied, metadata) = h.applied(applied);
    let b = second.state.project.tracks()[1].clips()[0].id();
    let original = second.state.project.snapshot();
    let target = ArrangementMoveAudition {
        context: metadata.context,
        gesture: 11,
        sequence: 1,
        clips: vec![a, b].into(),
        anchor: a,
        destination: hex_session::service::ArrangementMoveDestination::Existing(
            second.state.project.tracks()[0].id(),
        ),
        position: ClipGroupPosition::Audio(48_000),
        mode: ArrangementMoveMode::Move,
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert!(h.render().iter().all(|f| (f[0] - 0.5).abs() < 1e-5));
    assert!(h.service.audition_arrangement_move(Some(target.clone())));
    let advance = |h: &mut ResidentHarness, previous| {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            h.pump();
            h.render();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let revision = status.applied_revision.load(Acquire);
            if revision > previous {
                break revision;
            }
            assert!(Instant::now() < deadline, "group preview did not apply");
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    let preview = advance(&mut h, applied);
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    assert!(h.service.try_event().is_err());
    assert!(!h.service.audition_arrangement_move(None));
    let restored = advance(&mut h, preview);
    assert!(h.render().iter().all(|f| (f[0] - 0.5).abs() < 1e-5));
    assert!(h.service.audition_arrangement_move(Some(target.clone())));
    let preview = advance(&mut h, restored);
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    h.service.finish_arrangement_move_audition();
    let committed = h.run(Request::MoveExistingClips {
        project_generation: target.context.project_generation,
        revision: target.context.editor_revision,
        command: Command::MoveClipsWithOverwrite {
            clips: target.clips.to_vec(),
            anchor: a,
            track: second.state.project.tracks()[0].id(),
            position: target.position,
            copy: false,
        },
    });
    assert!(committed.error.is_none());
    assert_eq!(committed.state.revision, second.state.revision + 1);
    h.service.retire_arrangement_move_audition();
    let committed_audio = advance(&mut h, preview);
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    assert_eq!(h.run(Request::Undo).state.project.snapshot(), original);
    let undone_audio = advance(&mut h, committed_audio);
    assert!(h.render().iter().all(|f| (f[0] - 0.5).abs() < 1e-5));
    assert_eq!(
        h.run(Request::Redo).state.project.snapshot(),
        committed.state.project.snapshot()
    );
    let redone_audio = advance(&mut h, undone_audio);
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));

    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project.snapshot(), original);
    let restored_audio = advance(&mut h, redone_audio);
    let copy = ArrangementMoveAudition {
        context: hex_engine::AudioSourceContext {
            project_generation: target.context.project_generation,
            editor_revision: undone.state.revision,
        },
        position: ClipGroupPosition::Audio(120_000),
        mode: ArrangementMoveMode::Copy,
        ..target.clone()
    };
    assert!(h.service.audition_arrangement_move(Some(copy.clone())));
    let copied_preview = advance(&mut h, restored_audio);
    assert!(h.render().iter().all(|f| (f[0] - 0.5).abs() < 1e-5));
    // The original ends before the new position, so PCM there proves the copy.
    for _ in 0..2_000 {
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
            >= 121_000
        {
            break;
        }
        h.render();
    }
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
            >= 121_000
    );
    assert!(h.render().iter().all(|f| (f[0] - 0.5).abs() < 1e-5));
    h.service.finish_arrangement_move_audition();
    let copied = h.run(Request::MoveExistingClips {
        project_generation: copy.context.project_generation,
        revision: copy.context.editor_revision,
        command: Command::MoveClipsWithOverwrite {
            clips: copy.clips.to_vec(),
            anchor: copy.anchor,
            track: second.state.project.tracks()[0].id(),
            position: copy.position,
            copy: true,
        },
    });
    assert!(copied.error.is_none());
    assert_eq!(copied.state.project.tracks()[0].clips().len(), 2);
    assert_eq!(copied.state.project.tracks()[1].clips().len(), 2);
    h.service.retire_arrangement_move_audition();
    let copied_audio = advance(&mut h, copied_preview);
    assert!(h.render().iter().all(|f| (f[0] - 0.5).abs() < 1e-5));
    let mut expected_after_copy_undo = original.clone();
    expected_after_copy_undo.next_id = copied.state.project.snapshot().next_id;
    let copy_undone = h.run(Request::Undo);
    assert_eq!(
        copy_undone.state.project.snapshot(),
        expected_after_copy_undo
    );
    let restored_audio = advance(&mut h, copied_audio);
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));

    let alias = ArrangementMoveAudition {
        context: hex_engine::AudioSourceContext {
            project_generation: target.context.project_generation,
            editor_revision: copy_undone.state.revision,
        },
        position: ClipGroupPosition::Audio(120_000),
        mode: ArrangementMoveMode::AliasCopy,
        ..target.clone()
    };
    assert!(h.service.audition_arrangement_move(Some(alias.clone())));
    let alias_preview = advance(&mut h, restored_audio);
    assert!(h.render().iter().all(|f| (f[0] - 0.5).abs() < 1e-5));
    h.service.finish_arrangement_move_audition();
    let aliased = h.run(Request::MoveExistingClips {
        project_generation: alias.context.project_generation,
        revision: alias.context.editor_revision,
        command: Command::DuplicateAliasClipsAt {
            clips: alias.clips.to_vec(),
            anchor: alias.anchor,
            track: second.state.project.tracks()[0].id(),
            position: alias.position,
        },
    });
    assert!(aliased.error.is_none());
    assert!(aliased.state.project.clip_alias(a).is_some());
    assert!(aliased.state.project.clip_alias(b).is_some());
    h.service.retire_arrangement_move_audition();
    let alias_audio = advance(&mut h, alias_preview);
    assert!(h.render().iter().all(|f| (f[0] - 0.5).abs() < 1e-5));
    let mut expected_after_alias_undo = original;
    expected_after_alias_undo.next_id = aliased.state.project.snapshot().next_id;
    assert_eq!(
        h.run(Request::Undo).state.project.snapshot(),
        expected_after_alias_undo
    );
    advance(&mut h, alias_audio);
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
}

#[test]
fn held_stretch_audition_reaches_playing_callback_and_releases_once() {
    use hex_session::service::ArrangementStretchAudition;

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 96_000)));
    let (applied, metadata) = h.applied(initial);
    assert_eq!(
        metadata.source_origin,
        hex_session::service::ResidentSourceOrigin::CanonicalCandidate
    );
    let before = imported.state.project.snapshot();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..438 {
        h.render();
    }
    assert!(h.render().iter().all(|f| (f[0] - 0.25).abs() < 1e-5));

    let target = ArrangementStretchAudition {
        context: metadata.context,
        clip,
        edge: hex_project::TrimEdge::Right,
        frame: 24_000,
    };
    let advance = |h: &mut ResidentHarness, previous| {
        let start = Instant::now();
        loop {
            h.pump();
            let before = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire);
            let pcm = h.render();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert!(status.playing.load(Acquire));
            assert_eq!(status.position.load(Acquire), before + 64);
            let revision = status.applied_revision.load(Acquire);
            if revision > previous {
                break (revision, pcm);
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "preview did not apply"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    h.service
        .audition_arrangement_stretch(Some(ArrangementStretchAudition {
            frame: 84_000,
            ..target
        }));
    h.service.audition_arrangement_stretch(Some(target));
    let previous = applied;
    let (applied, pcm) = advance(&mut h, previous);
    let (reported, preview_metadata) = h.applied(previous);
    assert_eq!(reported, applied);
    assert_eq!(preview_metadata.clip_preview, None);
    assert_eq!(
        preview_metadata.source_origin,
        hex_session::service::ResidentSourceOrigin::ClipStretchPreview
    );
    assert!(
        pcm.iter().all(|f| *f == [0.0; 2]),
        "latest held target is audible before release"
    );
    assert!(
        h.service.try_event().is_err(),
        "held preview does not edit the project"
    );

    let (entered, resume, _) = h.probe_next_native_publication();
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .audition_arrangement_stretch(Some(ArrangementStretchAudition {
            frame: 36_000,
            ..target
        }));
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    resume.send(()).unwrap();
    let unchanged = h.run(Request::QueryDeviceAutomationCloseSafety { request_id: 1 });
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    assert_eq!(unchanged.state.revision, imported.state.revision);
    assert_eq!(unchanged.state.project.snapshot(), before);
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
    h.service.audition_arrangement_stretch(None);
    let (restored, pcm) = advance(&mut h, applied);
    assert!(pcm.iter().all(|f| (f[0] - 0.25).abs() < 1e-5));

    h.service.audition_arrangement_stretch(Some(target));
    let (held, _) = advance(&mut h, restored);
    h.service.finish_arrangement_stretch_audition();
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    let committed = h.run(Request::Edit(hex_project::Command::StretchAudioClipEdge {
        clip,
        edge: target.edge,
        frame: target.frame,
    }));
    assert_eq!(committed.state.revision, imported.state.revision + 1);
    assert_eq!(
        committed.state.project.clip(clip).unwrap().duration(),
        target.frame
    );
    h.service.audition_arrangement_stretch(None);
    let (committed_audio, pcm) = advance(&mut h, held);
    assert!(pcm.iter().all(|f| *f == [0.0; 2]));
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project.snapshot(), before);
    let (undone_audio, pcm) = advance(&mut h, committed_audio);
    assert!(pcm.iter().all(|f| (f[0] - 0.25).abs() < 1e-5));
    let redone = h.run(Request::Redo);
    assert_eq!(
        redone.state.project.snapshot(),
        committed.state.project.snapshot()
    );
    let (redone_audio, pcm) = advance(&mut h, undone_audio);
    assert!(pcm.iter().all(|f| *f == [0.0; 2]));

    let (entered, _resume, _) = h.probe_next_native_publication();
    h.service
        .audition_arrangement_stretch(Some(ArrangementStretchAudition {
            frame: 84_000,
            ..target
        }));
    assert!(matches!(
        entered.recv_timeout(Duration::from_millis(120)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(h.render().iter().all(|f| *f == [0.0; 2]));
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        redone_audio
    );
}

#[test]
fn held_clip_and_event_gains_reach_the_callback_without_editing_history() {
    use hex_project::{AudioEventEdit, ClipGain, Command};
    use hex_session::service::{ArrangementGainAudition, GainEdit, ResidentSourceOrigin};

    let _native = native_fixture();
    let files = Files::new();
    // The event preview can retain the original, canonical composed root
    // and newly gained root together while the callback replaces its graph.
    let mut h = ResidentHarness::with_frame_limit(524_288);
    h.next();
    let (initial, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 96_000)));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let (applied, _) = h.applied(initial);
    let split = h.run(Request::Edit(Command::EditAudioEvents {
        clip,
        operation: AudioEventEdit::Split { at: 48_000 },
    }));
    assert!(split.error.is_none(), "{:?}", split.error);
    let (mut applied, metadata) = h.applied(applied);
    let event = split
        .state
        .project
        .clip(clip)
        .unwrap()
        .audio_events()
        .events[0]
        .id;
    let original = split.state.project.snapshot();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();

    let assert_audio = |h: &ResidentHarness, gain: ClipGain| {
        let expected = 0.25 * gain.linear();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let before = status.position.load(Acquire);
        let output = h.render();
        assert!(status.playing.load(Acquire));
        assert_eq!(status.position.load(Acquire), before + 64);
        assert!(
            output.iter().all(|f| (f[0] - expected).abs() < 1e-5),
            "audio should be {expected}"
        );
    };
    let hold = |h: &ResidentHarness, edit| {
        h.service
            .audition_arrangement_gain(Some(ArrangementGainAudition {
                context: metadata.context,
                edit,
            }));
    };
    let advance = |h: &mut ResidentHarness, after| {
        let deadline = Instant::now() + Duration::from_millis(750);
        loop {
            h.pump();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let applied = status.applied_revision.load(Acquire);
            if let Some((revision, metadata)) = h.submitted
                && revision == applied
                && revision > after
            {
                break (revision, metadata);
            }
            assert!(
                Instant::now() < deadline,
                "gain preview did not reach the callback"
            );
            if applied <= after {
                h.render_worker_step();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    assert_audio(&h, ClipGain::UNITY);
    for millidb in [-6_000, -12_000] {
        let gain = ClipGain::new(millidb).unwrap();
        hold(&h, GainEdit::Clip { clip, gain });
        let (revision, report) = advance(&mut h, applied);
        applied = revision;
        assert_eq!(report.source_origin, ResidentSourceOrigin::ClipGainPreview);
        assert_audio(&h, gain);
        assert!(
            h.service.try_event().is_err(),
            "held values must never edit the project"
        );
    }
    h.service.audition_arrangement_gain(None);
    let (applied, _) = advance(&mut h, applied);
    assert_audio(&h, ClipGain::UNITY);

    let gain = ClipGain::new(-3_000).unwrap();
    hold(
        &h,
        GainEdit::Event {
            clip,
            sources: Default::default(),
            event,
            gain,
        },
    );
    let (preview, report) = advance(&mut h, applied);
    assert_eq!(report.source_origin, ResidentSourceOrigin::ClipGainPreview);
    assert_audio(&h, gain);
    h.service.finish_arrangement_gain_audition();
    assert_audio(&h, gain);
    let committed = h.run(Request::Edit(Command::EditAudioEvents {
        clip,
        operation: AudioEventEdit::SetGain { event, gain },
    }));
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert_eq!(committed.state.revision, split.state.revision + 1);
    h.service.retire_arrangement_gain_audition();
    let (committed, _) = advance(&mut h, preview);
    assert_audio(&h, gain);
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project.snapshot(), original);
    advance(&mut h, committed);
    assert_audio(&h, ClipGain::UNITY);
}

#[test]
fn held_audio_play_start_changes_only_its_clip_during_arrangement_playback() {
    use hex_project::{AudioViewResizeOperation, Command};
    use hex_session::service::{ArrangementPlayStartAudition, PlayStartEdit};

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let source = files.wav(48_000, 96_000);
    let imported = h.run(Request::Import(source.clone()));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let (applied, _) = h.applied(initial);
    let mixed = h.run(Request::Import(source));
    let (applied, metadata) = h.applied(applied);
    assert_eq!(mixed.state.project.tracks().len(), 2);
    let original = mixed.state.project.snapshot();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();

    let target = ArrangementPlayStartAudition {
        context: metadata.context,
        edit: PlayStartEdit::Audio {
            clip,
            position: 48_000,
        },
    };
    let render = |h: &ResidentHarness, gain: f32| {
        assert!(
            h.render().iter().all(|f| (f[0] - gain).abs() < 1e-5),
            "mix should be {gain}"
        );
    };
    let advance = |h: &mut ResidentHarness, previous| {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            h.pump();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let before = status.position.load(Acquire);
            h.render();
            assert!(status.playing.load(Acquire));
            assert_eq!(status.position.load(Acquire), before + 64);
            let revision = status.applied_revision.load(Acquire);
            if revision > previous {
                break revision;
            }
            assert!(
                Instant::now() < deadline,
                "play-start preview did not apply"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    render(&h, 0.5);
    h.service.audition_arrangement_play_start(Some(target));
    let preview = advance(&mut h, applied);
    let (reported, preview_metadata) = h.applied(applied);
    assert_eq!(reported, preview);
    assert_eq!(preview_metadata.clip_preview, None);
    assert_eq!(
        preview_metadata.source_origin,
        hex_session::service::ResidentSourceOrigin::ClipPlayStartPreview
    );
    render(&h, 0.25);
    assert!(
        h.service.try_event().is_err(),
        "preview must leave canonical history alone"
    );

    h.service.audition_arrangement_play_start(None);
    let restored = advance(&mut h, preview);
    render(&h, 0.5);

    h.service.audition_arrangement_play_start(Some(target));
    let preview = advance(&mut h, restored);
    h.service.finish_arrangement_play_start_audition();
    render(&h, 0.25);
    let committed = h.run(Request::Edit(Command::ResizeAudioClipView {
        clip,
        operation: AudioViewResizeOperation::PlayStart { position: 48_000 },
    }));
    assert_eq!(committed.state.revision, mixed.state.revision + 1);
    h.service.retire_arrangement_play_start_audition();
    let committed = advance(&mut h, preview);
    render(&h, 0.25);
    assert_eq!(h.run(Request::Undo).state.project.snapshot(), original);
    let (undone, metadata) = h.applied(committed);
    render(&h, 0.5);

    h.service
        .audition_arrangement_play_start(Some(ArrangementPlayStartAudition {
            context: metadata.context,
            ..target
        }));
    let preview = advance(&mut h, undone);
    h.service.finish_arrangement_play_start_audition();
    h.service
        .submit(Request::Edit(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::PlayStart { position: u64::MAX },
        }))
        .unwrap();
    let refused = h.next();
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project.snapshot(), original);
    h.service.retire_arrangement_play_start_audition();
    advance(&mut h, preview);
    render(&h, 0.5);
}

#[test]
fn held_midi_play_start_reschedules_notes_in_the_playing_arrangement() {
    use hex_project::{BeatTime, Command, Created};
    use hex_session::service::{ArrangementPlayStartAudition, PlayStartEdit};

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    h.run(Request::Import(files.wav(48_000, 96_000)));
    (applied, _) = h.applied(applied);
    let added = h.run(Request::Edit(Command::AddTrack {
        name: "midi".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("midi track")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Instrument,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.instrument.sine")),
        before: None,
    });
    (applied, _) = h.applied(applied);
    let beat = |quarters| BeatTime::from_quarters(quarters).unwrap();
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: beat(4),
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("midi clip")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: beat(2),
        duration: beat(1),
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let source_edit = |source_start| Command::SetMidiClipSource {
        clip,
        source_length: beat(4),
        source_start,
        loop_start: BeatTime::ZERO,
        loop_end: beat(4),
        looping: true,
        length: beat(4),
    };
    let before = h.run(Request::Edit(source_edit(BeatTime::ZERO)));
    let (applied, metadata) = h.applied(applied);
    let first_note_frame = before
        .state
        .project
        .tempo_map()
        .beat_to_frame(beat(1))
        .unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let before_preview = h.render();
    assert!(before_preview.iter().all(|f| (f[0] - 0.25).abs() < 1e-5));
    let position = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
    };
    h.service
        .audition_arrangement_play_start(Some(ArrangementPlayStartAudition {
            context: metadata.context,
            edit: PlayStartEdit::from_midi_command(&source_edit(beat(1))).unwrap(),
        }));
    let start = position(&h);
    let (_, held_metadata) = h.applied(applied);
    assert_eq!(held_metadata.context, metadata.context);
    assert!((start..first_note_frame).contains(&position(&h)));
    let frames_to_note = first_note_frame - position(&h);
    for _ in 0..frames_to_note.div_ceil(64) {
        h.render();
    }
    assert!((first_note_frame..first_note_frame + 64).contains(&position(&h)));
    let mixed = h.render();
    assert!(
        mixed.iter().any(|f| (f[0] - 0.25).abs() > 0.005),
        "the shifted MIDI note should sound over the backing audio a beat earlier"
    );
    assert!(h.service.try_event().is_err());
}

#[test]
fn browser_insert_audition_survives_publication_after_the_entire_clip() {
    for (quantized, looped) in [(false, false), (true, false), (false, true)] {
        late_browser_audition(quantized, looped);
    }
}

fn late_browser_audition(quantized: bool, looped: bool) {
    use hex_engine::playback::BrowserAuditionTiming;
    use hex_project::{BeatTime, Command, ResolvedInputGrid};
    use hex_session::import_batch_decode::AudioBatchContext;

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(16_384);
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddTrack {
        name: "audition".into(),
    }));
    let (applied, _) = h.applied(initial);
    let track = added.state.project.tracks()[0].id();
    let mixed = h.run(Request::Edit(Command::Gain { track, gain: 0.5 }));
    let (applied, metadata) = h.applied(applied);
    let (revision, applied) = if looped {
        let looped = h.run(Request::Edit(Command::SetLoopRegion {
            start: 0,
            end: 512,
            enabled: true,
        }));
        (looped.state.revision, h.applied(applied).0)
    } else {
        (mixed.state.revision, applied)
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert!(h.render().iter().all(|frame| *frame == [0.0; 2]));
    let press = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    let grid = if quantized {
        ResolvedInputGrid::fixed(mixed.state.project.meter_map(), BeatTime::ONE).unwrap()
    } else {
        ResolvedInputGrid::off()
    };
    let start = grid
        .snap_frame(mixed.state.project.tempo_map(), press)
        .unwrap();
    assert_eq!(start, if quantized { 0 } else { press });

    let (entered, resume, _) = h.probe_next_native_publication();
    let (request, _) = Request::browser_audio_batch(
        files.wav(48_000, 256),
        track,
        start,
        AudioBatchContext {
            project_generation: metadata.context.project_generation,
            revision,
            active_gesture: false,
        },
        Some(BrowserAuditionTiming {
            grid,
            transport_epoch: h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .transport_epoch
                .load(Acquire),
        }),
    )
    .unwrap();
    h.service.submit(request).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    for _ in 0..6 {
        assert!(h.render().iter().all(|frame| *frame == [0.0; 2]));
    }
    let position = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
    };
    assert!(position(&h) >= start + 256);
    resume.send(()).unwrap();
    let imported = h.next();
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let clip = &imported.state.project.track(track).unwrap().clips()[0];
    assert_eq!((clip.start(), clip.duration()), (start, 256));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut audible = 0;
    let mut onset = None;
    loop {
        let before = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .performance_frame
            .load(Acquire);
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire));
        assert_eq!(status.performance_frame.load(Acquire), before + 64);
        assert_eq!(
            position(&h),
            if looped {
                (before + 64) % 512
            } else {
                before + 64
            }
        );
        if status.applied_revision.load(Acquire) > applied {
            let expected = *onset.get_or_insert(if quantized { 24_000 } else { before });
            for (index, frame) in pcm.into_iter().enumerate() {
                let at = before + index as u64;
                if (expected..expected + 256).contains(&at) {
                    assert_eq!(frame, [0.125; 2], "wrong sample at {at}");
                    audible += 1;
                } else {
                    assert_eq!(frame, [0.0; 2], "sample duplicated at {at}");
                }
            }
            if before >= expected + 256 {
                break;
            }
        }
        assert!(Instant::now() < deadline, "import did not apply");
        std::thread::yield_now();
    }
    assert_eq!(audible, 256, "monitor must hear the whole late sample");
    if looped {
        // The live read survives the first wrap, then the saved clip plays
        // normally on a later pass from its authored onset.
        assert!(onset.unwrap() < 512 && onset.unwrap() + 256 > 512);
        let mut waits = 0;
        while position(&h) != start {
            assert!(waits < 8, "saved clip never reached");
            assert_eq!(h.render(), [[0.0; 2]; 64]);
            waits += 1;
        }
        assert_eq!(h.render(), [[0.125; 2]; 64]);
    }
    let undone = h.run(Request::Undo);
    assert!(
        undone
            .state
            .project
            .track(track)
            .unwrap()
            .clips()
            .is_empty()
    );
}

#[test]
fn browser_insert_audition_replaces_the_voice_and_does_not_restart_after_stop() {
    use hex_engine::playback::BrowserAuditionTiming;
    use hex_project::{Command, ResolvedInputGrid};
    use hex_session::import_batch_decode::AudioBatchContext;

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(16_384);
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddTrack {
        name: "audition".into(),
    }));
    let (applied, metadata) = h.applied(initial);
    let track = added.state.project.tracks()[0].id();
    let gain = added.state.project.track(track).unwrap().gain();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render();

    let request = |h: &ResidentHarness, path, revision| {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let frame = status.position.load(Acquire);
        let epoch = status.transport_epoch.load(Acquire);
        let (request, _) = Request::browser_audio_batch(
            path,
            track,
            frame,
            AudioBatchContext {
                project_generation: metadata.context.project_generation,
                revision,
                active_gesture: false,
            },
            Some(BrowserAuditionTiming {
                grid: ResolvedInputGrid::off(),
                transport_epoch: epoch,
            }),
        )
        .unwrap();
        (request, frame, epoch)
    };
    let first_block = |h: &mut ResidentHarness, after| {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let pcm = h.render();
            let revision = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire);
            if revision > after {
                break (revision, pcm);
            }
            assert!(Instant::now() < deadline, "import did not apply");
            std::thread::yield_now();
        }
    };

    let (initial, _, _) = request(&h, files.wav(48_000, 1024), added.state.revision);
    let first = h.run(initial);
    let (applied, pcm) = first_block(&mut h, applied);
    assert_eq!(pcm, [[0.25 * gain; 2]; 64]);

    let (replacement, _, _) = request(
        &h,
        files.constant_wav("replacement.wav", 48_000, 128, &[16_384]),
        first.state.revision,
    );
    let second = h.run(replacement);
    let (applied, pcm) = first_block(&mut h, applied);
    assert_eq!(pcm, [[0.5 * gain; 2]; 64]);
    assert_eq!(h.render(), [[0.5 * gain; 2]; 64]);
    // The previous authored sample may leave a right-hand overwrite fragment.
    // The two blocks above must contain only the new voice.

    let (entered, resume, _) = h.probe_next_native_publication();
    let (pending, captured_frame, epoch) = request(
        &h,
        files.constant_wav("late.wav", 48_000, 512, &[8192]),
        second.state.revision,
    );
    h.service.submit(pending).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    for _ in 0..8 {
        h.render();
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(!status.playing.load(Acquire));
    assert_ne!(status.transport_epoch.load(Acquire), epoch);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(12_000))
        .unwrap();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    resume.send(()).unwrap();
    let committed = h.next();
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert!(
        committed
            .state
            .project
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .any(|clip| clip.start() == captured_frame),
        "the original placement remains a valid authored edit"
    );
    assert_eq!(first_block(&mut h, applied).1, [[0.0; 2]; 64]);
}

#[test]
fn track_gain_gesture_reaches_callback_without_a_pcm_step() {
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..12 {
        h.render();
    }
    let mut last = h.render()[63];
    assert!((last[0] - 0.25).abs() < 1e-5);
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    let began = h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    assert!(began.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    for (sequence, gain) in [(1, 0.0_f32), (2, 1.5_f32)] {
        h.service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence,
                value: ParameterValue::Float(gain),
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut event = None;
        let mut max_step = 0.0_f32;
        let mut largest = (0.0, 0.0);
        let mut blocks = 0;
        loop {
            h.pump();
            for frame in h.render() {
                for lane in 0..2 {
                    let step = (frame[lane] - last[lane]).abs();
                    if step > max_step {
                        max_step = step;
                        largest = (last[lane], frame[lane]);
                    }
                }
                last = frame;
            }
            blocks += 1;
            if event.is_none() {
                event = h.service.try_event().ok();
            }
            let active = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire);
            if let (Some(event), Some((revision, metadata))) = (&event, h.submitted)
                && event.gesture_ack.as_ref().is_some_and(|ack| ack.accepted)
                && metadata
                    .device_preview
                    .is_some_and(|preview| preview.sequence == sequence)
                && active == revision
                && revision > applied
            {
                applied = revision;
                break;
            }
            assert!(
                blocks < 450,
                "track gain preview did not apply: {:?}",
                h.submitted
            );
            assert!(Instant::now() < deadline, "track gain preview timed out");
            std::thread::sleep(Duration::from_micros(200));
        }
        for _ in 0..12 {
            for frame in h.render() {
                for lane in 0..2 {
                    let step = (frame[lane] - last[lane]).abs();
                    if step > max_step {
                        max_step = step;
                        largest = (last[lane], frame[lane]);
                    }
                }
                last = frame;
            }
        }
        assert!(
            (last[0] - 0.25 * gain).abs() < 1e-5,
            "gain {gain}: {last:?}"
        );
        assert!(
            max_step < 0.003,
            "gain {gain}: PCM step {max_step}, {largest:?}"
        );
    }
    let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
        manual_edit: true,
        key,
        sequence: 3,
        value: ParameterValue::Float(1.5),
    }));
    assert_eq!(committed.state.project.track(track).unwrap().gain(), 1.5);
    (applied, _) = h.applied(applied);
    for (request, expected) in [(Request::Undo, 1.0), (Request::Redo, 1.5)] {
        let event = h.run(request);
        assert_eq!(event.state.project.track(track).unwrap().gain(), expected);
        (applied, _) = h.applied(applied);
        for _ in 0..12 {
            last = h.render()[63];
        }
        assert!((last[0] - 0.25 * expected).abs() < 1e-5);
    }
}

pub(super) static CONFIRMED_TEST_INTERRUPTS: AtomicU64 = AtomicU64::new(0);
pub(super) static CONFIRMED_TEST_POINTS: AtomicU64 = AtomicU64::new(0);
pub(super) static CONFIRMED_TEST_READY: AtomicU64 = AtomicU64::new(0);
pub(super) static CONFIRMED_TEST_ENDS: AtomicU64 = AtomicU64::new(0);
pub(super) static CONFIRMED_TEST_NOOPS: AtomicU64 = AtomicU64::new(0);
/// Copied original SDK receipts for the independent recorded-versus-authored
/// replay oracle. This is test/control-side custody, never callback work.
pub(super) static SESSION_RECORDING_OBSERVATIONS: Mutex<
    Vec<hex_session::service::PluginParameterObservation>,
> = Mutex::new(Vec::new());
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) enum EditorRecordingProbe {
    Prepare(
        hex_session::service::PluginEditorRecordingInput,
        SyncSender<Result<hex_session::service::PluginParameterObservation, String>>,
    ),
    Start(
        hex_session::device_gesture::GestureKey,
        SyncSender<Result<(), String>>,
    ),
    Take(SyncSender<Option<hex_session::service::PluginParameterObservation>>),
}

struct ResidentSink {
    inner: NativeAudioSink,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    control_polls: Arc<AtomicU64>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    editor_recording_probe: Arc<Mutex<Option<EditorRecordingProbe>>>,
    initial_rate: u32,
    refuse: Arc<AtomicBool>,
    budget_refuse: Arc<AtomicBool>,
    loop_witness_fault: Arc<AtomicU8>,
    pending_present: Arc<AtomicBool>,
    file_publication_waiting: Arc<AtomicBool>,
    pending_observation_generation: Arc<AtomicU64>,
    publication_probe: Arc<Mutex<Option<PublicationProbe>>>,
    confirmation_probe: Arc<Mutex<Option<ConfirmationProbe>>>,
}
struct ConfirmationProbe {
    ready: SyncSender<()>,
    resume: mpsc::Receiver<()>,
}
struct PublicationProbe {
    ready: SyncSender<()>,
    resume: mpsc::Receiver<()>,
    busy: SyncSender<()>,
}
impl WorkerAudioSink for ResidentSink {
    fn capture_plugin_states(
        &mut self,
        input: hex_session::service::PluginStateCaptureInput<'_>,
    ) -> Result<Vec<hex_session::service::PluginStateCapture>, String> {
        self.inner.capture_plugin_states(input)
    }
    fn begin_request(&mut self, cancellation: hex_session::service::RequestCancellation) {
        self.inner.begin_request(cancellation);
    }
    fn finish_resident_effect_edit(&mut self, context: AudioSourceContext, accepted: bool) {
        self.inner.finish_resident_effect_edit(context, accepted);
    }
    fn plugin_state_restore_pending(&self, context: AudioSourceContext) -> bool {
        self.inner.plugin_state_restore_pending(context)
    }
    fn plugin_state_restore_ready(&self) -> Option<AudioSourceContext> {
        self.inner.plugin_state_restore_ready()
    }
    fn finish_plugin_state_restore(
        &mut self,
        context: AudioSourceContext,
        result: Result<(), &str>,
    ) {
        self.inner.finish_plugin_state_restore(context, result);
    }
    fn plugin_parameter_requires_stopped_restore(
        &mut self,
        context: AudioSourceContext,
        origin: hex_session::device_gesture::PluginParameterOrigin,
    ) -> Result<bool, String> {
        self.inner
            .plugin_parameter_requires_stopped_restore(context, origin)
    }
    fn prepare_stopped_playback_restore(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<Option<bool>, String> {
        self.inner.prepare_stopped_playback_restore(context)
    }
    fn stopped_playback_restore_ready(&self) -> bool {
        self.inner.stopped_playback_restore_ready()
    }
    fn cancel_stopped_playback_restore(&mut self) {
        self.inner.cancel_stopped_playback_restore();
    }
    fn prepare_plugin_effect(
        &mut self,
        input: hex_session::service::PluginEffectPreparation<'_>,
    ) -> Result<hex_session::service::PluginEffectPreparationState, String> {
        self.inner.prepare_plugin_effect(input)
    }
    fn plugin_effect_ready(&self) -> bool {
        self.inner.plugin_effect_ready()
    }
    fn toggle_plugin_editor(&mut self, generation: u64, device: u64) -> Result<(), String> {
        self.inner.toggle_plugin_editor(generation, device)
    }
    fn prepare_plugin_parameter_gesture(
        &mut self,
        input: hex_session::service::PluginParameterGestureInput<'_>,
    ) -> Result<
        hex_session::service::PluginParameterGestureResult,
        hex_session::service::PluginParameterGestureError,
    > {
        use hex_session::device_gesture::PluginGesturePreparation;
        let case = std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE").unwrap_or_default();
        let retire = matches!(
            case.as_str(),
            "delivery_retire" | "touch_close_pending" | "touch_close_queued"
        );
        let touch = case.starts_with("touch");
        let close = case == "touch_close_begin"
            && matches!(input.operation(), PluginGesturePreparation::Begin { .. })
            || case == "touch_close_point"
                && matches!(input.operation(), PluginGesturePreparation::Preview(_))
            || case == "touch_close_restored"
                && matches!(input.operation(), PluginGesturePreparation::Commit(_));
        let preview = matches!(input.operation(), PluginGesturePreparation::Preview(_));
        let confirm = touch || (retire || case == "delivery") && preview;
        let expected = match input.operation() {
            PluginGesturePreparation::Begin { snapshot, .. }
            | PluginGesturePreparation::Preview(snapshot)
            | PluginGesturePreparation::Commit(snapshot)
            | PluginGesturePreparation::Restore { snapshot, .. } => snapshot,
        };
        let result = self.inner.prepare_plugin_parameter_gesture(if touch {
            input.with_touch_recording()
        } else if confirm {
            input.with_delivery_confirmation()
        } else {
            input
        });
        if confirm {
            if close {
                assert!(result.is_ok());
                self.inner.relinquish_output();
            }
            if retire && preview {
                assert!(result.is_err());
                self.inner.relinquish_output();
            }
            while let Some(observation) = self.inner.take_plugin_parameter_observation() {
                use hex_session::service::PluginParameterObservation as Observation;
                match observation {
                    Observation::Grouped(_) => {
                        panic!("generic parameter gesture emitted a grouped editor observation");
                    }
                    Observation::Ready {
                        key,
                        context,
                        target,
                        source,
                    } => {
                        assert_eq!(key, expected.key);
                        assert_eq!(context.project_generation, key.generation);
                        assert_eq!(
                            source.target,
                            hex_session::service::DeviceCaptureTarget::Vst3(target)
                        );
                        assert_eq!(source.sample_rate, self.initial_rate);
                        assert_eq!(
                            CONFIRMED_TEST_POINTS.load(std::sync::atomic::Ordering::Relaxed),
                            CONFIRMED_TEST_ENDS.load(std::sync::atomic::Ordering::Relaxed)
                        );
                        CONFIRMED_TEST_READY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Observation::Delivered(point) | Observation::Restored(point) => {
                        assert_eq!(point.key, expected.key);
                        assert!(point.sequence <= expected.sequence);
                        assert_eq!(point.source.sample_rate, self.initial_rate);
                        assert_eq!(
                            point.source.target,
                            hex_session::service::DeviceCaptureTarget::Vst3(point.target)
                        );
                        assert_eq!(point.application.parameter, point.target.parameter);
                        assert_eq!(point.application.binding, point.target.binding);
                        assert!(
                            point.delivery_performance_frame
                                >= point.application.origin.performance_frame
                        );
                        match point.application.value {
                            hex_session::service::automation_draft::Vst3ApplicationValue::Value(
                                value,
                            ) => {
                                assert_eq!(value, expected.plugin_value.unwrap().normalized());
                                CONFIRMED_TEST_POINTS
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            hex_session::service::automation_draft::Vst3ApplicationValue::Restored => {
                                CONFIRMED_TEST_ENDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                    Observation::Unchanged { key } => {
                        assert_eq!(key, expected.key);
                        CONFIRMED_TEST_NOOPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Observation::Interrupted {
                        key,
                        context,
                        target,
                        source,
                        ..
                    } => {
                        assert!(case.starts_with("touch_close_"));
                        assert_eq!(key, expected.key);
                        assert_eq!(context.project_generation, key.generation);
                        assert_eq!(
                            source.target,
                            hex_session::service::DeviceCaptureTarget::Vst3(target)
                        );
                        assert_eq!(
                            CONFIRMED_TEST_POINTS.load(std::sync::atomic::Ordering::Relaxed),
                            u64::from(case != "touch_close_begin")
                        );
                        CONFIRMED_TEST_INTERRUPTS
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        }
        result
    }
    fn prepare_plugin_editor_recording(
        &mut self,
        input: hex_session::service::PluginEditorRecordingInput,
    ) -> Result<hex_session::service::PluginParameterObservation, String> {
        self.inner.prepare_plugin_editor_recording(input)
    }
    fn start_plugin_editor_recording(
        &mut self,
        key: hex_session::device_gesture::GestureKey,
    ) -> Result<(), String> {
        self.inner.start_plugin_editor_recording(key)
    }
    fn cancel_plugin_editor_recording(
        &mut self,
        key: hex_session::device_gesture::GestureKey,
    ) -> bool {
        self.inner.cancel_plugin_editor_recording(key)
    }
    fn take_plugin_parameter_observation(
        &mut self,
    ) -> Option<hex_session::service::PluginParameterObservation> {
        let observation = self.inner.take_plugin_parameter_observation();
        if (std::env::var("HEX_DESKTOP_ORIGINAL_EDITOR_RECORDING_CHILD")
            .is_ok_and(|case| case == "session" || case.starts_with("group-"))
            || std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE").is_ok_and(|case| {
                case.starts_with("session_record")
                    || matches!(
                        case.as_str(),
                        "session_first_lane" | "session_first_lane_with_other"
                    )
                    || case == "session_recover"
                    || case.starts_with("session_writer_")
            }))
            && let Some(observation) = observation
        {
            let mut originals = SESSION_RECORDING_OBSERVATIONS.lock().unwrap();
            assert!(originals.len() < 8, "one bounded recording oracle");
            originals.push(observation);
        }
        observation
    }
    fn device_automation_source(
        &mut self,
        context: hex_engine::AudioSourceContext,
        target: hex_session::service::DeviceCaptureTarget,
    ) -> Result<Option<hex_session::service::DeviceAutomationSource>, String> {
        self.inner.device_automation_source(context, target)
    }
    fn confirm_plugin_parameter_publication(
        &mut self,
        context: AudioSourceContext,
        acceptance: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<(), hex_session::service::PluginParameterGestureError> {
        let probe = self.confirmation_probe.lock().unwrap().take();
        if let Some(probe) = probe {
            probe.ready.send(()).unwrap();
            probe.resume.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        self.inner
            .confirm_plugin_parameter_publication(context, acceptance)
    }
    fn confirm_source_effect_publication(
        &mut self,
        context: AudioSourceContext,
        acceptance: ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        // A headless fixture must pump the real callback while the service
        // waits for the source graph's exact publication acknowledgement.
        self.file_publication_waiting
            .store(true, std::sync::atomic::Ordering::Release);
        let result = self
            .inner
            .confirm_source_effect_publication(context, acceptance);
        self.file_publication_waiting
            .store(false, std::sync::atomic::Ordering::Release);
        result
    }
    fn finish_plugin_parameter_commit(
        &mut self,
        context: AudioSourceContext,
        key: hex_session::device_gesture::GestureKey,
        sequence: u64,
        accepted: bool,
    ) {
        self.inner
            .finish_plugin_parameter_commit(context, key, sequence, accepted);
    }
    fn query_plugin_parameters(
        &mut self,
        query: hex_session::service::PluginParameterQuery,
    ) -> Result<(), String> {
        self.inner.query_plugin_parameters(query)
    }
    fn plugin_parameters_ready(&self) -> bool {
        self.inner.plugin_parameters_ready()
    }
    fn take_plugin_parameters(&mut self) -> Option<hex_session::service::PluginParameterResult> {
        self.inner.take_plugin_parameters()
    }
    fn query_plugin_parameter_conversion(
        &mut self,
        query: hex_session::service::PluginParameterConversionQuery,
    ) -> Result<(), String> {
        self.inner.query_plugin_parameter_conversion(query)
    }
    fn plugin_parameter_conversion_ready(&self) -> bool {
        self.inner.plugin_parameter_conversion_ready()
    }
    fn take_plugin_parameter_conversion(
        &mut self,
    ) -> Option<hex_session::service::PluginParameterConversionResult> {
        self.inner.take_plugin_parameter_conversion()
    }
    fn plugin_editor_ready(&self) -> bool {
        self.inner.plugin_editor_ready()
    }
    fn take_plugin_editor_result(&mut self, generation: u64) -> Option<Result<bool, String>> {
        self.inner.take_plugin_editor_result(generation)
    }
    fn finish_plugin_effect(
        &mut self,
        control: &Arc<hex_session::service::PluginEffectControl>,
        accepted: bool,
    ) {
        self.inner.finish_plugin_effect(control, accepted);
    }
    fn prepare_resident_effects(
        &mut self,
        input: hex_session::service::ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<hex_engine::PreparedRevision>, String> {
        self.inner.prepare_resident_effects(input, prepare)
    }
    fn begin_stopped_foreign_history(
        &mut self,
        predecessor: hex_session::service::ResidentGraphSubmission,
        context: AudioSourceContext,
    ) -> Result<bool, String> {
        self.inner
            .begin_stopped_foreign_history(predecessor, context)
    }
    fn confirm_stopped_foreign_history(
        &mut self,
        context: AudioSourceContext,
        accepted: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        self.inner
            .confirm_stopped_foreign_history(context, accepted)
    }
    fn received_foreign_program_on_control(
        &mut self,
        binding: hex_audio::plugin_effects::EffectBinding,
    ) -> hex_engine::ForeignProgramFrontier {
        self.inner.received_foreign_program_on_control(binding)
    }
    fn prepare_offline_bounce(
        &mut self,
        input: hex_session::service::ResidentEffectPreparation<'_>,
        control: &hex_export::ExportControl,
    ) -> Result<Option<Box<dyn hex_session::service::OfflineBounceSource>>, String> {
        self.inner.prepare_offline_bounce(input, control)
    }
    fn ordinary_source_reservation(&self) -> Option<hex_session::SourcePlanReservation> {
        self.inner.ordinary_source_reservation()
    }
    fn prepare_resident_file_metadata(
        &mut self,
        input: hex_session::service::ResidentFileInput<'_>,
    ) -> Result<Option<hex_session::service::ResidentFileMetadata>, String> {
        self.inner.prepare_resident_file_metadata(input)
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Exact production inputs and injected final admission refusal.
    fn publish_resident_with_files(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
        files: Option<hex_session::service::ResidentFilePublication<'_>>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        self.file_publication_waiting
            .store(files.is_some(), std::sync::atomic::Ordering::Release);
        let refuse = self.refuse.load(Acquire);
        if let Some(probe) = self.publication_probe.lock().unwrap().take() {
            self.inner.native_graph_gate = Some((probe.ready, probe.resume));
            self.inner.native_graph_busy_observer = Some(probe.busy);
        }
        let result = self.inner.publish_resident_with_files(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            &mut |actual| {
                admit(actual)?;
                if refuse {
                    Err("fixture final admission refusal".into())
                } else {
                    Ok(())
                }
            },
            files,
        );
        self.inner.native_graph_gate = None;
        self.inner.native_graph_busy_observer = None;
        self.file_publication_waiting
            .store(false, std::sync::atomic::Ordering::Release);
        result
    }
    fn streamed_exporter(&self) -> Option<hex_session::service::StreamedExporter> {
        self.inner.streamed_exporter()
    }
    fn streamed_bouncer(&self) -> Option<hex_session::StreamedBouncer> {
        self.inner.streamed_bouncer()
    }
    fn resident_file_seek_preparer(
        &self,
    ) -> Option<hex_session::service::ResidentFileSeekPreparer> {
        self.inner.resident_file_seek_preparer()
    }
    fn resident_file_seek_storage_bytes(
        &self,
        prepared: &hex_engine::PreparedRevision,
        sources: &hex_session::service::ResidentFileSet,
    ) -> Result<usize, String> {
        self.inner
            .resident_file_seek_storage_bytes(prepared, sources)
    }

    fn resident_runtime_loop(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<Option<hex_session::service::ResidentRuntimeLoop>, String> {
        let Some(mut actual) = self.inner.resident_runtime_loop(context)? else {
            return Ok(None);
        };
        // Emulate a stale view between capture and final worker publication.
        // The native owner still observes the real output and refuses this copy.
        match self.loop_witness_fault.load(Acquire) {
            1 => actual.output_identity += 1,
            2 => actual.audio_revision += 1,
            3 => {
                if let Some(source) = actual.source_context.as_mut() {
                    source.project_generation += 1;
                }
            }
            4 => {
                if let Some(source) = actual.source_context.as_mut() {
                    source.editor_revision += 1;
                }
            }
            5 => {
                actual.section_loop = if actual.section_loop.is_some() {
                    None
                } else {
                    Some(hex_project::LoopRegion::new(0, 48_000, true).unwrap())
                };
            }
            6 => {
                if let Some(region) = actual.section_loop {
                    actual.section_loop = Some(
                        hex_project::LoopRegion::new(region.start() + 1, region.end(), true)
                            .unwrap(),
                    );
                } else {
                    actual.section_loop =
                        Some(hex_project::LoopRegion::new(1, 48_000, true).unwrap());
                }
            }
            _ => {}
        }
        Ok(Some(actual))
    }
    fn resident_publication_enabled(&self) -> bool {
        self.inner.resident_publication_enabled()
    }
    fn resident_publication_readiness(
        &mut self,
    ) -> hex_session::service::ResidentPublicationReadiness {
        self.inner.resident_publication_readiness()
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        self.inner.resident_admission()
    }
    fn prepare_paused_input_monitor(
        &mut self,
        request: hex_session::service::PausedInputMonitorRequest,
        revision: &Arc<hex_engine::PreparedRevision>,
        resources: &PreparedDeviceResources,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<hex_engine::PausedTrackMonitor, String> {
        self.inner
            .prepare_paused_input_monitor(request, revision, resources, admit)
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Mirror the exact production ownership boundary.
    fn publish_resident(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        let refuse = self.refuse.load(Acquire);
        self.inner.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            &mut |actual| {
                admit(actual)?;
                if refuse {
                    Err("fixture final admission refusal".into())
                } else {
                    Ok(())
                }
            },
        )
    }
    fn poll_resident_pending(
        &mut self,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) {
        let refuse = self.refuse.load(Acquire);
        self.inner.poll_resident_pending(&mut |views| {
            admit(views)?;
            if refuse {
                Err("fixture final admission refusal".into())
            } else {
                Ok(())
            }
        });
    }
    fn begin_section_edit(
        &mut self,
        context: AudioSourceContext,
        allow_playing: bool,
    ) -> Result<(), String> {
        WorkerAudioSink::begin_section_edit(&mut self.inner, context, allow_playing)
    }
    fn begin_recoverable_section_edit(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<(), String> {
        self.inner.begin_recoverable_section_edit(context)
    }
    fn confirm_section_edit_recovery(
        &mut self,
        context: AudioSourceContext,
        acceptance: ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        self.inner
            .confirm_section_edit_recovery(context, acceptance)
    }
    fn section_edit_ready(
        &mut self,
    ) -> Option<Result<hex_session::service::ArrangementEditPosition, String>> {
        self.inner.section_edit_ready()
    }
    fn cancel_section_edit(&mut self) {
        self.inner.cancel_section_edit();
    }
    fn restart_audio(&mut self, generation: u64, request_id: u64) -> Result<(), String> {
        self.inner.restart_audio(generation, request_id)
    }
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String> {
        self.inner.initialize(shutdown)?;
        // The fixture selects its canonical rate before device completion.
        self.inner.requested_rate = Some(self.initial_rate);
        Ok(self.initial_rate)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        self.inner.canonical_changed(source);
    }
    fn native_sampler_hold(&self) -> hex_session::service::NativeSamplerReservationHold {
        let mut hold = self.inner.native_sampler_hold();
        if self.budget_refuse.load(Acquire) {
            hold.retained_bytes = usize::MAX;
            hold.construction_bytes = usize::MAX;
        }
        hold
    }
    fn poll(&mut self) {
        self.inner.poll();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some(probe) = self.editor_recording_probe.lock().unwrap().take() {
            match probe {
                EditorRecordingProbe::Prepare(input, reply) => {
                    reply
                        .try_send(self.inner.prepare_plugin_editor_recording(input))
                        .unwrap();
                }
                EditorRecordingProbe::Start(key, reply) => {
                    reply
                        .try_send(self.inner.start_plugin_editor_recording(key))
                        .unwrap();
                }
                EditorRecordingProbe::Take(reply) => {
                    reply
                        .try_send(self.inner.take_plugin_parameter_observation())
                        .unwrap();
                }
            }
        }
        self.pending_observation_generation.store(
            match self.inner.pending_observation.as_ref() {
                Some(EditorAudioEvent::Publication { context, .. }) => {
                    context.project_generation + 1
                }
                _ => 0,
            },
            std::sync::atomic::Ordering::Release,
        );
        self.pending_present.store(
            self.inner.resident.pending.is_some(),
            std::sync::atomic::Ordering::Release,
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.control_polls
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    fn close(&mut self) {
        self.inner.close();
    }
}

pub(super) struct ResidentHarness {
    pub(super) service: Service,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) control_polls: Arc<AtomicU64>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) editor_recording_probe: Arc<Mutex<Option<EditorRecordingProbe>>>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    event_sender: mpsc::SyncSender<EditorAudioEvent>,
    events: mpsc::Receiver<EditorAudioEvent>,
    pub(super) callback: Arc<Mutex<Option<HeadlessBackend>>>,
    pub(super) audio: Option<EditorAudio>,
    pub(super) submitted: Option<(u64, ResidentPublicationMetadata)>,
    submitted_observation: Option<Arc<PreparedPlaybackObservation>>,
    pub(super) refuse: Arc<AtomicBool>,
    pub(super) budget_refuse: Arc<AtomicBool>,
    pub(super) loop_witness_fault: Arc<AtomicU8>,
    pub(super) pending_present: Arc<AtomicBool>,
    file_publication_waiting: Arc<AtomicBool>,
    pub(super) pending_observation_generation: Arc<AtomicU64>,
    publication_probe: Arc<Mutex<Option<PublicationProbe>>>,
    confirmation_probe: Arc<Mutex<Option<ConfirmationProbe>>>,
    allow_unavailable: bool,
    pub(super) unavailable: Vec<String>,
    pub(super) publication_refusals: Vec<String>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) degraded_devices: Vec<u64>,
    constructor_submitted: Option<(u64, AudioSourceContext)>,
    /// `OutputReleased` events in delivery order: (generation, old output).
    pub(super) released: Vec<(u64, u64)>,
    pub(super) rebound: Option<(u64, u64)>,
    native_plugins: bool,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_completed: Arc<AtomicU64>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) latency_restart: Option<hex_audio::prepared::AppliedAudioSource>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_queued: Arc<AtomicBool>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_records: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_progress: Arc<Mutex<crate::native_audio::plugins::PreparationObservation>>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_failure: Arc<Mutex<Option<String>>>,
}
impl ResidentHarness {
    pub(super) fn new() -> Self {
        Self::with_frame_limit(8192)
    }
    pub(super) fn with_frame_limit(frame_limit: u64) -> Self {
        Self::with_open_gate(frame_limit, Arc::new(AtomicBool::new(true)))
    }
    pub(super) fn with_frame_limit_at_rate(frame_limit: u64, rate: u32) -> Self {
        Self::with_event_capacity_at_rate(frame_limit, Arc::new(AtomicBool::new(true)), 4, rate)
    }
    pub(super) fn with_open_gate(frame_limit: u64, gate: Arc<AtomicBool>) -> Self {
        Self::with_event_capacity(frame_limit, gate, 4)
    }
    pub(super) fn with_event_capacity(
        frame_limit: u64,
        gate: Arc<AtomicBool>,
        capacity: usize,
    ) -> Self {
        Self::with_event_capacity_at_rate(frame_limit, gate, capacity, 48_000)
    }
    fn with_event_capacity_at_rate(
        frame_limit: u64,
        gate: Arc<AtomicBool>,
        capacity: usize,
        rate: u32,
    ) -> Self {
        Self::with_plugin_fixture(frame_limit, gate, capacity, rate, None, None, None, None)
    }
    pub(super) fn with_recovery(frame_limit: u64, recovery: PathBuf) -> Self {
        Self::with_recovery_at_rate(frame_limit, recovery, 48_000)
    }
    pub(super) fn with_recovery_at_rate(frame_limit: u64, recovery: PathBuf, rate: u32) -> Self {
        Self::with_plugin_fixture(
            frame_limit,
            Arc::new(AtomicBool::new(true)),
            4,
            rate,
            None,
            None,
            None,
            Some(recovery),
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    ) -> Self {
        Self::with_plugins_at_rate(host, presentation, 48_000)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_at_rate(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        rate: u32,
    ) -> Self {
        Self::with_plugin_fixture(
            8192,
            Arc::new(AtomicBool::new(true)),
            4,
            rate,
            Some((host, presentation, None)),
            None,
            None,
            None,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_paused_at_facts(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        facts: crate::native_audio::plugins::FactsGate,
    ) -> Self {
        Self::with_plugin_fixture(
            8192,
            Arc::new(AtomicBool::new(true)),
            4,
            48_000,
            Some((host, presentation, None)),
            Some(facts),
            None,
            None,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_and_catalog(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        catalog: PathBuf,
    ) -> Self {
        Self::with_plugins_at_rate_and_catalog(host, presentation, 48_000, catalog, 8192)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_catalog_and_diagnostics(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        catalog: PathBuf,
        diagnostics: hex_diagnostics::DiagnosticsHandle,
    ) -> Self {
        Self::with_plugin_fixture(
            8192,
            Arc::new(AtomicBool::new(true)),
            4,
            48_000,
            Some((host, presentation, Some(diagnostics))),
            None,
            Some(catalog),
            None,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_at_rate_and_catalog(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        rate: u32,
        catalog: PathBuf,
        frame_limit: u64,
    ) -> Self {
        Self::with_plugin_fixture(
            frame_limit,
            Arc::new(AtomicBool::new(true)),
            4,
            rate,
            Some((host, presentation, None)),
            None,
            Some(catalog),
            None,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_at_rate_and_catalog_recovery(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        rate: u32,
        frame_limit: u64,
        catalog: PathBuf,
        recovery: PathBuf,
    ) -> Self {
        Self::with_plugin_fixture(
            frame_limit,
            Arc::new(AtomicBool::new(true)),
            4,
            rate,
            Some((host, presentation, None)),
            None,
            Some(catalog),
            Some(recovery),
        )
    }
    #[allow(clippy::too_many_arguments)] // Test fixture covers independent native failure modes.
    fn with_plugin_fixture(
        frame_limit: u64,
        gate: Arc<AtomicBool>,
        capacity: usize,
        rate: u32,
        plugin: Option<PluginFixture>,
        facts_gate: Option<PluginFacts>,
        catalog: Option<PathBuf>,
        recovery: Option<PathBuf>,
    ) -> Self {
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        assert!(plugin.is_none() && facts_gate.is_none() && catalog.is_none());
        let native_plugins = plugin.is_some();
        let allow_unavailable = !gate.load(Acquire);
        let (sender, events) = mpsc::sync_channel(capacity);
        let callback = Arc::new(Mutex::new(None));
        let target = Arc::clone(&callback);
        let mut inner = NativeAudioSink::new(sender.clone());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_completed = Arc::new(AtomicU64::new(0));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_queued = Arc::new(AtomicBool::new(false));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_records = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_progress = Arc::new(Mutex::new(
            crate::native_audio::plugins::PreparationObservation::default(),
        ));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_failure = Arc::new(Mutex::new(None));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some((host, presentation, diagnostics)) = plugin {
            inner.plugins = crate::native_audio::plugins::Plugins::with_fixture(host, presentation);
            inner.plugins.set_diagnostics(diagnostics);
            inner.plugins.catalog = catalog;
            inner.plugins.facts_gate = facts_gate;
            inner.plugins.progress = Some(Arc::clone(&plugin_progress));
            inner.plugins.failure_observation = Some(Arc::clone(&plugin_failure));
            // This optional Session-poll diagnostic can lag actual DSP replies.
            // The continuity driver must work without it as a processing clock.
            if std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE").as_deref()
                != Ok("touch_control_lag")
            {
                inner.plugins.completed = Some(Arc::clone(&plugin_completed));
            }
            inner.plugins.queued = Some(Arc::clone(&plugin_queued));
            inner.plugins.records = Some(Arc::clone(&plugin_records));
        }
        assert!(
            inner.worker_owned_resident_graphs,
            "production resident publication default"
        );
        inner.output_factory = Some(Box::new(move |requested_rate| {
            if !gate.load(Acquire) {
                return Err("fixture output unavailable".into());
            }
            // The prior fixture's async native carrier can still own the exclusive Lease
            // after its control owner has returned. Retry exactly as the existing Harness does.
            let (output, backend) =
                match hex_audio::worker_fixture::open_with_reliable_action_outcomes(
                    requested_rate.unwrap_or(rate),
                ) {
                    Ok(pair) => pair,
                    Err(error) if error == "audio output is open or still shutting down" => {
                        return Ok(None);
                    }
                    Err(error) => return Err(error.into()),
                };
            *target.lock().unwrap() = Some(backend);
            Ok(Some(output))
        }));
        let refuse = Arc::new(AtomicBool::new(false));
        let budget_refuse = Arc::new(AtomicBool::new(false));
        let loop_witness_fault = Arc::new(AtomicU8::new(0));
        let pending_present = Arc::new(AtomicBool::new(false));
        let file_publication_waiting = Arc::new(AtomicBool::new(false));
        let pending_observation_generation = Arc::new(AtomicU64::new(0));
        let publication_probe = Arc::new(Mutex::new(None));
        let confirmation_probe = Arc::new(Mutex::new(None));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let control_polls = Arc::new(AtomicU64::new(0));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let editor_recording_probe = Arc::new(Mutex::new(None));
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            frame_limit,
            recovery,
            Box::new(ResidentSink {
                inner,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                control_polls: Arc::clone(&control_polls),
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                editor_recording_probe: Arc::clone(&editor_recording_probe),
                initial_rate: rate,
                refuse: Arc::clone(&refuse),
                budget_refuse: Arc::clone(&budget_refuse),
                loop_witness_fault: Arc::clone(&loop_witness_fault),
                pending_present: Arc::clone(&pending_present),
                file_publication_waiting: Arc::clone(&file_publication_waiting),
                pending_observation_generation: Arc::clone(&pending_observation_generation),
                publication_probe: Arc::clone(&publication_probe),
                confirmation_probe: Arc::clone(&confirmation_probe),
            }),
            AudioPolicy::new().unwrap().resident_native,
        )
        .unwrap();
        Self {
            service,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            control_polls,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            editor_recording_probe,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            event_sender: sender,
            events,
            callback,
            audio: None,
            submitted: None,
            submitted_observation: None,
            refuse,
            budget_refuse,
            loop_witness_fault,
            pending_present,
            file_publication_waiting,
            pending_observation_generation,
            publication_probe,
            confirmation_probe,
            allow_unavailable,
            unavailable: Vec::new(),
            publication_refusals: Vec::new(),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            degraded_devices: Vec::new(),
            constructor_submitted: None,
            released: Vec::new(),
            rebound: None,
            native_plugins,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_completed,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            latency_restart: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_queued,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_records,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_progress,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_failure,
        }
    }
    /// Attach the actual already-open output and latest submission observation
    /// to a rendered view. These are original events, never manufactured ACKs;
    /// the simulated callback and publication owner stay in their usual custody.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn into_plugin_ui_backend(mut self) -> EditorBackend {
        let audio = self.audio.take().expect("actual opened output");
        let output_identity = audio.control.output_identity().unwrap();
        let (audio_revision, metadata) = self.submitted.unwrap();
        assert_eq!(
            audio.status().unwrap().applied_revision.load(Acquire),
            audio_revision
        );
        self.event_sender
            .try_send(EditorAudioEvent::Opened {
                project_generation: metadata.context.project_generation,
                restart_request: None,
                audio,
            })
            .unwrap_or_else(|_| panic!("actual opened event handoff"));
        self.event_sender
            .try_send(EditorAudioEvent::Publication {
                context: metadata.context,
                output_identity,
                state: EditorPublicationState::ResidentSubmitted {
                    audio_revision,
                    metadata,
                    observation: self.submitted_observation.take().unwrap(),
                },
            })
            .unwrap_or_else(|_| panic!("actual submission event handoff"));
        EditorBackend {
            service: self.service,
            audio_events: self.events,
            source_reservation: AudioPolicy::source_reservation(),
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn submitted_observation(&self) -> Option<&PreparedPlaybackObservation> {
        self.submitted_observation.as_deref()
    }
    pub(super) fn pump(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                EditorAudioEvent::Opened { audio, .. } => self.audio = Some(audio),
                EditorAudioEvent::OutputRebound {
                    project_generation,
                    output_identity,
                } => {
                    let audio = self.audio.as_mut().unwrap();
                    assert_eq!(audio.control.output_identity(), Some(output_identity));
                    audio.control.bind_resident_transport(project_generation);
                    self.rebound = Some((project_generation, output_identity));
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                EditorAudioEvent::NativeLatencyRestart { source } => {
                    self.latency_restart = Some(source)
                }
                EditorAudioEvent::Publication {
                    state:
                        EditorPublicationState::ResidentSubmitted {
                            audio_revision,
                            metadata,
                            observation,
                        },
                    ..
                } => {
                    self.submitted = Some((audio_revision, metadata));
                    self.submitted_observation = Some(observation);
                }
                EditorAudioEvent::Unavailable { error, .. } => {
                    assert!(self.allow_unavailable, "{error}");
                    self.unavailable.push(error.to_string());
                }
                EditorAudioEvent::Publication {
                    context,
                    state: EditorPublicationState::Submitted { audio_revision },
                    ..
                } => self.constructor_submitted = Some((audio_revision, context)),
                EditorAudioEvent::Publication {
                    state: EditorPublicationState::Refused(error),
                    ..
                } => self.publication_refusals.push(error),
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                EditorAudioEvent::DegradedPlugins { devices, .. } => {
                    self.degraded_devices = devices;
                }
                EditorAudioEvent::OutputReleased {
                    project_generation,
                    old_output_identity,
                } => {
                    // Tests close the held endpoint with the headless carrier
                    // in `release_relinquished_output`; record the outcome.
                    self.released
                        .push((project_generation, old_output_identity));
                }
                _ => {}
            }
        }
    }
    #[track_caller]
    pub(super) fn next(&mut self) -> Event {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            if self.file_publication_waiting.load(Acquire) {
                self.render_worker_step();
            }
            if let Ok(event) = self.service.try_event() {
                return event;
            }
            assert!(Instant::now() < end, "canonical event timed out");
            std::thread::yield_now();
        }
    }
    pub(super) fn run(&mut self, request: Request) -> Event {
        self.service.submit(request).unwrap();
        let event = self.next();
        assert!(event.error.is_none(), "{:?}", event.error);
        assert!(
            event.playback.is_none(),
            "native graph must stay worker-private"
        );
        event
    }
    pub(super) fn wait_constructor_applied(&mut self, context: AudioSourceContext) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            self.render();
            if let Some(applied) = self
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source()
                && applied.context == context
                && Some((applied.audio_revision, context)) == self.constructor_submitted
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "verified constructor did not apply its exact context"
            );
            std::thread::yield_now();
        }
    }
    pub(super) fn release_relinquished_output(&mut self) {
        let mut slot = self.callback.lock().unwrap();
        assert!(slot.as_ref().unwrap().shutdown_requested());
        slot.take().unwrap().close();
        self.audio = None;
        self.submitted = None;
        self.constructor_submitted = None;
    }
    pub(super) fn wait_fresh_output(&mut self, old_output: u64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            if self.audio.as_ref().is_some_and(|audio| {
                audio
                    .control
                    .output_identity()
                    .is_some_and(|id| id != old_output)
            }) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "fresh output did not open after actual carrier/client release"
            );
            std::thread::yield_now();
        }
    }
    pub(super) fn render(&self) -> [hex_engine::Frame; 64] {
        let mut pcm = [[0.0; 2]; 64];
        let mut slot = self.callback.lock().unwrap();
        let callback = slot.as_mut().unwrap();
        let mut host = [0.0; 128];
        let allocations = allocation_counter::measure(|| {
            if self.native_plugins {
                callback.render_native_direct(&mut pcm, &mut host).unwrap();
            } else {
                callback.render(&mut pcm).unwrap();
            }
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        pcm
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn render_plugin_block(&self) -> [hex_engine::Frame; 256] {
        let mut pcm = [[0.0; 2]; 256];
        let mut host = [0.0; 512];
        let mut slot = self.callback.lock().unwrap();
        let callback = slot.as_mut().unwrap();
        let mut rendered = Ok(());
        let counts = allocation_counter::measure(|| {
            rendered = callback.render_native_direct(&mut pcm, &mut host);
        });
        assert!(
            rendered.is_ok(),
            "native render: {rendered:?}, fault={:?}, stream={:?}, worker={:?}",
            self.audio
                .as_ref()
                .and_then(EditorAudio::status)
                .and_then(|status| status.render_fault_snapshot()),
            self.audio
                .as_ref()
                .and_then(EditorAudio::status)
                .map(|status| (
                    status.stream_buffering.load(Acquire),
                    status.stream_committed_frames.load(Acquire),
                )),
            callback.native_worker_phase_for_test(),
        );
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(host.chunks_exact(2).zip(&pcm).all(|(h, p)| h == p));
        pcm
    }
    /// Advance the simulated callback, then observe each exact original
    /// endpoint's committed requests on control. The Session worker may be
    /// waiting for page/callback progress and is not the DSP completion clock.
    pub(super) fn render_worker_step(&self) {
        self.render();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self.native_plugins
            && self
                .audio
                .as_ref()
                .and_then(EditorAudio::status)
                .is_some_and(|status| {
                    status.playing.load(Acquire) && status.native_effect_source().is_some()
                })
        {
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                let ready = self
                    .callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control();
                if ready == Some(true) {
                    break;
                }
                assert!(
                    Instant::now() < end,
                    "original worker requests did not complete"
                );
                std::thread::yield_now();
            }
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn hold_native_work_on_control(
        &self,
        held: SyncSender<()>,
        release: mpsc::Receiver<()>,
    ) -> std::thread::JoinHandle<()> {
        let callback = Arc::clone(&self.callback);
        std::thread::spawn(move || {
            let result = {
                callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .with_native_source_work_held_on_control(|| {
                        held.send(()).map_err(|error| error.to_string())?;
                        release
                            .recv_timeout(Duration::from_secs(10))
                            .map_err(|error| error.to_string())
                    })
            };
            // A broken fixture channel must fail after both original locks
            // are released, without poisoning Root's production Work mutex.
            assert_eq!(result, Some(Ok(())));
        })
    }
    pub(super) fn probe_next_parameter_confirmation(&self) -> (mpsc::Receiver<()>, SyncSender<()>) {
        let (ready, entered) = mpsc::sync_channel(1);
        let (resume, resumed) = mpsc::sync_channel(1);
        assert!(
            self.confirmation_probe
                .lock()
                .unwrap()
                .replace(ConfirmationProbe {
                    ready,
                    resume: resumed
                })
                .is_none()
        );
        (entered, resume)
    }
    pub(super) fn probe_next_native_publication(
        &self,
    ) -> (mpsc::Receiver<()>, SyncSender<()>, mpsc::Receiver<()>) {
        let (ready, entered) = mpsc::sync_channel(1);
        let (resume, resumed) = mpsc::sync_channel(1);
        let (busy, observed_busy) = mpsc::sync_channel(1);
        let old = self
            .publication_probe
            .lock()
            .unwrap()
            .replace(PublicationProbe {
                ready,
                resume: resumed,
                busy,
            });
        assert!(old.is_none(), "one exact publication probe");
        (entered, resume, observed_busy)
    }
    pub(super) fn applied(&mut self, after: u64) -> (u64, ResidentPublicationMetadata) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            // Stop advancing once the callback has installed the successor. Scalar UI delivery
            // may lag independently; spinning extra audio blocks here can exhaust short media
            // before its literal PCM oracle, without indicating a renderer failure.
            if self.callback.lock().unwrap().is_some()
                && self
                    .audio
                    .as_ref()
                    .and_then(EditorAudio::status)
                    .is_none_or(|status| status.applied_revision.load(Acquire) <= after)
            {
                self.render_worker_step();
            }
            if let Some((revision, reason)) = self
                .audio
                .as_ref()
                .and_then(EditorAudio::status)
                .and_then(|status| status.rejected_revision())
                .filter(|(revision, _)| *revision > after)
            {
                panic!(
                    "native graph rejected after {after}: revision={revision}, reason={reason:?}, submitted={:?}",
                    self.submitted
                );
            }
            if let Some((revision, metadata)) =
                self.submitted.filter(|(revision, _)| *revision > after)
                && let Some(audio) = self.audio.as_ref()
                && audio.status().unwrap().applied_revision.load(Acquire) == revision
            {
                return (revision, metadata);
            }
            assert!(
                Instant::now() < end,
                "exact native ACK timed out: after={after}, submitted={:?}, actual={:?}",
                self.submitted,
                self.audio.as_ref().and_then(EditorAudio::status).map(|s| (
                    s.applied_revision.load(Acquire),
                    s.position.load(Acquire),
                    s.render_fault.load(Acquire)
                ))
            );
            std::thread::yield_now();
        }
    }
}

#[test]
fn default_audio_policy_admits_the_actual_stream_workspace_on_startup() {
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (revision, _) = h.applied(0);
    assert!(revision > 0);
}

#[test]
fn session_prepares_one_monitor_for_the_actual_paused_source_until_retired() {
    use hex_audio::{input_capture::MonitorStream, prepared::input_monitor::MonitorStatus};
    use hex_project::{Command, Created};
    use hex_session::service::{PausedInputMonitorRequest, ResidentGraphSubmission};

    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddTrack {
        name: "input".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    let (applied, _) = h.applied(initial);
    let request = |h: &ResidentHarness| {
        let (source, transport_epoch) = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .paused_input_source()
            .unwrap();
        PausedInputMonitorRequest {
            source: ResidentGraphSubmission {
                output_identity: source.output_identity,
                audio_revision: source.audio_revision,
                context: source.context,
            },
            track,
            transport_epoch,
        }
    };
    let prepare = |h: &mut ResidentHarness, request| {
        let (reply, result) = mpsc::sync_channel(1);
        let event = h.run(Request::PreparePausedInputMonitor { request, reply });
        assert!(event.error.is_none(), "{:?}", event.error);
        result.recv_timeout(Duration::from_millis(250)).unwrap()
    };
    let first = request(&h);
    let mut stale = first;
    stale.transport_epoch -= 1;
    assert_eq!(
        prepare(&mut h, stale).err().as_deref(),
        Some("paused input monitor transport changed")
    );
    let monitor = prepare(&mut h, first).unwrap();
    assert_eq!(
        prepare(&mut h, first).err().as_deref(),
        Some("paused input monitor is still retained")
    );

    let (input, feed, observer) = hex_audio::worker_fixture::monitored_input().unwrap();
    let stream = Box::new(MonitorStream::new_on_control(feed, 48_000).ok().unwrap());
    assert!(input.start_on_control());
    let deadline = Instant::now() + Duration::from_millis(250);
    while !observer.started() {
        assert!(Instant::now() < deadline, "synthetic capture did not start");
        std::thread::yield_now();
    }
    let source = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    assert!(
        h.audio
            .as_mut()
            .unwrap()
            .control
            .input_monitor()
            .unwrap()
            .publish_paused(stream, monitor, source, first.transport_epoch)
            .is_ok()
    );
    let mut audible = false;
    while !audible {
        audible = h
            .render()
            .iter()
            .any(|frame| frame[0] > 0.0 && frame[1] > 0.0);
        assert!(Instant::now() < deadline, "synthetic monitor did not prime");
        std::thread::yield_now();
    }
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire),
        0
    );
    assert!(
        h.run(Request::Edit(Command::RenameTrack {
            track,
            name: "edited".into()
        }))
        .error
        .is_none()
    );
    h.applied(applied);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let next = request(&h);
    assert_eq!(
        prepare(&mut h, first).err().as_deref(),
        Some("paused input monitor source changed")
    );
    assert_eq!(
        prepare(&mut h, next).err().as_deref(),
        Some("paused input monitor is still retained")
    );
    let control = h.audio.as_mut().unwrap().control.input_monitor().unwrap();
    assert_eq!(control.status(), MonitorStatus::SourceChanged);
    assert!(control.reclaim().is_some());
    assert!(prepare(&mut h, next).is_ok());

    let audio = h.audio.as_mut().unwrap();
    audio
        .control
        .command(hex_engine::playback::PlaybackCommand::Play)
        .unwrap();
    assert!(audio.status().unwrap().paused_input_source().is_none());
    h.render();
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    assert_eq!(
        prepare(&mut h, next).err().as_deref(),
        Some("paused input monitor transport changed")
    );
}

#[test]
fn refused_unpublished_gain_releases_the_fader_and_restart_retires_a_real_preview() {
    use hex_project::{Command, Created};
    use hex_session::device_gesture::{
        GestureKey, GesturePhase, GestureRequest, GestureTarget, ParameterValue,
    };
    use std::sync::atomic::Ordering::Release;

    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let before = h.run(Request::Edit(Command::AddTrack {
        name: "gain".into(),
    }));
    let Some(Created::Track(track)) = before.created else {
        panic!("track");
    };
    let (mut applied, _) = h.applied(applied);
    let key = |token| GestureKey {
        generation: 0,
        token,
    };
    let begin = |token| {
        Request::DeviceGesture(GestureRequest::Begin {
            key: key(token),
            target: GestureTarget::TrackGain { track },
        })
    };
    let change = |token| {
        Request::DeviceGesture(GestureRequest::Change {
            key: key(token),
            sequence: 1,
            value: ParameterValue::Float(0.5),
        })
    };
    let commit = |token, value| {
        Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: true,
            key: key(token),
            sequence: 2,
            value: ParameterValue::Float(value),
        })
    };

    assert!(h.run(begin(1)).gesture_ack.unwrap().accepted);
    h.refuse.store(true, Release);
    let refused = h.run(change(1));
    assert!(!refused.gesture_ack.unwrap().accepted);
    assert!(refused.device_preview.is_none());
    let refused = h.run(commit(1, 0.25));
    let ack = refused.gesture_ack.unwrap();
    assert_eq!((ack.key, ack.phase), (key(1), GesturePhase::Commit));
    assert!(!ack.accepted && !ack.preview_retained && ack.reason.is_some());
    assert!(refused.active_device_gesture.is_none());
    assert!(refused.device_preview.is_none());
    assert_eq!(refused.state.revision, before.state.revision);
    assert_eq!(refused.state.project, before.state.project);

    h.refuse.store(false, Release);
    assert!(h.run(begin(2)).gesture_ack.unwrap().accepted);
    assert!(h.run(change(2)).gesture_ack.unwrap().accepted);
    (applied, _) = h.applied(applied);
    let committed = h.run(commit(2, 0.25));
    assert!(committed.gesture_ack.unwrap().accepted);
    assert_eq!(committed.state.project.track(track).unwrap().gain(), 0.25);
    assert_eq!(committed.state.revision, before.state.revision + 1);
    (applied, _) = h.applied(applied);

    assert!(h.run(begin(3)).gesture_ack.unwrap().accepted);
    assert!(h.run(change(3)).gesture_ack.unwrap().accepted);
    let (_, metadata) = h.applied(applied);
    assert_eq!(metadata.device_preview.unwrap().key, key(3));
    h.refuse.store(true, Release);
    let retained = h.run(commit(3, 0.75));
    assert!(retained.gesture_ack.unwrap().preview_retained);
    assert!(retained.device_preview.is_some());
    h.refuse.store(false, Release);
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let restarted = h.run(Request::RestartAudio {
        project_generation: 0,
        request_id: 1,
    });
    let retired = restarted.gesture_ack.unwrap();
    assert_eq!((retired.key, retired.phase), (key(3), GesturePhase::Cancel));
    assert!(!retired.preview_retained);
    assert!(restarted.active_device_gesture.is_none());
    assert!(restarted.device_preview.is_none());
    assert_eq!(restarted.state.revision, committed.state.revision);
    assert_eq!(restarted.state.project, committed.state.project);
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    let (applied, metadata) = h.applied(0);
    assert_eq!(metadata.context.editor_revision, committed.state.revision);
    assert!(metadata.device_preview.is_none());

    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project, before.state.project);
    let (applied, _) = h.applied(applied);
    let redone = h.run(Request::Redo);
    assert_eq!(redone.state.project, committed.state.project);
    h.applied(applied);
}

#[test]
fn loaded_resident_preview_refused_commit_keeps_pcm_and_cancel_restores_canonical() {
    use hex_project::{Command, Created};
    use hex_session::device_gesture::ParameterValue;
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none(), "{:?}", initial.error);
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        slot: hex_session::service::SamplerLoadSlot::New(
            hex_project::InstrumentLoadPlacement::First,
        ),
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let device = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: loaded.state.revision,
        request_id: 2,
        track,
        input: Arc::clone(&input),
    });
    (applied, _) = h.applied(applied);
    input.set_key(0, Some((60, 127, 1))).unwrap();
    assert!(h.render().iter().flatten().any(|sample| *sample != 0.0));
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
        key,
        sequence: 1,
        value: ParameterValue::Float(0.5),
    }));
    let (preview_audio, metadata) = h.applied(applied);
    assert_eq!(metadata.device_preview.unwrap().sequence, 1);
    assert_eq!(preview.state.project, loaded.state.project);
    assert_eq!(
        preview
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .gain(),
        0.5
    );
    assert_eq!(
        preview
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );
    // The active preview approaches its target over 5 ms. Compare settled
    // blocks so this test isolates refusal rather than ramp progress.
    for _ in 0..4 {
        h.render();
    }
    let preview_pcm = h.render();
    assert!(preview_pcm.iter().flatten().any(|sample| *sample != 0.0));
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: true,
            key,
            sequence: 2,
            value: ParameterValue::Float(0.25),
        }))
        .unwrap();
    let refused = h.next();
    let ack = refused.gesture_ack.as_ref().unwrap();
    assert!(!ack.accepted && ack.preview_retained, "{ack:?}");
    assert_eq!(refused.active_device_gesture, preview.active_device_gesture);
    assert_eq!(refused.state.project, loaded.state.project);
    assert_eq!(refused.state.revision, loaded.state.revision);
    assert_eq!(refused.state.can_undo, loaded.state.can_undo);
    assert_eq!(
        refused.device_preview.as_ref().unwrap().identity,
        preview.device_preview.as_ref().unwrap().identity
    );
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        preview_audio
    );
    assert_eq!(
        h.render(),
        preview_pcm,
        "refused commit must preserve actual audible preview"
    );
    h.service
        .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
        .unwrap();
    let refused_cancel = h.next();
    assert!(refused_cancel.gesture_ack.unwrap().preview_retained);
    assert_eq!(
        refused_cancel.active_device_gesture,
        preview.active_device_gesture
    );
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    let (_, metadata) = h.applied(preview_audio);
    assert!(metadata.device_preview.is_none());
    assert!(cancelled.active_device_gesture.is_none());
    assert!(cancelled.device_preview.is_none());
    assert_eq!(cancelled.state.project, loaded.state.project);
    assert_eq!(cancelled.state.revision, loaded.state.revision);
    for _ in 0..4 {
        h.render();
    }
    assert_eq!(
        h.render(),
        preview_pcm.map(|frame| frame.map(|sample| sample * 2.0))
    );
    // Both an untouched Commit and a preview returned to its original value publish a new
    // audio revision in the SAME editor context, without fabricating canonical history.
    for (token, changed) in [(2, false), (3, true)] {
        let after = h.submitted.unwrap().0;
        let key = GestureKey {
            generation: 0,
            token,
        };
        h.run(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }));
        let mut last = after;
        if changed {
            h.run(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.25),
            }));
            (last, _) = h.applied(after);
        }
        let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: true,
            key,
            sequence: 2,
            value: ParameterValue::Float(1.0),
        }));
        let (_, metadata) = h.applied(last);
        assert_eq!(metadata.context.editor_revision, loaded.state.revision);
        assert!(metadata.device_preview.is_none());
        assert!(committed.active_device_gesture.is_none());
        assert!(committed.device_preview.is_none());
        assert_eq!(committed.state.project, loaded.state.project);
        assert_eq!(committed.state.revision, loaded.state.revision);
        assert_eq!(committed.state.can_undo, loaded.state.can_undo);
        assert_eq!(committed.state.can_redo, loaded.state.can_redo);
    }
}

impl ResidentHarness {
    pub(super) fn seek_intent(
        &self,
        request_id: u64,
        revision: u64,
        device_preview: Option<hex_session::service::DevicePreviewIdentity>,
        tuning_preview: Option<hex_session::service::TuningPreviewIdentity>,
    ) -> hex_session::service::ResidentArrangementSeekIntent {
        let control = &self.audio.as_ref().unwrap().control;
        let fence = control.capture_fresh_runtime_fence().unwrap();
        hex_session::service::ResidentArrangementSeekIntent {
            history: hex_session::service::ResidentSeekHistory::Fresh,
            request_id,
            project_generation: 0,
            project_revision: revision,
            destination_frame: 129,
            start_playing: true,
            section: None,
            locator_quantization: None,
            fence: hex_session::service::ResidentFreshRuntimeFence {
                output_identity: control.output_identity().unwrap(),
                active_audio_revision: fence.expected_active_revision(),
                route_generation: fence.route_state_generation(),
                admitted_command_ticket: fence.admitted_command_ticket(),
                output_fault_epoch: fence.output_fault_epoch(),
                consumed_stop_generation: fence.consumed_stop_generation(),
            },
            device_preview,
            tuning_preview,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(super) fn seek_result(&mut self) -> hex_session::service::ResidentArrangementSeekResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            assert!(
                self.service.try_arrangement_seek_result().is_none(),
                "native Playback must not escape through legacy result getter"
            );
            if let Some(result) = self.service.try_resident_arrangement_seek_result() {
                return result;
            }
            assert!(Instant::now() < deadline, "seek result timed out");
            std::thread::yield_now();
        }
    }
}

#[test]
fn resident_seek_refuses_fault_epoch_changed_after_capture() {
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none());
    h.applied(0);
    let intent = h.seek_intent(1, initial.state.revision, None, None);
    h.callback
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .report_render_error()
        .unwrap();
    // Isolate epoch validation at the actual Session/Desktop handoff even
    // after the fault flag changes; keep the same output and revision.
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .render_fault
        .store(false, std::sync::atomic::Ordering::Release);
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    let error = h.seek_result().result.unwrap_err();
    assert!(error.contains("runtime changed"), "{error}");
}

#[test]
fn loaded_resident_tuning_and_preview_seek_use_actual_sources_and_keep_refused_overlay() {
    use hex_project::{BeatTime, Command, Created};
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};
    use hex_session::tuning_gesture::{TuningGestureKey, TuningGestureRequest};
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        slot: hex_session::service::SamplerLoadSlot::New(
            hex_project::InstrumentLoadPlacement::First,
        ),
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    let canonical = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
        key,
        sequence: 1,
        value: ParameterValue::Float(0.5),
    }));
    let (device_audio, device_metadata) = h.applied(applied);
    applied = device_audio;
    assert_eq!(
        device_metadata.source_origin,
        hex_session::service::ResidentSourceOrigin::DevicePreview
    );
    let identity = preview.device_preview.as_ref().unwrap().identity;

    // An explicit input request during a live overlay must not publish the canonical gain.
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let keyboard = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: canonical.state.revision,
        request_id: 9,
        track,
        input: Arc::clone(&input),
    });
    let (keyboard_audio, metadata) = h.applied(applied);
    assert_eq!(metadata.device_preview, Some(identity));
    assert_eq!(
        metadata.source_origin,
        hex_session::service::ResidentSourceOrigin::DevicePreview
    );
    assert_eq!(metadata.computer_keyboard_request, Some(9));
    assert_eq!(keyboard.device_preview.as_ref().unwrap().identity, identity);
    // The preview was published while stopped; monitoring advances its ramp.
    for _ in 0..4 {
        h.render();
    }
    assert_eq!(
        h.render(),
        [[0.125; 2]; 64],
        "original PCM 8192/32768 × preview gain0.5"
    );
    // A refused replacement input must not cancel the installed client or advance its selection.
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    let rejected_input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    h.service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: canonical.state.revision,
            request_id: 11,
            track,
            input: Arc::clone(&rejected_input),
        })
        .unwrap();
    let refused_input = h.next();
    assert!(
        refused_input
            .error
            .as_ref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert!(rejected_input.is_cancelled());
    assert!(!input.is_cancelled());
    assert_eq!(
        refused_input.device_preview.as_ref().unwrap().identity,
        identity
    );
    assert_eq!(h.render(), [[0.125; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    input.set_key(0, None).unwrap();
    h.render();

    let intent = h.seek_intent(1, canonical.state.revision, Some(identity), None);
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    let seek = h.seek_result();
    let ResidentPublicationAcceptance::Submitted {
        output_identity,
        audio_revision,
    } = seek.result.unwrap()
    else {
        panic!("actual native submission")
    };
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(output_identity)
    );
    let (seek_audio, metadata) = h.applied(keyboard_audio);
    assert_eq!(seek_audio, audio_revision);
    assert_eq!(metadata.device_preview, Some(identity));
    assert_eq!(metadata.seek_request, Some(1));
    assert_eq!(
        h.render(),
        [[0.125; 2]; 64],
        "nonzero preroll must use the actual source-bearing overlay"
    );
    input.set_key(0, Some((60, 127, 2))).unwrap();
    assert_eq!(
        h.render(),
        [[0.25; 2]; 64],
        "seek attaches the same live monitor only after historical preroll"
    );
    input.set_key(0, None).unwrap();
    // Drain the released monitor voice's declick tail (the prepared ~2 ms
    // floor, 96 frames at 48 kHz) so the refusals below compare the settled
    // authored overlay alone.
    h.render();
    h.render();

    let mut stale = identity;
    stale.sequence += 1;
    h.service
        .submit_resident_arrangement_seek(h.seek_intent(
            2,
            canonical.state.revision,
            Some(stale),
            None,
        ))
        .unwrap();
    assert!(h.next().error.as_ref().unwrap().contains("preview changed"));
    assert!(
        h.seek_result()
            .result
            .unwrap_err()
            .contains("preview changed")
    );
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        seek_audio
    );
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit_resident_arrangement_seek(h.seek_intent(
            3,
            canonical.state.revision,
            Some(identity),
            None,
        ))
        .unwrap();
    assert!(h.next().error.is_none());
    assert!(
        h.seek_result()
            .result
            .unwrap_err()
            .contains("final admission refusal")
    );
    assert_eq!(h.render(), [[0.125; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    // Terminal input cancellation changes physical input words, not the accepted selection
    // version. The first seek must prune that still-captured binding on acceptance; the actual
    // Service post-drive mirror must let the next seek capture the same updated preview version.
    // Cancel before dispatch so this owning regression needs no worker scheduling race or delay.
    input.cancel();
    let mut last_seek_audio = seek_audio;
    for request_id in [4, 5] {
        h.service
            .submit_resident_arrangement_seek(h.seek_intent(
                request_id,
                canonical.state.revision,
                Some(identity),
                None,
            ))
            .unwrap();
        assert!(h.next().error.is_none());
        let result = h.seek_result();
        let ResidentPublicationAcceptance::Submitted { audio_revision, .. } =
            result.result.unwrap()
        else {
            panic!("native repeated preview seek")
        };
        let (actual, metadata) = h.applied(last_seek_audio);
        assert_eq!(actual, audio_revision);
        assert_eq!(metadata.device_preview, Some(identity));
        assert_eq!(metadata.seek_request, Some(request_id));
        assert_eq!(h.render(), [[0.125; 2]; 64]);
        last_seek_audio = actual;
    }
    h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    (applied, _) = h.applied(last_seek_audio);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    h.render();

    let key = TuningGestureKey {
        generation: 0,
        token: 1,
    };
    let tuned = super::sampler::tuning(880.0);
    h.run(Request::TuningGesture(TuningGestureRequest::Begin { key }));
    let preview = h.run(Request::TuningGesture(TuningGestureRequest::Change {
        key,
        sequence: 1,
        tuning: tuned,
    }));
    let (tuning_audio, tuning_metadata) = h.applied(applied);
    applied = tuning_audio;
    assert_eq!(
        tuning_metadata.source_origin,
        hex_session::service::ResidentSourceOrigin::TuningPreview
    );
    let identity = preview.tuning_preview.as_ref().unwrap().identity;
    assert_eq!(
        preview
            .tuning_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .tuning(),
        tuned
    );
    let next = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    next.set_key(0, Some((60, 127, 1))).unwrap();
    let prepared = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: canonical.state.revision,
        request_id: 10,
        track,
        input: Arc::clone(&next),
    });
    let (next_applied, metadata) = h.applied(applied);
    applied = next_applied;
    assert_eq!(metadata.tuning_preview, Some(identity));
    assert_eq!(
        metadata.source_origin,
        hex_session::service::ResidentSourceOrigin::TuningPreview
    );
    assert_eq!(prepared.tuning_preview.as_ref().unwrap().identity, identity);
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    // A preview returning exactly to canonical tuning still carries the
    // accepted family's live-note phase. Then change away again before Cancel.
    let canonical_preview = h.run(Request::TuningGesture(TuningGestureRequest::Change {
        key,
        sequence: 2,
        tuning: canonical.state.project.tuning(),
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(canonical_preview.state.project, canonical.state.project);
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    let preview = h.run(Request::TuningGesture(TuningGestureRequest::Change {
        key,
        sequence: 3,
        tuning: tuned,
    }));
    (applied, _) = h.applied(applied);
    let identity = preview.tuning_preview.as_ref().unwrap().identity;
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::TuningGesture(TuningGestureRequest::Commit {
            key,
            sequence: 4,
            tuning: super::sampler::tuning(660.0),
        }))
        .unwrap();
    let refused = h.next();
    assert!(
        refused
            .tuning_gesture_ack
            .as_ref()
            .unwrap()
            .preview_retained
    );
    assert_eq!(refused.active_tuning_gesture, preview.active_tuning_gesture);
    assert_eq!(refused.tuning_preview.as_ref().unwrap().identity, identity);
    assert_eq!(refused.state.project, canonical.state.project);
    assert_eq!(refused.state.revision, canonical.state.revision);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let cancelled = h.run(Request::TuningGesture(TuningGestureRequest::Cancel { key }));
    let (cancelled_audio, metadata) = h.applied(applied);
    applied = cancelled_audio;
    assert!(metadata.tuning_preview.is_none());
    assert!(cancelled.active_tuning_gesture.is_none());
    assert!(cancelled.tuning_preview.is_none());
    assert_eq!(cancelled.state.project, canonical.state.project);
    assert_eq!(cancelled.state.revision, canonical.state.revision);
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    // Canonical tuning edits and their global history use the same validated
    // family as previews; none may cancel the held physical input.
    let committed = h.run(Request::Edit(Command::SetTuning(tuned)));
    (applied, _) = h.applied(applied);
    assert_eq!(committed.state.project.tuning(), tuned);
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    for (request, expected) in [
        (Request::Undo, canonical.state.project.tuning()),
        (Request::Redo, tuned),
        (Request::Undo, canonical.state.project.tuning()),
    ] {
        let changed = h.run(request);
        (applied, _) = h.applied(applied);
        assert_eq!(changed.state.project.tuning(), expected);
        assert!(!next.is_cancelled());
        assert_eq!(h.render(), [[0.25; 2]; 64]);
    }
    assert!(loaded.playback.is_none());
}

#[test]
fn midi_recording_wire_and_physical_input_deliver_then_commit_one_native_history_edit() {
    use hex_audio::midi_recording::{Receipt, Target};
    use hex_engine::midi_capture::MidiCaptureBinding;
    use hex_project::{BeatTime, Command};
    use hex_session::midi_recording::{MidiTake, MidiTakeBinding};
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::InsertTrackWithDevice {
        name: "record".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().unwrap(),
    }));
    let track = added.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4).unwrap(),
    }));
    let clip = placed.state.project.track(track).unwrap().midi_clips()[0].id();
    (applied, _) = h.applied(applied);
    let binding = Arc::new(MidiTakeBinding::capture(&placed.state.project, track, clip).unwrap());
    let input = Arc::new(
        h.audio
            .as_ref()
            .unwrap()
            .control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_live_notes()
            .unwrap()
            .with_recording_support()
            .unwrap(),
    );
    h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: placed.state.revision,
        request_id: 51,
        track,
        input: Arc::clone(&input),
    });
    (applied, _) = h.applied(applied);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
            .audio_revision,
        applied
    );
    let hardware = input.begin_live_notes_activation_on_control().unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    // Existing render is deliberately not host-delivery evidence. Confirmed
    // capture below uses the native conversion helper into an explicit fake sink.
    h.render();
    let keys = input.begin_ordered_keys_on_control().unwrap();
    let source = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    let target = Target {
        capture: MidiCaptureBinding {
            take_id: 1,
            track,
            device: binding.instrument(),
            sample_rate: 48_000,
            output_epoch: input.output_epoch_on_control().unwrap(),
            keys,
            hardware: Some(hardware),
            first_frame: binding.first_frame(),
            end_frame: binding.end_frame(),
        },
        project_generation: 0,
        canonical_revision: placed.state.revision,
        audio_revision: source.audio_revision,
        output_identity: source.output_identity,
        transport_epoch: h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .transport_epoch
            .load(Acquire),
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .midi_begin(target)
        .unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    input.set_key(0, None).unwrap();
    let held = hex_midi::HeldNoteInput::default();
    hex_midi::deliver_packets_with_notes(
        &mut hex_midi::Decoder::default(),
        2,
        Default::default(),
        [&[0x92, 64, 37, 64, 0, 67, 70][..]],
        &held,
        &|_| {},
        &|event| {
            use hex_engine::computer_keyboard::LiveNoteEvent;
            let event = match event {
                hex_midi::NoteEvent::On {
                    channel,
                    pitch,
                    velocity,
                } => LiveNoteEvent::NoteOn {
                    channel,
                    pitch,
                    velocity,
                },
                hex_midi::NoteEvent::Off {
                    channel,
                    pitch,
                    velocity,
                } => LiveNoteEvent::NoteOff {
                    channel,
                    pitch,
                    velocity,
                },
                _ => panic!("valid fixture message"),
            };
            input
                .live_notes()
                .unwrap()
                .publish(hardware, event)
                .unwrap();
        },
    );
    assert!(h.audio.as_mut().unwrap().control.midi_receipt().is_none());
    let mut frames = [[0.0; 2]; 64];
    let mut sink = [0.0_f32; 128];
    {
        let mut backend = h.callback.lock().unwrap();
        let allocations = allocation_counter::measure(|| {
            backend
                .as_mut()
                .unwrap()
                .render_direct(&mut frames, &mut sink)
                .unwrap()
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    assert_eq!(frames.as_flattened(), &sink);
    assert!(sink.iter().any(|value| *value != 0.0));
    let Receipt::Armed { clock, .. } = h.audio.as_mut().unwrap().control.midi_receipt().unwrap()
    else {
        panic!("armed before notes")
    };
    let mut take = MidiTake::new(binding.first_frame(), binding.end_frame(), clock).unwrap();
    while let Some(receipt) = h.audio.as_mut().unwrap().control.midi_receipt() {
        let Receipt::Note {
            note,
            content_frame,
            ..
        } = receipt
        else {
            panic!("note prefix")
        };
        take.push(note, content_frame).unwrap();
    }
    h.audio.as_ref().unwrap().control.stop();
    h.callback
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .render_direct(&mut frames, &mut sink)
        .unwrap();
    let Receipt::Finished {
        clock: Some(stop), ..
    } = h.audio.as_mut().unwrap().control.midi_receipt().unwrap()
    else {
        panic!("finish from delivered boundary")
    };
    assert_eq!(stop.content_frame, clock.content_frame + 64);
    take.finish(stop).unwrap();
    let mut captured = take
        .notes()
        .iter()
        .map(|n| (n.pitch, n.channel, n.velocity))
        .collect::<Vec<_>>();
    captured.sort_unstable();
    assert_eq!(captured, [(60, 0, 100), (64, 2, 37), (67, 2, 70)]);
    assert_eq!(take.notes().last().unwrap().end_frame, stop.content_frame);
    let _ = input.seal_ordered_keys_on_control(keys);
    let notes: Arc<[_]> = Arc::from(take.notes());
    // Refuse the actual final resource admission. Canonical notes/history stay
    // unchanged and the immutable delivered take remains available for retry.
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::CommitMidiTake {
            request_id: 61,
            project_generation: 0,
            revision: placed.state.revision,
            binding: Arc::clone(&binding),
            notes: Arc::clone(&notes),
            stop_frame: stop.content_frame,
        })
        .unwrap();
    let refused = h.next();
    assert!(!refused.midi_take_complete.unwrap().accepted);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, placed.state.project);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let committed = h.run(Request::CommitMidiTake {
        request_id: 62,
        project_generation: 0,
        revision: placed.state.revision,
        binding,
        notes: Arc::clone(&notes),
        stop_frame: stop.content_frame,
    });
    assert!(committed.midi_take_complete.unwrap().accepted);
    (applied, _) = h.applied(applied);
    let recorded = committed.state.project.midi_clip(clip).unwrap().notes();
    assert_eq!(recorded.len(), 3);
    for (actual, expected) in recorded.iter().zip(notes.iter()) {
        assert_eq!(
            (actual.pitch(), actual.channel(), actual.velocity()),
            (expected.pitch, expected.channel, expected.velocity)
        );
        let actual_frame = committed
            .state
            .project
            .tempo_map()
            .beat_to_frame(actual.start())
            .unwrap();
        assert!(actual_frame.abs_diff(expected.start_frame) <= 1);
        let end = BeatTime::from_raw(
            actual
                .start()
                .raw()
                .checked_add(actual.duration().raw())
                .unwrap(),
        );
        let actual_end = committed
            .state
            .project
            .tempo_map()
            .beat_to_frame(end)
            .unwrap();
        assert!(actual_end.abs_diff(expected.end_frame) <= 1);
    }
    let encoded = hex_storage::encode(&committed.state.project).unwrap();
    assert_eq!(
        hex_storage::decode(&encoded).unwrap().snapshot(),
        committed.state.project.snapshot()
    );
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert!(
        undone
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .is_empty()
    );
    let redone = h.run(Request::Redo);
    h.applied(applied);
    assert_eq!(
        redone.state.project.snapshot(),
        committed.state.project.snapshot()
    );
    assert_eq!(notes.len(), 3);
}

#[test]
fn sealed_native_take_recovery_uses_canonical_publication_and_one_undo() {
    use hex_session::service::{
        AutomationRecoveryFileIdentity, DeviceAutomationTakeProgress, DeviceAutomationTakeScope,
        DeviceCaptureTarget,
    };
    use hex_storage::automation_draft::{
        AppliedEvent, CaptureClock, DraftClock, DraftProvenance, DraftTarget, DraftTerminal,
        DraftWriter,
    };
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(8192);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let inserted = h.run(Request::Edit(hex_project::Command::InsertMainDevice {
        before: None,
        state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.utility",
        ))
        .unwrap(),
    }));
    applied = h.applied(applied).0;
    let device = inserted.state.project.main_devices()[0].id();
    let target = hex_engine::native_parameter_input_target(
        &inserted.state.project,
        device,
        hex_project::DeviceAutomationTarget::HostOutputGain,
    )
    .unwrap();
    let context = h.submitted.unwrap().1.context;
    let scope = DeviceAutomationTakeScope {
        target: DeviceCaptureTarget::Native(target),
        stamp: DraftProvenance {
            sample_rate: 48_000,
            output_identity: 1,
            project_generation: context.project_generation,
            editor_revision: context.editor_revision,
            audio_revision: 1,
            transport_epoch: 1,
            gesture_serial: 1,
        },
    };
    // Canonical sealed-file fixture; capture receipt provenance is tested by the
    // recording owner. This test exercises the real native publication consumer.
    let path = files.0.join("native-sealed.hex-automation-draft");
    let mut writer = DraftWriter::create_with_clock(
        &path,
        &inserted.state.project,
        DraftTarget::Device {
            device,
            target: target.target(),
        },
        scope.stamp,
        CaptureClock {
            sample_rate: 48_000,
            loop_region: None,
        },
    )
    .unwrap();
    writer
        .append(&[AppliedEvent {
            sequence: 1,
            performance_frame: 12,
            content_frame: 4,
            loop_pass: 0,
            value: hex_project::AutomationValue::new(0.75).unwrap(),
        }])
        .unwrap();
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: 24,
            content_frame: 16,
            loop_pass: 0,
        }))
        .unwrap();
    drop(writer);
    let bytes = std::fs::read(&path).unwrap();
    let inspected = h.run(Request::InspectSealedAutomationRecovery {
        request_id: 41,
        path: path.clone(),
    });
    assert!(inspected.error.is_none(), "{:?}", inspected.error);
    assert_eq!(inspected.state.project, inserted.state.project);
    assert_eq!(inspected.state.revision, inserted.state.revision);
    let DeviceAutomationTakeProgress::SealedFile(selected) =
        inspected.device_automation_take_complete.unwrap().progress
    else {
        panic!("finished recovery file did not produce an exact selection");
    };
    let recovered = h.run(Request::RecoverDeviceAutomationTake {
        request_id: 42,
        target: selected.target,
        expected: selected.provenance,
        path: selected.path,
        context,
        identity: AutomationRecoveryFileIdentity::Sealed(selected.identity),
    });
    assert_eq!(
        recovered.device_automation_take_complete.unwrap().progress,
        DeviceAutomationTakeProgress::Committed
    );
    assert_ne!(recovered.state.project, inserted.state.project);
    h.applied(applied);
    #[cfg(all(
        feature = "allocation-probe",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    {
        let mut output = [[0.0; 2]; 64];
        let mut host = [0.0; 128];
        let mut slot = h.callback.lock().unwrap();
        let callback = slot.as_mut().unwrap();
        let measured = allocation_counter::measure(|| {
            callback
                .render_native_direct(&mut output, &mut host)
                .unwrap()
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project, inserted.state.project);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
