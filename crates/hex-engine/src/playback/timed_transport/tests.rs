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
