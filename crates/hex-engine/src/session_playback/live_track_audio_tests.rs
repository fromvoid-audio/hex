//! Live track input follows the same signal graph as arrangement audio.
use super::*;
use crate::signal_graph::tests::{fixture, prepare};
use hex_project::{Command, Created};

#[test]
fn paused_input_uses_private_track_return_and_main_history() {
    use crate::{playback::Playback, signal_graph::tests::state};
    for compensated in [false, true] {
        let delays: &[u32] = if compensated { &[7] } else { &[] };
        let mut f = fixture(&[delays, &[]], 512);
        let track = f.tracks[0];
        f.editor
            .execute(Command::Gain { track, gain: 0.5 })
            .unwrap();
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        f.editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        if compensated {
            f.editor
                .execute(Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state: state(3).unwrap(),
                })
                .unwrap();
            f.editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: state(2).unwrap(),
                })
                .unwrap();
        }
        let revision = prepare(&f).unwrap();
        let latency = if compensated { 12 } else { 0 };
        assert_eq!(revision.arrangement().main_latency_frames(), latency);
        let mut arrangement = Playback::from_prepared(Arc::clone(&revision));
        let mut uninterrupted = Playback::from_prepared(Arc::clone(&revision));
        let mut monitor = PausedTrackMonitor::prepare_on_control(
            Arc::clone(&revision),
            track,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
        )
        .unwrap();
        for playback in [&mut arrangement, &mut uninterrupted] {
            playback.apply(crate::PlaybackCommand::Play);
            playback.render(&mut [[0.0; 2]; 4]).unwrap();
        }
        arrangement.apply(crate::PlaybackCommand::Pause);
        let mut monitored = [[0.0; 2]; 32];
        arrangement
            .render_with_paused_track_monitor(&mut monitor, &[[0.8, -0.4]], &mut monitored[..1])
            .unwrap();
        arrangement
            .render_with_paused_track_monitor(&mut monitor, &[[0.0; 2]; 31], &mut monitored[1..])
            .unwrap();
        for (frame, sample) in monitored.iter().enumerate() {
            let expected: Frame = if frame == latency as usize {
                [0.6, -0.3]
            } else {
                [0.0; 2]
            };
            assert!(
                (sample[0] - expected[0]).abs() < 1e-6 && (sample[1] - expected[1]).abs() < 1e-6,
                "latency {latency}, frame {frame}: {sample:?} != {expected:?}"
            );
        }
        assert_eq!(arrangement.position(), uninterrupted.position());
        arrangement.apply(crate::PlaybackCommand::Play);
        let mut actual = [[0.0; 2]; 32];
        let mut expected = actual;
        arrangement.render(&mut actual).unwrap();
        uninterrupted.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        if compensated {
            assert!(expected.iter().any(|sample| *sample != [0.0; 2]));
        }
    }
}

#[test]
fn paused_input_refuses_wrong_revision_cursor_bounds_and_reservation() {
    let f = fixture(&[&[7]], 512);
    let revision = prepare(&f).unwrap();
    let track = f.tracks[0];
    assert!(matches!(
        PausedTrackMonitor::prepare_on_control(Arc::clone(&revision), track, 0),
        Err(SessionPlaybackError::Allocation)
    ));
    let mut monitor = PausedTrackMonitor::prepare_on_control(
        Arc::clone(&revision),
        track,
        MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
    )
    .unwrap();
    let mut arrangement = SessionPlayback::new(revision).unwrap();
    let same_content = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    let mut first = [[0.0; 2]; 1];
    monitor
        .render(&arrangement, &[[1.0, 1.0]], &mut first)
        .unwrap();
    assert_eq!(first, [[0.0; 2]; 1]);
    let mut refused = [[9.0; 2]; 257];
    assert!(
        monitor
            .render(&same_content, &[[0.0; 2]; 8], &mut refused[..8])
            .is_err()
    );
    assert_eq!(refused[..8], [[0.0; 2]; 8]);
    assert!(
        monitor
            .render(&arrangement, &[[0.0; 2]; 257], &mut refused)
            .is_err()
    );
    assert!(
        monitor
            .render(&arrangement, &[[f32::NAN, 0.0]], &mut refused[..1])
            .is_err()
    );
    assert_eq!(refused, [[0.0; 2]; 257]);
    let mut drain = [[0.0; 2]; 8];
    monitor
        .render(&arrangement, &[[0.0; 2]; 8], &mut drain)
        .unwrap();
    assert_eq!(drain[6], [1.0, 1.0]);
    arrangement.apply(SessionPlaybackCommand::Seek(10)).unwrap();
    assert!(
        monitor
            .render(&arrangement, &[[0.0; 2]], &mut first)
            .is_err()
    );
    assert_eq!(first, [[0.0; 2]; 1]);
    assert_eq!(arrangement.arrangement_frame(), 10);
}

#[test]
fn live_track_audio_matches_arrangement_through_effects_fader_and_send() {
    for delays in [&[][..], &[7][..]] {
        for partition in [1, 17, 256] {
            let mut f = fixture(&[delays, &[]], 512);
            let track = f.tracks[0];
            f.editor
                .execute(Command::Gain { track, gain: 0.5 })
                .unwrap();
            let Created::ReturnTrack(bus) = f
                .editor
                .execute(Command::AddReturnTrack {
                    name: "return".into(),
                })
                .unwrap()
            else {
                panic!("return")
            };
            f.editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain: 0.5,
                })
                .unwrap();
            // The second route stays audible and must not receive the live input.
            let mut reference = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            *f.media.values_mut().next().unwrap() =
                crate::Audio::new(48000, vec![[0.0; 2]; 512]).unwrap();
            let mut live = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            reference.apply(SessionPlaybackCommand::Play).unwrap();
            live.apply(SessionPlaybackCommand::Play).unwrap();
            let mut expected = vec![[0.0; 2]; 512];
            let mut actual = expected.clone();
            for ((out, expected), input) in actual
                .chunks_mut(partition)
                .zip(expected.chunks_mut(partition))
                .zip(f.samples.chunks(partition))
            {
                reference.render(expected).unwrap();
                live.render_with_live_track_audio(
                    out,
                    track,
                    input,
                    None,
                    None,
                    None,
                    &mut hex_lattice::RefuseForeign,
                )
                .unwrap();
            }
            assert_eq!(actual, expected, "delay {delays:?}, partition {partition}");
            assert!(actual.iter().any(|frame| frame[0] != 0.0));
        }
    }
}

#[test]
fn invalid_live_blocks_are_silent_and_do_not_advance() {
    let mut f = fixture(&[&[]], 512);
    let mut playback = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    let track = f.tracks[0];
    let Created::Track(missing) = f
        .editor
        .execute(Command::AddTrack {
            name: "missing".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut out = [[1.0; 2]; 2];
    assert!(
        playback
            .render_with_live_track_audio(
                &mut out,
                track,
                &[[0.5; 2]; 2],
                None,
                None,
                None,
                &mut hex_lattice::RefuseForeign
            )
            .is_err()
    );
    assert_eq!(out, [[0.0; 2]; 2]);
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(
        playback
            .render_with_live_track_audio(
                &mut out,
                missing,
                &[[0.5; 2]; 2],
                None,
                None,
                None,
                &mut hex_lattice::RefuseForeign
            )
            .is_err()
    );
    assert_eq!(out, [[0.0; 2]; 2]);
    let mut oversized = vec![[1.0; 2]; MAX_OUTPUT_CAPTURE_FRAMES + 1];
    assert!(
        playback
            .render_with_live_track_audio(
                &mut oversized,
                track,
                &vec![[0.5; 2]; MAX_OUTPUT_CAPTURE_FRAMES + 1],
                None,
                None,
                None,
                &mut hex_lattice::RefuseForeign
            )
            .is_err()
    );
    assert!(oversized.iter().all(|frame| *frame == [0.0; 2]));
    for input in [&[[0.5; 2]][..], &[[f32::NAN; 2]; 2][..]] {
        out.fill([1.0; 2]);
        assert!(
            playback
                .render_with_live_track_audio(
                    &mut out,
                    track,
                    input,
                    None,
                    None,
                    None,
                    &mut hex_lattice::RefuseForeign
                )
                .is_err()
        );
        assert_eq!(out, [[0.0; 2]; 2]);
        assert_eq!(playback.performance_frame(), 0);
    }
}

#[test]
fn live_input_offsets_survive_an_arrangement_loop_inside_a_block() {
    let mut f = fixture(&[&[]], 512);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 400,
            enabled: true,
        })
        .unwrap();
    let track = f.tracks[0];
    let mut reference = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    *f.media.values_mut().next().unwrap() = crate::Audio::new(48_000, vec![[0.0; 2]; 512]).unwrap();
    let mut live = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    live.apply(SessionPlaybackCommand::Play).unwrap();
    let input: Vec<_> = (0..512).map(|index| f.samples[index % 400]).collect();
    let mut expected = [[0.0; 2]; 512];
    let mut actual = expected;
    for ((out, expected), source) in actual
        .chunks_mut(256)
        .zip(expected.chunks_mut(256))
        .zip(input.chunks(256))
    {
        reference.render(expected).unwrap();
        live.render_with_live_track_audio(
            out,
            track,
            source,
            None,
            None,
            None,
            &mut hex_lattice::RefuseForeign,
        )
        .unwrap();
    }
    assert_eq!(actual, expected);
    assert_eq!(actual[400], f.samples[0]);
}

#[test]
fn live_audio_follows_target_track_through_its_group() {
    let mut f = fixture(&[&[7]], 512);
    let track = f.tracks[0];
    let Created::Track(group) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    f.editor
        .execute(Command::Gain {
            track: group,
            gain: 0.5,
        })
        .unwrap();
    let mut reference = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    *f.media.values_mut().next().unwrap() = crate::Audio::new(48_000, vec![[0.0; 2]; 512]).unwrap();
    let mut live = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    live.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 256];
    let mut actual = expected;
    reference.render(&mut expected).unwrap();
    live.render_with_live_track_audio(
        &mut actual,
        track,
        &f.samples[..256],
        None,
        None,
        None,
        &mut hex_lattice::RefuseForeign,
    )
    .unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
}

#[test]
fn project_capture_and_live_monitor_are_refused_together() {
    let f = fixture(&[&[]], 512);
    let track = f.tracks[0];
    let mut playback = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let ids = [track.value()];
    let mut observed = false;
    let mut observe = |_, _, _: &[Frame]| observed = true;
    let mut tap = ProjectOutputTap::new(&ids, &mut observe).unwrap();
    let mut output = [[1.0; 2]; 2];
    assert!(
        playback
            .render_with_live_track_audio(
                &mut output,
                track,
                &[[0.5; 2]; 2],
                None,
                None,
                Some(&mut tap),
                &mut hex_lattice::RefuseForeign
            )
            .is_err()
    );
    assert_eq!(output, [[0.0; 2]; 2]);
    assert_eq!(playback.performance_frame(), 0);
    assert!(!observed);
}
