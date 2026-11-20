//! A short fade on the departing native stage or audio track. Preparation
//! previews the existing handoff before changing the donor. Publication owns
//! the outer source fence and restores the donor when it returns a candidate.
use super::{RouteOwner, SessionPlayback, SessionPlaybackError};
use crate::signal_graph::FreshHistory;
use hex_lattice::{NativeRemovalTransition, NativeRuntimeChain, ProcessorChainOwner};

/// Progress of a staged native removal at a render boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeRemovalProgress {
    NotApplicable,
    Fading,
    Ready,
}

#[derive(Clone, Copy)]
pub(super) struct PendingNativeRemoval {
    transition: RemovalTransition,
    runtime: u64,
    epoch: u64,
    started: bool,
}

#[derive(Clone, Copy)]
enum RemovalTransition {
    Device(NativeRemovalTransition),
    Track(usize),
}

impl PendingNativeRemoval {
    pub(super) fn matches(self, previous: &SessionPlayback) -> bool {
        self.runtime == previous.capture_runtime && self.epoch == previous.capture_epoch
    }

    pub(super) fn device_transition(self) -> Option<NativeRemovalTransition> {
        match self.transition {
            RemovalTransition::Device(transition) => Some(transition),
            RemovalTransition::Track(_) => None,
        }
    }

    pub(super) fn staged_track_id(
        self,
        previous: &SessionPlayback,
    ) -> Option<hex_project::TrackId> {
        match self.transition {
            RemovalTransition::Track(index) => previous.routes.get(index).map(|route| route.id),
            RemovalTransition::Device(_) => None,
        }
    }

    pub(super) fn ready_on(self, previous: &SessionPlayback) -> bool {
        match self.transition {
            RemovalTransition::Device(transition) => previous.native_removal_ready(transition),
            RemovalTransition::Track(index) => {
                previous
                    .track_faders
                    .get(index)
                    .is_some_and(|fader| fader.removal_ready())
                    && previous.compensation.as_ref().is_none_or(|compensation| {
                        // A zero fader can still have its last audible samples
                        // waiting in the old route's alignment ring.
                        previous.routes.get(index).is_some_and(|route| {
                            compensation.owner_history_is_silent(FreshHistory::RemovedTrack {
                                track: route.id,
                                staged: true,
                            })
                        })
                    })
            }
        }
    }
}

impl SessionPlayback {
    /// Preview a fitting resident removal and start its stage-local fade.
    /// Call again at render boundaries and install only at `Ready`. This
    /// never changes the transport or transfers the removed stage's custody.
    pub fn advance_native_removal_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<NativeRemovalProgress, SessionPlaybackError> {
        if self.native_removal.is_some() {
            return self.advance_planned_native_removal_from(previous);
        }
        if self.revision.contains_streamed_sources()
            || previous.revision.contains_streamed_sources()
            || !self.plan_native_removal_from(previous)?
        {
            return Ok(NativeRemovalProgress::NotApplicable);
        }
        let preview = self.preview_stream_handoff_with_policy_and_report(
            previous,
            1,
            None,
            &mut |_| Err(crate::Invalid("native removal requires resident audio")),
            true,
            false,
        );
        match preview {
            Ok((_, report)) if !report.running_audio_restart => {}
            result => {
                self.native_removal = None;
                return Err(result
                    .err()
                    .unwrap_or(SessionPlaybackError::DeviceInsertNeedsConfirmation));
            }
        }
        self.advance_planned_native_removal_from(previous)
    }

    /// Classify a candidate before preview, without touching the running stage.
    /// For a streamed graph the caller must prove page readiness at this plan
    /// before advancing it. The exact successor still goes through full handoff.
    pub(crate) fn plan_native_removal_from(
        &mut self,
        previous: &Self,
    ) -> Result<bool, SessionPlaybackError> {
        if let Some(pending) = self.native_removal {
            return if pending.matches(previous) {
                Ok(true)
            } else {
                Err(SessionPlaybackError::InvalidCommand)
            };
        }
        if !self.require_continuous_insert
            || !previous.playing
            || self.live_restart
            || self.revision.arrangement().main_latency_frames()
                != previous.revision.arrangement().main_latency_frames()
        {
            return Ok(false);
        }
        let Some(transition) = self
            .native_removal_transition_from(previous)
            .map(RemovalTransition::Device)
            .or_else(|| {
                self.direct_audio_track_removal_from(previous)
                    .map(RemovalTransition::Track)
            })
        else {
            return Ok(false);
        };
        self.native_removal = Some(PendingNativeRemoval {
            transition,
            runtime: previous.capture_runtime,
            epoch: previous.capture_epoch,
            started: false,
        });
        Ok(true)
    }

    pub(crate) fn advance_planned_native_removal_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<NativeRemovalProgress, SessionPlaybackError> {
        let Some(pending) = self.native_removal.as_mut() else {
            return Ok(NativeRemovalProgress::NotApplicable);
        };
        if !pending.matches(previous) {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if !pending.started {
            let started = match pending.transition {
                RemovalTransition::Device(transition) => previous
                    .native_removal_chain_mut(transition.identity().owner())
                    .is_some_and(|chain| chain.begin_native_removal(transition)),
                RemovalTransition::Track(index) => {
                    let rate = previous.revision.arrangement().sample_rate();
                    previous.track_faders.get_mut(index).is_some_and(|fader| {
                        fader.begin_removal(rate);
                        true
                    })
                }
            };
            if !started {
                self.native_removal = None;
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            pending.started = true;
        }
        Ok(if pending.ready_on(previous) {
            NativeRemovalProgress::Ready
        } else {
            NativeRemovalProgress::Fading
        })
    }

    pub(crate) fn native_removal_started(&self) -> bool {
        self.native_removal.is_some_and(|pending| pending.started)
    }

    pub(crate) fn native_removal_pending(&self) -> bool {
        self.native_removal.is_some()
    }

    /// Return a pending removal and restore the original stage from its
    /// current wet/dry mix, including after a transport command invalidates it.
    pub fn cancel_native_removal_from(&mut self, previous: &mut Self) {
        if let Some(pending) = self.native_removal.take()
            && pending.started
            && pending.runtime == previous.capture_runtime
        {
            match pending.transition {
                RemovalTransition::Device(transition) => {
                    if let Some(chain) =
                        previous.native_removal_chain_mut(transition.identity().owner())
                    {
                        chain.cancel_native_removal(transition);
                    }
                }
                RemovalTransition::Track(index) => {
                    let arrangement = previous.revision.arrangement();
                    if let Some(track) = arrangement.tracks().get(index)
                        && let Some(fader) = previous.track_faders.get_mut(index)
                    {
                        fader.cancel_removal(track.mix().channel_gain(), arrangement.sample_rate());
                    }
                }
            }
        }
    }

    fn native_removal_ready(&self, transition: NativeRemovalTransition) -> bool {
        let chain = match transition.identity().owner() {
            ProcessorChainOwner::Track(id) => self
                .routes
                .iter()
                .position(|route| route.id.value() == id)
                .and_then(|index| self.device_chains.get(index)),
            ProcessorChainOwner::Return(id) => self
                .revision
                .arrangement()
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == id)
                .and_then(|index| self.return_chains.get(index)),
            ProcessorChainOwner::Main => Some(&self.main_chain),
            ProcessorChainOwner::Source(source) => self
                .source_effect_removal_receiver(source)
                .and_then(|receiver| self.source_effects.chain_for(receiver, source)),
        };
        chain.is_some_and(|chain| chain.native_removal_ready(transition))
    }

    fn native_removal_chain_mut(
        &mut self,
        owner: ProcessorChainOwner,
    ) -> Option<&mut NativeRuntimeChain> {
        match owner {
            ProcessorChainOwner::Track(id) => self
                .routes
                .iter()
                .position(|route| route.id.value() == id)
                .and_then(|index| self.device_chains.get_mut(index)),
            ProcessorChainOwner::Return(id) => self
                .revision
                .arrangement()
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == id)
                .and_then(|index| self.return_chains.get_mut(index)),
            ProcessorChainOwner::Main => Some(&mut self.main_chain),
            ProcessorChainOwner::Source(source) => {
                let receiver = self.source_effect_removal_receiver(source)?;
                self.source_effects.chain_for_mut(receiver, source)
            }
        }
    }

    fn source_effect_removal_receiver(&self, source: u64) -> Option<usize> {
        self.revision
            .arrangement()
            .tracks()
            .iter()
            .position(|track| {
                track.receives_source_audio()
                    && track.source_id().is_some_and(|id| id.value() == source)
            })
    }

    /// A direct audio track has no DSP or independent clock to detach. Its
    /// existing post-fader output already feeds both Main and ordinary sends.
    fn direct_audio_track_removal_from(&self, previous: &Self) -> Option<usize> {
        let old = previous.revision.arrangement().tracks();
        let new = self.revision.arrangement().tracks();
        if old.len().checked_sub(1) != Some(new.len()) {
            return None;
        }
        let remaining = new.iter().map(|track| track.canonical_track_id());
        let mut new_ids = remaining.clone();
        let index = old
            .iter()
            .position(|track| new_ids.next() != Some(track.canonical_track_id()))?;
        if !old
            .iter()
            .enumerate()
            .filter_map(|(i, track)| (i != index).then_some(track.canonical_track_id()))
            .eq(remaining)
        {
            return None;
        }
        let track = old.get(index)?;
        let id = track.canonical_track_id()?;
        let route = previous.routes.get(index)?;
        (route.id == id
            && route.owner == RouteOwner::Arrangement
            && !track.clips.is_empty()
            && track.device_chain.is_empty()
            && track.instrument_id.is_none()
            && track.source_id.is_none()
            && track.midi_source_index.is_none()
            && !track.has_gain_automation()
            && previous.revision.gain_recording_track() != Some(id)
            && self.direct_track_removal_keeps_timing(previous, id))
        .then_some(index)
    }

    fn direct_track_removal_keeps_timing(&self, previous: &Self, id: hex_project::TrackId) -> bool {
        match (&self.compensation, &previous.compensation) {
            (None, None) => true,
            (Some(next), Some(old)) => next.can_copy_from(
                old,
                Some(FreshHistory::RemovedTrack {
                    track: id,
                    staged: true,
                }),
            ),
            _ => false,
        }
    }

    fn native_removal_transition_from(&self, previous: &Self) -> Option<NativeRemovalTransition> {
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        if !old
            .tracks()
            .iter()
            .map(|track| track.canonical_track_id())
            .eq(new.tracks().iter().map(|track| track.canonical_track_id()))
            || !old
                .return_buses()
                .iter()
                .map(|bus| bus.id)
                .eq(new.return_buses().iter().map(|bus| bus.id))
        {
            return None;
        }
        let mut removal = None;
        for (current, old) in self
            .device_chains
            .iter()
            .zip(&previous.device_chains)
            .chain(self.return_chains.iter().zip(&previous.return_chains))
            .chain(std::iter::once((&self.main_chain, &previous.main_chain)))
        {
            if current.len() != old.len() {
                let transition = current.native_removal_transition_from(old)?;
                if removal.replace(transition).is_some() {
                    return None;
                }
            }
        }
        let nested = self
            .source_effects
            .native_removal_transition_from(&previous.source_effects);
        match (removal, nested) {
            (Some(_), Some(_)) => None,
            (main, nested) => main.or(nested),
        }
    }
}
