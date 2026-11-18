//! Ordinary authoring after loading an original sampler source uses the exact same native
//! publication/output and command history as resource-free editing.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{BeatTime, Command, Created, TrimEdge};
use hex_session::service::ResidentPublicationAcceptance;

#[test]
fn active_source_provider_change_applies_while_playing() {
    let _native = native_fixture();
    let file = Files::new();
    let mut h = ResidentHarness::with_frame_limit(32_768);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track");
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
        path: file.wav(48_000, 8192),
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
        length: BeatTime::from_quarters(4).unwrap(),
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip");
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4).unwrap(),
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Play)
        .unwrap();
    assert!(h.render().iter().flatten().any(|sample| *sample != 0.0));
    let vector = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_session::device_gesture::DeviceTypeId::from_static("hex.instrument.fm4"),
    )
    .unwrap();
    let replaced = h.run(Request::Edit(Command::ReplaceDeviceState {
        device: sampler.id(),
        state: vector,
    }));
    // Replacing the sounding sampler's source provider while playing used to
    // refuse with SourceHistoryUnavailable until Stop. It now restarts audio
    // at the cursor (Pulse #3784): the edit applies and playback continues.
    (applied, _) = h.applied(applied);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(status.rejected_revision().is_none());
    assert_ne!(
        replaced.state.project.track(track).unwrap().instrument(),
        Some(&sampler)
    );
    assert!(
        (0..8).any(|_| h.render().iter().flatten().any(|sample| *sample != 0.0)),
        "the replacement instrument sounds after the restart"
    );
    let undone = h.run(Request::Undo);
    h.applied(applied);
    assert_eq!(
        undone.state.project.track(track).unwrap().instrument(),
        Some(&sampler)
    );
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    assert!(
        (0..8).any(|_| h.render().iter().flatten().any(|sample| *sample != 0.0)),
        "the restored sampler sounds after Undo"
    );
}

fn global_bpm_preserves_pcm_at_rate(rate: u32) {
    let _native = native_fixture();
    let file = Files::new();
    // Preserve the original four-to-one budget and edit offsets with a shorter source.
    let mut h = ResidentHarness::with_frame_limit_at_rate(65_536, rate);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(file.wav(rate, 16_384)));
    (applied, _) = h.applied(applied);
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    h.run(Request::Edit(Command::EditAudioEvents {
        clip,
        operation: hex_project::AudioEventEdit::Split { at: 2_048 },
    }));
    (applied, _) = h.applied(applied);
    let edited = h.run(Request::Edit(Command::EditAudioEvents {
        clip,
        operation: hex_project::AudioEventEdit::Move {
            event: 1,
            start: 6_144,
        },
    }));
    (applied, _) = h.applied(applied);
    let settings = hex_project::default_audio_warp(
        edited.state.project.clip(clip).unwrap(),
        edited.state.project.tempo_map(),
        hex_project::WarpMode::RePitch,
    )
    .unwrap();
    let warped = h.run(Request::Edit(Command::SetAudioClipWarp {
        clip,
        settings: Some(settings),
    }));
    (applied, _) = h.applied(applied);
    let before = h.run(Request::Edit(Command::SetProjectTempo {
        tempo: hex_project::Tempo::new(180_000_000).unwrap(),
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(
        before
            .state
            .project
            .tempo_map()
            .tempo_at(BeatTime::ZERO)
            .micro_bpm(),
        180_000_000
    );
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(undone.state.project, warped.state.project);
    let redone = h.run(Request::Redo);
    h.applied(applied);
    assert_eq!(redone.state.project, before.state.project);
    assert!(h.publication_refusals.is_empty());
}

#[test]
fn global_bpm_preserves_pcm_at_44100() {
    global_bpm_preserves_pcm_at_rate(44_100);
}

#[test]
fn global_bpm_preserves_pcm_at_48000() {
    global_bpm_preserves_pcm_at_rate(48_000);
}

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
    let destination = intent.destination_frame;
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
        // Seeks start voices empty: the held sampler note retriggers at the
        // destination, so its attack restarts there.
        let envelope = if attack_frames == 0 {
            1.0
        } else {
            ((position - destination + offset as u64) as f64 / attack_frames as f64).min(1.0)
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
    let _native = native_fixture();
    let sampler_file = Files::new();
    let audio_file = Files::new();
    // This fixture reserves below the production aggregate frame policy, with room for actual distinct
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
        slot: hex_session::service::SamplerLoadSlot::New(
            hex_project::InstrumentLoadPlacement::First,
        ),
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
    let original_note = h.run(Request::Edit(Command::AddMidiNote {
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
    let overwritten = h.run(Request::Edit(Command::AddMidiNote {
        clip: midi_clip,
        start: future_beat,
        duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    // A real resident source must accept the collateral prefix edit through the
    // same closed authoring receipt, publication, and global history path.
    let Some(Created::Note(original_id)) = original_note.created else {
        panic!("original note")
    };
    assert_eq!(
        overwritten
            .state
            .project
            .midi_note(original_id)
            .unwrap()
            .duration(),
        future_beat
    );
    let undo_overwrite = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(
        undo_overwrite.state.project.midi_source(midi_clip),
        original_note.state.project.midi_source(midi_clip)
    );
    let redo_overwrite = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    assert_eq!(redo_overwrite.state.project, overwritten.state.project);
    // UI authoring past the previous source extent must use ordinary resident
    // admission and one global Undo edge after a real sampler has loaded.
    let extended = h.run(Request::Edit(Command::EditMidiNotesWithSourceExtent {
        clip: midi_clip,
        remove: Vec::new(),
        insert: vec![hex_project::MidiNoteInsert {
            active: true,
            start: BeatTime::ONE,
            duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
            pitch: 64,
            channel: 0,
            velocity: 100,
        }],
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(
        extended
            .state
            .project
            .midi_source(midi_clip)
            .unwrap()
            .source_length()
            .raw(),
        BeatTime::ONE.raw() * 5 / 4
    );
    assert_eq!(
        extended
            .state
            .project
            .midi_clip(midi_clip)
            .unwrap()
            .arrangement_length(),
        BeatTime::ONE
    );
    let undone_extent = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(
        undone_extent
            .state
            .project
            .midi_source(midi_clip)
            .unwrap()
            .source_length(),
        BeatTime::ONE
    );
    let redone_extent = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    assert_eq!(
        redone_extent.state.project.midi_source(midi_clip),
        extended.state.project.midi_source(midi_clip)
    );
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
    // A provider change under the same retained source/device identity has no
    // DSP-history transfer proof. Acknowledge Stop before replacement and Undo;
    // the explicit seek below starts the restored source again.
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Stop)
        .unwrap();
    // The queued Stop settles after its declick fade.
    for _ in 0..super::FADE_BLOCKS {
        h.render();
    }
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
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

/// Loaded sampler on a MIDI track with one full-velocity note, all applied.
struct SamplerNote {
    h: ResidentHarness,
    _files: Files,
    notes: Event,
    track: hex_project::TrackId,
    sampler: hex_project::Device,
    applied: u64,
}

fn sampler_note_fixture() -> SamplerNote {
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
        slot: hex_session::service::SamplerLoadSlot::New(
            hex_project::InstrumentLoadPlacement::First,
        ),
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
    SamplerNote {
        h,
        _files: files,
        notes,
        track,
        sampler,
        applied,
    }
}

#[test]
fn loaded_resident_scalar_edits_reach_pcm_history_and_current_save_reopen() {
    use hex_engine::SamplerParameter::{AttackMs, Gain};
    use hex_session::service::{SamplerEdit, SamplerParameterEdit};
    let _native = native_fixture();
    let SamplerNote {
        mut h,
        _files: files,
        notes,
        track,
        sampler,
        mut applied,
    } = sampler_note_fixture();
    let device = sampler.id();
    let mut request_id = 100;
    pcm_at_cursor(&mut h, &notes, &mut applied, &mut request_id, 0.25);
    let make_edit = |event: &Event, parameter, value| SamplerParameterEdit {
        request_id: 2,
        project_generation: 0,
        revision: event.state.revision,
        owner: hex_session::service::SamplerParameterOwner::Track(track),
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
    // The queued Stop settles after its declick fade.
    for _ in 0..super::FADE_BLOCKS {
        h.render();
    }
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

#[test]
fn clip_loop_resize_publishes_native_pcm_with_retained_preview_root_and_exact_history() {
    use hex_project::AudioViewResizeOperation as Resize;
    let _native = native_fixture();
    for rate in [44_100, 48_000] {
        let file = Files::new();
        let mut h = ResidentHarness::with_frame_limit_at_rate(32_768, rate);
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::Import(file.wav(rate, 128)));
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        (applied, _) = h.applied(applied);
        let before = h.run(Request::Edit(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 16,
        }));
        (applied, _) = h.applied(applied);
        let repeated = h.run(Request::Edit(Command::ResizeAudioClipView {
            clip,
            operation: Resize::RepeatOuterEnd { project_frame: 640 },
        }));
        (applied, _) = h.applied(applied);
        let resized = h.run(Request::Edit(Command::ResizeAudioClipView {
            clip,
            operation: Resize::CycleEnd { project_frame: 80 },
        }));
        let (revision, metadata) = h.applied(applied);
        applied = revision;
        assert_eq!(metadata.context.editor_revision, resized.state.revision);
        let peaks = &resized.state.audio_clip_waveforms[&clip];
        assert_eq!(peaks.waveform.source_frames(), 624);
        assert_eq!(
            peaks
                .preview_root
                .as_ref()
                .unwrap()
                .waveform
                .source_frames(),
            112
        );
        // The real callback starts at frame129, beyond the first boundary80,
        // and crosses the next boundary144. Every render measures zero total
        // and signed-net allocations/frees through ResidentHarness::render.
        let mut request_id = 800;
        pcm_at_cursor(&mut h, &resized, &mut applied, &mut request_id, 0.25);
        let undone = h.run(Request::Undo);
        (applied, _) = h.applied(applied);
        assert_eq!(undone.state.project, repeated.state.project);
        let undone = h.run(Request::Undo);
        (applied, _) = h.applied(applied);
        assert_eq!(undone.state.project, before.state.project);
        h.run(Request::Redo);
        (applied, _) = h.applied(applied);
        let redone = h.run(Request::Redo);
        h.applied(applied);
        assert_eq!(redone.state.project, resized.state.project);
        assert!(h.publication_refusals.is_empty());
    }
}

#[test]
fn copied_frame_groups_share_audio_under_tight_admission_and_survive_history_reopen() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(256);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 16)));
    let track = imported.state.project.tracks()[0].id();
    let first = imported.state.project.tracks()[0].clips()[0].id();
    (applied, _) = h.applied(applied);
    let duplicated = h.run(Request::Edit(Command::DuplicateClip(first)));
    let Some(Created::Clip(second)) = duplicated.created else {
        panic!("duplicate");
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::GroupClips(vec![first, second])));
    (applied, _) = h.applied(applied);
    h.run(Request::CopyTimeRange(
        hex_project::TimeRange::new(vec![track], 0, 32).unwrap(),
    ));
    for index in 1..=8 {
        h.run(Request::PasteTimeRange {
            track,
            frame: index * 32,
        });
        (applied, _) = h.applied(applied);
    }
    for (command, frames) in [
        (None, 288),
        (Some(Request::Undo), 256),
        (Some(Request::Redo), 288),
    ] {
        if let Some(command) = command {
            h.run(command);
            (applied, _) = h.applied(applied);
        }
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        let pcm: Vec<_> = (0..5).flat_map(|_| h.render()).collect();
        assert!(pcm[..frames].iter().all(|frame| *frame == [0.25; 2]));
        assert!(pcm[frames..].iter().all(|frame| *frame == [0.0; 2]));
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Stop)
            .unwrap();
        h.render();
    }
    let path = files.0.join("shared-clips.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    assert_eq!(hex_storage::load(&path).unwrap(), *saved.state.project);
    assert!(h.publication_refusals.is_empty());
    drop(h);
    let mut reopened = ResidentHarness::with_frame_limit(256);
    assert!(reopened.next().error.is_none());
    reopened.applied(0);
    let old_output = reopened
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    let opened = reopened.run(Request::Open(path));
    assert_eq!(opened.state.project, saved.state.project);
    super::resident_lifecycle::finish_replacement(&mut reopened, &opened, old_output);
    reopened
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let pcm: Vec<_> = (0..5).flat_map(|_| reopened.render()).collect();
    assert!(pcm[..288].iter().all(|frame| *frame == [0.25; 2]));
    assert!(pcm[288..].iter().all(|frame| *frame == [0.0; 2]));
    // The actual worker publishes all candidates before canonical commit;
    // every render also checks zero total and signed-net callback allocations.
}

#[test]
fn history_refused_while_edit_publication_pending_keeps_both_stacks_and_pcm() {
    use hex_engine::SamplerParameter::Gain;
    use hex_session::service::{SamplerEdit, SamplerParameterEdit};
    let _native = native_fixture();
    let SamplerNote {
        mut h,
        _files,
        notes,
        track,
        sampler,
        mut applied,
    } = sampler_note_fixture();
    let device = sampler.id();
    let mut request_id = 100;
    pcm_at_cursor(&mut h, &notes, &mut applied, &mut request_id, 0.25);
    let gain = h.run(Request::SamplerEdit(SamplerEdit::Parameter(
        SamplerParameterEdit {
            request_id: 2,
            project_generation: 0,
            revision: notes.state.revision,
            owner: hex_session::service::SamplerParameterOwner::Track(track),
            device,
            parameter: Gain,
            value: 0.5,
            cancel: Arc::new(AtomicBool::new(false)),
        },
    )));
    assert_eq!(gain.sampler_edit_complete, Some(2));
    // One resident publication may be pending: history is refused, not queued or lost.
    let refuse = |h: &mut ResidentHarness, request: Request, current: &Event| {
        h.service.submit(request).unwrap();
        let refused = h.next();
        assert_eq!(
            refused.error.as_deref(),
            Some("playback publication already pending")
        );
        assert_eq!(refused.state.revision, current.state.revision);
        assert_eq!(refused.state.project, current.state.project);
        refused
    };
    assert!(refuse(&mut h, Request::Undo, &gain).state.can_undo);
    (applied, _) = h.applied(applied);
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project, notes.state.project);
    assert!(refuse(&mut h, Request::Redo, &undone).state.can_redo);
    (applied, _) = h.applied(applied);
    let redone = h.run(Request::Redo);
    assert_eq!(redone.state.project, gain.state.project);
    (applied, _) = h.applied(applied);
    pcm_at_cursor(&mut h, &redone, &mut applied, &mut request_id, 0.125);
}

/// Field report (Pulse #3770): editing a sampler note sounding under the
/// cursor refused with `MutableInstrumentChase`, leaving audio on the old
/// revision. The live publication now restarts audio at the cursor, so the
/// edit and later edits apply, paused or playing, without Stop.
#[test]
fn editing_a_sounding_sampler_note_applies_while_paused_or_playing() {
    let _native = native_fixture();
    for paused in [true, false] {
        let file = Files::new();
        let mut h = ResidentHarness::with_frame_limit(32_768);
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let added = h.run(Request::Edit(Command::AddMidiTrack {
            name: "sampler".into(),
        }));
        let Some(Created::Track(track)) = added.created else {
            panic!("track");
        };
        (applied, _) = h.applied(applied);
        h.run(Request::LoadFirstSampler {
            request_id: 1,
            project_generation: 0,
            revision: added.state.revision,
            track,
            slot: hex_session::service::SamplerLoadSlot::New(
                hex_project::InstrumentLoadPlacement::First,
            ),
            path: file.wav(48_000, 8192),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        (applied, _) = h.applied(applied);
        let placed = h.run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        }));
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("clip");
        };
        (applied, _) = h.applied(applied);
        let with_note = h.run(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(4).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 127,
        }));
        let note = with_note.state.project.track(track).unwrap().midi_clips()[0].notes()[0].id();
        (applied, _) = h.applied(applied);
        let control = |h: &mut ResidentHarness, command| {
            h.audio.as_mut().unwrap().control.command(command).unwrap();
        };
        control(&mut h, hex_engine::playback::PlaybackCommand::Play);
        assert!(h.render().iter().flatten().any(|sample| *sample != 0.0));
        if paused {
            control(&mut h, hex_engine::playback::PlaybackCommand::Pause);
            h.render();
        }
        h.run(Request::Edit(Command::UpdateMidiNote {
            note,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(4).unwrap(),
            pitch: 64,
            channel: 0,
            velocity: 127,
        }));
        (applied, _) = h.applied(applied);
        h.run(Request::Edit(Command::MasterGain { gain: 0.5 }));
        h.applied(applied);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert_eq!(status.playing.load(Acquire), !paused, "paused={paused}");
        if paused {
            control(&mut h, hex_engine::playback::PlaybackCommand::Play);
        }
        assert!(
            (0..4).any(|_| h.render().iter().flatten().any(|sample| *sample != 0.0)),
            "edited note sounds after the restart; paused={paused}"
        );
    }
}

/// Audit (Pulse #3792): the running restart excludes private sampler scopes.
/// A resident sampler revision carries only the ordinary device resource
/// scope, so adding a latency device while its note sounds restarts audio at
/// the cursor instead of refusing.
#[test]
fn adding_a_latency_device_while_a_sampler_plays_applies_without_refusal() {
    let _native = native_fixture();
    let file = Files::new();
    let mut h = ResidentHarness::with_frame_limit(32_768);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track");
    };
    (applied, _) = h.applied(applied);
    h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        slot: hex_session::service::SamplerLoadSlot::New(
            hex_project::InstrumentLoadPlacement::First,
        ),
        path: file.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4).unwrap(),
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip");
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4).unwrap(),
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Play)
        .unwrap();
    assert!(h.render().iter().flatten().any(|sample| *sample != 0.0));
    // Lookahead latency changes the compensated layout under a sounding voice.
    let limited = h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    });
    assert_eq!(
        limited.state.project.track(track).unwrap().devices().len(),
        1
    );
    // `applied` panics on any callback rejection of the new revision.
    h.applied(applied);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire), "the transport keeps running");
    assert!(
        (0..4).any(|_| h.render().iter().flatten().any(|sample| *sample != 0.0)),
        "the sampler sounds after the restart"
    );
}

#[test]
fn running_tempo_edit_with_latency_keeps_the_audio_graph_playing() {
    let _native = native_fixture();
    for (compensated, partition, looped) in [
        (false, 1, false),
        (false, 257, false),
        (true, 1, false),
        (true, 257, false),
        (false, 1, true),
        (false, 257, true),
        (true, 1, true),
        (true, 257, true),
    ] {
        let files = Files::new();
        let mut h = ResidentHarness::with_frame_limit(32_768);
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::Import(files.wav(48_000, 8192)));
        (applied, _) = h.applied(applied);
        let track = imported.state.project.tracks()[0].id();
        if compensated {
            h.run(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
                before: None,
            });
            (applied, _) = h.applied(applied);
        }
        if looped {
            h.run(Request::Edit(Command::SetLoopRegion {
                start: 128,
                // At 90 BPM this becomes a 128-frame loop, so a 257-frame
                // publication callback necessarily crosses at least one seam.
                end: 224,
                enabled: true,
            }));
            (applied, _) = h.applied(applied);
        }
        let latency = h
            .submitted_observation()
            .and_then(|observation| observation.track_latency(track))
            .map_or(0, |latency| latency.local_frames);
        assert_eq!(latency > 0, compensated, "actual applied track latency");
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(hex_engine::playback::PlaybackCommand::Play)
            .unwrap();
        for _ in 0..8 {
            h.render();
        }
        let old_tempo = imported.state.project.tempo_map().clone();
        h.service
            .submit(Request::Edit(Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(90_000_000).unwrap(),
            }))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        let mut edited = None;
        let mut cut = None;
        while edited.is_none() || cut.is_none() {
            h.pump();
            if let Ok(event) = h.service.try_event() {
                edited = Some(event);
            }
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let before_revision = status.applied_revision.load(Acquire);
            let before_position = status.position.load(Acquire);
            let mut frames = [[0.0; 2]; 257];
            let mut callback = h.callback.lock().unwrap();
            let measured = allocation_counter::measure(|| {
                callback
                    .as_mut()
                    .unwrap()
                    .render(&mut frames[..partition])
                    .unwrap();
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
            drop(callback);
            let status = h.audio.as_ref().unwrap().status().unwrap();
            if before_revision == applied && status.applied_revision.load(Acquire) > applied {
                cut = Some((before_position, status.position.load(Acquire)));
            }
            assert!(Instant::now() < until, "tempo edit did not publish");
            std::thread::sleep(Duration::from_millis(1));
        }
        let edited = edited.unwrap();
        assert!(
            edited.error.is_none(),
            "compensated={compensated}: {:?}",
            edited.error
        );
        let (before_position, after_position) = cut.unwrap();
        let beat = old_tempo.frame_to_beat(before_position).unwrap();
        let relocated = edited
            .state
            .project
            .tempo_map()
            .beat_to_frame(beat)
            .unwrap();
        let advanced = relocated + partition as u64;
        let expected_position = match edited.state.project.loop_region() {
            Some(region) if region.enabled() && advanced >= region.end() => {
                if partition == 257 {
                    assert!(relocated >= region.start() && relocated < region.end());
                    assert!(partition as u64 > region.end() - region.start());
                }
                region.start() + (advanced - region.start()) % (region.end() - region.start())
            }
            _ => advanced,
        };
        assert_eq!(
            after_position, expected_position,
            "tempo publication must preserve its beat; compensated={compensated}, partition={partition}, looped={looped}, cut={before_position}"
        );
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!(status.rejected_revision().is_none());
        assert!(
            (0..16).any(|_| h.render().iter().flatten().any(|sample| *sample != 0.0)),
            "audio must continue after the tempo edit; compensated={compensated}"
        );
    }
}

#[test]
fn running_tempo_edit_with_streamed_audio_and_latency_preserves_position() {
    check_streamed_tempo_edit(false);
}

#[test]
fn running_looped_tempo_edit_reanchors_streamed_audio_with_latency() {
    check_streamed_tempo_edit(true);
}

fn check_streamed_tempo_edit(looped: bool) {
    const PAGE: u64 = hex_engine::STREAM_PAGE_FRAMES as u64;
    const OLD_START: u64 = 6015;
    const OLD_END: u64 = 6207;
    let _native = native_fixture();
    for partition in [1, 257] {
        let files = Files::new();
        // Force this ordinary document's import through its file-backed source path.
        let mut h = ResidentHarness::with_frame_limit_at_rate(4, 48_000);
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let path = files.wav(48_000, 32_768);
        if looped {
            let mut wav = fs::read(&path).unwrap();
            // The old loop stays in page 1. After 120 -> 90 BPM its new
            // range straddles page 2, whose sign distinguishes real render.
            for sample in wav[44 + (2 * PAGE) as usize * 2..].chunks_exact_mut(2) {
                sample.copy_from_slice(&(-8192_i16).to_le_bytes());
            }
            fs::write(&path, wav).unwrap();
        }
        let imported = h.run(Request::Import(path));
        (applied, _) = h.applied(applied);
        assert!(
            imported
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .sources()
                .values()
                .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
        );
        let track = imported.state.project.tracks()[0].id();
        h.run(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
            before: None,
        });
        (applied, _) = h.applied(applied);
        if looped {
            h.run(Request::Edit(Command::SetLoopRegion {
                start: OLD_START,
                end: OLD_END,
                enabled: true,
            }));
            (applied, _) = h.applied(applied);
        }
        assert!(
            h.submitted_observation()
                .unwrap()
                .track_latency(track)
                .unwrap()
                .local_frames
                > 0
        );
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(if looped {
                PlaybackCommand::PlayFrom(OLD_START)
            } else {
                PlaybackCommand::Play
            })
            .unwrap();
        let ready = Instant::now() + Duration::from_secs(2);
        while !h.render().iter().flatten().any(|sample| *sample != 0.0) {
            assert!(Instant::now() < ready, "streamed audio did not start");
            std::thread::sleep(Duration::from_millis(1));
        }
        let old_tempo = imported.state.project.tempo_map().clone();
        h.service
            .submit(Request::Edit(Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(90_000_000).unwrap(),
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut edited = None;
        let mut cut = None;
        while edited.is_none() || cut.is_none() {
            h.pump();
            if let Ok(event) = h.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                edited = Some(event);
            }
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let before_revision = status.applied_revision.load(Acquire);
            let before_position = status.position.load(Acquire);
            let mut frames = [[0.0; 2]; 257];
            let mut callback = h.callback.lock().unwrap();
            let allocations = allocation_counter::measure(|| {
                callback
                    .as_mut()
                    .unwrap()
                    .render(&mut frames[..partition])
                    .unwrap();
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
            drop(callback);
            let status = h.audio.as_ref().unwrap().status().unwrap();
            if before_revision == applied && status.applied_revision.load(Acquire) > applied {
                cut = Some((before_position, status.position.load(Acquire)));
            }
            assert!(
                Instant::now() < deadline,
                "streamed tempo edit did not publish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let (before, after) = cut.unwrap();
        let beat = old_tempo.frame_to_beat(before).unwrap();
        let project = &edited.unwrap().state.project;
        let relocated = project.tempo_map().beat_to_frame(beat).unwrap();
        let advanced = relocated + partition as u64;
        let expected = if looped {
            let region = project.loop_region().unwrap();
            assert!(region.enabled());
            assert_eq!(region.start()..region.end(), 8020..8276);
            assert!((OLD_START..OLD_END).contains(&before));
            assert!((region.start()..region.end()).contains(&relocated));
            assert_eq!(region.start() / PAGE + 1, (region.end() - 1) / PAGE);
            if partition == 257 {
                assert!(advanced >= region.end(), "publication block must wrap");
            }
            region.start() + (advanced - region.start()) % (region.end() - region.start())
        } else {
            advanced
        };
        assert_eq!(after, expected, "partition={partition} looped={looped}");
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!(status.rejected_revision().is_none());
        // Drain more than the Limiter's 288-frame delay before checking that
        // the new graph is still producing streamed audio.
        for _ in 0..8 {
            h.render();
        }
        let ready = Instant::now() + Duration::from_secs(2);
        let mut positive = false;
        let mut negative = false;
        while !positive || (looped && !negative) {
            for sample in h.render().iter().flatten() {
                positive |= *sample > 0.05;
                negative |= *sample < -0.05;
            }
            assert!(Instant::now() < ready, "streamed audio did not resume");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
