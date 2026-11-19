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
fn looped_compensated_tempo_restart_publishes_without_callback_allocations() {
    run_tempo_cases(&[(1, true, true), (257, true, true)]);
}

fn run_tempo_cases(cases: &[(usize, bool, bool)]) {
    for &(partition, compensated, looped) in cases {
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
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(90_000_000).unwrap(),
            })
            .unwrap();
        let candidate = prepare(editor.project());
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
            let (anchor, fence) = control
                .prepare_fenced_tempo_anchor(
                    source_revision,
                    1,
                    Arc::clone(&old_for_anchor),
                    Arc::clone(&candidate),
                    cut_frames as u64 + 257,
                )
                .unwrap()
                .unwrap();
            assert_eq!(fence.expected_active_revision(), source_revision);
            donor.render(&mut [[0.0; 2]; 257]).unwrap();
            let mut planned =
                Playback::try_from_prepared_with_session(Arc::clone(&candidate)).unwrap();
            let measured = allocation_counter::measure(|| {
                planned
                    .try_inherit_runtime_with_tempo_anchor(&mut donor, Some(1), Some(1), &anchor)
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
            let mut fresh =
                Playback::try_from_prepared_with_session(Arc::clone(&candidate)).unwrap();
            fresh.apply(PlaybackCommand::PlayFrom(planned.position()));
            let mut projected = [[0.0; 2]; 512];
            let mut expected = projected;
            planned.render(&mut projected).unwrap();
            fresh.render(&mut expected).unwrap();
            assert_eq!(
                projected, expected,
                "future cut partition={partition} looped={looped}"
            );
        }
        let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&candidate)).unwrap();
        oracle.apply(PlaybackCommand::PlayFrom(800));
        let mut expected = [[0.0; 2]; 1024];
        oracle.render(&mut expected).unwrap();
        if compensated && !looped {
            // The ordinary linear publication keeps the predecessor's
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
