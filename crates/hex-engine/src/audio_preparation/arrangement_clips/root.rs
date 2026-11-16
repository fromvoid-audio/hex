//! Private final Arrangement value and its persistent, reversible staging.
use super::*;
mod signal;
use crate::{
    PreparedMasterGain, Project, ReturnBus, Track, owned_shared::OwnedShared,
    return_feed_plan::ReturnFeedPlan, shared_analytic::SharedAnalyticContextTable,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrangementRootCapacity {
    pub track_container_bytes: usize,
    pub return_container_bytes: usize,
    pub main_prepared_bytes: usize,
    /// Temporary closed chain Vec/payload overlap, excluded from retained DATA.
    pub main_construction_overlap_bytes: usize,
    /// Retained ceiling selected before any closed topology construction.
    pub signal_graph_reserved_bytes: usize,
    pub signal_graph_data_bytes: usize,
    /// Known construction DATA policy; opaque builder contexts are separately
    /// bounded by the existing node/edge cardinalities, not allocator/RSS bytes.
    pub signal_graph_construction_overlap_bytes: usize,
    pub previous_growth_backing_bytes: usize,
    pub installed_tracks: usize,
    pub installed_returns: usize,
    pub constructed: bool,
}
impl ArrangementRootCapacity {
    pub(crate) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.track_container_bytes
            .checked_add(self.return_container_bytes)
            .and_then(|n| n.checked_add(self.main_prepared_bytes))
            .and_then(|n| {
                n.checked_add(
                    self.signal_graph_reserved_bytes
                        .max(self.signal_graph_data_bytes),
                )
            })
            .ok_or(AudioStorageError::Overflow)
    }
}
#[derive(Clone, Copy)]
struct Scalars {
    sample_rate: u32,
    loop_region: Option<hex_project::LoopRegion>,
    master_gain: PreparedMasterGain,
    audible_end_frame: Option<u64>,
}
pub(super) struct RootStorageView<'a> {
    pub(super) tracks: &'a [Track],
    pub(super) returns: &'a [ReturnBus],
    pub(super) feed: Option<&'a ReturnFeedPlan>,
}
#[derive(Default)]
pub(super) struct ArrangementRootAssembly {
    project: Option<Project>,
    tracks: Vec<Track>,
    returns: Vec<ReturnBus>,
    feed: ReturnFeedPlan,
    contexts: Option<OwnedShared<SharedAnalyticContextTable>>,
    main_chain: Option<Arc<[hex_lattice::PreparedRuntimeInstance]>>,
    main_declaration: Option<crate::owned_shared::SharedDeclaration>,
    main_custody: super::chains::ChainCustody,
    main_handle: u64,
    main_disposal: Disposal,
    signal_graph: Option<Arc<crate::signal_graph::SignalGraph>>,
    signal_declaration: Option<crate::owned_shared::SharedDeclaration>,
    signal_handle: u64,
    signal_disposal: Disposal,
    signal_reserved: bool,
    has_feed: bool,
    scalars: Option<Scalars>,
    growth: usize,
    #[cfg(test)]
    pub(super) fail_after: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after: Option<usize>,
}
impl ArrangementRootAssembly {
    #[cfg(test)]
    pub(super) fn capacity(
        &self,
        growth: bool,
    ) -> Result<ArrangementRootCapacity, AudioStorageError> {
        self.capacity_at(growth, None)
    }
    #[cfg(test)]
    pub(super) fn main_carrier_bytes() -> Result<usize, AudioStorageError> {
        let (payload, overhead) =
            OwnedShared::<[hex_lattice::PreparedRuntimeInstance]>::declared_slice_parts(0)
                .ok_or(AudioStorageError::UnknownStorage)?;
        payload
            .checked_add(overhead)
            .ok_or(AudioStorageError::Overflow)
    }
    pub(super) fn main_quote(
        &self,
        project: &hex_project::Project,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(usize, usize), AudioStorageError> {
        self.main_custody.quote_with_input(
            project.main_devices(),
            project.sample_rate(),
            native_input,
        )
    }
    pub(super) fn reserve_main_carrier(
        &mut self,
        project: &hex_project::Project,
        effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(), AudioStorageError> {
        if self.scalars.is_none() || self.project.is_some() {
            return Err(AudioStorageError::Capacity);
        }
        // Arc::new([T; 0]) has the exact sized-zero-array layout quoted above;
        // unsizing transfers this allocation without a second birth.
        if self
            .main_chain
            .as_ref()
            .is_some_and(|chain| chain.is_empty())
            && !project.main_devices().is_empty()
        {
            self.main_chain = None;
            self.main_declaration = None;
        }
        if self.main_chain.is_none() {
            self.main_handle = crate::audio_only_inputs::handle()?;
            self.main_disposal = Disposal::Unstarted;
            let chain = self.main_custody.prepare_with_effects_and_input(
                project.main_devices(),
                hex_lattice::ProcessorChainOwner::Main,
                project.sample_rate(),
                effects,
                native_input,
                Some(project),
            )?;
            let (chain, declaration) = chain.into_paired_parts_on_control();
            self.main_chain = Some(chain);
            self.main_declaration = Some(declaration);
        }
        Ok(())
    }
    pub(super) fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }
    pub(super) fn transfer_project(&mut self, target: &mut Option<Project>) {
        *target = self.project.take();
    }
    pub(super) fn can_restore_project(&self) -> bool {
        self.project.is_none()
            && self.tracks.capacity() == 0
            && self.returns.capacity() == 0
            && self.main_chain.is_none()
            && self.signal_graph.is_none()
            && self.contexts.is_none()
            && self.feed.has_no_owned_storage()
    }
    pub(super) fn restore_project(&mut self, source: &mut Option<Project>) {
        self.project = source.take();
    }
    pub(super) fn reserve_with_effects(
        &mut self,
        project: &hex_project::Project,
        sampler: bool,
        effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
    ) -> Result<(), AudioStorageError> {
        if self.project.is_some()
            || !self.tracks.is_empty()
            || !self.returns.is_empty()
            || self.contexts.is_some()
            || self.has_feed
            || self.scalars.is_some()
        {
            return Err(AudioStorageError::Capacity);
        }
        // Eligibility is audited again here rather than inferred from an empty
        // mutable-descriptor destination or from a legacy compiler outcome.
        if sampler {
            if !crate::native_device_resources::current_sampler::validate_project_with_effects(
                project,
                effects.is_some(),
            )? {
                return Err(AudioStorageError::UnsupportedGraph);
            }
        } else {
            if (effects.is_none() && !project.main_devices().is_empty())
                || project.tracks().iter().any(|t| {
                    t.devices().iter().any(|d| {
                        effects.is_none()
                            && d.state().type_id() != hex_devices::utility::UTILITY_TYPE_ID
                    }) || t.instrument().is_some()
                        || !t.midi_clips().is_empty()
                })
                || project
                    .return_tracks()
                    .iter()
                    .any(|r| effects.is_none() && !r.devices().is_empty())
            {
                return Err(AudioStorageError::UnsupportedGraph);
            }
        }
        let master_gain = PreparedMasterGain::new(project.master_gain())
            .map_err(|_| AudioStorageError::SourceMismatch)?;
        let mut end = None;
        for clip in project.tracks().iter().flat_map(|t| t.clips()) {
            let value = clip
                .start()
                .checked_add(clip.duration())
                .ok_or(AudioStorageError::Overflow)?;
            end = Some(end.map_or(value, |previous: u64| previous.max(value)));
        }
        self.signal_reserved = effects.is_some() || project.has_group_tracks();
        self.growth = 0;
        for (old, count, stride) in [
            (
                self.tracks.capacity(),
                project.tracks().len(),
                mem::size_of::<Track>(),
            ),
            (
                self.returns.capacity(),
                project.return_tracks().len(),
                mem::size_of::<ReturnBus>(),
            ),
        ] {
            if count > old {
                self.growth = self
                    .growth
                    .checked_add(old.checked_mul(stride).ok_or(AudioStorageError::Overflow)?)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        self.scalars = Some(Scalars {
            sample_rate: project.sample_rate(),
            loop_region: project.loop_region(),
            master_gain,
            audible_end_frame: end,
        });
        self.tracks
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        #[cfg(test)]
        self.fault(0)?;
        self.returns
            .try_reserve_exact(project.return_tracks().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        #[cfg(test)]
        self.fault(1)?;
        Ok(())
    }
    pub(super) fn view_at<'a>(
        &'a self,
        external: Option<&'a Project>,
    ) -> Result<RootStorageView<'a>, AudioStorageError> {
        self.signal_data_bytes(external)?;
        if self.project.is_some() && external.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        if let Some(project) = self.project.as_ref().or(external) {
            if project.audio_clip_views.capacity() != 0
                || project.tempo_map.is_some()
                || project.midi_routing.capacity() != 0
                || self.tracks.capacity() != 0
                || self.returns.capacity() != 0
                || self.main_chain.is_some()
                || self.contexts.is_some()
                || !self.feed.has_no_owned_storage()
            {
                return Err(AudioStorageError::UnknownStorage);
            }
            Ok(RootStorageView {
                tracks: &project.tracks,
                returns: &project.return_buses,
                feed: Some(&project.return_feed_plan),
            })
        } else {
            Ok(RootStorageView {
                tracks: &self.tracks,
                returns: &self.returns,
                feed: self.has_feed.then_some(&self.feed),
            })
        }
    }
    pub(super) fn context_at<'a>(
        &'a self,
        external: Option<&'a Project>,
    ) -> Option<&'a OwnedShared<SharedAnalyticContextTable>> {
        self.project
            .as_ref()
            .or(external)
            .map(|p| &p.shared_contexts)
            .or(self.contexts.as_ref())
    }
    pub(super) fn capacity_at(
        &self,
        growth: bool,
        external: Option<&Project>,
    ) -> Result<ArrangementRootCapacity, AudioStorageError> {
        let view = self.view_at(external)?;
        let (tracks, returns) = self
            .project
            .as_ref()
            .or(external)
            .map_or((&self.tracks, &self.returns), |p| {
                (&p.tracks, &p.return_buses)
            });
        Ok(ArrangementRootCapacity {
            track_container_bytes: tracks
                .capacity()
                .checked_mul(mem::size_of::<Track>())
                .ok_or(AudioStorageError::Overflow)?,
            return_container_bytes: returns
                .capacity()
                .checked_mul(mem::size_of::<ReturnBus>())
                .ok_or(AudioStorageError::Overflow)?,
            main_prepared_bytes: if self.main_chain.is_some()
                || self.project.is_some()
                || external.is_some()
            {
                let main = self
                    .project
                    .as_ref()
                    .or(external)
                    .map(|p| &p.main_device_chain)
                    .or(self.main_chain.as_ref())
                    .ok_or(AudioStorageError::UnknownStorage)?;
                let (payload, overhead) = self
                    .main_declaration
                    .as_ref()
                    .and_then(|d| d.parts(main))
                    .ok_or(AudioStorageError::UnknownStorage)?;
                payload
                    .checked_add(overhead)
                    .and_then(|n| n.checked_add(self.main_custody.bytes(main).ok()?))
                    .ok_or(AudioStorageError::Overflow)?
            } else {
                self.main_custody.metadata_bytes()?
            },
            main_construction_overlap_bytes: self.main_custody.construction_overlap(),
            signal_graph_reserved_bytes: if self.signal_reserved {
                crate::compensation::MAX_COMPENSATION_BYTES
            } else {
                0
            },
            signal_graph_data_bytes: self.signal_data_bytes(external)?,
            signal_graph_construction_overlap_bytes: if self.signal_reserved {
                crate::compensation::MAX_COMPENSATION_BYTES
            } else {
                0
            },
            previous_growth_backing_bytes: if growth { self.growth } else { 0 },
            installed_tracks: view.tracks.len(),
            installed_returns: view.returns.len(),
            constructed: self.project.is_some() || external.is_some(),
        })
    }
    pub(super) fn construct(
        &mut self,
        tracks: &mut tracks::TrackAssembly,
        returns: &mut returns::ReturnAssembly,
        shared: &mut crate::session_shared_preparation::SessionSharedConstruction,
    ) -> Result<(), AudioStorageError> {
        let scalars = self.scalars.ok_or(AudioStorageError::Capacity)?;
        if self.project.is_some()
            || !self.tracks.is_empty()
            || !self.returns.is_empty()
            || self.has_feed
            || self.contexts.is_some()
            || self.main_chain.is_none()
            || self.tracks.capacity() < tracks.root_count()
            || self.returns.capacity() < returns.root_count()
        {
            return Err(AudioStorageError::Capacity);
        }
        tracks.preflight_root_move()?;
        returns.preflight_root_move()?;
        shared.preflight_arrangement_transfer(&self.contexts)?;
        let mut frontier = 2;
        for index in 0..tracks.root_count() {
            tracks.move_root_row(index, &mut self.tracks);
            #[cfg(test)]
            self.fault(frontier)?;
            frontier += 1;
        }
        for index in 0..returns.root_count() {
            returns.move_root_row(index, &mut self.returns);
            #[cfg(test)]
            self.fault(frontier)?;
            frontier += 1;
        }
        let _ = frontier;
        returns.move_root_feed(&mut self.feed);
        self.has_feed = true;
        #[cfg(test)]
        self.fault(frontier)?;
        shared.commit_arrangement_transfer(&mut self.contexts);
        #[cfg(test)]
        self.fault(frontier + 1)?;
        // Every field was validated/reserved before any owner move. The only
        // take below is immediately secured in the persistent final location.
        if let (Some(contexts), Some(main_device_chain)) =
            (self.contexts.take(), self.main_chain.take())
        {
            let has_streamed_sources = self
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(Clip::is_streamed);
            let audible_end_frame =
                self.tracks
                    .iter()
                    .flat_map(|track| {
                        track.clips.iter().map(|clip| clip.end).chain(
                            track.instrument.iter().flat_map(|instrument| {
                                instrument
                                    .timeline_end
                                    .into_iter()
                                    .chain(instrument.spans.iter().map(|span| span.end))
                            }),
                        )
                    })
                    .max()
                    .or(scalars.audible_end_frame);
            self.project = Some(Project {
                project_track_ids: crate::project_output::ProjectTrackIds::prepare(&self.tracks),
                audio_clip_views: Vec::new(),
                signal_graph: None,
                main_output_latency_frames: 0,
                tempo_map: None,
                tempo_max_micro_bpm: 0,
                midi_routing: Vec::new(),
                source_effects: Vec::new(),
                sample_rate: scalars.sample_rate,
                main_device_chain,
                tracks: mem::take(&mut self.tracks),
                return_buses: mem::take(&mut self.returns),
                return_feed_plan: mem::take(&mut self.feed),
                loop_region: scalars.loop_region,
                master_gain: scalars.master_gain,
                audible_end_frame,
                has_streamed_sources,
                requires_scalar_automation: false,
                shared_contexts: contexts,
            });
        }
        #[cfg(test)]
        self.fault(frontier + 2)?;
        Ok(())
    }
    pub(super) fn restore(
        &mut self,
        tracks: &mut tracks::TrackAssembly,
        returns: &mut returns::ReturnAssembly,
        shared: &mut crate::session_shared_preparation::SessionSharedConstruction,
    ) -> Result<(), AudioStorageError> {
        let view = self.view_at(None)?;
        tracks.preflight_root_restore(view.tracks)?;
        returns.preflight_root_restore(view.returns, self.has_feed)?;
        if self.context_at(None).is_some() {
            shared.preflight_arrangement_restore()?;
        }
        // Validate the whole restoration batch before unpacking any final owner.
        if let Some(Project {
            project_track_ids: _,
            audio_clip_views: _,
            signal_graph,
            main_output_latency_frames: _,
            tempo_map: _,
            tempo_max_micro_bpm: _,
            midi_routing: _,
            source_effects: _,
            sample_rate: _,
            main_device_chain,
            tracks,
            return_buses,
            return_feed_plan,
            loop_region: _,
            master_gain: _,
            audible_end_frame: _,
            has_streamed_sources: _,
            requires_scalar_automation: _,
            shared_contexts,
        }) = self.project.take()
        {
            self.signal_graph = signal_graph.or(self.signal_graph.take());
            self.main_chain = Some(main_device_chain);
            self.tracks = tracks;
            self.returns = return_buses;
            self.feed = return_feed_plan;
            self.contexts = Some(shared_contexts);
        }
        shared.restore_arrangement(&mut self.contexts);
        returns.restore_root(&mut self.returns, &mut self.feed, self.has_feed);
        tracks.restore_root_rows(&mut self.tracks);
        self.has_feed = false;
        self.scalars = None;
        Ok(())
    }
    pub(super) fn dispose_main_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.main_disposal == Disposal::Pending {
            return Err(AudioStorageError::UnknownStorage);
        }
        // The accepted zero-element carrier has no device owners and remains
        // reusable with the same constructor identity across empty revisions.
        if self
            .main_chain
            .as_ref()
            .is_some_and(|chain| chain.is_empty())
        {
            return Ok(());
        }
        if self.main_chain.is_none() {
            self.main_custody.clear();
            self.main_declaration = None;
            self.main_handle = 0;
            return Ok(());
        }
        if self.project.is_some() || self.main_disposal != Disposal::Unstarted {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.main_disposal = Disposal::Pending;
        drop(self.main_chain.take());
        self.main_disposal = Disposal::Complete;
        self.main_custody.clear();
        self.main_declaration = None;
        self.main_handle = 0;
        Ok(())
    }
    #[cfg(test)]
    fn fault(&self, frontier: usize) -> Result<(), AudioStorageError> {
        assert_ne!(
            self.panic_after,
            Some(frontier),
            "persistent Arrangement root panic"
        );
        if self.fail_after == Some(frontier) {
            return Err(AudioStorageError::Allocation);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod revision_tests;

impl ArrangementRootAssembly {
    pub(super) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
        external: Option<&'a Project>,
    ) -> Result<RootStorageView<'a>, AudioStorageError> {
        let Self {
            project,
            tracks,
            returns,
            feed,
            contexts: _,
            main_chain,
            main_declaration,
            main_custody,
            main_handle,
            main_disposal: _,
            signal_graph: _,
            signal_declaration: _,
            signal_handle: _,
            signal_disposal: _,
            signal_reserved: _,
            has_feed: _,
            scalars: _,
            growth,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
        } = self;
        let view = self.view_at(external)?;
        self.visit_signal_storage(l, external)?;
        l.vec(tracks)?;
        l.vec(returns)?;
        feed.visit_audio_storage(l)?;
        l.growth(*growth)?;
        main_custody.visit_metadata(l)?;
        let actual_main = project
            .as_ref()
            .or(external)
            .map(|p| &p.main_device_chain)
            .or(main_chain.as_ref());
        if let Some(main) = actual_main {
            l.shared(
                main,
                main_declaration.as_ref().and_then(|d| d.parts(main)),
                crate::audio_storage::Owner::exclusive(main),
                *main_handle,
                None,
            )?;
            main_custody.visit(main, l)?;
        }
        if let Some(project) = project.as_ref().or(external) {
            let Project {
                project_track_ids: _,
                audio_clip_views,
                sample_rate: _,
                tracks,
                return_buses,
                return_feed_plan: _,
                signal_graph: _,
                main_output_latency_frames: _,
                tempo_map: _,
                tempo_max_micro_bpm: _,
                midi_routing,
                source_effects,
                main_device_chain: _,
                loop_region: _,
                master_gain: _,
                audible_end_frame: _,
                has_streamed_sources: _,
                requires_scalar_automation: _,
                shared_contexts: _,
            } = project;
            if !source_effects.is_empty() {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            l.vec(audio_clip_views)?;
            l.vec(tracks)?;
            l.vec(return_buses)?;
            l.vec(midi_routing)?;
        }
        Ok(view)
    }
}
