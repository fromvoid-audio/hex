use super::*;
use crate::{AudioEventEdit, AudioEvents};

fn shaped() -> AudioEventFade {
    AudioEventFade::new(
        0,
        ClipFade::new(128, 0, 8, 16)
            .unwrap()
            .with_shape(2, 4, -300, 400)
            .unwrap()
            .with_curve_positions(250, 700)
            .unwrap(),
    )
    .unwrap()
}
#[test]
fn split_copy_move_keep_source_envelope_and_explicit_trim_reanchors() {
    let fade = shaped();
    let faded = AudioEvents::identity(128)
        .edited(
            AudioEventEdit::SetFade {
                event: 1,
                fade: Some(fade),
            },
            128,
        )
        .unwrap();
    let split = faded.edited(AudioEventEdit::Split { at: 64 }, 128).unwrap();
    assert!(split.events.iter().all(|e| e.fade == Some(fade)));
    let copy = split.copy_range(32, 96);
    assert_eq!(copy.len(), 2);
    assert_eq!(
        copy.iter().map(|e| e.source_start).collect::<Vec<_>>(),
        [32, 64]
    );
    assert!(copy.iter().all(|e| e.fade == Some(fade)));
    let moved = split
        .edited(AudioEventEdit::Move { event: 2, start: 0 }, 128)
        .unwrap();
    assert_eq!(moved.events.len(), 1);
    assert_eq!(moved.events[0].source_start, 64);
    assert_eq!(moved.events[0].fade, Some(fade));
    let trimmed = moved
        .edited(
            AudioEventEdit::Trim {
                event: 2,
                start: 8,
                end: 60,
            },
            128,
        )
        .unwrap();
    let actual = trimmed.events[0].fade.unwrap();
    assert_eq!(actual.source_start(), 72);
    assert_eq!(actual.shape().span(), 52);
    assert_eq!(
        (actual.shape().fade_in(), actual.shape().fade_out()),
        (8, 16)
    );
    assert_eq!(
        (actual.shape().in_curve(), actual.shape().out_curve()),
        (-300, 400)
    );
    assert_eq!(
        (actual.shape().in_position(), actual.shape().out_position()),
        (250, 700)
    );
}
#[test]
fn foreign_stale_overflow_and_uncropped_fade_domains_refuse_atomically() {
    let original = AudioEvents::identity(128);
    let invalid = AudioEventFade::new(32, ClipFade::new(64, 0, 8, 8).unwrap()).unwrap();
    for (event, fade) in [(1, Some(invalid)), (7, Some(shaped())), (7, None)] {
        assert!(
            original
                .edited(AudioEventEdit::SetFade { event, fade }, 128)
                .is_err()
        );
        assert_eq!(original, AudioEvents::identity(128));
    }
    assert!(AudioEventFade::new(u64::MAX, ClipFade::neutral(1).unwrap()).is_err());
    assert!(AudioEventFade::new(0, ClipFade::new(8, 1, 2, 2).unwrap()).is_err());
    assert_eq!(
        original
            .edited(
                AudioEventEdit::SetFade {
                    event: 1,
                    fade: Some(AudioEventFade::new(0, ClipFade::neutral(128).unwrap()).unwrap())
                },
                128
            )
            .unwrap(),
        original
    );
}
#[test]
fn source_clock_projection_maps_every_knot_and_keeps_both_shape_coordinates() {
    let fade = AudioEventFade::new(
        8,
        ClipFade::new(32, 0, 6, 10)
            .unwrap()
            .with_shape(2, 4, 123, -456)
            .unwrap()
            .with_curve_positions(321, 654)
            .unwrap(),
    )
    .unwrap();
    let projected = fade
        .reproject(|at| Ok(if at < 20 { at * 2 } else { 40 + (at - 20) * 3 }))
        .unwrap();
    assert_eq!(
        projected,
        AudioEventFade::new(
            16,
            ClipFade::new(84, 0, 12, 30)
                .unwrap()
                .with_shape(4, 12, 123, -456)
                .unwrap()
                .with_curve_positions(321, 654)
                .unwrap()
        )
        .unwrap()
    );
    assert!(fade.reproject(|_| Ok(0)).is_err());
    let scaled = fade.reproject(|at| Ok(at * 3)).unwrap();
    assert_eq!(scaled.reproject(|at| Ok(at / 3)).unwrap(), fade);
}
