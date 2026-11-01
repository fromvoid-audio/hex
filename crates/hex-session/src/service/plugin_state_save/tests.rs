//! Real model/codec/file projection; SDK ownership is tested by native callers.
use super::*;
use hex_project::{Command, Created, DeviceComparison, DeviceVariant, Editor};

fn state(component: &[u8], controller: &[u8]) -> NativeDeviceState {
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [2; 32],
                info_sha256: [3; 32],
            },
        },
        component: Some(component),
        controller: Some(controller),
    })
    .unwrap()
    .with_input_gain_millidb(-3000)
    .unwrap()
    .with_output_gain_millidb(2000)
    .unwrap()
}
fn fixture() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Track(midi) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("midi");
    };
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return");
    };
    let original = state(b"original", b"original gui");
    for command in [
        Command::InsertDevice {
            track,
            before: None,
            state: original.clone(),
        },
        Command::InsertInstrument {
            track: midi,
            state: original.clone(),
        },
        Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: original.clone(),
        },
        Command::InsertMainDevice {
            before: None,
            state: original.clone(),
        },
    ] {
        let Created::Device(device) = editor.execute(command).unwrap() else {
            panic!("device");
        };
        editor
            .execute(Command::ReplaceDeviceComparison {
                device,
                state: original.clone(),
                comparison: Some(DeviceComparison::new(
                    DeviceVariant::B,
                    state(b"inactive", b"inactive gui"),
                )),
            })
            .unwrap();
    }
    editor.project().clone()
}
fn captures(project: &Project) -> Vec<PluginStateCapture> {
    let cancellation = RequestCancellation::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        0,
    );
    let input = PluginStateSaveInput {
        project,
        context: hex_engine::AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        },
        cancellation: &cancellation,
    };
    let mut result = Vec::new();
    input
        .visit_plugin_devices(|owner, device, state| {
            result.push(PluginStateCapture {
                owner,
                device_id: device,
                expected: state.clone(),
                captured: super::tests::state(b"current", b"current gui")
                    .with_input_gain_millidb(0)
                    .unwrap()
                    .with_output_gain_millidb(0)
                    .unwrap(),
            });
            Ok(())
        })
        .unwrap();
    result
}

#[test]
fn save_projection_preserves_complete_model_comparison_and_host_trims() {
    let original = fixture();
    let saved = project_with_captures(original.clone(), captures(&original)).unwrap();
    let mut restored = saved.snapshot();
    let mut count = 0;
    for device in restored
        .tracks
        .iter_mut()
        .flat_map(|t| t.devices.iter_mut().chain(t.instrument.iter_mut()))
        .chain(
            restored
                .return_tracks
                .iter_mut()
                .flat_map(|t| &mut t.devices),
        )
        .chain(restored.main_devices.iter_mut())
    {
        let decoded = hex_plugin_state::decode(&device.state).unwrap();
        assert_eq!(decoded.component, Some(&b"current"[..]));
        assert_eq!(decoded.controller, Some(&b"current gui"[..]));
        assert_eq!(
            (
                device.state.input_gain_millidb(),
                device.state.output_gain_millidb()
            ),
            (-3000, 2000)
        );
        assert_eq!(
            device.comparison.as_ref().unwrap().active(),
            DeviceVariant::B
        );
        assert_eq!(
            device.comparison.as_ref().unwrap().inactive(),
            &state(b"inactive", b"inactive gui")
        );
        device.state = state(b"original", b"original gui");
        count += 1;
    }
    assert_eq!(count, 4);
    // The current-format registry owns the bound instrument state. Its track
    // projection alone cannot qualify a complete saved project.
    assert_eq!(restored.native_sources.len(), 1);
    let retained = &mut restored.native_sources[0].devices[0];
    let decoded = hex_plugin_state::decode(&retained.state).unwrap();
    assert_eq!(decoded.component, Some(&b"current"[..]));
    assert_eq!(decoded.controller, Some(&b"current gui"[..]));
    retained.state = state(b"original", b"original gui");
    assert_eq!(Project::from_snapshot(restored).unwrap(), original);
}

#[test]
fn save_projection_refuses_partial_stale_duplicate_malformed_and_wrong_binary() {
    let project = fixture();
    for fault in 0..6 {
        let mut states = captures(&project);
        match fault {
            0 => states[1].device_id = u64::MAX,
            1 => states[1].owner = ProcessorChainOwner::Main,
            2 => states[1].expected = state(b"stale", b"old gui"),
            3 => states[1].device_id = states[0].device_id,
            4 => {
                states[1].captured =
                    NativeDeviceState::new(hex_plugin_state::TYPE_ID, 1, vec![0]).unwrap()
            }
            _ => {
                let mut bytes = states[1].captured.payload().to_vec();
                bytes[24] ^= 1;
                states[1].captured =
                    NativeDeviceState::new(hex_plugin_state::TYPE_ID, 1, bytes).unwrap();
            }
        }
        assert!(
            project_with_captures(project.clone(), states).is_err(),
            "fault {fault}"
        );
    }
    assert_eq!(
        project_with_captures(project.clone(), Vec::new()).unwrap(),
        project
    );
}

#[test]
fn save_projection_retains_unavailable_opaque_state_without_decode_or_defaults() {
    let project = fixture();
    let mut snapshot = project.snapshot();
    snapshot.main_devices[0].state =
        NativeDeviceState::new(hex_plugin_state::TYPE_ID, 99, vec![0xff, 0, 7])
            .unwrap()
            .with_input_gain_millidb(-3000)
            .unwrap()
            .with_output_gain_millidb(2000)
            .unwrap();
    let unavailable = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        project_with_captures(unavailable.clone(), Vec::new()).unwrap(),
        unavailable
    );
}

#[test]
fn save_capture_refusal_preserves_existing_file_dirty_state_and_new_destination() {
    struct Sink(Arc<AtomicBool>);
    impl WorkerAudioSink for Sink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn capture_plugin_states_for_save(
            &mut self,
            _: PluginStateSaveInput<'_>,
        ) -> Result<Vec<PluginStateCapture>, String> {
            if self.0.load(Ordering::Acquire) {
                Err("controlled native capture refusal".into())
            } else {
                Ok(Vec::new())
            }
        }
    }
    fn event(service: &Service) -> Event {
        // Blocks on the event channel; bounded only against hangs.
        match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
            Ok(event) => event,
            Err(error) => panic!("event: worker event absent: {error}"),
        }
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let folder = std::env::temp_dir().join(format!(
        "hex-save-capture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&folder).unwrap();
    let fail = Arc::new(AtomicBool::new(false));
    let service =
        Service::with_audio_sink(16, 1_000_000, None, Box::new(Sink(Arc::clone(&fail)))).unwrap();
    event(&service);
    let path = folder.join("existing.hex");
    service.submit(Request::SaveNew(path.clone())).unwrap();
    assert!(event(&service).save_complete);
    let before = std::fs::read(&path).unwrap();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "authored".into(),
        }))
        .unwrap();
    let authored = event(&service);
    fail.store(true, Ordering::Release);
    service.submit(Request::SaveCurrent).unwrap();
    let refused = event(&service);
    assert!(refused.save_complete && refused.error.is_some() && refused.state.unsaved);
    assert_eq!(refused.state.project, authored.state.project);
    assert_eq!(
        (refused.state.can_undo, refused.state.can_redo),
        (authored.state.can_undo, authored.state.can_redo)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let new = folder.join("refused.hex");
    service.submit(Request::SaveNew(new.clone())).unwrap();
    assert!(event(&service).error.is_some());
    assert!(!new.exists());
    fail.store(false, Ordering::Release);
    service.submit(Request::SaveCurrent).unwrap();
    let saved = event(&service);
    assert!(saved.save_complete && saved.error.is_none() && !saved.state.unsaved);
    assert_ne!(std::fs::read(&path).unwrap(), before);
    drop(service);
    std::fs::remove_dir_all(folder).unwrap();
}
