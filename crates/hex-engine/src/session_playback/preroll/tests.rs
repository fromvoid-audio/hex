use super::*;
use crate::session_playback::tests::{
    mutable_feedback_with_stateful_track_and_return_edited, render_checkpoint_partitioned,
};

fn fixture() -> Arc<PreparedRevision> {
    mutable_feedback_with_stateful_track_and_return_edited(|snapshot| {
        let clips = match &mut snapshot.tracks[0].content {
            hex_project::TrackContentSnapshot::Instrument(clips) => clips,
            hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            hex_project::TrackContentSnapshot::Audio(_) => panic!("instrument fixture"),
        };
        clips[0].notes[0].duration_raw = 1 << 32;
    })
}

fn on_worker_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(test)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn history_preserving_interactive_matches_held_and_released_pcm_and_handoff() {
    on_worker_stack(|| {
        let revision = fixture();
        // Onset, held, immediately before/at/after NoteOff, and feedback tail.
        for destination in [0, 512, 23_999, 24_000, 24_001, 24_004] {
            let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            render_checkpoint_partitioned(&mut oracle, &[17, 61, 193], destination);
            let expected = render_checkpoint_partitioned(&mut oracle, &[1, 7, 31], 193);
            assert!(
                expected
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 1.0e-6)
            );
            for playing in [false, true] {
                let mut candidate =
                    SessionPlayback::prepare_history_preserving_arrangement_preroll(
                        Arc::clone(&revision),
                        None,
                        destination as u64,
                        destination as u64,
                        playing,
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                assert!(Arc::ptr_eq(&candidate.revision, &revision));
                assert!(!candidate.is_linear_arrangement_export());
                assert_eq!(candidate.performance_frame(), destination as u64);
                assert_eq!(candidate.arrangement_frame(), destination as u64);
                assert_eq!(candidate.playing(), playing);
                if !playing {
                    let mut silence = [[1.0; 2]; 19];
                    candidate.render(&mut silence).unwrap();
                    assert_eq!(silence, [[0.0; 2]; 19]);
                    assert_eq!(candidate.performance_frame(), destination as u64);
                    candidate.apply(SessionPlaybackCommand::Play).unwrap();
                }
                // A normal compiled interactive successor must retain this
                // warmed history; export-only candidates cannot do this.
                let mut successor = SessionPlayback::new(Arc::clone(&revision)).unwrap();
                successor.try_inherit_state_from(&mut candidate).unwrap();
                let actual = render_checkpoint_partitioned(&mut successor, &[5, 127, 3], 193);
                assert_eq!(
                    actual, expected,
                    "destination={destination}, playing={playing}"
                );
                let clip = revision.session().midi_clips()[0].id;
                successor
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
                successor.render(&mut [[0.0; 2]; 17]).unwrap();
            }
        }
    });
}

#[test]
fn history_preserving_refuses_work_storage_loop_and_cancellation_atomically() {
    on_worker_stack(|| {
        let revision = fixture();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut twin = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        for runtime in [&mut old, &mut twin] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 37]).unwrap();
        }
        for (destination, budget) in [(512, 511), (0, MAX_ARRANGEMENT_PREROLL_FRAMES + 1)] {
            assert_eq!(
                SessionPlayback::prepare_history_preserving_arrangement_preroll(
                    Arc::clone(&revision),
                    None,
                    destination,
                    budget,
                    true,
                    &AtomicBool::new(false),
                )
                .unwrap_err(),
                ArrangementPrerollError::WorkBudgetExceeded
            );
        }
        let looped = mutable_feedback_with_stateful_track_and_return_edited(|snapshot| {
            snapshot.loop_region = Some(hex_project::LoopRegion::new(0, 1024, true).unwrap());
        });
        assert_eq!(
            SessionPlayback::prepare_history_preserving_arrangement_preroll(
                looped,
                None,
                512,
                512,
                true,
                &AtomicBool::new(false),
            )
            .unwrap_err(),
            ArrangementPrerollError::UnsupportedTransportLoop
        );
        let preflight = arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
        let polls = std::cell::Cell::new(0);
        for (storage, cancel_at, expected) in [
            (
                preflight - 1,
                usize::MAX,
                ArrangementPrerollError::StorageBudgetExceeded,
            ),
            (
                MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                0,
                ArrangementPrerollError::Cancelled,
            ),
            (
                MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                2,
                ArrangementPrerollError::Cancelled,
            ),
            (
                MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                3,
                ArrangementPrerollError::Cancelled,
            ),
        ] {
            polls.set(0);
            let error = SessionPlayback::prepare_arrangement_preroll_from_revision_with_policy(
                Arc::clone(&revision),
                256,
                256,
                true,
                storage,
                PrerollRuntimePolicy::HistoryPreservingInteractive,
                None,
                || {
                    let count = polls.get();
                    polls.set(count + 1);
                    count >= cancel_at
                },
                |candidate, output| candidate.render(output).map_err(Into::into),
            )
            .unwrap_err();
            assert_eq!(error, expected);
        }
        assert_eq!(
            SessionPlayback::prepare_arrangement_preroll_from_revision_with_policy(
                Arc::clone(&revision),
                256,
                256,
                true,
                MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                PrerollRuntimePolicy::HistoryPreservingInteractive,
                None,
                || false,
                |_, _| Err(ArrangementPrerollError::MediaPreparation(
                    "unavailable media"
                )),
            )
            .unwrap_err(),
            ArrangementPrerollError::MediaPreparation("unavailable media")
        );
        assert_eq!(old.performance_frame(), 37);
        assert!(old.playing());
        assert_eq!(
            render_checkpoint_partitioned(&mut old, &[3, 127], 193),
            render_checkpoint_partitioned(&mut twin, &[17, 61], 193),
            "failed private preparation cannot damage the audible owner",
        );
    });
}

#[test]
fn history_preserving_requires_the_exact_ordinary_resource_scope() {
    on_worker_stack(|| {
        use crate::native_device_resources::{
            CompilerDeviceResources, PreparedDeviceResources, ResidentDeviceAssets,
        };
        let project = hex_project::Project::new(48_000).unwrap();
        let cancelled = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &ResidentDeviceAssets::default(),
            &project,
            1 << 20,
            &cancelled,
        )
        .unwrap();
        let foreign = PreparedDeviceResources::prepare_on_control(
            &ResidentDeviceAssets::default(),
            &project,
            1 << 20,
            &cancelled,
        )
        .unwrap();
        let compiler = CompilerDeviceResources::new(&resources, 0, &cancelled);
        let revision = Arc::new(
            PreparedRevision::prepare_with_registries_and_sampler_sources(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(crate::SineTable::new()),
                None,
                &hex_devices::native_registry().unwrap(),
                &hex_devices::native_instrument_registry().unwrap(),
                &crate::audio_transform::AudioTransformPreparation::default(),
                Some(&compiler),
            )
            .unwrap(),
        );
        for scope in [None, Some(&foreign)] {
            assert!(matches!(
                SessionPlayback::prepare_history_preserving_arrangement_preroll(
                    Arc::clone(&revision),
                    scope,
                    17,
                    17,
                    true,
                    &cancelled,
                ),
                Err(ArrangementPrerollError::Playback(_))
            ));
        }
        let candidate = SessionPlayback::prepare_history_preserving_arrangement_preroll(
            Arc::clone(&revision),
            Some(&resources),
            17,
            17,
            true,
            &cancelled,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&candidate.revision, &revision));
        assert_eq!(candidate.performance_frame(), 17);
    });
}

#[test]
fn history_pages_refuse_invalid_width_failure_and_mid_chunk_cancellation() {
    on_worker_stack(|| {
        struct Pages<'a> {
            width: usize,
            cancel: &'a AtomicBool,
            cancel_on_prepare: bool,
            fail: bool,
        }
        impl crate::MediaPages for Pages<'_> {
            fn page(
                &mut self,
                _: crate::StreamSourceKey,
                _: u64,
            ) -> Option<crate::BorrowedMediaPage<'_>> {
                None
            }
            fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {}
        }
        impl PreparedPrerollPages for Pages<'_> {
            fn prepare_chunk(
                &mut self,
                _: &SessionPlayback,
                maximum: usize,
                _: &AtomicBool,
            ) -> Result<usize, ArrangementPrerollError> {
                if self.fail {
                    return Err(ArrangementPrerollError::MediaPreparation(
                        "fixture read failed",
                    ));
                }
                if self.cancel_on_prepare {
                    self.cancel.store(true, Ordering::Release);
                }
                Ok(if self.width == 7 {
                    maximum.min(7)
                } else {
                    self.width
                })
            }
            fn complete_chunk(&mut self) -> Result<(), ArrangementPrerollError> {
                Ok(())
            }
        }
        let revision = fixture();
        for (width, fail, cancel_on_prepare, expected) in [
            (
                0,
                false,
                false,
                ArrangementPrerollError::MediaPreparation("invalid preroll page demand width"),
            ),
            (
                usize::MAX,
                false,
                false,
                ArrangementPrerollError::MediaPreparation("invalid preroll page demand width"),
            ),
            (
                7,
                true,
                false,
                ArrangementPrerollError::MediaPreparation("fixture read failed"),
            ),
            (7, false, true, ArrangementPrerollError::Cancelled),
        ] {
            let cancel = AtomicBool::new(false);
            let mut pages = Pages {
                width,
                cancel: &cancel,
                cancel_on_prepare,
                fail,
            };
            assert_eq!(
                SessionPlayback::prepare_history_preserving_arrangement_preroll_with_pages(
                    Arc::clone(&revision),
                    None,
                    256,
                    256,
                    true,
                    &cancel,
                    &mut pages,
                )
                .unwrap_err(),
                expected
            );
        }
    });
}

// This is an opt-in elapsed-time probe, not a realtime or native-device test.
// Run with: cargo test --release -p hex-engine --lib history_preparation_profile -- --ignored --nocapture
#[test]
#[ignore = "manual optimized preparation profile"]
#[allow(clippy::assertions_on_constants, reason = "reject debug profiling")]
fn history_preparation_profile() {
    on_worker_stack(|| {
        use hex_lattice_contract::{
            DeviceTypeId, NativeDeviceMetadata, ParameterId, ParameterValue,
        };
        use hex_project::{BeatTime, Command, Created, DeviceSnapshot, Editor, Project};
        use std::time::Instant;

        assert!(
            !cfg!(debug_assertions),
            "profile requires an optimized build"
        );
        let setup = Instant::now();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "history profile".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("expected instrument track")
        };
        let instrument = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let operator = DeviceTypeId::from_static("hex.instrument.operator");
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Instrument,
            operator,
        )
        .unwrap();
        let state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(78),
            ParameterValue::Float(0.4),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument,
                state,
            })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("expected MIDI clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let mut snapshot = editor.project().snapshot();
        let track_effect_id = snapshot.next_id;
        let return_id = track_effect_id + 1;
        let return_effect_id = track_effect_id + 2;
        snapshot.next_id += 3;
        let filter = hex_devices::utility::UTILITY_DEFINITION
            .patch_parameter(
                &hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
                hex_devices::utility::parameter::DC_REMOVE,
                ParameterValue::Bool(true),
            )
            .unwrap();
        snapshot.tracks[0].devices.push(DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: track_effect_id,
            bypassed: false,
            state: filter.clone(),
        });
        snapshot.tracks[0]
            .sends
            .push(hex_project::SendLevelSnapshot {
                return_track: return_id,
                gain: 0.5,
            });
        snapshot
            .return_tracks
            .push(hex_project::ReturnTrackSnapshot {
                id: return_id,
                name: "return".into(),
                gain: 0.75,
                muted: false,
                devices: vec![DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: return_effect_id,
                    bypassed: false,
                    state: filter,
                }],
            });
        let project = Project::from_legacy_snapshot(snapshot).unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        println!(
            "HISTORY_PROFILE,rate=48000,tracks=1,voices=1,track_effect=dc_remove,return_effect=dc_remove,feedback=0.4,replay_block_max=256,work_limit_frames={MAX_ARRANGEMENT_PREROLL_FRAMES},storage_limit_bytes={MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES},warmup=1,measured=3,setup_us={}",
            setup.elapsed().as_micros()
        );
        for destination in [512_u64, 24_001] {
            let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            let mut remaining = destination;
            let mut buffer = [[0.0_f32; 2]; 256];
            while remaining > 0 {
                let frames = remaining.min(buffer.len() as u64) as usize;
                oracle.render(&mut buffer[..frames]).unwrap();
                remaining -= frames as u64;
            }
            let mut expected = [[0.0_f32; 2]; 256];
            oracle.render(&mut expected).unwrap();
            for history in [false, true] {
                for iteration in 0..4 {
                    let start = Instant::now();
                    let mut candidate = if history {
                        SessionPlayback::prepare_history_preserving_arrangement_preroll(
                            Arc::clone(&revision),
                            None,
                            destination,
                            destination,
                            true,
                            &AtomicBool::new(false),
                        )
                    } else {
                        SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                            Arc::clone(&revision),
                            destination,
                            destination,
                            true,
                            &AtomicBool::new(false),
                        )
                    }
                    .unwrap();
                    let elapsed = start.elapsed();
                    let mut output = [[0.0_f32; 2]; 256];
                    candidate.render(&mut output).unwrap();
                    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
                    if history {
                        assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-7));
                        assert_eq!(output, expected, "history must match linear playback");
                    }
                    let digest =
                        output
                            .iter()
                            .flatten()
                            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, sample| {
                                (hash ^ u64::from(sample.to_bits()))
                                    .wrapping_mul(0x0000_0100_0000_01b3)
                            });
                    println!(
                        "HISTORY_SAMPLE,destination={destination},mode={},warmup={},elapsed_us={},pcm_digest={digest:016x}",
                        if history { "history" } else { "fresh" },
                        iteration == 0,
                        elapsed.as_micros()
                    );
                }
            }
        }
    });
}
