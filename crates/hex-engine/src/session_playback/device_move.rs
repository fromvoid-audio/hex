//! One direct Resident audio-track effect move, cut with both route faders.
use super::{RouteOwner, SessionPlayback};
use hex_lattice::ProcessorInstanceIdentity;
use hex_lattice_contract::NativeDeviceMetadata;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct NativeTrackMove {
    pub(super) source: usize,
    pub(super) destination: usize,
    pub(super) identity: ProcessorInstanceIdentity,
}

impl SessionPlayback {
    pub(super) fn direct_audio_track_move_from(&self, previous: &Self) -> Option<NativeTrackMove> {
        // The shared paged planner can reach this proof before page readiness
        // or custody is established, including for unrelated streamed tracks.
        if self.revision.contains_streamed_sources()
            || previous.revision.contains_streamed_sources()
            || self.compensation.is_some()
            || previous.compensation.is_some()
        {
            return None;
        }
        if self.main_chain.len() != previous.main_chain.len()
            || !self
                .main_chain
                .configuration_matches_range(&previous.main_chain, 0..previous.main_chain.len())
            || self.return_chains.len() != previous.return_chains.len()
            || self
                .return_chains
                .iter()
                .zip(&previous.return_chains)
                .any(|(new, old)| {
                    new.len() != old.len() || !new.configuration_matches_range(old, 0..old.len())
                })
        {
            return None;
        }
        let old = previous.revision.arrangement().tracks();
        let new = self.revision.arrangement().tracks();
        if !old
            .iter()
            .map(|t| t.canonical_track_id())
            .eq(new.iter().map(|t| t.canonical_track_id()))
        {
            return None;
        }
        let (mut source, mut destination) = (None, None);
        for (index, (next, previous_chain)) in self
            .device_chains
            .iter()
            .zip(&previous.device_chains)
            .enumerate()
        {
            if next.len() == previous_chain.len()
                && next.configuration_matches_range(previous_chain, 0..previous_chain.len())
            {
                continue;
            }
            let lengths = (previous_chain.len(), next.len());
            if matches!(lengths, (1, 2) | (2, 1)) {
                let before = old.get(index)?.device_chain.first()?;
                let after = new.get(index)?.device_chain.first()?;
                let supported_prefix = |prefix: &hex_lattice::PreparedRuntimeInstance| {
                    prefix.device_type == hex_devices::utility::UTILITY_TYPE_ID
                        && prefix.algorithmic_latency_frames() == Some(0)
                        && !prefix.bypassed
                        && prefix.automation.is_empty()
                        && prefix.modulation.is_none()
                };
                if !next.configuration_matches_range(previous_chain, 0..1)
                    || ![before, after].into_iter().all(supported_prefix)
                {
                    return None;
                }
            }
            match lengths {
                (1, 0) | (2, 1) if source.replace(index).is_none() => {}
                (0, 1) | (1, 2) if destination.replace(index).is_none() => {}
                _ => return None,
            }
        }
        let (source, destination) = (source?, destination?);
        for index in [source, destination] {
            let old = old.get(index)?;
            let new = new.get(index)?;
            if old.mix != new.mix
                || old.clips.len() != new.clips.len()
                || !old
                    .clips
                    .iter()
                    .zip(&new.clips)
                    .all(|(a, b)| same_clip(a, b))
            {
                return None;
            }
        }
        // An empty receiver can start its fresh effect on silence. Pending
        // Arrangement ownership and earlier route releases use their own cuts.
        for playback in [self, previous] {
            for index in [source, destination] {
                let track = playback.revision.arrangement().tracks().get(index)?;
                let route = playback.routes.get(index)?;
                if !matches!(route.owner, RouteOwner::Arrangement)
                    || route.pending.is_some()
                    || route.audio_release.is_some()
                    || track.instrument_id.is_some()
                    || track.source_id.is_some()
                    || track.midi_source_index.is_some()
                    || track.has_gain_automation()
                    || !track.sends.is_empty()
                    || playback.revision.gain_recording_track() == Some(route.id)
                    || track.device_chain.len() != playback.device_chains.get(index)?.len()
                {
                    return None;
                }
            }
        }
        // Provider-specific trust belongs here, not in Lattice's generic proof.
        let device_type = old.get(source)?.device_chain.last()?.device_type;
        if device_type != hex_devices::delay::DEFINITION.type_id()
            && device_type != hex_devices::reverb::REVERB_TYPE_ID
        {
            return None;
        }
        let identity = self
            .device_chains
            .get(destination)?
            .cross_track_native_identity_from(previous.device_chains.get(source)?)?;
        Some(NativeTrackMove {
            source,
            destination,
            identity,
        })
    }
}

pub(super) fn same_clip(a: &crate::Clip, b: &crate::Clip) -> bool {
    a.active == b.active
        && a.start == b.start
        && a.end == b.end
        && a.looping == b.looping
        && a.source_offset == b.source_offset
        && a.source_length == b.source_length
        && a.audio
            .as_ref()
            .zip(b.audio.as_ref())
            .is_some_and(|(a, b)| {
                a.sample_rate == b.sample_rate && std::sync::Arc::ptr_eq(&a.frames, &b.frames)
            })
        && match (&a.metadata, &b.metadata) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.gain == b.gain
                    && a.source_phase == b.source_phase
                    && a.reversed == b.reversed
                    && a.fade == b.fade
                    && a.streamed.is_none()
                    && b.streamed.is_none()
                    && a.sparse.is_none()
                    && b.sparse.is_none()
            }
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Audio, PreparedRevision, SessionPlaybackCommand};
    use hex_project::{Command, Created, DeviceSelectionIds, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn exact_delay_move_stages_and_restores_both_routes() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut media = BTreeMap::new();
        let mut tracks = Vec::new();
        let mut assets = Vec::new();
        for index in 0..3 {
            let Created::Imported { track, asset, .. } = editor
                .execute(Command::ImportAudioTrack {
                    name: format!("track {index}"),
                    path: format!("{index}.wav").into(),
                    sample_rate: 48_000,
                    frames: 2048,
                })
                .unwrap()
            else {
                panic!("track")
            };
            tracks.push(track);
            assets.push(asset);
            let frame = if index == 2 { [0.0; 2] } else { [0.25; 2] };
            media.insert(asset, Audio::new(48_000, vec![frame; 2048]).unwrap());
        }
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track: tracks[0],
                before: None,
                state: hex_devices::delay::DEFINITION.default_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("delay")
        };
        let prepare = |project: &Project| {
            Arc::new(
                PreparedRevision::prepare(project, &media, Arc::new(crate::SineTable::new()), None)
                    .unwrap(),
            )
        };
        let before_project = editor.project().clone();
        let before = prepare(&before_project);
        editor
            .execute(Command::MoveTrackDevices {
                source: tracks[0],
                destination: tracks[1],
                selected: DeviceSelectionIds::new(&[device]).unwrap(),
            })
            .unwrap();
        let after = prepare(editor.project());
        // Only an unrelated third track is streamed. Both moved tracks retain
        // their exact Resident PCM; the shared paged planner still must refuse.
        let mut sources = media
            .iter()
            .map(|(&asset, audio)| {
                (
                    asset,
                    crate::compile::PreparedAudioSource::Resident(audio.clone()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        sources.insert(
            assets[2],
            crate::compile::PreparedAudioSource::Streamed(Arc::new(
                crate::StreamedSourceMetadata::new(
                    crate::StreamSourceKey::new(9, assets[2]),
                    48_000,
                    48_000,
                    2048,
                    2048,
                    hex_project::IDENTITY_CONVERSION,
                    [19; 32],
                )
                .unwrap(),
            )),
        );
        let prepare_mixed = |project: &Project| {
            Arc::new(
                PreparedRevision::prepare_with_prepared_audio_sources(
                    project,
                    &sources,
                    9,
                    Arc::new(crate::SineTable::new()),
                    None,
                )
                .unwrap(),
            )
        };
        let before_mixed = prepare_mixed(&before_project);
        let after_mixed = prepare_mixed(editor.project());
        for (old, next) in [
            (&before_mixed, &after_mixed),
            (&before, &after_mixed),
            (&before_mixed, &after),
        ] {
            let mut old = SessionPlayback::new(Arc::clone(old)).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            let mut next = SessionPlayback::new(Arc::clone(next)).unwrap();
            next.require_continuous_insert = true;
            assert!(next.direct_audio_track_move_from(&old).is_none());
            assert!(!next.plan_native_removal_from(&old).unwrap());
            assert!(!next.native_removal_pending());
            assert!(old.track_faders.iter().all(|fader| !fader.active()));
        }
        let mut old = SessionPlayback::new(before).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        let mut next = SessionPlayback::new(after).unwrap();
        assert!(next.direct_audio_track_move_from(&old).is_some());
        next.routes[1].owner = RouteOwner::Silence;
        assert!(next.direct_audio_track_move_from(&old).is_none());
        next.routes[1].owner = RouteOwner::Arrangement;
        next.require_continuous_insert = true;
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Fading
        );
        let mut faded = [[0.0; 2]; 240];
        old.render(&mut faded).unwrap();
        assert_eq!(faded[239], [0.0; 2]);
        assert!(
            old.track_faders
                .iter()
                .take(2)
                .all(|fader| fader.removal_ready())
        );
        next.cancel_native_removal_from(&mut old);
        old.render(&mut faded).unwrap();
        assert!(old.track_faders.iter().take(2).all(|fader| !fader.active()));
        assert!(faded[239][0] > 0.25);
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Fading
        );
        old.render(&mut faded).unwrap();
        assert_eq!(
            next.advance_native_removal_from(&mut old).unwrap(),
            super::super::NativeRemovalProgress::Ready
        );
        next.try_inherit_state_live_from(&mut old).unwrap();
    }
}
