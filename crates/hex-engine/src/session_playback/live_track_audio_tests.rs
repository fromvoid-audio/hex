//! Live track input follows the same signal graph as arrangement audio.
use super::*;
use crate::signal_graph::tests::{fixture, prepare};
use hex_project::{Command, Created};

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
    let mut oversized = [[1.0; 2]; 257];
    assert!(
        playback
            .render_with_live_track_audio(
                &mut oversized,
                track,
                &[[0.5; 2]; 257],
                None,
                None,
                None,
                &mut hex_lattice::RefuseForeign
            )
            .is_err()
    );
    assert_eq!(oversized, [[0.0; 2]; 257]);
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
