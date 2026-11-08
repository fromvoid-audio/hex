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
