use super::*;

struct SaturatingDefinition {
    inner: MutableFeedbackDefinition,
}

struct SaturatingPrepared {
    inner: Box<dyn hex_lattice::PreparedNativeInstrument>,
}

struct SaturatingFactory {
    inner: Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>,
}

struct SaturatingRuntime {
    inner: Box<dyn hex_lattice::MutableInstrumentRuntime>,
}

impl hex_lattice_contract::NativeDeviceMetadata for SaturatingDefinition {
    fn type_id(&self) -> hex_lattice_contract::DeviceTypeId {
        self.inner.type_id()
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("shared nonlinear test instrument")
    }
    fn state_version(&self) -> u32 {
        self.inner.state_version()
    }
    fn descriptors(&self) -> &'static [hex_lattice_contract::ParameterDescriptor] {
        self.inner.descriptors()
    }
    fn default_state(
        &self,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        self.inner.default_state()
    }
    fn migrate_state(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        self.inner.migrate_state(state)
    }
    fn read_parameter(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        id: hex_lattice_contract::ParameterId,
    ) -> Result<hex_lattice_contract::ParameterValue, hex_lattice_contract::DeviceContractError>
    {
        self.inner.read_parameter(state, id)
    }
    fn patch_parameter(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        id: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        self.inner.patch_parameter(state, id, value)
    }
}

impl hex_lattice::NativeInstrumentDefinition for SaturatingDefinition {
    fn prepare_instrument(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<
        Box<dyn hex_lattice::PreparedNativeInstrument>,
        hex_lattice_contract::DeviceContractError,
    > {
        Ok(Box::new(SaturatingPrepared {
            inner: self.inner.prepare_instrument(state, context)?,
        }))
    }
}

impl hex_lattice::PreparedNativeInstrument for SaturatingPrepared {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.inner.algorithmic_latency_frames()
    }
    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        self.inner.prepare_note(pitch, velocity)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<
        Option<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>>,
        hex_lattice_contract::DeviceContractError,
    > {
        self.inner.prepare_mutable_runtime().map(|factory| {
            factory.map(|inner| {
                Box::new(SaturatingFactory { inner })
                    as Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>
            })
        })
    }
}

impl hex_lattice::PreparedMutableInstrumentRuntime for SaturatingFactory {
    fn supports_active_occurrence_ids(&self) -> bool {
        self.inner.supports_active_occurrence_ids()
    }
    fn supports_occurrence_gain(&self) -> bool {
        self.inner.supports_occurrence_gain()
    }
    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        self.inner.resource_factory_inline_bytes()
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.inner.algorithmic_latency_frames()
    }
    fn runtime_version(&self) -> u32 {
        self.inner.runtime_version()
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<SaturatingRuntime>() + self.inner.declared_twin_bytes()
    }
    fn max_events_per_block(&self) -> usize {
        self.inner.max_events_per_block()
    }
    fn max_active_occurrences(&self) -> usize {
        self.inner.max_active_occurrences()
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.inner.max_release_tail_frames()
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::MutableInstrumentRuntime> {
        Box::new(SaturatingRuntime {
            inner: self.inner.create_runtime(),
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl hex_lattice::MutableInstrumentRuntime for SaturatingRuntime {
    fn preflight_events(
        &self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frame_count: usize,
        resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        self.inner.preflight_events(events, frame_count, resources)
    }
    fn process_block(
        &mut self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frames: &mut [[f32; 2]],
        resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.inner.process_block(events, frames, resources);
        for frame in frames {
            frame[0] = frame[0].tanh();
            frame[1] = frame[1].tanh();
        }
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn is_quiescent(&self) -> bool {
        self.inner.is_quiescent()
    }
    fn can_copy_history_from(&self, previous: &dyn hex_lattice::MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.inner.can_copy_history_from(previous.inner.as_ref()))
    }
    fn copy_history_from(&mut self, previous: &dyn hex_lattice::MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.inner.copy_history_from(previous.inner.as_ref());
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.inner.fixed_storage_bytes()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Copy, Default)]
struct OracleVoice {
    gated: bool,
    release_left: u8,
}

struct NonlinearOracle {
    voices: [Option<OracleVoice>; 4],
    feedback: f32,
    tail_left: u8,
}

impl NonlinearOracle {
    fn note_on(&mut self, index: usize) {
        self.voices[index] = Some(OracleVoice {
            gated: true,
            release_left: 8,
        });
    }
    fn note_off(&mut self, index: usize) {
        if let Some(voice) = self.voices[index].as_mut() {
            voice.gated = false;
            voice.release_left = 8;
        }
    }
    fn next_sample(&mut self) -> [f32; 2] {
        let active = self.voices.iter().flatten().count();
        let direct = active as f32;
        self.feedback = self.feedback * 0.5 + direct * 0.25;
        let sample = (direct + self.feedback).tanh();
        for slot in &mut self.voices {
            let expired = if let Some(voice) = slot {
                if !voice.gated {
                    voice.release_left = voice.release_left.saturating_sub(1);
                    voice.release_left == 0
                } else {
                    false
                }
            } else {
                false
            };
            if expired {
                *slot = None;
            }
        }
        if active > 0 && self.voices.iter().all(Option::is_none) {
            self.tail_left = 8;
        }
        if self.voices.iter().all(Option::is_none) && self.tail_left > 0 {
            self.tail_left -= 1;
            if self.tail_left == 0 {
                self.feedback = 0.0;
            }
        }
        [sample; 2]
    }
}

fn shared_project() -> (Project, hex_project::TrackId, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(receiver) = editor
        .execute(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("receiver track")
    };
    let type_id =
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    editor
        .execute(Command::InsertInstrument {
            track: receiver,
            state: hex_lattice_contract::NativeDeviceState::new(type_id, 1, vec![0]).unwrap(),
        })
        .unwrap();
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor track")
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    for track in [receiver, contributor] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        else {
            panic!("arrangement MIDI clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(4).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    (editor.project().clone(), receiver, contributor)
}

fn render_and_compare(
    playback: &mut Playback,
    oracle: &mut NonlinearOracle,
    frames: usize,
    partitions: &[usize],
) {
    let mut actual = vec![[0.0; 2]; frames];
    let mut offset = 0;
    let mut partition = 0;
    while offset < frames {
        let count = partitions[partition % partitions.len()]
            .max(1)
            .min(frames - offset);
        playback
            .render(&mut actual[offset..offset + count])
            .unwrap();
        offset += count;
        partition += 1;
    }
    for (index, frame) in actual.into_iter().enumerate() {
        let expected = oracle.next_sample();
        assert!(
            (frame[0] - expected[0]).abs() < 0.000_01 && (frame[1] - expected[1]).abs() < 0.000_01,
            "frame {index}: actual={frame:?}, expected={expected:?}"
        );
    }
}

#[test]
fn shared_mutable_source_keeps_one_nonlinear_history_for_clips_and_live_input() {
    let (project, _receiver, contributor) = shared_project();
    let constructions = Arc::new(AtomicUsize::new(0));
    let definition = SaturatingDefinition {
        inner: MutableFeedbackDefinition {
            metadata: ContinuityInstrumentDefinition {
                id: hex_lattice_contract::DeviceTypeId::from_static(
                    "test.instrument.mutable.feedback",
                ),
                levels: [1.0, 1.0],
                release_frames: 8,
            },
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let prepared = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );

    MUTABLE_RUNTIME_CREATE_COUNTER.with(|counter| {
        *counter.borrow_mut() = Some(Arc::clone(&constructions));
    });
    let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
    assert_eq!(constructions.load(Ordering::Relaxed), 2, "one twin pair");
    let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 100, 1))).unwrap();
    playback
        .prepare_computer_keyboard(&project, contributor, Arc::clone(&input))
        .unwrap();
    assert_eq!(
        constructions.load(Ordering::Relaxed),
        2,
        "attaching live input to a contributor reuses the source pair"
    );

    playback.apply(PlaybackCommand::Play);
    let mut oracle = NonlinearOracle {
        voices: [None; 4],
        feedback: 0.0,
        tail_left: 0,
    };
    oracle.note_on(0);
    oracle.note_on(1);
    oracle.note_on(2);
    let partitions = [1, 7, 3, 19, 2, 11];
    render_and_compare(&mut playback, &mut oracle, 24, &partitions);

    input.set_key(0, None).unwrap();
    oracle.note_off(2);
    render_and_compare(&mut playback, &mut oracle, 8, &partitions);

    input.set_key(0, Some((60, 100, 2))).unwrap();
    oracle.note_on(3);
    render_and_compare(&mut playback, &mut oracle, 8, &partitions);

    playback.apply(PlaybackCommand::Pause);
    oracle.note_off(0);
    oracle.note_off(1);
    render_and_compare(&mut playback, &mut oracle, 16, &partitions);
    assert!(!playback.playing());

    input.set_key(0, None).unwrap();
    oracle.note_off(3);
    render_and_compare(&mut playback, &mut oracle, 40, &partitions);
    assert_eq!(constructions.load(Ordering::Relaxed), 2);
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|counter| *counter.borrow_mut() = None);
}

#[test]
fn retained_receiver_keeps_one_mutable_source_for_peer_and_refuses_own_keyboard() {
    let (ordinary, receiver, contributor) = shared_project();
    let mut snapshot = ordinary.snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let project = Project::from_snapshot(snapshot).unwrap();
    let definition = SaturatingDefinition {
        inner: MutableFeedbackDefinition {
            metadata: ContinuityInstrumentDefinition {
                id: hex_lattice_contract::DeviceTypeId::from_static(
                    "test.instrument.mutable.feedback",
                ),
                levels: [1.0, 1.0],
                release_frames: 8,
            },
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let prepared = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );
    let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
    let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
    assert!(
        playback
            .prepare_computer_keyboard(&project, receiver, Arc::clone(&input))
            .is_err()
    );
    input.set_key(0, Some((60, 100, 1))).unwrap();
    playback
        .prepare_computer_keyboard(&project, contributor, Arc::clone(&input))
        .unwrap();
    playback.apply(PlaybackCommand::Play);
    let mut oracle = NonlinearOracle {
        voices: [None; 4],
        feedback: 0.0,
        tail_left: 0,
    };
    oracle.note_on(0);
    oracle.note_on(1);
    render_and_compare(&mut playback, &mut oracle, 24, &[1, 7, 3, 19]);
}

#[test]
fn retained_role_change_refuses_live_handoff_without_touching_old_source() {
    let (ordinary, receiver, _) = shared_project();
    let mut snapshot = ordinary.snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let retained = Project::from_snapshot(snapshot).unwrap();
    let definition = SaturatingDefinition {
        inner: MutableFeedbackDefinition {
            metadata: ContinuityInstrumentDefinition {
                id: hex_lattice_contract::DeviceTypeId::from_static(
                    "test.instrument.mutable.feedback",
                ),
                levels: [1.0, 1.0],
                release_frames: 8,
            },
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let prepare = |project| {
        Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                project,
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        )
    };
    let mut old = SessionPlayback::new(prepare(&ordinary)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(&retained)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 8];
    old.render(&mut first).unwrap();
    assert!(matches!(
        candidate.inherit_state_live(&mut old, None),
        Err(SessionPlaybackError::Arrangement(
            "retained source path changes require stopped graph replacement"
        ))
    ));
    let mut next = [[0.0; 2]; 8];
    old.render(&mut next).unwrap();
    let mut oracle = NonlinearOracle {
        voices: [None; 4],
        feedback: 0.0,
        tail_left: 0,
    };
    oracle.note_on(0);
    oracle.note_on(1);
    for actual in first.into_iter().chain(next) {
        let expected = oracle.next_sample();
        assert!(
            (actual[0] - expected[0]).abs() < 0.000_01
                && (actual[1] - expected[1]).abs() < 0.000_01
        );
    }
}

#[test]
fn retained_receiver_with_no_peers_suppresses_own_mutable_midi() {
    let (ordinary, receiver, contributor) = shared_project();
    let mut editor = Editor::new(ordinary, 32).unwrap();
    editor.execute(Command::RemoveTrack(contributor)).unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let project = Project::from_snapshot(snapshot).unwrap();
    let definition = SaturatingDefinition {
        inner: MutableFeedbackDefinition {
            metadata: ContinuityInstrumentDefinition {
                id: hex_lattice_contract::DeviceTypeId::from_static(
                    "test.instrument.mutable.feedback",
                ),
                levels: [1.0, 1.0],
                release_frames: 8,
            },
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[1.0; 2]; 32];
    playback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 32]);
}
