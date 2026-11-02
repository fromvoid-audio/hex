//! User-facing discrete tuning/comparison and direct source APIs share native transactions.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{
    BeatTime, Command, Created, DeviceComparisonAction as Action, DeviceVariant as Slot,
};
use hex_session::service::{ResidentPublicationAcceptance, preset::Owner};
use hex_session::tuning_gesture::TuningLibraryOperation;

fn owner(event: &Event, device: hex_project::DeviceId) -> Owner {
    Owner {
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
    }
}
fn seek_output(
    h: &mut ResidentHarness,
    event: &Event,
    applied: &mut u64,
    id: u64,
) -> (usize, [hex_engine::Frame; 64]) {
    let mut intent = h.seek_intent(id, event.state.revision, None, None);
    intent.destination_frame = 0;
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    let ResidentPublicationAcceptance::Submitted { audio_revision, .. } =
        h.seek_result().result.unwrap()
    else {
        panic!("seek")
    };
    let (actual, metadata) = h.applied(*applied);
    assert_eq!(actual, audio_revision);
    assert_eq!(metadata.context.editor_revision, event.state.revision);
    *applied = actual;
    let start = usize::try_from(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire),
    )
    .unwrap();
    (start, h.render())
}
fn seek_pcm(
    h: &mut ResidentHarness,
    event: &Event,
    applied: &mut u64,
    id: u64,
    step: usize,
    gain: f32,
) {
    let (start, pcm) = seek_output(h, event, applied, id);
    if step == 1 {
        let expected = std::array::from_fn(|i| [((start + i) % 4 + 1) as f32 * 0.125 * gain; 2]);
        assert_eq!(pcm, expected);
        return;
    }
    // Faster voices are band-limited (#4089). Every partial of the looped
    // 4-frame ramp except DC lies at or above the output Nyquist at step 2, so
    // the voice plays the ramp's mean, (1 + 2 + 3 + 4) / 4 * 0.125.
    assert_eq!(step, 2);
    for frame in pcm {
        for sample in frame {
            assert!((sample - 0.3125 * gain).abs() < 1e-3, "{pcm:?}");
        }
    }
}

#[test]
fn loaded_resident_library_import_comparison_and_direct_source_requests_are_atomic() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 8192);
    let mut bytes = fs::read(&path).unwrap();
    for (index, sample) in bytes[44..].chunks_exact_mut(2).enumerate() {
        sample.copy_from_slice(&((index % 4 + 1) as i16 * 4096).to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    let mut h = ResidentHarness::with_frame_limit(32_768);
    h.next();
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::ImportDeviceAsset {
            path: path.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let refused = h.next();
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, added.state.project);
    assert_eq!(refused.state.revision, added.state.revision);
    assert_eq!(refused.state.can_undo, added.state.can_undo);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let imported = h.run(Request::ImportDeviceAsset {
        path: path.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let Some(Created::Asset(asset)) = imported.created else {
        panic!("asset")
    };
    (applied, _) = h.applied(applied);
    let assigned = h.run(Request::AssignSampler { track, asset });
    (applied, _) = h.applied(applied);
    let device = assigned
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let noop = h.run(Request::AssignSampler { track, asset });
    (applied, _) = h.applied(applied);
    assert_eq!(noop.state.project, assigned.state.project);
    assert_eq!(noop.state.revision, assigned.state.revision);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    let authored = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    seek_pcm(&mut h, &authored, &mut applied, 10, 1, 1.0);
    let authored = h.run(Request::Edit(Command::SetTuning(super::sampler::tuning(
        440.0,
    ))));
    (applied, _) = h.applied(applied);

    let tuned = super::sampler::tuning(880.0);
    let preset = files.0.join("double.freqs");
    hex_storage::tuning_library::export_file(&preset, "double", tuned.scale()).unwrap();
    let request = |event: &Event, operation_id| Request::TuningLibrary {
        project_generation: 0,
        revision: event.state.revision,
        operation_id,
        operation: TuningLibraryOperation::Import {
            path: preset.clone(),
        },
    };
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service.submit(request(&authored, 1)).unwrap();
    let refused = h.next();
    assert!(refused.tuning_library.as_ref().unwrap().error.is_some());
    assert_eq!(refused.state.project, authored.state.project);
    assert_eq!(refused.state.revision, authored.state.revision);
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
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let imported = h.run(request(&authored, 2));
    assert!(imported.tuning_library.as_ref().unwrap().error.is_none());
    assert_eq!(imported.state.project.tuning().scale(), tuned.scale());
    (applied, _) = h.applied(applied);
    seek_pcm(&mut h, &imported, &mut applied, 11, 2, 1.0);

    // Identical bank Switch is metadata, then a real bank edit must compile/publish normally.
    let switched = h.run(Request::CompareDevice {
        owner: owner(&imported, device),
        expected: Slot::A,
        action: Action::Switch,
    });
    assert_eq!(
        switched
            .state
            .project
            .device(device)
            .unwrap()
            .active_variant(),
        Slot::B
    );
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
    assert!(switched.state.comparison_audio.as_ref().unwrap().follows(
        &imported.state,
        &switched.state,
        0
    ));
    let quiet = hex_engine::patch_sampler_parameter_on_control(
        hex_engine::SamplerParameter::Gain,
        switched.state.project.device(device).unwrap().state(),
        0.5,
    )
    .unwrap();
    let _edited = h.run(Request::Edit(Command::ReplaceDeviceState {
        device,
        state: quiet,
    }));
    (applied, _) = h.applied(applied);
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(undone.state.project, switched.state.project);
    let edited = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    assert_eq!(
        edited
            .state
            .project
            .device(device)
            .unwrap()
            .active_variant(),
        Slot::B
    );
    seek_pcm(&mut h, &edited, &mut applied, 12, 2, 0.5);
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::CompareDevice {
            owner: owner(&edited, device),
            expected: Slot::B,
            action: Action::Switch,
        })
        .unwrap();
    let refused = h.next();
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, edited.state.project);
    assert_eq!(refused.state.revision, edited.state.revision);
    assert_eq!(refused.state.can_redo, edited.state.can_redo);
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
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let loud = h.run(Request::CompareDevice {
        owner: owner(&edited, device),
        expected: Slot::B,
        action: Action::Switch,
    });
    (applied, _) = h.applied(applied);
    assert_eq!(
        loud.state.project.device(device).unwrap().active_variant(),
        Slot::A
    );
    seek_pcm(&mut h, &loud, &mut applied, 13, 2, 1.0);
    let undo = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(undo.state.project, edited.state.project);
    seek_pcm(&mut h, &undo, &mut applied, 14, 2, 0.5);

    // A rejected library request must not retire an already audible overlay at its barrier.
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};
    let key = GestureKey {
        generation: 0,
        token: 9,
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
    (applied, _) = h.applied(applied);
    let before_pcm = h.render();
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service.submit(request(&undo, 3)).unwrap();
    let refused = h.next();
    assert!(refused.tuning_library.as_ref().unwrap().error.is_some());
    assert_eq!(refused.state.project, undo.state.project);
    assert_eq!(refused.state.revision, undo.state.revision);
    assert_eq!(refused.active_device_gesture, preview.active_device_gesture);
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
        applied
    );
    assert_eq!(h.render(), before_pcm);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    (applied, _) = h.applied(applied);
    let extra = h.run(Request::ImportDeviceAsset {
        path,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert_eq!(extra.state.project.assets().count(), 2);
    (applied, _) = h.applied(applied);
    seek_pcm(&mut h, &extra, &mut applied, 15, 2, 0.5);
}

#[test]
fn resource_free_library_import_and_audible_comparison_publish_native_graphs() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    h.next();
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::InsertTrackWithDevice {
        name: "sine".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().unwrap(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let utility = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Effect)
                .unwrap()
                .into_iter()
                .find(|entry| entry.name == "utility")
                .unwrap()
                .type_id,
        )
        .unwrap(),
    }));
    let Some(Created::Device(device)) = utility.created else {
        panic!("utility")
    };
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
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let enabled = h.run(Request::Edit(Command::SetTuning(super::sampler::tuning(
        440.0,
    ))));
    (applied, _) = h.applied(applied);
    let (start, original) = seek_output(&mut h, &enabled, &mut applied, 20);
    assert!(original.iter().flatten().any(|sample| sample.abs() > 0.01));
    let preset = files.0.join("double.freqs");
    hex_storage::tuning_library::export_file(
        &preset,
        "double",
        super::sampler::tuning(880.0).scale(),
    )
    .unwrap();
    let tuned = h.run(Request::TuningLibrary {
        project_generation: 0,
        revision: enabled.state.revision,
        operation_id: 1,
        operation: TuningLibraryOperation::Import { path: preset },
    });
    assert!(tuned.tuning_library.as_ref().unwrap().error.is_none());
    (applied, _) = h.applied(applied);
    let (next_start, loud) = seek_output(&mut h, &tuned, &mut applied, 21);
    assert_eq!(start, next_start);
    assert_ne!(original, loud);
    let effect_owner = |event: &Event| Owner {
        class: hex_engine::NativeDeviceClass::Effect,
        ..owner(event, device)
    };
    let switched = h.run(Request::CompareDevice {
        owner: effect_owner(&tuned),
        expected: Slot::A,
        action: Action::Switch,
    });
    assert!(switched.state.comparison_audio.as_ref().unwrap().follows(
        &tuned.state,
        &switched.state,
        0
    ));
    let quiet = switched
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    // Provider gain belongs to its A/B bank; the global host gain remains unchanged.
    let parameter =
        hex_engine::native_device_view_for_class(hex_engine::NativeDeviceClass::Effect, &quiet)
            .unwrap()
            .parameters
            .into_iter()
            .find(|parameter| {
                parameter.descriptor.label == "gain"
                    && parameter.descriptor.id != hex_engine::DEVICE_OUTPUT_GAIN.id
                    && parameter.descriptor.id != hex_engine::DEVICE_INPUT_GAIN.id
            })
            .unwrap()
            .descriptor
            .id;
    let quiet = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Effect,
        &quiet,
        parameter,
        hex_session::device_gesture::ParameterValue::Float(0.5),
    )
    .unwrap();
    let edited = h.run(Request::Edit(Command::ReplaceDeviceState {
        device,
        state: quiet,
    }));
    (applied, _) = h.applied(applied);
    let (_, quiet) = seek_output(&mut h, &edited, &mut applied, 22);
    for (actual, expected) in quiet.into_iter().flatten().zip(loud.into_iter().flatten()) {
        assert!((actual - expected * 0.5).abs() < 1e-6);
    }
    let restored = h.run(Request::CompareDevice {
        owner: effect_owner(&edited),
        expected: Slot::B,
        action: Action::Switch,
    });
    (applied, _) = h.applied(applied);
    assert_eq!(seek_output(&mut h, &restored, &mut applied, 23).1, loud);
    assert_eq!(restored.state.project.assets().count(), 0);
}
