//! Private final Arrangement value and its persistent, reversible staging.
use super::*;
use crate::{
    PreparedMasterGain, Project, ReturnBus, Track, owned_shared::OwnedShared,
    return_feed_plan::ReturnFeedPlan, shared_analytic::SharedAnalyticContextTable,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrangementRootCapacity {
    pub track_container_bytes: usize,
    pub return_container_bytes: usize,
    pub tempo_backing_bytes: usize,
    pub midi_routing_backing_bytes: usize,
    pub signal_topology_bytes: usize,
    pub previous_growth_backing_bytes: usize,
    pub installed_tracks: usize,
    pub installed_returns: usize,
    pub constructed: bool,
}
impl ArrangementRootCapacity {
    pub(crate) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.track_container_bytes
            .checked_add(self.return_container_bytes)
            .and_then(|n| n.checked_add(self.tempo_backing_bytes))
            .and_then(|n| n.checked_add(self.midi_routing_backing_bytes))
            .and_then(|n| n.checked_add(self.signal_topology_bytes))
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
    tempo_map: Option<hex_project::TempoMap>,
    midi_routing: Vec<(hex_project::TrackId, hex_project::TrackId)>,
    topology: Option<OwnedShared<crate::signal_graph::SignalGraph>>,
    metadata_drop_pending: bool,
    has_feed: bool,
    scalars: Option<Scalars>,
    growth: usize,
    #[cfg(test)]
    pub(super) fail_after: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after_metadata_take: bool,
}
impl ArrangementRootAssembly {
    #[cfg(test)]
    pub(super) fn capacity(
        &self,
        growth: bool,
    ) -> Result<ArrangementRootCapacity, AudioStorageError> {
        self.capacity_at(growth, None)
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
            && self.contexts.is_none()
            && self.tempo_map.is_none()
            && self.topology.is_none()
            && self.midi_routing.capacity() == 0
            && !self.metadata_drop_pending
            && self.feed.has_no_owned_storage()
    }
    pub(super) fn restore_project(&mut self, source: &mut Option<Project>) {
        self.project = source.take();
    }
    pub(super) fn reserve(
        &mut self,
        project: &hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        if self.project.is_some()
            || !self.tracks.is_empty()
            || !self.returns.is_empty()
            || self.contexts.is_some()
            || self.has_feed
            || self.tempo_map.is_some()
            || self.topology.is_some()
            || self.midi_routing.capacity() != 0
            || self.metadata_drop_pending
            || self.scalars.is_some()
        {
            return Err(AudioStorageError::Capacity);
        }
        // Eligibility is audited again here rather than inferred from an empty
        // mutable-descriptor destination or from a legacy compiler outcome.
        if project.tracks().iter().any(|t| {
            t.devices()
                .iter()
                .any(|d| d.state().type_id() != hex_devices::utility::UTILITY_TYPE_ID)
                || t.instrument().is_some()
                || !t.midi_clips().is_empty()
        }) || project
            .return_tracks()
            .iter()
            .any(|r| !r.devices().is_empty())
        {
            return Err(AudioStorageError::UnsupportedGraph);
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
        if self.metadata_drop_pending {
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.project.is_some() && external.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        if let Some(project) = self.project.as_ref().or(external) {
            if project.audio_clip_views.capacity() != 0
                || project
                    .signal_graph
                    .as_ref()
                    .is_some_and(|graph| graph.parts().is_none())
                || self.tracks.capacity() != 0
                || self.returns.capacity() != 0
                || self.contexts.is_some()
                || self.topology.is_some()
                || self.tempo_map.is_some()
                || self.midi_routing.capacity() != 0
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
        let tempo_map = self
            .project
            .as_ref()
            .or(external)
            .and_then(|project| project.tempo_map.as_ref())
            .or(self.tempo_map.as_ref());
        let tempo_backing_bytes = tempo_map
            .map(|map| {
                let layout = map.owned_allocation_layout();
                layout
                    .point_bytes()
                    .and_then(|n| layout.prefix_bytes().and_then(|m| n.checked_add(m)))
                    .ok_or(AudioStorageError::Overflow)
            })
            .transpose()?
            .unwrap_or(0);
        let routes = self
            .project
            .as_ref()
            .or(external)
            .map_or(&self.midi_routing, |project| &project.midi_routing);
        let topology = self
            .project
            .as_ref()
            .or(external)
            .and_then(|project| project.signal_graph.as_ref())
            .or(self.topology.as_ref());
        let signal_topology_bytes = topology
            .map(|graph| {
                let (_, overhead) = graph
                    .parts()
                    .ok_or(AudioStorageError::UnsupportedConstructor)?;
                graph
                    .table_bytes()
                    .and_then(|bytes| bytes.checked_add(overhead))
                    .ok_or(AudioStorageError::Overflow)
            })
            .transpose()?
            .unwrap_or(0);
        Ok(ArrangementRootCapacity {
            signal_topology_bytes,
            tempo_backing_bytes,
            midi_routing_backing_bytes: routes
                .capacity()
                .checked_mul(mem::size_of::<(hex_project::TrackId, hex_project::TrackId)>())
                .ok_or(AudioStorageError::Overflow)?,
            track_container_bytes: tracks
                .capacity()
                .checked_mul(mem::size_of::<Track>())
                .ok_or(AudioStorageError::Overflow)?,
            return_container_bytes: returns
                .capacity()
                .checked_mul(mem::size_of::<ReturnBus>())
                .ok_or(AudioStorageError::Overflow)?,
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
            || self.tempo_map.is_some()
            || self.topology.is_some()
            || self.midi_routing.capacity() != 0
            || self.metadata_drop_pending
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
        if let Some(contexts) = self.contexts.take() {
            let has_streamed_sources = self
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(Clip::is_streamed);
            self.project = Some(Project {
                project_track_ids: crate::project_output::ProjectTrackIds::prepare(&self.tracks),
                audio_clip_views: Vec::new(),
                signal_graph: None,
                tempo_map: None,
                midi_routing: Vec::new(),
                sample_rate: scalars.sample_rate,
                tracks: mem::take(&mut self.tracks),
                return_buses: mem::take(&mut self.returns),
                return_feed_plan: mem::take(&mut self.feed),
                loop_region: scalars.loop_region,
                master_gain: scalars.master_gain,
                audible_end_frame: scalars.audible_end_frame,
                has_streamed_sources,
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
            tempo_map,
            midi_routing,
            sample_rate: _,
            tracks,
            return_buses,
            return_feed_plan,
            loop_region: _,
            master_gain: _,
            audible_end_frame: _,
            has_streamed_sources: _,
            shared_contexts,
        }) = self.project.take()
        {
            self.tracks = tracks;
            self.returns = return_buses;
            self.feed = return_feed_plan;
            self.contexts = Some(shared_contexts);
            self.topology = signal_graph;
            self.install_restored_metadata(tempo_map, midi_routing);
        }
        shared.restore_arrangement(&mut self.contexts);
        returns.restore_root(&mut self.returns, &mut self.feed, self.has_feed);
        tracks.restore_root_rows(&mut self.tracks);
        self.has_feed = false;
        self.scalars = None;
        Ok(())
    }
    /// Fixed move after the actual aggregate/destination preflight. No fresh
    /// constructor receipt or provenance is manufactured by moving owned Vecs.
    fn install_restored_metadata(
        &mut self,
        tempo_map: Option<hex_project::TempoMap>,
        midi_routing: Vec<(hex_project::TrackId, hex_project::TrackId)>,
    ) {
        self.tempo_map = tempo_map;
        self.midi_routing = midi_routing;
    }
    pub(super) fn has_metadata_disposal(&self) -> bool {
        self.metadata_drop_pending
            || self.tempo_map.is_some()
            || self.topology.is_some()
            || self.midi_routing.capacity() != 0
    }
    pub(super) fn dispose_metadata_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.metadata_drop_pending {
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.project.is_some()
            || !self.tracks.is_empty()
            || !self.returns.is_empty()
            || self.contexts.is_some()
        {
            return Err(AudioStorageError::Capacity);
        }
        if self
            .topology
            .as_ref()
            .is_some_and(|graph| graph.parts().is_none())
        {
            return Err(AudioStorageError::UnsupportedConstructor);
        }
        // SignalGraph descendants are audited fixed scalar vectors; no provider
        // destructor, mutable runtime buffer, or adopted raw Arc is accepted.
        // Canonical TempoMap owns only TempoPoint/u128 Vecs; routing entries
        // are Copy IDs. Parent catch retains pending/FailedIncomplete on unwind.
        fn copy_element<T: Copy>() {}
        copy_element::<hex_project::TempoPoint>();
        copy_element::<(hex_project::TrackId, hex_project::TrackId)>();
        self.metadata_drop_pending = true;
        drop(self.topology.take());
        drop(self.tempo_map.take());
        #[cfg(test)]
        assert!(
            !self.panic_after_metadata_take,
            "persistent Arrangement metadata drop panic"
        );
        drop(mem::take(&mut self.midi_routing));
        self.metadata_drop_pending = false;
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
            tempo_map,
            midi_routing,
            topology,
            metadata_drop_pending: _,
            has_feed: _,
            scalars: _,
            growth,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
            #[cfg(test)]
                panic_after_metadata_take: _,
        } = self;
        let view = self.view_at(external)?;
        l.vec(tracks)?;
        l.vec(returns)?;
        feed.visit_audio_storage(l)?;
        l.growth(*growth)?;
        l.vec(midi_routing)?;
        if let Some(graph) = topology {
            crate::signal_graph::storage::visit(graph, l)?;
        }
        if let Some(map) = tempo_map {
            l.mutate(|ledger| ledger.map(map))?;
        }
        if let Some(project) = project.as_ref().or(external) {
            let Project {
                project_track_ids: _,
                audio_clip_views,
                sample_rate: _,
                tracks,
                return_buses,
                return_feed_plan: _,
                signal_graph,
                tempo_map,
                midi_routing,
                loop_region: _,
                master_gain: _,
                audible_end_frame: _,
                has_streamed_sources: _,
                shared_contexts: _,
            } = project;
            if let Some(graph) = signal_graph {
                crate::signal_graph::storage::visit(graph, l)?;
            }
            l.vec(audio_clip_views)?;
            l.vec(tracks)?;
            l.vec(return_buses)?;
            l.vec(midi_routing)?;
            if let Some(map) = tempo_map {
                l.mutate(|ledger| ledger.map(map))?;
            }
        }
        Ok(view)
    }
}

#[cfg(test)]
mod metadata_tests;
