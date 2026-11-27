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

    fn direct_instrument_removal_graph_matches(&self, previous: &Self) -> bool {
        if self.compensation.is_some()
            || previous.compensation.is_some()
            || self.revision.contains_streamed_sources()
            || previous.revision.contains_streamed_sources()
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
            && old
                .return_buses()
                .iter()
                .map(|b| b.id)
                .eq(new.return_buses().iter().map(|b| b.id))
            && old
                .return_feed_plan
                .edges()
                .eq(new.return_feed_plan.edges())
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

    fn direct_instrument_removal_route(&self, index: usize, track: &crate::Track) -> bool {
        self.routes.get(index).is_some_and(|route| {
            matches!(route.owner, RouteOwner::Arrangement)
                && route.pending.is_none()
                && route.audio_release.is_none()
                && track
                    .source_receiver_index
                    .is_none_or(|receiver| receiver == index)
                && track.midi_source_index.is_none()
                && !track.has_gain_automation()
                && track.send_envelopes.iter().all(Option::is_none)
                && track.device_chain.len()
                    == self
                        .device_chains
                        .get(index)
                        .map_or(usize::MAX, |c| c.len())
                && track.device_chain.iter().all(|node| {
                    node.foreign_processor().is_none()
                        && node.algorithmic_latency_frames() == Some(0)
                })
                && !self.source_effects.has_chain(index)
                && self.revision.gain_recording_track() != Some(route.id)
        })
    }

    fn direct_instrument_removal_keyboard_matches(
        &self,
        previous: &Self,
        removed_track: hex_project::TrackId,
    ) -> Option<bool> {
        if self
            .computer_keyboard
            .as_ref()
            .is_some_and(|keyboard| keyboard.track() == removed_track)
        {
            return None;
        }
        match (
            previous.computer_keyboard.as_ref(),
            self.computer_keyboard.as_ref(),
        ) {
            (None, None) => Some(false),
            (Some(old_kb), None) if old_kb.track() == removed_track => Some(true),
            (Some(old_kb), None) if old_kb.track() != removed_track => Some(false),
            (Some(old_kb), Some(new_kb))
                if old_kb.track() != removed_track && new_kb.track() == old_kb.track() =>
            {
                Some(false)
            }
            _ => None,
        }
    }

    pub(super) fn direct_instrument_removal_from(&self, previous: &Self) -> Option<usize> {
        if !self.direct_instrument_removal_graph_matches(previous) {
            return None;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        let mut removed = None;
        for (index, (old, new)) in old.tracks().iter().zip(new.tracks()).enumerate() {
            if old.instrument_config_matches(new) {
                continue;
            }
            let track_id = old.canonical_track_id()?;
            let allow_live_occurrences =
                self.direct_instrument_removal_keyboard_matches(previous, track_id)?;
            if removed.replace(index).is_some()
                || !old.instrument_slot_enabled()
                || !previous.direct_removable_instrument(index, old, allow_live_occurrences)
                || old.foreign_instrument.is_some()
                || old.instrument_latency != Some(0)
                // Authored MIDI templates remain after the canonical slot is removed.
                || new.instrument_id.is_some()
                || old.mix != new.mix
                || old.sends != new.sends
                || old.instrument_position() != new.instrument_position()
                || !old.clips.is_empty()
                || !new.clips.is_empty()
            {
                return None;
            }
            if !previous.direct_instrument_removal_route(index, old)
                || !self.direct_instrument_removal_route(index, new)
            {
                return None;
            }
        }
        removed
    }

    fn direct_removable_instrument(
        &self,
        index: usize,
        track: &crate::Track,
        allow_live_occurrences: bool,
    ) -> bool {
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
                            && (allow_live_occurrences || !runtime.has_committed_live_occurrences())
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

    #[test]
    fn routed_instrument_removal_ends_track_effect_and_keeps_shared_return_bed() {
        use hex_devices::delay::DEFINITION as DELAY;
        use hex_devices::fm4::device::FM4_TYPE_ID;
        use hex_devices::utility::UTILITY_DEFINITION;
        use hex_lattice_contract::NativeDeviceMetadata;

        const RATE: u32 = 48_000;
        const FADE: usize = 240;
        const BED: crate::Frame = [0.2, -0.1];
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
        let Created::Imported {
            track: bed_track,
            asset: bed_asset,
            ..
        } = editor
            .execute(Command::ImportAudioTrack {
                name: "bed".into(),
                path: "bed.wav".into(),
                sample_rate: RATE,
                frames: 8192,
            })
            .unwrap()
        else {
            panic!("bed track");
        };
        let Created::Track(inst_track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "fm4".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Instrument,
                    FM4_TYPE_ID,
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("instrument track");
        };
        editor
            .execute(Command::InsertDevice {
                track: inst_track,
                before: None,
                state: DELAY.default_state().unwrap(),
            })
            .unwrap();
        let Created::ReturnTrack(shared_return) = editor
            .execute(Command::AddReturnTrack {
                name: "shared".into(),
            })
            .unwrap()
        else {
            panic!("return track");
        };
        editor
            .execute(Command::InsertReturnDevice {
                return_track: shared_return,
                before: None,
                state: UTILITY_DEFINITION.default_state().unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetReturnSend {
                track: bed_track,
                return_track: shared_return,
                gain: 0.5,
            })
            .unwrap();
        editor
            .execute(Command::SetReturnSend {
                track: inst_track,
                return_track: shared_return,
                gain: 0.5,
            })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: inst_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(4).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();

        let media =
            BTreeMap::from([(bed_asset, crate::Audio::new(RATE, vec![BED; 8192]).unwrap())]);
        let table = Arc::new(crate::SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let before = prepare(editor.project());
        editor
            .execute(Command::RemoveInstrument { track: inst_track })
            .unwrap();
        let after = prepare(editor.project());

        let mut old = SessionPlayback::new(before).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 512]).unwrap();

        let mut next = SessionPlayback::new(after).unwrap();
        assert_eq!(next.direct_instrument_removal_from(&old), Some(1));
        next.require_continuous_insert = true;
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Fading
        );
        old.render(&mut [[0.0; 2]; FADE]).unwrap();
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Ready
        );
        next.try_inherit_state_live_from(&mut old).unwrap();
        assert!(next.releases.is_empty());
        assert!(next.tail_groups.iter().all(Option::is_none));
        assert!(next.tail_mutable_instruments.iter().all(Option::is_none));

        // After the cut, track 1 (instrument + track Delay) contributes pure silence while
        // track 0 (bed) + its 0.5 send through the shared Return remain active (1.5 * BED).
        let mut post = [[0.0; 2]; 128];
        next.render(&mut post).unwrap();
        let expected_bed_plus_return = [BED[0] * 1.5, BED[1] * 1.5];
        for frame in post {
            assert!((frame[0] - expected_bed_plus_return[0]).abs() < 1e-5);
            assert!((frame[1] - expected_bed_plus_return[1]).abs() < 1e-5);
        }
    }

    #[test]
    fn live_keyboard_instrument_removal_fades_and_clears_held_notes_without_tails() {
        use crate::computer_keyboard::ComputerKeyboardInput;
        use hex_devices::fm4::device::FM4_TYPE_ID;

        const RATE: u32 = 48_000;
        const FADE: usize = 240;
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "live fm4".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Instrument,
                    FM4_TYPE_ID,
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let original_project = editor.project().clone();
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        let removed_project = editor.project().clone();

        let table = Arc::new(crate::SineTable::new());
        let before = Arc::new(
            PreparedRevision::prepare(
                &original_project,
                &BTreeMap::new(),
                Arc::clone(&table),
                None,
            )
            .unwrap(),
        );
        let after = Arc::new(
            PreparedRevision::prepare(&removed_project, &BTreeMap::new(), Arc::clone(&table), None)
                .unwrap(),
        );

        let keys = Arc::new(ComputerKeyboardInput::default());
        keys.set_key(0, Some((60, 100, 1))).unwrap();

        let mut old = SessionPlayback::new(before).unwrap();
        let keyboard = crate::computer_keyboard::ComputerKeyboardRuntime::prepare_source_input(
            &original_project,
            track,
            Arc::clone(&keys),
            old.source_runtime_on_control(0),
            None,
            62,
        )
        .unwrap();
        old.computer_keyboard = Some(Box::new(keyboard));
        old.apply(SessionPlaybackCommand::Play).unwrap();
        let mut sounding = [[0.0; 2]; 256];
        old.render(&mut sounding).unwrap();
        assert!(sounding.iter().any(|frame| frame[0].abs() > 1e-3));

        let mut next = SessionPlayback::new(after).unwrap();
        assert_eq!(next.direct_instrument_removal_from(&old), Some(0));
        next.require_continuous_insert = true;
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Fading
        );
        // Cancel mid-fade recovers live output smoothly.
        old.render(&mut [[0.0; 2]; 64]).unwrap();
        next.cancel_native_removal_from(&mut old);
        let mut recovered = [[0.0; 2]; FADE];
        old.render(&mut recovered).unwrap();
        assert!(!old.track_faders[0].active());
        assert!(recovered[FADE - 1][0].abs() > 1e-4);

        // Re-stage and finish removal -> pure silence, zero detached tails.
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Fading
        );
        old.render(&mut [[0.0; 2]; FADE]).unwrap();
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Ready
        );
        next.try_inherit_state_live_from(&mut old).unwrap();
        assert!(next.releases.is_empty());
        assert!(next.tail_mutable_instruments.iter().all(Option::is_none));
        let mut post = [[1.0; 2]; 64];
        next.render(&mut post).unwrap();
        assert_eq!(post, [[0.0; 2]; 64]);
    }
}
