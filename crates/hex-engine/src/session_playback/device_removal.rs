//! A short fade on the departing native stage or audio track. Preparation
//! previews the existing handoff before changing the donor. Publication owns
//! the outer source fence and restores the donor when it returns a candidate.
use super::{RouteOwner, SessionPlayback, SessionPlaybackError};
use crate::signal_graph::FreshHistory;
use hex_lattice::{NativeRemovalTransition, NativeRuntimeChain, ProcessorChainOwner};
use hex_lattice_contract::NativeDeviceMetadata;

/// Progress of a staged native removal at a render boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeRemovalProgress {
    NotApplicable,
    Fading,
    Ready,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Audio, Frame, LaunchQuantization, PreparedRevision, SessionPlaybackCommand};
    use hex_project::{Command, Created, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn session_audio_track_removal_fades_the_live_route_before_handoff() {
        const RATE: u32 = 48_000;
        const FADE: usize = 240;
        const SIGNAL: Frame = [0.25, -0.125];
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "session audio".into(),
                path: "session.wav".into(),
                sample_rate: RATE,
                frames: 2048,
            })
            .unwrap()
        else {
            panic!("audio");
        };
        let clip = editor.project().track(track).unwrap().clips()[0].id();
        editor.execute(Command::RemoveClip(clip)).unwrap();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene");
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "session".into(),
                asset,
                source_start: 0,
                source_end: 2048,
            })
            .unwrap()
        else {
            panic!("session clip");
        };
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![SIGNAL; 2048]).unwrap())]);
        let table = Arc::new(crate::SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let before = prepare(editor.project());
        editor.execute(Command::RemoveTrack(track)).unwrap();
        let after = prepare(editor.project());
        for cancel in [false, true] {
            let mut old = SessionPlayback::new(Arc::clone(&before)).unwrap();
            old.apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            let mut prefix = [[0.0; 2]; 32];
            old.render(&mut prefix).unwrap();
            assert_eq!(prefix, [SIGNAL; 32]);
            let mut next = SessionPlayback::new(Arc::clone(&after)).unwrap();
            next.require_continuous_insert = true;
            assert_eq!(
                next.advance_native_removal_from(&mut old).unwrap(),
                NativeRemovalProgress::Fading
            );
            old.render(&mut prefix).unwrap();
            for (index, frame) in prefix.iter().enumerate() {
                let gain = 1.0 - (index + 1) as f32 / FADE as f32;
                assert!((frame[0] - SIGNAL[0] * gain).abs() < 1e-6);
            }
            if cancel {
                next.cancel_native_removal_from(&mut old);
                let mut restored = [[0.0; 2]; FADE];
                old.render(&mut restored).unwrap();
                assert!(restored[0][0] > prefix[31][0]);
                assert_eq!(restored[FADE - 1], SIGNAL);
            } else {
                let mut rest = [[0.0; 2]; FADE - 32];
                old.render(&mut rest).unwrap();
                assert_eq!(rest[FADE - 33], [0.0; 2]);
                assert_eq!(
                    next.advance_native_removal_from(&mut old).unwrap(),
                    NativeRemovalProgress::Ready
                );
                next.try_inherit_state_live_from(&mut old).unwrap();
                let mut output = [[0.0; 2]; 32];
                next.render(&mut output).unwrap();
                assert_eq!(output, [[0.0; 2]; 32]);
                assert_eq!(next.performance_frame(), (32 + FADE + 32) as u64);
            }
        }
    }
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
    Reorder(usize),
    Move(super::device_move::NativeTrackMove),
}

impl PendingNativeRemoval {
    pub(super) fn matches(self, previous: &SessionPlayback) -> bool {
        self.runtime == previous.capture_runtime && self.epoch == previous.capture_epoch
    }

    pub(super) fn device_transition(self) -> Option<NativeRemovalTransition> {
        match self.transition {
            RemovalTransition::Device(transition) => Some(transition),
            RemovalTransition::Track(_)
            | RemovalTransition::Reorder(_)
            | RemovalTransition::Move(_) => None,
        }
    }

    pub(super) fn track_move(self) -> Option<super::device_move::NativeTrackMove> {
        match self.transition {
            RemovalTransition::Move(movement) => Some(movement),
            _ => None,
        }
    }

    pub(super) fn faded_native_identity(self) -> Option<hex_lattice::ProcessorInstanceIdentity> {
        self.device_transition()
            .map(|transition| transition.identity())
            .or_else(|| self.track_move().map(|movement| movement.identity))
    }

    pub(super) fn faded_track(self, previous: &SessionPlayback, id: hex_project::TrackId) -> bool {
        self.reordered_track(previous) == Some(id)
            || self.track_move().is_some_and(|movement| {
                [movement.source, movement.destination]
                    .into_iter()
                    .any(|index| {
                        previous
                            .routes
                            .get(index)
                            .is_some_and(|route| route.id == id)
                    })
            })
    }

    pub(super) fn reordered_track(
        self,
        previous: &SessionPlayback,
    ) -> Option<hex_project::TrackId> {
        match self.transition {
            RemovalTransition::Reorder(index) => previous.routes.get(index).map(|route| route.id),
            _ => None,
        }
    }

    pub(super) fn staged_track_id(
        self,
        previous: &SessionPlayback,
    ) -> Option<hex_project::TrackId> {
        match self.transition {
            RemovalTransition::Track(index) => previous.routes.get(index).map(|route| route.id),
            RemovalTransition::Device(_)
            | RemovalTransition::Reorder(_)
            | RemovalTransition::Move(_) => None,
        }
    }

    pub(super) fn ready_on(self, previous: &SessionPlayback) -> bool {
        match self.transition {
            RemovalTransition::Move(movement) => [movement.source, movement.destination]
                .into_iter()
                .all(|index| {
                    previous
                        .track_faders
                        .get(index)
                        .is_some_and(|fader| fader.removal_ready())
                }),
            RemovalTransition::Device(transition) => previous.native_removal_ready(transition),
            RemovalTransition::Track(index) | RemovalTransition::Reorder(index) => {
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
            .or_else(|| {
                self.direct_audio_track_reorder_from(previous)
                    .map(RemovalTransition::Reorder)
            })
            .or_else(|| {
                self.direct_audio_track_move_from(previous)
                    .map(RemovalTransition::Move)
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
                RemovalTransition::Move(movement) => {
                    let indices = [movement.source, movement.destination];
                    let valid = indices
                        .into_iter()
                        .all(|index| previous.track_faders.get(index).is_some());
                    if valid {
                        let rate = previous.revision.arrangement().sample_rate();
                        for index in indices {
                            if let Some(fader) = previous.track_faders.get_mut(index) {
                                fader.begin_removal(rate);
                            }
                        }
                    }
                    valid
                }
                RemovalTransition::Device(transition) => previous
                    .native_removal_chain_mut(transition.identity().owner())
                    .is_some_and(|chain| chain.begin_native_removal(transition)),
                RemovalTransition::Track(index) | RemovalTransition::Reorder(index) => {
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
                RemovalTransition::Move(movement) => {
                    let arrangement = previous.revision.arrangement();
                    for index in [movement.source, movement.destination] {
                        if let Some(track) = arrangement.tracks().get(index)
                            && let Some(fader) = previous.track_faders.get_mut(index)
                        {
                            fader.cancel_removal(
                                track.mix().channel_gain(),
                                arrangement.sample_rate(),
                            );
                        }
                    }
                }
                RemovalTransition::Device(transition) => {
                    if let Some(chain) =
                        previous.native_removal_chain_mut(transition.identity().owner())
                    {
                        chain.cancel_native_removal(transition);
                    }
                }
                RemovalTransition::Track(index) | RemovalTransition::Reorder(index) => {
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

    /// A direct audio track's post-fader output feeds Main and ordinary sends.
    /// One native zero-latency Delay can keep processing through that fade
    /// and retire with the donor without attaching a separate tail owner.
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
        let direct_audio = match route.owner {
            RouteOwner::Arrangement => !track.clips.is_empty(),
            RouteOwner::SessionAudio { clip, .. } => previous
                .revision
                .session()
                .audio_clip(clip)
                .is_some_and(|clip| clip.track == id),
            RouteOwner::Session { .. } | RouteOwner::Silence => false,
        };
        let can_retire_chain = match track.device_chain.as_ref() {
            [] => true,
            [device] => {
                matches!(route.owner, RouteOwner::Arrangement)
                    && route.pending.is_none()
                    && route.audio_release.is_none()
                    && self.compensation.is_none()
                    && previous.compensation.is_none()
                    && !self.revision.contains_streamed_sources()
                    && !previous.revision.contains_streamed_sources()
                    && device.device_type == hex_devices::delay::DEFINITION.type_id()
                    && device.foreign_processor().is_none()
                    && device.algorithmic_latency_frames() == Some(0)
            }
            _ => false,
        };
        (route.id == id
            && direct_audio
            && can_retire_chain
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
