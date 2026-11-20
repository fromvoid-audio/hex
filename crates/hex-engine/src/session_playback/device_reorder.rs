//! A same-track, zero-latency native permutation shares the staged track fade.
//! The handoff moves actual processor history once at zero output; no old tail
//! route is retained. Compensated/Group/receiver paths need separate proof.
use super::{RouteOwner, SessionPlayback};

impl SessionPlayback {
    pub(super) fn direct_audio_track_reorder_from(&self, previous: &Self) -> Option<usize> {
        // Compensation and Group routing need their own timing proof. This
        // first route-local cut keeps the existing uncompensated render path.
        if self.compensation.is_some() || previous.compensation.is_some() {
            return None;
        }
        let old = previous.revision.arrangement().tracks();
        let new = self.revision.arrangement().tracks();
        if !old
            .iter()
            .map(|track| track.canonical_track_id())
            .eq(new.iter().map(|track| track.canonical_track_id()))
        {
            return None;
        }
        let mut reordered = None;
        for (index, (next, old)) in self
            .device_chains
            .iter()
            .zip(&previous.device_chains)
            .enumerate()
        {
            if next.len() == old.len() && next.configuration_matches_range(old, 0..old.len()) {
                continue;
            }
            if !next.is_exact_native_reorder_from(old) || reordered.replace(index).is_some() {
                return None;
            }
        }
        let index = reordered?;
        for playback in [self, previous] {
            let track = playback.revision.arrangement().tracks().get(index)?;
            let route = playback.routes.get(index)?;
            if track.instrument_id.is_some()
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
        let track = previous.revision.arrangement().tracks().get(index)?;
        match previous.routes.get(index)?.owner {
            RouteOwner::Arrangement if !track.clips.is_empty() => Some(index),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::device_removal::NativeRemovalProgress;
    use super::*;
    use crate::{Audio, PreparedRevision, SessionPlaybackCommand};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{Command, Created, DeviceChainOwner, DeviceSelectionIds, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn exact_reorder_installs_at_zero_and_cancellation_restores_the_running_track() {
        const RATE: u32 = 48_000;
        const FADE: usize = 240;
        const SIGNAL: [f32; 2] = [0.25, -0.125];
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: "memory/audio.wav".into(),
                sample_rate: RATE,
                frames: 2048,
            })
            .unwrap()
        else {
            panic!("audio");
        };
        let mut first = None;
        for _ in 0..2 {
            let Created::Device(device) = editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap()
            else {
                panic!("utility");
            };
            first.get_or_insert(device);
        }
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![SIGNAL; 2048]).unwrap())]);
        let table = Arc::new(crate::SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let before = prepare(editor.project());
        editor
            .execute(Command::ReorderDevices {
                owner: DeviceChainOwner::Track(track),
                selected: DeviceSelectionIds::new(&[first.unwrap()]).unwrap(),
                before: None,
            })
            .unwrap();
        let after = prepare(editor.project());
        for cancel in [false, true] {
            let mut old = SessionPlayback::new(Arc::clone(&before)).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            old.render(&mut [[0.0; 2]; 32]).unwrap();
            let mut next = SessionPlayback::new(Arc::clone(&after)).unwrap();
            next.require_continuous_insert = true;
            assert_eq!(
                next.advance_native_removal_from(&mut old).unwrap(),
                NativeRemovalProgress::Fading
            );
            let mut fading = [[0.0; 2]; FADE];
            old.render(&mut fading).unwrap();
            assert_eq!(fading[FADE - 1], [0.0; 2]);
            if cancel {
                next.cancel_native_removal_from(&mut old);
                old.render(&mut fading).unwrap();
                assert_eq!(fading[FADE - 1], SIGNAL);
                assert!(!old.track_faders[0].active());
            } else {
                assert_eq!(
                    next.advance_native_removal_from(&mut old).unwrap(),
                    NativeRemovalProgress::Ready
                );
                next.try_inherit_state_live_from(&mut old).unwrap();
                next.render(&mut fading).unwrap();
                assert_eq!(fading[FADE - 1], SIGNAL);
                assert!(!next.track_faders[0].active());
                assert_eq!(next.performance_frame(), (32 + FADE * 2) as u64);
            }
        }
    }
}
