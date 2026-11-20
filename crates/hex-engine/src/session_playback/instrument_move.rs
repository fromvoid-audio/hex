//! Classify one unchanged native Sine move between direct Resident MIDI routes.
use super::{SessionPlayback, device_move::same_clip};
use crate::Track;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct AnalyticTrackMove {
    pub(super) source: usize,
    pub(super) destination: usize,
    pub(super) source_id: hex_project::SourceId,
}

impl SessionPlayback {
    pub(super) fn direct_analytic_instrument_move_from(
        &self,
        previous: &Self,
    ) -> Option<AnalyticTrackMove> {
        if !self.direct_analytic_transition_graph_matches(previous) {
            return None;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        if old.sample_rate != new.sample_rate
            || old.tempo_map != new.tempo_map
            || old.loop_region != new.loop_region
            || old.master_gain != new.master_gain
            || !old.midi_routing.is_empty()
            || !new.midi_routing.is_empty()
        {
            return None;
        }
        let mut source = None;
        let mut destination = None;
        for (index, (before, after)) in old.tracks().iter().zip(new.tracks()).enumerate() {
            // Only the two instrument routes fade. An independently prepared
            // candidate must not use that receipt to replace unrelated PCM.
            if before.clips.len() != after.clips.len()
                || !before
                    .clips
                    .iter()
                    .zip(&after.clips)
                    .all(|(a, b)| same_clip(a, b))
            {
                return None;
            }
            if before.instrument_config_matches(after) && before.source_id == after.source_id {
                continue;
            }
            if before.mix != after.mix
                || !before.clips.is_empty()
                || !after.clips.is_empty()
                || !previous.direct_analytic_transition_route(index, before)
                || !self.direct_analytic_transition_route(index, after)
            {
                return None;
            }
            match (before.instrument_id, after.instrument_id) {
                (Some(_), None)
                    if before.direct_native_sine(index)
                        && after.source_id.is_none()
                        && source.is_none() =>
                {
                    source = Some((index, before));
                }
                (None, Some(_))
                    if after.direct_native_sine(index)
                        && before.source_id.is_none()
                        && destination.is_none() =>
                {
                    destination = Some((index, after));
                }
                _ => return None,
            }
        }
        let (source, donor) = source?;
        let (destination, receiver) = destination?;
        let source_id = donor.source_id?;
        let donor_track = donor.canonical_track_id()?;
        // An earlier cut may still own a held gate. Only transfer releases
        // already carried by this exact generator on its current output.
        if previous.releases.iter().any(|release| {
            let output = release.location.track_index();
            release.end_frame() > previous.performance_frame
                && release.location.tail_group().is_none()
                && [source, destination].contains(&output)
                && !(output == source
                    && release.source_origin.is_some_and(|origin| {
                        origin.source == source_id
                            && origin.receiver == donor_track
                            && origin.moved_sine
                            && origin.follows_current_configuration
                    }))
        }) {
            return None;
        }
        (receiver.source_id == Some(source_id) && donor.instrument_config_matches(receiver))
            .then_some(AnalyticTrackMove {
                source,
                destination,
                source_id,
            })
    }
}

impl Track {
    pub(super) fn direct_native_sine(&self, index: usize) -> bool {
        self.instrument_slot_enabled()
            && self.source_receiver_index == Some(index)
            && self.receives_source_audio()
            && !self.has_mutable_instrument()
            && self.instrument_latency == Some(0)
            && self.foreign_instrument.is_none()
            && self
                .instrument_state
                .as_ref()
                .is_some_and(|state| state.type_id() == hex_project::DEFAULT_INSTRUMENT_TYPE_ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PreparedRevision, SessionPlaybackCommand};
    use hex_project::{BeatTime, Command, Created, DeviceSelectionIds, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn direct_sine_move_stages_two_flat_routes() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut track = |name: &str| {
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack { name: name.into() })
                .unwrap()
            else {
                panic!("midi track");
            };
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("midi clip");
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
            track
        };
        let source = track("source");
        let destination = track("destination");
        let Created::Imported {
            track: unrelated,
            asset,
            clip,
        } = editor
            .execute(Command::ImportAudioTrack {
                name: "dry".into(),
                path: "memory/dry.wav".into(),
                sample_rate: 48_000,
                frames: 1_024,
            })
            .unwrap()
        else {
            panic!("audio track");
        };
        editor
            .execute(Command::InsertInstrument {
                track: source,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let device = editor
            .project()
            .track(source)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let table = Arc::new(crate::SineTable::new());
        let media = BTreeMap::from([(
            asset,
            crate::Audio::new(48_000, vec![[0.125; 2]; 1_024]).unwrap(),
        )]);
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let mut before = SessionPlayback::new(prepare(editor.project())).unwrap();
        before.apply(SessionPlaybackCommand::Play).unwrap();
        before.render(&mut [[0.0; 2]; 512]).unwrap();
        editor
            .execute(Command::MoveTrackDevices {
                source,
                destination,
                selected: DeviceSelectionIds::new(&[device]).unwrap(),
            })
            .unwrap();
        let mut after = SessionPlayback::new(prepare(editor.project())).unwrap();

        let movement = after
            .direct_analytic_instrument_move_from(&before)
            .expect("paired classification");
        assert_eq!((movement.source, movement.destination), (0, 1));
        let mut independent_mix = Editor::new(editor.project().clone(), 8).unwrap();
        independent_mix
            .execute(Command::Gain {
                track: unrelated,
                gain: 0.5,
            })
            .unwrap();
        let compatible = SessionPlayback::new(prepare(independent_mix.project())).unwrap();
        assert!(
            compatible
                .direct_analytic_instrument_move_from(&before)
                .is_some()
        );

        let mut compound = Editor::new(editor.project().clone(), 8).unwrap();
        compound.execute(Command::RemoveClip(clip)).unwrap();
        let changed = SessionPlayback::new(prepare(compound.project())).unwrap();
        assert!(
            changed
                .direct_analytic_instrument_move_from(&before)
                .is_none()
        );
        let mut grouped = Editor::new(editor.project().clone(), 8).unwrap();
        grouped
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[source, destination]).unwrap(),
                name: "group".into(),
            })
            .unwrap();
        let grouped_after = SessionPlayback::new(prepare(grouped.project())).unwrap();
        grouped
            .execute(Command::MoveTrackDevices {
                source: destination,
                destination: source,
                selected: DeviceSelectionIds::new(&[device]).unwrap(),
            })
            .unwrap();
        let grouped_before = SessionPlayback::new(prepare(grouped.project())).unwrap();
        assert!(grouped_before.compensation.is_some());
        assert!(
            grouped_after
                .direct_analytic_instrument_move_from(&grouped_before)
                .is_none()
        );

        after.require_continuous_insert = true;
        assert_eq!(
            after.advance_native_removal_from(&mut before).unwrap(),
            crate::session_playback::NativeRemovalProgress::Fading
        );
    }
}
