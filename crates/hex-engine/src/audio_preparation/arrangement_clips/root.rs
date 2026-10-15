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
    pub previous_growth_backing_bytes: usize,
    pub installed_tracks: usize,
    pub installed_returns: usize,
    pub constructed: bool,
}
impl ArrangementRootCapacity {
    pub(crate) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.track_container_bytes
            .checked_add(self.return_container_bytes)
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
    has_feed: bool,
    scalars: Option<Scalars>,
    growth: usize,
    #[cfg(test)]
    pub(super) fail_after: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after: Option<usize>,
}
impl ArrangementRootAssembly {
    pub(super) fn reserve(
        &mut self,
        project: &hex_project::Project,
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
        if project.tracks().iter().any(|t| {
            !t.devices().is_empty() || t.instrument().is_some() || !t.midi_clips().is_empty()
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
    pub(super) fn view(&self) -> Result<RootStorageView<'_>, AudioStorageError> {
        if let Some(project) = &self.project {
            if self.tracks.capacity() != 0
                || self.returns.capacity() != 0
                || self.contexts.is_some()
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
    pub(super) fn context(&self) -> Option<&OwnedShared<SharedAnalyticContextTable>> {
        self.project
            .as_ref()
            .map(|p| &p.shared_contexts)
            .or(self.contexts.as_ref())
    }
    pub(super) fn capacity(
        &self,
        growth: bool,
    ) -> Result<ArrangementRootCapacity, AudioStorageError> {
        let view = self.view()?;
        let (tracks, returns) = self
            .project
            .as_ref()
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
            previous_growth_backing_bytes: if growth { self.growth } else { 0 },
            installed_tracks: view.tracks.len(),
            installed_returns: view.returns.len(),
            constructed: self.project.is_some(),
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
        let view = self.view()?;
        tracks.preflight_root_restore(view.tracks)?;
        returns.preflight_root_restore(view.returns, self.has_feed)?;
        if self.context().is_some() {
            shared.preflight_arrangement_restore()?;
        }
        // Validate the whole restoration batch before unpacking any final owner.
        if let Some(Project {
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
        }
        shared.restore_arrangement(&mut self.contexts);
        returns.restore_root(&mut self.returns, &mut self.feed, self.has_feed);
        tracks.restore_root_rows(&mut self.tracks);
        self.has_feed = false;
        self.scalars = None;
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
