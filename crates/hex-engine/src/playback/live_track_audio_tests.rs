use super::*;
use crate::signal_graph::tests::{fixture, prepare};

#[test]
fn live_audio_matches_arrangement_across_large_blocks_and_transport_splits() {
    for mode in 0..4 {
        let mut f = fixture(&[&[7]], 1024);
        let track = f.tracks[0];
        for (index, sample) in f.samples.iter_mut().enumerate() {
            *sample = [(index % 31) as f32 / 128.0, -((index % 17) as f32) / 128.0];
        }
        *f.media.values_mut().next().unwrap() =
            crate::Audio::new(48_000, f.samples.clone()).unwrap();
        let mut reference = Playback::try_from_prepared_with_session(prepare(&f).unwrap()).unwrap();
        *f.media.values_mut().next().unwrap() =
            crate::Audio::new(48_000, vec![[0.0; 2]; 1024]).unwrap();
        let mut live = Playback::try_from_prepared_with_session(prepare(&f).unwrap()).unwrap();
        for playback in [&mut reference, &mut live] {
            playback
                .try_apply_declicked_action(PlaybackAction::Transport(PlaybackCommand::Play))
                .unwrap();
            if mode == 1 {
                playback
                    .try_apply_declicked_action(PlaybackAction::Transport(PlaybackCommand::Pause))
                    .unwrap();
            } else if mode == 2 {
                playback.transport_fade.begin_in(128);
            } else if mode == 3 {
                playback.transport_fade.section_end = Some(150);
            }
        }
        let mut expected = [[0.0; 2]; 512];
        let mut actual = expected;
        reference
            .render_with_foreign_processors(
                &mut expected,
                None,
                None,
                None,
                &mut hex_lattice::RefuseForeign,
            )
            .unwrap();
        live.render_with_live_track_audio(
            &mut actual,
            track,
            &f.samples[..512],
            None,
            None,
            None,
            &mut hex_lattice::RefuseForeign,
        )
        .unwrap();
        assert_eq!(actual, expected, "mode {mode}");
        assert_eq!(live.position(), reference.position());
        assert_eq!(live.playing(), reference.playing());
    }
}

#[test]
fn invalid_live_input_is_rejected_before_playback_advances() {
    let f = fixture(&[&[]], 512);
    let track = f.tracks[0];
    let mut live = Playback::try_from_prepared_with_session(prepare(&f).unwrap()).unwrap();
    live.apply(PlaybackCommand::Play);
    let mut output = [[1.0; 2]; 512];
    let mut input = [[0.0; 2]; 512];
    input[511][0] = f32::NAN;
    assert!(
        live.render_with_live_track_audio(
            &mut output,
            track,
            &input,
            None,
            None,
            None,
            &mut hex_lattice::RefuseForeign
        )
        .is_err()
    );
    assert_eq!(live.position(), 0);
    assert_eq!(output, [[0.0; 2]; 512]);
}

#[test]
fn live_audio_still_plays_across_a_declicked_seek() {
    let mut f = fixture(&[&[]], 1024);
    let track = f.tracks[0];
    for (i, frame) in f.samples.iter_mut().enumerate() {
        *frame = [(i % 31) as f32 / 128.0, 0.0];
    }
    *f.media.values_mut().next().unwrap() =
        crate::Audio::new(48_000, vec![[0.0; 2]; 1024]).unwrap();
    let revision = prepare(&f).unwrap();
    let mut whole = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
    let mut partitioned = Playback::try_from_prepared_with_session(revision).unwrap();
    for playback in [&mut whole, &mut partitioned] {
        playback.apply(PlaybackCommand::Play);
        playback
            .try_apply_declicked_action(PlaybackAction::Transport(PlaybackCommand::Seek(32)))
            .unwrap();
    }
    let mut expected = [[0.0; 2]; 512];
    let mut actual = expected;
    whole
        .render_with_live_track_audio(
            &mut expected,
            track,
            &f.samples[..512],
            None,
            None,
            None,
            &mut hex_lattice::RefuseForeign,
        )
        .unwrap();
    for (out, input) in actual.chunks_mut(64).zip(f.samples[..512].chunks(64)) {
        partitioned
            .render_with_live_track_audio(
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
    assert_eq!(actual, expected);
    assert!(actual[256..].iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn playback_refuses_project_capture_with_live_input_before_advancing() {
    let f = fixture(&[&[]], 512);
    let track = f.tracks[0];
    let mut live = Playback::try_from_prepared_with_session(prepare(&f).unwrap()).unwrap();
    live.apply(PlaybackCommand::Play);
    let ids = [track.value()];
    let mut observed = false;
    let mut observe = |_, _, _: &[Frame]| observed = true;
    let mut tap = crate::ProjectOutputTap::new(&ids, &mut observe).unwrap();
    let mut output = [[1.0; 2]; 512];
    assert!(
        live.render_with_live_track_audio(
            &mut output,
            track,
            &[[0.5; 2]; 512],
            None,
            None,
            Some(&mut tap),
            &mut hex_lattice::RefuseForeign
        )
        .is_err()
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(live.position(), 0);
    assert!(!observed);
}
