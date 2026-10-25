//! Source-owned kernel continuity, separate from track-local effect history.
use super::*;

pub(super) struct SharedSourceHandoff {
    pub(super) new_for_old: [usize; MAX_TRACKS],
    pub(super) retarget_for_old: [usize; MAX_TRACKS],
}
impl SharedSourceHandoff {
    pub(super) fn handles_old(&self, index: usize) -> bool {
        self.new_for_old
            .get(index)
            .is_some_and(|index| *index != usize::MAX)
            || self
                .retarget_for_old
                .get(index)
                .is_some_and(|index| *index != usize::MAX)
    }
    pub(super) fn handles_new(&self, index: usize) -> bool {
        self.new_for_old.contains(&index) || self.retarget_for_old.contains(&index)
    }
    pub(super) fn prepare(
        candidate: &SessionPlayback,
        previous: &SessionPlayback,
        timing_changed: bool,
    ) -> Result<Self, SessionPlaybackError> {
        let mut plan = Self {
            new_for_old: [usize::MAX; MAX_TRACKS],
            retarget_for_old: [usize::MAX; MAX_TRACKS],
        };
        let old_routing = previous.revision.source_routing();
        let new_routing = candidate.revision.source_routing();
        for (old_index, old_runtime) in previous.mutable_instruments.iter().enumerate() {
            let Some(old_runtime) = old_runtime else {
                continue;
            };
            let Some(source) = old_routing.source(old_index) else {
                continue;
            };
            let Some(new_index) = candidate
                .revision
                .arrangement()
                .tracks()
                .iter()
                .enumerate()
                .find(|(index, track)| {
                    new_routing.source(*index) == Some(source) && track.receives_source_audio()
                })
                .map(|(index, _)| index)
            else {
                continue;
            };
            if !old_runtime.is_source_owner()
                && !old_routing.is_shared(old_index)
                && !new_routing.is_shared(new_index)
            {
                continue;
            }
            let Some(new_runtime) = candidate
                .mutable_instruments
                .get(new_index)
                .and_then(Option::as_ref)
            else {
                if !old_runtime.committed_is_quiescent() {
                    return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                        "shared source processing class changed while notes are active",
                    ));
                }
                continue;
            };
            let exact = !timing_changed
                && previous
                    .revision
                    .retained_source_configuration_matches(&candidate.revision, source);
            let new_receiver = candidate.routes[new_index].id;
            let retarget = !timing_changed
                && previous
                    .revision
                    .retained_source_identity_matches(&candidate.revision, source)
                && candidate
                    .revision
                    .session()
                    .mutable_history_retarget(new_receiver)
                    .is_some_and(|proof| new_runtime.can_retarget_from(old_runtime, proof));
            if !exact && !retarget {
                if !old_runtime.committed_is_quiescent() {
                    return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                        "shared source configuration changed while notes are active",
                    ));
                }
                continue;
            }
            if exact && !new_runtime.can_inherit_from(old_runtime) {
                return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                    "shared source runtime or original resources are incompatible",
                ));
            }
            // Existing contributors keep their exact authored source law. A
            // removed/unbound contributor instead releases only its owned IDs
            // after every handoff preflight has succeeded.
            for &contributor in old_routing.contributors(old_index) {
                let old_contributor = usize::from(contributor);
                let track = previous.routes[old_contributor].id;
                let Some(_new_contributor) = candidate
                    .routes
                    .iter()
                    .position(|route| route.id == track)
                    .filter(|index| new_routing.source(*index) == Some(source))
                else {
                    continue;
                };
                let old_route =
                    effective_midi_route(&previous.revision, &previous.routes, old_contributor)?;
                let unchanged = match old_route.owner {
                    RouteOwner::Arrangement => previous
                        .revision
                        .mutable_arrangement_sources_match(&candidate.revision, track),
                    RouteOwner::Session { clip, .. } => {
                        crate::session_inventory::EffectiveSessionMidi::new(
                            previous.revision.session(),
                            previous.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, track)
                        .zip(
                            crate::session_inventory::EffectiveSessionMidi::new(
                                candidate.revision.session(),
                                candidate.midi_admission.as_ref(),
                            )
                            .midi_clip_for_track(clip, track),
                        )
                        .is_some_and(|(old, new)| old.source_content_matches(&new))
                    }
                    RouteOwner::SessionAudio { .. } | RouteOwner::Silence => true,
                };
                if !unchanged
                    && old_runtime
                        .proofs()
                        .any(|proof| proof.signature.key().track == track && !proof.note_off_sent)
                {
                    return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                        "shared contributor source changed while its notes are active",
                    ));
                }
            }
            // Incoming pre-cut occurrences are not chased: the source collector
            // admits only future onsets. Existing source kernels and unrelated
            // held contributors keep their dispatched proofs.
            if exact {
                plan.new_for_old[old_index] = new_index;
            } else {
                plan.retarget_for_old[old_index] = new_index;
            }
        }
        Ok(plan)
    }
}

/// Provider history adoption owns one source; contributor content and identity
/// stay independent of the receiver's local effect topology.
pub(super) fn analytic_import_sources_match(
    candidate: &SessionPlayback,
    previous: &SessionPlayback,
    old_receiver: usize,
    new_receiver: usize,
) -> Result<bool, SessionPlaybackError> {
    let old = previous.revision.source_routing();
    let new = candidate.revision.source_routing();
    let Some(source) = old.source(old_receiver) else {
        return Ok(false);
    };
    if new.source(new_receiver) != Some(source)
        || !previous
            .revision
            .retained_source_identity_matches(&candidate.revision, source)
        || old.contributors(old_receiver).len() != new.contributors(new_receiver).len()
    {
        return Ok(false);
    }
    for &index in old.contributors(old_receiver) {
        let index = usize::from(index);
        let track = previous.routes[index].id;
        let Some(next) = candidate
            .routes
            .iter()
            .position(|route| route.id == track)
            .filter(|index| new.source(*index) == Some(source))
        else {
            return Ok(false);
        };
        if previous.revision.arrangement().tracks()[index].source_input_enabled()
            != candidate.revision.arrangement().tracks()[next].source_input_enabled()
        {
            return Ok(false);
        }
        let route = effective_midi_route(&previous.revision, &previous.routes, index)?;
        if route.pending.is_some() {
            return Ok(false);
        }
        let unchanged = match route.owner {
            RouteOwner::Arrangement => previous
                .revision
                .mutable_arrangement_sources_match(&candidate.revision, track),
            RouteOwner::Session { clip, .. } => {
                crate::session_inventory::EffectiveSessionMidi::new(
                    previous.revision.session(),
                    previous.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, track)
                .zip(
                    crate::session_inventory::EffectiveSessionMidi::new(
                        candidate.revision.session(),
                        candidate.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, track),
                )
                .is_some_and(|(old, new)| old.source_content_matches(&new))
            }
            RouteOwner::Silence | RouteOwner::SessionAudio { .. } => true,
        };
        if !unchanged {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn release_disconnected_contributors(
    runtime: &mut MutableInstrumentTrackRuntime,
    previous: &PreparedRevision,
    candidate: &PreparedRevision,
    frame: u64,
) {
    let disconnected = |track: TrackId| {
        let old = previous
            .arrangement()
            .tracks()
            .iter()
            .position(|row| row.canonical_track_id() == Some(track));
        let new = candidate
            .arrangement()
            .tracks()
            .iter()
            .position(|row| row.canonical_track_id() == Some(track));
        old.and_then(|index| previous.source_routing().source(index))
            .is_some_and(|source| {
                new.is_none_or(|index| {
                    candidate.source_routing().source(index) != Some(source)
                        || candidate.source_routing().receiver_index(index).is_none()
                })
            })
    };
    runtime.reconcile_gate_deadlines_at(frame, |signature| {
        disconnected(signature.key().track).then_some(frame)
    });
    for track in previous
        .arrangement()
        .tracks()
        .iter()
        .filter_map(crate::Track::canonical_track_id)
    {
        if disconnected(track) {
            runtime.release_live_contributor(track);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_device_resources::{
        PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
    };
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    use hex_project::{Command, Created, Editor};
    use std::{
        collections::BTreeMap,
        sync::{Arc, atomic::AtomicBool},
    };

    struct Fixture {
        editor: Editor,
        store: ResidentDeviceAssets,
        receiver: TrackId,
        contributor: TrackId,
        source: hex_project::SourceId,
    }
    const PCM: [Frame; 4] = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.25, 0.125]];
    fn fixture() -> Fixture {
        let mut editor = Editor::new(hex_project::Project::new(8000).unwrap(), 64).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::ImportDeviceAsset {
                path: "held.wav".into(),
                sample_rate: 8000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let Created::Track(receiver) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
        state.mode = hex_devices::sampler::state::AuthoredMode::Classic {
            loop_enabled: true,
            loop_region: state.region,
        };
        state.envelope = hex_devices::sampler::Envelope {
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            release_seconds: 0.0,
        };
        let state = state
            .to_native()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap();
        editor
            .execute(Command::InsertInstrument {
                track: receiver,
                state,
            })
            .unwrap();
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor")
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        for track in [receiver, contributor] {
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
                    velocity: 127,
                })
                .unwrap();
        }
        editor
            .execute(Command::Gain {
                track: receiver,
                gain: 0.25,
            })
            .unwrap();
        editor
            .execute(Command::Gain {
                track: contributor,
                gain: 0.75,
            })
            .unwrap();
        let mut store = ResidentDeviceAssets::default();
        store
            .insert_on_control(
                (
                    asset,
                    crate::OwnedAudioSource::try_resident(8000, PCM.to_vec())
                        .ok()
                        .unwrap(),
                ),
                0,
                crate::AudioStorageLedgerReservation {
                    data_rows: 256,
                    handle_rows: 64,
                    root_rows: 32,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 20_000_000,
                },
                4,
                &AtomicBool::new(false),
                || Ok::<_, ResourcePreparationError>(()),
            )
            .unwrap();
        Fixture {
            editor,
            store,
            receiver,
            contributor,
            source,
        }
    }
    #[test]
    fn shared_analytic_release_keeps_receiver_mix_and_one_nonlinear_suffix() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            let note = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .midi_clips()[0]
                .notes()[0]
                .id();
            f.editor
                .execute(Command::UpdateMidiNote {
                    note,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 67,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            let registry = hex_devices::native_registry().unwrap();
            let definition = registry
                .definition(hex_devices::saturator::SATURATOR_TYPE_ID)
                .unwrap();
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::saturator::parameter::DRIVE,
                    hex_lattice_contract::ParameterValue::Float(30.0),
                )
                .unwrap();
            f.editor
                .execute(Command::InsertDevice {
                    track: f.receiver,
                    before: None,
                    state,
                })
                .unwrap();
            let mut baseline_editor = Editor::new(f.editor.project().clone(), 16).unwrap();
            baseline_editor
                .execute(Command::RemoveTrack(f.contributor))
                .unwrap();
            let clip = baseline_editor
                .project()
                .track(f.receiver)
                .unwrap()
                .midi_clips()[0]
                .id();
            baseline_editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 67,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            fn prepare(project: &hex_project::Project) -> Arc<PreparedRevision> {
                Arc::new(
                    PreparedRevision::prepare(
                        project,
                        &BTreeMap::new(),
                        Arc::new(crate::SineTable::new()),
                        None,
                    )
                    .unwrap(),
                )
            }
            let mut actual = SessionPlayback::new(prepare(f.editor.project())).unwrap();
            let mut baseline = SessionPlayback::new(prepare(baseline_editor.project())).unwrap();
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            baseline.apply(SessionPlaybackCommand::Play).unwrap();
            let mut left = [[0.0; 2]; 129];
            let mut right = left;
            actual.render(&mut left).unwrap();
            baseline.render(&mut right).unwrap();
            assert_eq!(left, right);
            baseline
                .apply(SessionPlaybackCommand::StopTrack {
                    track: f.receiver,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
            f.editor
                .execute(Command::BindTrackSource {
                    track: f.receiver,
                    source: None,
                })
                .unwrap();
            let mut candidate = SessionPlayback::new(prepare(f.editor.project())).unwrap();
            candidate.try_inherit_state_from(&mut actual).unwrap();
            assert_eq!(candidate.releases.len(), 2);
            let group = candidate.releases[0].location.tail_group();
            assert!(group.is_some());
            assert!(
                candidate
                    .releases
                    .iter()
                    .all(|release| release.location.tail_group() == group)
            );
            let mut actual_pcm = [[0.0; 2]; 512];
            let mut expected_pcm = actual_pcm;
            for chunk in actual_pcm.chunks_mut(17) {
                candidate.render(chunk).unwrap();
            }
            for chunk in expected_pcm.chunks_mut(31) {
                baseline.render(chunk).unwrap();
            }
            assert!(expected_pcm.iter().any(|frame| frame[0].abs() > 0.001));
            for (actual, expected) in actual_pcm.iter().zip(expected_pcm) {
                for channel in 0..2 {
                    assert!(
                        (actual[channel] - expected[channel]).abs() < 0.000_002,
                        "actual={actual:?} expected={expected:?}"
                    );
                }
            }
        });
    }

    #[test]
    fn analytic_pause_advances_release_age_without_clock_or_resume_retrigger() {
        on_control(|| {
            for console in [false, true] {
                let mut f = fixture();
                let device = f
                    .editor
                    .project()
                    .track(f.receiver)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id();
                f.editor
                    .execute(Command::ReplaceDeviceState {
                        device,
                        state: hex_project::default_instrument_state().unwrap(),
                    })
                    .unwrap();
                let scene = if console {
                    let Created::Scene(scene) = f
                        .editor
                        .execute(Command::AddScene {
                            name: "scene".into(),
                        })
                        .unwrap()
                    else {
                        panic!("scene")
                    };
                    for track in [f.receiver, f.contributor] {
                        let Created::SessionClip(clip) = f
                            .editor
                            .execute(Command::CreateSessionMidiClip {
                                track,
                                scene,
                                name: "notes".into(),
                                source_length: BeatTime::ONE,
                            })
                            .unwrap()
                        else {
                            panic!("console clip")
                        };
                        f.editor
                            .execute(Command::AddMidiNote {
                                clip,
                                start: BeatTime::ZERO,
                                duration: BeatTime::ONE,
                                pitch: 60,
                                channel: 0,
                                velocity: 127,
                            })
                            .unwrap();
                    }
                    Some(scene)
                } else {
                    None
                };
                let mut actual = playback(&f);
                let mut baseline = playback(&f);
                for playback in [&mut actual, &mut baseline] {
                    if let Some(scene) = scene {
                        playback
                            .apply(SessionPlaybackCommand::LaunchScene {
                                scene,
                                quantization: LaunchQuantization::Immediate,
                            })
                            .unwrap();
                    }
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    playback.render(&mut [[0.0; 2]; 129]).unwrap();
                }
                actual.apply(SessionPlaybackCommand::Pause).unwrap();
                baseline
                    .apply(SessionPlaybackCommand::StopAllClips {
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
                let mut monitored = vec![[0.0; 2]; 4096];
                let mut expected = monitored.clone();
                for chunk in monitored.chunks_mut(17) {
                    actual.render(chunk).unwrap();
                }
                for chunk in expected.chunks_mut(31) {
                    baseline.render(chunk).unwrap();
                }
                assert_eq!(actual.performance_frame, 129);
                assert_eq!(actual.arrangement_frame, 129);
                assert!(expected.iter().any(|frame| frame[0].abs() > 0.001));
                for (actual, expected) in monitored.iter().zip(expected) {
                    for channel in 0..2 {
                        assert!(
                            (actual[channel] - expected[channel]).abs() < 0.000_002,
                            "actual={actual:?} expected={expected:?}"
                        );
                    }
                }
                assert!(actual.releases.is_empty());
                actual.apply(SessionPlaybackCommand::Play).unwrap();
                let mut resumed = [[1.0; 2]; 64];
                actual.render(&mut resumed).unwrap();
                assert_eq!(resumed, [[0.0; 2]; 64]);
                actual.apply(SessionPlaybackCommand::Pause).unwrap();
                assert!(
                    actual.releases.is_empty(),
                    "already suppressed clip notes are not recaptured"
                );
            }
        });
    }
    #[test]
    fn analytic_pause_suppresses_only_the_current_arrangement_loop_pass() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 64,
                    enabled: true,
                })
                .unwrap();
            let mut actual = playback(&f);
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            actual.render(&mut [[0.0; 2]; 15]).unwrap();
            actual.apply(SessionPlaybackCommand::Pause).unwrap();
            actual.render(&mut [[0.0; 2]; 4096]).unwrap();
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            let mut resumed = [[0.0; 2]; 64];
            actual.render(&mut resumed).unwrap();
            assert!(resumed[..49].iter().all(|frame| *frame == [0.0; 2]));
            assert!(resumed[49..].iter().any(|frame| frame[0].abs() > 0.000_001));
            assert_eq!(actual.arrangement_loop_pass, 1);
        });
    }

    #[test]
    fn shared_handoff_relink_does_not_chase_or_reset_either_source() {
        on_control(|| {
            let mut f = fixture();
            let state = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .clone();
            let Created::Track(other) = f
                .editor
                .execute(Command::AddMidiTrack {
                    name: "other receiver".into(),
                })
                .unwrap()
            else {
                panic!("other")
            };
            f.editor
                .execute(Command::InsertInstrument {
                    track: other,
                    state,
                })
                .unwrap();
            f.editor
                .execute(Command::Gain {
                    track: other,
                    gain: 0.5,
                })
                .unwrap();
            let target = f
                .editor
                .project()
                .track(other)
                .unwrap()
                .source_id()
                .unwrap();
            let Created::Clip(clip) = f
                .editor
                .execute(Command::PlaceMidiClip {
                    track: other,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            let mut old = playback(&f);
            old.apply(SessionPlaybackCommand::Play).unwrap();
            check_pcm(&mut old, 0, 7, 1.0);
            f.editor
                .execute(Command::BindTrackSource {
                    track: f.contributor,
                    source: Some(target),
                })
                .unwrap();
            let mut next = playback(&f);
            next.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(next.mutable_instruments.iter().flatten().count(), 2);
            check_pcm(&mut next, 7, 19, 0.75);
        });
    }

    #[test]
    fn shared_operator_class_adoption_imports_every_contributor_before_receiver_effects() {
        on_control(|| {
            use hex_devices::operator_state::{harmonic_parameter, parameter};
            use hex_lattice_contract::ParameterValue;
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            let state = crate::operator_legacy_test::legacy_operator_state();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: state.clone(),
                })
                .unwrap();
            let note = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .midi_clips()[0]
                .notes()[0]
                .id();
            f.editor
                .execute(Command::UpdateMidiNote {
                    note,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 67,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            let mut old = playback(&f);
            let mut oracle = playback(&f);
            for runtime in [&mut old, &mut oracle] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime.render(&mut [[0.0; 2]; 129]).unwrap();
            }
            assert!(old.mutable_instruments.iter().all(Option::is_none));
            let state = crate::patch_native_device_parameter_for_class(
                crate::NativeDeviceClass::Instrument,
                &state,
                harmonic_parameter(1, 2),
                ParameterValue::Float(1.0),
            )
            .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            // A receiver move must only change the final track fader. Local
            // topology is not the identity of the sounding source kernel.
            f.editor
                .execute(Command::SetNativeSourceReceiver {
                    source: f.source,
                    track: Some(f.contributor),
                })
                .unwrap();
            let mut next = playback(&f);
            next.try_inherit_state_from(&mut old).unwrap();
            let runtime = next.mutable_instruments[1].as_ref().unwrap();
            assert_eq!(runtime.proofs().count(), 2);
            assert!(
                runtime
                    .proofs()
                    .any(|proof| proof.signature.key().track == f.receiver)
            );
            assert!(
                runtime
                    .proofs()
                    .any(|proof| proof.signature.key().track == f.contributor)
            );
            let mut actual = [[0.0; 2]; 1];
            let mut expected = actual;
            next.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            for channel in 0..2 {
                assert!(
                    (actual[0][channel] - expected[0][channel] * 3.0).abs() < 0.000_02,
                    "source boundary {:?} != {:?}",
                    actual,
                    expected
                );
            }
            // Exercise the existing mutable provider retarget seam on the
            // adopted source; it must retain both contributor keys.
            let state = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .instrument()
                .unwrap()
                .state();
            let state = crate::patch_native_device_parameter_for_class(
                crate::NativeDeviceClass::Instrument,
                state,
                parameter::oscillator_level(1),
                ParameterValue::Float(0.6),
            )
            .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            let mut edited = playback(&f);
            edited.try_inherit_state_from(&mut next).unwrap();
            assert_eq!(
                edited.mutable_instruments[1]
                    .as_ref()
                    .unwrap()
                    .proofs()
                    .count(),
                2
            );
            let mut output = [[0.0; 2]; 37];
            edited.render(&mut output).unwrap();
            assert!(
                output
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.000_001)
            );
        });
    }

    fn live_playback(
        f: &Fixture,
        input: &Arc<crate::computer_keyboard::ComputerKeyboardInput>,
    ) -> (crate::playback::Playback, Arc<PreparedRevision>) {
        let cancel = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &f.store,
            f.editor.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare_with_device_resources(
                f.editor.project(),
                &BTreeMap::new(),
                Arc::new(crate::SineTable::new()),
                None,
                &crate::audio_transform::AudioTransformPreparation::default(),
                &resources,
                resources
                    .compiler_scan_work_on_control(f.editor.project())
                    .unwrap(),
                &cancel,
            )
            .unwrap(),
        );
        let mut result = crate::playback::Playback::try_from_prepared_with_device_resources(
            Arc::clone(&revision),
            &resources,
        )
        .unwrap();
        result
            .prepare_computer_keyboard(f.editor.project(), f.contributor, Arc::clone(input))
            .unwrap();
        (result, revision)
    }

    #[test]
    fn shared_operator_class_adoption_preserves_live_generation_and_real_key_off() {
        on_control(|| {
            use crate::playback::PlaybackCommand;
            use hex_devices::operator_state::{harmonic_parameter, parameter};
            use hex_lattice_contract::ParameterValue;
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            let mut state = crate::operator_legacy_test::legacy_operator_state();
            for operator in 1..=4 {
                state = crate::patch_native_device_parameter_for_class(
                    crate::NativeDeviceClass::Instrument,
                    &state,
                    parameter::envelope(operator, parameter::EnvelopeField::Release),
                    ParameterValue::Float(0.001),
                )
                .unwrap();
            }
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: state.clone(),
                })
                .unwrap();
            let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            let oracle_input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            for input in [&input, &oracle_input] {
                input.set_key(0, Some((60, 127, 1))).unwrap();
            }
            let (mut old, old_revision) = live_playback(&f, &input);
            let (mut oracle, _) = live_playback(&f, &oracle_input);
            assert!(
                old_revision
                    .session()
                    .mutable_instrument_factory(f.receiver)
                    .is_none()
            );
            for runtime in [&mut old, &mut oracle] {
                runtime.apply(PlaybackCommand::Play);
                runtime.render(&mut [[0.0; 2]; 129]).unwrap();
            }
            state = crate::patch_native_device_parameter_for_class(
                crate::NativeDeviceClass::Instrument,
                &state,
                harmonic_parameter(1, 2),
                ParameterValue::Float(1.0),
            )
            .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            let (mut next, next_revision) = live_playback(&f, &input);
            next.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .unwrap();
            let mut actual = [[0.0; 2]; 1];
            let mut expected = actual;
            next.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            for channel in 0..2 {
                assert!(
                    (actual[0][channel] - expected[0][channel]).abs() < 0.000_02,
                    "adopted live source boundary {:?} != {:?}",
                    actual,
                    expected
                );
            }
            input.set_key(0, None).unwrap();
            // Clip Pause is independent of the real physical Off. If the live
            // generation/native ID mapping was lost, this held voice survives.
            next.apply(PlaybackCommand::Pause);
            let tail = next_revision
                .session()
                .mutable_instrument_factory(f.receiver)
                .unwrap()
                .max_release_tail_frames();
            assert!(tail < 16000, "finite short-release fixture");
            let mut released = vec![[0.0; 2]; tail as usize + 512];
            for chunk in released.chunks_mut(17) {
                next.render(chunk).unwrap();
            }
            assert!(
                released[..16]
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.000_001)
            );
            assert!(
                released[released.len() - 128..]
                    .iter()
                    .flatten()
                    .all(|sample| sample.abs() < 0.000_001),
                "imported live note must release through its original input token"
            );
        });
    }

    fn playback(f: &Fixture) -> SessionPlayback {
        let cancel = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &f.store,
            f.editor.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare_with_device_resources(
                f.editor.project(),
                &BTreeMap::new(),
                Arc::new(crate::SineTable::new()),
                None,
                &crate::audio_transform::AudioTransformPreparation::default(),
                &resources,
                resources
                    .compiler_scan_work_on_control(f.editor.project())
                    .unwrap(),
                &cancel,
            )
            .unwrap(),
        );
        SessionPlayback::new_with_device_resources(revision, &resources).unwrap()
    }
    fn check_pcm(playback: &mut SessionPlayback, start: usize, count: usize, gain: f32) {
        let mut output = vec![[0.0; 2]; count];
        for chunk in output.chunks_mut(3) {
            playback.render(chunk).unwrap();
        }
        for (index, frame) in output.iter().enumerate() {
            for channel in 0..2 {
                assert!(
                    (frame[channel] - PCM[(start + index) % 4][channel] * gain).abs() < 0.000_002,
                    "start={start} index={index} frame={frame:?} gain={gain}"
                );
            }
        }
    }
    fn started(f: &Fixture) -> SessionPlayback {
        let mut old = playback(f);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        check_pcm(&mut old, 0, 7, 0.5);
        old
    }
    fn on_control(run: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap();
    }
    #[test]
    fn shared_handoff_receiver_change_preserves_held_phase_and_single_kernel() {
        on_control(|| {
            let mut f = fixture();
            let mut old = started(&f);
            f.editor
                .execute(Command::SetNativeSourceReceiver {
                    source: f.source,
                    track: Some(f.contributor),
                })
                .unwrap();
            let mut candidate = playback(&f);
            candidate.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(candidate.mutable_instruments.iter().flatten().count(), 1);
            check_pcm(&mut candidate, 7, 19, 1.5);
        });
    }
    #[test]
    fn shared_analytic_receiver_change_preserves_every_held_phase() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            let mut previous = playback(&f);
            let mut oracle = playback(&f);
            for runtime in [&mut previous, &mut oracle] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime.render(&mut [[0.0; 2]; 129]).unwrap();
            }
            f.editor
                .execute(Command::SetNativeSourceReceiver {
                    source: f.source,
                    track: Some(f.contributor),
                })
                .unwrap();
            let mut next = playback(&f);
            next.try_inherit_state_from(&mut previous).unwrap();
            let mut actual = [[0.0; 2]; 117];
            let mut expected = actual;
            for chunk in actual.chunks_mut(17) {
                next.render(chunk).unwrap();
            }
            for chunk in expected.chunks_mut(31) {
                oracle.render(chunk).unwrap();
            }
            assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.001));
            for (actual, expected) in actual.iter().zip(expected) {
                for channel in 0..2 {
                    assert!((actual[channel] - expected[channel] * 3.0).abs() < 0.000_002);
                }
            }
        });
    }

    #[test]
    fn shared_handoff_unlink_and_remove_release_only_the_disconnected_contributor() {
        on_control(|| {
            for remove in [false, true] {
                let mut f = fixture();
                let mut old = started(&f);
                f.editor
                    .execute(if remove {
                        Command::RemoveTrack(f.contributor)
                    } else {
                        Command::BindTrackSource {
                            track: f.contributor,
                            source: None,
                        }
                    })
                    .unwrap();
                let mut candidate = playback(&f);
                candidate.try_inherit_state_from(&mut old).unwrap();
                check_pcm(&mut candidate, 7, 19, 0.25);
            }
        });
    }
    #[test]
    fn shared_handoff_local_effect_edit_undo_redo_keep_source_history() {
        on_control(|| {
            let mut f = fixture();
            let mut old = started(&f);
            let registry = hex_devices::native_registry().unwrap();
            let state = registry
                .default_state(hex_devices::utility::UTILITY_TYPE_ID)
                .unwrap()
                .with_output_gain_millidb(-6000)
                .unwrap();
            let effect_gain = state.output_gain_linear();
            f.editor
                .execute(Command::InsertDevice {
                    track: f.receiver,
                    before: None,
                    state,
                })
                .unwrap();
            let mut changed = playback(&f);
            changed.try_inherit_state_from(&mut old).unwrap();
            check_pcm(&mut changed, 7, 5, 0.5 * effect_gain);
            assert!(f.editor.undo());
            let mut undone = playback(&f);
            undone.try_inherit_state_from(&mut changed).unwrap();
            check_pcm(&mut undone, 12, 5, 0.5);
            assert!(f.editor.redo());
            let mut redone = playback(&f);
            redone.try_inherit_state_from(&mut undone).unwrap();
            check_pcm(&mut redone, 17, 5, 0.5 * effect_gain);
        });
    }
    #[test]
    fn shared_handoff_unsupported_active_source_edit_refuses_without_touching_donor() {
        on_control(|| {
            let mut f = fixture();
            let mut old = started(&f);
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap();
            let mut state =
                hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
            state.gain = 0.5;
            let next = state
                .to_native()
                .unwrap()
                .with_asset_references(device.state().asset_references().to_vec())
                .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device: device.id(),
                    state: next,
                })
                .unwrap();
            let mut candidate = playback(&f);
            assert!(candidate.try_inherit_state_from(&mut old).is_err());
            check_pcm(&mut old, 7, 19, 0.5);
        });
    }
}

pub(super) fn track_has_analytic_source(revision: &PreparedRevision, index: usize) -> bool {
    let track = &revision.arrangement().tracks()[index];
    track.has_instrument_source()
        && track.source_input_enabled()
        && if track.source_id().is_some() {
            revision.source_routing().receiver_index(index).is_some()
        } else {
            track.instrument_enabled()
        }
}

pub(super) struct ReleaseAudioPlan {
    pub(super) current_receiver: Option<usize>,
    pub(super) old_receiver: Option<usize>,
}
/// The entry's recorded source origin is authoritative. A current configuration
/// follows the new receiver as one summed generator; a detached configuration
/// keeps the old receiver suffix once for all of its contributor voices.
pub(super) fn plan_release_audio_owner(
    release: &mut ReleaseEntry,
    previous: &SessionPlayback,
    candidate: &PreparedRevision,
    analytic_owner_matches: &[bool; MAX_TRACKS],
) -> Result<ReleaseAudioPlan, SessionPlaybackError> {
    let Some(mut origin) = release.source_origin else {
        return Ok(ReleaseAudioPlan {
            current_receiver: None,
            old_receiver: previous
                .routes
                .iter()
                .position(|route| route.id == release.track_id),
        });
    };
    if !origin.follows_current_configuration {
        return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
            "detached source release has no captured receiver suffix",
        ));
    }
    let old_receiver = previous
        .routes
        .iter()
        .position(|route| route.id == origin.receiver)
        .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
    let current_receiver = candidate
        .arrangement()
        .tracks()
        .iter()
        .enumerate()
        .find(|(index, track)| {
            candidate.source_routing().source(*index) == Some(origin.source)
                && track.receives_source_audio()
        })
        .map(|(index, _)| index);
    let continued_context = previous
        .revision
        .arrangement()
        .shared_contexts()
        .index_for_source(origin.source)
        .is_some_and(|index| analytic_owner_matches[usize::from(index)]);
    let follows = previous
        .revision
        .retained_source_configuration_matches(candidate, origin.source)
        || continued_context;
    if follows && let Some(receiver) = current_receiver {
        origin.receiver = candidate.arrangement().tracks()[receiver]
            .canonical_track_id()
            .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
        release.source_origin = Some(origin);
        release.mix = gain_recording::effective_mix(
            &candidate.arrangement().tracks()[receiver],
            previous.arrangement_frame,
            candidate.gain_recording_track(),
            None,
        );
        return Ok(ReleaseAudioPlan {
            current_receiver: Some(receiver),
            old_receiver: Some(old_receiver),
        });
    }
    origin.follows_current_configuration = false;
    release.source_origin = Some(origin);
    let source_track = &previous.revision.arrangement().tracks()[old_receiver];
    if !source_track.instrument_bypassed {
        release.clip_gain *= f64::from(source_track.instrument_output_gain);
    }
    release.mix = gain_recording::effective_mix(
        &previous.revision.arrangement().tracks()[old_receiver],
        previous.arrangement_frame,
        previous.revision.gain_recording_track(),
        previous.gain_recording_override,
    );
    Ok(ReleaseAudioPlan {
        current_receiver: None,
        old_receiver: Some(old_receiver),
    })
}
