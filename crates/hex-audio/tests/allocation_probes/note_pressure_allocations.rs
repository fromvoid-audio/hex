//! A fixture provider gives each held occurrence a separate output channel.
use hex_lattice::{
    DeviceResources, EMPTY_DEVICE_RESOURCES, MutableInstrumentRuntime, NativeInstrumentEvent,
    NativeInstrumentRuntimeError as Error, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, PreparedMutableInstrumentRuntime, StereoFrame,
};
use std::{any::Any, num::NonZeroU64};

#[derive(Clone, Copy)]
struct Factory {
    grant: bool,
    runtime_grant: bool,
}

impl PreparedMutableInstrumentRuntime for Factory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        12
    }
    fn max_active_occurrences(&self) -> usize {
        3
    }
    fn max_release_tail_frames(&self) -> u64 {
        0
    }
    fn supports_note_pressure(&self) -> bool {
        self.grant
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime {
            grant: self.runtime_grant,
            voices: [None; 3],
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy)]
struct Runtime {
    grant: bool,
    voices: [Option<(NonZeroU64, u8)>; 3],
}

impl Runtime {
    fn apply(&mut self, event: NativeInstrumentEvent) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                let id = NonZeroU64::new(occurrence_id).ok_or(Error::InvalidEventBatch)?;
                let slot = self
                    .voices
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .ok_or(Error::EventCapacityExceeded)?;
                *slot = Some((id, 0));
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(slot) = self
                    .voices
                    .iter_mut()
                    .find(|slot| slot.is_some_and(|(id, _)| id.get() == occurrence_id))
                {
                    *slot = None;
                }
            }
            NativeInstrumentEvent::NotePressure {
                occurrence_id,
                value,
                ..
            } => {
                let (_, pressure) = self
                    .voices
                    .iter_mut()
                    .flatten()
                    .find(|(id, _)| *id == occurrence_id)
                    .ok_or(Error::InvalidEventBatch)?;
                *pressure = value;
            }
            NativeInstrumentEvent::Expression { .. } => return Err(Error::InvalidEventBatch),
        }
        Ok(())
    }
}

impl MutableInstrumentRuntime for Runtime {
    fn supports_note_pressure(&self) -> bool {
        self.grant
    }

    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        _: usize,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        let mut proof = *self;
        for &event in events {
            proof.apply(event)?;
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: &dyn DeviceResources,
    ) {
        let mut events = events.iter().copied().peekable();
        for (offset, frame) in frames.iter_mut().enumerate() {
            while let Some(event) = events.next_if(|event| event.frame_offset() == offset) {
                self.apply(event).unwrap();
            }
            for (sample, voice) in frame.iter_mut().zip(self.voices) {
                if let Some((_, pressure)) = voice {
                    *sample += f32::from(pressure) / 127.0;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.voices = [None; 3];
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(Option::is_none)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| previous.grant == self.grant)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        self.voices = previous.as_any().downcast_ref::<Self>().unwrap().voices;
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

const LIMITS: NativeInstrumentRuntimeLimits = NativeInstrumentRuntimeLimits {
    max_pair_bytes: 1_024,
    max_events_per_block: 12,
    max_active_occurrences: 3,
    max_tail_frames: 0,
};

#[test]
fn note_pressure_targets_distinct_held_occurrences_and_retries_without_allocation() {
    let on = |id| NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: id,
        pitch: 60,
        velocity: 100,
    };
    let off = |offset, id| NativeInstrumentEvent::NoteOff {
        frame_offset: offset,
        occurrence_id: id,
    };
    let pressure = |offset, id, value| NativeInstrumentEvent::NotePressure {
        frame_offset: offset,
        occurrence_id: NonZeroU64::new(id).unwrap(),
        value,
    };
    let mismatch = Factory {
        grant: true,
        runtime_grant: false,
    };
    assert!(matches!(
        NativeInstrumentRuntimePair::new(&mismatch, LIMITS),
        Err(Error::IncompatibleTwins)
    ));
    let mut unsupported = NativeInstrumentRuntimePair::new(
        &Factory {
            grant: false,
            runtime_grant: false,
        },
        LIMITS,
    )
    .unwrap();
    assert!(!unsupported.supports_note_pressure());
    let mut pair = NativeInstrumentRuntimePair::new(
        &Factory {
            grant: true,
            runtime_grant: true,
        },
        LIMITS,
    )
    .unwrap();
    assert!(pair.supports_note_pressure());

    let events = [
        on(1),
        on(2),
        pressure(0, 1, 64),
        pressure(1, 2, 127),
        pressure(2, 1, 0),
        off(3, 2),
    ];
    let first = 64.0 / 127.0;
    let expected = [[first, 0.0], [first, 1.0], [0.0, 1.0], [0.0; 2]];
    let mut frames = [[0.0; 2]; 4];
    let counts = allocation_counter::measure(|| {
        unsupported.begin_block();
        assert_eq!(
            unsupported.process_block(&events, &mut frames, &EMPTY_DEVICE_RESOURCES),
            Err(Error::UnsupportedNotePressure)
        );
        assert_eq!(frames, [[0.0; 2]; 4]);
        unsupported.rollback_block();

        pair.begin_block();
        for rejected in [
            [pressure(0, 1, 127), on(1)],
            [on(1), pressure(0, 1, 128)],
            [on(1), pressure(4, 1, 127)],
        ] {
            assert_eq!(
                pair.process_block(&rejected, &mut frames, &EMPTY_DEVICE_RESOURCES),
                Err(Error::InvalidEventBatch)
            );
            assert_eq!(frames, [[0.0; 2]; 4]);
        }
        pair.process_block(&events, &mut frames, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        assert_eq!(frames, expected);
        pair.rollback_block();
        frames.fill([0.0; 2]);
        pair.begin_block();
        pair.process_block(&events, &mut frames, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        assert_eq!(frames, expected);
        pair.commit_block();

        frames.fill([0.0; 2]);
        pair.begin_block();
        for rejected in [
            [off(0, 1), pressure(0, 1, 127)],
            [pressure(0, 1, 127), pressure(0, 2, 127)],
        ] {
            assert_eq!(
                pair.process_block(&rejected, &mut frames, &EMPTY_DEVICE_RESOURCES),
                Err(Error::InvalidEventBatch)
            );
            assert_eq!(frames, [[0.0; 2]; 4]);
        }
        pair.process_block(&[pressure(0, 1, 127)], &mut frames, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        assert_eq!(frames, [[1.0, 0.0]; 4]);
        pair.rollback_block();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

mod live {
    use super::*;
    use hex_audio::prepared;
    use hex_engine::{
        PreparedRevision, SineTable,
        computer_keyboard::{ComputerKeyboardInput, LiveNoteEvent},
        playback::{Playback, PlaybackCommand},
    };
    use hex_lattice::{
        DevicePreparationContext, NativeInstrumentDefinition, NativeInstrumentRegistry,
        PreparedAnalyticNote, PreparedNativeInstrument,
    };
    use hex_lattice_contract::{
        DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
        ParameterDescriptor, ParameterId, ParameterValue,
    };
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};

    const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("test.instrument.live-pressure");

    struct Definition;
    impl NativeDeviceMetadata for Definition {
        fn type_id(&self) -> DeviceTypeId {
            TYPE_ID
        }
        fn display_name(&self) -> Option<&'static str> {
            Some("pressure fixture")
        }
        fn state_version(&self) -> u32 {
            1
        }
        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            &[]
        }
        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(TYPE_ID, 1, Vec::new())
        }
        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if *state == self.default_state()? {
                Ok(state.clone())
            } else {
                Err(DeviceContractError::MalformedState)
            }
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
    impl NativeInstrumentDefinition for Definition {
        fn prepare_instrument(
            &self,
            state: &NativeDeviceState,
            _: &DevicePreparationContext<'_>,
        ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
            self.migrate_state(state)?;
            Ok(Box::new(Factory {
                grant: true,
                runtime_grant: true,
            }))
        }
    }
    impl PreparedNativeInstrument for Factory {
        fn algorithmic_latency_frames(&self) -> Option<u32> {
            Some(0)
        }
        fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
            Err(DeviceContractError::UnsupportedProcessingClass)
        }
        fn prepare_mutable_runtime(
            &self,
        ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError>
        {
            Ok(Some(Box::new(*self)))
        }
    }

    #[test]
    fn live_pressure_uses_the_held_channel_key_and_releases_on_queue_fault() {
        let definition = Definition;
        let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
        let registry = NativeInstrumentRegistry::new(&definitions).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "pressure fixture".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: definition.default_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        // An earlier authored occurrence consumes a native ID without using
        // a keyboard token. The live PCM will fail if those IDs are conflated.
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(1).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw((1 << 32) / 12_000),
                pitch: 48,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare_with_test_instrument_registry(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        );
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        let input = Arc::new(ComputerKeyboardInput::default().with_live_notes().unwrap());
        let queue = input.live_notes().unwrap();
        let activation = queue.current_activation_on_control().unwrap();
        playback
            .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
            .unwrap();
        let (mut control, mut callback) = prepared::channel(48_000).unwrap();
        assert!(control.publish(playback).is_ok());

        control.command(PlaybackCommand::Play).unwrap();

        let on = |channel| LiveNoteEvent::NoteOn {
            channel,
            pitch: 60,
            velocity: 100,
        };
        let off = |channel| LiveNoteEvent::NoteOff {
            channel,
            pitch: 60,
            velocity: 0,
        };
        let pressure = |channel, value| LiveNoteEvent::PolyPressure {
            channel,
            pitch: 60,
            value,
        };
        let mut actual = [[0.0; 2]; 7];
        let mut render = |expected| {
            actual.fill([0.0; 2]);
            let counts = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut actual).unwrap();
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(actual, expected);
        };

        render([[0.0; 2]; 7]);
        for event in [on(0), on(1), pressure(0, 64), pressure(1, 127)] {
            queue.publish(activation, event).unwrap();
        }
        let half = 64.0 / 127.0;
        render([
            [0.0; 2],
            [0.0; 2],
            [half, 0.0],
            [half, 1.0],
            [half, 1.0],
            [half, 1.0],
            [half, 1.0],
        ]);
        // The late packet addresses an absent note. Reusing the key does not
        // inherit its previous pressure, and another channel stays independent.
        for event in [off(0), pressure(0, 127), on(0), pressure(0, 64)] {
            queue.publish(activation, event).unwrap();
        }
        render([
            [0.0, 1.0],
            [0.0, 1.0],
            [0.0, 1.0],
            [half, 1.0],
            [half, 1.0],
            [half, 1.0],
            [half, 1.0],
        ]);
        for _ in 0..256 {
            queue.publish(activation, pressure(1, 12)).unwrap();
        }
        assert_eq!(
            queue.publish(activation, pressure(1, 12)),
            Err(hex_engine::computer_keyboard::LiveNotePublishError::Full)
        );
        render([[0.0; 2]; 7]);
        let replacement = input.begin_live_notes_activation_on_control().unwrap();
        assert!(queue.publish(activation, pressure(1, 127)).is_err());
        queue.publish(replacement, on(1)).unwrap();
        queue.publish(replacement, pressure(1, 64)).unwrap();
        render([
            [0.0; 2],
            [half, 0.0],
            [half, 0.0],
            [half, 0.0],
            [half, 0.0],
            [half, 0.0],
            [half, 0.0],
        ]);
        control.command(PlaybackCommand::Stop).unwrap();
        // The ordinary 48 kHz Stop declick ends after 128 frames.
        for start in (0_usize..133).step_by(7) {
            render(std::array::from_fn(|offset| {
                let left = 127_usize.saturating_sub(start + offset);
                [half * (left as f32 / 128.0), 0.0]
            }));
        }
        render([[0.0; 2]; 7]);
        control.request_shutdown();
        render([[0.0; 2]; 7]);
        assert!(control.reclaim() > 0);
    }
}
