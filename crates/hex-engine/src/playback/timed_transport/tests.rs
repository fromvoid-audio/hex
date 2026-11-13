use super::*;
use crate::{Audio, PreparedRevision, SineTable};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

fn fixture(prepared: bool) -> (Playback, Vec<Frame>) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 4_096,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let samples: Vec<_> = (0..4_096)
        .map(|frame| {
            let value = frame as f32 / 8192.0;
            [value, -value]
        })
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples.clone()).unwrap())]);
    let playback = if prepared {
        Playback::from_prepared(Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        ))
    } else {
        Playback::new(crate::compile::project(editor.project(), &media).unwrap())
    };
    (playback, samples)
}

#[test]
fn timed_transport_matches_position_oracle_across_partitions() {
    let schedule = [
        (0, PlaybackCommand::PlayFrom(300)),
        (11, PlaybackCommand::Seek(100)),
        (23, PlaybackCommand::Stop),
        (29, PlaybackCommand::PlayFrom(600)),
        (41, PlaybackCommand::Pause),
        (47, PlaybackCommand::Play),
    ];
    for prepared in [false, true] {
        for block in [1, 7, 64, 128, 256, 512] {
            let (mut playback, samples) = fixture(prepared);
            let mut expected = Vec::new();
            expected.extend_from_slice(&samples[300..311]);
            expected.extend_from_slice(&samples[100..112]);
            expected.extend_from_slice(&[[0.0; 2]; 6]);
            expected.extend_from_slice(&samples[600..612]);
            expected.extend_from_slice(&[[0.0; 2]; 6]);
            expected.extend_from_slice(&samples[612..1589]);
            let mut actual = [[0.0; 2]; 1024];
            let mut next = 0;
            for (block_index, part) in actual.chunks_mut(block).enumerate() {
                let start = block_index * block;
                let end = start + part.len();
                let mut commands = Vec::new();
                while let Some(&(offset, command)) = schedule.get(next) {
                    if offset >= end {
                        break;
                    }
                    commands.push(TimedTransportCommand {
                        offset: offset - start,
                        command,
                    });
                    next += 1;
                }
                playback
                    .render_with_timed_transport(part, &commands)
                    .unwrap();
            }
            assert_eq!(next, schedule.len());
            assert_eq!(
                actual.as_slice(),
                expected,
                "prepared={prepared}, block={block}"
            );
            assert_eq!(playback.position(), 1589);
        }
    }
}

#[test]
fn equal_and_end_offsets_apply_in_order_including_empty_output() {
    for prepared in [false, true] {
        let (mut playback, samples) = fixture(prepared);
        let mut output = [[9.0; 2]; 4];
        playback
            .render_with_timed_transport(
                &mut output,
                &[
                    TimedTransportCommand {
                        offset: 0,
                        command: PlaybackCommand::PlayFrom(10),
                    },
                    TimedTransportCommand {
                        offset: 0,
                        command: PlaybackCommand::Seek(20),
                    },
                    TimedTransportCommand {
                        offset: 4,
                        command: PlaybackCommand::Pause,
                    },
                ],
            )
            .unwrap();
        assert_eq!(output, samples[20..24]);
        assert!(!playback.playing());
        playback
            .render_with_timed_transport(
                &mut [],
                &[
                    TimedTransportCommand {
                        offset: 0,
                        command: PlaybackCommand::Seek(30),
                    },
                    TimedTransportCommand {
                        offset: 0,
                        command: PlaybackCommand::Play,
                    },
                ],
            )
            .unwrap();
        assert!(playback.playing());
        let mut next = [[0.0; 2]; 1];
        playback
            .render_with_timed_transport(&mut next, &[])
            .unwrap();
        assert_eq!(next[0], samples[30]);
        let commands = [TimedTransportCommand {
            offset: 0,
            command: PlaybackCommand::Pause,
        }; MAX_TIMED_TRANSPORT_COMMANDS];
        playback
            .render_with_timed_transport(&mut [], &commands)
            .unwrap();
        assert!(!playback.playing());
    }
}

#[test]
fn invalid_schedules_leave_output_and_state_untouched() {
    let (mut playback, _) = fixture(true);
    playback.apply(PlaybackCommand::PlayFrom(30));
    let mut output = [[9.0; 2]; 4];
    let cmd = |offset| TimedTransportCommand {
        offset,
        command: PlaybackCommand::Stop,
    };
    assert_eq!(
        playback.render_with_timed_transport(&mut output, &[cmd(2), cmd(1)]),
        Err(TimedTransportError::InvalidOffset { index: 1 })
    );
    assert_eq!(
        playback.render_with_timed_transport(&mut output, &[cmd(5)]),
        Err(TimedTransportError::InvalidOffset { index: 0 })
    );
    assert_eq!(
        playback
            .render_with_timed_transport(&mut output, &[cmd(0); MAX_TIMED_TRANSPORT_COMMANDS + 1]),
        Err(TimedTransportError::TooManyCommands)
    );
    assert_eq!(output, [[9.0; 2]; 4]);
    assert!(playback.playing());
    assert_eq!(playback.position(), 30);
}

#[test]
fn failed_action_keeps_completed_prefix_and_silences_suffix() {
    let (mut playback, samples) = fixture(true);
    let mut output = [[9.0; 2]; 8];
    let result = playback.render_with_timed_transport(
        &mut output,
        &[
            TimedTransportCommand {
                offset: 0,
                command: PlaybackCommand::PlayFrom(10),
            },
            TimedTransportCommand {
                offset: 4,
                command: PlaybackCommand::PlayFrom(u64::MAX),
            },
            TimedTransportCommand {
                offset: 6,
                command: PlaybackCommand::Stop,
            },
        ],
    );
    assert!(matches!(
        result,
        Err(TimedTransportError::Action { index: 1, .. })
    ));
    assert_eq!(&output[..4], &samples[10..14]);
    assert_eq!(&output[4..], &[[0.0; 2]; 4]);
    assert!(playback.playing());
}

#[test]
fn timed_node_observation_preserves_offsets_and_pcm_across_partitions() {
    use crate::NodeSignalPhase;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: crate::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap();
    let samples: Vec<_> = (0..4096).map(|frame| [frame as f32 / 8192.0; 2]).collect();
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples.clone()).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let schedule = [
        (0, PlaybackCommand::PlayFrom(300)),
        (11, PlaybackCommand::Seek(100)),
        (23, PlaybackCommand::Pause),
        (29, PlaybackCommand::Play),
    ];
    let mut expected = Vec::new();
    expected.extend_from_slice(&samples[300..311]);
    expected.extend_from_slice(&samples[100..112]);
    expected.extend_from_slice(&[[0.0; 2]; 6]);
    expected.extend_from_slice(&samples[112..1107]);
    for block in [1, 7, 64, 128, 256, 512] {
        let mut playback = Playback::from_prepared(Arc::clone(&revision));
        let mut output = [[0.0; 2]; 1024];
        let mut input = [[0.0; 2]; 1024];
        let mut observed = [[0.0; 2]; 1024];
        let mut next = 0;
        for (index, part) in output.chunks_mut(block).enumerate() {
            let start = index * block;
            let end = start + part.len();
            let mut commands = Vec::new();
            while let Some(&(offset, command)) = schedule.get(next) {
                if offset >= end {
                    break;
                }
                commands.push(TimedTransportCommand {
                    offset: offset - start,
                    command,
                });
                next += 1;
            }
            playback
                .render_with_timed_transport_and_node_observer(
                    part,
                    &commands,
                    &mut |_, phase, offset, frames| {
                        assert!(offset + frames.len() <= end - start);
                        let target = match phase {
                            NodeSignalPhase::Input => &mut input,
                            NodeSignalPhase::Output => &mut observed,
                        };
                        target[start + offset..start + offset + frames.len()]
                            .copy_from_slice(frames);
                    },
                )
                .unwrap();
        }
        assert_eq!(output.as_slice(), expected, "output block {block}");
        assert_eq!(input, output, "input block {block}");
        assert_eq!(observed, output, "observed block {block}");
    }
    let mut playback = Playback::from_prepared(Arc::clone(&revision));
    let mut output = [[9.0; 2]; 4];
    let mut calls = 0;
    let result = playback.render_with_timed_transport_and_node_observer(
        &mut output,
        &[TimedTransportCommand {
            offset: 5,
            command: PlaybackCommand::Play,
        }],
        &mut |_, _, _, _| calls += 1,
    );
    assert_eq!(result, Err(TimedTransportError::InvalidOffset { index: 0 }));
    assert_eq!(calls, 0);
    assert_eq!(output, [[9.0; 2]; 4]);
    assert!(!playback.playing());

    let mut output = [[9.0; 2]; 8];
    let mut calls = 0;
    let result = playback.render_with_timed_transport_and_node_observer(
        &mut output,
        &[
            TimedTransportCommand {
                offset: 0,
                command: PlaybackCommand::PlayFrom(10),
            },
            TimedTransportCommand {
                offset: 4,
                command: PlaybackCommand::PlayFrom(u64::MAX),
            },
        ],
        &mut |_, _, offset, frames| {
            assert!(offset + frames.len() <= 4);
            calls += 1;
        },
    );
    assert!(matches!(
        result,
        Err(TimedTransportError::Action { index: 1, .. })
    ));
    assert!(calls > 0);
    assert_eq!(&output[..4], &samples[10..14]);
    assert_eq!(&output[4..], &[[0.0; 2]; 4]);
    let mut playback = Playback::from_prepared(revision);
    let mut node_calls = 0;
    let mut project_calls = 0;
    let mut node = |_, _, offset, frames: &[Frame]| {
        assert!(offset + frames.len() <= 8);
        node_calls += 1;
    };
    let mut project = |_, offset, frames: &[Frame]| {
        assert!(offset + frames.len() <= 8);
        project_calls += 1;
    };
    let mut tap = crate::ProjectOutputTap::new(&[], &mut project).unwrap();
    playback
        .render_with_timed_transport_and_signal_observers(
            &mut [[0.0; 2]; 8],
            &[
                TimedTransportCommand {
                    offset: 0,
                    command: PlaybackCommand::Play,
                },
                TimedTransportCommand {
                    offset: 4,
                    command: PlaybackCommand::Seek(20),
                },
            ],
            Some(&mut node),
            Some(&mut tap),
        )
        .unwrap();
    assert!(node_calls > 0 && project_calls > 0);
}

#[test]
fn timed_project_taps_keep_pcm_clock_and_offsets_across_partitions() {
    use crate::{ProjectOutputTap, ProjectSignal};
    let (base, samples) = fixture(true);
    let revision = Arc::clone(base.prepared_revision.as_ref().unwrap());
    let track = base.project.tracks()[0]
        .canonical_track_id()
        .unwrap()
        .value();
    let schedule = [
        (0, PlaybackCommand::PlayFrom(300)),
        (11, PlaybackCommand::Seek(100)),
        (23, PlaybackCommand::Pause),
        (29, PlaybackCommand::Play),
    ];
    let mut expected = Vec::new();
    expected.extend_from_slice(&samples[300..311]);
    expected.extend_from_slice(&samples[100..112]);
    expected.extend_from_slice(&[[0.0; 2]; 6]);
    expected.extend_from_slice(&samples[112..211]);
    let mut expected_clock = vec![None; 128];
    for (i, slot) in expected_clock.iter_mut().enumerate() {
        *slot = match i {
            0..=10 => Some(300 + i as u64),
            11..=22 => Some(100 + (i - 11) as u64),
            29..=127 => Some(112 + (i - 29) as u64),
            _ => None,
        };
    }
    for block in [1, 7, 64, 128, 256, 512] {
        let mut playback = Playback::from_prepared(Arc::clone(&revision));
        let mut output = [[0.0; 2]; 128];
        let mut track_output = [[0.0; 2]; 128];
        let mut master = [[0.0; 2]; 128];
        let mut clock = [None; 128];
        let mut next = 0;
        for (index, part) in output.chunks_mut(block).enumerate() {
            let start = index * block;
            let end = start + part.len();
            let mut commands = Vec::new();
            while let Some(&(offset, command)) = schedule.get(next) {
                if offset >= end {
                    break;
                }
                commands.push(TimedTransportCommand {
                    offset: offset - start,
                    command,
                });
                next += 1;
            }
            let mut observe = |signal, offset, frames: &[Frame]| {
                assert!(offset + frames.len() <= end - start);
                let target = match signal {
                    ProjectSignal::Track(_) => &mut track_output,
                    ProjectSignal::Master => &mut master,
                    other => panic!("unexpected {other:?}"),
                };
                target[start + offset..start + offset + frames.len()].copy_from_slice(frames);
            };
            let mut timing = |offset, frame, len, _: &crate::PreparedTempoMap| {
                assert!(offset + len <= end - start);
                for i in 0..len {
                    clock[start + offset + i] = Some(frame + i as u64);
                }
            };
            let tracks = [track];
            let mut tap = ProjectOutputTap::new(&tracks, &mut observe)
                .unwrap()
                .with_timing(&mut timing);
            playback
                .render_with_timed_transport_and_signal_observers(
                    part,
                    &commands,
                    None,
                    Some(&mut tap),
                )
                .unwrap();
        }
        assert_eq!(output.as_slice(), expected, "output block {block}");
        assert_eq!(track_output, output, "track block {block}");
        assert_eq!(master, output, "master block {block}");
        assert_eq!(clock.as_slice(), expected_clock, "clock block {block}");
    }
}

#[test]
fn timed_project_tap_handles_section_split_invalid_schedule_and_reuse() {
    use crate::{ProjectOutputTap, ProjectSignal};
    let (mut playback, samples) = fixture(true);
    playback.apply(PlaybackCommand::Play);
    playback
        .arm_section_once(hex_project::LoopRegion::new(0, 64, false).unwrap())
        .unwrap();
    let mut seen = Vec::new();
    let mut observe = |signal, offset, frames: &[Frame]| {
        if signal == ProjectSignal::Master {
            seen.push((offset, frames.to_vec()));
        }
    };
    let mut tap = ProjectOutputTap::new(&[], &mut observe).unwrap();
    let mut output = [[9.0; 2]; 96];
    let invalid = [TimedTransportCommand {
        offset: 97,
        command: PlaybackCommand::Stop,
    }];
    assert_eq!(
        playback.render_with_timed_transport_and_signal_observers(
            &mut output,
            &invalid,
            None,
            Some(&mut tap)
        ),
        Err(TimedTransportError::InvalidOffset { index: 0 })
    );
    assert_eq!(output, [[9.0; 2]; 96]);
    assert!(playback.playing());
    let commands = [TimedTransportCommand {
        offset: 16,
        command: PlaybackCommand::Play,
    }];
    playback
        .render_with_timed_transport_and_signal_observers(
            &mut output,
            &commands,
            None,
            Some(&mut tap),
        )
        .unwrap();
    assert_eq!(&output[..64], &samples[..64]);
    assert_eq!(&output[64..], &[[0.0; 2]; 32]);
    playback
        .render_with_timed_transport_and_signal_observers(
            &mut [[0.0; 2]; 2],
            &[],
            None,
            Some(&mut tap),
        )
        .unwrap();
    assert_eq!(
        seen.iter()
            .map(|(offset, frames)| (*offset, frames.len()))
            .collect::<Vec<_>>(),
        [(0, 16), (16, 48), (64, 32), (0, 2)]
    );
}
