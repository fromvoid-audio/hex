//! Ordinary authoring after loading an original sampler source uses the exact same native
//! publication/output and command history as resource-free editing.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{BeatTime, Command, Created, TrimEdge};
use hex_session::service::ResidentPublicationAcceptance;

fn pcm_at_cursor(
    h: &mut ResidentHarness,
    state: &Event,
    applied: &mut u64,
    request_id: &mut u64,
    value: f32,
) {
    scalar_pcm_at_cursor(h, state, applied, request_id, value, 0);
}

fn scalar_pcm_at_cursor(
    h: &mut ResidentHarness,
    state: &Event,
    applied: &mut u64,
    request_id: &mut u64,
    value: f32,
    attack_frames: u64,
) {
    *request_id += 1;
    let intent = h.seek_intent(*request_id, state.state.revision, None, None);
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    let ResidentPublicationAcceptance::Submitted { audio_revision, .. } =
        h.seek_result().result.unwrap()
    else {
        panic!("native seek")
    };
    let (actual, metadata) = h.applied(*applied);
    assert_eq!(actual, audio_revision);
    assert_eq!(
        metadata.context,
        hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: state.state.revision,
        }
    );
    let position = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    assert!(
        position < 1024,
        "fixture advanced past its short audio clip while awaiting scalar observation: {position}"
    );
    for (offset, frame) in h.render().into_iter().enumerate() {
        let envelope = if attack_frames == 0 {
            1.0
        } else {
            ((position + offset as u64) as f64 / attack_frames as f64).min(1.0)
        };
        let expected = (f64::from(value) * envelope) as f32;
        assert_eq!(
            frame, [expected; 2],
            "literal source/gain/attack oracle at {position}+{offset}"
        );
    }
    *applied = actual;
}

#[test]
fn loaded_resident_normal_authoring_history_and_noops_keep_exact_native_context() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let sampler_file = Files::new();
    let audio_file = Files::new();
    // This fixture reserves below the production8M frame policy, with room for actual distinct
    // original and clip PCM. It does not raise the device/source extent or production caps.
    let mut h = ResidentHarness::with_frame_limit(32_768);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let mut request_id = 100;
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
        empty_sampler: None,
        path: sampler_file.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let sampler = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .clone();
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(midi_clip)) = placed.created else {
        panic!("midi clip")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip: midi_clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let future_beat = placed.state.project.tempo_map().frame_to_beat(768).unwrap();
    let future_frame = placed
        .state
        .project
        .tempo_map()
        .beat_to_frame(future_beat)
        .unwrap();
    h.run(Request::Edit(Command::AddMidiNote {
        clip: midi_clip,
        start: future_beat,
        duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let resized_midi = h.run(Request::Edit(Command::TrimMidiClipEdge {
        clip: midi_clip,
        edge: TrimEdge::Right,
        beat: BeatTime::from_raw(BeatTime::ONE.raw() * 2),
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(
        resized_midi
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()[0]
            .length(),
        BeatTime::from_raw(BeatTime::ONE.raw() * 2)
    );
    let audio_track = h.run(Request::Edit(Command::AddTrack {
        name: "audio".into(),
    }));
    let Some(Created::Track(audio_track_id)) = audio_track.created else {
        panic!("audio track")
    };
    (applied, _) = h.applied(applied);
    let audio_path = audio_file.wav(48_000, 2048);
    let imported = h.run(Request::ImportToTrack {
        path: audio_path.clone(),
        track: audio_track_id,
        start: 0,
    });
    (applied, _) = h.applied(applied);
    let audio_clip = imported
        .state
        .project
        .track(audio_track_id)
        .unwrap()
        .clips()[0]
        .id();
    let resized = h.run(Request::Edit(Command::TrimClipEdge {
        clip: audio_clip,
        edge: TrimEdge::Right,
        frame: 1024,
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(
        resized.state.project.track(audio_track_id).unwrap().clips()[0].duration(),
        1024
    );
    pcm_at_cursor(&mut h, &resized, &mut applied, &mut request_id, 0.5);

    // Both edge edits happen after a genuine nonzero preroll. Extending the MIDI clip changes
    // its descriptor but preserves the active occurrence; existing per-occurrence history proof
    // must still carry it. Audio edge changes retain the independent clip source.
    let live_midi = h.run(Request::Edit(Command::TrimMidiClipEdge {
        clip: midi_clip,
        edge: TrimEdge::Right,
        beat: BeatTime::from_raw(BeatTime::ONE.raw() * 3),
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(h.render(), [[0.5; 2]; 64]);
    let live_audio = h.run(Request::Edit(Command::TrimClipEdge {
        clip: audio_clip,
        edge: TrimEdge::Right,
        frame: 1536,
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(
        live_audio.state.project.track(track).unwrap().midi_clips()[0].length(),
        BeatTime::from_raw(BeatTime::ONE.raw() * 3)
    );
    assert_eq!(h.render(), [[0.5; 2]; 64]);
    let resized = h.run(Request::Edit(Command::TrimClipEdge {
        clip: audio_clip,
        edge: TrimEdge::Right,
        frame: 1024,
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(
        live_midi.state.project.track(track).unwrap().midi_clips()[0].length(),
        BeatTime::from_raw(BeatTime::ONE.raw() * 3)
    );
    let saved = h.run(Request::SaveNew(audio_file.0.join("normal.hex")));
    assert!(saved.save_complete && !saved.state.unsaved);
    // No-op controls preserve authored revision, dirty state and both history directions while
    // their actual new audio revisions still correlate exactly with that unchanged context.
    for command in [
        Command::Mute {
            track,
            muted: false,
        },
        Command::TrimClipEdge {
            clip: audio_clip,
            edge: TrimEdge::Right,
            frame: 1024,
        },
    ] {
        let no_op = h.run(Request::Edit(command));
        let (next, metadata) = h.applied(applied);
        assert_eq!(metadata.context.editor_revision, saved.state.revision);
        assert_eq!(no_op.state.project, saved.state.project);
        assert_eq!(no_op.state.revision, saved.state.revision);
        assert_eq!(no_op.state.can_undo, saved.state.can_undo);
        assert_eq!(no_op.state.can_redo, saved.state.can_redo);
        assert!(!no_op.state.unsaved);
        applied = next;
    }
    let undone_resize = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(
        undone_resize
            .state
            .project
            .track(audio_track_id)
            .unwrap()
            .clips()[0]
            .duration(),
        1536
    );
    let redone_resize = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    assert_eq!(redone_resize.state.project, resized.state.project);

    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let keyboard = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: redone_resize.state.revision,
        request_id: 8,
        track,
        input: Arc::clone(&input),
    });
    (applied, _) = h.applied(applied);
    pcm_at_cursor(&mut h, &keyboard, &mut applied, &mut request_id, 0.75);
    let muted = h.run(Request::Edit(Command::Mute { track, muted: true }));
    (applied, _) = h.applied(applied);
    pcm_at_cursor(&mut h, &muted, &mut applied, &mut request_id, 0.25);
    let unmuted = h.run(Request::Edit(Command::Mute {
        track,
        muted: false,
    }));
    (applied, _) = h.applied(applied);
    assert!(!input.is_cancelled());
    pcm_at_cursor(&mut h, &unmuted, &mut applied, &mut request_id, 0.75);
    let bypassed = h.run(Request::Edit(Command::SetDeviceBypass {
        device: sampler.id(),
        bypassed: true,
    }));
    (applied, _) = h.applied(applied);
    assert!(
        input.is_cancelled(),
        "accepted invalid monitor binding retires its client"
    );
    pcm_at_cursor(&mut h, &bypassed, &mut applied, &mut request_id, 0.25);
    let enabled = h.run(Request::Edit(Command::SetDeviceBypass {
        device: sampler.id(),
        bypassed: false,
    }));
    (applied, _) = h.applied(applied);
    let mut heard_future = false;
    for _ in 0..8 {
        let position = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        let pcm = h.render();
        for (offset, frame) in pcm.into_iter().enumerate() {
            let is_future = position + offset as u64 >= future_frame;
            assert_eq!(
                frame,
                [if is_future { 0.5 } else { 0.25 }; 2],
                "enabled device must suppress pre-cut gates and admit future onset at {future_frame}; cursor={position}, offset={offset}"
            );
            heard_future |= is_future;
        }
        if heard_future {
            break;
        }
    }
    assert!(heard_future);
    // An explicit seek reconstructs the earlier held gate under the normal preroll law.
    pcm_at_cursor(&mut h, &enabled, &mut applied, &mut request_id, 0.5);
    let removed = h.run(Request::Edit(Command::RemoveInstrument { track }));
    (applied, _) = h.applied(applied);
    pcm_at_cursor(&mut h, &removed, &mut applied, &mut request_id, 0.25);
    let restored = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(
        restored.state.project.track(track).unwrap().instrument(),
        Some(&sampler)
    );
    pcm_at_cursor(&mut h, &restored, &mut applied, &mut request_id, 0.5);
    let again_removed = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    pcm_at_cursor(&mut h, &again_removed, &mut applied, &mut request_id, 0.25);
    h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    let vector = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_session::device_gesture::DeviceTypeId::from_static("hex.instrument.fm4"),
    )
    .unwrap();
    let replaced = h.run(Request::Edit(Command::ReplaceDeviceState {
        device: sampler.id(),
        state: vector.clone(),
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(
        replaced
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &vector
    );
    let restored = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(
        restored.state.project.track(track).unwrap().instrument(),
        Some(&sampler)
    );
    pcm_at_cursor(&mut h, &restored, &mut applied, &mut request_id, 0.5);

    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service.submit(Request::Import(audio_path)).unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert_eq!(refused.state.project, restored.state.project);
    assert_eq!(refused.state.revision, restored.state.revision);
    assert_eq!(refused.state.can_redo, restored.state.can_redo);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    pcm_at_cursor(&mut h, &refused, &mut applied, &mut request_id, 0.5);
}

#[test]
fn loaded_resident_scalar_edits_reach_pcm_history_and_current_save_reopen() {
    use hex_engine::SamplerParameter::{AttackMs, Gain};
    use hex_session::service::{SamplerEdit, SamplerParameterEdit};
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(32_768);
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
        empty_sampler: None,
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let sampler = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .clone();
    let device = sampler.id();
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    let notes = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let mut request_id = 100;
    pcm_at_cursor(&mut h, &notes, &mut applied, &mut request_id, 0.25);
    let make_edit = |event: &Event, parameter, value| SamplerParameterEdit {
        request_id: 2,
        project_generation: 0,
        revision: event.state.revision,
        track,
        device,
        parameter,
        value,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let gain = h.run(Request::SamplerEdit(SamplerEdit::Parameter(make_edit(
        &notes, Gain, 0.5,
    ))));
    assert_eq!(gain.sampler_edit_complete, Some(2));
    (applied, _) = h.applied(applied);
    // Literal PCM is tested before inspecting state, so bypassing the provider patch must
    // fail at the actual native consumer, even if a facade-only comparison would pass.
    pcm_at_cursor(&mut h, &gain, &mut applied, &mut request_id, 0.125);
    let mut expected = sampler.state().payload().to_vec();
    expected[40..48].copy_from_slice(&0.5_f64.to_le_bytes());
    assert_eq!(
        gain.state.project.device(device).unwrap().state().payload(),
        expected
    );
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(undone.state.project, notes.state.project);
    pcm_at_cursor(&mut h, &undone, &mut applied, &mut request_id, 0.25);
    let redone = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    assert_eq!(redone.state.project, gain.state.project);
    pcm_at_cursor(&mut h, &redone, &mut applied, &mut request_id, 0.125);

    let attack = h.run(Request::SamplerEdit(SamplerEdit::Parameter(make_edit(
        &redone, AttackMs, 125.0,
    ))));
    (applied, _) = h.applied(applied);
    scalar_pcm_at_cursor(&mut h, &attack, &mut applied, &mut request_id, 0.125, 6000);
    expected[64..72].copy_from_slice(&0.125_f64.to_le_bytes());
    let state = attack.state.project.device(device).unwrap().state();
    assert_eq!(state.payload(), expected);
    assert_eq!(state.type_id(), sampler.state().type_id());
    assert_eq!(state.state_version(), 2);
    assert_eq!(state.asset_references(), sampler.state().asset_references());
    assert_eq!(
        state.input_gain_millidb(),
        sampler.state().input_gain_millidb()
    );
    assert_eq!(
        state.output_gain_millidb(),
        sampler.state().output_gain_millidb()
    );
    assert_eq!(
        attack.state.project.assets().collect::<Vec<_>>(),
        notes.state.project.assets().collect::<Vec<_>>()
    );
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(undone.state.project, gain.state.project);
    pcm_at_cursor(&mut h, &undone, &mut applied, &mut request_id, 0.125);
    let redone = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    assert_eq!(redone.state.project, attack.state.project);

    for case in 0..4 {
        let mut edit = make_edit(&redone, Gain, 0.75);
        match case {
            0 => edit.revision -= 1,
            1 => edit
                .cancel
                .store(true, std::sync::atomic::Ordering::Release),
            2 => edit.value = f64::NAN,
            _ => h.refuse.store(true, std::sync::atomic::Ordering::Release),
        }
        h.service
            .submit(Request::SamplerEdit(SamplerEdit::Parameter(edit)))
            .unwrap();
        let refused = h.next();
        assert!(refused.error.is_some(), "case {case}");
        assert_eq!(refused.state.project, redone.state.project);
        assert_eq!(refused.state.revision, redone.state.revision);
        assert_eq!(refused.state.can_undo, redone.state.can_undo);
        assert_eq!(refused.state.can_redo, redone.state.can_redo);
        h.refuse.store(false, std::sync::atomic::Ordering::Release);
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
        scalar_pcm_at_cursor(&mut h, &refused, &mut applied, &mut request_id, 0.125, 6000);
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Stop)
        .unwrap();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    scalar_pcm_at_cursor(&mut h, &redone, &mut applied, &mut request_id, 0.125, 6000);

    let path = files.0.join("scalar.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    assert!(saved.save_complete && !saved.state.unsaved);
    let persisted = hex_storage::load(&path).unwrap();
    assert_eq!(persisted.tracks(), saved.state.project.tracks());
    let original_asset = saved.state.project.assets().next().unwrap();
    let persisted_asset = persisted.assets().next().unwrap();
    assert_eq!(persisted_asset.id(), original_asset.id());
    assert_eq!(persisted_asset.path(), original_asset.path());
    assert_eq!(persisted_asset.source_sample_rate(), 48_000);
    assert_eq!(persisted_asset.source_frames(), 8192);
    assert_eq!(
        persisted_asset.playback_frames(),
        original_asset.playback_frames()
    );
    // Saving enriches the copy with verified original-source facts; the live snapshot
    // intentionally has no such digest yet (the existing save contract).
    assert!(persisted_asset.original_source().is_some());
    let opened = h.run(Request::Open(path));
    assert!(opened.project_replaced);
    assert_eq!(*opened.state.project, persisted);
    // New/Open replaces output custody; existing lifecycle tests cover that handoff. This
    // assertion pins the complete current-format state through the real Session reopen.
    assert_eq!(opened.state.project.device(device).unwrap().state(), state);
}
