//! Stage-local removal on a retained playback source. Preparation previews the
//! existing handoff before changing the donor. Publication owns the outer
//! source fence and must cancel the fade when it returns a refused candidate.
use super::{SessionPlayback, SessionPlaybackError};
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
    pub(super) transition: NativeRemovalTransition,
    runtime: u64,
    epoch: u64,
    started: bool,
}

impl PendingNativeRemoval {
    pub(super) fn matches(self, previous: &SessionPlayback) -> bool {
        self.runtime == previous.capture_runtime && self.epoch == previous.capture_epoch
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
        let Some(transition) = self.native_removal_transition_from(previous) else {
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
            if !previous
                .native_removal_chain_mut(pending.transition.identity().owner())
                .is_some_and(|chain| chain.begin_native_removal(pending.transition))
            {
                self.native_removal = None;
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            pending.started = true;
        }
        Ok(if previous.native_removal_ready(pending.transition) {
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
            && let Some(chain) =
                previous.native_removal_chain_mut(pending.transition.identity().owner())
        {
            chain.cancel_native_removal(pending.transition);
        }
    }

    pub(super) fn native_removal_ready(&self, transition: NativeRemovalTransition) -> bool {
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
            ProcessorChainOwner::Source(_) => None,
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
            ProcessorChainOwner::Source(_) => None,
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
        removal
    }
}
