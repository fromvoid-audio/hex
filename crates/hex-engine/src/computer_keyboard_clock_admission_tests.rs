//! Preparation validates event domains without fabricating a performance clock.
use super::*;
use hex_lattice::{
    MutableInstrumentRuntime, NativeInstrumentClock, NativeInstrumentRuntimeError,
    PreparedMutableInstrumentRuntime, PreparedNativeInstrument,
};
use hex_lattice_contract::DeviceContractError;
use std::any::Any;

struct ClockRequiredInstrument;
#[derive(Clone, Default)]
struct ClockRequiredRuntime {
    active: bool,
}
impl PreparedNativeInstrument for ClockRequiredInstrument {
    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::InvalidParameterValue)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(Self)))
    }
}
impl PreparedMutableInstrumentRuntime for ClockRequiredInstrument {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<ClockRequiredRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        128
    }
    fn max_active_occurrences(&self) -> usize {
        1
    }
    fn max_release_tail_frames(&self) -> u64 {
        0
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(ClockRequiredRuntime::default())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl MutableInstrumentRuntime for ClockRequiredRuntime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        _: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let mut active = self.active;
        for event in events {
            match event {
                NativeInstrumentEvent::NoteOn { pitch, .. } if !active && *pitch <= 96 => {
                    active = true
                }
                NativeInstrumentEvent::NoteOff { .. } if active => active = false,
                _ => return Err(NativeInstrumentRuntimeError::InvalidEventBatch),
            }
        }
        Ok(())
    }
    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let clock = clock.ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
        clock
            .transport_is_running()
            .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
        for offset in 0..=frames {
            clock
                .beat_at_offset(offset)
                .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [Frame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for event in events {
            self.active = matches!(event, NativeInstrumentEvent::NoteOn { .. });
        }
        if self.active {
            for frame in frames {
                frame[0] += 0.25;
                frame[1] += 0.25;
            }
        }
    }
    fn reset(&mut self) {
        self.active = false;
    }
    fn is_quiescent(&self) -> bool {
        !self.active
    }
    fn can_copy_history_from(&self, old: &dyn MutableInstrumentRuntime) -> bool {
        old.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, old: &dyn MutableInstrumentRuntime) {
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            self.clone_from(old);
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct AuditionClock(Option<bool>);
impl NativeInstrumentClock for AuditionClock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        Some((3 << 32) + offset as u64)
    }
    fn transport_is_running(&self) -> Option<bool> {
        self.0
    }
}

#[test]
fn keyboard_preparation_accepts_clock_required_notes_but_render_still_requires_actual_clock() {
    let (_, track) = super::tests::fixture();
    let input = Arc::new(ComputerKeyboardInput::default());
    let mut keyboard = ComputerKeyboardRuntime::prepare_instrument(
        track,
        Arc::clone(&input),
        48_000,
        &ClockRequiredInstrument,
    )
    .unwrap();
    let velocity = COMPUTER_KEY_VELOCITIES
        .iter()
        .position(|v| *v == 80)
        .unwrap();
    assert!(keyboard.supported[velocity * 128 + 60]);
    assert!(
        !keyboard.supported[velocity * 128 + 127],
        "runtime event-domain validation remains authoritative"
    );
    assert!(keyboard.mutable.as_ref().unwrap().committed_is_quiescent());
    input.set_key(0, Some((60, 80, 1))).unwrap();
    for clock in [
        None,
        Some(&AuditionClock(None) as &dyn NativeInstrumentClock),
    ] {
        keyboard.begin_block();
        let mut pcm = [[1.0, -1.0]; 4];
        assert!(keyboard.render(&mut pcm, clock).is_err());
        assert_eq!(pcm, [[1.0, -1.0]; 4]);
        keyboard.rollback();
        assert!(keyboard.mutable.as_ref().unwrap().committed_is_quiescent());
        assert!(keyboard.primary.iter().all(Option::is_none));
    }
    // Stopped musical audition is authoritative even though transport is false.
    keyboard.begin_block();
    let mut pcm = [[1.0, -1.0]; 4];
    keyboard
        .render(&mut pcm, Some(&AuditionClock(Some(false))))
        .unwrap();
    keyboard.commit();
    assert_eq!(pcm, [[1.25, -0.75]; 4]);
    assert!(!keyboard.mutable.as_ref().unwrap().committed_is_quiescent());
}

#[test]
fn mutable_instrument_refuses_a_whole_bank_if_one_pitch_is_unsupported() {
    // The fixture runtime refuses pitches above 96 in its event preflight.
    let (_, track) = super::tests::fixture();
    let prepare = |notes: &[(u8, u8)]| {
        let input = ComputerKeyboardInput::default()
            .with_note_audition(notes, 64)
            .unwrap();
        ComputerKeyboardRuntime::prepare_instrument(
            track,
            Arc::new(input),
            48_000,
            &ClockRequiredInstrument,
        )
        .err()
    };
    assert_eq!(prepare(&[(60, 37)]), None);
    assert_eq!(
        prepare(&[(60, 37), (127, 93)]),
        Some(Invalid(
            "instrument cannot audition the complete selected chord"
        ))
    );
}

#[test]
fn unsupported_live_key_sets_rejected_and_later_keys_still_sound() {
    let (_, track) = super::tests::fixture();
    let input = Arc::new(ComputerKeyboardInput::default());
    let mut keyboard = ComputerKeyboardRuntime::prepare_instrument(
        track,
        Arc::clone(&input),
        48_000,
        &ClockRequiredInstrument,
    )
    .unwrap();
    let clock = AuditionClock(Some(true));
    input.set_key(0, Some((127, 80, 1))).unwrap();
    keyboard.begin_block();
    let mut pcm = [[1.0, -1.0]; 4];
    keyboard.render(&mut pcm, Some(&clock)).unwrap();
    keyboard.commit();
    assert!(input.rejected());
    assert_eq!(pcm, [[1.0, -1.0]; 4], "refused key adds no sound");
    input.set_key(0, Some((60, 80, 2))).unwrap();
    keyboard.begin_block();
    keyboard.render(&mut pcm, Some(&clock)).unwrap();
    keyboard.commit();
    assert_eq!(pcm, [[1.25, -0.75]; 4]);
}

#[test]
fn fm4_releases_held_key_before_rejecting_unsupported_replacement_and_recovers() {
    let (_, track) = super::tests::fixture();
    let registry = hex_devices::native_instrument_registry().unwrap();
    let state = registry
        .default_state(hex_devices::fm4::device::FM4_TYPE_ID)
        .unwrap();
    let tuning = hex_lattice::TuningTable::default();
    let instrument = registry
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &tuning,
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    let input = Arc::new(ComputerKeyboardInput::default());
    let mut keyboard = ComputerKeyboardRuntime::prepare_instrument(
        track,
        Arc::clone(&input),
        48000,
        instrument.as_ref(),
    )
    .unwrap();
    let clock = AuditionClock(Some(false));
    let mut render = |frames: usize| {
        let mut pcm = vec![[0.0; 2]; frames];
        keyboard.begin_block();
        keyboard.render(&mut pcm, Some(&clock)).unwrap();
        keyboard.commit();
        pcm
    };
    input.set_key(0, Some((60, 80, 1))).unwrap();
    assert!(render(256).iter().any(|frame| frame[0] != 0.0));
    // One physical snapshot releases the supported key and presses a rejected
    // high pitch. Rejection must not retain the previous key's held voice.
    input.set_key(0, Some((127, 80, 2))).unwrap();
    let tail = (0..20).flat_map(|_| render(256)).collect::<Vec<_>>();
    assert!(input.rejected());
    assert!(tail[..2400].iter().any(|frame| frame[0] != 0.0));
    assert_eq!(&tail[4800..], &[[0.0; 2]; 320]);
    input.set_key(0, Some((64, 80, 3))).unwrap();
    assert!(render(256).iter().any(|frame| frame[0] != 0.0));
}
