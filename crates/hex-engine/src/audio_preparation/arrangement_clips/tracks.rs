//! Sealed control-owned Track targets; no Track/Arc/Clone escape.
mod sampler;
use super::*;
use crate::{
    CompiledSend, PreparedTrackMix, Track,
    gain_envelope::{
        PreparedGainEnvelope,
        persistent::{GainCopyCustody, GainCopyError},
    },
    owned_shared::OwnedShared,
};
pub(crate) use sampler::{
    compiler_quote as sampler_compiler_quote, compiler_workspace_quote as sampler_workspace_bound,
};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrangementTrackCapacity {
    pub row_bytes: usize,
    pub destination_scratch_bytes: usize,
    pub clip_bytes: usize,
    /// Backing of each track's prepared clip index (one entry per clip).
    pub clip_index_bytes: usize,
    pub send_bytes: usize,
    pub gain_bytes: usize,
    pub pair_bytes: usize,
    pub midi_bytes: usize,
    pub meter_payload_bytes: usize,
    pub meter_declared_overhead_bytes: usize,
    pub chain_payload_bytes: usize,
    pub chain_declared_overhead_bytes: usize,
    pub independent_handle_obligations: usize,
    pub reserved_handle_destinations: usize,
    pub previous_growth_backing_bytes: usize,
}
impl ArrangementTrackCapacity {
    pub(crate) fn bytes(self) -> Result<usize, AudioStorageError> {
        [
            self.row_bytes,
            self.destination_scratch_bytes,
            self.clip_bytes,
            self.clip_index_bytes,
            self.send_bytes,
            self.gain_bytes,
            self.pair_bytes,
            self.midi_bytes,
            self.meter_payload_bytes,
            self.meter_declared_overhead_bytes,
            self.chain_payload_bytes,
            self.chain_declared_overhead_bytes,
        ]
        .into_iter()
        .try_fold(0usize, |n, b| {
            n.checked_add(b).ok_or(AudioStorageError::Overflow)
        })
    }
}
struct PairedClipBinding {
    binding: Binding,
    declaration: Option<SharedDeclaration>,
    handle: Option<u64>,
    slot: Option<usize>,
}
struct ClipDestination {
    source: usize,
    origin: Binding,
    owner: Option<PairedClipBinding>,
}
#[derive(Default)]
struct Row {
    ordinal: usize,
    installed_ordinal: Option<usize>,
    id: Option<hex_project::TrackId>,
    source_id: Option<hex_project::SourceId>,
    source_receiver_index: Option<usize>,
    source_audio_receiver: bool,
    source_input_enabled: bool,
    audio_content_enabled: bool,
    mix: Option<PreparedTrackMix>,
    source_mix: Option<PreparedTrackMix>,
    clips: Vec<Clip>,
    clip_index: Vec<crate::clip_index::ClipIndexEntry>,
    pairs: Vec<ClipDestination>,
    sends: Vec<CompiledSend>,
    gain: GainCopyCustody,
    prepared_gain: Option<PreparedGainEnvelope>,
    meter: Option<OwnedShared<hex_lattice::DeviceMeter>>,
    chain: Option<OwnedShared<[hex_lattice::PreparedRuntimeInstance]>>,
    effects: super::chains::ChainCustody,
    sampler: sampler::SamplerTrackConstruction,
    bare_meter: Option<Arc<hex_lattice::DeviceMeter>>,
    bare_chain: Option<Arc<[hex_lattice::PreparedRuntimeInstance]>>,
    meter_declaration: Option<SharedDeclaration>,
    chain_declaration: Option<SharedDeclaration>,
    handles: [u64; 2],
    track: Option<Track>,
    root_index: Option<usize>,
    meter_disposal: Disposal,
    chain_disposal: Disposal,
}
#[derive(Default)]
pub(super) struct TrackAssembly {
    rows: Vec<Row>,
    destinations: Vec<bool>,
    active: usize,
    phase: Phase,
    growth: usize,
    cursor: usize,
    #[cfg(test)]
    pub(super) fail_after: Option<usize>,
    #[cfg(test)]
    pub(super) mismatch: bool,
    #[cfg(test)]
    pub(super) panic_after: Option<usize>,
    #[cfg(test)]
    pub(super) probe: Option<DropProbe>,
    #[cfg(test)]
    pub(super) chain_probe: Option<DropProbe>,
}
impl Row {
    fn actual<'a>(&'a self, root: &'a [Track]) -> Result<Option<&'a Track>, AudioStorageError> {
        if let Some(index) = self.root_index {
            if self.track.is_some() || self.installed_ordinal != Some(index) {
                return Err(AudioStorageError::SourceMismatch);
            }
            let track = root.get(index).ok_or(AudioStorageError::SourceMismatch)?;
            if track.track_id != self.id {
                return Err(AudioStorageError::SourceMismatch);
            }
            Ok(Some(track))
        } else {
            Ok(self.track.as_ref())
        }
    }
}
fn backing<T>(v: &Vec<T>) -> Result<usize, AudioStorageError> {
    v.capacity()
        .checked_mul(mem::size_of::<T>())
        .ok_or(AudioStorageError::Overflow)
}
fn reserve<T>(v: &mut Vec<T>, count: usize, growth: &mut usize) -> Result<(), AudioStorageError> {
    if count > v.capacity() {
        *growth = growth
            .checked_add(backing(v)?)
            .ok_or(AudioStorageError::Overflow)?;
    }
    v.try_reserve_exact(count.saturating_sub(v.len()))
        .map_err(|_| AudioStorageError::Allocation)
}
fn invalid(_: crate::Invalid) -> AudioStorageError {
    AudioStorageError::SourceMismatch
}
fn gain_error(error: GainCopyError) -> AudioStorageError {
    match error {
        GainCopyError::Invalid => AudioStorageError::SourceMismatch,
        GainCopyError::Capacity => AudioStorageError::Capacity,
        GainCopyError::Allocation => AudioStorageError::Allocation,
        GainCopyError::Overflow => AudioStorageError::Overflow,
    }
}
impl TrackAssembly {
    #[cfg(test)]
    pub(super) fn capacity(
        &self,
        growth: bool,
    ) -> Result<ArrangementTrackCapacity, AudioStorageError> {
        self.capacity_at(growth, &[])
    }
    pub(super) fn capacity_at(
        &self,
        growth: bool,
        root: &[Track],
    ) -> Result<ArrangementTrackCapacity, AudioStorageError> {
        let mut c = ArrangementTrackCapacity {
            row_bytes: backing(&self.rows)?,
            destination_scratch_bytes: backing(&self.destinations)?,
            previous_growth_backing_bytes: if growth && self.phase != Phase::Vacant {
                self.growth
            } else {
                0
            },
            ..Default::default()
        };
        for r in &self.rows {
            c.midi_bytes = c
                .midi_bytes
                .checked_add(r.sampler.backing_bytes(r.actual(root)?)?)
                .ok_or(AudioStorageError::Overflow)?;
            let (clips, clip_index, sends, gain) = if let Some(t) = r.actual(root)? {
                if r.clips.capacity() != 0
                    || r.clip_index.capacity() != 0
                    || r.sends.capacity() != 0
                    || r.prepared_gain.is_some()
                    || r.bare_meter.is_some()
                    || r.bare_chain.is_some()
                {
                    return Err(AudioStorageError::UnknownStorage);
                }
                (&t.clips, &t.clip_index, &t.sends, t.gain_envelope.as_ref())
            } else {
                (&r.clips, &r.clip_index, &r.sends, r.prepared_gain.as_ref())
            };
            c.clip_bytes = c
                .clip_bytes
                .checked_add(backing(clips)?)
                .ok_or(AudioStorageError::Overflow)?;
            c.clip_index_bytes = c
                .clip_index_bytes
                .checked_add(backing(clip_index)?)
                .ok_or(AudioStorageError::Overflow)?;
            c.send_bytes = c
                .send_bytes
                .checked_add(backing(sends)?)
                .ok_or(AudioStorageError::Overflow)?;
            c.pair_bytes = c
                .pair_bytes
                .checked_add(backing(&r.pairs)?)
                .ok_or(AudioStorageError::Overflow)?;
            let installed_gain_bytes = gain
                .map_or(Ok(0), PreparedGainEnvelope::checked_backing_bytes)
                .map_err(invalid)?;
            c.gain_bytes = c
                .gain_bytes
                .checked_add(r.gain.bytes().map_err(gain_error)?)
                .and_then(|n| n.checked_add(installed_gain_bytes))
                .ok_or(AudioStorageError::Overflow)?;
            let meter = if let Some(t) = r.actual(root)? {
                r.meter_declaration
                    .as_ref()
                    .and_then(|d| d.parts(&t.instrument_meter))
            } else if let Some(m) = &r.meter {
                m.parts()
            } else {
                r.bare_meter
                    .as_ref()
                    .and_then(|m| r.meter_declaration.as_ref().and_then(|d| d.parts(m)))
            };
            let nested = if let Some(t) = r.actual(root)? {
                r.effects.bytes(&t.device_chain)?
            } else if let Some(chain) = &r.chain {
                r.effects.bytes(chain)?
            } else if let Some(chain) = &r.bare_chain {
                r.effects.bytes(chain)?
            } else {
                r.effects.metadata_bytes()?
            };
            c.chain_payload_bytes = c
                .chain_payload_bytes
                .checked_add(nested)
                .ok_or(AudioStorageError::Overflow)?;
            let chain = if let Some(t) = r.actual(root)? {
                r.chain_declaration
                    .as_ref()
                    .and_then(|d| d.parts(&t.device_chain))
            } else if let Some(m) = &r.chain {
                m.parts()
            } else {
                r.bare_chain
                    .as_ref()
                    .and_then(|m| r.chain_declaration.as_ref().and_then(|d| d.parts(m)))
            };
            if r.meter.is_some()
                || r.bare_meter.is_some()
                || r.root_index.is_some()
                || r.track.is_some()
            {
                let (p, o) = meter.ok_or(AudioStorageError::UnsupportedConstructor)?;
                c.meter_payload_bytes = c
                    .meter_payload_bytes
                    .checked_add(p)
                    .ok_or(AudioStorageError::Overflow)?;
                c.meter_declared_overhead_bytes = c
                    .meter_declared_overhead_bytes
                    .checked_add(o)
                    .ok_or(AudioStorageError::Overflow)?;
            }
            if r.chain.is_some()
                || r.bare_chain.is_some()
                || r.root_index.is_some()
                || r.track.is_some()
            {
                let (p, o) = chain.ok_or(AudioStorageError::UnsupportedConstructor)?;
                c.chain_payload_bytes = c
                    .chain_payload_bytes
                    .checked_add(p)
                    .ok_or(AudioStorageError::Overflow)?;
                c.chain_declared_overhead_bytes = c
                    .chain_declared_overhead_bytes
                    .checked_add(o)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        c.independent_handle_obligations = self
            .rows
            .iter()
            .map(|r| {
                usize::from(
                    r.meter.is_some()
                        || r.bare_meter.is_some()
                        || r.root_index.is_some()
                        || r.track.is_some(),
                ) + usize::from(
                    r.chain.is_some()
                        || r.bare_chain.is_some()
                        || r.root_index.is_some()
                        || r.track.is_some(),
                ) + r.effects.handles()
                    + r.sampler.handles()
            })
            .sum();
        c.reserved_handle_destinations = self
            .active
            .checked_mul(2)
            .and_then(|n| {
                n.checked_add(self.rows.iter().map(|r| r.sampler.handles()).sum::<usize>())
            })
            .ok_or(AudioStorageError::Overflow)?;
        c.bytes()?;
        Ok(c)
    }
    pub(super) fn metadata_boxes(&self, root: &[Track]) -> Result<usize, AudioStorageError> {
        self.rows
            .iter()
            .map(|r| {
                Ok::<_, AudioStorageError>(
                    r.actual(root)?
                        .map_or(r.clips.as_slice(), |t| t.clips.as_slice())
                        .iter()
                        .filter(|c| c.metadata.is_some())
                        .count(),
                )
            })
            .try_fold(0usize, |n, value| {
                n.checked_add(value?).ok_or(AudioStorageError::Overflow)
            })
    }
    pub(super) fn installed_clips(&self, root: &[Track]) -> usize {
        self.rows
            .iter()
            .map(|r| {
                r.actual(root)
                    .ok()
                    .flatten()
                    .map_or(r.clips.len(), |t| t.clips.len())
            })
            .sum()
    }
    pub(super) fn reserve_with_effects(
        &mut self,
        project: &hex_project::Project,
        pairs: &[Pair],
        samplers: Option<(
            &crate::native_device_resources::current_sampler::CurrentSamplerMetadata,
            &OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        )>,
        effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Vacant
            || self.rows.iter().any(|r| {
                r.id.is_some()
                    || r.root_index.is_some()
                    || r.track.is_some()
                    || !r.clips.is_empty()
                    || !r.clip_index.is_empty()
                    || !r.pairs.is_empty()
                    || !r.sends.is_empty()
                    || r.meter.is_some()
                    || r.chain.is_some()
                    || r.bare_meter.is_some()
                    || r.bare_chain.is_some()
                    || r.prepared_gain.is_some()
                    || r.sampler.present()
            })
        {
            return Err(AudioStorageError::Capacity);
        }
        crate::gain_envelope::validate_project(project).map_err(invalid)?;
        if samplers.is_none()
            && project.tracks().iter().any(|t| {
                t.instrument().is_some()
                    || !t.midi_clips().is_empty()
                    || t.devices().iter().any(|d| {
                        effects.is_none()
                            && d.state().type_id() != hex_devices::utility::UTILITY_TYPE_ID
                    })
            })
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        self.phase = Phase::Reserved;
        self.growth = 0;
        self.active = 0;
        let wanted = project.tracks().len();
        reserve(&mut self.rows, wanted, &mut self.growth)?;
        while self.rows.len() < wanted {
            self.rows.push(Row::default());
        }
        self.active = wanted;
        reserve(&mut self.destinations, pairs.len(), &mut self.growth)?;
        self.destinations.resize(pairs.len(), false);
        let solo = crate::TrackSoloSelection::prepare(project);
        // Reserve ALL target backing before any gain fill or owned constructor.
        for (ordinal, (r, t)) in self.rows.iter_mut().zip(project.tracks()).enumerate() {
            r.ordinal = ordinal;
            r.installed_ordinal = None;
            r.id = Some(t.id());
            r.source_id = t.source_id();
            r.source_receiver_index = t
                .source_id()
                .and_then(|id| project.native_source(id))
                .and_then(|source| source.receiver())
                .and_then(|id| project.tracks().iter().position(|track| track.id() == id));
            r.source_audio_receiver = t.receives_source_audio();
            r.source_input_enabled =
                !t.muted() && (!solo.any || solo.selected(ordinal).map_err(invalid)?);
            r.audio_content_enabled = r.source_input_enabled;
            r.meter_disposal = Disposal::Unstarted;
            r.chain_disposal = Disposal::Unstarted;
            r.mix = Some(
                PreparedTrackMix::new(
                    t.gain(),
                    t.balance(),
                    solo.muted(ordinal, t).map_err(invalid)?,
                )
                .map_err(invalid)?,
            );
            r.source_mix = r.mix;
            reserve(&mut r.clips, t.clips().len(), &mut self.growth)?;
            reserve(&mut r.clip_index, t.clips().len(), &mut self.growth)?;
            reserve(&mut r.pairs, t.clips().len(), &mut self.growth)?;
            reserve(&mut r.sends, t.sends().len(), &mut self.growth)?;
            let neutral =
                PreparedTrackMix::new(1.0, t.balance(), solo.muted(ordinal, t).map_err(invalid)?)
                    .map_err(invalid)?;
            r.gain
                .reserve(t.gain_envelope(), project.tempo_map(), neutral, t.gain())
                .map_err(gain_error)?;
            self.growth = self
                .growth
                .checked_add(r.gain.growth)
                .ok_or(AudioStorageError::Overflow)?;
            for clip in t.clips() {
                let source = pairs
                    .iter()
                    .position(|p| p.binding.track == t.id() && p.binding.clip == clip.id())
                    .ok_or(AudioStorageError::SourceMismatch)?;
                r.pairs.push(ClipDestination {
                    source,
                    origin: pairs[source].binding,
                    owner: None,
                });
            }
            for s in t.sends() {
                let i = project
                    .return_tracks()
                    .iter()
                    .position(|x| x.id() == s.return_track())
                    .ok_or(AudioStorageError::SourceMismatch)?;
                r.sends.push(CompiledSend {
                    return_index: i,
                    gain: s.gain(),
                });
            }
            r.handles = [handle()?, handle()?];
            if !t.devices().is_empty() || !t.sends().is_empty() || !t.gain_envelope().is_empty() {
                r.source_mix = Some(PreparedTrackMix::identity());
            }
        }
        if self.rows[..self.active]
            .iter()
            .map(|r| r.pairs.len())
            .sum::<usize>()
            != pairs.len()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (r, t) in self.rows.iter_mut().zip(project.tracks()) {
            if let Some((metadata, contexts)) = samplers
                && let Some(config) = metadata.configuration(t.id())
            {
                let _ = (config, contexts); // compile only after workspace admission
                r.source_mix = Some(PreparedTrackMix::identity());
            }
            r.gain
                .fill(t.gain_envelope(), project.tempo_map())
                .map_err(gain_error)?;
            r.meter = Some(OwnedShared::new_owned(hex_lattice::DeviceMeter::default()));
            r.chain = Some(r.effects.prepare_with_effects_and_input(
                t.devices(),
                hex_lattice::ProcessorChainOwner::Track(t.id().value()),
                project.sample_rate(),
                effects,
                native_input,
            )?);
            self.growth = self
                .growth
                .checked_add(r.effects.construction_overlap())
                .ok_or(AudioStorageError::Overflow)?;
            let meter = r.meter.as_ref().ok_or(AudioStorageError::UnknownStorage)?;
            let (p, _) = meter
                .parts()
                .ok_or(AudioStorageError::UnsupportedConstructor)?;
            if meter.control_storage_declaration()
                != (hex_lattice::PreparedStorageDeclaration::ExclusiveOnly { bytes: p })
            {
                return Err(AudioStorageError::UnknownStorage);
            }
            r.chain
                .as_ref()
                .and_then(OwnedShared::parts)
                .ok_or(AudioStorageError::UnsupportedConstructor)?;
        }
        #[cfg(test)]
        if self.mismatch
            && let Some(plan) = self.rows.first_mut().and_then(|r| r.pairs.first_mut())
        {
            plan.origin.handle = plan
                .origin
                .handle
                .checked_add(1)
                .ok_or(AudioStorageError::Overflow)?;
        }
        Ok(())
    }
    pub(super) fn compile_current_sampler_tracks(
        &mut self,
        project: &hex_project::Project,
        metadata: &crate::native_device_resources::current_sampler::CurrentSamplerMetadata,
        contexts: &OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    ) -> Result<(), AudioStorageError> {
        for (row, track) in self.rows.iter_mut().zip(project.tracks()) {
            if let Some(config) = metadata.configuration(track.id()) {
                row.sampler
                    .reserve(project, track, config, metadata.id, contexts)?;
            }
        }
        Ok(())
    }
    pub(super) fn construct(&mut self, pairs: &mut [Pair]) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Reserved {
            return Err(AudioStorageError::Capacity);
        }
        if self.destinations.len() != pairs.len() {
            return Err(AudioStorageError::Capacity);
        }
        self.destinations.fill(false);
        // Control-only preallocated bijection scratch; partial marks are not
        // ownership changes and reset before every preflight/retry.
        // Validate entire independent destination batch BEFORE any clip moves.
        for r in &self.rows[..self.active] {
            if r.handles.contains(&0)
                || r.handles[0] == r.handles[1]
                || r.installed_ordinal.is_some()
                || r.track.is_some()
                || !r.clips.is_empty()
                || r.clips.capacity() < r.pairs.len()
                || !r.clip_index.is_empty()
                || r.clip_index.capacity() < r.pairs.len()
                || r.prepared_gain.is_some()
                || r.bare_meter.is_some()
                || r.bare_chain.is_some()
                || r.meter_declaration.is_some()
                || r.chain_declaration.is_some()
                || r.meter.is_none()
                || r.chain.is_none()
                || r.id.is_none()
                || r.mix.is_none()
                || r.source_mix.is_none()
                || !r.gain.can_install(&r.prepared_gain)
            {
                return Err(AudioStorageError::Capacity);
            }
            for plan in &r.pairs {
                let p = pairs
                    .get(plan.source)
                    .ok_or(AudioStorageError::SourceMismatch)?;
                if plan.origin.track != r.id.ok_or(AudioStorageError::SourceMismatch)?
                    || self.destinations[plan.source]
                    || p.binding != plan.origin
                    || p.clip.is_none()
                    || p.installed_handle != Some(plan.origin.handle)
                    || p.installed_slot != Some(plan.origin.slot)
                    || p.transferred
                    || plan.owner.is_some()
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                self.destinations[plan.source] = true;
            }
        }
        if self.destinations.iter().any(|marked| !marked) {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (index, r) in self.rows[..self.active].iter_mut().enumerate() {
            if let Some(m) = r.meter.take() {
                let (a, d) = m.into_paired_parts_on_control();
                r.meter_declaration = Some(d);
                r.bare_meter = Some(a);
            }
            if let Some(c) = r.chain.take() {
                let (a, d) = c.into_paired_parts_on_control();
                r.chain_declaration = Some(d);
                r.bare_chain = Some(a);
            }
            r.gain.install(&mut r.prepared_gain);
            for plan in &mut r.pairs {
                let p = &mut pairs[plan.source];
                if let Some(clip) = p.clip.take() {
                    r.clips.push(clip);
                }
                plan.owner = Some(PairedClipBinding {
                    binding: p.binding,
                    declaration: p.declaration.take(),
                    handle: p.installed_handle.take(),
                    slot: p.installed_slot.take(),
                });
                p.transferred = true;
            }
            // Preflight proved the capacity, so this writes without growing.
            crate::clip_index::fill(&r.clips, Clip::index_bounds, &mut r.clip_index)
                .map_err(|_| AudioStorageError::Capacity)?;
            let has_streamed_clips = r.clips.iter().any(Clip::is_streamed);
            // Only fixed moves after the batch preflight. No public constructors
            // which allocate replacement/default owners or consume owning errors.
            if let (Some(meter), Some(chain), Some(mix), Some(source_mix)) = (
                r.bare_meter.take(),
                r.bare_chain.take(),
                r.mix,
                r.source_mix,
            ) {
                r.track = Some(Track {
                    track_id: r.id,
                    source_id: r.source_id,
                    source_receiver_index: r.source_receiver_index,
                    source_audio_receiver: r.source_audio_receiver,
                    source_input_enabled: r.source_input_enabled,
                    audio_content_enabled: r.audio_content_enabled,
                    clips: mem::take(&mut r.clips),
                    clip_index: mem::take(&mut r.clip_index),
                    has_streamed_clips,
                    mix,
                    source_mix,
                    gain_envelope: r.prepared_gain.take(),
                    instrument: None,
                    instrument_position: 0,
                    ordered_prefix: None,
                    ordered_suffix: None,
                    device_chain: chain,
                    sends: mem::take(&mut r.sends),
                    instrument_id: None,
                    instrument_state: None,
                    tuning_identity: Default::default(),
                    instrument_bypassed: false,
                    instrument_input_gain: 1.0,
                    instrument_output_gain: 1.0,
                    instrument_latency: None,
                    instrument_runtime_latency: None,
                    midi_source_index: None,
                    instrument_meter: meter,
                    mutable_instrument: false,
                    mutable_looping_source: false,
                });
                if let Some(track) = r.track.as_mut() {
                    r.sampler.install(track)?;
                }
            }
            r.installed_ordinal = Some(index);
            #[cfg(test)]
            {
                assert_ne!(
                    self.panic_after,
                    Some(index),
                    "persistent Track construction panic"
                );
                if self.fail_after == Some(index) {
                    return Err(AudioStorageError::Allocation);
                }
            }
        }
        self.phase = Phase::Installed;
        Ok(())
    }
    pub(super) fn has_next(&self) -> bool {
        !matches!(self.phase, Phase::Vacant | Phase::Complete)
    }
    pub(super) fn dispose_next(&mut self, pairs: &mut [Pair]) -> Result<(), AudioStorageError> {
        self.phase = Phase::Cleaning;
        if self.cursor == self.active {
            self.phase = Phase::Complete;
            return Ok(());
        }
        let r = &mut self.rows[self.cursor];
        if let Some(t) = r.track.as_mut()
            && r.sampler.present()
        {
            r.sampler.restore(t)?;
        }
        if let Some(t) = &r.track {
            if r.installed_ordinal != Some(r.ordinal)
                || r.clips.capacity() != 0
                || r.clip_index.capacity() != 0
                || r.sends.capacity() != 0
                || r.prepared_gain.is_some()
                || r.bare_meter.is_some()
                || r.bare_chain.is_some()
                || t.instrument.is_some()
                || t.instrument_state.is_some()
                || t.instrument_id.is_some()
                || t.mutable_instrument
                || t.mutable_looping_source
                || t.ordered_prefix.is_some()
                || t.ordered_suffix.is_some()
                || t.instrument_position != 0
                || t.instrument_bypassed
                || t.instrument_output_gain != 1.0
                || t.track_id != r.id
                || t.clips.len() != r.pairs.len()
            {
                return Err(AudioStorageError::UnknownStorage);
            }
            for plan in &r.pairs {
                let p = pairs
                    .get(plan.source)
                    .ok_or(AudioStorageError::SourceMismatch)?;
                let b = plan
                    .owner
                    .as_ref()
                    .ok_or(AudioStorageError::SourceMismatch)?;
                if !p.transferred
                    || p.binding != plan.origin
                    || p.clip.is_some()
                    || p.metadata.is_some()
                    || p.stream.is_some()
                    || p.installed_handle.is_some()
                    || p.installed_slot.is_some()
                    || p.declaration.is_some()
                    || b.binding != plan.origin
                    || b.handle != Some(plan.origin.handle)
                    || b.slot != Some(plan.origin.slot)
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
            if let Some(Track {
                track_id: _,
                source_id: _,
                source_receiver_index: _,
                source_audio_receiver: _,
                source_input_enabled: _,
                audio_content_enabled: _,
                clips,
                clip_index,
                has_streamed_clips: _,
                mix: _,
                source_mix: _,
                gain_envelope,
                instrument: _,
                instrument_position: _,
                ordered_prefix: _,
                ordered_suffix: _,
                device_chain,
                sends,
                instrument_id: _,
                instrument_state: _,
                tuning_identity: _,
                instrument_bypassed: _,
                instrument_input_gain: _,
                instrument_output_gain: _,
                instrument_latency: _,
                instrument_runtime_latency: _,
                midi_source_index: _,
                instrument_meter,
                mutable_instrument: _,
                mutable_looping_source: _,
            }) = r.track.take()
            {
                r.clips = clips;
                r.clip_index = clip_index;
                r.prepared_gain = gain_envelope;
                r.bare_chain = Some(device_chain);
                r.sends = sends;
                r.bare_meter = Some(instrument_meter);
            }
            // Return exact pair+clip together, no storage free/owner destruction.
            for plan in r.pairs.iter_mut().rev() {
                let p = &mut pairs[plan.source];
                p.clip = r.clips.pop();
                if let Some(b) = plan.owner.take() {
                    p.declaration = b.declaration;
                    p.installed_handle = b.handle;
                    p.installed_slot = b.slot;
                }
                p.transferred = false;
            }
            return Ok(());
        }
        if r.meter_disposal == Disposal::Unstarted {
            r.meter_disposal = Disposal::Pending;
            drop(r.meter.take());
            drop(r.bare_meter.take());
            #[cfg(test)]
            drop(self.probe.take());
            r.meter_disposal = Disposal::Complete;
            return Ok(());
        }
        if r.meter_disposal != Disposal::Complete {
            return Err(AudioStorageError::UnknownStorage);
        }
        if r.chain_disposal == Disposal::Unstarted {
            r.chain_disposal = Disposal::Pending;
            drop(r.chain.take());
            drop(r.bare_chain.take());
            #[cfg(test)]
            drop(self.chain_probe.take());
            r.effects.clear();
            r.chain_disposal = Disposal::Complete;
            return Ok(());
        }
        if r.chain_disposal != Disposal::Complete {
            return Err(AudioStorageError::UnknownStorage);
        }
        r.gain.restore(&mut r.prepared_gain).map_err(gain_error)?;
        r.gain.clear_on_control();
        r.sampler.dispose();
        r.sends.clear();
        r.clip_index.clear();
        r.pairs.clear();
        r.id = None;
        r.mix = None;
        r.source_mix = None;
        r.handles = [0; 2];
        r.installed_ordinal = None;
        r.meter_declaration = None;
        r.chain_declaration = None;
        self.cursor += 1;
        Ok(())
    }
    pub(super) fn clear_disposed(&mut self) -> Result<(), AudioStorageError> {
        if self.phase == Phase::Vacant {
            return Ok(());
        }
        if self.phase != Phase::Complete
            || self.rows.iter().any(|r| {
                r.track.is_some()
                    || r.id.is_some()
                    || !r.clips.is_empty()
                    || !r.clip_index.is_empty()
                    || !r.pairs.is_empty()
                    || r.meter.is_some()
                    || r.chain.is_some()
                    || r.bare_meter.is_some()
                    || r.bare_chain.is_some()
                    || r.prepared_gain.is_some()
                    || r.sampler.present()
            })
        {
            return Err(AudioStorageError::Capacity);
        }
        self.destinations.clear();
        self.cursor = 0;
        self.active = 0;
        self.phase = Phase::Vacant;
        Ok(())
    }
    pub(super) fn validate_root_sources(
        &self,
        root: &[Track],
        slots: &[SourceSlot],
        bank: &OwnedAudioInputs,
    ) -> Result<(), AudioStorageError> {
        for row in &self.rows[..self.active] {
            let Some(track) = row.actual(root)? else {
                continue;
            };
            if row.pairs.len() != track.clips.len() {
                return Err(AudioStorageError::SourceMismatch);
            }
            for (pair, clip) in row.pairs.iter().zip(&track.clips) {
                let binding = pair
                    .owner
                    .as_ref()
                    .ok_or(AudioStorageError::SourceMismatch)?;
                if binding.binding != pair.origin
                    || binding.handle != Some(pair.origin.handle)
                    || binding.slot != Some(pair.origin.slot)
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                let slot = slots
                    .get(pair.origin.slot)
                    .ok_or(AudioStorageError::SourceMismatch)?;
                if slot.track != pair.origin.track
                    || slot.clip != pair.origin.clip
                    || slot.source != pair.origin.source
                    || slot.handle != pair.origin.handle
                    || slot.disposal != Disposal::Transferred
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                let source = slot.source_owner(bank)?;
                let matches = match (&source.kind, &clip.audio, clip.metadata.as_deref()) {
                    (SourceKind::Resident(original), Some(actual), _) => {
                        original.shares_pcm_with(actual) && binding.declaration.is_none()
                    }
                    (SourceKind::Streamed(original), None, Some(metadata)) => {
                        metadata.streamed.as_ref().is_some_and(|actual| {
                            Arc::ptr_eq(original.arc(), actual)
                                && binding.declaration.as_ref().and_then(|d| d.parts(actual))
                                    == original.parts()
                        })
                    }
                    _ => false,
                };
                if !matches {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
        }
        Ok(())
    }
    pub(super) fn root_count(&self) -> usize {
        self.active
    }
    pub(super) fn preflight_root_move(&self) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Installed
            || self.rows[..self.active].iter().enumerate().any(|(i, r)| {
                r.root_index.is_some()
                    || r.installed_ordinal != Some(i)
                    || r.track.as_ref().is_none_or(|t| t.track_id != r.id)
            })
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(super) fn move_root_row(&mut self, index: usize, target: &mut Vec<Track>) {
        if let Some(track) = self.rows[index].track.take() {
            target.push(track);
            self.rows[index].root_index = Some(index);
        }
    }
    pub(super) fn preflight_root_restore(&self, root: &[Track]) -> Result<(), AudioStorageError> {
        for (index, track) in root.iter().enumerate() {
            let row = self
                .rows
                .get(index)
                .ok_or(AudioStorageError::SourceMismatch)?;
            if row.root_index != Some(index) || row.track.is_some() || track.track_id != row.id {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        if self.rows.iter().filter(|r| r.root_index.is_some()).count() != root.len() {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(super) fn restore_root_rows(&mut self, root: &mut Vec<Track>) {
        while let Some(track) = root.pop() {
            let row = &mut self.rows[root.len()];
            row.track = Some(track);
            row.root_index = None;
        }
    }
    pub(super) fn metadata<'a>(
        &'a self,
        index: usize,
        root: &'a [Track],
    ) -> Option<ArrangementTrackMetadata<'a>> {
        if self.phase != Phase::Installed {
            return None;
        }
        {
            let r = self.rows.get(index)?;
            if r.installed_ordinal != Some(index) {
                return None;
            }
            Some(ArrangementTrackMetadata {
                track: r.actual(root).ok()??,
            })
        }
    }
}
/// Restricted metadata; never a legacy Track or owning/cloneable child.
/// ```compile_fail
/// fn escape(v: hex_engine::ArrangementTrackMetadata<'_>) { let _ = v.device_meter(); }
/// ```
/// ```compile_fail
/// fn escape(v: hex_engine::ArrangementTrackMetadata<'_>) { let _ = v.clips(); }
/// ```
/// ```compile_fail
/// fn escape(v: hex_engine::ArrangementTrackMetadata<'_>) -> hex_engine::Track { v.clone() }
/// ```
pub struct ArrangementTrackMetadata<'a> {
    track: &'a Track,
}
impl ArrangementTrackMetadata<'_> {
    pub fn id(&self) -> Option<hex_project::TrackId> {
        self.track.track_id
    }
    pub fn mix(&self) -> PreparedTrackMix {
        self.track.mix
    }
    pub fn knots(&self) -> &[crate::GainEnvelopeKnot] {
        self.track.gain_envelope_knots()
    }
    pub fn gain_at(&self, frame: u64) -> Option<f32> {
        self.track.prepared_gain_at(frame)
    }
    pub fn sends(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        self.track.sends.iter().map(|s| (s.return_index, s.gain))
    }
}

#[cfg(test)]
#[path = "track_tests.rs"]
mod tests;

impl TrackAssembly {
    pub(super) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
        root: &'a [Track],
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::Owner;
        self.capacity_at(false, root)?;
        let Self {
            rows,
            destinations,
            active: _,
            phase: _,
            growth,
            cursor: _,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                mismatch: _,
            #[cfg(test)]
                panic_after: _,
            #[cfg(test)]
                probe: _,
            #[cfg(test)]
                chain_probe: _,
        } = self;
        l.vec(rows)?;
        l.vec(destinations)?;
        l.growth(*growth)?;
        for r in rows {
            let Row {
                source_id: _,
                source_receiver_index: _,
                source_audio_receiver: _,
                source_input_enabled: _,
                audio_content_enabled: _,
                ordinal: _,
                installed_ordinal: _,
                id: _,
                mix: _,
                source_mix: _,
                clips,
                clip_index,
                pairs,
                sends,
                gain,
                prepared_gain,
                meter,
                chain,
                effects,
                sampler,
                bare_meter,
                bare_chain,
                meter_declaration,
                chain_declaration,
                handles,
                track: _,
                root_index: _,
                meter_disposal: _,
                chain_disposal: _,
            } = r;
            effects.visit_metadata(l)?;
            l.vec(clips)?;
            l.vec(clip_index)?;
            l.vec(pairs)?;
            l.vec(sends)?;
            gain.visit_audio_storage(l)?;
            if let Some(gain) = prepared_gain {
                gain.visit_audio_storage(l)?;
            }
            let actual = r.actual(root)?;
            sampler.visit(l, actual)?;
            let actual_clips = if let Some(t) = actual {
                let Track {
                    track_id: _,
                    source_id: _,
                    source_receiver_index: _,
                    source_audio_receiver: _,
                    source_input_enabled: _,
                    audio_content_enabled: _,
                    clips,
                    clip_index,
                    has_streamed_clips: _,
                    mix: _,
                    source_mix: _,
                    gain_envelope,
                    instrument,
                    instrument_position: _,
                    ordered_prefix: _,
                    ordered_suffix: _,
                    device_chain,
                    sends,
                    instrument_id: _,
                    instrument_state,
                    tuning_identity: _,
                    instrument_bypassed: _,
                    instrument_input_gain: _,
                    instrument_output_gain: _,
                    instrument_latency: _,
                    instrument_runtime_latency: _,
                    instrument_meter,
                    mutable_instrument,
                    mutable_looping_source,
                    midi_source_index,
                } = t;
                if (!sampler.present() && (instrument.is_some() || *mutable_instrument))
                    || instrument_state.is_some()
                    || (!sampler.present() && *mutable_looping_source)
                    || midi_source_index.is_some()
                {
                    return Err(AudioStorageError::UnknownStorage);
                }
                l.vec(clips)?;
                l.vec(clip_index)?;
                l.vec(sends)?;
                if let Some(gain) = gain_envelope {
                    gain.visit_audio_storage(l)?;
                }
                l.shared(
                    instrument_meter,
                    meter_declaration
                        .as_ref()
                        .and_then(|d| d.parts(instrument_meter)),
                    Owner::exclusive(instrument_meter),
                    handles[0],
                    None,
                )?;
                l.shared(
                    device_chain,
                    chain_declaration
                        .as_ref()
                        .and_then(|d| d.parts(device_chain)),
                    Owner::exclusive(device_chain),
                    handles[1],
                    None,
                )?;
                effects.visit(device_chain, l)?;
                clips
            } else {
                if let Some(owner) = meter {
                    l.shared(
                        owner.arc(),
                        owner.parts(),
                        Owner::exclusive(owner),
                        handles[0],
                        None,
                    )?;
                }
                if let Some(owner) = chain {
                    effects.visit(owner.arc(), l)?;
                    l.shared(
                        owner.arc(),
                        owner.parts(),
                        Owner::exclusive(owner),
                        handles[1],
                        None,
                    )?;
                }
                if let Some(owner) = bare_meter {
                    l.shared(
                        owner,
                        meter_declaration.as_ref().and_then(|d| d.parts(owner)),
                        Owner::exclusive(owner),
                        handles[0],
                        None,
                    )?;
                }
                if let Some(owner) = bare_chain {
                    effects.visit(owner, l)?;
                    l.shared(
                        owner,
                        chain_declaration.as_ref().and_then(|d| d.parts(owner)),
                        Owner::exclusive(owner),
                        handles[1],
                        None,
                    )?;
                }
                clips
            };
            if actual_clips.len() > pairs.len() {
                return Err(AudioStorageError::SourceMismatch);
            }
            for (clip, plan) in actual_clips.iter().zip(pairs) {
                let owner = plan
                    .owner
                    .as_ref()
                    .ok_or(AudioStorageError::SourceMismatch)?;
                if owner.binding != plan.origin
                    || owner.handle != Some(plan.origin.handle)
                    || owner.slot != Some(plan.origin.slot)
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                l.clip(
                    clip,
                    owner.declaration.as_ref(),
                    owner.handle.ok_or(AudioStorageError::SourceMismatch)?,
                )?;
            }
        }
        Ok(())
    }
}
