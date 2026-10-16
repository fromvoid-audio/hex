//! Bounded, transient native keyboard audition. Preparation and destruction are control work.
use crate::{Frame, Invalid};
use hex_lattice::{
    NativeInstrumentEvent, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
    PreparedAnalyticNote, PreparedSharedAnalyticContext, PreparedSharedAnalyticNote,
};
use hex_project::TrackId;
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU64,
        Ordering::{Acquire, Release},
    },
};

pub const COMPUTER_KEY_COUNT: usize = 17;
pub const COMPUTER_KEY_VELOCITIES: [u8; 7] = [20, 40, 60, 80, 100, 120, 127];
const VOICES: usize = 64;
const NOTES: usize = 128 * COMPUTER_KEY_VELOCITIES.len();
const MAX_STORAGE: usize = 3 * 1024 * 1024;

/// One immutable activation lease. Cancellation bypasses every worker/audio queue.
/// Control is the sole desired-note writer; each physical key is independent.
/// Create a fresh lease for every activation. Cancellation is terminal, including
/// after a successful graph replacement; held keys require release and a fresh press.
#[derive(Debug)]
pub struct ComputerKeyboardInput {
    cancelled: AtomicBool,
    notes: [AtomicU64; COMPUTER_KEY_COUNT],
    rejected: AtomicBool,
}
impl Default for ComputerKeyboardInput {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notes: std::array::from_fn(|_| AtomicU64::new(0)),
            rejected: AtomicBool::new(false),
        }
    }
}
impl ComputerKeyboardInput {
    pub fn cancel(&self) {
        self.cancelled.store(true, Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Acquire)
    }
    pub fn rejected(&self) -> bool {
        self.rejected.load(Acquire)
    }
    /// Token must be nonzero and monotonically unique for this activation.
    pub fn set_key(&self, key: usize, note: Option<(u8, u8, u64)>) -> Result<(), Invalid> {
        let slot = self
            .notes
            .get(key)
            .ok_or(Invalid("computer keyboard key is out of range"))?;
        let word = match note {
            None => 0,
            Some((pitch, velocity, token))
                if pitch <= 127
                    && COMPUTER_KEY_VELOCITIES.contains(&velocity)
                    && token > 0
                    && token <= (u64::MAX >> 16) =>
            {
                (token << 16) | (u64::from(velocity) << 8) | u64::from(pitch + 1)
            }
            Some(_) => return Err(Invalid("computer keyboard note is out of range")),
        };
        slot.store(word, Release);
        Ok(())
    }
}

#[derive(Clone, Copy)]
#[allow(clippy::large_enum_variant)] // One bounded selected-bank allocation; no per-note owners in the callback.
enum Sampler {
    Inline(PreparedAnalyticNote),
    Shared(PreparedSharedAnalyticNote),
}
#[derive(Clone, Copy)]
struct Voice {
    key: usize,
    token: u64,
    sampler: usize,
    age: u64,
    gate: Option<u64>,
    id: u64,
}

struct PreparationClock(u32);
impl hex_lattice::NativeInstrumentClock for PreparationClock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        u64::try_from((offset as u128).checked_mul(2_u128 << 32)? / u128::from(self.0)).ok()
    }
}

pub(crate) enum KeyboardClock<'a> {
    Playing {
        timing: &'a dyn crate::MidiTiming,
        start: u64,
    },
    Monitor {
        beat: u64,
        elapsed: u64,
        micro_bpm: u32,
        sample_rate: u32,
    },
}
impl hex_lattice::NativeInstrumentClock for KeyboardClock<'_> {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        match *self {
            Self::Playing { timing, start } => timing
                .frame_to_beat(start.checked_add(offset as u64)?)
                .ok()
                .map(|beat| beat.raw()),
            Self::Monitor {
                beat,
                elapsed,
                micro_bpm,
                sample_rate,
            } => {
                let frames = elapsed.checked_add(offset as u64)?;
                let delta = u128::from(frames)
                    .checked_mul(u128::from(micro_bpm))?
                    .checked_mul(1_u128 << 32)?
                    / (u128::from(sample_rate) * 60_000_000);
                beat.checked_add(u64::try_from(delta).ok()?)
            }
        }
    }
}

/// Selected-track pool, owned by its ordinary prepared graph and retired with it.
/// Authored voices and live voices deliberately have independent mutable pools.
pub(crate) struct ComputerKeyboardRuntime {
    track: TrackId,
    input: Arc<ComputerKeyboardInput>,
    samplers: Vec<Option<Sampler>>,
    supported: [bool; NOTES],
    context: Option<Box<dyn PreparedSharedAnalyticContext>>,
    mutable: Option<NativeInstrumentRuntimePair>,
    release_frames: u64,
    primary: [Option<Voice>; VOICES],
    scratch: [Option<Voice>; VOICES],
    desired: [u64; COMPUTER_KEY_COUNT],
    next_id: u64,
    scratch_next_id: u64,
    age: u64,
    scratch_age: u64,
    monitor_anchor: Option<(u64, u64, u32)>,
    scratch_monitor_anchor: Option<(u64, u64, u32)>,
    boundary: bool,
    faulted: bool,
}
impl ComputerKeyboardRuntime {
    pub(crate) fn prepare(
        project: &hex_project::Project,
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
    ) -> Result<Self, Invalid> {
        let track_data = project
            .tracks()
            .iter()
            .find(|candidate| candidate.id() == track)
            .ok_or(Invalid("computer keyboard track is unavailable"))?;
        let device = track_data
            .instrument()
            .filter(|device| !device.bypassed())
            .ok_or(Invalid("select a track with an enabled instrument"))?;
        let registry = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("instrument registry is unavailable"))?;
        let instrument = registry
            .prepare_instrument(device.state(), project.sample_rate())
            .map_err(|_| Invalid("computer keyboard instrument preparation failed"))?;
        let factory = instrument
            .prepare_mutable_runtime()
            .map_err(|_| Invalid("computer keyboard runtime preparation failed"))?;
        let mut mutable = factory
            .as_ref()
            .map(|factory| {
                NativeInstrumentRuntimePair::new(
                    factory.as_ref(),
                    NativeInstrumentRuntimeLimits {
                        max_pair_bytes: 2 * 1024 * 1024,
                        max_events_per_block: 256,
                        max_active_occurrences: VOICES,
                        max_tail_frames: u64::from(project.sample_rate()) * 10,
                    },
                )
            })
            .transpose()
            .map_err(|_| Invalid("computer keyboard runtime exceeds its storage budget"))?;
        let context = if mutable.is_none() {
            instrument
                .prepare_shared_analytic_context()
                .map_err(|_| Invalid("computer keyboard context preparation failed"))?
        } else {
            None
        };
        if context.as_ref().is_some_and(|context| {
            context.declared_context_bytes() > hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES
        }) {
            return Err(Invalid(
                "computer keyboard context exceeds its storage budget",
            ));
        }
        let mut samplers = Vec::new();
        if mutable.is_none() {
            samplers
                .try_reserve_exact(NOTES)
                .map_err(|_| Invalid("computer keyboard storage is unavailable"))?;
        }
        let mut supported = [false; NOTES];
        let mut release_frames = mutable
            .as_ref()
            .map_or(0, NativeInstrumentRuntimePair::max_release_tail_frames);
        for (velocity_index, velocity) in COMPUTER_KEY_VELOCITIES.into_iter().enumerate() {
            for pitch in 0..128_u8 {
                let index = velocity_index * 128 + usize::from(pitch);
                if let Some(runtime) = mutable.as_mut() {
                    // Validate against the actual native runtime too. A provider's
                    // optional source validator may not cover every pitch domain.
                    runtime.begin_block();
                    let mut probe = [[0.0; 2]; 1];
                    supported[index] = instrument.validate_event_note(pitch, velocity).is_ok()
                        && runtime
                            .process_block_with_clock(
                                &[NativeInstrumentEvent::NoteOn {
                                    frame_offset: 0,
                                    occurrence_id: 1,
                                    pitch,
                                    velocity,
                                }],
                                &mut probe,
                                Some(&PreparationClock(project.sample_rate())),
                            )
                            .is_ok();
                    runtime.rollback_block();
                    continue;
                }
                let sampler = if let Some(context) = &context {
                    context
                        .prepare_note(pitch, velocity)
                        .ok()
                        .map(Sampler::Shared)
                } else {
                    instrument
                        .prepare_note(pitch, velocity)
                        .ok()
                        .map(Sampler::Inline)
                };
                if let Some(sampler) = sampler {
                    release_frames = release_frames.max(match sampler {
                        Sampler::Inline(note) => note.release_frames(),
                        Sampler::Shared(note) => note.release_frames(),
                    });
                    supported[index] = true;
                }
                samplers.push(sampler);
            }
        }
        if !supported.iter().any(|supported| *supported) {
            return Err(Invalid(
                "computer keyboard instrument supports no playable pitches",
            ));
        }
        let result = Self {
            track,
            input,
            samplers,
            supported,
            context,
            mutable,
            release_frames,
            primary: [None; VOICES],
            scratch: [None; VOICES],
            desired: [0; COMPUTER_KEY_COUNT],
            next_id: 0,
            scratch_next_id: 0,
            age: 0,
            scratch_age: 0,
            monitor_anchor: None,
            scratch_monitor_anchor: None,
            boundary: false,
            faulted: false,
        };
        if result
            .storage_bytes()
            .is_none_or(|bytes| bytes > MAX_STORAGE)
        {
            return Err(Invalid("computer keyboard storage exceeds its budget"));
        }
        Ok(result)
    }
    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                std::mem::size_of::<ComputerKeyboardInput>() + 2 * std::mem::size_of::<usize>(),
            )?
            .checked_add(
                self.samplers
                    .capacity()
                    .checked_mul(std::mem::size_of::<Option<Sampler>>())?,
            )?
            .checked_add(
                self.context
                    .as_ref()
                    .map_or(0, |context| context.declared_context_bytes()),
            )?
            .checked_add(self.mutable.as_ref().map_or(
                Some(0),
                NativeInstrumentRuntimePair::fixed_storage_pair_bytes,
            )?)
    }
    pub(crate) fn musical_clock<'a>(
        &mut self,
        timing: &'a crate::prepared_tempo::SessionTimingView<'_>,
        performance_frame: u64,
        playing: bool,
    ) -> Result<KeyboardClock<'a>, Invalid> {
        if playing {
            self.scratch_monitor_anchor = None;
            return Ok(KeyboardClock::Playing {
                timing,
                start: performance_frame,
            });
        }
        let (start_age, beat, micro_bpm) = match self.scratch_monitor_anchor {
            Some(anchor) => anchor,
            None => {
                use crate::MidiTiming;
                let anchor = (
                    self.scratch_age,
                    timing
                        .frame_to_beat(performance_frame)
                        .map_err(|_| Invalid("computer keyboard monitor clock unavailable"))?
                        .raw(),
                    timing
                        .micro_bpm_at_frame(performance_frame)
                        .map_err(|_| Invalid("computer keyboard monitor tempo unavailable"))?,
                );
                self.scratch_monitor_anchor = Some(anchor);
                anchor
            }
        };
        use crate::MidiTiming;
        Ok(KeyboardClock::Monitor {
            beat,
            elapsed: self
                .scratch_age
                .checked_sub(start_age)
                .ok_or(Invalid("computer keyboard monitor clock reversed"))?,
            micro_bpm,
            sample_rate: timing.sample_rate(),
        })
    }
    pub(crate) fn track(&self) -> TrackId {
        self.track
    }
    pub(crate) fn cancel(&mut self) {
        self.input.cancel();
        self.primary.fill(None);
        self.scratch.fill(None);
        if let Some(runtime) = &mut self.mutable {
            runtime.reset();
        }
    }
    pub(crate) fn begin_block(&mut self) {
        if self.input.is_cancelled() || self.faulted {
            self.primary.fill(None);
            if let Some(runtime) = &mut self.mutable {
                runtime.reset();
            }
        }
        self.scratch = self.primary;
        self.scratch_next_id = self.next_id;
        self.scratch_age = self.age;
        self.scratch_monitor_anchor = self.monitor_anchor;
        self.desired = std::array::from_fn(|index| {
            if self.input.is_cancelled() || self.faulted {
                0
            } else {
                self.input.notes[index].load(Acquire)
            }
        });
        self.boundary = true;
        if let Some(runtime) = &mut self.mutable {
            runtime.begin_block();
        }
    }
    pub(crate) fn rollback(&mut self) {
        if let Some(runtime) = &mut self.mutable {
            runtime.rollback_block();
        }
    }
    pub(crate) fn commit(&mut self) {
        self.primary = self.scratch;
        self.next_id = self.scratch_next_id;
        self.age = self.scratch_age;
        self.monitor_anchor = self.scratch_monitor_anchor;
        if let Some(runtime) = &mut self.mutable {
            runtime.commit_block();
        }
    }
    pub(crate) fn render(
        &mut self,
        output: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), Invalid> {
        if output.is_empty() {
            return Ok(());
        }
        let mut events = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 0,
        }; 2 * VOICES];
        let mut len = 0;
        self.scratch.iter_mut().for_each(|slot| {
            if slot.is_some_and(|voice| {
                voice
                    .gate
                    .is_some_and(|gate| voice.age >= gate.saturating_add(self.release_frames))
            }) {
                *slot = None;
            }
        });
        if self.boundary {
            for voice in self.scratch.iter_mut().flatten() {
                if voice.gate.is_none() && self.desired[voice.key] != voice.token {
                    voice.gate = Some(voice.age);
                    events[len] = NativeInstrumentEvent::NoteOff {
                        frame_offset: 0,
                        occurrence_id: voice.id,
                    };
                    len += 1;
                }
            }
            for (key, token) in self.desired.into_iter().enumerate() {
                if token == 0
                    || self
                        .scratch
                        .iter()
                        .flatten()
                        .any(|voice| voice.key == key && voice.token == token)
                {
                    continue;
                }
                let pitch = ((token & 255) as u8).saturating_sub(1);
                let velocity = ((token >> 8) & 255) as u8;
                let velocity_index = COMPUTER_KEY_VELOCITIES
                    .iter()
                    .position(|value| *value == velocity)
                    .ok_or(Invalid("computer keyboard velocity is invalid"))?;
                let sampler = velocity_index * 128 + usize::from(pitch);
                if !self.supported[sampler] {
                    self.input.rejected.store(true, Release);
                    continue;
                }
                let capacity = self
                    .mutable
                    .as_ref()
                    .map_or(VOICES, NativeInstrumentRuntimePair::max_active_occurrences);
                let event_capacity = self.mutable.as_ref().map_or(
                    2 * VOICES,
                    NativeInstrumentRuntimePair::max_events_per_block,
                );
                if self.scratch.iter().flatten().count() >= capacity || len >= event_capacity {
                    self.input.rejected.store(true, Release);
                    self.faulted = true;
                    self.input.cancel();
                    self.scratch.fill(None);
                    if let Some(runtime) = &mut self.mutable {
                        runtime.reset();
                        runtime.begin_block();
                    }
                    len = 0;
                    break;
                }
                let Some(slot) = self.scratch.iter_mut().find(|slot| slot.is_none()) else {
                    self.input.rejected.store(true, Release);
                    self.faulted = true;
                    self.scratch.fill(None);
                    if let Some(runtime) = &mut self.mutable {
                        runtime.reset();
                        runtime.begin_block();
                    }
                    len = 0;
                    break;
                };
                self.scratch_next_id = self
                    .scratch_next_id
                    .checked_add(1)
                    .ok_or(Invalid("computer keyboard identity exhausted"))?;
                *slot = Some(Voice {
                    key,
                    token,
                    sampler,
                    age: 0,
                    gate: None,
                    id: self.scratch_next_id,
                });
                events[len] = NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: self.scratch_next_id,
                    pitch,
                    velocity,
                };
                len += 1;
            }
            self.boundary = false;
        }
        if let Some(runtime) = &mut self.mutable {
            runtime
                .process_block_with_clock(&events[..len], output, clock)
                .map_err(|_| Invalid("computer keyboard runtime refused a note block"))?;
        } else {
            for voice in self.scratch.iter().flatten() {
                let Some(sampler) = self.samplers[voice.sampler] else {
                    continue;
                };
                for (offset, target) in output.iter_mut().enumerate() {
                    let age = voice
                        .age
                        .checked_add(offset as u64)
                        .ok_or(Invalid("computer keyboard age overflow"))?;
                    let gate = voice.gate.unwrap_or(u64::MAX);
                    let sample = match sampler {
                        Sampler::Inline(note) => note.sample(age, gate),
                        Sampler::Shared(note) => self
                            .context
                            .as_ref()
                            .ok_or(Invalid("computer keyboard context missing"))?
                            .sample(&note, age, gate),
                    };
                    target[0] += sample[0];
                    target[1] += sample[1];
                }
            }
        }
        self.scratch_age = self
            .scratch_age
            .checked_add(output.len() as u64)
            .ok_or(Invalid("computer keyboard clock overflow"))?;
        for voice in self.scratch.iter_mut().flatten() {
            voice.age = voice
                .age
                .checked_add(output.len() as u64)
                .ok_or(Invalid("computer keyboard age overflow"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PreparedRevision,
        playback::{Playback, PlaybackCommand},
    };
    use hex_project::{Command, Created, Editor, Project};
    use std::collections::BTreeMap;

    pub(super) fn fixture() -> (Project, TrackId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "keys".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        (editor.project().clone(), track)
    }
    fn prepared(project: &Project) -> Arc<PreparedRevision> {
        Arc::new(
            PreparedRevision::prepare(
                project,
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    }
    fn playback(project: &Project, track: TrackId, input: &Arc<ComputerKeyboardInput>) -> Playback {
        let mut playback = Playback::from_prepared(prepared(project));
        playback
            .prepare_computer_keyboard(project, track, Arc::clone(input))
            .unwrap();
        playback
    }
    #[test]
    fn computer_keyboard_stopped_polyphony_release_matches_independent_sine_pcm_and_freezes_transport()
     {
        let (project, track) = fixture();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let a = instrument.prepare_note(60, 100).unwrap();
        let b = instrument.prepare_note(64, 80).unwrap();
        for partition in [1, 17, 127, 256] {
            let input = Arc::new(ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 100, 1))).unwrap();
            input.set_key(1, Some((64, 80, 2))).unwrap();
            let mut playback = playback(&project, track, &input);
            let mut output = [[0.0; 2]; 512];
            for chunk in output.chunks_mut(partition) {
                playback.render(chunk).unwrap();
            }
            for (age, actual) in output.into_iter().enumerate() {
                let x = a.sample(age as u64, u64::MAX);
                let y = b.sample(age as u64, u64::MAX);
                assert_eq!(actual, [x[0] + y[0], x[1] + y[1]]);
            }
            input.set_key(0, None).unwrap();
            input.set_key(1, None).unwrap();
            let mut release = [[0.0; 2]; 3000];
            for chunk in release.chunks_mut(partition) {
                playback.render(chunk).unwrap();
            }
            for (offset, actual) in release.into_iter().enumerate() {
                let x = a.sample(512 + offset as u64, 512);
                let y = b.sample(512 + offset as u64, 512);
                assert_eq!(actual, [x[0] + y[0], x[1] + y[1]]);
            }
            assert!(!playback.playing());
            assert_eq!(playback.position(), 0);
            assert_eq!(playback.performance_frame(), 0);
        }
    }
    #[test]
    fn computer_keyboard_cancel_stop_and_bank_replacement_never_retrigger_old_held_keys() {
        let (project, track) = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut old = playback(&project, track, &input);
        old.apply(PlaybackCommand::Play);
        old.render(&mut [[0.0; 2]; 256]).unwrap();
        assert!(!input.is_cancelled());
        let mut new = Playback::from_prepared(prepared(&project));
        new.try_inherit_runtime_from(&mut old).unwrap();
        assert!(
            input.is_cancelled(),
            "successful same-revision handoff must end the old lease"
        );
        let mut silent = [[1.0; 2]; 256];
        new.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 256]);
        let fresh = Arc::new(ComputerKeyboardInput::default());
        fresh.set_key(0, Some((60, 100, 1))).unwrap();
        new.prepare_computer_keyboard(&project, track, Arc::clone(&fresh))
            .unwrap();
        new.render(&mut silent).unwrap();
        assert!(silent.iter().flatten().any(|sample| sample.abs() > 1e-6));
        new.apply(PlaybackCommand::Stop);
        assert!(fresh.is_cancelled());
        new.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 256]);
        assert_eq!(new.position(), 0);
    }
    #[test]
    fn computer_keyboard_voice_pressure_cancels_only_live_pool_and_does_not_stick() {
        let (project, track) = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        let mut playback = playback(&project, track, &input);
        for token in 1..=80 {
            input.set_key(0, Some((60, 100, token))).unwrap();
            playback.render(&mut [[0.0; 2]; 1]).unwrap();
        }
        assert!(input.rejected());
        let mut output = [[1.0; 2]; 256];
        playback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 256]);
    }
    #[test]
    fn computer_keyboard_source_binding_refuses_mismatched_instrument_and_rate() {
        let (project, track) = fixture();
        let mut playback = Playback::from_prepared(prepared(&project));
        let mut snapshot = project.snapshot();
        snapshot.sample_rate = 44_100;
        assert!(
            playback
                .prepare_computer_keyboard(
                    &Project::from_snapshot(snapshot).unwrap(),
                    track,
                    Arc::new(ComputerKeyboardInput::default())
                )
                .is_err()
        );
        let mut editor = Editor::new(project, 4).unwrap();
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        assert!(
            playback
                .prepare_computer_keyboard(
                    editor.project(),
                    track,
                    Arc::new(ComputerKeyboardInput::default())
                )
                .is_err()
        );
    }
    #[test]
    fn computer_keyboard_invalid_payloads_and_cancelled_late_preparation_are_silent() {
        let (project, track) = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        assert!(input.set_key(17, Some((60, 100, 1))).is_err());
        assert!(input.set_key(0, Some((60, 99, 1))).is_err());
        assert!(input.set_key(0, Some((60, 100, 0))).is_err());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        input.cancel();
        let mut playback = playback(&project, track, &input);
        let mut output = [[1.0; 2]; 256];
        playback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 256]);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::{
        PreparedRevision,
        playback::{Playback, PlaybackCommand},
    };
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    use std::collections::BTreeMap;
    fn prepare(project: &Project) -> Arc<PreparedRevision> {
        Arc::new(
            PreparedRevision::prepare(
                project,
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    }
    #[test]
    fn computer_keyboard_graph_preparation_preserves_sounding_authored_note_history() {
        let (source, track) = super::tests::fixture();
        let mut editor = Editor::new(source, 16).unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 48,
                velocity: 80,
                channel: 0,
            })
            .unwrap();
        let source = editor.project();
        let revision = prepare(source);
        let mut old = Playback::from_prepared(Arc::clone(&revision));
        let mut oracle = Playback::from_prepared(Arc::clone(&revision));
        for runtime in [&mut old, &mut oracle] {
            runtime.apply(PlaybackCommand::Play);
            runtime.render(&mut [[0.0; 2]; 257]).unwrap();
        }
        let mut candidate = Playback::from_prepared(revision);
        let input = Arc::new(ComputerKeyboardInput::default());
        candidate
            .prepare_computer_keyboard(source, track, Arc::clone(&input))
            .unwrap();
        candidate.try_inherit_runtime_from(&mut old).unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut with_live = [[0.0; 2]; 256];
        let mut authored = with_live;
        candidate.render(&mut with_live).unwrap();
        oracle.render(&mut authored).unwrap();
        assert_ne!(with_live, authored);
        input.cancel();
        candidate.render(&mut with_live).unwrap();
        oracle.render(&mut authored).unwrap();
        assert_eq!(
            with_live, authored,
            "live cancellation must preserve the authored sounding note"
        );
        assert_eq!(
            (candidate.position(), candidate.performance_frame()),
            (769, 769)
        );
    }
    #[test]
    fn computer_keyboard_stopped_live_audio_follows_typed_routing_and_one_suffix() {
        use hex_devices::utility::{UTILITY_DEFINITION, parameter};
        let (source, track) = super::tests::fixture();
        let mut editor = Editor::new(source, 16).unwrap();
        let state = UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                parameter::INVERT_LEFT,
                ParameterValue::Bool(true),
            )
            .unwrap();
        for _ in 0..2 {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
        }
        let mut snapshot = editor.project().snapshot();
        snapshot.tracks[0].instrument_position = Some(1);
        let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
        let Created::Track(destination) = editor
            .execute(Command::AddTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("bus")
        };
        editor
            .execute(Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Track(track),
                destination,
                gain: 0.5,
            })
            .unwrap();
        let source = editor.project();
        let mut playback = Playback::from_prepared(prepare(source));
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        playback
            .prepare_computer_keyboard(source, track, input)
            .unwrap();
        let mut output = [[0.0; 2]; 512];
        playback.render(&mut output).unwrap();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let note = instrument.prepare_note(60, 100).unwrap();
        for (age, actual) in output.into_iter().enumerate() {
            let sample = note.sample(age as u64, u64::MAX)[0];
            assert!(
                (actual[0] + sample * 1.5).abs() < 1e-6,
                "left at {age}: {actual:?} vs {sample}"
            );
            assert!((actual[1] - sample * 1.5).abs() < 1e-6);
        }
        assert_eq!((playback.position(), playback.performance_frame()), (0, 0));
    }
    #[test]
    fn computer_keyboard_mutable_operator_has_polyphony_and_bounded_release() {
        let (source, track) = super::tests::fixture();
        let metadata = &hex_devices::operator_state::OPERATOR_METADATA;
        let mut state = metadata.default_state().unwrap();
        state = metadata
            .patch_parameter(
                &state,
                ParameterId::from_static(78),
                ParameterValue::Float(0.4),
            )
            .unwrap();
        let mut editor = Editor::new(source, 8).unwrap();
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
        let source = editor.project();
        let mut playback = Playback::from_prepared(prepare(source));
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        input.set_key(1, Some((64, 80, 2))).unwrap();
        playback
            .prepare_computer_keyboard(source, track, Arc::clone(&input))
            .unwrap();
        let mut output = [[0.0; 2]; 256];
        playback.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        input.set_key(0, None).unwrap();
        input.set_key(1, None).unwrap();
        for _ in 0..2000 {
            playback.render(&mut output).unwrap();
        }
        assert_eq!(output, [[0.0; 2]; 256]);
        assert!(!input.rejected());
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    use crate::{
        MidiTiming, PreparedTempoMap,
        prepared_tempo::{
            AnchoredSessionTempoView, SessionBaseTimingView, SessionClockAnchor, SessionTimingView,
        },
    };
    use hex_lattice::NativeInstrumentClock;
    use hex_project::{BeatTime, Tempo, TempoMap, TempoPoint};
    #[test]
    fn computer_keyboard_musical_clock_uses_nonzero_performance_and_anchors_monitor_at_current_tempo()
     {
        let map = PreparedTempoMap::prepare(
            TempoMap::new(
                48_000,
                vec![
                    TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
                    TempoPoint::new(
                        BeatTime::from_quarters(1).unwrap(),
                        Tempo::new(60_000_000).unwrap(),
                    ),
                    TempoPoint::new(
                        BeatTime::from_quarters(5).unwrap(),
                        Tempo::new(30_000_000).unwrap(),
                    ),
                ],
            )
            .unwrap(),
            48_000,
        )
        .unwrap();
        let performance = map
            .beat_to_frame(BeatTime::from_quarters(4).unwrap())
            .unwrap();
        let anchor = SessionClockAnchor {
            cut_frame: performance,
            old_beat: BeatTime::from_quarters(2).unwrap(),
            new_beat: BeatTime::from_quarters(4).unwrap(),
        };
        let timing = SessionTimingView::Anchored(AnchoredSessionTempoView::new(
            SessionBaseTimingView::Canonical(&map),
            anchor,
        ));
        let (project, track) = super::tests::fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        let mut runtime = ComputerKeyboardRuntime::prepare(&project, track, input).unwrap();
        runtime.begin_block();
        let clock = runtime.musical_clock(&timing, performance, true).unwrap();
        assert_eq!(
            clock.beat_at_offset(0),
            Some(BeatTime::from_quarters(2).unwrap().raw())
        );
        assert_eq!(
            clock.beat_at_offset(48_000),
            timing
                .frame_to_beat(performance + 48_000)
                .ok()
                .map(|beat| beat.raw())
        );
        let clock = runtime.musical_clock(&timing, performance, false).unwrap();
        assert_eq!(
            clock.beat_at_offset(96_000),
            Some(BeatTime::from_quarters(4).unwrap().raw()),
            "stopped monitor stays at60bpm beyond future30bpm marker"
        );
        runtime.scratch_age = 48_000;
        let clock = runtime.musical_clock(&timing, performance, false).unwrap();
        assert_eq!(
            clock.beat_at_offset(0),
            Some(BeatTime::from_quarters(3).unwrap().raw())
        );
        runtime.rollback();
        runtime.begin_block();
        assert!(
            runtime.scratch_monitor_anchor.is_none(),
            "failed monitor block must not retain clock anchor"
        );
    }
}

#[cfg(test)]
mod pitch_domain_tests {
    use super::*;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    #[test]
    fn computer_keyboard_mutable_nyquist_refusal_keeps_audio_render_usable() {
        let (source, track) = super::tests::fixture();
        let mut snapshot = source.snapshot();
        snapshot.sample_rate = 8000;
        let mut source =
            hex_project::Editor::new(hex_project::Project::from_snapshot(snapshot).unwrap(), 4)
                .unwrap();
        let metadata = &hex_devices::operator_state::OPERATOR_METADATA;
        let state = metadata
            .patch_parameter(
                &metadata.default_state().unwrap(),
                ParameterId::from_static(78),
                ParameterValue::Float(0.4),
            )
            .unwrap();
        let device = source
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        source
            .execute(hex_project::Command::ReplaceDeviceState { device, state })
            .unwrap();
        let revision = Arc::new(
            crate::PreparedRevision::prepare(
                source.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut playback = crate::playback::Playback::from_prepared(revision);
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((127, 100, 1))).unwrap();
        playback
            .prepare_computer_keyboard(source.project(), track, Arc::clone(&input))
            .unwrap();
        let mut frames = [[1.0; 2]; 256];
        playback.render(&mut frames).unwrap();
        assert!(input.rejected());
        assert_eq!(frames, [[0.0; 2]; 256]);
        input.set_key(0, Some((60, 100, 2))).unwrap();
        playback.render(&mut frames).unwrap();
        assert!(frames.iter().flatten().any(|sample| sample.abs() > 1e-6));
    }
}
