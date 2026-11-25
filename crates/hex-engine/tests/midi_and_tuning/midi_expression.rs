use hex_devices::SineTable;
use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
use hex_lattice::{
    DevicePreparationContext, ForeignNote, ForeignNoteKind, ForeignProcessorNode,
    ForeignProcessors, NativeDeviceDefinition, NativeDeviceRegistry, PreparedForeignProcessor,
    PreparedProcessorFactory, ProcessorChainOwner, ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{
    BeatTime, Command, Created, Editor, MidiLaneKind, MidiLanePoint, NoteExpression, Project,
};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;

fn beat(quarters: u64) -> BeatTime {
    BeatTime::from_quarters(quarters).expect("bounded test beat")
}

fn render_native_instrument(project: &Project, frames: usize) -> Vec<[f32; 2]> {
    let revision = Arc::new(
        PreparedRevision::prepare(project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("prepare native instrument revision"),
    );
    let mut playback = SessionPlayback::new(revision).expect("session playback");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut out = vec![[0.0_f32; 2]; frames];
    for chunk in out.chunks_mut(128) {
        playback.render(chunk).expect("render native instrument");
    }
    out
}

fn peak_abs(samples: &[[f32; 2]]) -> f32 {
    samples
        .iter()
        .flat_map(|f| f.iter())
        .fold(0.0_f32, |acc, &s| acc.max(s.abs()))
}

fn max_diff(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    a.iter().zip(b.iter()).fold(0.0_f32, |acc, (x, y)| {
        acc.max((x[0] - y[0]).abs()).max((x[1] - y[1]).abs())
    })
}

#[test]
fn clip_cc_pitch_bend_and_per_note_mpe_reach_fm4_and_operator() {
    for type_name in ["hex.instrument.fm4", "hex.instrument.operator"] {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: format!("{type_name} expr"),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let instrument = editor
            .project()
            .track(track)
            .and_then(|t| t.instrument())
            .unwrap()
            .id();
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            DeviceTypeId::from_static(type_name),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument,
                state,
            })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: beat(4),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: beat(2),
                pitch: 60,
                channel: 0,
                velocity: 120,
            })
            .unwrap()
        else {
            panic!("note");
        };

        let baseline = render_native_instrument(editor.project(), 2_048);
        assert!(
            peak_abs(&baseline) > 1e-4,
            "{type_name} baseline must produce audible signal"
        );

        // 1. Per-note MPE pitch offset + timbre changes rendered pitch & waveform.
        editor
            .execute(Command::SetMidiNoteExpression {
                clip,
                note,
                expression: NoteExpression {
                    pitch_offset_cents: 500,
                    timbre: 110,
                    pressure: 90,
                },
                points: Vec::new(),
            })
            .unwrap();
        let with_mpe = render_native_instrument(editor.project(), 2_048);
        assert!(
            max_diff(&baseline, &with_mpe) > 1e-2,
            "{type_name} must respond audibly to per-note MPE expression"
        );

        // 2. Clip Pitch Bend + CC74 expression lanes further modulate the instrument.
        editor
            .execute(Command::SetMidiExpressionLane {
                clip,
                kind: MidiLaneKind::PitchBend,
                points: vec![MidiLanePoint {
                    beat: BeatTime::from_ratio(1, 64).unwrap(),
                    channel: 0,
                    value: 4096,
                }],
            })
            .unwrap();
        editor
            .execute(Command::SetMidiExpressionLane {
                clip,
                kind: MidiLaneKind::ControlChange(74),
                points: vec![MidiLanePoint {
                    beat: BeatTime::from_ratio(1, 64).unwrap(),
                    channel: 0,
                    value: 120,
                }],
            })
            .unwrap();
        let with_lanes = render_native_instrument(editor.project(), 2_048);
        assert!(
            max_diff(&with_mpe, &with_lanes) > 1e-2,
            "{type_name} must respond audibly to clip CC and pitch-bend lanes"
        );

        // 3. Undoing all three expression edits restores bit-identical baseline output.
        assert!(editor.undo());
        assert!(editor.undo());
        assert!(editor.undo());
        let restored = render_native_instrument(editor.project(), 2_048);
        assert_eq!(
            baseline, restored,
            "{type_name} undo must restore bit-identical output"
        );
    }
}

const VST3: DeviceTypeId = DeviceTypeId::from_static("hex.vst3");

struct RecordingVst3Definition(ProcessorInstanceIdentity);

impl NativeDeviceMetadata for RecordingVst3Definition {
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

impl NativeDeviceDefinition for RecordingVst3Definition {
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
        if identity != self.0 || context.sample_rate != RATE {
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
struct RecordingForeignEndpoint {
    received: Vec<ForeignNote>,
}

impl ForeignProcessors for RecordingForeignEndpoint {
    fn process_segment(
        &mut self,
        _: ForeignProcessorNode<'_>,
        _: usize,
        _: &mut [[f32; 2]],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingFailed)
    }
    fn process_segment_with_notes(
        &mut self,
        _: ForeignProcessorNode<'_>,
        _: usize,
        _: &mut [[f32; 2]],
        _: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[ForeignNote],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.received.extend_from_slice(notes);
        Ok(())
    }
    fn reject_callback(&mut self) {}
}

#[test]
fn clip_cc_pitch_bend_and_per_note_mpe_reach_hosted_vst3_instrument() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "vst3 expr".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let state = NativeDeviceState::new(VST3, 1, vec![19]).unwrap();
    editor
        .execute(Command::InsertInstrument { track, state })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("clip");
    };
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1),
            pitch: 64,
            channel: 2,
            velocity: 112,
        })
        .unwrap()
    else {
        panic!("note");
    };
    editor
        .execute(Command::SetMidiNoteExpression {
            clip,
            note,
            expression: NoteExpression {
                pitch_offset_cents: 700,
                timbre: 96,
                pressure: 80,
            },
            points: Vec::new(),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiExpressionLane {
            clip,
            kind: MidiLaneKind::ControlChange(1),
            points: vec![MidiLanePoint {
                beat: BeatTime::ZERO,
                channel: 2,
                value: 105,
            }],
        })
        .unwrap();
    editor
        .execute(Command::SetMidiExpressionLane {
            clip,
            kind: MidiLaneKind::PitchBend,
            points: vec![MidiLanePoint {
                beat: BeatTime::ZERO,
                channel: 2,
                value: 4096,
            }],
        })
        .unwrap();

    let instrument = editor.project().track(track).unwrap().instrument().unwrap();
    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(track.value()),
        instrument.id().value(),
    )
    .unwrap();
    let definition = RecordingVst3Definition(identity);
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();

    let revision = Arc::new(
        PreparedRevision::prepare_with_effect_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();

    let mut endpoint = RecordingForeignEndpoint::default();
    let mut out = [[0.0_f32; 2]; 16];
    playback
        .render_with_foreign_processors(&mut out, None, None, None, &mut endpoint)
        .unwrap();

    assert!(
        endpoint
            .received
            .iter()
            .any(|n| n.kind == ForeignNoteKind::On && n.channel == 2 && n.pitch == 64),
        "expected NoteOn, got {:?}",
        endpoint.received
    );
    assert!(
        endpoint
            .received
            .iter()
            .any(|n| n.kind == ForeignNoteKind::ControlChange && n.pitch == 74 && n.value == 96),
        "expected per-note CC74 timbre, got {:?}",
        endpoint.received
    );
    assert!(
        endpoint
            .received
            .iter()
            .any(|n| n.kind == ForeignNoteKind::PolyPressure && n.pitch == 64 && n.value == 80),
        "expected per-note PolyPressure, got {:?}",
        endpoint.received
    );
    assert!(
        endpoint
            .received
            .iter()
            .any(|n| n.kind == ForeignNoteKind::ControlChange && n.pitch == 1 && n.value == 105),
        "expected clip CC1 mod wheel, got {:?}",
        endpoint.received
    );
    let bend_count = endpoint
        .received
        .iter()
        .filter(|n| n.kind == ForeignNoteKind::PitchBend && n.channel == 2)
        .count();
    assert!(
        bend_count >= 2,
        "expected both per-note MPE pitch bend and clip pitch bend, got {bend_count} in {:?}",
        endpoint.received
    );
}
