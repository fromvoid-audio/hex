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
    let input = PluginStateCaptureInput {
        project,
        context: hex_engine::AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        },
        cancellation: &cancellation,
        flush_idle_editor_input: false,
        device_filter: None,
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
fn export_capture_projects_live_state_without_editing_the_project() {
    struct Sink {
        fail: bool,
    }
    impl WorkerAudioSink for Sink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn capture_plugin_states(
            &mut self,
            input: PluginStateCaptureInput<'_>,
        ) -> Result<Vec<PluginStateCapture>, String> {
            assert_eq!(input.context().editor_revision, 2);
            if self.fail {
                return Err("capture refused".into());
            }
            Ok(captures(input.project))
        }
    }
    let project = fixture();
    let original = project.clone();
    let cancellation = RequestCancellation::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        0,
    );
    let context = hex_engine::AudioSourceContext {
        project_generation: 1,
        editor_revision: 2,
    };
    let mut sink = Sink { fail: false };
    let projected =
        project_for_export_with_capture(&project, context, &cancellation, &mut sink).unwrap();
    assert_ne!(projected, original);
    assert_eq!(project, original);
    assert_eq!(
        projected,
        project_with_captures(original.clone(), captures(&original)).unwrap()
    );
    sink.fail = true;
    assert_eq!(
        project_for_export_with_capture(&project, context, &cancellation, &mut sink).unwrap_err(),
        "capture refused"
    );
    assert_eq!(project, original);
    let control = hex_export::ExportControl::default();
    let cancelled = cancellation.clone().with_bounce(&control);
    assert!(control.cancel());
    assert_eq!(
        project_for_export_with_capture(&project, context, &cancelled, &mut sink).unwrap_err(),
        "export cancelled"
    );
}

#[test]
fn save_projection_preserves_complete_model_comparison_and_host_trims() {
    let mut editor = Editor::new(fixture(), 8).unwrap();
    let track = editor.project().tracks()[1].id();
    editor
        .execute(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        })
        .unwrap();
    let original = editor.project();
    let saved = project_with_captures(original.clone(), captures(original)).unwrap();
    let reopened = hex_storage::decode(&hex_storage::encode(&saved).unwrap()).unwrap();
    let mut restored = reopened.snapshot();
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
    assert_eq!(&Project::from_snapshot(restored).unwrap(), original);
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
    let mut source_owned = captures(&project);
    let source = project.tracks()[1].source_id().unwrap();
    source_owned[1].owner = ProcessorChainOwner::Source(source.value());
    assert_eq!(
        project_with_captures(project.clone(), source_owned).unwrap_err(),
        "captured source effects are not supported"
    );
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
        fn capture_plugin_states(
            &mut self,
            _: PluginStateCaptureInput<'_>,
        ) -> Result<Vec<PluginStateCapture>, String> {
            if self.0.load(Ordering::Acquire) {
                Err("native capture output is closing".into())
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
    assert!(refused.save_complete && refused.state.unsaved);
    assert_eq!(
        refused.error.as_deref(),
        Some("vst3 state could not be captured · restart audio and retry save")
    );
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
    // A committed copy must consult the same capture owner even when the
    // identified built-in preview itself needs no vendor bytes.
    service
        .submit(Request::AddNativeDevice {
            track: authored.state.project.tracks()[0].id(),
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap()),
            before: None,
        })
        .unwrap();
    let added = event(&service);
    assert!(added.error.is_none(), "{:?}", added.error);
    let device = added.state.project.tracks()[0].devices()[0].id();
    let key = crate::device_gesture::GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::device_gesture(GestureRequest::Begin {
            key,
            target: crate::device_gesture::GestureTarget::DeviceParameter {
                device,
                parameter: hex_lattice_contract::ParameterId::new(1).unwrap(),
            },
        }))
        .unwrap();
    assert!(event(&service).error.is_none());
    service
        .submit(Request::device_gesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: hex_lattice_contract::ParameterValue::Float(2.0),
        }))
        .unwrap();
    let preview = event(&service);
    assert!(preview.active_device_gesture.is_some());
    let (reply, receipt) = std::sync::mpsc::sync_channel(1);
    service
        .submit(Request::SaveCommittedParameterCopy {
            path: new.clone(),
            revision: preview.state.revision,
            key,
            sequence: 1,
            reply,
        })
        .unwrap();
    let copy_refused = event(&service);
    assert_eq!(
        receipt.try_recv().unwrap().unwrap_err(),
        refused.error.unwrap()
    );
    assert_eq!(
        copy_refused.active_device_gesture,
        preview.active_device_gesture
    );
    assert!(copy_refused.state.unsaved && !copy_refused.save_complete);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!new.exists() && !folder.join("refused").exists());
    fail.store(false, Ordering::Release);
    service.submit(Request::SaveCurrent).unwrap();
    let saved = event(&service);
    assert!(saved.save_complete && saved.error.is_none() && !saved.state.unsaved);
    assert_ne!(std::fs::read(&path).unwrap(), before);
    drop(service);
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn rate_history_retains_the_exact_captured_vst3_state_on_both_sides() {
    use crate::Session;
    use crate::service::project_rate::history_needs_replacement;
    use hex_project::{SampleRateHistoryAction, SampleRateMode};

    let project = fixture();
    let captured = project_with_captures(project.clone(), captures(&project)).unwrap();
    assert_ne!(captured, project);

    for rate in [48_000, 96_000] {
        let mut session = Session::new(48_000, 8, 1_000_000).unwrap();
        session.editor = Editor::new(project.clone(), 8).unwrap();
        let change = || SampleRateHistoryAction::Apply {
            rate,
            mode: SampleRateMode::FollowInterface,
        };
        let prepared = |_: &Project| Ok::<(), hex_project::Invalid>(());

        let refused =
            session
                .editor
                .change_sample_rate_prepared(change(), Some(captured.clone()), |_| {
                    Err::<(), _>(hex_project::Invalid("fixture output refused"))
                });
        assert!(refused.is_err());
        assert_eq!(session.project(), &project);
        assert!(!session.editor.can_undo());

        session
            .editor
            .change_sample_rate_prepared(change(), Some(captured.clone()), prepared)
            .unwrap()
            .unwrap();
        let expected = captured
            .at_sample_rate(rate)
            .unwrap()
            .with_sample_rate_mode(SampleRateMode::FollowInterface);
        assert_eq!(session.project(), &expected);
        assert!(history_needs_replacement(&session, false));

        let captured_at_new_rate = project
            .at_sample_rate(rate)
            .unwrap()
            .with_sample_rate_mode(SampleRateMode::FollowInterface);
        session
            .editor
            .change_sample_rate_prepared(
                SampleRateHistoryAction::Undo,
                Some(captured_at_new_rate.clone()),
                prepared,
            )
            .unwrap()
            .unwrap();
        assert_eq!(session.project(), &captured);
        assert!(history_needs_replacement(&session, true));

        session
            .editor
            .change_sample_rate_prepared(
                SampleRateHistoryAction::Redo,
                Some(project.clone()),
                prepared,
            )
            .unwrap()
            .unwrap();
        assert_eq!(session.project(), &captured_at_new_rate);
        assert!(session.editor.undo());
        assert_eq!(session.project(), &project);
    }
}

#[test]
fn saved_reading_selection_keeps_raw_zero_and_excludes_base_and_host_targets() {
    use hex_lattice_contract::DeviceAutomationTarget;
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
        BeatTime,
    };
    let project = fixture();
    let device = project.main_devices()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    for target in [
        DeviceAutomationTarget::Vst3 { parameter: 0 },
        DeviceAutomationTarget::Vst3 { parameter: 7 },
        DeviceAutomationTarget::HostInputGain,
    ] {
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.5).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                }],
            })
            .unwrap();
    }
    editor
        .execute(Command::SetDeviceAutomationReadSelection {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: 7 },
            selection: AutomationReadSelection::Base,
        })
        .unwrap();
    let cancellation = RequestCancellation::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        0,
    );
    let input = PluginStateCaptureInput {
        project: editor.project(),
        context: hex_engine::AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        },
        cancellation: &cancellation,
        flush_idle_editor_input: false,
        device_filter: None,
    };
    assert_eq!(
        input
            .authored_plugin_targets(device.value())
            .collect::<Vec<_>>(),
        vec![(0, Some([1; 16]))]
    );
    assert_eq!(input.authored_plugin_targets(u64::MAX).count(), 0);
}
#[test]
fn bounce_capture_uses_the_live_receiver_and_projects_only_the_isolated_instrument() {
    struct Sink {
        owner: ProcessorChainOwner,
        device: u64,
        refuse: bool,
    }
    impl WorkerAudioSink for Sink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn capture_plugin_states(
            &mut self,
            input: PluginStateCaptureInput<'_>,
        ) -> Result<Vec<PluginStateCapture>, String> {
            let mut captured = Vec::new();
            input.visit_plugin_devices(|owner, id, expected| {
                assert_eq!((owner, id), (self.owner, self.device));
                captured.push(PluginStateCapture {
                    owner,
                    device_id: id,
                    expected: expected.clone(),
                    captured: state(b"preset", b"preset gui"),
                });
                Ok(())
            })?;
            assert_eq!(captured.len(), 1);
            if self.refuse {
                return Err("controlled vst3 state capture refusal".into());
            }
            Ok(captured)
        }
    }

    let mut editor = Editor::new(fixture(), 8).unwrap();
    let receiver = &editor.project().tracks()[1];
    let mut sink = Sink {
        owner: ProcessorChainOwner::Track(receiver.id().value()),
        device: receiver.instrument().unwrap().id().value(),
        refuse: false,
    };
    let source = receiver.source_id().unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor track");
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    let canonical = editor.project();
    let mut isolated = canonical.snapshot();
    isolated.tracks.retain(|track| track.instrument.is_some());
    isolated.main_devices.clear();
    isolated.return_tracks.clear();
    isolated.native_sources[0].receiver = Some(contributor.value());
    let isolated = Project::from_snapshot(isolated).unwrap();
    let input = ResidentEffectPreparation {
        project: &isolated,
        context: hex_engine::AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        },
        restoring_plugin_state: false,
        preparing_edit: false,
        power_on: None,
        running_power_off: None,
        hosted_move: None,
        reload_plugin_device: None,
    };
    let control = hex_export::ExportControl::default();
    let cancellation = RequestCancellation::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        0,
    )
    .with_bounce(&control);
    let projected = project_for_bounce_with_capture(canonical, &input, &cancellation, &mut sink)
        .unwrap()
        .unwrap();
    for track in projected.tracks() {
        assert_eq!(
            track.instrument().unwrap().state(),
            &state(b"preset", b"preset gui")
        );
    }
    assert_eq!(
        canonical
            .track(contributor)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &state(b"original", b"original gui")
    );

    sink.refuse = true;
    assert_eq!(
        project_for_bounce_with_capture(canonical, &input, &cancellation, &mut sink).unwrap_err(),
        "controlled vst3 state capture refusal"
    );
    assert!(control.cancel());
    assert_eq!(
        project_for_bounce_with_capture(canonical, &input, &cancellation, &mut sink).unwrap_err(),
        "bounce cancelled"
    );
}

#[test]
fn device_clipboard_capture_projects_live_vst3_state_across_all_owners_and_refuses_cleanly() {
    use hex_project::{DeviceChainOwner, DeviceSelectionIds};

    struct FilteredSink {
        expected_owner: ProcessorChainOwner,
        expected_device: u64,
        fail: bool,
        calls: usize,
    }
    impl WorkerAudioSink for FilteredSink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn capture_plugin_states(
            &mut self,
            input: PluginStateCaptureInput<'_>,
        ) -> Result<Vec<PluginStateCapture>, String> {
            self.calls += 1;
            if self.fail {
                return Err("controlled device capture refusal".into());
            }
            let mut result = Vec::new();
            input.visit_plugin_devices(|owner, id, expected| {
                assert_eq!((owner, id), (self.expected_owner, self.expected_device));
                result.push(PluginStateCapture {
                    owner,
                    device_id: id,
                    expected: expected.clone(),
                    captured: state(b"live patch", b"live gui")
                        .with_input_gain_millidb(0)
                        .unwrap()
                        .with_output_gain_millidb(0)
                        .unwrap(),
                });
                Ok(())
            })?;
            assert_eq!(result.len(), 1);
            Ok(result)
        }
    }

    let project = fixture();
    let original = project.clone();
    let track = project.tracks()[0].id();
    let track_effect = project.tracks()[0].devices()[0].id();
    let midi = project.tracks()[1].id();
    let midi_instrument = project.tracks()[1].instrument().unwrap().id();
    let bus = project.return_tracks()[0].id();
    let return_effect = project.return_tracks()[0].devices()[0].id();
    let main_effect = project.main_devices()[0].id();

    let cancellation = RequestCancellation::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        0,
    );
    let context = hex_engine::AudioSourceContext {
        project_generation: 1,
        editor_revision: 2,
    };

    for (owner, chain_owner, device_id) in [
        (
            DeviceChainOwner::Track(track),
            ProcessorChainOwner::Track(track.value()),
            track_effect,
        ),
        (
            DeviceChainOwner::Track(midi),
            ProcessorChainOwner::Track(midi.value()),
            midi_instrument,
        ),
        (
            DeviceChainOwner::Return(bus),
            ProcessorChainOwner::Return(bus.value()),
            return_effect,
        ),
        (
            DeviceChainOwner::Main,
            ProcessorChainOwner::Main,
            main_effect,
        ),
    ] {
        let selected = DeviceSelectionIds::new(&[device_id]).unwrap();
        let mut sink = FilteredSink {
            expected_owner: chain_owner,
            expected_device: device_id.value(),
            fail: false,
            calls: 0,
        };
        let clipboard = capture_devices_for_clipboard(
            &project,
            owner,
            &selected,
            context,
            &cancellation,
            Some(&mut sink),
            "device copy cancelled",
        )
        .unwrap();
        assert_eq!(sink.calls, 1);
        assert_eq!(project, original);

        // Paste the clipboard into a clean target editor and verify live bytes + trims.
        let mut target = Editor::new(project.clone(), 8).unwrap();
        let paste_owner = if device_id == midi_instrument {
            let Created::Track(dest_midi) = target
                .execute(Command::AddMidiTrack {
                    name: "dest".into(),
                })
                .unwrap()
            else {
                panic!("dest midi track");
            };
            DeviceChainOwner::Track(dest_midi)
        } else {
            owner
        };
        target
            .execute(Command::PasteDevices {
                clipboard: Arc::new(clipboard),
                owner: paste_owner,
                before: None,
            })
            .unwrap();
        let pasted_device = match paste_owner {
            DeviceChainOwner::Track(id) if device_id == midi_instrument => {
                target.project().track(id).unwrap().instrument().unwrap()
            }
            DeviceChainOwner::Track(id) => target
                .project()
                .track(id)
                .unwrap()
                .devices()
                .last()
                .unwrap(),
            DeviceChainOwner::Return(id) => target
                .project()
                .return_tracks()
                .iter()
                .find(|r| r.id() == id)
                .unwrap()
                .devices()
                .last()
                .unwrap(),
            DeviceChainOwner::Main => target.project().main_devices().last().unwrap(),
        };
        let decoded = hex_plugin_state::decode(pasted_device.state()).unwrap();
        assert_eq!(decoded.component, Some(&b"live patch"[..]));
        assert_eq!(decoded.controller, Some(&b"live gui"[..]));
        assert_eq!(pasted_device.state().input_gain_millidb(), -3000);
        assert_eq!(pasted_device.state().output_gain_millidb(), 2000);

        // Refusal propagates cleanly and preserves project.
        sink.fail = true;
        assert_eq!(
            capture_devices_for_clipboard(
                &project,
                owner,
                &selected,
                context,
                &cancellation,
                Some(&mut sink),
                "device cut cancelled",
            )
            .unwrap_err(),
            "controlled device capture refusal"
        );
        assert_eq!(project, original);
    }

    // Cancellation returns the caller's message before invoking the sink.
    let shutdown = Arc::new(AtomicBool::new(true));
    let cancelled =
        RequestCancellation::new(shutdown, Arc::new(std::sync::atomic::AtomicU64::new(0)), 0);
    let mut sink = FilteredSink {
        expected_owner: ProcessorChainOwner::Main,
        expected_device: main_effect.value(),
        fail: false,
        calls: 0,
    };
    assert_eq!(
        capture_devices_for_clipboard(
            &project,
            DeviceChainOwner::Main,
            &DeviceSelectionIds::new(&[main_effect]).unwrap(),
            context,
            &cancelled,
            Some(&mut sink),
            "device copy cancelled",
        )
        .unwrap_err(),
        "device copy cancelled"
    );
    assert_eq!(sink.calls, 0);
}

#[test]
fn clipboard_copy_cut_duplicate_preserve_live_vst3_state_and_keep_originals_on_capture_failure() {
    use hex_project::{DeviceChainOwner, DeviceSelectionIds, TrackSelectionIds};

    struct LiveStateSink {
        fail: Arc<AtomicBool>,
        next_audio_revision: u64,
        latest_submission: Option<ResidentRuntimeLoop>,
    }
    impl WorkerAudioSink for LiveStateSink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn resident_publication_enabled(&self) -> bool {
            true
        }
        fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
            match self.latest_submission {
                Some(submission) => ResidentPublicationReadiness::Applied {
                    submission,
                    publication_in_flight: false,
                },
                None => ResidentPublicationReadiness::Idle {
                    output_identity: 1,
                    applied_revision: 0,
                    publication_in_flight: false,
                },
            }
        }
        fn publish_resident(
            &mut self,
            _playback: Playback,
            _resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
            _observation: Arc<hex_engine::PreparedPlaybackObservation>,
            metadata: ResidentPublicationMetadata,
            _policy: ResidentPublicationPolicy,
            _metadata_limit: usize,
            _admit: &mut dyn FnMut(
                &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
            ) -> Result<(), String>,
        ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
            self.next_audio_revision += 1;
            let audio_revision = self.next_audio_revision;
            self.latest_submission = Some(ResidentRuntimeLoop {
                source_origin: ResidentSourceOrigin::Unknown,
                output_identity: 1,
                audio_revision,
                source_context: Some(metadata.context),
                section_loop: None,
            });
            Ok(ResidentPublicationAcceptance::Submitted {
                output_identity: 1,
                audio_revision,
            })
        }
        fn prepare_resident_effects(
            &mut self,
            _input: ResidentEffectPreparation<'_>,
            prepare: &mut dyn FnMut(
                &hex_engine::AudioCandidateEffects<'_, '_>,
            ) -> Result<PreparedRevision, hex_engine::Invalid>,
        ) -> Result<Option<PreparedRevision>, String> {
            hex_engine::AudioCandidateEffects::with_test_foreign_effect_program(
                ProcessorChainOwner::Main,
                u64::MAX,
                hex_lattice_contract::ForeignProcessorBinding {
                    root: 1,
                    instance: 1,
                    graph: 1,
                    epoch: 1,
                },
                std::num::NonZeroU64::new(1).unwrap(),
                0,
                prepare,
            )
            .map(Some)
            .map_err(|error| error.to_string())
        }
        fn capture_plugin_states(
            &mut self,
            input: PluginStateCaptureInput<'_>,
        ) -> Result<Vec<PluginStateCapture>, String> {
            if self.fail.load(Ordering::Acquire) {
                return Err("controlled live vst3 capture refusal".into());
            }
            let mut result = Vec::new();
            input.visit_plugin_devices(|owner, device_id, expected| {
                result.push(PluginStateCapture {
                    owner,
                    device_id,
                    expected: expected.clone(),
                    captured: state(b"live edited patch", b"live edited gui")
                        .with_input_gain_millidb(0)
                        .unwrap()
                        .with_output_gain_millidb(0)
                        .unwrap(),
                });
                Ok(())
            })?;
            Ok(result)
        }
    }

    fn next(service: &Service) -> Event {
        match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
            Ok(event) => event,
            Err(error) => panic!("worker event absent: {error}"),
        }
    }
    fn run(service: &Service, request: Request) -> Event {
        service.submit(request).unwrap();
        let event = next(service);
        assert!(event.error.is_none(), "unexpected error: {:?}", event.error);
        event
    }
    fn device_req(event: &Event, action: DeviceClipboardAction) -> Request {
        Request::DeviceClipboard {
            project_generation: 0,
            revision: event.state.revision,
            action,
        }
    }
    fn track_req(event: &Event, action: TrackClipboardAction) -> Request {
        Request::TrackClipboard {
            project_generation: 0,
            revision: event.state.revision,
            action,
        }
    }
    fn assert_live(device: &hex_project::Device) {
        let decoded = hex_plugin_state::decode(device.state()).unwrap();
        assert_eq!(decoded.component, Some(&b"live edited patch"[..]));
        assert_eq!(decoded.controller, Some(&b"live edited gui"[..]));
    }

    let fail = Arc::new(AtomicBool::new(false));
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        32,
        1_000_000,
        None,
        Box::new(LiveStateSink {
            fail: Arc::clone(&fail),
            next_audio_revision: 0,
            latest_submission: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    + hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    next(&service);

    let initial = state(b"stale original", b"stale gui");
    let e = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "audio".into(),
        }),
    );
    let audio_track = e.state.project.tracks()[0].id();
    let e = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "midi".into(),
        }),
    );
    let midi_track = e.state.project.tracks()[1].id();
    let e = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "midi dest".into(),
        }),
    );
    let midi_dest = e.state.project.tracks()[2].id();
    let e = run(
        &service,
        Request::Edit(Command::AddReturnTrack {
            name: "return".into(),
        }),
    );
    let return_track = e.state.project.return_tracks()[0].id();

    let e = run(
        &service,
        Request::Edit(Command::InsertDevice {
            track: audio_track,
            before: None,
            state: initial.clone(),
        }),
    );
    let track_effect = e.state.project.track(audio_track).unwrap().devices()[0].id();
    let e = run(
        &service,
        Request::Edit(Command::InsertInstrument {
            track: midi_track,
            state: initial.clone(),
        }),
    );
    let midi_instrument = e
        .state
        .project
        .track(midi_track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let e = run(
        &service,
        Request::Edit(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: initial.clone(),
        }),
    );
    let return_effect = e.state.project.return_tracks()[0].devices()[0].id();
    let e = run(
        &service,
        Request::Edit(Command::InsertMainDevice {
            before: None,
            state: initial.clone(),
        }),
    );
    let main_effect = e.state.project.main_devices()[0].id();

    // 1. DeviceClipboardAction::Copy -> Paste across Track, Return, Main, and Instrument.
    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Copy {
                owner: DeviceChainOwner::Track(audio_track),
                selected: DeviceSelectionIds::new(&[track_effect]).unwrap(),
            },
        ),
    );
    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Paste {
                owner: DeviceChainOwner::Track(audio_track),
                before: None,
            },
        ),
    );
    assert_live(&e.state.project.track(audio_track).unwrap().devices()[1]);

    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Copy {
                owner: DeviceChainOwner::Return(return_track),
                selected: DeviceSelectionIds::new(&[return_effect]).unwrap(),
            },
        ),
    );
    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Paste {
                owner: DeviceChainOwner::Return(return_track),
                before: None,
            },
        ),
    );
    assert_live(&e.state.project.return_tracks()[0].devices()[1]);

    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Copy {
                owner: DeviceChainOwner::Main,
                selected: DeviceSelectionIds::new(&[main_effect]).unwrap(),
            },
        ),
    );
    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Paste {
                owner: DeviceChainOwner::Main,
                before: None,
            },
        ),
    );
    assert_live(&e.state.project.main_devices()[1]);

    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Copy {
                owner: DeviceChainOwner::Track(midi_track),
                selected: DeviceSelectionIds::new(&[midi_instrument]).unwrap(),
            },
        ),
    );
    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Paste {
                owner: DeviceChainOwner::Track(midi_dest),
                before: None,
            },
        ),
    );
    assert_live(
        e.state
            .project
            .track(midi_dest)
            .unwrap()
            .instrument()
            .unwrap(),
    );

    // 2. DeviceClipboardAction::Duplicate captures live state.
    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Duplicate {
                source: DeviceChainOwner::Main,
                selected: DeviceSelectionIds::new(&[main_effect]).unwrap(),
                destination: DeviceChainOwner::Main,
                before: None,
            },
        ),
    );
    assert_live(&e.state.project.main_devices()[2]);

    // 3. DeviceClipboardAction::Cut failure preserves original device untouched.
    fail.store(true, Ordering::Release);
    service
        .submit(device_req(
            &e,
            DeviceClipboardAction::Cut {
                owner: DeviceChainOwner::Track(audio_track),
                selected: DeviceSelectionIds::new(&[track_effect]).unwrap(),
            },
        ))
        .unwrap();
    let refused_cut = next(&service);
    assert_eq!(
        refused_cut.error.as_deref(),
        Some("controlled live vst3 capture refusal")
    );
    assert_eq!(refused_cut.state.project, e.state.project);
    fail.store(false, Ordering::Release);

    // 4. DeviceClipboardAction::Cut -> Paste preserves live edited VST3 state.
    let e = run(
        &service,
        device_req(
            &refused_cut,
            DeviceClipboardAction::Cut {
                owner: DeviceChainOwner::Track(midi_track),
                selected: DeviceSelectionIds::new(&[midi_instrument]).unwrap(),
            },
        ),
    );
    assert!(
        e.state
            .project
            .track(midi_track)
            .unwrap()
            .instrument()
            .is_none()
    );
    let e = run(
        &service,
        device_req(
            &e,
            DeviceClipboardAction::Paste {
                owner: DeviceChainOwner::Track(midi_track),
                before: None,
            },
        ),
    );
    assert_live(
        e.state
            .project
            .track(midi_track)
            .unwrap()
            .instrument()
            .unwrap(),
    );

    // 5. TrackClipboardAction::Copy -> Paste captures live VST3 state (effect + instrument).
    let copy_tracks = TrackSelectionIds::new(&[audio_track, midi_track]).unwrap();
    let e = run(
        &service,
        track_req(&e, TrackClipboardAction::Copy(copy_tracks)),
    );
    let track_count_before_paste = e.state.project.tracks().len();
    let e = run(
        &service,
        track_req(&e, TrackClipboardAction::Paste { before: None }),
    );
    assert_eq!(e.state.project.tracks().len(), track_count_before_paste + 2);
    assert_live(&e.state.project.tracks()[track_count_before_paste].devices()[0]);
    assert_live(
        e.state.project.tracks()[track_count_before_paste + 1]
            .instrument()
            .unwrap(),
    );

    // 6. TrackClipboardAction::Cut failure preserves original track untouched.
    let cut_track = TrackSelectionIds::new(&[audio_track]).unwrap();
    fail.store(true, Ordering::Release);
    service
        .submit(track_req(&e, TrackClipboardAction::Cut(cut_track.clone())))
        .unwrap();
    let refused_track_cut = next(&service);
    assert_eq!(
        refused_track_cut.error.as_deref(),
        Some("controlled live vst3 capture refusal")
    );
    assert_eq!(refused_track_cut.state.project, e.state.project);
    fail.store(false, Ordering::Release);

    // 7. TrackClipboardAction::Cut -> Paste preserves live VST3 state.
    let count_before_cut = refused_track_cut.state.project.tracks().len();
    let e = run(
        &service,
        track_req(&refused_track_cut, TrackClipboardAction::Cut(cut_track)),
    );
    assert_eq!(e.state.project.tracks().len(), count_before_cut - 1);
    let e = run(
        &service,
        track_req(&e, TrackClipboardAction::Paste { before: None }),
    );
    assert_eq!(e.state.project.tracks().len(), count_before_cut);
    assert_live(&e.state.project.tracks().last().unwrap().devices()[0]);
}
