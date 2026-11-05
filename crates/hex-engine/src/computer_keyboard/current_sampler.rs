//! Constructor-only keyboard pair using the same input scheduler and sampler DSP.
use super::*;
use hex_lattice::PreparedMutableInstrumentRuntime;

impl ComputerKeyboardRuntime {
    pub(crate) fn prepare_current_sampler(
        track: TrackId,
        device: hex_project::DeviceId,
        factory: &hex_devices::sampler::PreparedSamplerRuntime,
        sample_rate: u32,
        resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<Box<Self>, Invalid> {
        #[cfg(test)]
        CURRENT_SAMPLER_CONSTRUCTION_ENTRIES.with(|n| n.set(n.get() + 1));
        let mutable = NativeInstrumentRuntimePair::new(
            factory,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY,
                max_active_occurrences: VOICES,
                max_tail_frames: crate::mutable_instrument::max_mutable_tail_frames(sample_rate)
                    .ok_or(Invalid("sampler keyboard tail budget is unavailable"))?,
            },
        )
        .map_err(|_| Invalid("sampler keyboard runtime exceeds its budget"))?;
        let mut supported = SupportedNotes::Computer([false; COMPUTER_NOTES]);
        for (v, velocity) in COMPUTER_KEY_VELOCITIES.into_iter().enumerate() {
            for pitch in 0..128_u8 {
                supported[v * 128 + usize::from(pitch)] = mutable
                    .preflight_event_batch(
                        &[NativeInstrumentEvent::NoteOn {
                            frame_offset: 0,
                            occurrence_id: 1,
                            pitch,
                            velocity,
                        }],
                        1,
                        resources,
                    )
                    .is_ok();
            }
        }
        Ok(Box::new(Self {
            resources: None,
            track,
            instrument_id: Some(device),
            expression: None,
            input: KeyboardInput::Current {
                words: [0; COMPUTER_KEY_COUNT],
                cancelled: true,
                rejected: false,
            },
            samplers: Vec::new(),
            supported,
            context: None,
            mutable: Some(mutable),
            source_input: None,
            retarget: None,
            tuning_family: None,
            tuning_preservation: false,
            tuning: Default::default(),
            release_frames: factory.max_release_tail_frames(),
            primary: [None; VOICES],
            scratch: [None; VOICES],
            desired: [0; COMPUTER_KEY_COUNT],
            rejected_desired: [0; COMPUTER_KEY_COUNT],
            scratch_rejected_desired: [0; COMPUTER_KEY_COUNT],
            activation_epoch: None,
            next_id: 0,
            scratch_next_id: 0,
            age: 0,
            scratch_age: 0,
            audition_start: None,
            scratch_audition_start: None,
            monitor_anchor: None,
            scratch_monitor_anchor: None,
            boundary: false,
            faulted: false,
            preserve_on_handoff: false,
            lease_detached: false,
            live_batch: None,
            live_consumed: 0,
            live_activation: None,
            scratch_live_activation: None,
            capture: None,
        }))
    }
    pub(crate) fn current_sampler_preflight_bytes() -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(2 * std::mem::size_of::<hex_devices::sampler::SamplerRuntime>())
    }
    pub(crate) fn visit_current_sampler_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        if !matches!(self.input, KeyboardInput::Current { .. })
            || !self.samplers.is_empty()
            || self.samplers.capacity() != 0
            || self.context.is_some()
            || self.source_input.is_some()
            || self.retarget.is_some()
            || self.expression.is_some()
            || self.tuning_family.is_some()
        {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        let pair = self
            .mutable
            .as_ref()
            .ok_or(crate::AudioStorageError::UnsupportedGraph)?;
        let (first, second) = pair
            .inline_storage_for_type::<hex_devices::sampler::SamplerRuntime>()
            .ok_or(crate::AudioStorageError::UnsupportedGraph)?;
        ledger.bytes(
            crate::audio_storage::Owner::exclusive(self),
            crate::audio_storage::Location("current-keyboard-runtime", 0),
            std::mem::size_of::<Self>(),
        )?;
        ledger.bytes(
            crate::audio_storage::Owner::exclusive(pair),
            crate::audio_storage::Location("current-keyboard-primary", 0),
            first,
        )?;
        ledger.bytes(
            crate::audio_storage::Owner::exclusive(pair),
            crate::audio_storage::Location("current-keyboard-scratch", 0),
            second,
        )
    }
    pub(crate) fn set_current_words(
        &mut self,
        words: [u64; COMPUTER_KEY_COUNT],
    ) -> Result<(), Invalid> {
        if words.iter().any(|word| !valid_key_word(*word)) {
            return Err(Invalid("sampler keyboard words are invalid"));
        }
        if self.faulted {
            return Err(Invalid("sampler keyboard activation is faulted"));
        }
        let KeyboardInput::Current {
            words: target,
            cancelled,
            rejected,
        } = &mut self.input
        else {
            return Err(Invalid("sampler keyboard input is unavailable"));
        };
        *target = words;
        *cancelled = false;
        *rejected = false;
        Ok(())
    }
    pub(crate) fn is_current_target(&self, track: u64, device: u64) -> bool {
        self.track.value() == track
            && self.instrument_id.is_some_and(|id| id.value() == device)
            && matches!(self.input, KeyboardInput::Current { .. })
    }
    pub(crate) fn reset_current_activation(&mut self) {
        self.cancel();
        self.desired.fill(0);
        self.faulted = false;
        self.next_id = 0;
        self.scratch_next_id = 0;
        self.age = 0;
        self.scratch_age = 0;
        self.monitor_anchor = None;
        self.scratch_monitor_anchor = None;
    }
    pub(crate) fn current_rejected(&self) -> bool {
        matches!(self.input, KeyboardInput::Current { rejected: true, .. }) || self.faulted
    }
}
