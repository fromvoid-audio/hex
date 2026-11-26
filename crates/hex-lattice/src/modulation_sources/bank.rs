use super::*;

struct Source {
    key: (ModulationSourceId, ModulationOutputId),
    sample_rate: f64,
    initial: SourceState,
    committed: SourceState,
    scratch: SourceState,
    samples: Vec<f32>,
}

/// Source state and scratch storage borrowed exclusively by the graph owner.
pub struct ModulationSourceBank {
    sources: Vec<Source>,
    tempo_cache: Vec<u32>,
    note_gate: Option<Box<NoteGate>>,
    chunk_frames: usize,
    last_frames: usize,
    active: bool,
    follows_input: bool,
    follows_notes: bool,
}
impl ModulationSourceBank {
    pub(crate) fn quoted_heap_bytes(
        sources: usize,
        frames: usize,
        note_gate: bool,
    ) -> Option<usize> {
        sources
            .checked_mul(std::mem::size_of::<Source>())?
            .checked_add(
                sources
                    .checked_mul(frames)?
                    .checked_mul(std::mem::size_of::<f32>())?,
            )?
            .checked_add(if sources == 0 {
                0
            } else {
                frames.checked_mul(std::mem::size_of::<u32>())?
            })
            .and_then(|bytes| {
                bytes.checked_add(if note_gate {
                    NoteGate::quoted_heap_bytes(frames)?
                } else {
                    0
                })
            })
    }
    pub(crate) fn matches_prepared(
        &self,
        sources: &[PreparedModulationSource],
        chunk_frames: usize,
    ) -> bool {
        self.chunk_frames == chunk_frames
            && self.sources.len() == sources.len()
            && self.sources.iter().zip(sources).all(|(runtime, prepared)| {
                runtime.key == prepared.key()
                    && runtime.sample_rate == prepared.sample_rate
                    && runtime.initial == prepared.initial
            })
    }

    pub(crate) fn same_declaration(&self, other: &Self) -> bool {
        self.chunk_frames == other.chunk_frames
            && self.sources.len() == other.sources.len()
            && self
                .sources
                .iter()
                .zip(&other.sources)
                .all(|(left, right)| {
                    left.key == right.key
                        && left.sample_rate == right.sample_rate
                        && left.initial == right.initial
                })
    }

    pub(crate) fn validate_sources(
        sources: &[PreparedModulationSource],
        chunk_frames: usize,
    ) -> Result<(), Error> {
        if sources.len() > MAX_MODULATION_SOURCE_SOURCES
            || chunk_frames == 0
            || chunk_frames > MAX_MODULATION_SOURCE_CHUNK_FRAMES
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let minimum = Self::quoted_heap_bytes(
            sources.len(),
            chunk_frames,
            sources
                .iter()
                .any(PreparedModulationSource::reads_note_gate),
        )
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
        .ok_or(Error::RuntimeBudgetExceeded)?;
        if minimum > MAX_MODULATION_SOURCE_STORAGE_BYTES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        if sources
            .windows(2)
            .any(|pair| pair[0].key() >= pair[1].key())
        {
            return Err(Error::InvalidId);
        }
        Ok(())
    }

    /// Source and output pairs must be unique and strictly increasing. Each
    /// prepared output uses one of the existing bounded signal buffers. The host
    /// accounts this storage before publication.
    pub fn new(sources: &[PreparedModulationSource], chunk_frames: usize) -> Result<Self, Error> {
        Self::validate_sources(sources, chunk_frames)?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(sources.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        for source in sources {
            let mut samples = Vec::new();
            samples
                .try_reserve_exact(chunk_frames)
                .map_err(|_| Error::RuntimeAllocation)?;
            samples.resize(chunk_frames, 0.0);
            prepared.push(Source {
                key: source.key(),
                sample_rate: source.sample_rate,
                initial: source.initial,
                committed: source.initial,
                scratch: source.initial,
                samples,
            });
        }
        let mut tempo_cache = Vec::new();
        let cache_frames = if sources.is_empty() { 0 } else { chunk_frames };
        tempo_cache
            .try_reserve_exact(cache_frames)
            .map_err(|_| Error::RuntimeAllocation)?;
        tempo_cache.resize(cache_frames, 0);
        let note_gate = sources
            .iter()
            .any(PreparedModulationSource::reads_note_gate)
            .then(|| NoteGate::new(chunk_frames))
            .transpose()?;
        let bank = Self {
            sources: prepared,
            tempo_cache,
            note_gate,
            chunk_frames,
            last_frames: 0,
            active: false,
            follows_input: sources
                .iter()
                .any(PreparedModulationSource::reads_audio_input),
            follows_notes: sources
                .iter()
                .any(PreparedModulationSource::reads_note_events),
        };
        if bank
            .storage_bytes()?
            .checked_add(std::mem::size_of::<Self>())
            .is_none_or(|bytes| bytes > MAX_MODULATION_SOURCE_STORAGE_BYTES)
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        Ok(bank)
    }

    /// Reserved vector capacities for host admission, excluding the inline
    /// bank header and allocator bookkeeping.
    pub fn storage_bytes(&self) -> Result<usize, Error> {
        let mut bytes = self
            .sources
            .capacity()
            .checked_mul(std::mem::size_of::<Source>())
            .ok_or(Error::RuntimeBudgetExceeded)?;
        bytes = bytes
            .checked_add(
                self.tempo_cache
                    .capacity()
                    .checked_mul(std::mem::size_of::<u32>())
                    .ok_or(Error::RuntimeBudgetExceeded)?,
            )
            .ok_or(Error::RuntimeBudgetExceeded)?;
        for source in &self.sources {
            bytes = source
                .samples
                .capacity()
                .checked_mul(std::mem::size_of::<f32>())
                .and_then(|size| size.checked_add(bytes))
                .ok_or(Error::RuntimeBudgetExceeded)?;
        }
        if let Some(note_gate) = &self.note_gate {
            bytes = note_gate
                .storage_bytes()
                .and_then(|gate| bytes.checked_add(gate))
                .ok_or(Error::RuntimeBudgetExceeded)?;
        }
        Ok(bytes)
    }

    /// Source count for comparing this bank with an admitted route program.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// The stable identity at this bank's sorted, transient source index.
    pub fn source_id(&self, index: usize) -> Option<ModulationSourceId> {
        self.source_key(index).map(|(id, _)| id)
    }

    pub(crate) fn source_key(
        &self,
        index: usize,
    ) -> Option<(ModulationSourceId, ModulationOutputId)> {
        self.sources.get(index).map(|source| source.key)
    }

    /// Maximum frames in one render chunk; longer segments require chunking.
    pub fn chunk_frames(&self) -> usize {
        self.chunk_frames
    }

    /// Start or restart a transaction from committed source state.
    pub fn begin(&mut self) {
        for source in &mut self.sources {
            source.scratch = source.committed;
        }
        if let Some(gate) = &mut self.note_gate {
            gate.begin();
        }
        self.last_frames = 0;
        self.active = true;
    }

    /// Generate one chunk. Refusal leaves both phase and last samples untouched.
    pub fn render_chunk(&mut self, frames: usize) -> Result<(), Error> {
        self.render_chunk_with_clock(frames, None)
    }

    /// Tempo sources follow the borrowed sample-local clock, retaining phase.
    /// Missing tempo uses the prepared fallback; invalid tempo refuses the
    /// entire chunk before source state or the previous samples are changed.
    pub fn render_chunk_with_clock(
        &mut self,
        frames: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), Error> {
        self.render_chunk_with_input(frames, clock, None)
    }

    pub(crate) fn follows_input(&self) -> bool {
        self.follows_input
    }

    pub(crate) fn render_chunk_with_input(
        &mut self,
        frames: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        input: Option<&[StereoFrame]>,
    ) -> Result<(), Error> {
        self.render_chunk_with_routes(frames, clock, input, None, &[], 0)
    }

    pub(crate) fn render_chunk_with_routes(
        &mut self,
        frames: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        input: Option<&[StereoFrame]>,
        note_events: Option<&[NativeInstrumentEvent]>,
        routes: &[PreparedSourceRoute],
        target_mask: u32,
    ) -> Result<(), Error> {
        if !self.active || frames > self.chunk_frames {
            return Err(Error::InvalidParameterValue);
        }
        if self
            .sources
            .iter()
            .any(|source| source.samples.len() < frames)
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        if self.follows_input
            && input.is_none_or(|input| {
                input.len() != frames || input.iter().flatten().any(|sample| !sample.is_finite())
            })
        {
            return Err(Error::InvalidParameterValue);
        }
        if self.follows_notes {
            validate_events(note_events.ok_or(Error::InvalidParameterValue)?, frames)?;
        }
        let follows_tempo = self
            .sources
            .iter()
            .any(|source| source.scratch.tempo_timing().is_some());
        let constant_tempo = if follows_tempo {
            if let Some(clock) = clock {
                match clock.constant_tempo_micro_bpm(0, frames) {
                    Some(0) => return Err(Error::InvalidParameterValue),
                    Some(tempo) => {
                        for source in &self.sources {
                            if let Some(timing) = source.scratch.tempo_timing() {
                                timing.rate(source.sample_rate, tempo)?;
                            }
                        }
                        Some(tempo)
                    }
                    None => {
                        let cache = self
                            .tempo_cache
                            .get_mut(..frames)
                            .ok_or(Error::RuntimeBudgetExceeded)?;
                        for (offset, cached) in cache.iter_mut().enumerate() {
                            *cached = match clock.tempo_micro_bpm_at_offset(offset) {
                                Some(0) => return Err(Error::InvalidParameterValue),
                                Some(tempo) => tempo,
                                None => 0,
                            };
                        }
                        for source in &self.sources {
                            if let Some(timing) = source.scratch.tempo_timing() {
                                for &tempo in cache.iter() {
                                    timing.rate(source.sample_rate, tempo)?;
                                }
                            }
                        }
                        None
                    }
                }
            } else {
                Some(0)
            }
        } else {
            None
        };
        let gate = self
            .note_gate
            .as_mut()
            .map(|gate| gate.render(frames, note_events.ok_or(Error::InvalidParameterValue)?))
            .transpose()?;
        for (index, source) in self.sources.iter_mut().enumerate() {
            if target_mask & (1 << index) != 0 {
                continue;
            }
            let output = source
                .samples
                .get_mut(..frames)
                .ok_or(Error::RuntimeBudgetExceeded)?;
            match &mut source.scratch {
                SourceState::Lfo {
                    oscillator,
                    timing: Timing::Free(authored),
                } => {
                    oscillator.set_rate(*authored);
                    oscillator.render(output);
                }
                SourceState::Lfo { oscillator, timing } => {
                    if let Some(tempo) = constant_tempo {
                        oscillator.set_rate(timing.rate(source.sample_rate, tempo)?);
                        oscillator.render(output);
                    } else {
                        let tempos = self
                            .tempo_cache
                            .get(..frames)
                            .ok_or(Error::RuntimeBudgetExceeded)?;
                        for (sample, &tempo) in output.iter_mut().zip(tempos) {
                            oscillator.set_rate(timing.rate(source.sample_rate, tempo)?);
                            *sample = oscillator.next_sample();
                        }
                    }
                }
                SourceState::Follower(follower) => follower
                    .render(input.ok_or(Error::InvalidParameterValue)?, output)
                    .map_err(|_| Error::InvalidParameterValue)?,
                SourceState::AudioRate => {
                    let input = input.ok_or(Error::InvalidParameterValue)?;
                    for (&[left, right], sample) in input.iter().zip(output) {
                        // Scale before addition to avoid overflow for finite input.
                        *sample = (left * 0.5 + right * 0.5).clamp(-1.0, 1.0);
                    }
                }
                SourceState::Macro(value) => output.fill(*value),
                SourceState::Math(controls) => output.fill(controls.sample([0.0; 2])),
                SourceState::Mix(controls) => output.fill(controls.sample([0.0; 3])),
                SourceState::NoteCounter(counter) => {
                    counter.render(note_events.ok_or(Error::InvalidParameterValue)?, output);
                }
                SourceState::Ramp(ramp) => {
                    ramp.render(note_events.ok_or(Error::InvalidParameterValue)?, output);
                }
                SourceState::AhdRelease(envelope) => {
                    envelope.render(note_events.ok_or(Error::InvalidParameterValue)?, output);
                }
                SourceState::Adsr(envelope) => {
                    envelope.render(gate.ok_or(Error::InvalidParameterValue)?, output);
                }
                SourceState::Random(random) => random.render(output),
                SourceState::Steps(steps) => steps.render(output),
                SourceState::TransportSteps(steps) => steps.render(clock, output),
            }
        }
        if let Err(error) = self.render_routed_sources(frames, routes, constant_tempo) {
            self.rollback();
            return Err(error);
        }
        self.last_frames = frames;
        Ok(())
    }

    /// Upstream buffers are complete before this target renders. Each group
    /// names one target output and excludes self edges.
    fn render_routed_sources(
        &mut self,
        frames: usize,
        routes: &[PreparedSourceRoute],
        constant_tempo: Option<u32>,
    ) -> Result<(), Error> {
        for incoming in routes.chunk_by(|a, b| a.target == b.target) {
            let Some(route) = incoming.first() else {
                continue;
            };
            let index = route.target;
            let (before, target_and_after) = self
                .sources
                .split_at_mut_checked(index)
                .ok_or(Error::InvalidDescriptor)?;
            let (target, after) = target_and_after
                .split_first_mut()
                .ok_or(Error::InvalidDescriptor)?;
            let contribution = |route: &PreparedSourceRoute, frame| {
                let source = if route.source < index {
                    &before[route.source]
                } else {
                    &after[route.source - index - 1]
                };
                route.contribution.sample(source.samples[frame])
            };
            let sum_at = |frame| -> f64 {
                incoming
                    .iter()
                    .map(|route| contribution(route, frame))
                    .sum()
            };
            let output = target
                .samples
                .get_mut(..frames)
                .ok_or(Error::RuntimeBudgetExceeded)?;
            match (route.parameter, &mut target.scratch) {
                (SourceModulationParameter::MacroValue, SourceState::Macro(base)) => {
                    for (frame, sample) in output.iter_mut().enumerate() {
                        let sum = sum_at(frame);
                        *sample = if sum == 0.0 {
                            *base
                        } else {
                            (f64::from(*base) + sum).clamp(0.0, 1.0) as f32
                        };
                    }
                }
                (
                    SourceModulationParameter::MathA | SourceModulationParameter::MathB,
                    SourceState::Math(controls),
                ) => {
                    for (frame, sample) in output.iter_mut().enumerate() {
                        let mut offsets = [0.0; 2];
                        for route in incoming {
                            let input =
                                usize::from(route.parameter == SourceModulationParameter::MathB);
                            offsets[input] += contribution(route, frame);
                        }
                        *sample = controls.sample(offsets);
                    }
                }
                (
                    SourceModulationParameter::MixA
                    | SourceModulationParameter::MixB
                    | SourceModulationParameter::MixBlend,
                    SourceState::Mix(controls),
                ) => {
                    for (frame, sample) in output.iter_mut().enumerate() {
                        let mut offsets = [0.0; 3];
                        for route in incoming {
                            let input = match route.parameter {
                                SourceModulationParameter::MixA => 0,
                                SourceModulationParameter::MixB => 1,
                                SourceModulationParameter::MixBlend => 2,
                                _ => return Err(Error::InvalidDescriptor),
                            };
                            offsets[input] += contribution(route, frame);
                        }
                        *sample = controls.sample(offsets);
                    }
                }
                (
                    SourceModulationParameter::FreeLfoRate,
                    SourceState::Lfo {
                        oscillator,
                        timing: Timing::Free(authored),
                    },
                ) => {
                    render_rate_route(oscillator, output, |_| Ok(*authored), sum_at)?;
                }
                (
                    SourceModulationParameter::TempoLfoRate,
                    SourceState::Lfo {
                        oscillator,
                        timing: timing @ Timing::Tempo { .. },
                    },
                ) => {
                    if let Some(tempo) = constant_tempo {
                        let base_rate = timing.rate(target.sample_rate, tempo)?;
                        render_rate_route(oscillator, output, |_| Ok(base_rate), sum_at)?;
                    } else {
                        let tempos = self
                            .tempo_cache
                            .get(..frames)
                            .ok_or(Error::RuntimeBudgetExceeded)?;
                        render_rate_route(
                            oscillator,
                            output,
                            |frame| timing.rate(target.sample_rate, tempos[frame]),
                            sum_at,
                        )?;
                    }
                }
                _ => return Err(Error::UnsupportedProcessingClass),
            }
        }
        Ok(())
    }

    /// Change parameters at a chunk boundary, preserving speculative history.
    /// Rollback restores the previously committed parameters and history.
    pub fn retarget(&mut self, prepared: PreparedModulationSource) -> Result<(), Error> {
        if !self.active {
            return Err(Error::InvalidParameterValue);
        }
        let index = self
            .sources
            .binary_search_by_key(&prepared.key(), |source| source.key)
            .map_err(|_| Error::InvalidId)?;
        let source = self.sources.get_mut(index).ok_or(Error::InvalidId)?;
        if source.sample_rate != prepared.sample_rate {
            return Err(Error::InvalidParameterValue);
        }
        let mut next = prepared.initial;
        if !next.inherit_history(source.scratch) {
            return Err(Error::UnsupportedProcessingClass);
        }
        source.scratch = next;
        Ok(())
    }

    /// Borrow the last rendered chunk of a source's MAIN output.
    pub fn samples(&self, id: ModulationSourceId) -> Option<&[f32]> {
        self.samples_for_output(id, ModulationOutputId::MAIN)
    }

    /// Borrow the last rendered chunk of an exact source/output pair.
    pub fn samples_for_output(
        &self,
        id: ModulationSourceId,
        output: ModulationOutputId,
    ) -> Option<&[f32]> {
        let index = self
            .sources
            .binary_search_by_key(&(id, output), |source| source.key)
            .ok()?;
        self.samples_at_index(index)
    }

    pub(crate) fn samples_at_index(&self, index: usize) -> Option<&[f32]> {
        if !self.active {
            return None;
        }
        self.sources.get(index)?.samples.get(..self.last_frames)
    }

    /// Install all speculative phases with the enclosing graph transaction.
    pub fn commit(&mut self) -> Result<(), Error> {
        if !self.active {
            return Err(Error::InvalidParameterValue);
        }
        for source in &mut self.sources {
            source.committed = source.scratch;
        }
        if let Some(gate) = &mut self.note_gate {
            gate.commit();
        }
        self.active = false;
        self.last_frames = 0;
        Ok(())
    }

    /// Abandon a speculative transaction; the next begin starts at committed phase.
    pub fn rollback(&mut self) {
        self.active = false;
        self.last_frames = 0;
    }

    pub(crate) fn reset_all(&mut self) {
        for source in &mut self.sources {
            source.committed = source.initial;
            source.scratch = source.initial;
        }
        if let Some(gate) = &mut self.note_gate {
            gate.reset();
        }
        self.active = false;
        self.last_frames = 0;
    }

    /// Copy committed history by identity while retaining candidate parameters.
    /// Called only by the existing graph owner after its replacement preflight.
    pub fn inherit_committed(&mut self, previous: &Self) -> Result<(), Error> {
        if self.active || previous.active {
            return Err(Error::InvalidParameterValue);
        }
        self.inherit_graph_committed(previous);
        Ok(())
    }

    pub(crate) fn inherit_graph_committed(&mut self, previous: &Self) {
        for source in &mut self.sources {
            if let Some(old) = previous
                .sources
                .iter()
                .find(|old| old.key == source.key && old.sample_rate == source.sample_rate)
                && source.committed.inherit_history(old.committed)
            {
                source.scratch = source.committed;
            }
        }
        if let Some(gate) = &mut self.note_gate {
            gate.reset();
            if let Some(previous) = &previous.note_gate {
                gate.inherit(previous);
            }
        }
        self.active = false;
        self.last_frames = 0;
    }

    /// Explicitly reset MAIN after a discontinuity, outside a speculative block.
    pub fn reset(&mut self, id: ModulationSourceId, phase: f64) -> Result<(), Error> {
        if self.active {
            return Err(Error::InvalidParameterValue);
        }
        let index = self
            .sources
            .binary_search_by_key(&(id, ModulationOutputId::MAIN), |source| source.key)
            .map_err(|_| Error::InvalidId)?;
        let source = self.sources.get_mut(index).ok_or(Error::InvalidId)?;
        let SourceState::Lfo { oscillator, .. } = &mut source.committed else {
            return Err(Error::UnsupportedProcessingClass);
        };
        oscillator
            .reset(phase)
            .map_err(|_| Error::InvalidParameterValue)?;
        source.scratch = source.committed;
        Ok(())
    }
}
