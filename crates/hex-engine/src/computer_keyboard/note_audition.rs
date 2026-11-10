//! One-shot exact-note banks inside the existing live-keyboard lease. Banks are
//! immutable after control construction and identity is the enclosing input Arc.
//! The audio-owned commit latch expires one trigger without cutting release tails.
use super::*;
use std::sync::atomic::Ordering::SeqCst;

/// Finite audition storage/work budget; larger selections are refused as a whole.
pub const MAX_NOTE_AUDITION_NOTES: usize = COMPUTER_KEY_COUNT;
/// Hard frame ceiling independent of UI progress. The UI normally requests 250 ms.
pub const MAX_NOTE_AUDITION_GATE_FRAMES: u64 = 768_000;

#[derive(Debug)]
pub(super) struct NoteAudition {
    notes: [(u8, u8); MAX_NOTE_AUDITION_NOTES],
    len: usize,
    gate_frames: u64,
}

/// Control-written alongside the physical snapshot sequence. Callback reads
/// are bounded and never take ownership of the input or hardware queue.
#[derive(Debug, Default)]
pub(super) struct LiveAudition {
    id: AtomicU64,
    frames: AtomicU64,
    words: [AtomicU64; COMPUTER_KEY_COUNT],
}

#[derive(Clone, Copy)]
pub(crate) struct AuditionRequest {
    id: u64,
    frames: u64,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct AuditionGate {
    request: Option<AuditionRequest>,
    rendered: u64,
}

impl AuditionGate {
    pub(crate) fn observe(
        &mut self,
        request: Option<AuditionRequest>,
        input: Option<&ComputerKeyboardInput>,
    ) {
        if self.request.map(|gate| gate.id) != request.map(|gate| gate.id) {
            self.rendered = request.map_or(0, |gate| {
                if input.is_some_and(|input| input.audition_started.load(Acquire) >= gate.id) {
                    // A rebuilt runtime without inherited progress cannot replay a trigger.
                    gate.frames
                } else {
                    0
                }
            });
        }
        self.request = request;
    }

    pub(crate) fn active(self) -> bool {
        self.request.is_some()
    }

    pub(crate) fn expired(self) -> bool {
        self.request
            .is_some_and(|gate| self.rendered >= gate.frames)
    }

    pub(crate) fn advance(&mut self, frames: u64) {
        if let Some(gate) = self.request {
            self.rendered = self.rendered.saturating_add(frames).min(gate.frames);
        }
    }

    pub(super) fn finish(&mut self) {
        if let Some(gate) = self.request {
            self.rendered = gate.frames;
        }
    }

    pub(crate) fn commit(self, input: &ComputerKeyboardInput) {
        if let Some(gate) = self.request.filter(|_| self.rendered != 0) {
            if input.audition_started.load(Acquire) < gate.id {
                input.audition_started.fetch_max(gate.id, Release);
            }
            if self.expired() && input.audition_finished.load(Acquire) < gate.id {
                input.audition_finished.fetch_max(gate.id, Release);
            }
        }
    }
}

fn valid_bank(notes: &[(u8, u8)], frames: u64) -> bool {
    !notes.is_empty()
        && notes.len() <= MAX_NOTE_AUDITION_NOTES
        && notes
            .iter()
            .all(|(pitch, velocity)| *pitch <= 127 && (1..=127).contains(velocity))
        && (1..=MAX_NOTE_AUDITION_GATE_FRAMES).contains(&frames)
}

impl ComputerKeyboardInput {
    /// Turn a fresh control-created input into one immutable exact-velocity chord.
    /// No source/runtime authority changes. Each selection uses a fresh input Arc;
    /// the private CURRENT mailbox intentionally retains its physical-key protocol.
    /// The gate starts at first accepted audio, expires at the next render-block
    /// boundary after `gate_frames`, and then uses the instrument's normal release.
    /// Failed audio transactions do not consume the gate or latch completion.
    pub fn with_note_audition(
        mut self,
        notes: &[(u8, u8)],
        gate_frames: u64,
    ) -> Result<Self, Invalid> {
        if !valid_bank(notes, gate_frames)
            || self.native_activation.is_some()
            || self.sequence.load(Acquire) != 0
            || self.is_cancelled()
            || self.audition.is_some()
            || self.live_notes.is_some()
        {
            return Err(Invalid(
                "note audition requires 1–17 valid notes and a fresh ordinary input",
            ));
        }
        let mut bank = NoteAudition {
            notes: [(0, 0); MAX_NOTE_AUDITION_NOTES],
            len: notes.len(),
            gate_frames,
        };
        bank.notes[..notes.len()].copy_from_slice(notes);
        self.audition = Some(bank);
        Ok(self)
    }
    pub fn is_note_audition(&self) -> bool {
        self.audition.is_some()
    }

    pub fn note_audition_finished(&self) -> bool {
        self.is_note_audition() && self.audition_finished.load(Acquire) != 0
    }

    /// Audition on the already admitted live root. This replaces only the
    /// preceding preview and never writes to the hardware or recording queues.
    /// Physical keys take priority over the preview.
    pub fn trigger_live_note_audition_on_control(
        &self,
        notes: &[(u8, u8)],
        gate_frames: u64,
    ) -> Result<(), Invalid> {
        let bank = self.live_audition.as_ref().ok_or(Invalid(
            "live note audition requires a prepared controller input",
        ))?;
        if !valid_bank(notes, gate_frames)
            || self.is_cancelled()
            || self
                .ordered_keys
                .as_ref()
                .is_some_and(|keys| keys.has_recording_prefix())
            || self.sequence.load(Acquire) >= (u64::MAX >> 15) - 1
            || self.notes.iter().any(|word| word.load(Acquire) != 0)
        {
            return Err(Invalid("live note audition is unavailable for this input"));
        }
        if self.output_activation.is_some() {
            self.ensure_output_activation_on_control()?;
        }
        let after = self.begin_write()?;
        for (index, destination) in bank.words.iter().enumerate() {
            // Slot identity is separate from physical words. A change of bank
            // releases even an identical pitch/velocity before retriggering.
            let word = notes.get(index).map_or(0, |&(pitch, velocity)| {
                ((after >> 1) << 16) | (u64::from(velocity) << 8) | u64::from(pitch + 1)
            });
            destination.store(word, SeqCst);
        }
        bank.frames.store(gate_frames, SeqCst);
        bank.id.store(after, SeqCst);
        self.sequence.store(after, SeqCst);
        Ok(())
    }

    pub fn cancel_live_note_audition_on_control(&self) {
        // The sole control writer publishes a scalar revocation. The snapshot
        // verifies this identity after reading the words, as well as its sequence.
        if let Some(bank) = &self.live_audition {
            bank.id.store(0, SeqCst);
        }
    }

    pub(super) fn audition_snapshot(
        &self,
        words: &mut [u64; COMPUTER_KEY_COUNT],
        sequence: u64,
    ) -> Option<AuditionRequest> {
        if let Some(bank) = &self.audition {
            if self.note_audition_finished() {
                words.fill(0);
                return None;
            }
            return (sequence != 0).then_some(AuditionRequest {
                id: 2,
                frames: bank.gate_frames,
            });
        }
        let bank = self.live_audition.as_ref()?;
        let id = bank.id.load(SeqCst);
        if id == 0
            || self.audition_finished.load(Acquire) >= id
            || words.iter().any(|word| *word != 0)
        {
            return None;
        }
        let frames = bank.frames.load(SeqCst);
        let preview = std::array::from_fn(|index| bank.words[index].load(SeqCst));
        if id != bank.id.load(SeqCst) {
            return None;
        }
        *words = preview;
        Some(AuditionRequest { id, frames })
    }
    /// Publish the whole bank in one coherent transaction, once per input.
    /// Control triggers only after its preparation ACK; cancellation always wins.
    pub fn trigger_note_audition(&self) -> Result<(), Invalid> {
        let bank = self
            .audition
            .as_ref()
            .ok_or(Invalid("input has no note audition bank"))?;
        if self.is_cancelled() || self.note_audition_finished() || self.sequence.load(Acquire) != 0
        {
            return Err(Invalid("note audition activation is no longer fresh"));
        }
        let after = self.begin_write()?;
        for (index, destination) in self.notes.iter().enumerate() {
            let word = if index < bank.len {
                let (pitch, velocity) = bank.notes[index];
                (((index + 1) as u64) << 16) | (u64::from(velocity) << 8) | u64::from(pitch + 1)
            } else {
                0
            };
            destination.store(word, SeqCst);
        }
        self.sequence.store(after, SeqCst);
        Ok(())
    }
    pub(super) fn template_count(&self) -> usize {
        self.audition.as_ref().map_or(
            if self.live_notes.is_some() {
                NOTES
            } else {
                COMPUTER_NOTES
            },
            |bank| bank.len,
        )
    }
    pub(super) fn template_note(&self, index: usize) -> Option<(u8, u8)> {
        if let Some(bank) = &self.audition {
            return bank.notes.get(index).copied().filter(|_| index < bank.len);
        }
        if self.live_notes.is_some() {
            return (index < NOTES).then_some(((index % 128) as u8, (index / 128 + 1) as u8));
        }
        Some((
            (index % 128) as u8,
            *COMPUTER_KEY_VELOCITIES.get(index / 128)?,
        ))
    }
}
impl KeyboardInput {
    pub(super) fn sampler_index(
        &self,
        key: usize,
        pitch: u8,
        velocity: u8,
    ) -> Result<usize, Invalid> {
        if let Some(bank) = self.lease().and_then(|input| input.audition.as_ref()) {
            return bank
                .notes
                .get(key)
                .filter(|note| key < bank.len && **note == (pitch, velocity))
                .map(|_| key)
                .ok_or(Invalid("note audition word differs from its prepared bank"));
        }
        if self.lease().is_some_and(|input| input.live_notes.is_some()) {
            return Ok((usize::from(velocity) - 1) * 128 + usize::from(pitch));
        }
        let velocity_index = COMPUTER_KEY_VELOCITIES
            .iter()
            .position(|value| *value == velocity)
            .ok_or(Invalid("computer keyboard velocity is invalid"))?;
        Ok(velocity_index * 128 + usize::from(pitch))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, playback};
    use super::*;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    use hex_project::{Command, Editor, Project};

    #[test]
    fn exact_bank_trigger_is_atomic_bounded_and_one_shot() {
        for notes in [vec![], vec![(60, 0)], vec![(128, 37)], vec![(60, 100); 18]] {
            assert!(
                ComputerKeyboardInput::default()
                    .with_note_audition(&notes, 64)
                    .is_err()
            );
        }
        for gate in [0, MAX_NOTE_AUDITION_GATE_FRAMES + 1] {
            assert!(
                ComputerKeyboardInput::default()
                    .with_note_audition(&[(60, 37)], gate)
                    .is_err()
            );
        }
        let (project, track) = fixture();
        let full = Arc::new(
            ComputerKeyboardInput::default()
                .with_note_audition(&[(60, 37); 17], 64)
                .unwrap(),
        );
        let mut complete = playback(&project, track, &full);
        full.trigger_note_audition().unwrap();
        let mut output = [[0.0; 2]; 64];
        complete.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|sample| *sample != 0.0));
        assert!(!full.rejected());
        let input = ComputerKeyboardInput::default()
            .with_note_audition(&[(60, 37), (64, 93)], 64)
            .unwrap();
        assert!(input.set_key(0, Some((60, 100, 1))).is_err());
        assert!(input.reactivate_on_control().is_err());
        assert!(
            matches!(input.note_snapshot(), NoteSnapshot::Active(_, words, _) if words == [0; COMPUTER_KEY_COUNT])
        );
        input.trigger_note_audition().unwrap();
        let NoteSnapshot::Active(_, words, _) = input.note_snapshot() else {
            panic!("complete bank")
        };
        assert_eq!((words[0] & 255, (words[0] >> 8) & 255), (61, 37));
        assert_eq!((words[1] & 255, (words[1] >> 8) & 255), (65, 93));
        assert!(words[2..].iter().all(|word| *word == 0));
        assert!(input.trigger_note_audition().is_err());
        input.sequence.store(3, SeqCst);
        assert!(matches!(input.note_snapshot(), NoteSnapshot::Deferred(_)));
        input.cancel();
        assert!(matches!(input.note_snapshot(), NoteSnapshot::Inactive));
    }

    #[test]
    fn exact_velocity_chord_matches_independent_pcm_and_audio_gate_cannot_retrigger() {
        let (project, track) = fixture();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let a = instrument.prepare_note(60, 37).unwrap();
        let b = instrument.prepare_note(64, 93).unwrap();
        for partition in [1, 17, 127, 256] {
            let input = Arc::new(
                ComputerKeyboardInput::default()
                    .with_note_audition(&[(60, 37), (64, 93)], 64)
                    .unwrap(),
            );
            let mut tested = playback(&project, track, &input);
            let mut waiting = [[1.0; 2]; 256];
            tested.render(&mut waiting).unwrap();
            assert_eq!(waiting, [[0.0; 2]; 256], "preparation alone never triggers");
            assert!(!input.note_audition_finished());
            input.trigger_note_audition().unwrap();
            let mut output = vec![[0.0; 2]; 4096];
            for frames in output.chunks_mut(partition) {
                tested.render(frames).unwrap();
            }
            let gate = 64_u64.div_ceil(partition as u64) * partition as u64;
            assert!((64..64 + partition as u64).contains(&gate));
            for (age, actual) in output.into_iter().enumerate() {
                let x = a.sample(age as u64, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                let y = b.sample(age as u64, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                assert_eq!(
                    actual,
                    [x[0] + y[0], x[1] + y[1]],
                    "partition {partition}, frame {age}"
                );
            }
            assert!(input.note_audition_finished());
            let mut reprepared = playback(&project, track, &input);
            let mut silent = [[1.0; 2]; 256];
            reprepared.render(&mut silent).unwrap();
            assert_eq!(
                silent, [[0.0; 2]; 256],
                "a new graph cannot replay an expired bank"
            );
        }
    }

    #[test]
    fn rejected_audio_transaction_does_not_consume_the_audition_gate() {
        let (_, track) = fixture();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let input = Arc::new(
            ComputerKeyboardInput::default()
                .with_note_audition(&[(60, 37)], 64)
                .unwrap(),
        );
        let mut runtime = ComputerKeyboardRuntime::prepare_instrument(
            track,
            Arc::clone(&input),
            48_000,
            instrument.as_ref(),
        )
        .unwrap();
        input.trigger_note_audition().unwrap();
        runtime.begin_block();
        runtime.render(&mut [[0.0; 2]; 128], None).unwrap();
        runtime.rollback();
        assert!(!input.note_audition_finished());
        for expected_finished in [false, true] {
            runtime.begin_block();
            runtime.render(&mut [[0.0; 2]; 32], None).unwrap();
            runtime.commit();
            assert_eq!(input.note_audition_finished(), expected_finished);
        }
        input.sequence.store(3, SeqCst);
        let mut release = [[0.0; 2]; 8];
        runtime.begin_block();
        runtime.render(&mut release, None).unwrap();
        runtime.commit();
        let note = instrument.prepare_note(60, 37).unwrap();
        for (offset, actual) in release.into_iter().enumerate() {
            assert_eq!(
                actual,
                note.sample(64 + offset as u64, 64, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            );
        }
    }

    #[test]
    fn fresh_runtime_cannot_replay_a_bank_consumed_before_its_gate_expired() {
        let (project, track) = fixture();
        let input = Arc::new(
            ComputerKeyboardInput::default()
                .with_note_audition(&[(60, 37)], 4096)
                .unwrap(),
        );
        let mut first = playback(&project, track, &input);
        input.trigger_note_audition().unwrap();
        let mut output = [[0.0; 2]; 64];
        first.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|sample| *sample != 0.0));
        assert!(!input.note_audition_finished());
        let mut fresh = playback(&project, track, &input);
        fresh.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 64]);
        assert!(input.note_audition_finished());
    }

    #[test]
    fn mutable_instrument_admits_a_bank_whose_high_pitch_silences_nyquist_operators() {
        let (source, track) = fixture();
        let mut snapshot = source.snapshot();
        snapshot.sample_rate = 8000;
        let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
        let metadata = &hex_devices::operator_state::OPERATOR_METADATA;
        let state = metadata
            .patch_parameter(
                &metadata.default_state().unwrap(),
                ParameterId::from_static(78),
                ParameterValue::Float(0.4),
            )
            .unwrap();
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let before = editor.project().snapshot();
        let input = Arc::new(
            ComputerKeyboardInput::default()
                .with_note_audition(&[(60, 37), (127, 93)], 64)
                .unwrap(),
        );
        // Operators above nyquist are silenced per note (Pulse #3957).
        assert!(ComputerKeyboardRuntime::prepare(editor.project(), track, input).is_ok());
        assert_eq!(editor.project().snapshot(), before);
    }
}

#[cfg(test)]
mod mutable_pcm_tests {
    use super::super::tests::{fixture, playback, prepared};
    use super::*;
    use crate::playback::{Playback, PlaybackCommand};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{BeatTime, Command, Created, Editor};
    #[test]
    fn exact_velocity_mutable_chord_matches_canonical_playback_at_the_audio_gate() {
        let (project, track) = fixture();
        let mut source = Editor::new(project, 16).unwrap();
        let device = source
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        source
            .execute(Command::ReplaceDeviceState {
                device,
                state: hex_devices::operator_state::OPERATOR_METADATA
                    .default_state()
                    .unwrap(),
            })
            .unwrap();
        let input = Arc::new(
            ComputerKeyboardInput::default()
                .with_note_audition(&[(60, 37), (64, 93)], 375)
                .unwrap(),
        );
        let mut actual = playback(source.project(), track, &input);
        let Created::Clip(clip) = source
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("fixture clip")
        };
        for (pitch, velocity) in [(60, 37), (64, 93)] {
            source
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::from_raw(BeatTime::ONE.raw() / 64),
                    pitch,
                    channel: 0,
                    velocity,
                })
                .unwrap();
        }
        let mut expected = Playback::from_prepared(prepared(source.project()));
        expected.apply(PlaybackCommand::Play);
        actual.apply(PlaybackCommand::Play);
        input.trigger_note_audition().unwrap();
        let mut peak = 0.0_f32;
        for _ in 0..64 {
            let mut a = [[0.0; 2]; 125];
            let mut b = a;
            actual.render(&mut a).unwrap();
            expected.render(&mut b).unwrap();
            for (a, b) in a.into_iter().flatten().zip(b.into_iter().flatten()) {
                peak = peak.max(a.abs());
                assert!((a - b).abs() < 1.0e-5, "{a} != {b}");
            }
        }
        assert!(peak > 0.001);
        assert!(input.note_audition_finished());
    }
}
