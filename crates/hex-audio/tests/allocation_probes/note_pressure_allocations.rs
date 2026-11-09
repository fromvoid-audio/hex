//! A fixture provider gives each held occurrence a separate output channel.
use hex_lattice::{
    DeviceResources, EMPTY_DEVICE_RESOURCES, MutableInstrumentRuntime, NativeInstrumentEvent,
    NativeInstrumentRuntimeError as Error, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, PreparedMutableInstrumentRuntime, StereoFrame,
};
use std::{any::Any, num::NonZeroU64};

struct Factory {
    grant: bool,
    runtime_grant: bool,
}

impl PreparedMutableInstrumentRuntime for Factory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        8
    }
    fn max_active_occurrences(&self) -> usize {
        2
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
            voices: [None; 2],
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy)]
struct Runtime {
    grant: bool,
    voices: [Option<(NonZeroU64, u8)>; 2],
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
        self.voices = [None; 2];
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
    max_events_per_block: 8,
    max_active_occurrences: 2,
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
