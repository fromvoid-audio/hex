use super::*;
use crate::native_device_resources::{
    PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceMetadata,
};
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
    fixture_at(8000)
}
fn fixture_at(rate: u32) -> Fixture {
    let mut editor = Editor::new(hex_project::Project::new(rate).unwrap(), 64).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "held.wav".into(),
            sample_rate: rate,
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
                crate::OwnedAudioSource::try_resident(rate, PCM.to_vec())
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
        shared_analytic_release_through_receiver_suffix(
            hex_devices::saturator::SATURATOR_TYPE_ID,
            hex_devices::saturator::parameter::DRIVE,
            30.0,
            0,
        )
    });
}
/// Pulse #4188: Overdrive declares oversampler latency, so the released
/// notes run through a latent nonlinear receiver suffix.
#[test]
fn shared_analytic_release_runs_through_a_latent_nonlinear_receiver_suffix() {
    on_control(|| {
        shared_analytic_release_through_receiver_suffix(
            hex_devices::overdrive::OVERDRIVE_TYPE_ID,
            hex_devices::overdrive::parameter::DRIVE,
            0.8,
            0,
        )
    });
}
/// Pulse #4188: when compensation delays the receiver's own edge to Main,
/// a detached copy of its suffix would land early. The strict planner
/// refuses explicitly instead of committing a silent cut.
#[test]
fn misaligned_latent_receiver_suffix_release_refuses_instead_of_silencing() {
    on_control(|| {
        shared_analytic_release_through_receiver_suffix(
            hex_devices::overdrive::OVERDRIVE_TYPE_ID,
            hex_devices::overdrive::parameter::DRIVE,
            0.8,
            2,
        )
    });
}
/// Unbinding a shared source while its notes sound releases them through
/// the receiver's suffix exactly as a baseline that stops the track. With
/// `contributor_effects` copies of the effect on the contributor, the
/// contributor arrives later than the receiver and the handoff refuses.
fn shared_analytic_release_through_receiver_suffix(
    effect: hex_lattice_contract::DeviceTypeId,
    drive: hex_lattice_contract::ParameterId,
    drive_value: f32,
    contributor_effects: usize,
) {
    {
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
        let definition = registry.definition(effect).unwrap();
        let state = definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                drive,
                hex_lattice_contract::ParameterValue::Float(drive_value),
            )
            .unwrap();
        for _ in 0..contributor_effects {
            f.editor
                .execute(Command::InsertDevice {
                    track: f.contributor,
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
        }
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
        if contributor_effects == 0 {
            // The baseline has no contributor, so only an equal latency
            // layout renders the same prefix.
            assert_eq!(left, right);
        }
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
        if contributor_effects != 0 {
            assert!(matches!(
                candidate.try_inherit_state_from(&mut actual),
                Err(SessionPlaybackError::Arrangement(message))
                    if message == super::super::COMPENSATED_TAILS_CHANGED
            ));
            assert!(
                candidate.candidate_admission_open,
                "refusal commits nothing"
            );
            return;
        }
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
    }
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
        // The relinked contributor's note releases on the old receiver.
        check_released_pcm(&mut next, 7, 19, 0.75, 0.25);
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
    live_playback_on(f, input, f.contributor)
}

fn live_playback_on(
    f: &Fixture,
    input: &Arc<crate::computer_keyboard::ComputerKeyboardInput>,
    track: TrackId,
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
    if !f
        .editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .bypassed()
    {
        result
            .prepare_computer_keyboard(f.editor.project(), track, Arc::clone(input))
            .unwrap();
    }
    (result, revision)
}

#[test]
fn operator_mute_retains_prepared_arrangement_identity() {
    on_control(|| {
        use hex_lattice_contract::ParameterValue;
        let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 32).unwrap();
        let registry = hex_devices::native_instrument_registry().unwrap();
        let state = registry
            .default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)
            .unwrap();
        let state = registry
            .patch_parameter(
                &state,
                hex_devices::operator_state::parameter::GLOBAL_MODE,
                ParameterValue::Choice(1),
            )
            .unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state,
            })
            .unwrap()
        else {
            panic!("receiver")
        };
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
        for (track, count) in [(receiver, 2), (contributor, 45)] {
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
            for pitch in 0..count {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap();
            }
        }
        let prepare = |project: &hex_project::Project| {
            Arc::new(
                PreparedRevision::prepare(
                    project,
                    &BTreeMap::new(),
                    Arc::new(crate::SineTable::new()),
                    None,
                )
                .unwrap(),
            )
        };
        let old = prepare(editor.project());
        // Resource-scoped preparation keeps complete predecessor evidence.
        // Its actual retained metadata must fit the pre-construction quote.
        let cancel = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &ResidentDeviceAssets::default(),
            editor.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
        let scoped = PreparedRevision::prepare_with_device_resources(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
            &crate::audio_transform::AudioTransformPreparation::default(),
            &resources,
            resources
                .compiler_scan_work_on_control(editor.project())
                .unwrap(),
            &cancel,
        )
        .unwrap();
        let retained = scoped.device_handoff_metadata_bytes().unwrap();
        let quoted =
            PreparedRevision::device_handoff_metadata_construction_bytes(editor.project()).unwrap();
        assert!(
            retained > 0 && retained <= quoted,
            "retained {retained}, quoted {quoted}"
        );
        editor
            .execute(Command::Mute {
                track: contributor,
                muted: true,
            })
            .unwrap();
        let candidate = prepare(editor.project());
        assert!(
            old.retained_source_configuration_matches(&candidate, source),
            "operator source configuration changed"
        );
        for track in [receiver, contributor] {
            assert!(
                old.has_source_track_revision(track),
                "old descriptor missing for {track:?}"
            );
            assert!(
                candidate.has_source_track_revision(track),
                "candidate descriptor missing for {track:?}"
            );
            assert!(
                old.bounded_mutable_arrangement_sources_match(&candidate, track, 16),
                "operator arrangement identity changed for {track:?}"
            );
        }
        assert!(
            !old.has_mutable_track_revision(contributor),
            "supplemental evidence must not change the existing mutable-owner query"
        );
        assert!(!candidate.has_mutable_track_revision(contributor));
        let clip = editor.project().track(contributor).unwrap().midi_clips()[0].id();
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 90,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let changed = prepare(editor.project());
        assert!(
            !old.bounded_mutable_arrangement_sources_match(&changed, contributor, 16),
            "changed source must not acquire continuity proof"
        );
        let notes: Vec<_> = editor
            .project()
            .midi_source(clip)
            .unwrap()
            .notes()
            .iter()
            .skip(1)
            .map(|note| note.id())
            .collect();
        for note in notes {
            editor.execute(Command::RemoveMidiNote(note)).unwrap();
        }
        let remaining = editor.project().midi_source(clip).unwrap().notes()[0].id();
        editor
            .execute(Command::UpdateMidiNote {
                note: remaining,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 67,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        editor
            .execute(Command::Mute {
                track: contributor,
                muted: false,
            })
            .unwrap();
        let device = editor
            .project()
            .track(receiver)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::SetNativeSourceReceiver {
                source,
                track: Some(contributor),
            })
            .unwrap();
        let swapped = prepare(editor.project());
        editor
            .execute(Command::Mute {
                track: receiver,
                muted: true,
            })
            .unwrap();
        let swapped_muted = prepare(editor.project());
        assert!(swapped.retained_source_configuration_matches(&swapped_muted, source));
        assert!(swapped.retained_source_identity_matches(&swapped_muted, source));
        assert!(swapped.retained_source_configuration_matches_on_callback(&swapped_muted, source));
        let mut playing = SessionPlayback::new(Arc::clone(&swapped)).unwrap();
        playing.apply(SessionPlaybackCommand::Play).unwrap();
        playing.render(&mut [[0.0; 2]; 64]).unwrap();
        let mut muted = SessionPlayback::new(Arc::clone(&swapped_muted)).unwrap();
        muted
            .try_inherit_state_from(&mut playing)
            .expect("muting must preserve a non-first receiver's source");
        let mut output = [[0.0; 2]; 64];
        muted.render(&mut output).unwrap();
        assert!(
            output
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.000_001)
        );
        let changed_state = registry
            .default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)
            .unwrap();
        let changed_state = registry
            .patch_parameter(
                &changed_state,
                hex_devices::operator_state::parameter::GLOBAL_MODE,
                ParameterValue::Choice(0),
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: changed_state,
            })
            .unwrap();
        let changed_configuration = prepare(editor.project());
        assert!(!swapped.retained_source_configuration_matches(&changed_configuration, source));
        assert!(
            !swapped
                .retained_source_configuration_matches_on_callback(&changed_configuration, source)
        );
        editor
            .execute(Command::SetNativeSourceReceiver {
                source,
                track: None,
            })
            .unwrap();
        let no_receiver = prepare(editor.project());
        assert!(!swapped.retained_source_configuration_matches(&no_receiver, source));
        assert!(!swapped.retained_source_configuration_matches_on_callback(&no_receiver, source));
    });
}

#[test]
fn shared_source_unchanged_peak_keeps_gain_edit_with_held_live_keys() {
    on_control(|| {
        use crate::computer_keyboard::ComputerKeyboardInput;
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        let clip = f.editor.project().track(f.receiver).unwrap().midi_clips()[0].id();
        for pitch in 40..50 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::from_ratio(1, 16).unwrap(),
                    duration: BeatTime::from_ratio(1, 2).unwrap(),
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let input = Arc::new(ComputerKeyboardInput::default());
        let (mut old, _) = live_playback_on(&f, &input, f.receiver);
        old.apply(PlaybackCommand::Play);
        let mut output = [[0.0; 2]; 64];
        old.render(&mut output).unwrap();
        for key in 0..16 {
            input
                .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                .unwrap();
        }
        old.render(&mut output).unwrap();
        assert!(input.rejected());
        assert!(
            old.session_runtime_mut_for_stream_action()
                .unwrap()
                .mutable_instruments
                .iter()
                .flatten()
                .any(|source| source.has_committed_live_occurrences())
        );
        for _ in 0..4 {
            old.render(&mut output).unwrap();
        }
        f.editor
            .execute(Command::Gain {
                track: f.receiver,
                gain: 0.5,
            })
            .unwrap();
        let (mut candidate, _) = live_playback_on(&f, &input, f.receiver);
        candidate
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .expect("unchanged source peak must preserve a gain edit");
        candidate.render(&mut output).unwrap();
        assert!(!input.is_cancelled());
        assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
    });
}

#[test]
fn shared_source_arrangement_peak_increase_with_held_live_keys_is_safe() {
    on_control(|| {
        use crate::computer_keyboard::ComputerKeyboardInput;
        use crate::playback::{PlaybackCommand, PlaybackHandoffError, PlaybackHandoffRejection};
        let mut f = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        let oracle_input = Arc::new(ComputerKeyboardInput::default());
        let (mut old, _) = live_playback_on(&f, &input, f.receiver);
        let (mut oracle, _) = live_playback_on(&f, &oracle_input, f.receiver);
        old.apply(PlaybackCommand::Play);
        oracle.apply(PlaybackCommand::Play);
        let mut output = [[0.0; 2]; 64];
        let mut expected = output;
        old.render(&mut output).unwrap();
        oracle.render(&mut expected).unwrap();
        for key in 0..16 {
            input
                .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                .unwrap();
            oracle_input
                .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                .unwrap();
        }
        old.render(&mut output).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(output, expected);
        assert!(
            input.rejected(),
            "the original source must reach live capacity"
        );
        assert!(
            old.session_runtime_mut_for_stream_action()
                .unwrap()
                .mutable_instruments
                .iter()
                .flatten()
                .any(|source| source.has_committed_live_occurrences()),
            "at least one live key must be admitted"
        );
        let clip = f.editor.project().track(f.receiver).unwrap().midi_clips()[0].id();
        for pitch in 40..50 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::from_ratio(1, 4).unwrap(),
                    duration: BeatTime::from_ratio(1, 2).unwrap(),
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let (mut candidate, _) = live_playback_on(&f, &input, f.receiver);
        let error = candidate
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap_err();
        assert!(matches!(
            error,
            PlaybackHandoffError::Session(SessionPlaybackError::SharedSourceVoiceCapacity)
        ));
        assert_eq!(
            error.rejection(),
            Some(PlaybackHandoffRejection::SharedSourceVoiceCapacity)
        );
        old.render(&mut output)
            .expect("rejected handoff must preserve old audio");
        oracle.render(&mut expected).unwrap();
        assert_eq!(output, expected, "refusal must leave playback unchanged");
        assert!(
            !input.is_cancelled(),
            "safe refusal must not cancel live input"
        );
        assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
        for key in 0..16 {
            input.set_key(key, None).unwrap();
            oracle_input.set_key(key, None).unwrap();
        }
        for _ in 0..3 {
            old.render(&mut output).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(output, expected);
        }
        let (mut retry, _) = live_playback_on(&f, &input, f.receiver);
        retry
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        let mut heard_new_notes = false;
        for _ in 0..12 {
            retry.render(&mut output).unwrap();
            oracle.render(&mut expected).unwrap();
            heard_new_notes |= output != expected;
        }
        assert!(
            heard_new_notes,
            "retry must render the newly authored notes"
        );
    });
}

#[test]
fn shared_source_mute_preserves_saturated_unchanged_arrangement() {
    on_control(|| {
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.envelope.release_seconds = 1.0;
        let replacement = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: replacement,
            })
            .unwrap();
        let receiver_clip = f.editor.project().track(f.receiver).unwrap().midi_clips()[0].id();
        let contributor_clip = f
            .editor
            .project()
            .track(f.contributor)
            .unwrap()
            .midi_clips()[0]
            .id();
        for (clip, pitches) in [
            (receiver_clip, vec![67]),
            (contributor_clip, (0..13).collect()),
        ] {
            for pitch in pitches {
                f.editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
        }
        let mut old = playback(&f);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut candidate = playback(&f);
        assert!(
            old.revision
                .retained_source_configuration_matches(&candidate.revision, f.source),
            "source configuration changed"
        );
        for track in [f.receiver, f.contributor] {
            assert!(
                old.revision.bounded_mutable_arrangement_sources_match(
                    &candidate.revision,
                    track,
                    16
                ),
                "arrangement identity changed for {track:?}"
            );
        }
        candidate
            .try_inherit_state_from(&mut old)
            .expect("muting cannot add future source demand");
        candidate.render(&mut [[0.0; 2]; 64]).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let mut reenabled = playback(&f);
        assert!(matches!(
            reenabled.try_inherit_state_from(&mut candidate),
            Err(SessionPlaybackError::SharedSourceVoiceCapacity)
        ));
        candidate.render(&mut [[0.0; 2]; 64]).unwrap();
    });
}

#[test]
fn shared_source_future_notes_do_not_overrun_retained_authored_releases() {
    on_control(|| {
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.envelope.release_seconds = 1.0;
        let replacement = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: replacement,
            })
            .unwrap();
        let clip = f
            .editor
            .project()
            .track(f.contributor)
            .unwrap()
            .midi_clips()[0]
            .id();
        for pitch in 0..13 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let mut old = playback(&f);
        let mut oracle = playback(&f);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        oracle.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        oracle.render(&mut [[0.0; 2]; 7]).unwrap();
        let notes: Vec<_> = f
            .editor
            .project()
            .midi_source(clip)
            .unwrap()
            .notes()
            .iter()
            .map(|note| note.id())
            .collect();
        assert_eq!(notes.len(), 14);
        for note in notes {
            f.editor.execute(Command::RemoveMidiNote(note)).unwrap();
        }
        for pitch in 30..44 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::from_ratio(1, 16).unwrap(),
                    duration: BeatTime::from_ratio(1, 2).unwrap(),
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let mut candidate = playback(&f);
        let mut output = [[0.0; 2]; 64];
        let mut expected = output;
        assert!(matches!(
            candidate.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::SharedSourceVoiceCapacity)
        ));
        old.render(&mut output)
            .expect("refused edit must leave the old source rendering");
        oracle.render(&mut expected).unwrap();
        assert_eq!(output, expected, "refusal must preserve existing audio");
        assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
        old.apply(SessionPlaybackCommand::Stop).unwrap();
        oracle.apply(SessionPlaybackCommand::Stop).unwrap();
        for _ in 0..130 {
            old.render(&mut output).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(output, expected);
        }
        let mut retry = playback(&f);
        let new_notes: Vec<_> = f
            .editor
            .project()
            .midi_source(clip)
            .unwrap()
            .notes()
            .iter()
            .map(|note| note.id())
            .collect();
        for note in new_notes {
            f.editor.execute(Command::RemoveMidiNote(note)).unwrap();
        }
        let mut peer_only = playback(&f);
        retry.try_inherit_state_from(&mut old).unwrap();
        peer_only.try_inherit_state_from(&mut oracle).unwrap();
        retry.apply(SessionPlaybackCommand::Play).unwrap();
        peer_only.apply(SessionPlaybackCommand::Play).unwrap();
        let mut heard_new_notes = false;
        for _ in 0..40 {
            retry.render(&mut output).unwrap();
            peer_only.render(&mut expected).unwrap();
            heard_new_notes |= output != expected;
        }
        assert!(
            heard_new_notes,
            "retry must render the newly authored notes"
        );
    });
}

#[test]
fn shared_source_peak_increase_with_held_live_keys_is_safe() {
    on_control(|| {
        use crate::computer_keyboard::ComputerKeyboardInput;
        use crate::playback::{PlaybackAction, PlaybackActionError, PlaybackCommand};
        let mut f = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        let (mut old, _) = live_playback_on(&f, &input, f.receiver);
        old.apply(PlaybackCommand::Play);
        let mut output = [[0.0; 2]; 64];
        old.render(&mut output).unwrap();
        for key in 0..16 {
            input
                .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                .unwrap();
        }
        old.render(&mut output).unwrap();
        assert!(input.rejected());
        assert!(
            old.session_runtime_mut_for_stream_action()
                .unwrap()
                .mutable_instruments
                .iter()
                .flatten()
                .any(|source| source.has_committed_live_occurrences())
        );
        let Created::Scene(scene) = f
            .editor
            .execute(Command::AddScene {
                name: "higher peak".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(clip) = f
            .editor
            .execute(Command::CreateSessionMidiClip {
                track: f.receiver,
                scene,
                name: "higher peak".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        for pitch in 40..50 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let (mut candidate, _) = live_playback_on(&f, &input, f.receiver);
        candidate
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        let launch = PlaybackAction::Session(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: crate::LaunchQuantization::Immediate,
        });
        assert!(matches!(
            candidate.try_apply_action(launch),
            Err(PlaybackActionError::Session(
                SessionPlaybackError::SharedSourceVoiceCapacity
            ))
        ));
        assert_eq!(
            candidate
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .owner(f.receiver),
            Some(SessionPlaybackOwner::Arrangement)
        );
        candidate.render(&mut output).unwrap();
        assert!(!input.is_cancelled());
        assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
        for key in 0..16 {
            input.set_key(key, None).unwrap();
        }
        candidate.render(&mut output).unwrap();
        candidate.render(&mut output).unwrap();
        candidate.render(&mut output).unwrap();
        candidate.try_apply_action(launch).unwrap();
        candidate.render(&mut output).unwrap();
        assert_eq!(
            candidate
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .owner(f.receiver),
            Some(SessionPlaybackOwner::Session { clip })
        );
    });
}

#[test]
fn shared_source_session_launch_with_live_keys_respects_reserved_capacity() {
    on_control(|| {
        use crate::computer_keyboard::ComputerKeyboardInput;
        use crate::playback::{PlaybackAction, PlaybackCommand};

        let mut f = fixture();
        let Created::Scene(scene) = f
            .editor
            .execute(Command::AddScene {
                name: "capacity".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(clip) = f
            .editor
            .execute(Command::CreateSessionMidiClip {
                track: f.receiver,
                scene,
                name: "next".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        for pitch in 40..48 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let input = Arc::new(ComputerKeyboardInput::default());
        let (mut old, _) = live_playback_on(&f, &input, f.receiver);
        old.apply(PlaybackCommand::Play);
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let (mut candidate, _) = live_playback_on(&f, &input, f.receiver);
        candidate
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        for key in 0..16 {
            input
                .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                .unwrap();
        }
        let mut output = [[0.0; 2]; 64];
        candidate.render(&mut output).unwrap();
        assert!(input.rejected(), "the fixture must saturate live admission");
        assert!(
            candidate
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .mutable_instruments
                .iter()
                .flatten()
                .any(|source| source.has_committed_live_occurrences()),
            "the fixture must also admit at least one live voice"
        );
        candidate
            .try_apply_action(PlaybackAction::Session(
                SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: crate::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        candidate.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
        assert_eq!(
            candidate
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .owner(f.receiver),
            Some(SessionPlaybackOwner::Session { clip })
        );
        assert!(
            candidate
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .mutable_instruments
                .iter()
                .flatten()
                .any(|source| source.has_committed_live_occurrences()),
            "the launch must retain admitted live voices"
        );
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let (mut unmuted, _) = live_playback_on(&f, &input, f.receiver);
        let (mut active, refused) = match unmuted.try_inherit_runtime_in_control_context(
            &mut candidate,
            Some(19),
            Some(19),
        ) {
            Ok(()) => (unmuted, false),
            Err(error) => {
                assert!(matches!(
                    error,
                    crate::playback::PlaybackHandoffError::Session(
                        SessionPlaybackError::SharedSourceVoiceCapacity
                    )
                ));
                (candidate, true)
            }
        };
        active.render(&mut output).unwrap();
        assert_eq!(
            active
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .owner(f.receiver),
            Some(SessionPlaybackOwner::Session { clip })
        );
        assert!(
            active
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .mutable_instruments
                .iter()
                .flatten()
                .any(|source| source.has_committed_live_occurrences()),
            "unmute or its refusal must retain live voices"
        );
        assert!(!input.is_cancelled());
        assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
        active.apply(PlaybackCommand::Stop);
        active.render(&mut output).unwrap();
        assert!(input.is_cancelled());
        if refused {
            let retry_input = Arc::new(ComputerKeyboardInput::default());
            let (mut retry, _) = live_playback_on(&f, &retry_input, f.receiver);
            retry
                .try_inherit_runtime_in_control_context(&mut active, Some(19), Some(19))
                .expect("unmute must retry after Stop clears source pressure");
            retry.apply(PlaybackCommand::Play);
            retry.render(&mut output).unwrap();
            assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
        }
    });
}

#[test]
fn shared_source_refuses_session_launch_at_retained_voice_capacity_atomically() {
    on_control(|| {
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.envelope.release_seconds = 1.0;
        let replacement = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: replacement,
            })
            .unwrap();
        let clip = f
            .editor
            .project()
            .track(f.contributor)
            .unwrap()
            .midi_clips()[0]
            .id();
        for pitch in 0..14 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let Created::Scene(scene) = f
            .editor
            .execute(Command::AddScene {
                name: "capacity".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(session_clip) = f
            .editor
            .execute(Command::CreateSessionMidiClip {
                track: f.receiver,
                scene,
                name: "next".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        f.editor
            .execute(Command::AddMidiNote {
                clip: session_clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 64,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        let mut old = playback(&f);
        let mut twin = playback(&f);
        for runtime in [&mut old, &mut twin] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 7]).unwrap();
        }
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut candidate = playback(&f);
        let mut oracle = playback(&f);
        candidate.try_inherit_state_from(&mut old).unwrap();
        oracle.try_inherit_state_from(&mut twin).unwrap();
        let result = candidate.apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: crate::LaunchQuantization::Immediate,
        });
        assert!(
            matches!(result, Err(SessionPlaybackError::SharedSourceVoiceCapacity)),
            "{result:?}"
        );
        let mut actual = [[0.0; 2]; 64];
        let mut expected = actual;
        candidate.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            actual, expected,
            "refused launch must not alter the running source"
        );
        assert!(actual.iter().flatten().any(|sample| sample.abs() > 0.001));
        // The held notes end at one quarter note and their declared
        // one-second tails then retire. A fresh explicit launch can retry.
        for _ in 0..190 {
            candidate.render(&mut actual).unwrap();
        }
        candidate
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: session_clip,
                quantization: crate::LaunchQuantization::Immediate,
            })
            .unwrap();
        candidate.render(&mut actual).unwrap();
        assert_eq!(
            candidate.owner(f.receiver),
            Some(SessionPlaybackOwner::Session { clip: session_clip })
        );
        assert!(actual.iter().flatten().any(|sample| sample.abs() > 0.001));
        candidate.apply(SessionPlaybackCommand::Stop).unwrap();
        for _ in 0..130 {
            candidate.render(&mut actual).unwrap();
        }
        assert_eq!(actual, [[0.0; 2]; 64]);
    });
}

#[test]
fn shared_source_rejects_live_key_at_capacity_without_fault_or_stuck_input() {
    on_control(|| {
        use crate::computer_keyboard::ComputerKeyboardInput;
        use crate::playback::PlaybackCommand;

        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.envelope.release_seconds = 1.0;
        let replacement = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: replacement,
            })
            .unwrap();
        let clip = f
            .editor
            .project()
            .track(f.contributor)
            .unwrap()
            .midi_clips()[0]
            .id();
        for pitch in 0..12 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let input = Arc::new(ComputerKeyboardInput::default());
        let (mut old, _) = live_playback_on(&f, &input, f.receiver);
        let muted_input = Arc::new(ComputerKeyboardInput::default());
        let (mut old_muted_input, _) = live_playback(&f, &muted_input);
        old.apply(PlaybackCommand::Play);
        old_muted_input.apply(PlaybackCommand::Play);
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        old_muted_input.render(&mut [[0.0; 2]; 7]).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let (mut muted, _) = live_playback_on(&f, &input, f.receiver);
        let (mut muted_selected, _) = live_playback(&f, &muted_input);
        muted
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        muted_selected
            .try_inherit_runtime_in_control_context(&mut old_muted_input, Some(19), Some(19))
            .unwrap();
        muted_input.set_key(0, Some((60, 100, 1))).unwrap();
        muted_selected.render(&mut [[0.0; 2]; 64]).unwrap();
        assert!(muted_input.rejected());
        assert_eq!(
            muted_selected
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .computer_keyboard
                .as_ref()
                .unwrap()
                .muted_source_held_count(),
            0,
            "a rejected press on a muted track must not become eligible"
        );
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut output = [[0.0; 2]; 64];
        muted.render(&mut output).unwrap();
        assert!(
            input.rejected(),
            "a full source must report the refused press"
        );
        assert_eq!(
            muted
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .computer_keyboard
                .as_ref()
                .unwrap()
                .muted_source_held_count(),
            0,
            "a capacity-rejected press is not mute-suppressed"
        );
        assert!(
            !input.is_cancelled(),
            "capacity refusal must not cancel the input"
        );
        assert!(
            output.iter().flatten().any(|sample| sample.abs() > 0.001),
            "the existing peer must continue"
        );
        for _ in 0..190 {
            muted.render(&mut output).unwrap();
        }
        assert_eq!(
            output, [[0.0; 2]; 64],
            "a rejected held key must not replay when capacity returns"
        );
        input.set_key(0, None).unwrap();
        muted.render(&mut output).unwrap();
        input.set_key(0, Some((60, 100, 2))).unwrap();
        muted.render(&mut output).unwrap();
        assert!(
            output.iter().flatten().any(|sample| sample.abs() > 0.001),
            "a fresh press must work after the voices retire"
        );
        input.set_key(0, None).unwrap();
        muted.render(&mut output).unwrap();
        muted.apply(PlaybackCommand::Stop);
        for _ in 0..130 {
            muted.render(&mut output).unwrap();
        }
        assert_eq!(
            output, [[0.0; 2]; 64],
            "stop and release must drain the source"
        );
    });
}

#[test]
fn live_sampler_power_cycles_without_reusing_old_input_ids() {
    on_control(|| {
        use crate::computer_keyboard::ComputerKeyboardInput;
        use crate::playback::PlaybackCommand;
        for (key_up, utility_gain_db) in [
            ("before-disable", None),
            ("while-disabled", None),
            ("after-reenable", None),
            ("after-reenable", Some(-6.0)),
        ] {
            let mut f = fixture();
            let clips: Vec<_> = f
                .editor
                .project()
                .tracks()
                .iter()
                .flat_map(|track| track.midi_clips().iter().map(|clip| clip.id()))
                .collect();
            for clip in clips {
                f.editor.execute(Command::RemoveMidiClip(clip)).unwrap();
            }
            let instrument = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap();
            let device = instrument.id();
            let mut state =
                hex_devices::sampler::state::SamplerState::from_native(instrument.state()).unwrap();
            state.envelope.release_seconds = 0.1;
            let state = state
                .to_native()
                .unwrap()
                .with_asset_references(instrument.state().asset_references().to_vec())
                .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            if let Some(gain_db) = utility_gain_db {
                let state = hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap()
                    .with_output_gain_db(gain_db)
                    .unwrap();
                f.editor
                    .execute(Command::InsertDevice {
                        track: f.receiver,
                        before: None,
                        state,
                    })
                    .unwrap();
            }
            let input = Arc::new(ComputerKeyboardInput::default());
            let oracle_input = Arc::new(ComputerKeyboardInput::default());
            for keys in [&input, &oracle_input] {
                keys.set_key(0, Some((60, 127, 1))).unwrap();
            }
            let (mut actual, _) = live_playback(&f, &input);
            let (mut oracle, _) = live_playback(&f, &oracle_input);
            actual.apply(PlaybackCommand::Play);
            oracle.apply(PlaybackCommand::Play);
            let compare = |actual: &mut crate::playback::Playback,
                           oracle: &mut crate::playback::Playback,
                           frames: usize,
                           phase: &str| {
                let mut a = vec![[0.0; 2]; frames];
                let mut b = a.clone();
                actual.render(&mut a).unwrap();
                oracle.render(&mut b).unwrap();
                assert_eq!(a, b, "{key_up}, utility={utility_gain_db:?}: {phase}");
                if phase == "held prefix" {
                    assert!(a.iter().flatten().any(|x| x.abs() > 0.001));
                }
            };
            compare(&mut actual, &mut oracle, 129, "held prefix");
            if key_up == "before-disable" {
                input.set_key(0, None).unwrap();
                oracle_input.set_key(0, None).unwrap();
                compare(&mut actual, &mut oracle, 13, "physical release");
            }
            f.editor
                .execute(Command::SetDeviceBypass {
                    device,
                    bypassed: true,
                })
                .unwrap();
            let (mut disabled, _) = live_playback(&f, &input);
            disabled
                .try_inherit_runtime_in_control_context(&mut actual, Some(19), Some(19))
                .unwrap();
            let mut actual_fade = [[0.0; 2]; 32];
            let mut expected_fade = actual_fade;
            disabled.render(&mut actual_fade).unwrap();
            oracle.render(&mut expected_fade).unwrap();
            apply_power_fade(&mut expected_fade, 0, 40);
            for (actual, expected) in actual_fade
                .iter()
                .flatten()
                .zip(expected_fade.iter().flatten())
            {
                assert!((actual - expected).abs() < 0.000_001);
            }
            assert!(input.is_cancelled(), "disabled input is terminal");
            if key_up == "while-disabled" {
                input.set_key(0, None).unwrap();
            }
            input.set_key(1, Some((67, 127, 2))).unwrap();
            let mut settled = [[0.0; 2]; 32];
            let mut expected = settled;
            disabled.render(&mut settled).unwrap();
            oracle.render(&mut expected).unwrap();
            apply_power_fade(&mut expected, 32, 40);
            for (actual, expected) in settled.iter().flatten().zip(expected.iter().flatten()) {
                assert!((actual - expected).abs() < 0.000_001);
            }
            f.editor
                .execute(Command::SetDeviceBypass {
                    device,
                    bypassed: false,
                })
                .unwrap();
            // Resident input attachment drops a bypassed source. A later
            // activation gets a fresh root/generation, even if tokens repeat.
            let fresh = Arc::new(ComputerKeyboardInput::default());
            let (mut enabled, _) = live_playback(&f, &fresh);
            enabled
                .try_inherit_runtime_in_control_context(&mut disabled, Some(19), Some(19))
                .unwrap();
            let oracle_input = Arc::new(ComputerKeyboardInput::default());
            let (mut oracle, _) = live_playback(&f, &oracle_input);
            oracle.apply(PlaybackCommand::Play);
            compare(&mut enabled, &mut oracle, 13, "fresh kernel");
            for keys in [&fresh, &oracle_input] {
                keys.set_key(1, Some((67, 127, 1))).unwrap();
            }
            compare(&mut enabled, &mut oracle, 13, "new physical key");
            input.set_key(0, None).unwrap();
            fresh.set_key(0, None).unwrap();
            compare(
                &mut enabled,
                &mut oracle,
                13,
                "old key up cannot cut the new key",
            );
            for keys in [&fresh, &oracle_input] {
                keys.set_key(1, None).unwrap();
            }
            compare(&mut enabled, &mut oracle, 900, "all released");
            let mut silence = [[1.0; 2]; 32];
            enabled.render(&mut silence).unwrap();
            assert_eq!(silence, [[0.0; 2]; 32]);
        }
    });
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

#[test]
fn singleton_clip_activation_and_effect_edit_keep_live_source_phase_and_key_off() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        f.editor.execute(Command::RemoveTrack(f.receiver)).unwrap();
        f.editor
            .execute(Command::SetNativeSourceReceiver {
                source: f.source,
                track: Some(f.contributor),
            })
            .unwrap();
        f.editor
            .execute(Command::Gain {
                track: f.contributor,
                gain: 0.25,
            })
            .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut previous, _) = live_playback(&f, &input);
        previous.apply(PlaybackCommand::Play);
        let mut prefix = [[0.0; 2]; 7];
        previous.render(&mut prefix).unwrap();
        for (index, frame) in prefix.iter().enumerate() {
            for channel in 0..2 {
                assert!((frame[channel] - PCM[index % 4][channel] * 0.5).abs() < 0.000_002);
            }
        }
        let clip = f
            .editor
            .project()
            .track(f.contributor)
            .unwrap()
            .midi_clips()[0]
            .id();
        f.editor
            .execute(Command::SetClipActive {
                clip,
                active: false,
            })
            .unwrap();
        let (mut muted, _) = live_playback(&f, &input);
        muted
            .try_inherit_runtime_in_control_context(&mut previous, Some(19), Some(19))
            .unwrap();
        // The deactivated clip's note releases from frame7 under the
        // still-held key.
        let release = release_frames(0.0);
        let mut after_cut = [[0.0; 2]; 5];
        muted.render(&mut after_cut).unwrap();
        for (index, frame) in after_cut.iter().enumerate() {
            let gain = 0.25 * (1.0 + release_level(index, release));
            for channel in 0..2 {
                assert!((frame[channel] - PCM[(7 + index) % 4][channel] * gain).abs() < 0.000_002);
            }
        }
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            hex_devices::utility::UTILITY_TYPE_ID,
        )
        .unwrap();
        let state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Effect,
            &state,
            hex_devices::utility::parameter::GAIN,
            hex_lattice_contract::ParameterValue::Float(0.5),
        )
        .unwrap();
        f.editor
            .execute(Command::InsertDevice {
                track: f.contributor,
                before: None,
                state,
            })
            .unwrap();
        let (mut effected, _) = live_playback(&f, &input);
        effected
            .try_inherit_runtime_in_control_context(&mut muted, Some(19), Some(19))
            .unwrap();
        let runtime = effected.session_runtime_mut_for_stream_action().unwrap();
        assert_eq!(runtime.mutable_instruments.iter().flatten().count(), 1);
        assert!(runtime.tail_mutable_instruments.iter().all(Option::is_none));
        assert!(
            runtime.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .has_committed_live_occurrences()
        );
        let mut after_effect = [[0.0; 2]; 11];
        effected.render(&mut after_effect).unwrap();
        let gain = 0.25 * 0.5;
        for (index, frame) in after_effect.iter().enumerate() {
            let gain = gain * (1.0 + release_level(5 + index, release));
            for channel in 0..2 {
                assert!((frame[channel] - PCM[(12 + index) % 4][channel] * gain).abs() < 0.000_002);
            }
        }
        // An unsupported source-state edit cannot retire a still-held
        // live generation into a concurrent second source kernel.
        let device = f
            .editor
            .project()
            .track(f.contributor)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.reverse = !state.reverse;
        let state = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state,
            })
            .unwrap();
        // The live handoff restarts audio at the cursor instead of
        // refusing (Pulse #3784). It must still never sound two source
        // kernels at once, and releasing the held key must leave silence.
        let (mut restarted, _) = live_playback(&f, &input);
        restarted
            .try_inherit_runtime_in_control_context(&mut effected, Some(19), Some(19))
            .unwrap();
        let mut after_restart = [[0.0; 2]; 64];
        restarted.render(&mut after_restart).unwrap();
        let single_kernel = 0.5 * gain + 0.000_002;
        assert!(
            after_restart
                .iter()
                .flatten()
                .all(|sample| sample.abs() <= single_kernel),
            "no second concurrent source kernel"
        );
        input.set_key(0, None).unwrap();
        let mut stopped = [[1.0; 2]; 64];
        restarted.render(&mut stopped).unwrap();
        assert_eq!(stopped[32..], [[0.0; 2]; 32], "no stuck live note");
    });
}

#[test]
fn reenable_refuses_a_second_source_kernel_until_retired_tail_is_quiet() {
    on_control(|| {
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.envelope.release_seconds = 0.001;
        let state = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        let device = device.id();
        f.editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let mut old = started(&f);
        let mut reference = started(&f);
        f.editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let mut disabled = playback(&f);
        disabled.try_inherit_state_from(&mut old).unwrap();
        assert!(disabled.mutable_instruments.iter().all(Option::is_none));
        assert_eq!(
            disabled.tail_mutable_instruments.iter().flatten().count(),
            1
        );
        f.editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: false,
            })
            .unwrap();
        let mut reenabled = playback(&f);
        assert_eq!(
            reenabled.try_inherit_state_from(&mut disabled),
            Err(SessionPlaybackError::MutableInstrumentActive)
        );
        assert_eq!(
            reenabled.try_inherit_state_live_from(&mut disabled),
            Err(SessionPlaybackError::DeviceTransitionBusy),
            "live publication must wait for the original power fade"
        );
        let mut actual = [[0.0; 2]; 64];
        let mut expected = actual;
        disabled.render(&mut actual).unwrap();
        reference.render(&mut expected).unwrap();
        apply_power_fade(&mut expected, 0, 40);
        assert_eq!(
            actual, expected,
            "refusal preserves the only original tail owner"
        );
        assert!(
            actual[..8]
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.001)
        );
        assert_eq!(actual[40..], [[0.0; 2]; 24]);
        reenabled
            .try_inherit_state_live_from(&mut disabled)
            .unwrap();
        let mut settled = [[0.0; 2]; 32];
        reenabled.render(&mut settled).unwrap();
        assert_eq!(
            settled, [[0.0; 2]; 32],
            "the old held note must not be chased"
        );
    });
}

fn long_sampler_release(f: &mut Fixture) -> hex_project::DeviceId {
    let instrument = f
        .editor
        .project()
        .track(f.receiver)
        .unwrap()
        .instrument()
        .unwrap();
    let mut state =
        hex_devices::sampler::state::SamplerState::from_native(instrument.state()).unwrap();
    state.envelope.release_seconds = 0.1;
    let state = state
        .to_native()
        .unwrap()
        .with_asset_references(instrument.state().asset_references().to_vec())
        .unwrap();
    let device = instrument.id();
    f.editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    device
}

fn apply_power_fade(pcm: &mut [Frame], elapsed: usize, fade: usize) {
    for (offset, frame) in pcm.iter_mut().enumerate() {
        let gain = fade.saturating_sub(elapsed + offset) as f32 / fade as f32;
        *frame = frame.map(|sample| sample * gain);
    }
}

#[test]
fn shared_mutable_power_off_fades_the_held_waveform_and_starts_fresh() {
    on_control(|| {
        let mut f = fixture();
        let device = long_sampler_release(&mut f);
        check_mutable_power_restart(&mut f, device);
    });
}

fn check_mutable_power_restart(f: &mut Fixture, device: hex_project::DeviceId) {
    const FADE: usize = 40;
    let mut previous = playback(f);
    let mut reference = playback(f);
    for runtime in [&mut previous, &mut reference] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 7]).unwrap();
    }
    f.editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let mut disabled = playback(f);
    disabled.try_inherit_state_from(&mut previous).unwrap();

    let mut actual = [[0.0; 2]; 64];
    let mut expected = actual;
    disabled.render(&mut actual[..13]).unwrap();

    // An ordinary following revision carries the remaining fade only.
    let mut successor = playback(f);
    successor
        .try_inherit_state_live_from(&mut disabled)
        .unwrap();
    f.editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: false,
        })
        .unwrap();
    let mut enabled = playback(f);
    assert_eq!(
        enabled.try_inherit_state_live_from(&mut successor),
        Err(SessionPlaybackError::DeviceTransitionBusy),
    );
    successor.render(&mut actual[13..]).unwrap();
    reference.render(&mut expected).unwrap();
    assert!(
        actual[FADE..].iter().all(|frame| *frame == [0.0; 2]),
        "old mutable DSP rendered past the 5 ms power-off deadline"
    );
    assert_ne!(expected[0], [0.0; 2]);
    apply_power_fade(&mut expected, 0, FADE);
    for (actual, expected) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert!((actual - expected).abs() < 0.000_001);
    }
    // The provider would still render the held note. The host has ended
    // its authority and keeps the allocation parked for control retirement.
    let tail = successor
        .tail_mutable_instruments
        .iter()
        .flatten()
        .next()
        .unwrap();
    assert!(!tail.runtime.committed_is_quiescent());
    assert!(tail.committed_is_quiescent());

    enabled.try_inherit_state_live_from(&mut successor).unwrap();
    enabled.render(&mut actual).unwrap();
    assert_eq!(
        actual, [[0.0; 2]; 64],
        "the preceding note onset stays silent"
    );
    assert!(
        enabled
            .mutable_instruments
            .iter()
            .flatten()
            .all(|runtime| runtime.committed_is_quiescent())
    );
    assert!(enabled.tail_mutable_instruments.iter().all(Option::is_none));
}

#[test]
fn power_fade_enters_the_live_stateful_receiver_once() {
    on_control(|| {
        let mut f = fixture();
        let instrument = long_sampler_release(&mut f);
        let Created::Device(gate) = f
            .editor
            .execute(Command::InsertDevice {
                track: f.receiver,
                before: None,
                state: crate::default_native_device_state(hex_devices::gate::GATE_TYPE_ID).unwrap(),
            })
            .unwrap()
        else {
            panic!("Gate");
        };
        let mut previous = playback(&f);
        let mut reference = playback(&f);
        for runtime in [&mut previous, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 129]).unwrap();
        }
        let observed = |runtime: &mut SessionPlayback| {
            let mut input = [[0.0; 2]; 64];
            let mut count = 0;
            let mut output = [[0.0; 2]; 64];
            runtime
                .render_with_node_signal_observer(
                    &mut output,
                    Some(&mut |node, phase, offset, frames| {
                        if node.device_id == gate.value() && phase == crate::NodeSignalPhase::Input
                        {
                            assert_eq!(offset, count, "one active receiver processor");
                            count += frames.len();
                            input[offset..count].copy_from_slice(frames);
                        }
                    }),
                )
                .unwrap();
            assert_eq!(count, input.len());
            (input, output)
        };
        f.editor
            .execute(Command::SetDeviceBypass {
                device: instrument,
                bypassed: true,
            })
            .unwrap();
        let mut disabled = playback(&f);
        disabled.try_inherit_state_from(&mut previous).unwrap();
        let (mut expected, uninterrupted) = observed(&mut reference);
        assert_ne!(expected[0], [0.0; 2]);
        apply_power_fade(&mut expected, 0, 40);
        let (actual, output) = observed(&mut disabled);
        assert_eq!(
            actual, expected,
            "the live Gate consumes the bounded source fade"
        );
        assert_eq!(
            output[0], uninterrupted[0],
            "receiver detector history survives the cut"
        );
        assert!(
            disabled
                .tail_groups
                .iter()
                .flatten()
                .all(|chain| chain.is_empty())
        );

        f.editor
            .execute(Command::SetDeviceBypass {
                device: instrument,
                bypassed: false,
            })
            .unwrap();
        let mut enabled = playback(&f);
        enabled.try_inherit_state_live_from(&mut disabled).unwrap();
        let (input, _) = observed(&mut enabled);
        assert_eq!(
            input, [[0.0; 2]; 64],
            "fresh kernel cannot replay the old note"
        );
    });
}

fn source_owned_effects(
    f: &mut Fixture,
    effects: impl IntoIterator<Item = hex_lattice_contract::NativeDeviceState>,
) -> (hex_project::DeviceId, Vec<hex_project::DeviceId>) {
    use hex_lattice_contract::NativeDeviceMetadata;
    let instrument = f
        .editor
        .project()
        .native_source(f.source)
        .unwrap()
        .instrument()
        .id();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: hex_devices::fm4::device::FM4_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    for track in [f.receiver, f.contributor] {
        f.editor
            .execute(Command::BindTrackSource {
                track,
                source: None,
            })
            .unwrap();
    }
    let ids = effects
        .into_iter()
        .map(|state| {
            let Created::Device(device) = f
                .editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state,
                })
                .unwrap()
            else {
                panic!("source effect");
            };
            device
        })
        .collect();
    for track in [f.receiver, f.contributor] {
        f.editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(f.source),
            })
            .unwrap();
    }
    (instrument, ids)
}

#[test]
fn source_owned_gate_keeps_its_single_path_through_mutable_power_cycles() {
    on_control(|| {
        let mut f = fixture();
        let gate = crate::default_native_device_state(hex_devices::gate::GATE_TYPE_ID).unwrap();
        let (instrument, _) = source_owned_effects(&mut f, [gate]);
        let mut previous = playback(&f);
        let mut reference = playback(&f);
        assert!(previous.source_effects.has_chain(0));
        for runtime in [&mut previous, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 129]).unwrap();
        }
        let mut uninterrupted = [[0.0; 2]; 1];
        reference.render(&mut uninterrupted).unwrap();
        assert_ne!(uninterrupted[0], [0.0; 2]);

        f.editor
            .execute(Command::SetDeviceBypass {
                device: instrument,
                bypassed: true,
            })
            .unwrap();
        let mut disabled = playback(&f);
        disabled.try_inherit_state_live_from(&mut previous).unwrap();
        assert!(disabled.receiver_power_off_slots[0].is_some());
        let mut output = [[0.0; 2]; 64];
        disabled.render(&mut output[..13]).unwrap();
        let mut successor = playback(&f);
        successor
            .try_inherit_state_live_from(&mut disabled)
            .unwrap();

        f.editor
            .execute(Command::SetDeviceBypass {
                device: instrument,
                bypassed: false,
            })
            .unwrap();
        let mut enabled = playback(&f);
        assert_eq!(
            enabled.try_inherit_state_live_from(&mut successor),
            Err(SessionPlaybackError::DeviceTransitionBusy)
        );
        successor.render(&mut output[13..]).unwrap();
        assert_eq!(output[0], uninterrupted[0]);
        assert_eq!(output[40..], [[0.0; 2]; 24]);
        assert!(
            successor
                .tail_groups
                .iter()
                .flatten()
                .all(|chain| chain.is_empty())
        );
        enabled.try_inherit_state_live_from(&mut successor).unwrap();
        enabled.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 64]);
    });
}

#[test]
fn source_owned_removal_fades_once_and_keeps_the_held_generator() {
    use crate::session_playback::NativeRemovalProgress;
    use hex_devices::delay::DEFINITION;
    use hex_lattice_contract::NativeDeviceMetadata;
    on_control(|| {
        let delay = DEFINITION.default_state().unwrap();
        for surviving_gate in [false, true] {
            let mut f = fixture();
            let gate = crate::default_native_device_state(hex_devices::gate::GATE_TYPE_ID).unwrap();
            let (_, devices) = source_owned_effects(
                &mut f,
                surviving_gate
                    .then_some(gate)
                    .into_iter()
                    .chain([delay.clone()]),
            );
            let mut old = playback(&f);
            let mut wet = playback(&f);
            f.editor
                .execute(Command::RemoveDevice(*devices.last().unwrap()))
                .unwrap();
            let mut next = playback(&f);
            let mut dry = playback(&f);
            for runtime in [&mut old, &mut wet, &mut dry] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime.render(&mut [[0.0; 2]; 129]).unwrap();
            }
            next.require_continuous_insert = true;
            assert_eq!(
                next.advance_native_removal_from(&mut old),
                Ok(NativeRemovalProgress::Fading)
            );
            assert_eq!(
                next.try_inherit_state_live_from(&mut old),
                Err(SessionPlaybackError::DeviceTransitionBusy)
            );
            let mut dry_pcm = [[0.0; 2]; 56];
            let mut wet_pcm = dry_pcm;
            let mut actual = dry_pcm;
            dry.render(&mut dry_pcm).unwrap();
            wet.render(&mut wet_pcm).unwrap();
            assert!(dry_pcm.iter().any(|sample| sample[0].abs() > 0.01));
            assert_ne!(wet_pcm, dry_pcm);
            old.render(&mut actual[..11]).unwrap();
            old.render(&mut actual[11..39]).unwrap();
            assert_eq!(
                next.advance_native_removal_from(&mut old),
                Ok(NativeRemovalProgress::Fading)
            );
            old.render(&mut actual[39..40]).unwrap();
            assert_eq!(
                next.advance_native_removal_from(&mut old),
                Ok(NativeRemovalProgress::Ready)
            );
            let report = next.try_inherit_state_live_from(&mut old).unwrap();
            assert!(!report.running_audio_restart);
            assert_eq!(next.performance_frame(), 169);
            assert_eq!(next.source_effects.has_chain(0), surviving_gate);
            next.render(&mut actual[40..]).unwrap();
            for (index, actual) in actual.iter().enumerate() {
                let elapsed = (index + 1).min(40) as f32 / 40.0;
                for channel in 0..2 {
                    let dry = dry_pcm[index][channel];
                    let wet = wet_pcm[index][channel];
                    let expected = dry + (wet - dry) * (1.0 - elapsed);
                    assert!(
                        (actual[channel] - expected).abs() < 1e-6,
                        "{index} {channel}"
                    );
                }
            }
        }
    });
}

#[test]
fn static_utility_receiver_preserves_its_processing_during_power_fade() {
    on_control(|| {
        use hex_devices::utility::{UTILITY_DEFINITION as utility, parameter as p};
        use hex_lattice_contract::ParameterValue;
        for case in 0..3 {
            let mut f = fixture();
            let device = long_sampler_release(&mut f);
            let mut state = utility.default_state().unwrap();
            if case == 1 {
                state = utility
                    .patch_parameter(&state, p::BALANCE, ParameterValue::Float(0.5))
                    .unwrap();
                state = utility
                    .patch_parameter(&state, p::WIDTH, ParameterValue::Float(2.0))
                    .unwrap();
            } else if case == 2 {
                state = utility
                    .patch_parameter(&state, p::INVERT_LEFT, ParameterValue::Bool(true))
                    .unwrap();
            }
            f.editor
                .execute(Command::InsertDevice {
                    track: f.receiver,
                    before: None,
                    state,
                })
                .unwrap();
            if case == 2 {
                let state = utility
                    .patch_parameter(
                        &utility.default_state().unwrap(),
                        p::WIDTH,
                        ParameterValue::Float(0.5),
                    )
                    .unwrap();
                f.editor
                    .execute(Command::InsertDevice {
                        track: f.receiver,
                        before: None,
                        state,
                    })
                    .unwrap();
            }
            check_mutable_power_restart(&mut f, device);
        }
    });
}

#[test]
fn rapid_shared_analytic_reenable_keeps_the_first_power_off_deadline() {
    on_control(|| {
        // A rate whose 5 ms deadline needs rounding to a whole frame.
        let mut f = fixture_at(44_100);
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
        assert!(playback(&f).mutable_instruments.iter().all(Option::is_none));
        let maximum_tail = f.editor.project().sample_rate().div_ceil(200) as usize;
        check_rapid_source_toggles(&mut f, device, maximum_tail);
    });
}

fn check_rapid_source_toggles(f: &mut Fixture, device: hex_project::DeviceId, maximum_tail: usize) {
    const BLOCK: usize = 13;
    const TOGGLES: [bool; 4] = [true, false, true, false];
    let mut current = playback(f);
    let mut reference = playback(f);
    let mut held = playback(f);
    for runtime in [&mut current, &mut reference, &mut held] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 129]).unwrap();
    }
    let mut boundary = [[0.0; 2]; 1];
    held.render(&mut boundary).unwrap();
    for (turn, bypassed) in TOGGLES.into_iter().enumerate() {
        f.editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let mut candidate = playback(f);
        candidate.try_inherit_state_live_from(&mut current).unwrap();
        if turn == 0 {
            let mut disabled = playback(f);
            disabled.try_inherit_state_from(&mut reference).unwrap();
            reference = disabled;
        }
        let mut actual = [[0.0; 2]; BLOCK];
        let mut expected = actual;
        candidate.render(&mut actual).unwrap();
        reference.render(&mut expected).unwrap();
        if turn == 0 {
            assert_eq!(actual[0], boundary[0], "bypass preserves every contributor");
        }
        assert!(
            expected.iter().flatten().any(|sample| sample.abs() > 0.001),
            "the original release must be audible on turn {turn}"
        );
        assert_eq!(actual, expected, "bypassed={bypassed}");
        current = candidate;
    }
    let mut settled = [[1.0; 2]; 256];
    let mut expected = settled;
    current.render(&mut settled).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(settled, expected, "rapid toggles cannot restart the fade");
    assert!(
        settled[maximum_tail - BLOCK * TOGGLES.len()..]
            .iter()
            .all(|frame| *frame == [0.0; 2]),
        "the original receiver cut sets the power-off deadline"
    );
    for _ in 0..4 {
        current.render(&mut settled).unwrap();
    }
    assert_eq!(settled, [[0.0; 2]; 256], "no held note was chased");
}

#[test]
fn completed_one_shot_keeps_past_notes_silent_across_live_and_receiver_handoffs() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
            gate: hex_devices::sampler::GateMode::Trigger,
        };
        let state = state.replace_in(device.state()).unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state,
            })
            .unwrap();
        let clip = f.editor.project().track(f.receiver).unwrap().midi_clips()[0].id();
        let future = f.editor.project().tempo_map().frame_to_beat(16).unwrap();
        assert_eq!(
            f.editor
                .project()
                .tempo_map()
                .beat_to_frame(future)
                .unwrap(),
            16
        );
        f.editor
            .execute(Command::AddMidiNote {
                clip,
                start: future,
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                pitch: 72,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut old, _) = live_playback(&f, &input);
        old.apply(PlaybackCommand::Play);
        let mut prefix = [[0.0; 2]; 8];
        old.render(&mut prefix).unwrap();
        assert!(prefix[..4].iter().flatten().any(|value| value.abs() > 0.01));
        assert_eq!(prefix[4..], [[0.0; 2]; 4]);
        let (mut same, _) = live_playback(&f, &input);
        same.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        let mut between = [[1.0; 2]; 4];
        same.render(&mut between).unwrap();
        assert_eq!(
            between, [[0.0; 2]; 4],
            "completed authored/live notes cannot restart"
        );
        f.editor
            .execute(Command::SetNativeSourceReceiver {
                source: f.source,
                track: Some(f.contributor),
            })
            .unwrap();
        let oracle_input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        oracle_input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut oracle, _) = live_playback(&f, &oracle_input);
        oracle.apply(PlaybackCommand::Play);
        oracle.render(&mut [[0.0; 2]; 12]).unwrap();
        let (mut moved, _) = live_playback(&f, &input);
        moved
            .try_inherit_runtime_in_control_context(&mut same, Some(19), Some(19))
            .unwrap();
        let mut actual = [[0.0; 2]; 32];
        let mut expected = actual;
        moved.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            actual, expected,
            "receiver move matches uninterrupted one-shot history"
        );
        assert_eq!(actual[..4], [[0.0; 2]; 4]);
        assert!(
            actual[4..8]
                .iter()
                .flatten()
                .any(|value| value.abs() > 0.01),
            "future onset still sounds"
        );
    });
}

#[test]
fn ended_one_shot_cannot_chase_authored_or_held_input_into_analytic_class() {
    on_control(|| {
        for live_only in [false, true] {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap();
            let mut state =
                hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
            state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
                gate: hex_devices::sampler::GateMode::Trigger,
            };
            let state = state.replace_in(device.state()).unwrap();
            let device = device.id();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            if live_only {
                for track in [f.receiver, f.contributor] {
                    let clip = f.editor.project().track(track).unwrap().midi_clips()[0].id();
                    f.editor.execute(Command::RemoveMidiClip(clip)).unwrap();
                }
                let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
                input.set_key(0, Some((60, 127, 1))).unwrap();
                let (mut old, _) = live_playback(&f, &input);
                old.apply(crate::playback::PlaybackCommand::Play);
                let mut prefix = [[0.0; 2]; 8];
                old.render(&mut prefix).unwrap();
                assert!(prefix[..4].iter().flatten().any(|value| value.abs() > 0.01));
                assert_eq!(prefix[4..], [[0.0; 2]; 4]);
                f.editor
                    .execute(Command::ReplaceDeviceState {
                        device,
                        state: hex_project::default_instrument_state().unwrap(),
                    })
                    .unwrap();
                // The live handoff restarts audio instead of refusing
                // (Pulse #3784). Releasing the held key leaves silence, and
                // a real new onset still plays on the new instrument.
                let (mut next, _) = live_playback(&f, &input);
                next.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                    .unwrap();
                input.set_key(0, None).unwrap();
                let mut drained = vec![[1.0; 2]; 16_064];
                for chunk in drained.chunks_mut(17) {
                    next.render(chunk).unwrap();
                }
                assert!(
                    drained[16_000..].iter().all(|frame| *frame == [0.0; 2]),
                    "no stuck note after the held key is released"
                );
                input.set_key(0, Some((60, 127, 2))).unwrap();
                let mut future = [[0.0; 2]; 32];
                next.render(&mut future).unwrap();
                assert!(
                    future.iter().flatten().any(|sample| sample.abs() > 0.001),
                    "a real new input onset remains available"
                );
            } else {
                let mut old = playback(&f);
                old.apply(SessionPlaybackCommand::Play).unwrap();
                let mut prefix = [[0.0; 2]; 8];
                old.render(&mut prefix).unwrap();
                assert!(prefix[..4].iter().flatten().any(|value| value.abs() > 0.01));
                assert_eq!(prefix[4..], [[0.0; 2]; 4]);
                f.editor
                    .execute(Command::ReplaceDeviceState {
                        device,
                        state: hex_project::default_instrument_state().unwrap(),
                    })
                    .unwrap();
                let mut refused = playback(&f);
                assert_eq!(
                    refused.try_inherit_state_from(&mut old),
                    Err(SessionPlaybackError::SourceHistoryUnavailable)
                );
                let mut quiet = [[1.0; 2]; 8];
                old.render(&mut quiet).unwrap();
                assert_eq!(
                    quiet, [[0.0; 2]; 8],
                    "canonical held gate cannot become an analytic chased note"
                );
            }
        }
    });
}

fn playback(f: &Fixture) -> SessionPlayback {
    playback_with_tuning(f, None)
}
fn playback_with_tuning(
    f: &Fixture,
    previous: Option<(&hex_project::Project, &PreparedRevision)>,
) -> SessionPlayback {
    let cancel = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &f.store,
        f.editor.project(),
        20_000_000,
        &cancel,
    )
    .unwrap();
    let revision = PreparedRevision::prepare_with_device_resources(
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
    .unwrap();
    let revision = match previous {
        Some((project, old)) => revision
            .with_tuning_continuation_from(f.editor.project(), project, old)
            .unwrap(),
        None => revision,
    };
    SessionPlayback::new_with_device_resources(Arc::new(revision), &resources).unwrap()
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
/// Prepared gated release, in 8 kHz output frames, of the fixture sampler
/// with `release_seconds`. Oracles derive from it so a host declick floor
/// only lengthens the release they expect.
fn release_frames(release_seconds: f64) -> u64 {
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    state.envelope.release_seconds = release_seconds;
    hex_devices::sampler::PreparedSamplerPlan::new(
        hex_devices::sampler::SampleShape::new(
            4,
            8000,
            hex_devices::sampler::SampleChannels::Stereo,
        )
        .unwrap(),
        8000,
        state.settings(),
    )
    .unwrap()
    .prepare_voice_coefficients(60, 127)
    .unwrap()
    .unwrap()
    .release_frames()
}
/// Linear release level `steps` frames after note-off.
fn release_level(steps: usize, release: u64) -> f32 {
    let steps = steps as u64;
    if steps >= release {
        0.0
    } else {
        1.0 - steps as f32 / release as f32
    }
}
/// Like `check_pcm`, plus one same-phase note released at `start` with
/// `released` gain over the fixture's zero-authored-release envelope.
fn check_released_pcm(
    playback: &mut SessionPlayback,
    start: usize,
    count: usize,
    held: f32,
    released: f32,
) {
    let release = release_frames(0.0);
    let mut output = vec![[0.0; 2]; count];
    for chunk in output.chunks_mut(3) {
        playback.render(chunk).unwrap();
    }
    for (index, frame) in output.iter().enumerate() {
        let gain = held + released * release_level(index, release);
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
fn exact_shared_handoff_preserves_held_and_released_pcm() {
    on_control(|| {
        let f = fixture();
        let mut previous = playback(&f);
        let mut oracle = playback(&f);
        for runtime in [&mut previous, &mut oracle] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut vec![[0.0; 2]; 3999]).unwrap();
        }
        let mut next = playback(&f);
        next.try_inherit_state_from(&mut previous).unwrap();
        assert_eq!(next.mutable_instruments.iter().flatten().count(), 1);
        for frames in [1, 257] {
            let mut actual = vec![[0.0; 2]; frames];
            let mut expected = actual.clone();
            next.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
            assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
        }
    });
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
fn pause_after_removing_all_receivers_keeps_the_detached_source_release() {
    on_control(|| {
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let device_id = device.id();
        let original = device.state().clone();
        let mut authored =
            hex_devices::sampler::state::SamplerState::from_native(&original).unwrap();
        authored.envelope.release_seconds = 4.0 / 8000.0;
        let state = authored
            .to_native()
            .unwrap()
            .with_asset_references(original.asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device_id,
                state,
            })
            .unwrap();
        let mut old = started(&f);
        let mut oracle_old = started(&f);
        old.apply(SessionPlaybackCommand::Pause).unwrap();
        oracle_old.apply(SessionPlaybackCommand::Pause).unwrap();
        for track in [f.contributor, f.receiver] {
            f.editor.execute(Command::RemoveTrack(track)).unwrap();
        }
        let mut actual = playback(&f);
        let mut oracle = playback(&f);
        actual.try_inherit_state_from(&mut old).unwrap();
        oracle.try_inherit_state_from(&mut oracle_old).unwrap();
        assert!(actual.revision.arrangement().tracks().is_empty());
        actual.apply(SessionPlaybackCommand::Play).unwrap();
        actual.apply(SessionPlaybackCommand::Pause).unwrap();
        // The detached release lasts the prepared release, then stays quiet.
        let quiet_from = release_frames(4.0 / 8000.0) as usize + 4;
        let mut expected = vec![[0.0; 2]; quiet_from + 8];
        let mut output = expected.clone();
        oracle.render(&mut expected).unwrap();
        for chunk in output.chunks_mut(3) {
            actual.render(chunk).unwrap();
        }
        assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
        assert_eq!(output, expected);
        assert!(output[quiet_from..].iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(actual.performance_frame, 7);
        assert_eq!(actual.arrangement_frame, 7);
    });
}

#[test]
fn shared_sampler_scalar_retarget_preserves_ages_and_owns_no_outgoing_kernel() {
    on_control(|| {
        let mut f = fixture();
        let mut old = started(&f);
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let original = f.editor.project().device(device).unwrap().state().clone();
        let mut settings =
            hex_devices::sampler::state::SamplerState::from_native(&original).unwrap();
        settings.gain = 0.5;
        f.editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: settings.replace_in(&original).unwrap(),
            })
            .unwrap();
        let before = old.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .occurrence_proofs()
            .to_vec();
        let mut next = playback(&f);
        let inventory = next.revision.session();
        let factory = inventory.mutable_instrument_factory(f.receiver).unwrap();
        let proof = inventory.mutable_history_retarget(f.receiver).unwrap();
        assert_eq!(
            inventory.resource_factory_inline_bytes().unwrap(),
            std::mem::size_of_val(factory) + std::mem::size_of_val(proof)
        );
        next.preview_stream_handoff(&mut old, 32, None, &mut |_| Ok(()))
            .unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(
            next.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .occurrence_proofs(),
            before
        );
        assert!(next.tail_mutable_instruments.iter().all(Option::is_none));
        check_pcm(&mut next, 7, 5, 0.25);

        // The new envelope is evaluated at each actual voice's existing
        // age; recreating an onset would incorrectly restart at zero.
        let current = f.editor.project().device(device).unwrap().state().clone();
        settings.envelope.attack_seconds = 0.002; // sixteen frames at 8 kHz
        f.editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: settings.replace_in(&current).unwrap(),
            })
            .unwrap();
        let mut attacked = playback(&f);
        attacked.try_inherit_state_from(&mut next).unwrap();
        assert_eq!(attacked.mutable_instruments.iter().flatten().count(), 1);
        assert!(
            attacked
                .tail_mutable_instruments
                .iter()
                .all(Option::is_none)
        );
        let mut output = [[0.0; 2]; 8];
        attacked.render(&mut output).unwrap();
        for (i, frame) in output.iter().enumerate() {
            let gain = 0.25 * ((12 + i) as f32 / 16.0).min(1.0);
            for c in 0..2 {
                assert!((frame[c] - PCM[(12 + i) % 4][c] * gain).abs() < 2e-6);
            }
        }
        f.editor
            .execute(Command::BindTrackSource {
                track: f.contributor,
                source: None,
            })
            .unwrap();
        let mut detached = playback(&f);
        detached.try_inherit_state_from(&mut attacked).unwrap();
        assert!(
            detached
                .tail_mutable_instruments
                .iter()
                .all(Option::is_none)
        );
        // The detached contributor's note releases from frame20 while the
        // receiver's own note holds.
        check_released_pcm(&mut detached, 20, 8, 0.125, 0.125);
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
            check_released_pcm(&mut candidate, 7, 19, 0.25, 0.25);
        }
    });
}
#[test]
fn shared_mute_matches_authored_oracle_at_8khz() {
    on_control(|| check_shared_mute_partitions(8_000));
}
#[test]
fn shared_mute_matches_authored_oracle_at_441khz() {
    on_control(|| check_shared_mute_partitions(44_100));
}
#[test]
fn shared_mute_matches_authored_oracle_at_96khz() {
    on_control(|| check_shared_mute_partitions(96_000));
}
fn check_shared_mute_partitions(rate: u32) {
    let duration = rate.div_ceil(200) as usize;
    for partition in [1, 7, 64, 128, 256, 512] {
        let mut f = fixture_at(rate);
        let mut old = started(&f);
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut muted = playback(&f);
        muted.try_inherit_state_from(&mut old).unwrap();
        let mut output = vec![[0.0; 2]; duration + 1];
        for chunk in output.chunks_mut(partition) {
            muted.render(chunk).unwrap();
        }
        for (index, frame) in output.iter().enumerate() {
            let factor = 1.0 - index.min(duration) as f32 / duration as f32;
            for channel in 0..2 {
                let expected = PCM[(7 + index) % 4][channel] * (0.25 + 0.25 * factor);
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "rate={rate} partition={partition} frame={index}: {frame:?} expected={expected}"
                );
            }
        }
        assert!(f.editor.undo());
        let mut restored = playback(&f);
        restored.try_inherit_state_from(&mut muted).unwrap();
        let mut output = vec![[0.0; 2]; duration + 1];
        for chunk in output.chunks_mut(partition) {
            restored.render(chunk).unwrap();
        }
        for (index, frame) in output.iter().enumerate() {
            let factor = index.min(duration) as f32 / duration as f32;
            for channel in 0..2 {
                let expected = PCM[(8 + duration + index) % 4][channel] * (0.25 + 0.25 * factor);
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "restore rate={rate} partition={partition} frame={index}: {frame:?} expected={expected}"
                );
            }
        }
    }
}

#[test]
fn shared_mute_multiplies_the_running_clip_volume_curve() {
    on_control(|| {
        use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
        let mut f = fixture();
        let clip = f
            .editor
            .project()
            .track(f.contributor)
            .unwrap()
            .midi_clips()[0]
            .id();
        let source_length = f
            .editor
            .project()
            .midi_source(clip)
            .unwrap()
            .source_length();
        let volume = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(BeatTime::ONE, MidiClipGain::new(0.75).unwrap()),
            ],
            source_length,
        )
        .unwrap();
        f.editor
            .execute(Command::ReplaceMidiClipVolume { clip, volume })
            .unwrap();
        let mut old = playback(&f);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut muted = playback(&f);
        muted.try_inherit_state_from(&mut old).unwrap();
        let mut output = [[0.0; 2]; 40];
        for chunk in output.chunks_mut(7) {
            muted.render(chunk).unwrap();
        }
        for (index, frame) in output.iter().enumerate() {
            let clip_gain = 0.25 + 0.5 * (7 + index) as f64 / 4000.0;
            let fade = 1.0 - index as f64 / 40.0;
            for channel in 0..2 {
                let expected =
                    f64::from(PCM[(7 + index) % 4][channel]) * (0.25 + 0.25 * clip_gain * fade);
                assert!(
                    (f64::from(frame[channel]) - expected).abs() < 0.000_002,
                    "frame={index} output={frame:?} expected={expected}"
                );
            }
        }
    });
}

#[test]
fn muted_authored_voice_retires_at_natural_gate_and_seek_restarts_cleanly() {
    on_control(|| {
        let mut f = fixture();
        let mut old = started(&f);
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut muted = playback(&f);
        muted.try_inherit_state_from(&mut old).unwrap();
        let mut elapsed = [[0.0; 2]; 4096];
        for chunk in elapsed.chunks_mut(97) {
            muted.render(chunk).unwrap();
        }
        assert!(elapsed[4020..].iter().all(|frame| *frame == [0.0; 2]));
        assert!(f.editor.undo());
        let mut restored = playback(&f);
        restored.try_inherit_state_from(&mut muted).unwrap();
        let mut after = [[1.0; 2]; 32];
        restored.render(&mut after).unwrap();
        assert_eq!(
            after, [[0.0; 2]; 32],
            "unmute must not revive an ended gate"
        );
        restored.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        check_pcm(&mut restored, 0, 8, 0.5);
        restored.apply(SessionPlaybackCommand::Stop).unwrap();
        restored.render(&mut [[0.0; 2]; 64]).unwrap();
        restored.render(&mut after).unwrap();
        assert_eq!(after, [[0.0; 2]; 32]);
    });
}

#[test]
fn changed_midi_content_uses_existing_release_instead_of_retaining_stale_voice() {
    on_control(|| {
        let mut f = fixture();
        let mut old = started(&f);
        let clip = f
            .editor
            .project()
            .track(f.contributor)
            .unwrap()
            .midi_clips()[0]
            .id();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        f.editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 62,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        let mut candidate = playback(&f);
        candidate.try_inherit_state_from(&mut old).unwrap();
        check_released_pcm(&mut candidate, 7, 19, 0.25, 0.25);
    });
}

#[test]
fn shared_authored_note_started_muted_resumes_when_unmuted() {
    on_control(|| {
        let mut f = fixture();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut old = playback(&f);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        check_pcm(&mut old, 0, 7, 0.25);
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let mut next = playback(&f);
        next.try_inherit_state_from(&mut old).unwrap();
        let mut frames = [[0.0; 2]; 160];
        for chunk in frames.chunks_mut(7) {
            next.render(chunk).unwrap();
        }
        for (i, frame) in frames.iter().enumerate() {
            let fade = i.min(40) as f32 / 40.0;
            for channel in 0..2 {
                let expected = 0.25 * (PCM[(7 + i) % 4][channel] + PCM[i % 4][channel] * fade);
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "frame {i} channel {channel}: got {} expected {expected}",
                    frame[channel]
                );
            }
        }
        let mut tail = [[0.0; 2]; 4096];
        for chunk in tail.chunks_mut(97) {
            next.render(chunk).unwrap();
        }
        assert!(tail[4020..].iter().all(|frame| *frame == [0.0; 2]));
    });
}

#[test]
fn pending_shared_unmute_survives_empty_render_and_refuses_another_handoff() {
    on_control(|| {
        let mut f = fixture();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut old = playback(&f);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        check_pcm(&mut old, 0, 7, 0.25);
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let mut next = playback(&f);
        next.try_inherit_state_from(&mut old).unwrap();
        next.render(&mut []).unwrap();
        let mut another = playback(&f);
        assert!(matches!(
            another.try_inherit_state_from(&mut next),
            Err(SessionPlaybackError::MutableInstrumentChase)
        ));
        let mut frames = [[0.0; 2]; 41];
        next.render(&mut frames).unwrap();
        for (i, frame) in frames.iter().enumerate() {
            let fade = i as f32 / 40.0;
            for channel in 0..2 {
                let expected = 0.25 * (PCM[(7 + i) % 4][channel] + PCM[i % 4][channel] * fade);
                assert!((frame[channel] - expected).abs() < 0.000_002);
            }
        }
        another.try_inherit_state_from(&mut next).unwrap();
    });
}

#[test]
fn pending_shared_unmute_clears_on_stop_and_seek() {
    on_control(|| {
        for seek in [false, true] {
            let mut f = fixture();
            f.editor
                .execute(Command::Mute {
                    track: f.contributor,
                    muted: true,
                })
                .unwrap();
            let mut old = playback(&f);
            old.apply(SessionPlaybackCommand::Play).unwrap();
            check_pcm(&mut old, 0, 7, 0.25);
            f.editor
                .execute(Command::Mute {
                    track: f.contributor,
                    muted: false,
                })
                .unwrap();
            let mut next = playback(&f);
            next.try_inherit_state_from(&mut old).unwrap();
            if seek {
                assert!(matches!(
                    next.apply(SessionPlaybackCommand::Seek(0)),
                    Err(SessionPlaybackError::MutableInstrumentActive)
                ));
                assert!(next.pending_shared_chase.is_some());
                next.apply(SessionPlaybackCommand::Stop).unwrap();
                next.apply(SessionPlaybackCommand::Seek(0)).unwrap();
                next.apply(SessionPlaybackCommand::Play).unwrap();
                check_pcm(&mut next, 0, 8, 0.5);
            } else {
                next.apply(SessionPlaybackCommand::Stop).unwrap();
                next.render(&mut [[0.0; 2]; 64]).unwrap();
                let mut silent = [[1.0; 2]; 8];
                next.render(&mut silent).unwrap();
                assert_eq!(silent, [[0.0; 2]; 8]);
            }
        }
    });
}

#[test]
fn shared_unmute_refuses_multiple_missing_notes_and_active_loop() {
    on_control(|| {
        for looped in [false, true] {
            let mut f = fixture();
            if looped {
                f.editor
                    .execute(Command::SetLoopRegion {
                        start: 0,
                        end: 512,
                        enabled: true,
                    })
                    .unwrap();
            } else {
                let clip = f
                    .editor
                    .project()
                    .track(f.contributor)
                    .unwrap()
                    .midi_clips()[0]
                    .id();
                f.editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch: 62,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
            f.editor
                .execute(Command::Mute {
                    track: f.contributor,
                    muted: true,
                })
                .unwrap();
            let mut old = playback(&f);
            old.apply(SessionPlaybackCommand::Play).unwrap();
            check_pcm(&mut old, 0, 7, 0.25);
            f.editor
                .execute(Command::Mute {
                    track: f.contributor,
                    muted: false,
                })
                .unwrap();
            let mut next = playback(&f);
            assert!(matches!(
                next.try_inherit_state_from(&mut old),
                Err(SessionPlaybackError::MutableInstrumentChase)
            ));
            check_pcm(&mut old, 7, 8, 0.25);
        }
    });
}

#[test]
fn failed_shared_unmute_render_retries_the_same_fade() {
    on_control(|| {
        let mut f = fixture();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut old = playback(&f);
        let mut reference_old = playback(&f);
        for playback in [&mut old, &mut reference_old] {
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            check_pcm(playback, 0, 7, 0.25);
        }
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let mut failed = playback(&f);
        let mut reference = playback(&f);
        failed.try_inherit_state_from(&mut old).unwrap();
        reference
            .try_inherit_state_from(&mut reference_old)
            .unwrap();
        let route = failed
            .routes
            .iter()
            .position(|route| route.id == f.contributor)
            .unwrap();
        failed.routes[route].current_incarnation = u64::MAX;
        failed.routes[route].pending = Some(crate::session_playback::PendingRoute {
            note_chase: crate::NoteChase::On,
            frame: 39,
            beat: BeatTime::ZERO,
            target: crate::session_playback::RouteTarget::Silence,
            current_reservation: None,
        });
        let mut rejected = [[1.0; 2]; 64];
        assert_eq!(
            failed.render(&mut rejected),
            Err(SessionPlaybackError::ArithmeticOverflow)
        );
        assert_eq!(rejected, [[0.0; 2]; 64]);
        failed.routes[route].pending = None;
        failed.routes[route].current_incarnation = reference.routes[route].current_incarnation;
        let mut retried = [[0.0; 2]; 64];
        let mut expected = [[0.0; 2]; 64];
        failed.render(&mut retried).unwrap();
        reference.render(&mut expected).unwrap();
        assert_eq!(retried, expected);
    });
}

#[test]
fn shared_contributor_mute_settles_independently_of_authored_release() {
    on_control(|| {
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.envelope.release_seconds = 1.0;
        let replacement = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: replacement,
            })
            .unwrap();
        let mut old = started(&f);
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut candidate = playback(&f);
        candidate.try_inherit_state_from(&mut old).unwrap();
        candidate.render(&mut [[0.0; 2]; 800]).unwrap();
        check_pcm(&mut candidate, 807, 8, 0.25);
    });
}

#[test]
fn shared_contributor_unmute_restores_held_voice_without_doubling() {
    on_control(|| {
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.envelope.release_seconds = 0.2;
        let replacement = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: replacement,
            })
            .unwrap();
        let mut old = started(&f);
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let mut muted = playback(&f);
        muted.try_inherit_state_from(&mut old).unwrap();
        muted.render(&mut [[0.0; 2]; 20]).unwrap();
        assert!(f.editor.undo());
        let mut unmuted = playback(&f);
        unmuted.try_inherit_state_from(&mut muted).unwrap();
        let mut output = [[0.0; 2]; 16];
        unmuted.render(&mut output).unwrap();
        for (index, frame) in output.iter().enumerate() {
            let gain = 0.25 + 0.25 * (0.5 + 0.5 * index as f32 / 40.0);
            for channel in 0..2 {
                let expected = PCM[(27 + index) % 4][channel] * gain;
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "index={index}, frame={frame:?}, expected={expected}"
                );
            }
        }
        unmuted.render(&mut [[0.0; 2]; 1800]).unwrap();
        check_pcm(&mut unmuted, 1843, 8, 0.5);
    });
}

#[test]
fn shared_contributor_mute_and_solo_preserve_receiver_voice() {
    on_control(|| {
        for solo in [false, true] {
            let mut f = fixture();
            let mut old = started(&f);
            f.editor
                .execute(if solo {
                    Command::ToggleSolo {
                        track: f.receiver,
                        exclusive: true,
                    }
                } else {
                    Command::Mute {
                        track: f.contributor,
                        muted: true,
                    }
                })
                .unwrap();
            let mut candidate = playback(&f);
            candidate.try_inherit_state_from(&mut old).unwrap();
            // One held voice continues unchanged while the other follows
            // the source-owned 5 ms contributor attenuation before summing.
            let mut transition = [[0.0; 2]; 19];
            for chunk in transition.chunks_mut(3) {
                candidate.render(chunk).unwrap();
            }
            for (index, frame) in transition.iter().enumerate() {
                let gain = 0.25 + 0.25 * (1.0 - index as f32 / 40.0);
                for channel in 0..2 {
                    assert!(
                        (frame[channel] - PCM[(7 + index) % 4][channel] * gain).abs() < 0.000_002,
                        "solo={solo} frame={index} output={frame:?}"
                    );
                }
            }
            candidate.render(&mut [[0.0; 2]; 128]).unwrap();
            check_pcm(&mut candidate, 154, 8, 0.25);
            assert!(f.editor.undo());
            let mut undone = playback(&f);
            undone.try_inherit_state_from(&mut candidate).unwrap();
            let mut output = [[0.0; 2]; 8];
            undone.render(&mut output).unwrap();
            for (index, frame) in output.iter().enumerate() {
                let gain = 0.25 + 0.25 * index as f32 / 40.0;
                for channel in 0..2 {
                    let expected = PCM[(162 + index) % 4][channel] * gain;
                    assert!(
                        (frame[channel] - expected).abs() < 0.000_002,
                        "solo={solo}, index={index}, frame={frame:?}, expected={expected}"
                    );
                }
            }
        }
    });
}
#[test]
fn muting_shared_contributor_fades_its_live_key_with_authored_voice() {
    for solo in [false, true] {
        for rate in [8_000_u32, 44_100, 96_000] {
            for partition in [1, 7, 256] {
                on_control(move || {
                    use crate::playback::PlaybackCommand;
                    let mut f = fixture_at(rate);
                    let input =
                        Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
                    input.set_key(0, Some((60, 127, 1))).unwrap();
                    let (mut old, _) = live_playback(&f, &input);
                    old.apply(PlaybackCommand::Play);
                    old.render(&mut [[0.0; 2]; 7]).unwrap();
                    f.editor
                        .execute(if solo {
                            Command::ToggleSolo {
                                track: f.receiver,
                                exclusive: true,
                            }
                        } else {
                            Command::Mute {
                                track: f.contributor,
                                muted: true,
                            }
                        })
                        .unwrap();
                    let (mut candidate, _) = live_playback(&f, &input);
                    candidate
                        .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                        .unwrap();
                    let fade_frames = rate.div_ceil(200) as usize;
                    let mut output = vec![[0.0; 2]; fade_frames + 16];
                    for chunk in output.chunks_mut(partition) {
                        candidate.render(chunk).unwrap();
                    }
                    for (index, frame) in output.iter().enumerate().take(fade_frames) {
                        let gain = 0.25 + 0.5 * (1.0 - index as f32 / fade_frames as f32);
                        for channel in 0..2 {
                            let expected = PCM[(7 + index) % 4][channel] * gain;
                            assert!(
                                (frame[channel] - expected).abs() < 0.000_002,
                                "solo={solo} rate={rate} partition={partition} index={index} frame={frame:?} expected={expected}"
                            );
                        }
                    }
                    for (index, frame) in output.iter().enumerate().skip(fade_frames) {
                        for channel in 0..2 {
                            assert!(
                                (frame[channel] - PCM[(7 + index) % 4][channel] * 0.25).abs()
                                    < 0.000_002,
                                "index={index} frame={frame:?}"
                            );
                        }
                    }
                });
            }
        }
    }
}
#[test]
fn live_contributor_rapid_unmute_preserves_key_up() {
    for solo in [false, true] {
        on_control(move || {
            use crate::playback::PlaybackCommand;
            let mut f = fixture();
            let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 127, 1))).unwrap();
            let (mut old, _) = live_playback(&f, &input);
            old.apply(PlaybackCommand::Play);
            old.render(&mut [[0.0; 2]; 7]).unwrap();
            f.editor
                .execute(if solo {
                    Command::ToggleSolo {
                        track: f.receiver,
                        exclusive: true,
                    }
                } else {
                    Command::Mute {
                        track: f.contributor,
                        muted: true,
                    }
                })
                .unwrap();
            let (mut muted, _) = live_playback(&f, &input);
            muted
                .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .unwrap();
            muted.render(&mut [[0.0; 2]; 20]).unwrap();
            assert!(f.editor.undo());
            let (mut restored, _) = live_playback(&f, &input);
            restored
                .try_inherit_runtime_in_control_context(&mut muted, Some(19), Some(19))
                .unwrap();
            let mut output = [[0.0; 2]; 40];
            restored.render(&mut output).unwrap();
            for (index, frame) in output.iter().enumerate() {
                let gain = 0.25 + 0.5 * (0.5 + 0.5 * index as f32 / 40.0);
                for channel in 0..2 {
                    let expected = PCM[(27 + index) % 4][channel] * gain;
                    assert!(
                        (frame[channel] - expected).abs() < 0.000_002,
                        "index={index} frame={frame:?} expected={expected}"
                    );
                }
            }
            input.set_key(0, None).unwrap();
            restored.render(&mut [[0.0; 2]; 128]).unwrap();
            let mut output = [[0.0; 2]; 8];
            restored.render(&mut output).unwrap();
            for (index, frame) in output.iter().enumerate() {
                for channel in 0..2 {
                    let expected = PCM[(195 + index) % 4][channel] * 0.5;
                    assert!(
                        (frame[channel] - expected).abs() < 0.000_002,
                        "key-up index={index} frame={frame:?} expected={expected}"
                    );
                }
            }
            restored.apply(PlaybackCommand::Stop);
            restored.render(&mut [[0.0; 2]; 64]).unwrap();
            assert!(input.is_cancelled());
        });
    }
}

#[test]
fn keyboard_tracks_only_a_held_press_suppressed_by_source_mute() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut old, _) = live_playback(&f, &input);
        old.apply(PlaybackCommand::Play);
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        let count = |playback: &mut crate::playback::Playback| {
            playback
                .session_runtime_mut_for_stream_action()
                .unwrap()
                .computer_keyboard
                .as_ref()
                .unwrap()
                .muted_source_held_count()
        };
        assert_eq!(count(&mut old), 1);
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let (mut next, _) = live_playback(&f, &input);
        next.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        assert_eq!(count(&mut next), 1);
        input.set_key(0, None).unwrap();
        next.render(&mut [[0.0; 2]; 8]).unwrap();
        assert_eq!(count(&mut next), 0);
        input.set_key(0, Some((60, 127, 2))).unwrap();
        next.render(&mut [[0.0; 2]; 8]).unwrap();
        assert_eq!(count(&mut next), 0, "an audible press was not suppressed");
        let keyboard = next
            .session_runtime_mut_for_stream_action()
            .unwrap()
            .computer_keyboard
            .as_mut()
            .unwrap();
        let id = keyboard.held_physical_id().unwrap();
        keyboard.begin_block();
        keyboard.mark_muted_source_onset(id);
        keyboard.rollback();
        assert_eq!(
            keyboard.muted_source_held_count(),
            0,
            "a failed block cannot commit the marker"
        );
        input.cancel();
        next.render(&mut [[0.0; 2]; 8]).unwrap();
        assert_eq!(count(&mut next), 0);
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut stopped, _) = live_playback(&f, &input);
        stopped.apply(PlaybackCommand::Play);
        stopped.render(&mut [[0.0; 2]; 7]).unwrap();
        assert_eq!(count(&mut stopped), 1);
        stopped.apply(PlaybackCommand::Stop);
        stopped.render(&mut [[0.0; 2]; 8]).unwrap();
        assert_eq!(count(&mut stopped), 0, "stop clears the muted input proof");
    });
}

#[test]
fn live_key_started_while_muted_resumes_on_unmute() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut old, _) = live_playback(&f, &input);
        old.apply(PlaybackCommand::Play);
        let mut before = [[0.0; 2]; 7];
        old.render(&mut before).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let (mut next, _) = live_playback(&f, &input);
        next.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        let mut output = [[0.0; 2]; 128];
        next.render(&mut output).unwrap();
        for (index, frame) in output.iter().enumerate() {
            for channel in 0..2 {
                let fade = index.min(40) as f32 / 40.0;
                let expected =
                    PCM[(7 + index) % 4][channel] * 0.25 + PCM[index % 4][channel] * 0.5 * fade;
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "frame {index}: {frame:?}, expected {expected}"
                );
            }
        }
        input.set_key(0, None).unwrap();
        next.render(&mut [[0.0; 2]; 64]).unwrap();
        let mut after_release = [[0.0; 2]; 16];
        next.render(&mut after_release).unwrap();
        for (index, frame) in after_release.iter().enumerate() {
            for channel in 0..2 {
                let expected =
                    0.25 * (PCM[(199 + index) % 4][channel] + PCM[(192 + index) % 4][channel]);
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "key-up frame {index}: {frame:?}, expected {expected}"
                );
            }
        }
    });
}
#[test]
fn muted_live_key_continuation_refuses_source_replacement_and_multiple_keys() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        for multiple in [false, true] {
            let mut f = fixture();
            f.editor
                .execute(Command::Mute {
                    track: f.contributor,
                    muted: true,
                })
                .unwrap();
            let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 127, 1))).unwrap();
            if multiple {
                input.set_key(1, Some((62, 127, 2))).unwrap();
            }
            let (mut old, _) = live_playback(&f, &input);
            old.apply(PlaybackCommand::Play);
            old.render(&mut [[0.0; 2]; 7]).unwrap();
            assert!(
                old.session_runtime_mut_for_stream_action()
                    .unwrap()
                    .computer_keyboard
                    .as_ref()
                    .unwrap()
                    .has_muted_source_onset(),
                "missing marker, multiple={multiple}"
            );
            if multiple {
                f.editor
                    .execute(Command::Mute {
                        track: f.contributor,
                        muted: false,
                    })
                    .unwrap();
            } else {
                let device = f
                    .editor
                    .project()
                    .track(f.receiver)
                    .unwrap()
                    .instrument()
                    .unwrap();
                let mut state =
                    hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
                state.reverse = !state.reverse;
                let replacement = state
                    .to_native()
                    .unwrap()
                    .with_asset_references(device.state().asset_references().to_vec())
                    .unwrap();
                f.editor
                    .execute(Command::ReplaceDeviceState {
                        device: device.id(),
                        state: replacement,
                    })
                    .unwrap();
            }
            let (mut candidate, _) = live_playback(&f, &input);
            assert!(
                candidate
                    .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                    .is_err(),
                "multiple={multiple}"
            );
            old.render(&mut [[0.0; 2]; 8]).unwrap();
            assert!(!input.is_cancelled());
        }
    });
}

#[test]
fn muted_live_key_released_before_first_render_is_not_replayed() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut old, _) = live_playback(&f, &input);
        old.apply(PlaybackCommand::Play);
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let (mut next, _) = live_playback(&f, &input);
        next.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        input.set_key(0, None).unwrap();
        let mut output = [[0.0; 2]; 64];
        next.render(&mut output).unwrap();
        for (i, frame) in output.iter().enumerate() {
            let fade = i.min(40) as f32 / 40.0;
            for channel in 0..2 {
                let expected = 0.25 * (PCM[(7 + i) % 4][channel] + PCM[i % 4][channel] * fade);
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "frame {i} channel {channel}: {frame:?}, expected {expected}"
                );
            }
        }
    });
}

#[test]
fn muted_live_key_keeps_peer_phase_through_unchanged_handoff() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut old, _) = live_playback(&f, &input);
        old.apply(PlaybackCommand::Play);
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        let (mut next, _) = live_playback(&f, &input);
        next.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        let mut output = [[0.0; 2]; 8];
        next.render(&mut output).unwrap();
        for (i, frame) in output.iter().enumerate() {
            for channel in 0..2 {
                let expected = PCM[(7 + i) % 4][channel] * 0.25;
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "frame {i}: {frame:?}, expected {expected}"
                );
            }
        }
    });
}

#[test]
fn resumed_live_key_rolls_back_when_later_route_fails() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        let reference_input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        reference_input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut old, _) = live_playback(&f, &input);
        let (mut reference_old, _) = live_playback(&f, &reference_input);
        for playback in [&mut old, &mut reference_old] {
            playback.apply(PlaybackCommand::Play);
            playback.render(&mut [[0.0; 2]; 7]).unwrap();
        }
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: false,
            })
            .unwrap();
        let (mut failed, _) = live_playback(&f, &input);
        let (mut reference, _) = live_playback(&f, &reference_input);
        failed
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        reference
            .try_inherit_runtime_in_control_context(&mut reference_old, Some(19), Some(19))
            .unwrap();
        let runtime = failed.session_runtime_mut_for_stream_action().unwrap();
        let route = runtime
            .routes
            .iter()
            .position(|route| route.id == f.contributor)
            .unwrap();
        let incarnation = runtime.routes[route].current_incarnation;
        runtime.routes[route].current_incarnation = u64::MAX;
        runtime.routes[route].pending = Some(crate::session_playback::PendingRoute {
            note_chase: crate::NoteChase::On,
            frame: 39,
            beat: BeatTime::ZERO,
            target: crate::session_playback::RouteTarget::Silence,
            current_reservation: None,
        });
        assert!(failed.render(&mut [[0.0; 2]; 64]).is_err());
        let runtime = failed.session_runtime_mut_for_stream_action().unwrap();
        runtime.routes[route].pending = None;
        runtime.routes[route].current_incarnation = incarnation;
        let mut actual = [[0.0; 2]; 64];
        let mut expected = [[0.0; 2]; 64];
        failed.render(&mut actual).unwrap();
        reference.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
    });
}
#[test]
fn live_contributor_fade_advances_during_stopped_monitoring() {
    on_control(|| {
        let mut f = fixture();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut old, _) = live_playback(&f, &input);
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let (mut muted, _) = live_playback(&f, &input);
        muted
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        let mut output = [[0.0; 2]; 64];
        muted.render(&mut output).unwrap();
        for (index, frame) in output.iter().enumerate() {
            let gain = 0.25 * (1.0 - index.min(40) as f32 / 40.0);
            for channel in 0..2 {
                let expected = PCM[(7 + index) % 4][channel] * gain;
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "stopped index={index} frame={frame:?} expected={expected}"
                );
            }
        }
        assert!(!input.is_cancelled());
        input.set_key(1, Some((62, 127, 2))).unwrap();
        muted.render(&mut output).unwrap();
        assert!(
            output
                .iter()
                .flatten()
                .all(|sample| sample.abs() < 0.000_002),
            "a new key must not bypass the contributor mute"
        );
        input.set_key(0, None).unwrap();
        muted.render(&mut output).unwrap();
        assert!(f.editor.undo());
        let (mut unmuted, _) = live_playback(&f, &input);
        unmuted
            .try_inherit_runtime_in_control_context(&mut muted, Some(19), Some(19))
            .unwrap();
        unmuted.render(&mut output).unwrap();
        assert!(
            output
                .iter()
                .flatten()
                .all(|sample| sample.abs() < 0.000_002),
            "a key pressed while muted must not be chased on unmute"
        );
        input.set_key(1, None).unwrap();
        unmuted.render(&mut output).unwrap();
        input.set_key(1, Some((62, 127, 3))).unwrap();
        unmuted.render(&mut output).unwrap();
        assert!(
            output
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.000_002),
            "a fresh press must sound after unmute"
        );
    });
}
#[test]
fn live_key_up_while_muted_does_not_revive_a_long_release() {
    on_control(|| {
        use crate::playback::PlaybackCommand;
        let mut f = fixture();
        let device = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
        state.envelope.release_seconds = 0.2;
        let replacement = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: replacement,
            })
            .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let (mut old, _) = live_playback(&f, &input);
        old.apply(PlaybackCommand::Play);
        old.render(&mut [[0.0; 2]; 7]).unwrap();
        f.editor
            .execute(Command::Mute {
                track: f.contributor,
                muted: true,
            })
            .unwrap();
        let (mut muted, _) = live_playback(&f, &input);
        muted
            .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
            .unwrap();
        muted.render(&mut [[0.0; 2]; 64]).unwrap();
        input.set_key(0, None).unwrap();
        muted.render(&mut [[0.0; 2]; 64]).unwrap();
        assert!(f.editor.undo());
        let (mut restored, _) = live_playback(&f, &input);
        restored
            .try_inherit_runtime_in_control_context(&mut muted, Some(19), Some(19))
            .unwrap();
        restored.render(&mut [[0.0; 2]; 64]).unwrap();
        let mut output = [[0.0; 2]; 8];
        restored.render(&mut output).unwrap();
        for (index, frame) in output.iter().enumerate() {
            for channel in 0..2 {
                let expected = PCM[(199 + index) % 4][channel] * 0.5;
                assert!(
                    (frame[channel] - expected).abs() < 0.000_002,
                    "long release index={index} frame={frame:?} expected={expected}"
                );
            }
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
        state.reverse = !state.reverse;
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
#[test]
fn shared_provider_replacement_requires_stop_and_undo_restores_one_source() {
    on_control(|| {
        let mut f = fixture();
        let mut old = started(&f);
        let device = f
            .editor
            .project()
            .native_source(f.source)
            .unwrap()
            .instrument()
            .id();
        let replacement = hex_devices::operator_state::OPERATOR_METADATA
            .default_state()
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: replacement,
            })
            .unwrap();
        let mut candidate = playback(&f);
        assert_eq!(
            candidate.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::SourceHistoryUnavailable)
        );
        assert_eq!(old.performance_frame(), 7);
        check_pcm(&mut old, 7, 19, 0.5);
        old.apply(SessionPlaybackCommand::Stop).unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        let mut output = [[1.0; 2]; 7];
        candidate.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 7]);
        assert_eq!(candidate.performance_frame(), 0);
        assert_eq!(
            f.editor
                .project()
                .native_source(f.source)
                .unwrap()
                .instrument()
                .id(),
            device
        );
        assert!(f.editor.undo());
        let mut restored = playback(&f);
        restored.try_inherit_state_from(&mut candidate).unwrap();
        assert_eq!(restored.mutable_instruments.iter().flatten().count(), 1);
        restored.apply(SessionPlaybackCommand::Play).unwrap();
        check_pcm(&mut restored, 0, 19, 0.5);
        assert_eq!(f.editor.project().source_contributors(f.source).count(), 2);
    });
}
#[test]
fn shared_sampler_tuning_uses_actual_cursor_through_skipped_preview_and_cancel() {
    on_control(|| {
        fn tune(f: &mut Fixture, hz: f64) {
            let mut scale = hex_tuning::ProjectTuning::default().scale();
            scale.set_reference(69, hz, 0).unwrap();
            f.editor
                .execute(Command::SetTuning(
                    hex_tuning::ProjectTuning::new(true, scale, hex_tuning::Audition::default())
                        .unwrap(),
                ))
                .unwrap();
        }
        fn literal(runtime: &mut SessionPlayback, position: usize, step: usize, count: usize) {
            let mut output = vec![[0.0; 2]; count];
            for chunk in output.chunks_mut(3) {
                runtime.render(chunk).unwrap();
            }
            for (i, frame) in output.iter().enumerate() {
                assert_eq!(
                    *frame,
                    PCM[(position + i * step) % 4].map(|sample| sample * 0.5)
                );
            }
        }
        // An octave up, band-limited reconstruction (#4089) of the
        // 4-frame loop keeps only its mean: every other partial lies at
        // or above the output Nyquist. The cursor is checked by the
        // literal unity segments on either side.
        fn octave(runtime: &mut SessionPlayback, count: usize) {
            let mut output = vec![[0.0; 2]; count];
            for chunk in output.chunks_mut(3) {
                runtime.render(chunk).unwrap();
            }
            let mean: [f32; 2] = std::array::from_fn(|channel| {
                PCM.iter().map(|frame| frame[channel]).sum::<f32>() / 4.0 * 0.5
            });
            for frame in output {
                for channel in 0..2 {
                    assert!((frame[channel] - mean[channel]).abs() < 1e-3, "{frame:?}");
                }
            }
        }
        let mut f = fixture();
        let neutral = f.editor.project().clone();
        let mut old = started(&f);
        let old_revision = Arc::clone(&old.revision);
        tune(&mut f, 660.0);
        let skipped_project = f.editor.project().clone();
        let skipped = playback_with_tuning(&f, Some((&neutral, &old_revision)));
        tune(&mut f, 880.0);
        let mut unrelated = playback(&f);
        assert!(
            unrelated.try_inherit_state_from(&mut old).is_err(),
            "a fresh open is not a tuning history proof"
        );
        literal(&mut old, 7, 1, 5);
        let tuned_project = f.editor.project().clone();
        let mut tuned = playback_with_tuning(&f, Some((&skipped_project, &skipped.revision)));
        tuned.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(tuned.mutable_instruments.iter().flatten().count(), 1);
        assert!(tuned.tail_mutable_instruments.iter().all(Option::is_none));
        octave(&mut tuned, 21);
        f.editor
            .execute(Command::SetTuning(neutral.tuning()))
            .unwrap();
        let mut cancelled = playback_with_tuning(&f, Some((&tuned_project, &tuned.revision)));
        cancelled.try_inherit_state_from(&mut tuned).unwrap();
        literal(&mut cancelled, 54, 1, 19);
        assert_eq!(cancelled.mutable_instruments.iter().flatten().count(), 1);
        assert!(
            cancelled
                .tail_mutable_instruments
                .iter()
                .all(Option::is_none)
        );
    });
}
