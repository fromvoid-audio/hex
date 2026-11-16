use super::fade_frames;
use crate::playback::{
    Playback, PlaybackAction, PlaybackActionError, PlaybackCommand, TransportFadeCompletion,
};
use crate::{Audio, Frame, PreparedRevision, ProjectOutputTap, ProjectSignal, SineTable};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
const F: usize = 128;
const CLIP: u64 = 20_000;

fn dc_project() -> (Project, BTreeMap<hex_project::AssetId, Audio>) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "dc".into(),
            path: "dc.wav".into(),
            sample_rate: RATE,
            frames: CLIP,
        })
        .unwrap()
    else {
        panic!("import result");
    };
    let media = BTreeMap::from([(
        asset,
        Audio::new(RATE, vec![[1.0, -1.0]; CLIP as usize]).unwrap(),
    )]);
    (editor.project().clone(), media)
}

fn session_playback() -> Playback {
    let (project, media) = dc_project();
    let revision =
        PreparedRevision::prepare(&project, &media, Arc::new(SineTable::new()), None).unwrap();
    let playback = Playback::from_prepared(Arc::new(revision));
    assert!(playback.session_runtime.is_some());
    playback
}

fn linear_playback() -> Playback {
    let (project, media) = dc_project();
    Playback::new(crate::compile::project(&project, &media).unwrap())
}

fn declicked(playback: &mut Playback, command: PlaybackCommand) {
    playback
        .try_apply_declicked_action(PlaybackAction::Transport(command))
        .unwrap();
}

/// Render `frames` in blocks of at most `block`.
fn render(playback: &mut Playback, block: usize, frames: usize, output: &mut Vec<Frame>) {
    let mut left = frames;
    while left > 0 {
        let len = left.min(block);
        let mut chunk = vec![[0.0; 2]; len];
        playback.render(&mut chunk).unwrap();
        output.extend_from_slice(&chunk);
        left -= len;
    }
}

/// The positions and fade outcomes observed by `pause_resume_stop`.
struct Observed {
    paused: u64,
    pause_end: Option<TransportFadeCompletion>,
    stop_command: u64,
    stopped: u64,
    stop_end: Option<TransportFadeCompletion>,
}

/// Play 1000 frames, Pause, wait, resume at 2000, Stop at 3000, then 1000
/// frames of silence. Returns the output and the observed positions.
fn pause_resume_stop(mut playback: Playback, block: usize) -> (Vec<Frame>, Observed) {
    let mut output = Vec::new();
    declicked(&mut playback, PlaybackCommand::Play);
    render(&mut playback, block, 1000, &mut output);
    declicked(&mut playback, PlaybackCommand::Pause);
    assert!(playback.playing(), "the fade keeps the transport running");
    render(&mut playback, block, 1000, &mut output);
    assert!(!playback.playing());
    assert_eq!(playback.pending_transport_fade(), None);
    let paused = playback.position();
    let pause_end = playback.take_transport_fade_completion();
    declicked(&mut playback, PlaybackCommand::Play);
    render(&mut playback, block, 1000, &mut output);
    let stop_command = playback.position();
    declicked(&mut playback, PlaybackCommand::Stop);
    assert_eq!(playback.take_transport_fade_completion(), None);
    render(&mut playback, block, 1000, &mut output);
    assert!(!playback.playing());
    let observed = Observed {
        paused,
        pause_end,
        stop_command,
        stopped: playback.position(),
        stop_end: playback.take_transport_fade_completion(),
    };
    assert_eq!(
        playback.take_transport_fade_completion(),
        None,
        "reported once"
    );
    (output, observed)
}

fn largest_step(output: &[Frame]) -> f32 {
    output
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn fade_length_scales_with_rate_and_is_at_least_one_frame() {
    assert_eq!(fade_frames(48_000), 128);
    assert_eq!(fade_frames(96_000), 256);
    assert_eq!(fade_frames(44_100), 118);
    assert_eq!(fade_frames(8_000), 21);
    assert_eq!(fade_frames(1), 1);
    assert_eq!(fade_frames(0), 1);
}

#[test]
fn render_failure_after_fade_clears_the_complete_output_buffer() {
    let mut playback = linear_playback();
    declicked(&mut playback, PlaybackCommand::Play);
    declicked(&mut playback, PlaybackCommand::Pause);
    let mut output = [[0.5; 2]; F + 8];
    let result = playback.render_declicked(&mut output, |_, part, offset| {
        part.fill([0.75; 2]);
        if offset == 0 {
            Ok(())
        } else {
            Err(crate::Invalid("injected render refusal"))
        }
    });
    assert!(result.is_err());
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn pause_stop_and_resume_ramp_a_dc_clip_identically_for_every_block_size() {
    for (name, make) in [
        ("session", session_playback as fn() -> Playback),
        ("linear", linear_playback),
    ] {
        let (reference, observed) = pause_resume_stop(make(), 1);
        assert_eq!(
            observed.paused,
            1000 + F as u64,
            "{name}: pause reports its audible end"
        );
        assert_eq!(
            observed.pause_end,
            Some(TransportFadeCompletion {
                action: PlaybackAction::Transport(PlaybackCommand::Pause),
                audible_end_frame: 1000 + F as u64,
                applied: true,
            }),
            "{name}"
        );
        // Stop keeps its reset to frame zero; only the fade outcome reports
        // the audible end, the Stop command frame plus F.
        assert_eq!(observed.stopped, 0, "{name}");
        assert_eq!(
            observed.stop_end,
            Some(TransportFadeCompletion {
                action: PlaybackAction::Transport(PlaybackCommand::Stop),
                audible_end_frame: observed.stop_command + F as u64,
                applied: true,
            }),
            "{name}"
        );
        // Frame zero keeps its transient; every later step is one ramp step.
        assert_eq!(reference[0], [1.0, -1.0], "{name}");
        assert!(
            largest_step(&reference[1..]) <= 1.0 / F as f32 + 1e-6,
            "{name}: step {}",
            largest_step(&reference[1..])
        );
        assert_eq!(reference[999], [1.0, -1.0], "{name}");
        assert_eq!(
            reference[1000 + F - 1],
            [0.0, 0.0],
            "{name}: fade ends at zero"
        );
        assert!(
            reference[1000 + F..2000].iter().all(|f| *f == [0.0, 0.0]),
            "{name}: paused output is silent"
        );
        assert_eq!(
            reference[2000][0],
            1.0 / F as f32,
            "{name}: resume ramps in"
        );
        assert_eq!(reference[2000 + F - 1], [1.0, -1.0], "{name}");
        assert!(
            reference[3000 + F..].iter().all(|f| *f == [0.0, 0.0]),
            "{name}: stopped output is silent"
        );
        for block in [7, 64, 512] {
            let (output, _) = pause_resume_stop(make(), block);
            assert_eq!(output, reference, "{name}: block size {block}");
        }
    }
}

#[test]
fn a_command_during_the_fade_completes_the_pending_stop_first() {
    for make in [session_playback as fn() -> Playback, linear_playback] {
        let mut playback = make();
        let mut output = Vec::new();
        declicked(&mut playback, PlaybackCommand::Play);
        render(&mut playback, 64, 1000, &mut output);
        declicked(&mut playback, PlaybackCommand::Pause);
        render(&mut playback, 64, F / 2, &mut output);
        // Stop arrives mid-fade: the Pause applies at once, then Stop, which
        // has nothing left to fade.
        declicked(&mut playback, PlaybackCommand::Stop);
        assert_eq!(playback.pending_transport_fade(), None);
        assert!(!playback.playing());
        assert_eq!(playback.position(), 0);
        // The early-completed Pause reports where its truncated fade ended.
        assert_eq!(
            playback.take_transport_fade_completion(),
            Some(TransportFadeCompletion {
                action: PlaybackAction::Transport(PlaybackCommand::Pause),
                audible_end_frame: 1000 + F as u64 / 2,
                applied: true,
            })
        );

        // Play from Stop keeps the first transient.
        declicked(&mut playback, PlaybackCommand::Play);
        let mut first = Vec::new();
        render(&mut playback, 64, 1, &mut first);
        assert_eq!(first, [[1.0, -1.0]]);
        declicked(&mut playback, PlaybackCommand::Stop);
        render(&mut playback, 64, F, &mut output);

        // An immediate action also completes a pending fade first.
        declicked(&mut playback, PlaybackCommand::Play);
        render(&mut playback, 64, 1000, &mut output);
        declicked(&mut playback, PlaybackCommand::Pause);
        render(&mut playback, 64, 10, &mut output);
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Seek(500)))
            .unwrap();
        assert_eq!(playback.pending_transport_fade(), None);
        assert!(!playback.playing());
        assert_eq!(playback.position(), 500);
    }
}

/// Play 1000 frames, Seek to 5000 while playing, then 1000 more frames.
/// Returns the output, the position mid-fade and at the end, and the
/// completion.
fn seek_while_playing(
    mut playback: Playback,
    block: usize,
) -> (Vec<Frame>, u64, u64, Option<TransportFadeCompletion>) {
    let mut output = Vec::new();
    declicked(&mut playback, PlaybackCommand::Play);
    render(&mut playback, block, 1000, &mut output);
    declicked(&mut playback, PlaybackCommand::Seek(5000));
    assert!(playback.playing());
    assert_eq!(
        playback.pending_transport_fade(),
        Some(PlaybackAction::Transport(PlaybackCommand::Seek(5000)))
    );
    render(&mut playback, block, F / 2, &mut output);
    let mid_fade = playback.position();
    render(&mut playback, block, 1000 - F / 2, &mut output);
    assert!(playback.playing());
    (
        output,
        mid_fade,
        playback.position(),
        playback.take_transport_fade_completion(),
    )
}

#[test]
fn seek_while_playing_fades_out_jumps_and_ramps_in_for_every_block_size() {
    for (name, make) in [
        ("session", session_playback as fn() -> Playback),
        ("linear", linear_playback),
    ] {
        let (reference, mid_fade, end, completion) = seek_while_playing(make(), 1);
        // The old position runs on through the fade; the target follows it.
        assert_eq!(mid_fade, 1000 + F as u64 / 2, "{name}");
        assert_eq!(end, 5000 + 1000 - F as u64, "{name}: target after the fade");
        assert_eq!(
            completion,
            Some(TransportFadeCompletion {
                action: PlaybackAction::Transport(PlaybackCommand::Seek(5000)),
                audible_end_frame: 1000 + F as u64,
                applied: true,
            }),
            "{name}"
        );
        let step = largest_step(&reference[1..]);
        assert!(step <= 1.0 / F as f32 + 1e-6, "{name}: step {step}");
        assert_eq!(reference[999], [1.0, -1.0], "{name}");
        assert_eq!(
            reference[1000 + F - 1],
            [0.0, 0.0],
            "{name}: fade ends at zero"
        );
        assert_eq!(reference[1000 + F][0], 1.0 / F as f32, "{name}: ramp in");
        assert_eq!(reference[1000 + 2 * F - 1], [1.0, -1.0], "{name}");
        for block in [7, 64, 100, 512] {
            let (output, ..) = seek_while_playing(make(), block);
            assert_eq!(output, reference, "{name}: block size {block}");
        }
    }
}

#[test]
fn session_seek_while_playing_declicks_across_blocks() {
    assert_seek_action(PlaybackAction::Session(
        crate::SessionPlaybackCommand::Seek(5000),
    ));
}

#[test]
fn play_from_while_playing_declicks_across_blocks() {
    assert_seek_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(5000)));
}

fn assert_seek_action(action: PlaybackAction) {
    let mut reference = None;
    for block in [1, 7, 64, 100, 512] {
        let mut playback = session_playback();
        let mut output = Vec::new();
        declicked(&mut playback, PlaybackCommand::Play);
        render(&mut playback, block, 1000, &mut output);
        playback.try_apply_declicked_action(action).unwrap();
        assert_eq!(playback.pending_transport_fade(), Some(action));
        render(&mut playback, block, 1000, &mut output);
        assert!(playback.playing());
        assert_eq!(playback.position(), 5000 + 1000 - F as u64);
        assert_eq!(
            playback.take_transport_fade_completion(),
            Some(TransportFadeCompletion {
                action,
                audible_end_frame: 1000 + F as u64,
                applied: true,
            })
        );
        let step = largest_step(&output[1..]);
        assert!(step <= 1.0 / F as f32 + 1e-6, "{action:?}: step {step}");
        assert_eq!(output[1000 + F - 1], [0.0; 2]);
        assert_eq!(output[1000 + F][0], 1.0 / F as f32);
        match &reference {
            None => reference = Some(output),
            Some(reference) => assert_eq!(&output, reference, "{action:?}, block {block}"),
        }
    }
}

#[test]
fn a_paused_seek_stays_immediate() {
    let mut playback = session_playback();
    declicked(&mut playback, PlaybackCommand::Seek(700));
    assert_eq!(playback.pending_transport_fade(), None);
    assert_eq!(playback.position(), 700);
}

#[test]
fn session_seek_and_play_from_stopped_are_immediate_and_invalid_play_from_refuses() {
    let mut playback = session_playback();
    let seek = PlaybackAction::Session(crate::SessionPlaybackCommand::Seek(700));
    playback.try_apply_declicked_action(seek).unwrap();
    assert_eq!(playback.position(), 700);
    assert_eq!(playback.pending_transport_fade(), None);
    let play = PlaybackAction::Transport(PlaybackCommand::PlayFrom(900));
    playback.try_apply_declicked_action(play).unwrap();
    assert!(playback.playing());
    assert_eq!(playback.position(), 900);
    assert_eq!(playback.pending_transport_fade(), None);
    let invalid = PlaybackAction::Transport(PlaybackCommand::PlayFrom(u64::MAX));
    assert!(playback.try_apply_declicked_action(invalid).is_err());
    assert!(playback.playing());
    assert_eq!(playback.position(), 900);
    assert_eq!(playback.pending_transport_fade(), None);
    let mut linear = linear_playback();
    declicked(&mut linear, PlaybackCommand::Play);
    assert_eq!(
        linear.try_apply_declicked_action(seek),
        Err(PlaybackActionError::SessionNotPrepared)
    );
    assert_eq!(linear.pending_transport_fade(), None);
}

#[test]
fn immediate_actions_and_stopped_transports_are_not_deferred() {
    let mut playback = session_playback();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Pause))
        .unwrap();
    assert!(!playback.playing(), "the compatibility path still cuts");
    declicked(&mut playback, PlaybackCommand::Stop);
    assert_eq!(playback.pending_transport_fade(), None);
    assert_eq!(playback.position(), 0);

    // A Session Pause without a Session runtime is refused when requested.
    let mut linear = linear_playback();
    declicked(&mut linear, PlaybackCommand::Play);
    assert_eq!(
        linear.try_apply_declicked_action(PlaybackAction::Session(
            crate::SessionPlaybackCommand::Pause
        )),
        Err(PlaybackActionError::SessionNotPrepared)
    );
    assert!(linear.playing());
    assert_eq!(linear.pending_transport_fade(), None);
}

#[test]
fn interleaved_immediate_transport_actions_match_sample_oracle_across_blocks() {
    let (project, mut media) = dc_project();
    let asset = *media.keys().next().unwrap();
    // Every source position has a distinct, exactly representable amplitude.
    let samples: Vec<Frame> = (0..CLIP)
        .map(|frame| {
            let value = frame as f32 / 32768.0;
            [value, -value]
        })
        .collect();
    media.insert(asset, Audio::new(RATE, samples.clone()).unwrap());
    let linear = Arc::new(crate::compile::project(&project, &media).unwrap());
    let session = Arc::new(
        PreparedRevision::prepare(&project, &media, Arc::new(SineTable::new()), None).unwrap(),
    );
    let actions = [
        (0, PlaybackCommand::PlayFrom(300)),
        (11, PlaybackCommand::Seek(100)),
        (23, PlaybackCommand::Stop),
        (29, PlaybackCommand::PlayFrom(600)),
        (41, PlaybackCommand::Pause),
        (47, PlaybackCommand::Play),
    ];
    let mut expected = Vec::new();
    expected.extend_from_slice(&samples[300..311]);
    expected.extend_from_slice(&samples[100..112]);
    expected.extend_from_slice(&[[0.0; 2]; 6]);
    expected.extend_from_slice(&samples[600..612]);
    expected.extend_from_slice(&[[0.0; 2]; 6]);
    expected.extend_from_slice(&samples[612..1589]);
    assert_eq!(expected.len(), 1024);

    for prepared in [false, true] {
        for block in [1, 7, 64, 128, 256, 512] {
            let mut playback = if prepared {
                Playback::from_prepared(Arc::clone(&session))
            } else {
                Playback::from_shared(Arc::clone(&linear))
            };
            let mut actual = [[0.0; 2]; 1024];
            let mut cursor = 0;
            let mut next_action = 0;
            while cursor < actual.len() {
                let block_end = (cursor + block).min(actual.len());
                while cursor < block_end {
                    if actions
                        .get(next_action)
                        .is_some_and(|(at, _)| *at == cursor)
                    {
                        playback
                            .try_apply_action(PlaybackAction::Transport(actions[next_action].1))
                            .unwrap();
                        next_action += 1;
                    }
                    let end = actions
                        .get(next_action)
                        .map_or(block_end, |(at, _)| block_end.min(*at));
                    playback.render(&mut actual[cursor..end]).unwrap();
                    cursor = end;
                }
            }
            assert_eq!(next_action, actions.len());
            assert!(playback.playing());
            assert_eq!(playback.position(), 1589);
            assert_eq!(
                actual.as_slice(),
                expected,
                "prepared={prepared}, block={block}"
            );
        }
    }
}

#[test]
fn project_taps_observe_each_render_part_at_its_callback_offset() {
    let mut playback = session_playback();
    declicked(&mut playback, PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 512];
    playback.render(&mut output).unwrap();
    declicked(&mut playback, PlaybackCommand::Pause);
    let mut seen = Vec::new();
    let mut observe = |signal: ProjectSignal, offset: usize, frames: &[Frame]| {
        if signal == ProjectSignal::Master {
            seen.push((offset, frames.len()));
        }
    };
    let mut tap = ProjectOutputTap::new(&[], &mut observe).unwrap();
    playback
        .render_with_signal_observers(&mut output, None, Some(&mut tap))
        .unwrap();
    assert_eq!(seen, [(0, F), (F, 512 - F)]);
}

#[test]
fn project_tap_offsets_compose_and_restore_across_nested_declick_parts() {
    let mut playback = session_playback();
    declicked(&mut playback, PlaybackCommand::Play);
    playback.render(&mut [[0.0; 2]; 512]).unwrap();
    declicked(&mut playback, PlaybackCommand::Pause);
    let mut seen = Vec::new();
    let mut observe = |signal: ProjectSignal, offset: usize, frames: &[Frame]| {
        if signal == ProjectSignal::Master {
            seen.push((offset, frames.len()));
        }
    };
    let mut tap = ProjectOutputTap::new(&[], &mut observe).unwrap();
    crate::playback::with_part_offset(Some(&mut tap), 31, |tap| {
        playback
            .render_with_signal_observers(&mut [[0.0; 2]; 512], None, tap)
            .unwrap();
    });
    playback
        .render_with_signal_observers(&mut [[0.0; 2]; 2], None, Some(&mut tap))
        .unwrap();
    assert_eq!(seen, [(31, F), (31 + F, 512 - F), (0, 2)]);
}
