use super::tests::{Directory, event, owner};
use super::*;
use crate::service::{
    CanonicalAudioSourceSnapshot, Event, Playback, Request, ResidentPublicationAcceptance,
    ResidentPublicationFailure, ResidentPublicationMetadata, ResidentPublicationPolicy,
    ResidentPublicationReadiness, ResidentPublicationRefusal, SamplerLoadSlot, SamplerLoadTarget,
    Service, WorkerAudioSink,
};
use hex_engine::native_device_resources::{
    DeviceResourceAdmission, PreparedDeviceResources, original_pcm_facts_on_control,
};
use hex_engine::{AudioStorageLedgerReservation, Frame};
use hex_lattice_contract::{DeviceAutomationTarget, ParameterId, ParameterValue};
use hex_project::{
    Command, Created, DeviceComparison, DeviceModulation, DeviceModulationRoute, DeviceVariant,
    ElementColor, ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator,
};
use hex_storage::preset::{DeviceClass, Preset, PresetContents};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[derive(Default)]
struct SinkState {
    refuse: AtomicBool,
    publications: AtomicUsize,
}

struct ResidentTestSink {
    state: Arc<SinkState>,
}

impl WorkerAudioSink for ResidentTestSink {
    fn initialize(&mut self, _shutdown: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }

    fn canonical_changed(&mut self, _source: Arc<CanonicalAudioSourceSnapshot>) {}

    fn poll(&mut self) {}

    fn close(&mut self) {}

    fn resident_publication_enabled(&self) -> bool {
        true
    }

    fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
        ResidentPublicationReadiness::NoOutput
    }

    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        [
            DeviceResourceAdmission::default(),
            DeviceResourceAdmission::default(),
            DeviceResourceAdmission::default(),
        ]
    }

    #[allow(clippy::result_large_err)]
    fn publish_resident(
        &mut self,
        playback: Playback,
        _resources: Option<&PreparedDeviceResources>,
        _observation: Arc<hex_engine::PreparedPlaybackObservation>,
        _metadata: ResidentPublicationMetadata,
        _policy: ResidentPublicationPolicy,
        _metadata_limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        if let Err(error) = admit(&[]) {
            return Err(ResidentPublicationRefusal {
                kind: ResidentPublicationFailure::Other,
                playback,
                error,
            });
        }
        if self.state.refuse.load(Ordering::Acquire) {
            return Err(ResidentPublicationRefusal {
                kind: ResidentPublicationFailure::Other,
                playback,
                error: "fixture sink refused resident publication".into(),
            });
        }
        self.state.publications.fetch_add(1, Ordering::AcqRel);
        Ok(ResidentPublicationAcceptance::OfflineRetained)
    }
}

fn resident_service(queue_capacity: usize) -> (Service, Arc<SinkState>) {
    let state = Arc::new(SinkState::default());
    let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let service = Service::with_audio_sink_and_native_reservation(
        queue_capacity,
        8192,
        None,
        Box::new(ResidentTestSink {
            state: Arc::clone(&state),
        }),
        crate::ResidentNativeReservation {
            storage: AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: bytes * 2,
                max_construction_peak_bytes: bytes * 3,
            },
            max_scan_frames: 8192,
        },
    )
    .unwrap();
    event(&service);
    (service, state)
}

fn write_wav(path: &Path, rate: u32, frames: &[Frame]) {
    let cancel = AtomicBool::new(false);
    let mut remaining = frames;
    hex_export::write_float_wav_new(path, rate, frames.len() as u64, &cancel, |block| {
        let (read, tail) = remaining.split_at(block.len());
        block.copy_from_slice(read);
        remaining = tail;
        Ok(())
    })
    .unwrap();
}

fn load_sampler_track(service: &Service, wav: &Path) -> (Event, hex_project::TrackId, DeviceId) {
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "sampler".into(),
        }))
        .unwrap();
    let added = event(service);
    assert!(added.error.is_none(), "{:?}", added.error);
    let track = added.state.project.tracks().last().unwrap().id();
    service
        .submit(Request::LoadFirstSampler {
            request_id: 1,
            project_generation: 0,
            revision: added.state.revision,
            target: SamplerLoadTarget::Track {
                track,
                slot: SamplerLoadSlot::New(hex_project::InstrumentLoadPlacement::First),
            },
            path: wav.to_owned(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let loaded = event(service);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    let root = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (loaded, track, root)
}

#[test]
fn two_project_media_unit_preset_and_default_roundtrip_remaps_identities_and_keeps_shared_source_runtime()
 {
    let dir = Directory::new();
    let app_data = dir.0.join("app-data");
    fs::create_dir(&app_data).unwrap();

    let frames_a = [[0.25, -0.25], [0.5, -0.5], [0.75, -0.125], [-0.25, 0.5]];
    let frames_b = [[-0.4, 0.2], [0.6, -0.3], [-0.1, 0.8], [0.2, -0.6]];
    let wav_a = dir.0.join("sample-a.wav");
    let wav_b = dir.0.join("sample-b.wav");
    write_wav(&wav_a, 8000, &frames_a);
    write_wav(&wav_b, 11025, &frames_b);
    let facts_a =
        original_pcm_facts_on_control(8000, &frames_a, 8192, &AtomicBool::new(false)).unwrap();
    let facts_b =
        original_pcm_facts_on_control(11025, &frames_b, 8192, &AtomicBool::new(false)).unwrap();

    // Project 1: author a media-backed Sampler with A/B comparison (two distinct originals)
    // and a macro modulator route on the root.
    let (mut project1, _sink1) = resident_service(32);
    project1
        .set_device_defaults_directory(app_data.clone())
        .unwrap();
    let (loaded_a, _track1, root1) = load_sampler_track(&project1, &wav_a);
    let asset_a_id = loaded_a
        .state
        .project
        .device(root1)
        .unwrap()
        .state()
        .asset_references()[0]
        .asset;
    // Load a second track with sample B so project 1 holds resident original B too.
    let (loaded_b, _track1_b, root1_b) = load_sampler_track(&project1, &wav_b);
    let state_a = loaded_b
        .state
        .project
        .device(root1)
        .unwrap()
        .state()
        .clone();
    let state_b = loaded_b
        .state
        .project
        .device(root1_b)
        .unwrap()
        .state()
        .clone();
    let inactive_b = state_b
        .with_input_gain_millidb(state_a.input_gain_millidb())
        .and_then(|s| s.with_output_gain_millidb(state_a.output_gain_millidb()))
        .unwrap();
    project1
        .submit(Request::Edit(Command::ReplaceDeviceComparison {
            device: root1,
            state: state_a.clone(),
            comparison: Some(DeviceComparison::new(DeviceVariant::A, inactive_b)),
        }))
        .unwrap();
    let compared_ev = event(&project1);
    assert!(compared_ev.error.is_none(), "{:?}", compared_ev.error);

    let next_id = compared_ev.state.project.next_identity();
    let mod_source1 = ModulationSourceId::new(next_id).unwrap();
    let mod_route1 = ModulationRouteId::new(next_id + 1).unwrap();
    project1
        .submit(Request::Edit(Command::ReplaceDeviceModulation {
            device: root1,
            modulation: DeviceModulation::new(
                vec![Modulator {
                    id: mod_source1,
                    color: ElementColor::Cyan,
                    enabled: true,
                    state: hex_engine::macro_modulator::Settings { value: 0.7 }
                        .encode()
                        .unwrap(),
                }],
                vec![DeviceModulationRoute {
                    id: mod_route1,
                    source: mod_source1,
                    output: ModulationOutputId::MAIN,
                    destination: ModulationDestination::Device {
                        device_type: state_a.type_id(),
                        target: DeviceAutomationTarget::Parameter(ParameterId::from_static(1)),
                        vst3_class: None,
                    },
                    amount: ModulationAmount::new(0.2).unwrap(),
                    range: ModulationRange::default(),
                    enabled: true,
                }],
            )
            .unwrap(),
        }))
        .unwrap();
    let authored1 = event(&project1);
    assert!(authored1.error.is_none(), "{:?}", authored1.error);

    let preset_path = dir.0.join("SamplerUnit.hex-device-preset.json");
    let owner1 = owner(&authored1, root1, NativeDeviceClass::Instrument);
    project1
        .submit(Request::SaveDeviceUnitPreset {
            owner: owner1,
            path: preset_path.clone(),
        })
        .unwrap();
    let saved_preset = event(&project1);
    assert!(saved_preset.error.is_none(), "{:?}", saved_preset.error);
    assert_eq!(saved_preset.state.project, authored1.state.project);

    project1
        .submit(Request::SaveDeviceUnitDefault {
            owner: owner1,
            root: app_data.clone(),
        })
        .unwrap();
    let saved_default = event(&project1);
    assert!(saved_default.error.is_none(), "{:?}", saved_default.error);

    // Confirm saved schema-3 unit carries both original manifests, then delete external WAVs
    // to prove Project 2 loads strictly from the app-owned OriginalStore.
    let PresetContents::Unit(saved_unit) =
        hex_storage::preset::load_contents(&preset_path).unwrap()
    else {
        panic!("expected schema-3 unit preset");
    };
    assert_eq!(saved_unit.nodes().len(), 1);
    assert_eq!(saved_unit.assets().len(), 2);
    assert!(saved_unit.assets().iter().any(|a| a.facts == facts_a));
    assert!(saved_unit.assets().iter().any(|a| a.facts == facts_b));
    fs::remove_file(&wav_a).unwrap();
    fs::remove_file(&wav_b).unwrap();

    // Project 2: separate service with offset identity space and its own audio sink.
    let (mut project2, sink2) = resident_service(32);
    project2
        .set_device_defaults_directory(app_data.clone())
        .unwrap();
    for index in 0..4 {
        project2
            .submit(Request::Edit(Command::AddMidiTrack {
                name: format!("offset-{index}"),
            }))
            .unwrap();
        assert!(event(&project2).error.is_none());
    }
    project2
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "target-a".into(),
        }))
        .unwrap();
    let track2_a_ev = event(&project2);
    assert!(track2_a_ev.error.is_none());
    let track2_a = track2_a_ev.state.project.tracks().last().unwrap().id();
    project2
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "target-b".into(),
        }))
        .unwrap();
    let track2_b_ev = event(&project2);
    assert!(track2_b_ev.error.is_none());
    let track2_b = track2_b_ev.state.project.tracks().last().unwrap().id();
    let before_insert_snapshot = track2_b_ev.state.project.snapshot();
    let pubs_before = sink2.publications.load(Ordering::Acquire);

    // 1. Fresh catalog insert uses saved media-backed unit default atomically.
    project2
        .submit(Request::AddNativeDevice {
            track: track2_a,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(DeviceTypeId::from_static("hex.instrument.sampler")),
            before: None,
        })
        .unwrap();
    let inserted = event(&project2);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    assert!(
        sink2.publications.load(Ordering::Acquire) > pubs_before,
        "fresh default unit must publish through resident audio sink"
    );
    let source2 = inserted
        .state
        .project
        .track(track2_a)
        .unwrap()
        .source_id()
        .unwrap();
    let root2 = inserted
        .state
        .project
        .native_source(source2)
        .unwrap()
        .instrument()
        .id();
    let root2_dev = inserted.state.project.device(root2).unwrap();
    let dest_active_asset = root2_dev.state().asset_references()[0].asset;
    let dest_inactive_asset = root2_dev
        .comparison()
        .unwrap()
        .inactive()
        .asset_references()[0]
        .asset;
    assert_ne!(dest_active_asset, asset_a_id);
    assert_ne!(dest_active_asset, dest_inactive_asset);
    assert_ne!(root2_dev.modulation().sources()[0].id, mod_source1);
    assert_ne!(root2_dev.modulation().routes()[0].id, mod_route1);
    assert_eq!(inserted.state.project.assets().count(), 2);
    assert!(
        inserted
            .state
            .project
            .find_portable_original(saved_unit.assets()[0])
            .is_some()
    );
    assert!(
        inserted
            .state
            .project
            .find_portable_original(saved_unit.assets()[1])
            .is_some()
    );

    // One-step Undo removes the entire default unit (root, modulation, registered originals).
    project2.submit(Request::Undo).unwrap();
    let undone_insert = event(&project2);
    assert!(undone_insert.error.is_none(), "{:?}", undone_insert.error);
    let mut expected_before = before_insert_snapshot;
    expected_before.next_id = undone_insert.state.project.snapshot().next_id;
    assert_eq!(undone_insert.state.project.snapshot(), expected_before);
    assert_eq!(undone_insert.state.project.assets().count(), 0);

    // One-step Redo restores the full default unit cleanly.
    project2.submit(Request::Redo).unwrap();
    let redone_insert = event(&project2);
    assert!(redone_insert.error.is_none(), "{:?}", redone_insert.error);
    assert_eq!(redone_insert.state.project.assets().count(), 2);

    // 2. Bind a second track to the same retained native source, clear its modulation,
    // then load the saved unit preset onto the shared source root.
    project2
        .submit(Request::Edit(Command::BindTrackSource {
            track: track2_b,
            source: Some(source2),
        }))
        .unwrap();
    let shared_ev = event(&project2);
    assert!(shared_ev.error.is_none(), "{:?}", shared_ev.error);
    project2
        .submit(Request::Edit(Command::ReplaceDeviceModulation {
            device: root2,
            modulation: DeviceModulation::default(),
        }))
        .unwrap();
    let stripped = event(&project2);
    assert!(stripped.error.is_none(), "{:?}", stripped.error);
    let assets_before_reload = stripped.state.project.assets().count();
    let before_reload_snapshot = stripped.state.project.snapshot();

    project2
        .submit(Request::LoadDevicePreset {
            owner: owner(&stripped, root2, NativeDeviceClass::Instrument),
            path: preset_path,
        })
        .unwrap();
    let reloaded = event(&project2);
    assert!(reloaded.error.is_none(), "{:?}", reloaded.error);
    // Shared native-source binding stays ONE source runtime with preserved SourceId and DeviceId.
    assert_eq!(
        reloaded.state.project.track(track2_a).unwrap().source_id(),
        Some(source2)
    );
    assert_eq!(
        reloaded.state.project.track(track2_b).unwrap().source_id(),
        Some(source2)
    );
    assert_eq!(reloaded.state.project.native_sources().len(), 1);
    assert_eq!(
        reloaded
            .state
            .project
            .native_source(source2)
            .unwrap()
            .instrument()
            .id(),
        root2
    );
    // Existing stamped destination originals were reused without duplicating assets.
    assert_eq!(
        reloaded.state.project.assets().count(),
        assets_before_reload
    );
    assert_eq!(
        reloaded
            .state
            .project
            .device(root2)
            .unwrap()
            .modulation()
            .sources()
            .len(),
        1
    );

    // One-step Undo / Redo on preset reload.
    project2.submit(Request::Undo).unwrap();
    let undone_reload = event(&project2);
    assert!(undone_reload.error.is_none(), "{:?}", undone_reload.error);
    let mut expected_stripped = before_reload_snapshot;
    expected_stripped.next_id = undone_reload.state.project.snapshot().next_id;
    assert_eq!(undone_reload.state.project.snapshot(), expected_stripped);
    project2.submit(Request::Redo).unwrap();
    let redone_reload = event(&project2);
    assert!(redone_reload.error.is_none(), "{:?}", redone_reload.error);
    assert_eq!(
        redone_reload
            .state
            .project
            .device(root2)
            .unwrap()
            .modulation()
            .sources()
            .len(),
        1
    );

    // 3. Nested instrument source unit (Operator + bypassed Utility child) across two projects:
    // shared native-source binding stays one source runtime and remaps child/modulator IDs atomically.
    let operator_type = DeviceTypeId::from_static("hex.instrument.operator");
    let utility_type = DeviceTypeId::from_static("hex.utility");
    project1
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "op-src".into(),
        }))
        .unwrap();
    let op_track_ev = event(&project1);
    let op_track1 = op_track_ev.state.project.tracks().last().unwrap().id();
    project1
        .submit(Request::AddNativeDevice {
            track: op_track1,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(operator_type),
            before: None,
        })
        .unwrap();
    let op_ev = event(&project1);
    assert!(op_ev.error.is_none(), "{:?}", op_ev.error);
    let op_root1 = op_ev
        .state
        .project
        .track(op_track1)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let utility_state =
        hex_engine::default_native_device_state_for_class(NativeDeviceClass::Effect, utility_type)
            .unwrap();
    project1
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent: op_root1,
            state: utility_state,
        }))
        .unwrap();
    let op_child_ev = event(&project1);
    assert!(op_child_ev.error.is_none(), "{:?}", op_child_ev.error);
    let Created::Device(op_child1) = op_child_ev.created.unwrap() else {
        panic!("expected operator nested child");
    };
    project1
        .submit(Request::Edit(Command::SetDeviceBypass {
            device: op_child1,
            bypassed: true,
        }))
        .unwrap();
    let op_authored = event(&project1);
    assert!(op_authored.error.is_none());
    let op_preset_path = dir.0.join("OperatorNested.hex-device-preset.json");
    project1
        .submit(Request::SaveDeviceUnitPreset {
            owner: owner(&op_authored, op_root1, NativeDeviceClass::Instrument),
            path: op_preset_path.clone(),
        })
        .unwrap();
    assert!(event(&project1).error.is_none());

    // Bind two tracks in project2 to one Operator source, then load the nested Operator preset.
    let op_t2a = redone_reload.state.project.tracks()[0].id();
    let op_t2b = redone_reload.state.project.tracks()[1].id();
    project2
        .submit(Request::AddNativeDevice {
            track: op_t2a,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(operator_type),
            before: None,
        })
        .unwrap();
    let op_added2 = event(&project2);
    assert!(op_added2.error.is_none(), "{:?}", op_added2.error);
    let op_source2 = op_added2
        .state
        .project
        .track(op_t2a)
        .unwrap()
        .source_id()
        .unwrap();
    let op_root2 = op_added2
        .state
        .project
        .native_source(op_source2)
        .unwrap()
        .instrument()
        .id();
    project2
        .submit(Request::Edit(Command::BindTrackSource {
            track: op_t2b,
            source: Some(op_source2),
        }))
        .unwrap();
    let op_shared2 = event(&project2);
    assert!(op_shared2.error.is_none(), "{:?}", op_shared2.error);
    let before_op_load = op_shared2.state.project.snapshot();
    project2
        .submit(Request::LoadDevicePreset {
            owner: owner(&op_shared2, op_root2, NativeDeviceClass::Instrument),
            path: op_preset_path,
        })
        .unwrap();
    let op_loaded2 = event(&project2);
    assert!(op_loaded2.error.is_none(), "{:?}", op_loaded2.error);
    assert_eq!(
        op_loaded2.state.project.track(op_t2a).unwrap().source_id(),
        Some(op_source2)
    );
    assert_eq!(
        op_loaded2.state.project.track(op_t2b).unwrap().source_id(),
        Some(op_source2)
    );
    let op_child2 = op_loaded2
        .state
        .project
        .devices()
        .find(|d| d.parent() == Some(op_root2))
        .expect("restored nested child on shared operator source");
    assert_ne!(op_child2.id(), op_child1);
    assert!(op_child2.bypassed());
    project2.submit(Request::Undo).unwrap();
    let op_undone2 = event(&project2);
    assert!(op_undone2.error.is_none());
    let mut expected_op_before = before_op_load;
    expected_op_before.next_id = op_undone2.state.project.snapshot().next_id;
    assert_eq!(op_undone2.state.project.snapshot(), expected_op_before);
}

#[test]
fn media_unit_failures_and_legacy_loads_retain_project_history_and_good_files() {
    let dir = Directory::new();
    let app_data = dir.0.join("app-data");
    fs::create_dir(&app_data).unwrap();
    let frames = [[0.25, -0.5], [0.5, -0.25], [0.125, 0.75]];
    let wav = dir.0.join("original.wav");
    write_wav(&wav, 8000, &frames);

    let (mut project1, _sink1) = resident_service(32);
    let (loaded, _track, root) = load_sampler_track(&project1, &wav);
    let target_owner = owner(&loaded, root, NativeDeviceClass::Instrument);
    let preset_path = dir.0.join("Sampler.hex-device-preset.json");

    // 1. Missing app data folder refuses media-backed preset save without creating a file.
    project1
        .submit(Request::SaveDeviceUnitPreset {
            owner: target_owner,
            path: preset_path.clone(),
        })
        .unwrap();
    let missing_root_err = event(&project1);
    assert!(
        missing_root_err
            .error
            .as_deref()
            .is_some_and(|e| e.contains("app data folder"))
    );
    assert!(!preset_path.exists());

    // Configure app data folder, save preset + default cleanly.
    project1
        .set_device_defaults_directory(app_data.clone())
        .unwrap();
    project1
        .submit(Request::SaveDeviceUnitPreset {
            owner: target_owner,
            path: preset_path.clone(),
        })
        .unwrap();
    assert!(event(&project1).error.is_none());
    project1
        .submit(Request::SaveDeviceUnitDefault {
            owner: target_owner,
            root: app_data.clone(),
        })
        .unwrap();
    assert!(event(&project1).error.is_none());

    // 2. Sink publication refusal in Project 2 rolls back editor + native_assets completely.
    let (mut project2, sink2) = resident_service(32);
    project2
        .set_device_defaults_directory(app_data.clone())
        .unwrap();
    project2
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "refused".into(),
        }))
        .unwrap();
    let ready2 = event(&project2);
    let track2 = ready2.state.project.tracks()[0].id();
    sink2.refuse.store(true, Ordering::Release);
    project2
        .submit(Request::AddNativeDevice {
            track: track2,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(DeviceTypeId::from_static("hex.instrument.sampler")),
            before: None,
        })
        .unwrap();
    let refused_pub = event(&project2);
    assert!(refused_pub.error.is_some());
    assert_eq!(refused_pub.state.project, ready2.state.project);
    assert_eq!(refused_pub.state.revision, ready2.state.revision);
    assert_eq!(refused_pub.state.project.assets().count(), 0);
    sink2.refuse.store(false, Ordering::Release);

    // 3. Corrupted original WAV in OriginalStore refuses cleanly without mutating project/history.
    let media_dir = app_data.join("device-preset-media").join("v1");
    let media_file = fs::read_dir(&media_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let good_wav_bytes = fs::read(&media_file).unwrap();
    let mut corrupt = good_wav_bytes.clone();
    if let Some(last) = corrupt.last_mut() {
        *last ^= 0xFF;
    }
    fs::write(&media_file, &corrupt).unwrap();
    project2
        .submit(Request::AddNativeDevice {
            track: track2,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(DeviceTypeId::from_static("hex.instrument.sampler")),
            before: None,
        })
        .unwrap();
    let corrupt_err = event(&project2);
    assert!(corrupt_err.error.is_some());
    assert_eq!(corrupt_err.state.project, ready2.state.project);
    assert_eq!(corrupt_err.state.revision, ready2.state.revision);
    fs::write(&media_file, &good_wav_bytes).unwrap();

    // 4. Existing malformed default on disk refuses SaveDeviceUnitDefault and preserves original bytes.
    let default_path = hex_storage::preset::defaults::path(
        &app_data,
        DeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.sampler"),
    )
    .unwrap();
    let bad_default = br#"{"format":"hex-device-preset","schema":99}"#.to_vec();
    fs::write(&default_path, &bad_default).unwrap();
    project1
        .submit(Request::SaveDeviceUnitDefault {
            owner: target_owner,
            root: app_data.clone(),
        })
        .unwrap();
    let bad_default_err = event(&project1);
    assert!(bad_default_err.error.is_some());
    assert_eq!(fs::read(&default_path).unwrap(), bad_default);

    // 5. Legacy schema-1 and schema-2 presets retain existing children, host gains, and bypass.
    let (legacy_service, _sink3) = resident_service(16);
    let utility_type = DeviceTypeId::from_static("hex.utility");
    let factory_utility =
        hex_engine::default_native_device_state_for_class(NativeDeviceClass::Effect, utility_type)
            .unwrap();
    legacy_service
        .submit(Request::Edit(Command::AddTrack {
            name: "legacy".into(),
        }))
        .unwrap();
    let t_ev = event(&legacy_service);
    let track = t_ev.state.project.tracks()[0].id();
    legacy_service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: factory_utility
                .clone()
                .with_input_gain_millidb(600)
                .unwrap()
                .with_output_gain_millidb(-1200)
                .unwrap(),
        }))
        .unwrap();
    let Created::Device(root_fx) = event(&legacy_service).created.unwrap() else {
        panic!("root fx");
    };
    legacy_service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device: root_fx,
            bypassed: true,
        }))
        .unwrap();
    assert!(event(&legacy_service).error.is_none());
    legacy_service
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent: root_fx,
            state: factory_utility.clone(),
        }))
        .unwrap();
    let child_ev = event(&legacy_service);
    let Created::Device(child_fx) = child_ev.created.unwrap() else {
        panic!("child fx");
    };

    let v1_path = dir.0.join("Schema1.hex-device-preset.json");
    let v1_json = serde_json::json!({
        "format": "hex-device-preset",
        "schema": 1,
        "class": "effect",
        "type_id": "hex.utility",
        "state_version": factory_utility.state_version(),
        "payload": factory_utility.payload(),
    });
    fs::write(&v1_path, serde_json::to_vec(&v1_json).unwrap()).unwrap();
    legacy_service
        .submit(Request::LoadDevicePreset {
            owner: owner(&child_ev, root_fx, NativeDeviceClass::Effect),
            path: v1_path,
        })
        .unwrap();
    let v1_loaded = event(&legacy_service);
    assert!(v1_loaded.error.is_none(), "{:?}", v1_loaded.error);
    let root_after_v1 = v1_loaded.state.project.device(root_fx).unwrap();
    assert_eq!(root_after_v1.state().input_gain_millidb(), 600);
    assert_eq!(root_after_v1.state().output_gain_millidb(), -1200);
    assert!(root_after_v1.bypassed());
    assert!(v1_loaded.state.project.device(child_fx).is_some());

    // Schema 2 with comparison preserves children, host trims, and bypass too.
    let v2_path = dir.0.join("Schema2.hex-device-preset.json");
    let alt_utility = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Effect,
        &factory_utility,
        ParameterId::from_static(1),
        ParameterValue::Float(0.25),
    )
    .unwrap();
    hex_storage::preset::save_new(
        &v2_path,
        &Preset {
            class: DeviceClass::Effect,
            state: alt_utility,
            comparison: Some(DeviceComparison::new(DeviceVariant::B, factory_utility)),
        },
    )
    .unwrap();
    legacy_service
        .submit(Request::LoadDevicePreset {
            owner: owner(&v1_loaded, root_fx, NativeDeviceClass::Effect),
            path: v2_path,
        })
        .unwrap();
    let v2_loaded = event(&legacy_service);
    assert!(v2_loaded.error.is_none(), "{:?}", v2_loaded.error);
    let root_after_v2 = v2_loaded.state.project.device(root_fx).unwrap();
    assert_eq!(root_after_v2.state().input_gain_millidb(), 600);
    assert_eq!(root_after_v2.state().output_gain_millidb(), -1200);
    assert!(root_after_v2.bypassed());
    assert!(root_after_v2.comparison().is_some());
    assert!(v2_loaded.state.project.device(child_fx).is_some());
}
