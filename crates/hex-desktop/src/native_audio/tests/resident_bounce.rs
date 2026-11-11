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
        track,
        slot: hex_session::service::SamplerLoadSlot::New { replacing: None },
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
