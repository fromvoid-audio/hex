//! Native conversion uses the same resource source renderer and one canonical history edge.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{
    BeatTime, Command, Created, DeviceComparisonAction, DeviceVariant, SessionCellContent,
};
use hex_session::service::preset::Owner;

fn edit(h: &mut ResidentHarness, applied: &mut u64, command: Command) -> Event {
    let event = h.run(Request::Edit(command));
    (*applied, _) = h.applied(*applied);
    event
}
fn seek(h: &mut ResidentHarness, applied: &mut u64, revision: u64, request: u64) -> [f32; 2] {
    let mut intent = h.seek_intent(request, revision, None, None);
    intent.destination_frame = 400;
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    h.seek_result().result.unwrap();
    (*applied, _) = h.applied(*applied);
    let pcm = h.render();
    for frame in &pcm {
        assert_eq!(*frame, pcm[0]);
    }
    pcm[0]
}
#[test]
fn loaded_resident_bounce_preserves_float_source_mix_single_history_and_native_playback() {
    let _native = native_fixture();
    let source = Files::new();
    let other = Files::new();
    let inactive = Files::new();
    let mut h = ResidentHarness::with_frame_limit(65_536);
    h.next();
    let (mut applied, _) = h.applied(0);
    let added = edit(
        &mut h,
        &mut applied,
        Command::AddMidiTrack {
            name: "bounce".into(),
        },
    );
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        target: hex_session::service::SamplerLoadTarget::Track {
            track,
            slot: hex_session::service::SamplerLoadSlot::New(
                hex_project::InstrumentLoadPlacement::First,
            ),
        },
        path: source.wav(48_000, 8192),
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
    let inactive_path = inactive.wav(48_000, 8192);
    let mut bytes = fs::read(&inactive_path).unwrap();
    for sample in bytes[44..].chunks_exact_mut(2) {
        sample.copy_from_slice(&(-4096_i16).to_le_bytes());
    }
    fs::write(&inactive_path, bytes).unwrap();
    let imported = h.run(Request::ImportDeviceAsset {
        path: inactive_path,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let Some(Created::Asset(inactive_asset)) = imported.created else {
        panic!("asset")
    };
    let compare = |event: &Event, expected| Request::CompareDevice {
        owner: Owner {
            project_generation: 0,
            revision: event.state.revision,
            device,
            class: hex_engine::NativeDeviceClass::Instrument,
            type_id: event
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .type_id(),
        },
        expected,
        action: DeviceComparisonAction::Switch,
    };
    // First switch is metadata-only. Give B its own original bank, then return to A.
    h.run(compare(&imported, DeviceVariant::A));
    let bank_b = h.run(Request::AssignSampler {
        track,
        asset: inactive_asset,
    });
    (applied, _) = h.applied(applied);
    let bank_a = h.run(compare(&bank_b, DeviceVariant::B));
    (applied, _) = h.applied(applied);
    let instrument = bank_a.state.project.device(device).unwrap();
    let states = instrument.authored_states().collect::<Vec<_>>();
    assert_eq!(states.len(), 2);
    assert_ne!(states[0].asset_references(), states[1].asset_references());
    let state = instrument
        .state()
        .clone()
        .with_input_gain_db(-6.0)
        .unwrap()
        .with_output_gain_db(6.0)
        .unwrap();
    let input_gain = state.input_gain_linear();
    let output_gain = state.output_gain_linear();
    edit(
        &mut h,
        &mut applied,
        Command::ReplaceDeviceState { device, state },
    );
    edit(&mut h, &mut applied, Command::Gain { track, gain: 0.5 });
    let offset = BeatTime::from_raw(BeatTime::ONE.raw() / 128);
    let length = BeatTime::from_raw(BeatTime::ONE.raw() / 16);
    let placed = edit(
        &mut h,
        &mut applied,
        Command::PlaceMidiClip {
            track,
            start: offset,
            length,
        },
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    edit(
        &mut h,
        &mut applied,
        Command::SetMidiClipSource {
            clip,
            source_length: BeatTime::ONE,
            source_start: offset,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::ONE,
            looping: false,
            length,
        },
    );
    edit(
        &mut h,
        &mut applied,
        Command::AddMidiNote {
            clip,
            start: offset,
            duration: length,
            pitch: 60,
            channel: 0,
            velocity: 127,
        },
    );
    let before = h.run(Request::ImportToNewTrack {
        path: other.wav(48_000, 8192),
        before_track: None,
        start: 0,
    });
    (applied, _) = h.applied(applied);
    let expected = 0.25 + 0.25 * output_gain * 0.5;
    let original_pcm = seek(&mut h, &mut applied, before.state.revision, 10);
    assert!((original_pcm[0] - expected).abs() < 1e-6);
    let range = before.state.project.midi_bounce_range(clip).unwrap();
    assert!(range.0 > 0);
    // The callback is playing inside the gate when the one reserved conversion is accepted.
    let converted = h.run(Request::ConvertClip {
        project_generation: 0,
        revision: before.state.revision,
        clip,
        to_audio: true,
    });
    (applied, _) = h.applied(applied);
    assert_eq!(converted.state.revision, before.state.revision + 1);
    let audio = converted.state.project.clip(clip).unwrap();
    assert_eq!((audio.start(), audio.duration()), range);
    let path = converted
        .state
        .project
        .asset(audio.asset())
        .unwrap()
        .path()
        .to_owned();
    let raw = fs::read(&path).unwrap();
    assert_eq!(&raw[20..22], &3_u16.to_le_bytes());
    let float = raw[hex_export::FLOAT32_WAV_HEADER_BYTES as usize..]
        .chunks_exact(8)
        .map(|frame| f32::from_le_bytes(frame[..4].try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(float.len(), range.1 as usize);
    for sample in float {
        assert!((sample - 0.25 / input_gain).abs() < 1e-6);
    }
    let bounced_pcm = seek(&mut h, &mut applied, converted.state.revision, 11);
    assert!((bounced_pcm[0] - original_pcm[0]).abs() < 1e-6);
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert!(undone.state.project.midi_clip(clip).is_some());
    assert!(undone.state.project.clip(clip).is_none());
    // Restoring a pre-cut gate does not invent its missing DSP history. It is silent
    // until a future authored onset; the unrelated audio continues. Explicit seek below
    // reconstructs the restored gate and must recover the original mixed PCM.
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    let restored = seek(&mut h, &mut applied, undone.state.revision, 12);
    assert!((restored[0] - original_pcm[0]).abs() < 1e-6);
    let redone = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    assert_eq!(redone.state.project.clip(clip).unwrap().id(), clip);
    assert!(
        (seek(&mut h, &mut applied, redone.state.revision, 13)[0] - original_pcm[0]).abs() < 1e-6
    );
    // A Session cell uses one initial pass even when its source loops. Refusing the
    // final publication or its budget must leave the MIDI cell and canonical edge intact.
    let restored = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert!(restored.state.project.midi_clip(clip).is_some());
    let scene = edit(
        &mut h,
        &mut applied,
        Command::AddScene {
            name: "bounce scene".into(),
        },
    );
    let Some(Created::Scene(scene)) = scene.created else {
        panic!("scene")
    };
    let copied = edit(
        &mut h,
        &mut applied,
        Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        },
    );
    let Some(SessionCellContent::Midi(cell)) = copied.state.project.session_cell(track, scene)
    else {
        panic!("cell")
    };
    let cell = cell.id();
    let selected = edit(
        &mut h,
        &mut applied,
        Command::SetSessionMidiClipSource {
            clip: cell,
            source_length: BeatTime::ONE,
            source_start: offset,
            loop_start: offset,
            loop_end: BeatTime::from_raw(offset.raw() + length.raw()),
            looping: true,
        },
    );
    let session_frames = selected.state.project.midi_bounce_range(cell).unwrap().1;
    assert_eq!(session_frames, range.1);
    for flag in [Arc::clone(&h.refuse), Arc::clone(&h.budget_refuse)] {
        flag.store(true, std::sync::atomic::Ordering::Release);
        h.service
            .submit(Request::ConvertClip {
                project_generation: 0,
                revision: selected.state.revision,
                clip: cell,
                to_audio: true,
            })
            .unwrap();
        let refused = h.next();
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, selected.state.project);
        assert_eq!(refused.state.revision, selected.state.revision);
        assert_eq!(refused.state.can_undo, selected.state.can_undo);
        assert_eq!(refused.state.can_redo, selected.state.can_redo);
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            applied
        );
        flag.store(false, std::sync::atomic::Ordering::Release);
    }
    let session = h.run(Request::ConvertClip {
        project_generation: 0,
        revision: selected.state.revision,
        clip: cell,
        to_audio: true,
    });
    (applied, _) = h.applied(applied);
    let audio = session.state.project.session_audio_clip(cell).unwrap();
    assert_eq!(audio.source_end(), session_frames);
    assert_eq!(audio.id(), cell);
    assert!(session.state.project.session_midi_clip(cell).is_none());
    let session_path = session
        .state
        .project
        .asset(audio.asset())
        .unwrap()
        .path()
        .to_owned();
    let bytes = fs::read(&session_path).unwrap();
    assert_eq!(&bytes[20..22], &3_u16.to_le_bytes());
    assert_eq!(
        bytes.len(),
        hex_export::FLOAT32_WAV_HEADER_BYTES as usize + session_frames as usize * 8
    );
    for frame in bytes[hex_export::FLOAT32_WAV_HEADER_BYTES as usize..].chunks_exact(8) {
        assert!(
            (f32::from_le_bytes(frame[..4].try_into().unwrap()) - 0.25 / input_gain).abs() < 1e-6
        );
    }
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert!(undone.state.project.session_midi_clip(cell).is_some());
    let redone = h.run(Request::Redo);
    h.applied(applied);
    assert!(redone.state.project.session_audio_clip(cell).is_some());
    // Current-format persistence keeps both authored source banks and generated media.
    let saved = source.0.join("bounced.hex");
    hex_storage::save_new(&saved, &redone.state.project).unwrap();
    let reopened = hex_storage::load(&saved).unwrap();
    assert_eq!(&reopened, redone.state.project.as_ref());
    assert_eq!(
        reopened.device(device).unwrap().authored_states().count(),
        2
    );
    fs::remove_file(&session_path).unwrap();
    fs::remove_dir(session_path.parent().unwrap()).unwrap();
    // Only this test's newly generated media is removed, never a source or preexisting bounce.
    fs::remove_file(&path).unwrap();
    fs::remove_dir(path.parent().unwrap()).unwrap();
}

/// Bounce in place while playing (Pulse #3784): the transport keeps running
/// and audio restarts at the cursor on the baked clip. A carried effect state
/// or detached tail would change the constant mix, so equal PCM proves none.
#[test]
fn track_bounce_in_place_and_its_history_apply_while_playing() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(2_000_000);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::ImportToNewTrack {
        path: files.wav(48_000, 48_000),
        before_track: None,
        start: 0,
    });
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(6.0)
    .unwrap();
    let inserted = edit(
        &mut h,
        &mut applied,
        Command::InsertDevice {
            track,
            before: None,
            state,
        },
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mix = h.render()[63];
    assert!(mix[0] > 0.4, "utility gain is audible: {mix:?}");
    let playing = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
    };
    let assert_mix = |h: &ResidentHarness, step: &str| {
        assert!(playing(h), "{step}: transport keeps running");
        for frame in h.render() {
            assert!((frame[0] - mix[0]).abs() < 1e-5, "{step}: {frame:?}");
        }
    };
    let bounced = super::resident_sections::run_callback(
        &mut h,
        Request::BounceArrangement {
            project_generation: 0,
            revision: inserted.state.revision,
            target: hex_project::BounceTarget::Track(track),
            in_place: true,
            adoption: hex_project::BounceAdoption::NewTracks,
            control: hex_export::ExportControl::default(),
        },
    );
    assert!(bounced.error.is_none(), "{:?}", bounced.error);
    let project = &bounced.state.project;
    assert!(project.track(track).unwrap().devices().is_empty());
    let baked = project
        .asset(project.track(track).unwrap().clips()[0].asset())
        .unwrap()
        .path()
        .to_owned();
    (applied, _) = h.applied(applied);
    assert_mix(&h, "bounce");
    for (request, step) in [(Request::Undo, "undo"), (Request::Redo, "redo")] {
        let event = super::resident_sections::run_callback(&mut h, request);
        assert!(event.error.is_none(), "{step}: {:?}", event.error);
        (applied, _) = h.applied(applied);
        assert_mix(&h, step);
    }
    // Only this test's generated media is removed, never its source.
    fs::remove_file(&baked).unwrap();
    fs::remove_dir(baked.parent().unwrap()).unwrap();
}

/// End-to-end M24 qualification (#3252, #2756): create/edit a native synth and
/// resource-backed sampler, share one live synth across two tracks without
/// doubling, whole-track bounce a contributor, delete all tracks to leave
/// zero-user retained sources, cold save/reopen from disk without preset export
/// or Undo recovery, assign the SAME retained sources to fresh tracks and render
/// with zero callback allocations, then verify explicit-only deletion and Undo.
#[test]
fn retained_native_synth_and_sampler_create_share_bounce_delete_save_reopen_reuse_and_explicit_delete()
 {
    let _native = native_fixture();
    let files = Files::new();
    let sampler_wav = files.wav(48_000, 16_384);
    let original_wav_bytes = fs::read(&sampler_wav).unwrap();
    let saved_path = files.0.join("m24-retained-workflow.hex");

    let mut h = ResidentHarness::with_frame_limit(2_000_000);
    h.next();
    let (mut applied, _) = h.applied(0);

    // 1. Create a native synth (receiver) with a track-local utility effect and a resource-backed Sampler.
    let added_receiver = edit(
        &mut h,
        &mut applied,
        Command::AddMidiTrack {
            name: "synth receiver".into(),
        },
    );
    let Some(Created::Track(receiver_track)) = added_receiver.created else {
        panic!("receiver track")
    };
    let inserted_synth = h.run(Request::AddNativeDevice {
        track: receiver_track,
        class: hex_engine::NativeDeviceClass::Instrument,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.instrument.sine")),
        before: None,
    });
    (applied, _) = h.applied(applied);
    let synth_source = inserted_synth
        .state
        .project
        .track(receiver_track)
        .unwrap()
        .source_id()
        .expect("synth auto-registered as retained source");
    let synth_device = inserted_synth
        .state
        .project
        .native_source(synth_source)
        .unwrap()
        .instrument()
        .id();
    edit(
        &mut h,
        &mut applied,
        Command::RenameNativeSource {
            source: synth_source,
            name: "retained sine lead".into(),
        },
    );
    let synth_state = inserted_synth
        .state
        .project
        .device(synth_device)
        .unwrap()
        .state()
        .clone()
        .with_output_gain_db(-6.0)
        .unwrap();
    edit(
        &mut h,
        &mut applied,
        Command::ReplaceDeviceState {
            device: synth_device,
            state: synth_state,
        },
    );
    let local_utility = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(6.0)
    .unwrap();
    edit(
        &mut h,
        &mut applied,
        Command::InsertDevice {
            track: receiver_track,
            before: None,
            state: local_utility,
        },
    );

    let added_sampler = edit(
        &mut h,
        &mut applied,
        Command::AddMidiTrack {
            name: "sampler track".into(),
        },
    );
    let Some(Created::Track(sampler_track)) = added_sampler.created else {
        panic!("sampler track")
    };
    let loaded_sampler = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added_sampler.state.revision,
        target: hex_session::service::SamplerLoadTarget::Track {
            track: sampler_track,
            slot: hex_session::service::SamplerLoadSlot::New(
                hex_project::InstrumentLoadPlacement::First,
            ),
        },
        path: sampler_wav.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let sampler_source = loaded_sampler
        .state
        .project
        .track(sampler_track)
        .unwrap()
        .source_id()
        .expect("sampler auto-registered as retained source");
    let sampler_device = loaded_sampler
        .state
        .project
        .native_source(sampler_source)
        .unwrap()
        .instrument()
        .id();
    let sampler_asset = loaded_sampler
        .state
        .project
        .device(sampler_device)
        .unwrap()
        .state()
        .asset_references()[0]
        .asset;
    edit(
        &mut h,
        &mut applied,
        Command::RenameNativeSource {
            source: sampler_source,
            name: "retained constant sampler".into(),
        },
    );
    let sampler_state = loaded_sampler
        .state
        .project
        .device(sampler_device)
        .unwrap()
        .state()
        .clone()
        .with_output_gain_db(-6.0)
        .unwrap();
    edit(
        &mut h,
        &mut applied,
        Command::ReplaceDeviceState {
            device: sampler_device,
            state: sampler_state,
        },
    );

    // 2. Bind a second contributor track to the SAME retained synth and place simultaneous MIDI.
    let added_contributor = edit(
        &mut h,
        &mut applied,
        Command::AddMidiTrack {
            name: "synth contributor".into(),
        },
    );
    let Some(Created::Track(contributor_track)) = added_contributor.created else {
        panic!("contributor track")
    };
    edit(
        &mut h,
        &mut applied,
        Command::BindTrackSource {
            track: contributor_track,
            source: Some(synth_source),
        },
    );
    for (track, pitch) in [(receiver_track, 60_u8), (contributor_track, 67_u8)] {
        let placed = edit(
            &mut h,
            &mut applied,
            Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            },
        );
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("midi clip")
        };
        edit(
            &mut h,
            &mut applied,
            Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch,
                channel: 0,
                velocity: 110,
            },
        );
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut shared_peak = 0.0_f32;
    let measured_shared = allocation_counter::measure(|| {
        for _ in 0..8 {
            for frame in h.render() {
                shared_peak = shared_peak.max(frame[0].abs()).max(frame[1].abs());
            }
        }
    });
    assert!(shared_peak > 0.01, "shared synth playback must be audible");
    assert_eq!(measured_shared.count_total, 0);
    assert_eq!(measured_shared.count_current, 0);
    assert_eq!(measured_shared.bytes_total, 0);
    assert_eq!(measured_shared.bytes_current, 0);

    // 3. Whole-track in-place bounce on the contributor track: contributor prints to audio
    // and unlinks, while receiver + retained synth stay intact and Undo/Redo works in one step.
    let before_bounce = h.run(Request::Edit(Command::Gain {
        track: contributor_track,
        gain: 1.0,
    }));
    (applied, _) = h.applied(applied);
    let bounced = super::resident_sections::run_callback(
        &mut h,
        Request::BounceArrangement {
            project_generation: 0,
            revision: before_bounce.state.revision,
            target: hex_project::BounceTarget::Track(contributor_track),
            in_place: true,
            adoption: hex_project::BounceAdoption::NewTracks,
            control: hex_export::ExportControl::default(),
        },
    );
    assert!(bounced.error.is_none(), "{:?}", bounced.error);
    (applied, _) = h.applied(applied);
    assert!(
        bounced
            .state
            .project
            .track(contributor_track)
            .unwrap()
            .source_id()
            .is_none()
    );
    assert_eq!(
        bounced
            .state
            .project
            .track(receiver_track)
            .unwrap()
            .source_id(),
        Some(synth_source)
    );
    assert!(bounced.state.project.native_source(synth_source).is_some());
    let baked_path = bounced
        .state
        .project
        .asset(
            bounced
                .state
                .project
                .track(contributor_track)
                .unwrap()
                .clips()[0]
                .asset(),
        )
        .unwrap()
        .path()
        .to_owned();
    let undone_bounce = super::resident_sections::run_callback(&mut h, Request::Undo);
    assert!(undone_bounce.error.is_none());
    (applied, _) = h.applied(applied);
    assert_eq!(
        undone_bounce
            .state
            .project
            .track(contributor_track)
            .unwrap()
            .source_id(),
        Some(synth_source)
    );
    let redone_bounce = super::resident_sections::run_callback(&mut h, Request::Redo);
    assert!(redone_bounce.error.is_none());
    (applied, _) = h.applied(applied);
    assert!(
        redone_bounce
            .state
            .project
            .track(contributor_track)
            .unwrap()
            .source_id()
            .is_none()
    );

    // 4. Delete printed track, receiver track, and sampler track: BOTH retained sources
    // survive with zero linked tracks and persist across a cold disk save.
    for track in [contributor_track, receiver_track, sampler_track] {
        edit(&mut h, &mut applied, Command::RemoveTrack(track));
    }
    let saved = h.run(Request::SaveNew(saved_path.clone()));
    assert!(saved.save_complete, "{:?}", saved.error);
    assert_eq!(saved.state.project.tracks().len(), 0);
    assert!(saved.state.project.native_source(synth_source).is_some());
    assert!(saved.state.project.native_source(sampler_source).is_some());
    assert_eq!(
        saved
            .state
            .project
            .native_source(synth_source)
            .unwrap()
            .receiver(),
        None
    );
    assert_eq!(
        saved
            .state
            .project
            .native_source(sampler_source)
            .unwrap()
            .receiver(),
        None
    );
    // 5. Reopen from disk (no Undo history, no preset export):
    // zero-user retained synth & resource-backed sampler keep identity, names, parameters & assets,
    // and immediately sound when bound to brand-new tracks with zero callback allocations.
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let opened = h.run(Request::Open(saved_path));
    assert!(
        opened.open_complete && opened.project_replaced && opened.error.is_none(),
        "{:?}",
        opened.error
    );
    assert!(!opened.state.can_undo);
    assert_eq!(
        opened
            .state
            .project
            .native_source(synth_source)
            .unwrap()
            .name(),
        "retained sine lead"
    );
    assert_eq!(
        opened
            .state
            .project
            .native_source(sampler_source)
            .unwrap()
            .name(),
        "retained constant sampler"
    );
    assert!(
        (opened
            .state
            .project
            .device(synth_device)
            .unwrap()
            .state()
            .output_gain_db()
            + 6.0)
            .abs()
            < 1e-6
    );
    assert_eq!(
        opened
            .state
            .project
            .device(sampler_device)
            .unwrap()
            .state()
            .asset_references()[0]
            .asset,
        sampler_asset
    );
    applied = super::resident_lifecycle::finish_replacement(&mut h, &opened, old_output);

    let fresh_synth_track = {
        let added = edit(
            &mut h,
            &mut applied,
            Command::AddMidiTrack {
                name: "reused synth".into(),
            },
        );
        let Some(Created::Track(track)) = added.created else {
            panic!("fresh synth track")
        };
        edit(
            &mut h,
            &mut applied,
            Command::BindTrackSource {
                track,
                source: Some(synth_source),
            },
        );
        let placed = edit(
            &mut h,
            &mut applied,
            Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            },
        );
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("fresh synth clip")
        };
        edit(
            &mut h,
            &mut applied,
            Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 64,
                channel: 0,
                velocity: 120,
            },
        );
        track
    };
    let fresh_sampler_track = {
        let added = edit(
            &mut h,
            &mut applied,
            Command::AddMidiTrack {
                name: "reused sampler".into(),
            },
        );
        let Some(Created::Track(track)) = added.created else {
            panic!("fresh sampler track")
        };
        edit(
            &mut h,
            &mut applied,
            Command::BindTrackSource {
                track,
                source: Some(sampler_source),
            },
        );
        let placed = edit(
            &mut h,
            &mut applied,
            Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            },
        );
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("fresh sampler clip")
        };
        edit(
            &mut h,
            &mut applied,
            Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            },
        );
        track
    };

    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut reused_peak = 0.0_f32;
    let measured_reused = allocation_counter::measure(|| {
        for _ in 0..8 {
            for frame in h.render() {
                reused_peak = reused_peak.max(frame[0].abs()).max(frame[1].abs());
            }
        }
    });
    assert!(
        reused_peak > 0.05,
        "reused retained synth + sampler must be audible after reopen"
    );
    assert_eq!(measured_reused.count_total, 0);
    assert_eq!(measured_reused.count_current, 0);
    assert_eq!(measured_reused.bytes_total, 0);
    assert_eq!(measured_reused.bytes_current, 0);

    // 6. Explicit manager deletion only: deleting a referenced source refuses cleanly;
    // unlinking then deleting removes the source, and Undo restores exact identity/state.
    h.service
        .submit(Request::Edit(Command::RemoveNativeSource(sampler_source)))
        .unwrap();
    let refused_delete = h.next();
    assert!(refused_delete.error.is_some());
    assert!(
        refused_delete
            .state
            .project
            .native_source(sampler_source)
            .is_some()
    );
    edit(
        &mut h,
        &mut applied,
        Command::BindTrackSource {
            track: fresh_sampler_track,
            source: None,
        },
    );
    let deleted = edit(
        &mut h,
        &mut applied,
        Command::RemoveNativeSource(sampler_source),
    );
    assert!(
        deleted
            .state
            .project
            .native_source(sampler_source)
            .is_none()
    );
    let restored = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(
        restored
            .state
            .project
            .native_source(sampler_source)
            .unwrap()
            .name(),
        "retained constant sampler"
    );
    let _ = (fresh_synth_track, applied);
    assert_eq!(fs::read(&sampler_wav).unwrap(), original_wav_bytes);
    if baked_path.exists() {
        fs::remove_file(&baked_path).unwrap();
        let _ = fs::remove_dir(baked_path.parent().unwrap());
    }
}
