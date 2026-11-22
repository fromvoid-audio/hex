//! Offline whole-project renderer and runtime stage execution.
use crate::{
    Frame, Invalid, MainOutputObserver, MediaPages, PreparedTrackMix, Project, Track,
    prepared_tempo, signal_graph, source_effect_stage,
};
use std::sync::Arc;

pub(crate) struct RendererRuntime {
    pub(crate) device_chains: Vec<hex_lattice::NativeRuntimeChain>,
    pub(crate) source_effects: source_effect_stage::SourceEffectStages,
    pub(crate) return_chains: Vec<hex_lattice::NativeRuntimeChain>,
    pub(crate) main_chain: hex_lattice::NativeRuntimeChain,
    pub(crate) return_scratch: Vec<[Frame; 256]>,
    pub(crate) runtime_error: Option<&'static str>,
    pub(crate) compensation: Option<signal_graph::SignalRuntime>,
}
impl RendererRuntime {
    pub(crate) fn new(project: &Project) -> Self {
        Self::with_prepared_source_effects(project, &project.source_effects)
    }

    /// Offline control-side stage construction from the same source recipe
    /// retained for Session playback.
    pub(crate) fn with_prepared_source_effects(
        project: &Project,
        recipes: &[source_effect_stage::PreparedSourceEffects],
    ) -> Self {
        let mut device_chains = Vec::new();
        let mut return_chains = Vec::new();
        let mut return_scratch = Vec::new();
        let mut runtime_error = None;
        let source_effects =
            match source_effect_stage::SourceEffectStages::prepare(project, recipes) {
                Ok(stages) => stages,
                Err(error) => {
                    return Self {
                        device_chains,
                        source_effects: source_effect_stage::SourceEffectStages::default(),
                        return_chains,
                        main_chain: hex_lattice::NativeRuntimeChain::empty(),
                        return_scratch,
                        runtime_error: Some(error.0),
                        compensation: None,
                    };
                }
            };
        let main_chain = match project.new_main_device_runtime() {
            Ok(chain) => chain,
            Err(_) => {
                runtime_error = Some("main runtime preparation failed");
                hex_lattice::NativeRuntimeChain::empty()
            }
        };
        let compensation = match project
            .signal_graph
            .as_ref()
            .filter(|graph| graph.needs_runtime())
            .map(|graph| signal_graph::SignalRuntime::new(Arc::clone(graph)))
            .transpose()
        {
            Ok(compensation) => compensation,
            Err(_) => {
                runtime_error = Some("compensation runtime storage is unavailable");
                None
            }
        };
        if device_chains
            .try_reserve_exact(project.tracks.len())
            .is_err()
        {
            runtime_error = Some("native device runtime allocation failed");
        } else {
            for track in &project.tracks {
                match track.new_device_runtime() {
                    Ok(chain) => device_chains.push(chain),
                    Err(_) => {
                        runtime_error = Some("native device runtime preparation failed");
                        device_chains.clear();
                        break;
                    }
                }
            }
        }
        if runtime_error.is_none() {
            if return_chains
                .try_reserve_exact(project.return_buses.len())
                .is_err()
                || return_scratch
                    .try_reserve_exact(project.return_buses.len())
                    .is_err()
            {
                runtime_error = Some("return runtime allocation failed");
            } else {
                for bus in &project.return_buses {
                    let mut instances = Vec::new();
                    if instances.try_reserve_exact(bus.device_chain.len()).is_err() {
                        runtime_error = Some("return device allocation failed");
                        return Self {
                            device_chains,
                            source_effects,
                            return_chains,
                            main_chain,
                            return_scratch,
                            runtime_error,
                            compensation,
                        };
                    }
                    instances.extend(bus.device_chain.iter().cloned());
                    match hex_lattice::NativeRuntimeChain::new_prepared(instances) {
                        Ok(chain) => {
                            return_chains.push(chain);
                            return_scratch.push([[0.0; 2]; 256]);
                        }
                        Err(_) => {
                            runtime_error = Some("return runtime preparation failed");
                            return_chains.clear();
                            return_scratch.clear();
                            break;
                        }
                    }
                }
            }
        }
        Self {
            device_chains,
            source_effects,
            return_chains,
            main_chain,
            return_scratch,
            runtime_error,
            compensation,
        }
    }

    pub(crate) fn reset(&mut self) {
        if let Some(compensation) = self.compensation.as_mut() {
            compensation.delays.reset_epoch();
        }
        for chain in &mut self.device_chains {
            chain.reset();
        }
        for chain in self.source_effects.chains_mut() {
            chain.reset();
        }
        for chain in &mut self.return_chains {
            chain.reset();
        }
        self.main_chain.reset();
    }

    pub(crate) fn select_processor_audition(
        &mut self,
        project: &Project,
        identity: crate::PostNodeIdentity,
        selection: Option<hex_lattice::ProcessorAuditionSelection>,
    ) -> bool {
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Source(_) => false,
            hex_lattice::ProcessorChainOwner::Main => self.main_chain.select_processor_audition(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection,
            ),
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.select_processor_audition(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.select_processor_audition(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
        }
    }

    pub(crate) fn processor_audition_is_active(
        &mut self,
        project: &Project,
        identity: crate::PostNodeIdentity,
        selection: hex_lattice::ProcessorAuditionSelection,
    ) -> bool {
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Source(_) => false,
            hex_lattice::ProcessorChainOwner::Main => self.main_chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection,
            ),
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.processor_audition_is_active(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.processor_audition_is_active(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
        }
    }

    pub(crate) fn cancel_processor_audition(
        &mut self,
        project: &Project,
        identity: crate::PostNodeIdentity,
    ) -> bool {
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Source(_) => false,
            hex_lattice::ProcessorChainOwner::Main => {
                self.main_chain.cancel_processor_audition_for_identity(
                    identity.owner,
                    identity.device_id,
                    identity.device_type,
                )
            }
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.cancel_processor_audition_for_identity(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.cancel_processor_audition_for_identity(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                    )
                }),
        }
    }

    pub(crate) fn cancel_all_processor_auditions(&mut self) {
        for chain in &mut self.device_chains {
            chain.cancel_all_processor_auditions();
        }
        for chain in &mut self.return_chains {
            chain.cancel_all_processor_auditions();
        }
        self.main_chain.cancel_all_processor_auditions();
    }

    pub(crate) fn has_stateful_processors(&self) -> bool {
        self.main_chain.has_stateful_processors()
            || self.source_effects.has_stateful_processors()
            || self
                .device_chains
                .iter()
                .any(hex_lattice::NativeRuntimeChain::has_stateful_processors)
            || self
                .return_chains
                .iter()
                .any(hex_lattice::NativeRuntimeChain::has_stateful_processors)
    }

    /// Settle this paused (previous) linear graph's committed transitions
    /// that `candidate` is busy from (Pulse #4558), pairing chains exactly as
    /// `transition_busy_from`; bounded and allocation-free.
    pub(crate) fn settle_idle_transitions_for(
        &mut self,
        project: &Project,
        candidate: &Self,
        candidate_project: &Project,
    ) {
        for (index, track) in candidate_project.tracks().iter().enumerate() {
            let Some(track_id) = track.canonical_track_id() else {
                continue;
            };
            if let Some((old, chain)) = project
                .tracks()
                .iter()
                .position(|old| old.canonical_track_id() == Some(track_id))
                .and_then(|old| self.device_chains.get_mut(old))
                .zip(candidate.device_chains.get(index))
            {
                old.settle_idle_transitions_for(chain);
            }
        }
        for (index, bus) in candidate_project.return_buses().iter().enumerate() {
            if let Some((old, chain)) = project
                .return_buses()
                .iter()
                .position(|old| old.id == bus.id)
                .and_then(|old| self.return_chains.get_mut(old))
                .zip(candidate.return_chains.get(index))
            {
                old.settle_idle_transitions_for(chain);
            }
        }
        self.main_chain
            .settle_idle_transitions_for(&candidate.main_chain);
    }

    /// Whether a same-identity chain must defer this handoff until the
    /// predecessor's committed live transitions drain (Pulse #4178). Pairs
    /// chains exactly as `can_inherit_from`; read-only and allocation-free.
    pub(crate) fn transition_busy_from(
        &self,
        project: &Project,
        previous: &Self,
        previous_project: &Project,
    ) -> bool {
        let tracks = project.tracks().iter().enumerate().any(|(index, track)| {
            track.canonical_track_id().is_some_and(|track_id| {
                previous_project
                    .tracks()
                    .iter()
                    .position(|old| old.canonical_track_id() == Some(track_id))
                    .and_then(|old| previous.device_chains.get(old))
                    .zip(self.device_chains.get(index))
                    .is_some_and(|(old, candidate)| candidate.transition_busy_from(old))
            })
        });
        let returns = project
            .return_buses()
            .iter()
            .enumerate()
            .any(|(index, bus)| {
                previous_project
                    .return_buses()
                    .iter()
                    .position(|old| old.id == bus.id)
                    .and_then(|old| previous.return_chains.get(old))
                    .zip(self.return_chains.get(index))
                    .is_some_and(|(old, candidate)| candidate.transition_busy_from(old))
            });
        tracks || returns || self.main_chain.transition_busy_from(&previous.main_chain)
    }

    pub(crate) fn can_inherit_from(
        &self,
        project: &Project,
        previous: &Self,
        previous_project: &Project,
    ) -> bool {
        let tracks_match = project.tracks().iter().enumerate().all(|(index, track)| {
            let Some(track_id) = track.canonical_track_id() else {
                return true;
            };
            let Some(previous_index) = previous_project
                .tracks()
                .iter()
                .position(|old_track| old_track.canonical_track_id() == Some(track_id))
            else {
                return true;
            };
            match (
                self.device_chains.get(index),
                previous.device_chains.get(previous_index),
            ) {
                (Some(candidate), Some(old)) => candidate.can_inherit_from(old),
                _ => false,
            }
        });
        tracks_match
            && self.main_chain.can_inherit_from(&previous.main_chain)
            && project
                .return_buses()
                .iter()
                .enumerate()
                .all(|(index, bus)| {
                    let Some(previous_index) = previous_project
                        .return_buses()
                        .iter()
                        .position(|old_bus| old_bus.id == bus.id)
                    else {
                        return true;
                    };
                    match (
                        self.return_chains.get(index),
                        previous.return_chains.get(previous_index),
                    ) {
                        (Some(candidate), Some(old)) => candidate.can_inherit_from(old),
                        _ => false,
                    }
                })
    }

    pub(crate) fn inherit_from(
        &mut self,
        project: &Project,
        previous: &Self,
        previous_project: &Project,
    ) {
        self.main_chain.inherit_from(&previous.main_chain);
        for (index, track) in project.tracks().iter().enumerate() {
            let Some(track_id) = track.canonical_track_id() else {
                continue;
            };
            let Some(previous_index) = previous_project
                .tracks()
                .iter()
                .position(|old_track| old_track.canonical_track_id() == Some(track_id))
            else {
                continue;
            };
            if let (Some(candidate), Some(old)) = (
                self.device_chains.get_mut(index),
                previous.device_chains.get(previous_index),
            ) {
                candidate.inherit_from(old);
            }
        }
        for (index, bus) in project.return_buses().iter().enumerate() {
            let Some(previous_index) = previous_project
                .return_buses()
                .iter()
                .position(|old_bus| old_bus.id == bus.id)
            else {
                continue;
            };
            if let (Some(candidate), Some(old)) = (
                self.return_chains.get_mut(index),
                previous.return_chains.get(previous_index),
            ) {
                candidate.inherit_from(old);
            }
        }
    }

    pub(crate) fn render_without_pages(
        &mut self,
        project: &Project,
        position: &mut u64,
        playing: bool,
        output: &mut [Frame],
    ) -> Result<(), Invalid> {
        self.render_with_pages::<dyn MediaPages>(project, position, playing, output, None)
    }

    pub(crate) fn render_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        project: &Project,
        position: &mut u64,
        playing: bool,
        output: &mut [Frame],
        pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        let mut began = false;
        let result =
            self.render_with_pages_inner(project, position, playing, output, pages, &mut began);
        if result.is_err() && began {
            for chain in self
                .device_chains
                .iter_mut()
                .chain(self.return_chains.iter_mut())
                .chain(std::iter::once(&mut self.main_chain))
            {
                chain.rollback_block();
            }
            output.fill([0.0; 2]);
        }
        result
    }

    pub(crate) fn render_with_pages_inner<P: MediaPages + ?Sized>(
        &mut self,
        project: &Project,
        position: &mut u64,
        playing: bool,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        began: &mut bool,
    ) -> Result<(), Invalid> {
        if project.requires_scalar_automation {
            output.fill([0.0; 2]);
            return Err(Invalid(
                "recorded parameters require shared session playback",
            ));
        }
        let volume_clock = project.midi_volume_timing();
        let volume_timing = volume_clock
            .as_ref()
            .map(|clock| clock as &dyn prepared_tempo::MidiTempoBreaks);
        if let Some(message) = self.runtime_error {
            return Err(Invalid(message));
        }
        if project
            .tracks
            .iter()
            .any(|track| track.foreign_instrument.is_some())
        {
            return Err(Invalid("vst3 instrument render owner is unavailable"));
        }
        if project.tracks.iter().any(|track| track.mutable_instrument) {
            return Err(Invalid(
                "mutable instrument requires shared Session playback",
            ));
        }
        let next = if playing {
            position
                .checked_add(output.len() as u64)
                .ok_or(Invalid("transport overflow"))?
        } else {
            *position
        };
        if playing && !output.is_empty() && pages.is_none() && project.contains_streamed_sources() {
            return Err(Invalid("streamed project requires a media page provider"));
        }
        output.fill([0.0; 2]);
        if !playing {
            return Ok(());
        }
        if output.is_empty() {
            return Ok(());
        }
        let block_start = *position;
        *began = true;
        if self.compensation.is_some() {
            self.render_compensated(project, block_start, output, pages.as_deref_mut())?;
            *position = next;
            return Ok(());
        }
        for chain in &mut self.device_chains {
            chain.begin_block();
        }
        for chain in self.source_effects.chains_mut() {
            chain.begin_block();
        }
        self.main_chain.begin_block();
        let has_return_routes = !project.return_buses.is_empty()
            || project.tracks.iter().any(|track| !track.sends.is_empty());
        if has_return_routes {
            for chain in &mut self.return_chains {
                chain.begin_block();
            }
            if let Err(error) =
                self.render_return_routes(project, block_start, output, pages.as_deref_mut())
            {
                output.fill([0.0; 2]);
                return Err(error);
            }
            crate::main_chain::process(
                &mut self.main_chain,
                output,
                0,
                Some(&crate::effect_clock::ArrangementEffectClock::canonical(
                    project.tempo_map.as_ref(),
                    block_start,
                )),
                &mut None,
            )
            .map_err(|_| Invalid("main effect runtime failed"))?;
            for chain in &mut self.device_chains {
                chain.commit_block();
            }
            for chain in self.source_effects.chains_mut() {
                chain.commit_block();
            }
            for chain in &mut self.return_chains {
                chain.commit_block();
            }
            self.main_chain.commit_block();
            project.master_gain.apply(output);
            *position = next;
            return Ok(());
        }
        for (index, track) in project.tracks.iter().enumerate() {
            let stateful = track.has_device_chain();
            let ordered = track.requires_ordered_processing();
            let render_direct_stateless_track = !stateful
                && !ordered
                && !self.source_effects.has_chain(index)
                && !track.has_mutable_instrument()
                && !track.has_gain_automation()
                && !track.source_id().is_some_and(|source| {
                    project
                        .tracks
                        .iter()
                        .filter(|candidate| candidate.source_id() == Some(source))
                        .take(2)
                        .count()
                        > 1
                });
            if render_direct_stateless_track {
                // Keep clip/note contributions in the destination mix order;
                // grouping a track into zeroed scratch changes exact PCM sums.
                track.render_additive_with_suppressions_and_pages(
                    block_start,
                    block_start,
                    output,
                    &[],
                    pages.as_deref_mut(),
                    &[],
                    volume_timing,
                    None,
                )?;
                continue;
            }
            let mut scratch = [[0.0; 2]; 256];
            for offset in (0..output.len()).step_by(scratch.len()) {
                let count = (output.len() - offset).min(scratch.len());
                let start = block_start
                    .checked_add(offset as u64)
                    .ok_or(Invalid("render range overflow"))?;
                let end = start
                    .checked_add(count as u64)
                    .ok_or(Invalid("render range overflow"))?;
                let target = &mut scratch[..count];
                target.fill([0.0; 2]);
                if track.audio_content_enabled() {
                    render_track_audio_raw(track, start, end, target, pages.as_deref_mut());
                }
                let effect_clock = crate::effect_clock::ArrangementEffectClock::canonical(
                    project.tempo_map.as_ref(),
                    start,
                );
                if ordered {
                    if stateful {
                        self.device_chains
                            .get_mut(index)
                            .ok_or(Invalid("native device runtime is unavailable"))?
                            .process_range_with_clock(
                                0..track.instrument_position(),
                                target,
                                Some(&effect_clock),
                                Some(&effect_clock),
                            )
                            .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                    } else {
                        track.apply_ordered_prefix(target);
                    }
                    track.apply_instrument_input_gain(target);
                    self.source_effects.add_generator(
                        project,
                        index,
                        start..end,
                        target,
                        volume_timing,
                    )?;
                    track.apply_instrument_output_gain(target);
                    if stateful {
                        let chain = self
                            .device_chains
                            .get_mut(index)
                            .ok_or(Invalid("native device runtime is unavailable"))?;
                        let effect_count = chain.len();
                        chain
                            .process_range_with_clock(
                                track.instrument_position()..effect_count,
                                target,
                                Some(&effect_clock),
                                Some(&effect_clock),
                            )
                            .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                    } else {
                        track.apply_ordered_suffix(target);
                    }
                } else if stateful {
                    let chain = self
                        .device_chains
                        .get_mut(index)
                        .ok_or(Invalid("native device runtime is unavailable"))?;
                    chain
                        .process_range_with_clock(
                            0..chain.len(),
                            target,
                            Some(&effect_clock),
                            Some(&effect_clock),
                        )
                        .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                }
                if !ordered {
                    self.source_effects.add_generator(
                        project,
                        index,
                        start..end,
                        target,
                        volume_timing,
                    )?;
                }
                for (frame_index, (destination, source)) in output[offset..offset + count]
                    .iter_mut()
                    .zip(target.iter().copied())
                    .enumerate()
                {
                    let mix = if track.has_gain_automation() {
                        track.mix_at(start + frame_index as u64)
                    } else {
                        track.mix
                    };
                    mix.add_frame(destination, source);
                }
            }
        }
        // Neutral projects retain their original hot path. Non-neutral master
        // gain is deliberately post-sum rather than folded into track gains.
        crate::main_chain::process(
            &mut self.main_chain,
            output,
            0,
            Some(&crate::effect_clock::ArrangementEffectClock::canonical(
                project.tempo_map.as_ref(),
                block_start,
            )),
            &mut None,
        )
        .map_err(|_| Invalid("main effect runtime failed"))?;
        for chain in &mut self.device_chains {
            chain.commit_block();
        }
        for chain in self.source_effects.chains_mut() {
            chain.commit_block();
        }
        self.main_chain.commit_block();
        project.master_gain.apply(output);
        *position = next;
        Ok(())
    }

    pub(crate) fn render_return_routes<P: MediaPages + ?Sized>(
        &mut self,
        project: &Project,
        block_start: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        let volume_clock = project.midi_volume_timing();
        let volume_timing = volume_clock
            .as_ref()
            .map(|clock| clock as &dyn prepared_tempo::MidiTempoBreaks);
        for offset in (0..output.len()).step_by(256) {
            let count = (output.len() - offset).min(256);
            let start = block_start
                .checked_add(offset as u64)
                .ok_or(Invalid("render range overflow"))?;
            let end = start
                .checked_add(count as u64)
                .ok_or(Invalid("render range overflow"))?;
            let output_chunk = &mut output[offset..offset + count];
            let effect_clock = crate::effect_clock::ArrangementEffectClock::canonical(
                project.tempo_map.as_ref(),
                start,
            );
            for scratch in &mut self.return_scratch {
                scratch[..count].fill([0.0; 2]);
            }
            for (track_index, track) in project.tracks.iter().enumerate() {
                let stateful = track.has_device_chain();
                let ordered = track.requires_ordered_processing();
                let mut scratch = [[0.0; 2]; 256];
                let rendered = &mut scratch[..count];
                if track.audio_content_enabled() {
                    render_track_audio_raw(track, start, end, rendered, pages.as_deref_mut());
                }
                if ordered {
                    if stateful {
                        self.device_chains
                            .get_mut(track_index)
                            .ok_or(Invalid("native device runtime is unavailable"))?
                            .process_range_with_clock(
                                0..track.instrument_position,
                                rendered,
                                Some(&effect_clock),
                                Some(&effect_clock),
                            )
                            .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                    } else {
                        track.apply_ordered_prefix(rendered);
                    }
                    track.apply_instrument_input_gain(rendered);
                    self.source_effects.add_generator(
                        project,
                        track_index,
                        start..end,
                        rendered,
                        volume_timing,
                    )?;
                    track.apply_instrument_output_gain(rendered);
                    if stateful {
                        let chain = self
                            .device_chains
                            .get_mut(track_index)
                            .ok_or(Invalid("native device runtime is unavailable"))?;
                        let effect_count = chain.len();
                        chain
                            .process_range_with_clock(
                                track.instrument_position..effect_count,
                                rendered,
                                Some(&effect_clock),
                                Some(&effect_clock),
                            )
                            .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                    } else {
                        track.apply_ordered_suffix(rendered);
                    }
                } else if stateful {
                    let chain = self
                        .device_chains
                        .get_mut(track_index)
                        .ok_or(Invalid("native device runtime is unavailable"))?;
                    chain
                        .process_range_with_clock(
                            0..chain.len(),
                            rendered,
                            Some(&effect_clock),
                            Some(&effect_clock),
                        )
                        .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                }
                if !ordered {
                    self.source_effects.add_generator(
                        project,
                        track_index,
                        start..end,
                        rendered,
                        volume_timing,
                    )?;
                }
                for (frame_index, source) in rendered.iter().copied().enumerate() {
                    let mixed = if track.has_gain_automation() {
                        track.mix_at(start + frame_index as u64).apply_frame(source)
                    } else {
                        track.mix.apply_frame(source)
                    };
                    output_chunk[frame_index][0] += mixed[0];
                    output_chunk[frame_index][1] += mixed[1];
                    for send in &track.sends {
                        let target = self
                            .return_scratch
                            .get_mut(send.return_index)
                            .ok_or(Invalid("compiled return send target is missing"))?;
                        target[frame_index][0] += mixed[0] * send.gain;
                        target[frame_index][1] += mixed[1] * send.gain;
                    }
                }
            }
            if !project.return_feed_plan.is_empty() {
                for &index in project.return_feed_plan.order() {
                    let bus = &project.return_buses[index];
                    let frames = &mut self.return_scratch[index][..count];
                    let chain = self
                        .return_chains
                        .get_mut(index)
                        .ok_or(Invalid("return runtime is unavailable"))?;
                    chain
                        .process_range_with_clock(
                            0..chain.len(),
                            frames,
                            Some(&effect_clock),
                            Some(&effect_clock),
                        )
                        .map_err(|_| Invalid("prepared return segment is invalid"))?;
                    if let Some(matrix) = bus.matrix {
                        matrix.apply_block(frames);
                    }
                    if bus.muted {
                        frames.fill([0.0; 2]);
                    } else {
                        for frame in frames {
                            frame[0] *= bus.gain;
                            frame[1] *= bus.gain;
                        }
                    }
                    project
                        .return_feed_plan
                        .propagate(index, &mut self.return_scratch, count);
                }
                // Main always sums in pinned bus order, not dependency order.
                for frames in &self.return_scratch {
                    for (destination, source) in output_chunk.iter_mut().zip(&frames[..count]) {
                        destination[0] += source[0];
                        destination[1] += source[1];
                    }
                }
                continue;
            }
            for (index, bus) in project.return_buses.iter().enumerate() {
                let bus_frames = self
                    .return_scratch
                    .get_mut(index)
                    .ok_or(Invalid("return scratch is unavailable"))?;
                let frames = &mut bus_frames[..count];
                if let Some(chain) = self.return_chains.get_mut(index) {
                    chain
                        .process_range_with_clock(
                            0..chain.len(),
                            frames,
                            Some(&effect_clock),
                            Some(&effect_clock),
                        )
                        .map_err(|_| Invalid("prepared return segment is invalid"))?;
                } else {
                    return Err(Invalid("return runtime is unavailable"));
                }
                if let Some(matrix) = bus.matrix {
                    matrix.apply_block(frames);
                }
                if !bus.muted {
                    for (destination, source) in output_chunk.iter_mut().zip(frames.iter()) {
                        destination[0] += source[0] * bus.gain;
                        destination[1] += source[1] * bus.gain;
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn render_track_audio_raw<P: MediaPages + ?Sized>(
    track: &Track,
    start_frame: u64,
    end_frame: u64,
    output: &mut [Frame],
    pages: Option<&mut P>,
) {
    track.render_audio_range_captured_with_mix(
        start_frame,
        end_frame,
        output,
        pages,
        None,
        PreparedTrackMix::identity(),
        [None; 2],
    );
}

/// Add every enabled contributor's generated signal to the chosen receiver's
/// source slot. The caller places this between local prefix and suffix stages.
pub(crate) fn render_source_generators(
    project: &Project,
    receiver_index: usize,
    start_frame: u64,
    end_frame: u64,
    output: &mut [Frame],
    timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
) -> Result<(), Invalid> {
    let receiver = project
        .tracks
        .get(receiver_index)
        .ok_or(Invalid("source receiver track is missing"))?;
    if !receiver.receives_source_audio() {
        return Ok(());
    }
    for (contributor_index, contributor) in project.tracks.iter().enumerate() {
        let contributes_to_receiver = match receiver.source_id() {
            Some(source) => {
                contributor.source_id() == Some(source)
                    && contributor.source_receiver_index() == Some(receiver_index)
            }
            None => contributor_index == receiver_index && contributor.source_id().is_none(),
        };
        if !contributes_to_receiver || !contributor.source_input_enabled() {
            continue;
        }
        contributor.render_instrument_range_with_mix(
            (start_frame, end_frame),
            output,
            None,
            0,
            &[],
            None,
            &[],
            PreparedTrackMix::identity(),
            timing,
            None,
        )?;
    }
    Ok(())
}

pub struct Renderer<'a> {
    pub(crate) project: &'a Project,
    pub(crate) position: u64,
    pub(crate) playing: bool,
    pub(crate) runtime: RendererRuntime,
}
impl<'a> Renderer<'a> {
    pub fn new(project: &'a Project) -> Self {
        Self {
            project,
            position: 0,
            playing: false,
            runtime: RendererRuntime::new(project),
        }
    }

    /// Enable effect telemetry explicitly for headless diagnostics. Offline
    /// exports leave this disabled; native instrument telemetry uses Playback.
    pub fn set_effect_metering(&mut self, enabled: bool) {
        for chain in self
            .runtime
            .device_chains
            .iter_mut()
            .chain(self.runtime.source_effects.chains_mut())
            .chain(self.runtime.return_chains.iter_mut())
            .chain(std::iter::once(&mut self.runtime.main_chain))
        {
            chain.set_metering(enabled);
        }
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn seek(&mut self, position: u64) {
        if self.position != position {
            self.runtime.reset();
        }
        self.position = position;
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    /// Overwrites the output. Pausing emits silence without advancing.
    /// Runtime storage is prepared by `new`; rendering does not allocate.
    /// On position overflow returns an error without changing output or transport.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        self.runtime
            .render_without_pages(self.project, &mut self.position, self.playing, output)
    }

    /// Observe the same completed render once, without another source pass.
    /// No observation occurs on error; observer failure/panic has no rollback
    /// guarantee because the existing render has already completed.
    pub fn render_with_main_observer(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut MainOutputObserver<'_>>,
    ) -> Result<(), Invalid> {
        self.render(output)?;
        if let Some(observer) = observer {
            observer(output);
        }
        Ok(())
    }

    /// Streamed counterpart; retains the existing source-local miss policy.
    /// The sample borrow is never retained and no telemetry storage is added.
    pub fn render_with_pages_and_main_observer<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut MainOutputObserver<'_>>,
    ) -> Result<(), Invalid> {
        self.render_with_pages(output, pages)?;
        if let Some(observer) = observer {
            observer(output);
        }
        Ok(())
    }

    /// Render with a borrowed page view. Missing streamed pages contribute
    /// silence while transport and other source contributions continue.
    pub fn render_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        self.runtime.render_with_pages(
            self.project,
            &mut self.position,
            self.playing,
            output,
            Some(pages),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Audio, Clip};

    fn project(looping: bool, duration: u64) -> Project {
        let audio = Audio::new(48_000, vec![[1.0, -1.0], [0.5, -0.5]]).unwrap();
        Project::new(
            48_000,
            vec![
                Track::new(
                    vec![Clip::new(audio, 2, duration, looping).unwrap()],
                    0.5,
                    false,
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn scheduled_loop_has_exact_boundaries() {
        let p = project(true, 5);
        let mut r = Renderer::new(&p);
        r.play();
        let mut out = [[9.0; 2]; 8];
        r.render(&mut out).unwrap();
        assert_eq!(
            out,
            [
                [0.0; 2],
                [0.0; 2],
                [0.5, -0.5],
                [0.25, -0.25],
                [0.5, -0.5],
                [0.25, -0.25],
                [0.5, -0.5],
                [0.0; 2]
            ]
        );
    }

    #[test]
    fn one_shot_exhausts_source() {
        let p = project(false, 5);
        let mut r = Renderer::new(&p);
        r.play();
        let mut out = [[0.0; 2]; 8];
        r.render(&mut out).unwrap();
        assert_eq!(out[3], [0.25, -0.25]);
        assert_eq!(out[4..], [[0.0; 2]; 4]);
    }

    #[test]
    fn block_partition_is_invariant() {
        let p = project(true, 20);
        let mut a = Renderer::new(&p);
        let mut b = Renderer::new(&p);
        a.play();
        b.play();
        let mut whole = [[0.0; 2]; 23];
        let mut chunks = whole;
        a.render(&mut whole).unwrap();
        for chunk in chunks.chunks_mut(3) {
            b.render(chunk).unwrap();
        }
        assert_eq!(whole, chunks);
        assert_eq!(a.position(), b.position());
    }

    #[test]
    fn pause_and_seek() {
        let p = project(true, 5);
        let mut r = Renderer::new(&p);
        r.seek(3);
        let mut out = [[9.0; 2]; 1];
        r.render(&mut out).unwrap();
        assert_eq!(out, [[0.0; 2]]);
        assert_eq!(r.position(), 3);
        r.play();
        r.render(&mut out).unwrap();
        assert_eq!(out, [[0.25, -0.25]]);
        r.pause();
        r.render(&mut out).unwrap();
        assert_eq!(r.position(), 4);
    }

    #[test]
    fn mixing_and_mute() {
        let a = Audio::new(48_000, vec![[0.5, -0.5]]).unwrap();
        let c = Clip::new(a, 0, 1, false).unwrap();
        let tracks = vec![
            Track::new(vec![c.clone()], 1.0, false).unwrap(),
            Track::new(vec![c.clone()], 0.5, false).unwrap(),
            Track::new(vec![c], 9.0, true).unwrap(),
        ];
        let p = Project::new(48_000, tracks).unwrap();
        let mut r = Renderer::new(&p);
        r.play();
        let mut out = [[0.0; 2]; 1];
        r.render(&mut out).unwrap();
        assert_eq!(out, [[0.75, -0.75]]);
    }

    #[test]
    fn rejects_render_position_overflow() {
        let p = project(true, 5);
        let mut r = Renderer::new(&p);
        r.seek(u64::MAX);
        r.play();
        let mut out = [[9.0; 2]; 1];
        assert!(r.render(&mut out).is_err());
        assert_eq!(r.position(), u64::MAX);
        assert_eq!(out, [[9.0; 2]]);
    }
}
