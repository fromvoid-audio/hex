use super::rectangle::*;
use gpui::Modifiers;
use std::cell::Cell;

#[test]
fn narrow_edges_keep_the_move_body_and_touching_seams_take_priority() {
    let clips = RectangleEdges {
        width: 4.0,
        minimum_body: 1.0,
    };
    assert_eq!(clips.handle_width(3.0), 1.0);
    assert_eq!(clips.hit(11.5, 10.0, 13.0), Some(RectanglePart::Body));
    assert_eq!(clips.hit(10.5, 10.0, 13.0), Some(RectanglePart::Leading));
    assert_eq!(clips.hit(12.5, 10.0, 13.0), Some(RectanglePart::Trailing));
    assert_eq!(clips.hit(0.0, -f32::MAX, f32::MAX), None);
    let sections = RectangleEdges {
        width: 5.0,
        minimum_body: 0.0,
    };
    assert_eq!(sections.hit(15.0, 10.0, 20.0), Some(RectanglePart::Leading));
    assert_eq!(
        sections.hit(15.1, 10.0, 20.0),
        Some(RectanglePart::Trailing)
    );
    assert!(RectangleEdges::seam_hit(20.9, 20.0, 1.0));
    assert!(!RectangleEdges::seam_hit(21.1, 20.0, 1.0));
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(clips.hit(invalid, 0.0, 10.0), None);
        assert_eq!(clips.hit(1.0, invalid, 10.0), None);
        assert_eq!(clips.handle_width(invalid), 0.0);
        assert!(!RectangleEdges::seam_hit(invalid, 20.0, 1.0));
    }
}

#[test]
fn capture_keeps_exact_origin_and_part_across_repaints_and_consumes_only_its_key() {
    let mut held = RectangleCapture::begin((3, 42), RectanglePart::Trailing, 790.0, true);
    let replaced_callbacks = held;
    assert!(RectangleCapture::take_matching(&mut held, (3, 42), RectanglePart::Leading).is_none());
    assert!(RectangleCapture::take_matching(&mut held, (4, 42), RectanglePart::Trailing).is_none());
    assert_eq!(held, replaced_callbacks);
    let capture =
        RectangleCapture::take_matching(&mut held, (3, 42), RectanglePart::Trailing).unwrap();
    let shared = Cell::new(Some(capture));
    assert_eq!(rectangle_pointer(&shared, 725.0), Some(725.0));
    let change = shared.get().unwrap().intent(RectanglePhase::Change);
    assert_eq!(change.origin_x, 790.0);
    assert_eq!(change.delta_x, -65.0);
    assert_eq!(change.part, RectanglePart::Trailing);
    let final_intent = shared.take().unwrap().finish(720.0).unwrap();
    assert_eq!(final_intent.phase, RectanglePhase::Commit);
    assert_eq!(final_intent.delta_x, -70.0);
    assert!(held.is_none());
    assert_eq!(
        rectangle_pointer(&Cell::<Option<RectangleCapture<usize>>>::new(None), 7.0),
        Some(7.0)
    );
}

#[test]
fn invalid_and_disabled_input_never_poison_capture_and_threshold_is_current_displacement() {
    assert!(RectangleCapture::begin(0, RectanglePart::Body, 1.0, false).is_none());
    assert!(RectangleCapture::begin(0, RectanglePart::Body, f32::NAN, true).is_none());
    let mut capture = RectangleCapture::begin(0, RectanglePart::Body, 10.0, true).unwrap();
    capture.update(15.0).unwrap();
    assert!(capture.displaced(4.0));
    capture.update(10.0).unwrap();
    assert!(!capture.displaced(4.0));
    let before = capture;
    assert!(capture.update(f32::INFINITY).is_none());
    assert_eq!(capture, before);
    assert_eq!(capture.cancel().phase, RectanglePhase::Cancel);
    assert!(capture.finish(f32::NAN).is_none());
    let mut capture = RectangleCapture::begin(0, RectanglePart::Leading, f32::MAX, true).unwrap();
    assert!(capture.update(-f32::MAX).is_none());
}

#[test]
fn selection_policy_keeps_clip_range_and_section_toggle_contracts_explicit() {
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    let command = Modifiers {
        platform: true,
        ..Default::default()
    };
    let both = Modifiers {
        platform: true,
        shift: true,
        ..Default::default()
    };
    assert_eq!(
        RectangleSelectionPolicy::Range.intent(shift),
        RectangleSelection::Range { additive: false }
    );
    assert_eq!(
        RectangleSelectionPolicy::Range.intent(command),
        RectangleSelection::Toggle
    );
    assert_eq!(
        RectangleSelectionPolicy::Range.intent(both),
        RectangleSelection::Range { additive: true }
    );
    assert_eq!(
        RectangleSelectionPolicy::Section.intent(shift),
        RectangleSelection::Range { additive: false }
    );
    assert_eq!(
        RectangleSelectionPolicy::Section.intent(both),
        RectangleSelection::Toggle
    );
    assert_eq!(
        RectangleSelectionPolicy::Section.intent(command),
        RectangleSelection::Replace
    );
}
