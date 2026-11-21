//! Direct Resident instrument removal uses its original route fader before the cut.
use super::{RouteOwner, SessionPlayback};
use hex_devices::{
    fm4::device::FM4_TYPE_ID, operator_state::OPERATOR_TYPE_ID,
    sampler::state::TYPE_ID as SAMPLER_TYPE_ID,
};

impl SessionPlayback {
    pub(super) fn direct_analytic_transition_graph_matches(&self, previous: &Self) -> bool {
        if self.compensation.is_some()
            || previous.compensation.is_some()
            || self.revision.contains_streamed_sources()
            || previous.revision.contains_streamed_sources()
            || self.computer_keyboard.is_some()
            || previous.computer_keyboard.is_some()
            || self.live_expression.is_some()
            || previous.live_expression.is_some()
            || !self.current_sampler_keyboards.is_empty()
            || !previous.current_sampler_keyboards.is_empty()
            || !self.foreign_arrangement.is_empty()
            || !previous.foreign_arrangement.is_empty()
            || self.revision.source_routing().has_shared_sources()
            || previous.revision.source_routing().has_shared_sources()
        {
            return false;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        old.tracks()
            .iter()
            .map(|t| t.canonical_track_id())
            .eq(new.tracks().iter().map(|t| t.canonical_track_id()))
            && self.main_chain.len() == previous.main_chain.len()
            && self
                .main_chain
                .configuration_matches_range(&previous.main_chain, 0..previous.main_chain.len())
            && self.return_chains.len() == previous.return_chains.len()
            && self
                .return_chains
                .iter()
                .zip(&previous.return_chains)
                .all(|(a, b)| a.len() == b.len() && a.configuration_matches_range(b, 0..b.len()))
            && self
                .device_chains
                .iter()
                .zip(&previous.device_chains)
                .all(|(a, b)| a.len() == b.len() && a.configuration_matches_range(b, 0..b.len()))
    }

    pub(super) fn direct_analytic_transition_route(
        &self,
        index: usize,
        track: &crate::Track,
    ) -> bool {
        self.routes.get(index).is_some_and(|route| {
            matches!(route.owner, RouteOwner::Arrangement)
                && route.pending.is_none()
                && route.audio_release.is_none()
                && track
                    .source_receiver_index
                    .is_none_or(|receiver| receiver == index)
                && track.midi_source_index.is_none()
                && !track.has_gain_automation()
                && track.sends.is_empty()
                && track.device_chain.is_empty()
                && !self.source_effects.has_chain(index)
                && self.revision.gain_recording_track() != Some(route.id)
        })
    }

    pub(super) fn direct_instrument_removal_from(&self, previous: &Self) -> Option<usize> {
        if !self.direct_analytic_transition_graph_matches(previous) {
            return None;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        let mut removed = None;
        for (index, (old, new)) in old.tracks().iter().zip(new.tracks()).enumerate() {
            if old.instrument_config_matches(new) {
                continue;
            }
            if removed.replace(index).is_some()
                || !old.instrument_slot_enabled()
                || !previous.direct_removable_instrument(index, old)
                || old.foreign_instrument.is_some()
                || old.instrument_latency != Some(0)
                // Authored MIDI templates remain after the canonical slot is removed.
                || new.instrument_id.is_some()
                || old.mix != new.mix
                || !old.clips.is_empty()
                || !new.clips.is_empty()
            {
                return None;
            }
            if !previous.direct_analytic_transition_route(index, old)
                || !self.direct_analytic_transition_route(index, new)
            {
                return None;
            }
        }
        removed
    }

    fn direct_removable_instrument(&self, index: usize, track: &crate::Track) -> bool {
        let Some(state) = track.instrument_state.as_ref() else {
            return false;
        };
        let kind = state.type_id();
        match (kind, track.has_mutable_instrument()) {
            (hex_project::DEFAULT_INSTRUMENT_TYPE_ID, false) => true,
            (FM4_TYPE_ID | OPERATOR_TYPE_ID | SAMPLER_TYPE_ID, true) => {
                let Some(id) = track.canonical_track_id() else {
                    return false;
                };
                self.mutable_instruments
                    .get(index)
                    .and_then(Option::as_ref)
                    .is_some_and(|runtime| {
                        runtime.is_source_owner()
                            && runtime.expression_identity == track.instrument_id
                            && (kind != SAMPLER_TYPE_ID || runtime.resources.is_some())
                            && !runtime.has_committed_live_occurrences()
                            && runtime
                                .transition_proofs(false)
                                .iter()
                                .all(|proof| proof.signature.key().track == id)
                    })
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PreparedRevision, SessionPlaybackCommand};
    use hex_project::{BeatTime, Command, Created, Editor, Project, TrackDeviceSlot};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn direct_sine_removal_stages_only_its_original_route() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "sine".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let table = Arc::new(crate::SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(
                PreparedRevision::prepare(project, &BTreeMap::new(), Arc::clone(&table), None)
                    .unwrap(),
            )
        };
        let before = prepare(editor.project());
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        let after = prepare(editor.project());
        let mut old = SessionPlayback::new(before).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        let mut next = SessionPlayback::new(after).unwrap();
        assert_eq!(next.direct_instrument_removal_from(&old), Some(0));
        next.routes[0].owner = RouteOwner::Silence;
        assert!(next.direct_instrument_removal_from(&old).is_none());
        next.routes[0].owner = RouteOwner::Arrangement;
        next.require_continuous_insert = true;
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Fading
        );
        old.render(&mut [[0.0; 2]; 64]).unwrap();
        next.cancel_native_removal_from(&mut old);
        old.render(&mut [[0.0; 2]; 240]).unwrap();
        assert!(!old.track_faders[0].active());
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Fading
        );
        old.render(&mut [[0.0; 2]; 240]).unwrap();
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Ready
        );
        next.try_inherit_state_live_from(&mut old).unwrap();
        assert!(next.releases.is_empty());
        let mut output = [[1.0; 2]; 64];
        next.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 64]);
    }
}
