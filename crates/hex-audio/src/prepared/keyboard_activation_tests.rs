//! Exact output activation ordering through actual prepared callbacks, including foreign roots.
use super::*;
use hex_engine::computer_keyboard::ComputerKeyboardInput;
use hex_project::{Command, Created, Editor, Project};

fn graph(
    project: &Project,
    track: hex_project::TrackId,
    input: Option<&Arc<ComputerKeyboardInput>>,
) -> Playback {
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut graph = Playback::try_from_prepared_with_session(revision).unwrap();
    if let Some(input) = input {
        graph
            .prepare_computer_keyboard(project, track, Arc::clone(input))
            .unwrap();
    }
    graph
}
fn block(callback: &mut Callback) -> [Frame; 64] {
    let mut output = [[0.0; 2]; 64];
    let count = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            count.count_total,
            count.bytes_total,
            count.count_current,
            count.bytes_current
        ),
        (0, 0, 0, 0)
    );
    output
}

#[test]
fn ordered_hardware_notes_render_exact_velocity_without_callback_allocation_or_free() {
    use hex_engine::computer_keyboard::LiveNoteEvent;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "hardware".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let (mut control, mut callback) = channel(48_000).unwrap();
    let input = Arc::new(
        control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_live_notes()
            .unwrap(),
    );
    control
        .publish(graph(editor.project(), track, Some(&input)))
        .unwrap();
    assert_eq!(block(&mut callback), [[0.0; 2]; 64]);
    let live = input.live_notes().unwrap();
    live.publish(
        live.current_activation_on_control().unwrap(),
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch: 60,
            velocity: 37,
        },
    )
    .unwrap();
    live.publish(
        live.current_activation_on_control().unwrap(),
        LiveNoteEvent::NoteOff {
            channel: 2,
            pitch: 60,
            velocity: 81,
        },
    )
    .unwrap();
    let actual = block(&mut callback);
    let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
    let note = instrument.prepare_note(60, 37).unwrap();
    for (age, frame) in actual.into_iter().enumerate() {
        assert_eq!(
            frame,
            note.sample(age as u64, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        );
    }
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    live.cancel();
    block(&mut callback);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn stop_reconnect_then_physical_press_keeps_the_hardware_output_epoch_live() {
    use hex_engine::computer_keyboard::LiveNoteEvent;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let (mut control, mut callback) = channel(48_000).unwrap();
    let input = Arc::new(
        control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_live_notes()
            .unwrap(),
    );
    control
        .publish(graph(editor.project(), track, Some(&input)))
        .unwrap();
    block(&mut callback);
    control.stop();
    assert_eq!(block(&mut callback), [[0.0; 2]; 64]);
    let activation = input.begin_live_notes_activation_on_control().unwrap();
    // Physical resume after hardware reconnect must reuse this permitted epoch.
    input.ensure_output_activation_on_control().unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    let live = input.live_notes().unwrap();
    live.publish(
        activation,
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch: 64,
            velocity: 37,
        },
    )
    .unwrap();
    let mut expected = graph(editor.project(), track, None);
    let oracle_input = Arc::new(
        ComputerKeyboardInput::default()
            .with_note_audition(&[(60, 100), (64, 37)], 128)
            .unwrap(),
    );
    expected
        .prepare_computer_keyboard(editor.project(), track, Arc::clone(&oracle_input))
        .unwrap();
    oracle_input.trigger_note_audition().unwrap();
    let mut oracle = [[0.0; 2]; 64];
    expected.render(&mut oracle).unwrap();
    assert_eq!(block(&mut callback), oracle);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn mutable_hardware_short_note_callback_has_zero_total_and_net_allocations() {
    use hex_engine::computer_keyboard::LiveNoteEvent;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    let metadata = &hex_devices::operator_state::OPERATOR_METADATA;
    let state = metadata
        .patch_parameter(
            &metadata.default_state().unwrap(),
            ParameterId::from_static(78),
            ParameterValue::Float(0.4),
        )
        .unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "hardware".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state,
        })
        .unwrap()
    else {
        panic!("track")
    };
    let (mut control, mut callback) = channel(48_000).unwrap();
    let input = Arc::new(
        control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_live_notes()
            .unwrap(),
    );
    control
        .publish(graph(editor.project(), track, Some(&input)))
        .unwrap();
    assert_eq!(block(&mut callback), [[0.0; 2]; 64]);
    let activation = input.begin_live_notes_activation_on_control().unwrap();
    let live = input.live_notes().unwrap();
    live.publish(
        activation,
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch: 60,
            velocity: 37,
        },
    )
    .unwrap();
    live.publish(
        activation,
        LiveNoteEvent::NoteOff {
            channel: 2,
            pitch: 60,
            velocity: 0,
        },
    )
    .unwrap();
    let oracle_input = Arc::new(
        ComputerKeyboardInput::default()
            .with_note_audition(&[(60, 37)], 1)
            .unwrap(),
    );
    let mut oracle = graph(editor.project(), track, Some(&oracle_input));
    oracle_input.trigger_note_audition().unwrap();
    let mut expected = [[0.0; 2]; 64];
    for frame in &mut expected {
        oracle.render(std::slice::from_mut(frame)).unwrap();
    }
    assert_eq!(block(&mut callback), expected);
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
    // Terminal hardware cancellation remains an allocation-free callback path.
    let _ = live.cancel_activation(activation);
    block(&mut callback);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
#[test]
fn priority_stop_preserves_only_its_own_output_keyboard_epochs_without_callback_allocation_or_free()
{
    for same_output in [true, false] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (foreign, _foreign_callback) = channel(48_000).unwrap();
        let input = Arc::new(
            if same_output {
                control.new_computer_keyboard_input_on_control()
            } else {
                foreign.new_computer_keyboard_input_on_control()
            }
            .unwrap(),
        );
        input.set_key(0, Some((60, 100, 1))).unwrap();
        control
            .publish(graph(editor.project(), track, Some(&input)))
            .unwrap();
        let first = block(&mut callback);
        assert!(first.iter().flatten().any(|sample| sample.abs() > 1e-6));

        control.stop();
        input.reactivate_on_control().unwrap();
        input.set_key(0, Some((60, 100, 2))).unwrap();
        let fresh = block(&mut callback);
        if same_output {
            assert_eq!(
                fresh, first,
                "new physical epoch begins in the Stop callback"
            );
            assert!(!input.is_cancelled());
            control.stop();
            assert_eq!(
                block(&mut callback),
                [[0.0; 2]; 64],
                "held pre-Stop epoch is revoked"
            );
            input.reactivate_on_control().unwrap();
            input.set_key(0, Some((60, 100, 3))).unwrap();
            control.stop();
            input.reactivate_on_control().unwrap();
            input.set_key(0, Some((60, 100, 4))).unwrap();
            control.stop();
            assert_eq!(
                block(&mut callback),
                [[0.0; 2]; 64],
                "latest Stop revokes the entire reserved prefix"
            );
            input.reactivate_on_control().unwrap();
            control.command(PlaybackCommand::Stop).unwrap();
            assert_eq!(block(&mut callback), [[0.0; 2]; 64]);
            assert!(input.is_cancelled(), "queued explicit Stop stays terminal");
        } else {
            assert_eq!(
                fresh, [[0.0; 2]; 64],
                "foreign output clock cannot evade this output's Stop"
            );
            assert!(input.is_cancelled());
        }
        assert!(input.reactivate_on_control().is_err());
        control
            .publish(graph(editor.project(), track, None))
            .unwrap();
        assert_eq!(block(&mut callback), [[0.0; 2]; 64]);
        control.reclaim(); // final graph/input destruction remains on control
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

mod vst_audition_probe {
    use super::*;
    use hex_lattice::{
        DevicePreparationContext, ForeignProcessorNode, ForeignProcessors, NativeDeviceDefinition,
        NativeDeviceRegistry, PreparedForeignProcessor, PreparedProcessorFactory,
        ProcessorChainOwner, ProcessorInstanceIdentity, StereoMatrix,
    };
    use hex_lattice_contract::{
        DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
        ParameterDescriptor, ParameterId, ParameterValue,
    };

    const VST3: DeviceTypeId = DeviceTypeId::from_static("hex.vst3");
    struct Definition(ProcessorInstanceIdentity);
    impl NativeDeviceMetadata for Definition {
        fn type_id(&self) -> DeviceTypeId {
            VST3
        }
        fn state_version(&self) -> u32 {
            1
        }
        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            &[]
        }
        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(VST3, 1, vec![19])
        }
        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state != &self.default_state()? {
                return Err(DeviceContractError::MalformedState);
            }
            Ok(state.clone())
        }
        fn read_parameter(
            &self,
            _: &NativeDeviceState,
            _: ParameterId,
        ) -> Result<ParameterValue, DeviceContractError> {
            Err(DeviceContractError::UnknownParameter)
        }
        fn patch_parameter(
            &self,
            _: &NativeDeviceState,
            _: ParameterId,
            _: ParameterValue,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            Err(DeviceContractError::UnknownParameter)
        }
    }
    impl NativeDeviceDefinition for Definition {
        fn prepare_matrix(
            &self,
            state: &NativeDeviceState,
        ) -> Result<StereoMatrix, DeviceContractError> {
            self.migrate_state(state)?;
            Ok(StereoMatrix::identity())
        }
        fn prepare_instance_runtime_factory(
            &self,
            identity: ProcessorInstanceIdentity,
            state: &NativeDeviceState,
            context: &DevicePreparationContext<'_>,
        ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
            self.migrate_state(state)?;
            if identity != self.0 || context.sample_rate != 48_000 {
                return Err(DeviceContractError::InvalidId);
            }
            let processor = PreparedForeignProcessor::new(8, 3, 16)?
                .with_binding(hex_lattice_contract::ForeignProcessorBinding {
                    root: 1,
                    instance: identity.device_id(),
                    graph: 1,
                    epoch: 1,
                })?
                .with_instrument_profile();
            Ok(Some(PreparedProcessorFactory::Foreign(processor)))
        }
    }
    #[derive(Default)]
    struct Endpoint {
        on: usize,
        off: usize,
        rejected: bool,
    }
    impl ForeignProcessors for Endpoint {
        fn process_segment(
            &mut self,
            _: ForeignProcessorNode<'_>,
            _: usize,
            _: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            Err(DeviceContractError::ForeignProcessingFailed)
        }
        fn process_segment_with_notes(
            &mut self,
            _: ForeignProcessorNode<'_>,
            _: usize,
            _: &mut [Frame],
            _: &[hex_lattice_contract::ForeignParameterPoint],
            notes: &[hex_lattice::ForeignNote],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            for note in notes {
                if note.on {
                    self.on += 1;
                } else {
                    self.off += 1;
                }
            }
            Ok(())
        }
        fn reject_callback(&mut self) {
            self.rejected = true;
        }
    }

    #[test]
    fn foreign_audition_render_and_release_do_not_allocate_or_free() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "probe".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let state = NativeDeviceState::new(VST3, 1, vec![19]).unwrap();
        editor
            .execute(Command::InsertInstrument { track, state })
            .unwrap();
        let instrument = editor.project().track(track).unwrap().instrument().unwrap();
        let identity = ProcessorInstanceIdentity::new(
            ProcessorChainOwner::Track(track.value()),
            instrument.id().value(),
        )
        .unwrap();
        let definition = Definition(identity);
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare_with_effect_registry(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        );
        let input = Arc::new(
            ComputerKeyboardInput::default()
                .with_note_audition(&[(60, 37), (64, 93)], 8)
                .unwrap(),
        );
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback
            .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
            .unwrap();
        input.trigger_note_audition().unwrap();
        let mut output = [[0.0; 2]; 8];
        let mut endpoint = Endpoint::default();
        for _ in 0..2 {
            let count = allocation_counter::measure(|| {
                playback
                    .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
                    .unwrap();
            });
            assert_eq!(
                (
                    count.count_total,
                    count.bytes_total,
                    count.count_current,
                    count.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        assert_eq!((endpoint.on, endpoint.off), (2, 2));
        assert!(!endpoint.rejected);
        assert!(input.note_audition_finished());
    }
}
