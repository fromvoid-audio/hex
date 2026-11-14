//! Input-only scheduling. No source DSP or analytic context is constructed;
//! the receiver owns synthesis while this owner retains ingress and capture.
use super::*;
use crate::mutable_instrument::{LiveInputId, MutableInstrumentTrackRuntime};

pub(super) struct SourceInputPreparation<'a> {
    pub(super) runtime: Option<&'a MutableInstrumentTrackRuntime>,
    pub(super) context: Option<Arc<crate::shared_analytic::SharedContextOwner>>,
    pub(super) capacity: usize,
}
pub(super) struct SourceInputState {
    pub(super) id: LiveInputId,
    pub(super) capacity: usize,
    pub(super) mutable: bool,
    pub(super) note_pressure: bool,
    pub(super) context: Option<Arc<crate::shared_analytic::SharedContextOwner>>,
    pub(super) events: [NativeInstrumentEvent; 259],
    pub(super) len: usize,
    pub(super) release: bool,
    pub(super) scratch_release: bool,
}
impl SourceInputState {
    pub(super) fn prepare(
        track: TrackId,
        source: &SourceInputPreparation<'_>,
    ) -> Result<Box<Self>, Invalid> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let generation = NEXT
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |next| next.checked_add(1),
            )
            .map_err(|_| Invalid("live source identity exhausted"))?;
        if source.capacity == 0 {
            return Err(Invalid("source has no live voice capacity"));
        }
        Ok(Box::new(Self {
            id: LiveInputId { track, generation },
            capacity: source.capacity,
            mutable: source.runtime.is_some(),
            note_pressure: source
                .runtime
                .is_some_and(MutableInstrumentTrackRuntime::supports_note_pressure),
            context: source.context.clone(),
            events: [NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 0,
            }; 259],
            len: 0,
            release: false,
            scratch_release: false,
        }))
    }
}
#[derive(Clone, Copy)]
pub(crate) struct AnalyticSourceVoice {
    pub(crate) input: crate::mutable_instrument::LiveInputId,
    pub(crate) token: u64,
    pub(crate) note: PreparedSharedAnalyticNote,
    pub(crate) age_frames: u64,
    pub(crate) gate_frame: Option<u64>,
}
impl ComputerKeyboardRuntime {
    pub(crate) fn validate_source_tuning_lease(
        &mut self,
        family: crate::prepared_revision::TuningContinuationFamily,
        hint: &ComputerKeyboardTuningHint,
    ) -> Result<(), Invalid> {
        if hint.family != family
            || hint.track != self.track
            || self
                .input
                .lease()
                .is_none_or(|input| !Arc::ptr_eq(input, &hint.input) || input.is_cancelled())
        {
            return Err(Invalid("computer keyboard tuning lease is stale"));
        }
        self.tuning_family = Some(family);
        self.tuning_preservation = true;
        self.preserve_on_handoff = true;
        Ok(())
    }

    pub(crate) fn has_mutable_source_input_voices(&self) -> bool {
        self.source_input
            .as_ref()
            .is_some_and(|source| source.mutable)
            && self.primary.iter().any(Option::is_some)
    }
    pub(crate) fn has_active_analytic_source_voices(&self) -> bool {
        self.source_input
            .as_ref()
            .is_some_and(|source| !source.mutable)
            && self.primary.iter().any(Option::is_some)
    }
    pub(crate) fn analytic_source_voices(
        &self,
    ) -> Option<([Option<AnalyticSourceVoice>; VOICES], usize)> {
        let source = self
            .source_input
            .as_ref()
            .filter(|source| !source.mutable)?;
        let mut result = [None; VOICES];
        let mut len = 0;
        for voice in self.primary.iter().flatten() {
            let Sampler::Shared(note) = self.samplers.get(voice.sampler).copied().flatten()? else {
                return None;
            };
            result[len] = Some(AnalyticSourceVoice {
                input: source.id,
                token: voice.id,
                note,
                age_frames: voice.age,
                gate_frame: voice.gate,
            });
            len += 1;
        }
        Some((result, len))
    }
    pub(crate) fn prepare_source_input(
        project: &hex_project::Project,
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
        source: Option<&MutableInstrumentTrackRuntime>,
        context: Option<Arc<crate::shared_analytic::SharedContextOwner>>,
        capacity: usize,
    ) -> Result<Self, Invalid> {
        let device = project
            .track(track)
            .and_then(hex_project::Track::instrument)
            .filter(|device| !device.bypassed())
            .ok_or(Invalid("live source is unavailable"))?;
        let registry = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("instrument registry is unavailable"))?;
        let tuning = project.tuning().table().map_err(|error| Invalid(error.0))?;
        let instrument = if source.is_none() {
            Some(
                registry
                    .prepare_instrument(
                        device.state(),
                        &hex_lattice::DevicePreparationContext {
                            sample_rate: project.sample_rate(),
                            tuning: &tuning,
                            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                        },
                    )
                    .map_err(|_| Invalid("live source note preparation failed"))?,
            )
        } else {
            None
        };
        let prepared = SourceInputPreparation {
            runtime: source,
            context,
            capacity,
        };
        let mut keyboard = Self::prepare_from_capabilities(
            track,
            input,
            project.sample_rate(),
            instrument.as_deref(),
            None,
            None,
            Some(&prepared),
        )?;
        keyboard.instrument_id = Some(device.id());
        keyboard.tuning = tuning;
        Ok(keyboard)
    }
    /// The next span contains at most one ordered ingress transition at its
    /// first sample. Otherwise synthesis may use the ordinary 256-frame query.
    /// The boundary sample reserves live press/release work separately from a
    /// full authored event window, so NoteOff cannot compete with new presses.
    pub(crate) fn source_input_span(&self, max_frames: usize) -> usize {
        if !self.requires_source_event_frames() {
            return max_frames;
        }
        let live = self
            .live_batch
            .as_ref()
            .is_some_and(|batch| self.live_consumed < batch.events().len());
        let keys = self.capture.as_ref().is_some_and(|capture| {
            capture.key_batch.as_ref().is_some_and(|batch| {
                batch
                    .events
                    .get(capture.keys_consumed)
                    .is_some_and(Option::is_some)
            })
        });
        if self.boundary || live || keys {
            max_frames.min(1)
        } else {
            max_frames
        }
    }
    pub(crate) fn requires_source_event_frames(&self) -> bool {
        self.source_input
            .as_ref()
            .is_some_and(|source| source.mutable)
    }
    pub(crate) fn is_source_input(&self) -> bool {
        self.source_input.is_some()
    }
    pub(crate) fn source_input_id(&self) -> Option<LiveInputId> {
        self.source_input.as_ref().map(|source| source.id)
    }
    pub(crate) fn source_input_events(&self) -> &[NativeInstrumentEvent] {
        self.source_input
            .as_ref()
            .map_or(&[], |source| &source.events[..source.len])
    }
    /// Retain only a physical press the muted source actually suppressed.
    /// The speculative voice pool commits or rolls back with keyboard input;
    /// this is not proof that a later source owns the same live runtime.
    pub(crate) fn mark_muted_source_onset(&mut self, id: u64) {
        if let Some(voice) = self.scratch.iter_mut().flatten().find(|voice| {
            voice.id == id
                && voice.source == VoiceSource::Physical
                && voice.gate.is_none()
                && !voice.muted_source_onset
        }) {
            voice.muted_source_onset = true;
        }
    }
    #[cfg(test)]
    pub(crate) fn muted_source_held_count(&self) -> usize {
        self.primary
            .iter()
            .flatten()
            .filter(|voice| voice.muted_source_onset && voice.gate.is_none())
            .count()
    }
    #[cfg(test)]
    pub(crate) fn held_physical_id(&self) -> Option<u64> {
        self.primary
            .iter()
            .flatten()
            .find(|voice| voice.source == VoiceSource::Physical && voice.gate.is_none())
            .map(|voice| voice.id)
    }
    pub(crate) fn take_source_input_release(&mut self) -> bool {
        self.source_input
            .as_mut()
            .is_some_and(|source| std::mem::take(&mut source.scratch_release))
    }
    pub(crate) fn set_source_capacity(&mut self, capacity: usize) {
        if let Some(source) = &mut self.source_input {
            source.capacity = capacity.min(VOICES);
        }
    }
    pub(super) fn voice_capacity(&self) -> usize {
        self.source_input.as_ref().map_or_else(
            || {
                self.mutable
                    .as_ref()
                    .map_or(VOICES, NativeInstrumentRuntimePair::max_active_occurrences)
            },
            |source| source.capacity,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::Playback;
    use hex_lattice_contract::ParameterValue;
    use hex_project::{Command, Editor};

    fn short_operator_project() -> (hex_project::Project, hex_project::TrackId) {
        let (project, track) = crate::computer_keyboard::tests::fixture();
        let registry = hex_devices::native_instrument_registry().unwrap();
        let mut state = registry
            .default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)
            .unwrap();
        for operator in 1..=4 {
            state = registry
                .patch_parameter(
                    &state,
                    hex_devices::operator_state::parameter::envelope(
                        operator,
                        hex_devices::operator_state::parameter::EnvelopeField::Release,
                    ),
                    ParameterValue::Float(0.001),
                )
                .unwrap();
        }
        let device = project.track(track).unwrap().instrument().unwrap().id();
        let mut editor = Editor::new(project, 8).unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        (editor.project().clone(), track)
    }

    #[test]
    fn replacing_input_on_one_playback_releases_only_the_old_source_generation() {
        let (project, track) = short_operator_project();
        let project = &project;
        let revision = crate::computer_keyboard::tests::prepared(project);
        let tail = revision
            .session()
            .mutable_instrument_factory(track)
            .unwrap()
            .max_release_tail_frames();
        assert!(tail < 16000, "short release fixture");
        let input_a = Arc::new(ComputerKeyboardInput::default());
        let input_b = Arc::new(ComputerKeyboardInput::default());
        let reference_input = Arc::new(ComputerKeyboardInput::default());
        input_a.set_key(0, Some((60, 100, 1))).unwrap();
        reference_input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut actual = Playback::from_prepared(Arc::clone(&revision));
        let mut reference = Playback::from_prepared(revision);
        actual
            .prepare_computer_keyboard(project, track, Arc::clone(&input_a))
            .unwrap();
        reference
            .prepare_computer_keyboard(project, track, Arc::clone(&reference_input))
            .unwrap();
        actual.render(&mut [[0.0; 2]; 512]).unwrap();
        reference.render(&mut [[0.0; 2]; 512]).unwrap();
        input_b.set_key(0, Some((64, 100, 1))).unwrap();
        reference_input.set_key(0, Some((64, 100, 2))).unwrap();
        actual
            .prepare_computer_keyboard(project, track, Arc::clone(&input_b))
            .unwrap();
        input_a.cancel();
        let mut a = [[0.0; 2]; 1024];
        let mut b = a;
        actual.render(&mut a).unwrap();
        reference.render(&mut b).unwrap();
        assert_eq!(a, b, "replacement is one old Off plus the new input's On");
        assert!(a.iter().flatten().any(|sample| sample.abs() > 0.000_001));
        input_b.set_key(0, None).unwrap();
        reference_input.set_key(0, None).unwrap();
        let mut a = vec![[0.0; 2]; tail as usize + 512];
        let mut b = a.clone();
        actual.render(&mut a).unwrap();
        reference.render(&mut b).unwrap();
        assert_eq!(a, b);
        assert!(
            a[a.len() - 128..]
                .iter()
                .flatten()
                .all(|sample| sample.abs() < 0.000_001)
        );
    }
    #[test]
    fn stopped_seek_monitors_live_input_without_destination_clip_history() {
        use crate::playback::PlaybackCommand;
        use hex_project::{BeatTime, Created};
        let (project, track) = short_operator_project();
        // Oracle: the same graph without the clip, so only live input sounds.
        let silent_clip_revision = crate::computer_keyboard::tests::prepared(&project);
        let mut editor = Editor::new(project, 8).unwrap();
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
                duration: BeatTime::from_quarters(3).unwrap(),
                pitch: 60,
                velocity: 100,
                channel: 0,
            })
            .unwrap();
        let project = editor.project();
        let revision = crate::computer_keyboard::tests::prepared(project);
        let primed = crate::SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
            Arc::clone(&revision),
            512,
            512,
            false,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        let mut actual = Playback::from_interactive_arrangement_preroll(primed).unwrap();
        let mut frozen = [[1.0; 2]; 19];
        actual.render(&mut frozen).unwrap();
        assert_eq!(frozen, [[0.0; 2]; 19]);
        // A stopped seek starts voices empty; the held clip note is chased on
        // Play, not released into live monitoring.
        let mut expected = Playback::from_prepared(silent_clip_revision);
        expected.apply(PlaybackCommand::Seek(512));
        let input = Arc::new(ComputerKeyboardInput::default());
        let reference = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((64, 100, 1))).unwrap();
        reference.set_key(0, Some((64, 100, 1))).unwrap();
        actual
            .prepare_computer_keyboard(project, track, input)
            .unwrap();
        expected
            .prepare_computer_keyboard(project, track, reference)
            .unwrap();
        let mut a = [[0.0; 2]; 512];
        let mut b = a;
        actual.render(&mut a).unwrap();
        expected.render(&mut b).unwrap();
        assert_eq!(a, b, "only the live input sounds while stopped");
        assert!(a.iter().flatten().any(|sample| sample.abs() > 0.000_001));
        assert_eq!(actual.performance_frame(), 512);
        assert_eq!(expected.performance_frame(), 512);
    }
}
