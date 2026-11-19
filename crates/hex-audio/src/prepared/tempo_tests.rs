//! Callback publication of a compensated tempo edit, including allocation custody.
use super::*;
use hex_engine::{Audio, PreparedRevision, SineTable};
use hex_project::{BeatTime, Command, Created, Editor, Project, Tempo};
use std::collections::BTreeMap;

#[test]
fn compensated_tempo_edits_publish_without_callback_allocations() {
    run_tempo_cases(&[(1, true, false), (257, true, false), (257, false, false)]);
}

#[test]
fn looped_compensated_tempo_edits_publish_without_callback_allocations() {
    run_tempo_cases(&[(1, true, true), (257, true, true)]);
}

fn tempo_revisions(compensated: bool, looped: bool) -> [Arc<PreparedRevision>; 2] {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 8_192,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    if looped {
        editor
            .execute(Command::SetLoopRegion {
                start: 400,
                end: 2400,
                enabled: true,
            })
            .unwrap();
    }
    if compensated {
        let limiter = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: limiter,
            })
            .unwrap();
    }
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            48_000,
            (0..8_192)
                .map(|frame| {
                    let value = (frame as f32 * 0.017).sin() * 0.2;
                    [value, -0.7 * value]
                })
                .collect(),
        )
        .unwrap(),
    )]);
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let old = prepare(editor.project());
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let candidate = prepare(editor.project());
    [old, candidate]
}

fn run_tempo_cases(cases: &[(usize, bool, bool)]) {
    for &(partition, compensated, looped) in cases {
        let [old, candidate] = tempo_revisions(compensated, looped);
        let old_for_anchor = Arc::clone(&old);
        assert_eq!(old.arrangement().main_latency_frames() > 0, compensated);
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(matches!(
            control.prepare_fenced_tempo_anchor(1, 1, Arc::clone(&old), Arc::clone(&old), 1),
            Ok(None)
        ));
        let source_revision = control
            .publish_with_policy_and_context_generation(
                Playback::try_from_prepared_with_session(old).unwrap(),
                PublicationPolicy::InstallFreshRuntime,
                1,
            )
            .unwrap();
        callback.begin_block();
        assert!(matches!(
            control.prepare_fenced_tempo_anchor(
                source_revision,
                1,
                Arc::clone(&old_for_anchor),
                Arc::clone(&old_for_anchor),
                1
            ),
            Ok(None)
        ));
        control.command(PlaybackCommand::Play).unwrap();
        let cut_frames = if looped { 4600 } else { 600 };
        for chunk in vec![[0.0; 2]; cut_frames].chunks_mut(partition) {
            callback.begin_block();
            callback.render(chunk).unwrap();
        }
        assert_eq!(control.status.position.load(Acquire), 600);
        assert_eq!(
            candidate.arrangement().main_latency_frames() > 0,
            compensated
        );
        if compensated {
            for (revision, generation) in [(source_revision + 1, 1), (source_revision, 2)] {
                assert!(matches!(
                    control.prepare_fenced_tempo_anchor(
                        revision,
                        generation,
                        Arc::clone(&old_for_anchor),
                        Arc::clone(&candidate),
                        cut_frames as u64 + 257
                    ),
                    Ok(None)
                ));
            }
            assert!(matches!(
                control.prepare_fenced_tempo_anchor(
                    source_revision,
                    1,
                    Arc::clone(&old_for_anchor),
                    Arc::clone(&candidate),
                    cut_frames as u64 - 1
                ),
                Err(hex_engine::session_playback::SessionPlaybackError::InvalidCommand)
            ));
            let mut donor =
                Playback::try_from_prepared_with_session(Arc::clone(&old_for_anchor)).unwrap();
            donor.apply(PlaybackCommand::Play);
            donor.render(&mut vec![[0.0; 2]; cut_frames]).unwrap();
            let cut = control
                .prepare_fenced_tempo_anchor(
                    source_revision,
                    1,
                    Arc::clone(&old_for_anchor),
                    Arc::clone(&candidate),
                    cut_frames as u64 + 257,
                )
                .unwrap()
                .unwrap();
            assert_eq!(cut.fence.expected_active_revision(), source_revision);
            donor.render(&mut [[0.0; 2]; 257]).unwrap();
            let mut planned =
                Playback::try_from_prepared_with_session(Arc::clone(&candidate)).unwrap();
            let measured = allocation_counter::measure(|| {
                planned
                    .try_inherit_runtime_with_tempo_anchor(
                        &mut donor,
                        Some(1),
                        Some(1),
                        &cut.anchor,
                    )
                    .unwrap();
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(planned.performance_frame(), cut_frames as u64 + 257);
            let mut fresh =
                Playback::try_from_prepared_with_session(Arc::clone(&candidate)).unwrap();
            fresh.apply(PlaybackCommand::PlayFrom(planned.position()));
            let mut projected = [[0.0; 2]; 512];
            let mut expected = projected;
            planned.render(&mut projected).unwrap();
            fresh.render(&mut expected).unwrap();
            let latency = candidate.arrangement().main_latency_frames() as usize;
            let mut uninterrupted =
                Playback::try_from_prepared_with_session(Arc::clone(&old_for_anchor)).unwrap();
            uninterrupted.apply(PlaybackCommand::Play);
            uninterrupted
                .render(&mut vec![[0.0; 2]; cut_frames + 257])
                .unwrap();
            uninterrupted.render(&mut expected[..latency]).unwrap();
            assert!(expected[..latency].iter().any(|frame| *frame != [0.0; 2]));
            assert_eq!(
                projected, expected,
                "future cut partition={partition} looped={looped}"
            );
        }
        let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&candidate)).unwrap();
        oracle.apply(PlaybackCommand::PlayFrom(800));
        let mut expected = [[0.0; 2]; 1024];
        oracle.render(&mut expected).unwrap();
        if compensated {
            // The live publication keeps the predecessor's
            // in-flight samples. Once they drain, the edited source supplies
            // the same samples as the candidate started at the new beat.
            let latency = candidate.arrangement().main_latency_frames() as usize;
            let mut uninterrupted =
                Playback::try_from_prepared_with_session(Arc::clone(&old_for_anchor)).unwrap();
            uninterrupted.apply(PlaybackCommand::Play);
            uninterrupted
                .render(&mut vec![[0.0; 2]; cut_frames])
                .unwrap();
            uninterrupted.render(&mut expected[..latency]).unwrap();
            assert!(expected[..latency].iter().any(|sample| *sample != [0.0; 2]));
        }
        let applied = control
            .publish(Playback::try_from_prepared_with_session(candidate).unwrap())
            .unwrap();
        let mut output = [[0.0; 2]; 1024];
        let measured = allocation_counter::measure(|| {
            for chunk in output.chunks_mut(partition) {
                callback.begin_block();
                callback.render(chunk).unwrap();
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), applied);
        assert!(control.status.playing.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), 1_824);
        assert_eq!(
            output, expected,
            "partition {partition}, compensated {compensated}"
        );
        assert!(output.iter().flatten().any(|sample| *sample != 0.0));
        control.command(PlaybackCommand::Seek(5)).unwrap();
        assert!(matches!(
            control.prepare_fenced_tempo_anchor(
                applied,
                1,
                Arc::clone(&old_for_anchor),
                Arc::clone(&old_for_anchor),
                cut_frames as u64 + 2048,
            ),
            Ok(None)
        ));
        control.reclaim();
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

const FIRST_SOURCE: hex_engine::AudioSourceContext = hex_engine::AudioSourceContext {
    project_generation: 1,
    editor_revision: 1,
};
const RETIMED_SOURCE: hex_engine::AudioSourceContext = hex_engine::AudioSourceContext {
    editor_revision: 2,
    ..FIRST_SOURCE
};

fn resident_tempo_output(
    source: &Arc<PreparedRevision>,
    warm_frames: usize,
    with_ordinary_streams: bool,
) -> (Control, Callback, u64) {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.enable_native_custody().unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
        )
        .unwrap();
    if with_ordinary_streams {
        control
            .configure_ordinary_streaming_on_control(
                crate::AudioCandidateDomainReservation {
                    max_retained_data_bytes: 8 * 1024 * 1024,
                    max_construction_peak_data_bytes: 8 * 1024 * 1024,
                },
                3,
                4096,
            )
            .unwrap();
    }
    let revision = control
        .publish_with_admission_and_source_on_control(
            Playback::try_from_prepared_with_session(Arc::clone(source)).unwrap(),
            PublicationPolicy::InstallFreshRuntime,
            FIRST_SOURCE,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 257]).unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    control.command(PlaybackCommand::Play).unwrap();
    for frames in vec![[0.0; 2]; warm_frames].chunks_mut(257) {
        callback.begin_block();
        callback.render(frames).unwrap();
    }
    assert_eq!(
        control.status.performance_frame.load(Acquire),
        warm_frames as u64
    );
    (control, callback, revision)
}

fn render_tempo_probe(callback: &mut Callback, output: &mut [Frame]) {
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn scheduled_tempo_cut_splits_the_callback_and_keeps_old_delay_and_audible_loop() {
    let prefix = 237;
    for (partition, looped, window_start) in [(1, false, 600), (257, true, 2300)] {
        let [old, candidate] = tempo_revisions(true, looped);
        let (mut control, mut callback, active) = resident_tempo_output(&old, window_start, looped);
        let cut_frame = (window_start + prefix) as u64;
        let cut = control
            .prepare_fenced_tempo_anchor(
                active,
                1,
                Arc::clone(&old),
                Arc::clone(&candidate),
                cut_frame,
            )
            .unwrap()
            .unwrap();
        let revision = control
            .publish_tempo_cut_with_admission_on_control(
                Playback::try_from_prepared_with_session(Arc::clone(&candidate)).unwrap(),
                cut,
                RETIMED_SOURCE,
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        let receipt = control.fresh_runtime_receipt(revision).unwrap();
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);

        let latency = old.arrangement().main_latency_frames() as usize;
        assert!(latency > 20);
        let mut uninterrupted = Playback::try_from_prepared_with_session(Arc::clone(&old)).unwrap();
        uninterrupted.apply(PlaybackCommand::Play);
        uninterrupted
            .render(&mut vec![[0.0; 2]; window_start])
            .unwrap();
        let mut expected = [[0.0; 2]; 514];
        uninterrupted.render(&mut expected[..prefix]).unwrap();
        let beat = old
            .session()
            .timing()
            .frame_to_beat(uninterrupted.position())
            .unwrap();
        let destination = candidate.session().timing().beat_to_frame(beat).unwrap();
        let mut new_clock = Playback::try_from_prepared_with_session(candidate).unwrap();
        new_clock.apply(PlaybackCommand::PlayFrom(destination));
        new_clock.render(&mut expected[prefix..]).unwrap();
        uninterrupted
            .render(&mut expected[prefix..prefix + latency])
            .unwrap();
        assert!(
            expected[prefix..prefix + latency]
                .iter()
                .any(|frame| *frame != [0.0; 2])
        );

        let mut output = [[0.0; 2]; 514];
        for (index, frames) in output.chunks_mut(partition).enumerate() {
            render_tempo_probe(&mut callback, frames);
            let advanced = (index + 1) * partition;
            let current = window_start + advanced;
            assert_eq!(
                control.status.performance_frame.load(Acquire),
                current as u64
            );
            if current < cut_frame as usize {
                assert_eq!(receipt.state(), PublicationReceiptState::Pending);
                assert_eq!(control.status.applied_revision.load(Acquire), active);
            } else {
                assert_eq!(receipt.state(), PublicationReceiptState::Installed);
                assert_eq!(control.status.applied_revision.load(Acquire), revision);
            }
            if advanced == 257 {
                let audible = (current - latency) as u64;
                let old_position = if looped {
                    400 + (audible - 2400) % 2000
                } else {
                    audible
                };
                assert_eq!(control.status.audible_position.load(Acquire), old_position);
            }
        }
        assert_eq!(output, expected);
        assert_eq!(
            control.status.position.load(Acquire),
            destination + (514 - prefix) as u64
        );
        control.reclaim();
    }
}

#[test]
fn missed_cancelled_or_stopped_tempo_cut_keeps_the_exact_predecessor() {
    enum Interruption {
        Late,
        Cancel,
        Stop,
        StopAtCut,
    }
    for interruption in [
        Interruption::Late,
        Interruption::Cancel,
        Interruption::Stop,
        Interruption::StopAtCut,
    ] {
        let [old, candidate] = tempo_revisions(true, false);
        let (mut control, mut callback, active) = resident_tempo_output(&old, 600, false);
        let cut = control
            .prepare_fenced_tempo_anchor(active, 1, Arc::clone(&old), Arc::clone(&candidate), 700)
            .unwrap()
            .unwrap();
        let mut origin = 600;
        if matches!(interruption, Interruption::Late) {
            render_tempo_probe(&mut callback, &mut [[0.0; 2]; 101]);
            origin += 101;
        }
        let revision = control
            .publish_tempo_cut_with_admission_on_control(
                Playback::try_from_prepared_with_session(candidate).unwrap(),
                cut,
                RETIMED_SOURCE,
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        let receipt = control.fresh_runtime_receipt(revision).unwrap();
        if matches!(interruption, Interruption::Cancel) {
            assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
        }
        if matches!(interruption, Interruption::Stop) {
            control.command(PlaybackCommand::Stop).unwrap();
        }
        let mut actual = [[0.0; 2]; 257];
        if matches!(interruption, Interruption::StopAtCut) {
            let cut_barrier = Arc::new(std::sync::Barrier::new(2));
            callback.test_before_fresh_runtime_apply = Some(Arc::clone(&cut_barrier));
            std::thread::scope(|scope| {
                scope.spawn(|| render_tempo_probe(&mut callback, &mut actual));
                cut_barrier.wait();
                control.command(PlaybackCommand::Stop).unwrap();
                cut_barrier.wait();
            });
        } else {
            render_tempo_probe(&mut callback, &mut actual);
        }
        assert_eq!(
            receipt.state(),
            match interruption {
                Interruption::Late =>
                    PublicationReceiptState::Refused(PlaybackHandoffRejection::TempoCutMissed),
                Interruption::Cancel => PublicationReceiptState::Cancelled,
                Interruption::Stop | Interruption::StopAtCut =>
                    PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged),
            }
        );
        assert_eq!(control.status.applied_revision.load(Acquire), active);
        assert!(!control.status.publication_pending.load(Acquire));
        if matches!(interruption, Interruption::Stop) {
            assert!(!control.status.playing.load(Acquire));
        } else {
            let mut uninterrupted = Playback::try_from_prepared_with_session(old).unwrap();
            uninterrupted.apply(PlaybackCommand::Play);
            uninterrupted.render(&mut vec![[0.0; 2]; origin]).unwrap();
            let mut expected = [[0.0; 2]; 257];
            uninterrupted.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
        }
        if matches!(interruption, Interruption::StopAtCut) {
            render_tempo_probe(&mut callback, &mut actual);
            assert!(!control.status.playing.load(Acquire));
        }
        control.reclaim();
    }
}
