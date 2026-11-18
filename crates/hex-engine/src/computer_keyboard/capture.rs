//! Runtime-local accepted transitions. Staging has no delivery authority.
use super::*;
use crate::midi_capture::*;

impl ComputerKeyboardRuntime {
    pub(crate) fn midi_capture_active(&self) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|capture| capture.is_active())
    }
    pub(crate) fn validate_capture_input(
        &self,
        binding: MidiCaptureBinding,
        clock: Option<&KeyboardActivationClock>,
    ) -> bool {
        let Some(input) = self.input.lease() else {
            return false;
        };
        let Some(output) = input.output_activation.as_ref() else {
            return false;
        };
        if input.is_cancelled()
            || self.lease_detached
            || self.faulted
            || input.is_note_audition()
            || input.native_activation.is_some()
            || self.track != binding.track
            || self.instrument_id != Some(binding.device)
            || output.epoch.load(Acquire) != binding.output_epoch
            || !output.clock.permits(binding.output_epoch)
            || clock.is_some_and(|clock| !input.bound_to_clock(clock))
        {
            return false;
        }
        match binding.hardware {
            Some(token) => {
                input
                    .live_notes
                    .as_ref()
                    .is_some_and(|live| live.same_current_activation(token))
                    && input.live_output_epoch.load(Acquire) == binding.output_epoch
            }
            None => input
                .live_notes
                .as_ref()
                .is_none_or(|live| live.is_cancelled()),
        }
    }
    pub(crate) fn arm_capture(
        &mut self,
        binding: MidiCaptureBinding,
        clock: &KeyboardActivationClock,
        route: usize,
        latency_frames: u64,
    ) -> Result<(), Invalid> {
        if !self.validate_capture_input(binding, Some(clock))
            || self.midi_capture_active()
            || self.capture.is_none()
        {
            return Err(Invalid("midi capture input is unavailable or stale"));
        }
        let queue = self
            .input
            .lease()
            .and_then(|input| input.ordered_keys.as_ref())
            .ok_or(Invalid("midi recording input was not prepared"))?;
        queue.accept(binding.keys, binding.output_epoch)?;
        let capture = self
            .capture
            .as_mut()
            .ok_or(Invalid("midi capture storage is unavailable"))?;
        capture.invalidate();
        capture.binding = Some(binding);
        capture.active = true;
        capture.faulted = None;
        capture.route = route;
        capture.first_occurrence = self.next_id;
        capture.latency_frames = latency_frames;
        Ok(())
    }
    pub(crate) fn capture_finish_fault(&self, take_id: u64) -> Option<MidiCaptureFault> {
        let capture = self.capture.as_ref().filter(|capture| {
            capture
                .binding
                .is_some_and(|binding| binding.take_id == take_id)
        })?;
        if let Some(fault) = capture.faulted {
            return Some(fault);
        }
        let binding = capture.binding?;
        let input = self.input.lease()?;
        if input
            .ordered_keys
            .as_ref()
            .is_some_and(|queue| queue.faulted(binding.keys))
            || binding.hardware.is_some_and(|token| {
                input
                    .live_notes()
                    .is_some_and(|live| live.activation_rejected(token))
            })
        {
            return Some(MidiCaptureFault::Capacity);
        }
        (!self.validate_capture_input(binding, None)).then_some(MidiCaptureFault::InputChanged)
    }
    pub(crate) fn capture_render(
        &self,
        take_id: u64,
    ) -> Result<Option<MidiCaptureRender<'_>>, Invalid> {
        self.capture
            .as_ref()
            .map_or(Ok(None), |capture| capture.render(take_id))
    }
    pub(crate) fn finish_capture(&mut self, take_id: u64) -> Result<(), Invalid> {
        let capture = self
            .capture
            .as_mut()
            .filter(|capture| {
                capture
                    .binding
                    .is_some_and(|binding| binding.take_id == take_id)
            })
            .ok_or(Invalid("midi capture take is stale"))?;
        if capture.faulted.is_some() {
            return Err(Invalid("midi capture was interrupted"));
        }
        capture.active = false;
        capture.invalidate();
        Ok(())
    }
    pub(crate) fn invalidate_capture_render(&mut self) {
        if let Some(capture) = &mut self.capture {
            capture.invalidate();
        }
    }
    pub(crate) fn fail_capture(&mut self, fault: MidiCaptureFault) {
        if let Some(capture) = &mut self.capture {
            if capture.active {
                capture.fail(fault);
            } else {
                capture.invalidate();
            }
        }
    }
    pub(super) fn capture_voice_changes(
        &mut self,
        before: &[Option<Voice>; VOICES],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) {
        let Some(capture) = self.capture.as_mut().filter(|capture| capture.is_active()) else {
            return;
        };
        let Some(binding) = capture.binding else {
            capture.fail(MidiCaptureFault::InputChanged);
            return;
        };
        let Some(frame) = clock.and_then(|clock| clock.performance_frame_at_offset(0)) else {
            capture.fail(MidiCaptureFault::Clock);
            return;
        };
        let first_occurrence = capture.first_occurrence;
        // Only occurrences admitted since Arm participate. Already-held monitor
        // notes retain phase and their later release does not invent a take onset.
        for voice in before
            .iter()
            .flatten()
            .filter(|voice| voice.id > first_occurrence && voice.gate.is_none())
        {
            if self
                .scratch
                .iter()
                .flatten()
                .any(|after| after.id == voice.id && after.gate.is_some())
                && let Some(source) = capture_source(binding, voice)
                && capture
                    .push(AcceptedMidiNote {
                        source,
                        occurrence_id: voice.id,
                        performance_frame: frame,
                        event: MidiCaptureEvent::Off,
                    })
                    .is_err()
            {
                return;
            }
        }
        for voice in self
            .scratch
            .iter()
            .flatten()
            .filter(|voice| voice.id > first_occurrence && voice.gate.is_none())
        {
            if !before.iter().flatten().any(|before| before.id == voice.id) {
                let Some(source) = capture_source(binding, voice) else {
                    continue;
                };
                let (pitch, velocity) = match voice.source {
                    VoiceSource::Physical => (
                        ((voice.token & 255) - 1) as u8,
                        ((voice.token >> 8) & 255) as u8,
                    ),
                    VoiceSource::Audition => continue,
                    VoiceSource::Hardware { pitch, .. } => (pitch, (voice.sampler / 128 + 1) as u8),
                };
                if capture
                    .push(AcceptedMidiNote {
                        source,
                        occurrence_id: voice.id,
                        performance_frame: frame,
                        event: MidiCaptureEvent::On { pitch, velocity },
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}
fn capture_source(binding: MidiCaptureBinding, voice: &Voice) -> Option<MidiCaptureSource> {
    match voice.source {
        VoiceSource::Physical => Some(MidiCaptureSource::Computer {
            activation: binding.keys,
            key: voice.key as u8,
            press_token: voice.token >> 16,
        }),
        VoiceSource::Audition => None,
        VoiceSource::Hardware { channel, .. } => Some(MidiCaptureSource::Hardware {
            activation: binding.hardware?,
            channel,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer_keyboard::tests::{fixture, playback};
    use crate::playback::PlaybackCommand;

    fn setup(
        hardware: bool,
    ) -> (
        hex_project::Project,
        TrackId,
        Arc<KeyboardActivationClock>,
        Arc<ComputerKeyboardInput>,
        crate::playback::Playback,
    ) {
        let (project, track) = fixture();
        let clock = Arc::new(KeyboardActivationClock::default());
        let mut input = ComputerKeyboardInput::with_output_activation(Arc::clone(&clock)).unwrap();
        if hardware {
            input = input.with_live_notes().unwrap();
        }
        let input = Arc::new(input.with_recording_support().unwrap());
        let mut runtime = playback(&project, track, &input);
        runtime.apply(PlaybackCommand::Play);
        (project, track, clock, input, runtime)
    }
    fn binding(
        project: &hex_project::Project,
        track: TrackId,
        input: &ComputerKeyboardInput,
        keys: MidiKeyActivation,
    ) -> MidiCaptureBinding {
        MidiCaptureBinding {
            take_id: 1,
            track,
            device: project.track(track).unwrap().instrument().unwrap().id(),
            sample_rate: project.sample_rate(),
            output_epoch: input.output_epoch_on_control().unwrap(),
            keys,
            hardware: input
                .live_notes()
                .map(|live| live.current_activation_on_control().unwrap()),
            first_frame: 0,
            end_frame: 48_000,
        }
    }
    #[test]
    fn arming_preserves_preheld_phase_and_short_new_press_survives_ack_delay() {
        let (project, track, clock, input, mut runtime) = setup(false);
        input.set_key(0, Some((60, 100, 1))).unwrap();
        runtime.render(&mut [[0.0; 2]; 64]).unwrap();
        let keys = input.begin_ordered_keys_on_control().unwrap();
        input.set_key(1, Some((64, 80, 2))).unwrap();
        input.set_key(1, None).unwrap();
        let mut before_arm = [[0.0; 2]; 32];
        runtime.render(&mut before_arm).unwrap();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let held = instrument.prepare_note(60, 100).unwrap();
        for (offset, actual) in before_arm.into_iter().enumerate() {
            assert_eq!(
                actual,
                held.sample(
                    64 + offset as u64,
                    u64::MAX,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES
                )
            );
        }
        let binding = binding(&project, track, &input, keys);
        let armed = runtime.arm_midi_capture(binding, &clock).unwrap();
        assert_eq!(armed.performance_frame, 96);
        let mut actual = [[0.0; 2]; 512];
        runtime.render(&mut actual).unwrap();
        let captured = runtime.midi_capture_render(1).unwrap().unwrap();
        assert_eq!(captured.frames, 512);
        assert_eq!(captured.clock, armed);
        assert_eq!(captured.events.len(), 2);
        assert_eq!(captured.events[0].performance_frame, 96);
        assert_eq!(captured.events[1].performance_frame, 97);
        assert_eq!(
            captured.events[0].event,
            MidiCaptureEvent::On {
                pitch: 64,
                velocity: 80
            }
        );
        assert_eq!(
            captured.events[0].occurrence_id,
            captured.events[1].occurrence_id
        );
        assert_eq!(
            captured.events[0].source,
            MidiCaptureSource::Computer {
                activation: keys,
                key: 1,
                press_token: 2
            }
        );
        let short = instrument.prepare_note(64, 80).unwrap();
        for (offset, actual) in actual.into_iter().enumerate() {
            let a = held.sample(
                96 + offset as u64,
                u64::MAX,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            let b = short.sample(offset as u64, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert_eq!(actual, [a[0] + b[0], a[1] + b[1]]);
        }
    }
    #[test]
    fn finish_returns_to_desired_monitor_without_retrigger_and_stop_wins() {
        let (project, track, clock, input, mut runtime) = setup(false);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        runtime
            .arm_midi_capture(binding(&project, track, &input, keys), &clock)
            .unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        runtime.render(&mut [[0.0; 2]; 16]).unwrap();
        runtime.finish_midi_capture(1).unwrap();
        input.seal_ordered_keys_on_control(keys).unwrap();
        runtime.render(&mut [[0.0; 2]; 1]).unwrap();
        let mut output = [[0.0; 2]; 32];
        runtime.render(&mut output).unwrap();
        let held = hex_devices::prepare_legacy_sine_instrument(48_000)
            .unwrap()
            .prepare_note(60, 100)
            .unwrap();
        for (offset, actual) in output.into_iter().enumerate() {
            assert_eq!(
                actual,
                held.sample(
                    17 + offset as u64,
                    u64::MAX,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES
                )
            );
        }
        let next = input.begin_ordered_keys_on_control().unwrap();
        runtime
            .arm_midi_capture(
                MidiCaptureBinding {
                    take_id: 2,
                    ..binding(&project, track, &input, next)
                },
                &clock,
            )
            .unwrap();
        input.set_key(1, Some((64, 80, 2))).unwrap();
        clock.revoke_reserved_prefix();
        runtime.apply_priority_stop(&clock);
        assert!(runtime.finish_midi_capture(2).is_err());
        assert_eq!(
            runtime.midi_capture_fault(2),
            Some(MidiCaptureFault::InputChanged)
        );
        input.reactivate_on_control().unwrap();
        runtime.apply(PlaybackCommand::Play);
        let mut output = [[0.0; 2]; 32];
        runtime.render(&mut output).unwrap();
        assert_eq!(
            output, [[0.0; 2]; 32],
            "stale ordered attacks cannot leak after Stop"
        );
    }
    #[test]
    fn one_frame_transactions_alternate_sources_and_preserve_channel_velocity() {
        let (project, track, clock, input, mut runtime) = setup(true);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        let binding = binding(&project, track, &input, keys);
        runtime.arm_midi_capture(binding, &clock).unwrap();
        let live = input.live_notes().unwrap();
        let hardware = binding.hardware.unwrap();
        for pitch in [60, 62] {
            live.publish(
                hardware,
                LiveNoteEvent::NoteOn {
                    channel: 9,
                    pitch,
                    velocity: 37,
                },
            )
            .unwrap();
        }
        input.set_key(0, Some((64, 80, 1))).unwrap();
        input.set_key(1, Some((67, 100, 2))).unwrap();
        let mut sources = Vec::new();
        for frame in 0..4 {
            runtime.render(&mut [[0.0; 2]; 1]).unwrap();
            let captured = runtime.midi_capture_render(1).unwrap().unwrap();
            assert_eq!(captured.events.len(), 1);
            assert_eq!(captured.events[0].performance_frame, frame);
            sources.push(captured.events[0].source);
            if frame % 2 == 0 {
                assert!(matches!(
                    captured.events[0].event,
                    MidiCaptureEvent::On { velocity: 37, .. }
                ));
            }
        }
        assert!(matches!(
            sources.as_slice(),
            [
                MidiCaptureSource::Hardware { channel: 9, .. },
                MidiCaptureSource::Computer { key: 0, .. },
                MidiCaptureSource::Hardware { channel: 9, .. },
                MidiCaptureSource::Computer { key: 1, .. }
            ]
        ));
    }
    #[test]
    fn rollback_hides_staging_and_preserves_both_prefixes() {
        let (project, track, clock, input, mut playback) = setup(true);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        let binding = binding(&project, track, &input, keys);
        playback.arm_midi_capture(binding, &clock).unwrap();
        input.set_key(0, Some((64, 100, 1))).unwrap();
        input.set_key(0, None).unwrap();
        let live = input.live_notes().unwrap();
        live.publish(
            binding.hardware.unwrap(),
            LiveNoteEvent::NoteOn {
                channel: 3,
                pitch: 60,
                velocity: 37,
            },
        )
        .unwrap();
        let mut keyboard =
            ComputerKeyboardRuntime::prepare(&project, track, Arc::clone(&input)).unwrap();
        // Use a second isolated runtime so its Arm and transaction ownership are
        // exercised without reaching through Playback's private graph field.
        keyboard.capture.as_mut().unwrap().binding = Some(binding);
        keyboard.capture.as_mut().unwrap().active = true;
        keyboard.capture.as_mut().unwrap().begin(0, 0, 16).unwrap();
        keyboard.begin_block();
        struct Clock;
        impl hex_lattice::NativeInstrumentClock for Clock {
            fn beat_at_offset(&self, _: usize) -> Option<u64> {
                Some(0)
            }
            fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
                Some(offset as u64)
            }
            fn transport_is_running(&self) -> Option<bool> {
                Some(true)
            }
        }
        let mut abandoned = [[0.0; 2]; 16];
        keyboard.render(&mut abandoned, Some(&Clock)).unwrap();
        keyboard.rollback();
        assert!(keyboard.capture_render(1).unwrap().is_none());
        assert_eq!(live.snapshot().unwrap().len, 1);
        assert_eq!(
            input.ordered_keys.as_ref().unwrap().snapshot().unwrap().len,
            2
        );
        keyboard.capture.as_mut().unwrap().begin(0, 0, 16).unwrap();
        keyboard.begin_block();
        let mut retry = [[0.0; 2]; 16];
        keyboard.render(&mut retry, Some(&Clock)).unwrap();
        assert_eq!(retry, abandoned);
        keyboard.commit();
        keyboard.capture.as_mut().unwrap().seal();
        assert_eq!(keyboard.capture_render(1).unwrap().unwrap().events.len(), 3);
    }
    #[test]
    fn section_runtime_loop_refuses_capture_without_changing_saved_loop() {
        let (project, track, clock, input, _) = setup(false);
        assert!(project.loop_region().is_none());
        let runtime = crate::SessionPlayback::prepare_section_arrangement_preroll(
            crate::computer_keyboard::tests::prepared(&project),
            None,
            hex_project::LoopRegion::new(0, 24_000, true).unwrap(),
            0,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        let mut playback =
            crate::playback::Playback::from_interactive_arrangement_preroll(runtime).unwrap();
        playback
            .prepare_computer_keyboard(&project, track, Arc::clone(&input))
            .unwrap();
        let keys = input.begin_ordered_keys_on_control().unwrap();
        assert!(
            playback
                .arm_midi_capture(binding(&project, track, &input, keys), &clock)
                .is_err()
        );
        assert!(!input.is_cancelled());
    }
    #[test]
    fn invalid_admission_and_capture_faults_preserve_monitoring() {
        let (project, track, clock, input, mut runtime) = setup(false);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        let binding = binding(&project, track, &input, keys);
        let other_clock = KeyboardActivationClock::default();
        assert!(runtime.arm_midi_capture(binding, &other_clock).is_err());
        assert!(
            runtime
                .arm_midi_capture(
                    MidiCaptureBinding {
                        sample_rate: 44_100,
                        ..binding
                    },
                    &clock
                )
                .is_err()
        );
        runtime
            .arm_midi_capture(
                MidiCaptureBinding {
                    end_frame: 4,
                    ..binding
                },
                &clock,
            )
            .unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        runtime.render(&mut [[0.0; 2]; 8]).unwrap();
        assert_eq!(runtime.midi_capture_render(1).unwrap().unwrap().frames, 4);
        let mut audible = [[0.0; 2]; 32];
        runtime.render(&mut audible).unwrap();
        assert_eq!(runtime.midi_capture_fault(1), Some(MidiCaptureFault::Clock));
        assert!(audible.iter().flatten().any(|sample| *sample != 0.0));
        assert!(!input.is_cancelled());
    }
    #[test]
    fn stale_off_does_not_release_repressed_key_and_bulk_clear_records_release() {
        let (project, track, clock, input, mut runtime) = setup(false);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        runtime
            .arm_midi_capture(binding(&project, track, &input, keys), &clock)
            .unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let old = input.key_word(0).unwrap();
        input.set_key(0, Some((64, 80, 2))).unwrap();
        input
            .ordered_keys
            .as_ref()
            .unwrap()
            .publish_change(0, old, 0);
        runtime.render(&mut [[0.0; 2]; 8]).unwrap();
        let captured = runtime.midi_capture_render(1).unwrap().unwrap();
        assert_eq!(captured.events.len(), 3);
        assert_eq!(captured.events[1].event, MidiCaptureEvent::Off);
        assert_eq!(
            captured.events[2].event,
            MidiCaptureEvent::On {
                pitch: 64,
                velocity: 80
            }
        );
        input.clear_keys_on_control();
        runtime.render(&mut [[0.0; 2]; 8]).unwrap();
        let captured = runtime.midi_capture_render(1).unwrap().unwrap();
        assert_eq!(captured.events.len(), 1);
        assert_eq!(captured.events[0].event, MidiCaptureEvent::Off);
        assert!(matches!(
            captured.events[0].source,
            MidiCaptureSource::Computer { press_token: 2, .. }
        ));
    }
    #[test]
    fn stale_ordered_activation_cannot_be_armed_with_a_fresh_output_epoch() {
        let (project, track, clock, input, mut runtime) = setup(false);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        clock.revoke_reserved_prefix();
        input.ensure_output_activation_on_control().unwrap();
        assert!(
            runtime
                .arm_midi_capture(binding(&project, track, &input, keys), &clock)
                .is_err()
        );
        assert!(runtime.midi_capture_render(1).unwrap().is_none());
    }
    #[test]
    fn sealed_batch_survives_control_stop_revocation_but_next_attempt_hides_it() {
        let (project, track, clock, input, mut runtime) = setup(false);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        runtime
            .arm_midi_capture(binding(&project, track, &input, keys), &clock)
            .unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        runtime.render(&mut [[0.0; 2]; 8]).unwrap();
        clock.revoke_reserved_prefix();
        assert_eq!(
            runtime
                .midi_capture_render(1)
                .unwrap()
                .unwrap()
                .events
                .len(),
            1,
            "engine evidence is sealed; host delivery owns subsequent Stop checks"
        );
        runtime.render(&mut []).unwrap();
        assert!(runtime.midi_capture_render(1).is_err());
        assert_eq!(
            runtime.midi_capture_fault(1),
            Some(MidiCaptureFault::InputChanged)
        );
    }
    #[test]
    fn pool_refusal_faults_capture_in_the_same_transaction_and_keeps_monitor_root() {
        let (project, track, clock, input, mut runtime) = setup(true);
        let live = input.live_notes().unwrap();
        let hardware = live.current_activation_on_control().unwrap();
        for pitch in 32..96 {
            live.publish(
                hardware,
                LiveNoteEvent::NoteOn {
                    channel: 2,
                    pitch,
                    velocity: 37,
                },
            )
            .unwrap();
        }
        runtime.render(&mut [[0.0; 2]; 64]).unwrap();
        let keys = input.begin_ordered_keys_on_control().unwrap();
        runtime
            .arm_midi_capture(binding(&project, track, &input, keys), &clock)
            .unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        runtime.render(&mut [[0.0; 2]; 8]).unwrap();
        assert_eq!(
            runtime.midi_capture_fault(1),
            Some(MidiCaptureFault::Capacity)
        );
        assert!(runtime.midi_capture_render(1).is_err());
        assert!(!input.is_cancelled());
        assert!(!live.is_cancelled());
    }
    #[test]
    fn pending_session_route_is_refused_before_arm_consumes_keys() {
        let (project, track, clock, input, mut runtime) = setup(false);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        runtime
            .try_apply_action(crate::PlaybackAction::Session(
                crate::SessionPlaybackCommand::StopTrack {
                    track,
                    quantization: crate::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        assert!(
            runtime
                .arm_midi_capture(binding(&project, track, &input, keys), &clock)
                .is_err()
        );
        assert!(input.ordered_keys.as_ref().unwrap().baseline().is_some());
        assert!(!input.is_cancelled());
    }
    #[test]
    fn graph_handoff_refuses_an_active_take_without_consuming_its_input() {
        let (project, track, clock, input, mut runtime) = setup(false);
        let keys = input.begin_ordered_keys_on_control().unwrap();
        runtime
            .arm_midi_capture(binding(&project, track, &input, keys), &clock)
            .unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut candidate = playback(&project, track, &input);
        assert!(candidate.try_inherit_runtime_from(&mut runtime).is_err());
        runtime.render(&mut [[0.0; 2]; 8]).unwrap();
        assert_eq!(
            runtime
                .midi_capture_render(1)
                .unwrap()
                .unwrap()
                .events
                .len(),
            1
        );
        assert!(!input.is_cancelled());
    }
    #[test]
    fn recording_storage_is_opt_in_and_actual_charge_includes_ring_and_capture() {
        let (project, track) = fixture();
        let plain = Arc::new(ComputerKeyboardInput::default());
        let recording = Arc::new(
            ComputerKeyboardInput::default()
                .with_recording_support()
                .unwrap(),
        );
        let a = ComputerKeyboardRuntime::prepare(&project, track, plain).unwrap();
        let b = ComputerKeyboardRuntime::prepare(&project, track, recording).unwrap();
        assert!(a.capture.is_none());
        assert!(b.capture.is_some());
        assert_eq!(
            b.storage_bytes().unwrap() - a.storage_bytes().unwrap(),
            std::mem::size_of::<CaptureStorage>()
                + std::mem::size_of::<ordered_keys::OrderedKeys>()
        );
        assert!(b.storage_bytes().unwrap() <= MAX_COMPUTER_KEYBOARD_STORAGE_BYTES);
    }
}
