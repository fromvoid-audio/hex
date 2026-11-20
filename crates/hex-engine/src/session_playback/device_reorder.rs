//! A same-track, zero-latency native permutation shares the staged track fade.
//! The handoff moves actual processor history once at zero output; no old tail
//! route is retained. Zero-delay Group routes use the existing typed-edge and
//! stamp-copy proof; other compensated/receiver paths need separate proof.
use super::{RouteOwner, SessionPlayback};

impl SessionPlayback {
    pub(super) fn direct_audio_track_reorder_from(&self, previous: &Self) -> Option<usize> {
        let grouped = match (&self.compensation, &previous.compensation) {
            (None, None) => false,
            (Some(next), Some(old))
                if !self.revision.contains_streamed_sources()
                    && !previous.revision.contains_streamed_sources()
                    && next.graph.supports_closed_group_stream()
                    && old.graph.supports_closed_group_stream()
                    && next.graph.supports_closed_compensated_audio_groups()
                    && old.graph.supports_closed_compensated_audio_groups()
                    && self.foreign_runtime_bindings().next().is_none()
                    && previous.foreign_runtime_bindings().next().is_none()
                    && next.can_copy_from(old, None) =>
            {
                // Resident-only: the shared stream planner reaches this branch
                // before proving file-backed pages or workspace readiness.
                // Changed serial edges carry no PCM. Existing typed routing,
                // arrivals and stamp history still pass the ordinary copy proof.
                true
            }
            _ => return None,
        };
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
        let route = previous.routes.get(index)?;
        match route.owner {
            RouteOwner::Arrangement if !track.clips.is_empty() => Some(index),
            RouteOwner::SessionAudio { clip, .. }
                if !grouped && route.pending.is_none() && route.audio_release.is_none() =>
            {
                // Retain the exact resident clip and its existing launch clock.
                // Concurrent source/route changes need their own transition proof.
                previous
                    .revision
                    .session()
                    .audio_clip(clip)
                    .zip(self.revision.session().audio_clip(clip))
                    .filter(|(old, next)| {
                        old.track == route.id
                            && old.resident_audio().is_some()
                            && next.resident_audio().is_some()
                            && next.source_matches(old)
                            && next.gain == old.gain
                    })
                    .map(|_| index)
            }
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
        exact_reorder(false, false);
    }

    #[test]
    fn session_reorder_preserves_its_source_origin_and_refuses_changed_sources() {
        exact_reorder(true, false);
    }

    #[test]
    fn grouped_reorder_keeps_stamps_and_refuses_streamed_sources_or_changed_layouts() {
        exact_reorder(false, true);
    }

    fn exact_reorder(session: bool, grouped: bool) {
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
        let group_destination = grouped.then(|| {
            let Created::Track(inner) = editor
                .execute(Command::GroupTracks {
                    tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                    name: "inner".into(),
                })
                .unwrap()
            else {
                panic!("group");
            };
            let Created::Track(outer) = editor
                .execute(Command::GroupTracks {
                    tracks: hex_project::TrackSelectionIds::new(&[inner]).unwrap(),
                    name: "outer".into(),
                })
                .unwrap()
            else {
                panic!("group");
            };
            outer
        });
        let session_clip = session.then(|| {
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
            let Created::Scene(second_scene) = editor
                .execute(Command::AddScene {
                    name: "second".into(),
                })
                .unwrap()
            else {
                panic!("scene");
            };
            editor
                .execute(Command::DuplicateSessionAudioCell {
                    track,
                    from_scene: scene,
                    to_scene: second_scene,
                })
                .unwrap();
            clip
        });
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![SIGNAL; 2048]).unwrap())]);
        let table = Arc::new(crate::SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let before_project = grouped.then(|| editor.project().clone());
        let before = prepare(editor.project());
        editor
            .execute(Command::ReorderDevices {
                owner: DeviceChainOwner::Track(track),
                selected: DeviceSelectionIds::new(&[first.unwrap()]).unwrap(),
                before: None,
            })
            .unwrap();
        let after = prepare(editor.project());
        if let Some(before_project) = &before_project {
            let sources = BTreeMap::from([(
                asset,
                crate::compile::PreparedAudioSource::Streamed(Arc::new(
                    crate::StreamedSourceMetadata::new(
                        crate::StreamSourceKey::new(9, asset),
                        RATE,
                        RATE,
                        2048,
                        2048,
                        hex_project::IDENTITY_CONVERSION,
                        [19; 32],
                    )
                    .unwrap(),
                )),
            )]);
            let prepare_streamed = |project: &Project| {
                Arc::new(
                    PreparedRevision::prepare_with_prepared_audio_sources(
                        project,
                        &sources,
                        9,
                        Arc::clone(&table),
                        None,
                    )
                    .unwrap(),
                )
            };
            let before_streamed = prepare_streamed(before_project);
            let after_streamed = prepare_streamed(editor.project());
            for (old, next) in [
                (&before_streamed, &after_streamed),
                (&before, &after_streamed),
                (&before_streamed, &after),
            ] {
                let mut old = SessionPlayback::new(Arc::clone(old)).unwrap();
                old.apply(SessionPlaybackCommand::Play).unwrap();
                let mut next = SessionPlayback::new(Arc::clone(next)).unwrap();
                next.require_continuous_insert = true;
                // This shared planner is called before the streamed page/workspace
                // proof; the resident-only Group class must never begin a fade.
                assert!(!next.plan_native_removal_from(&old).unwrap());
                assert!(!next.native_removal_pending());
                assert!(old.track_faders.iter().all(|fader| !fader.active()));
            }
        }
        let track_index = before
            .arrangement()
            .tracks()
            .iter()
            .position(|candidate| candidate.canonical_track_id() == Some(track))
            .unwrap();
        let mut group_refusals = Vec::new();
        if grouped {
            let mut rerouted = Editor::new(editor.project().clone(), 8).unwrap();
            rerouted
                .execute(Command::ReparentTracks {
                    tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                    destination: hex_project::TrackDestination {
                        parent_group: group_destination,
                        before: None,
                    },
                })
                .unwrap();
            let rerouted = prepare(rerouted.project());
            assert!(
                rerouted
                    .arrangement()
                    .signal_graph
                    .as_ref()
                    .unwrap()
                    .supports_closed_group_stream()
            );
            group_refusals.push(rerouted);
            let mut delayed = Editor::new(editor.project().clone(), 8).unwrap();
            delayed
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
                })
                .unwrap();
            let delayed = prepare(delayed.project());
            assert!(delayed.arrangement().main_latency_frames() > 0);
            group_refusals.push(delayed);
        }
        let changed_source = session_clip.map(|clip| {
            editor
                .execute(Command::SetSessionAudioClipSource {
                    clip,
                    asset,
                    source_start: 1,
                    source_end: 2048,
                })
                .unwrap();
            prepare(editor.project())
        });
        let missing_source = session_clip.map(|clip| {
            let scene = after.session().audio_clip(clip).unwrap().scene;
            editor
                .execute(Command::ClearSessionCell { track, scene })
                .unwrap();
            prepare(editor.project())
        });
        if let Some(clip) = session_clip {
            for pending in [false, true] {
                let mut old = SessionPlayback::new(Arc::clone(&before)).unwrap();
                old.apply(SessionPlaybackCommand::Play).unwrap();
                old.apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: crate::LaunchQuantization::Immediate,
                })
                .unwrap();
                old.render(&mut [[0.0; 2]; 32]).unwrap();
                if pending {
                    old.apply(SessionPlaybackCommand::StopTrack {
                        track,
                        quantization: crate::LaunchQuantization::OneBar,
                    })
                    .unwrap();
                    assert!(old.routes[track_index].pending.is_some());
                } else {
                    let second = before
                        .session()
                        .audio_clips()
                        .iter()
                        .find(|other| other.id != clip)
                        .unwrap()
                        .id;
                    old.apply(SessionPlaybackCommand::LaunchClip {
                        clip: second,
                        quantization: crate::LaunchQuantization::Immediate,
                    })
                    .unwrap();
                    assert!(old.routes[track_index].audio_release.is_some());
                }
                let mut next = SessionPlayback::new(Arc::clone(&after)).unwrap();
                next.require_continuous_insert = true;
                assert_eq!(
                    next.advance_native_removal_from(&mut old).unwrap(),
                    NativeRemovalProgress::NotApplicable
                );
                assert!(!old.track_faders[track_index].active());
            }
        }
        for cancel in [false, true] {
            let mut old = SessionPlayback::new(Arc::clone(&before)).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            let launch = if let Some(clip) = session_clip {
                old.render(&mut [[0.0; 2]; 17]).unwrap();
                old.apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: crate::LaunchQuantization::Immediate,
                })
                .unwrap();
                17
            } else {
                0
            };
            old.render(&mut [[0.0; 2]; 32]).unwrap();
            let owner = old.routes[track_index].owner;
            for changed in [changed_source.as_ref(), missing_source.as_ref()]
                .into_iter()
                .flatten()
            {
                let mut changed = SessionPlayback::new(Arc::clone(changed)).unwrap();
                changed.require_continuous_insert = true;
                assert_eq!(
                    changed.advance_native_removal_from(&mut old).unwrap(),
                    NativeRemovalProgress::NotApplicable
                );
                assert!(!old.track_faders[track_index].active());
            }
            for refused in &group_refusals {
                let mut refused = SessionPlayback::new(Arc::clone(refused)).unwrap();
                refused.require_continuous_insert = true;
                assert_eq!(refused.direct_audio_track_reorder_from(&old), None);
                assert_eq!(
                    refused.advance_native_removal_from(&mut old).unwrap(),
                    NativeRemovalProgress::NotApplicable
                );
                assert!(!old.track_faders[track_index].active());
            }
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
                assert!(!old.track_faders[track_index].active());
            } else {
                assert_eq!(
                    next.advance_native_removal_from(&mut old).unwrap(),
                    NativeRemovalProgress::Ready
                );
                next.try_inherit_state_live_from(&mut old).unwrap();
                if grouped {
                    assert_eq!(
                        next.compensation.as_ref().unwrap().delays.latest_stamp(),
                        old.compensation.as_ref().unwrap().delays.latest_stamp()
                    );
                }
                next.render(&mut fading).unwrap();
                assert_eq!(fading[FADE - 1], SIGNAL);
                assert!(!next.track_faders[track_index].active());
                assert_eq!(next.performance_frame(), (launch + 32 + FADE * 2) as u64);
                assert_eq!(next.routes[track_index].owner, owner);
            }
        }
    }
}
