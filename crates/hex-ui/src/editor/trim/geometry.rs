//! Frame and hit geometry for trim gestures.
use super::super::{drag::LaneBounds, viewport::Viewport};
use hex_project::{Clip, TrimEdge};

const HANDLE_WIDTH: f32 = 4.0;
const MIN_MOVE_WIDTH: f32 = 1.0;

pub(super) fn frame_at_x(x: f32, lane: LaneBounds, viewport: Viewport) -> Option<u64> {
    if !x.is_finite()
        || !lane.left.is_finite()
        || !lane.width.is_finite()
        || lane.width <= 0.0
        || viewport.length == 0
    {
        return None;
    }
    let fraction = ((f64::from(x) - f64::from(lane.left)) / f64::from(lane.width)).clamp(0.0, 1.0);
    let in_view = (fraction * viewport.length as f64).floor() as u64;
    viewport.start.checked_add(in_view.min(viewport.length))
}

pub(super) fn corrected_edge_frame(pointer_frame: u64, grab_delta: i128) -> u64 {
    (i128::from(pointer_frame) - grab_delta).clamp(0, i128::from(u64::MAX)) as u64
}

pub(super) fn edge_frame(clip: &Clip, edge: TrimEdge) -> Option<u64> {
    match edge {
        TrimEdge::Left => Some(clip.start()),
        TrimEdge::Right => clip.start().checked_add(clip.duration()),
    }
}

pub(super) fn preview_is_noop(original: &Clip, preview: &Clip) -> bool {
    original == preview
}

pub(in crate::editor) fn handle_width(clip_width: f32) -> f32 {
    hex_widgets::RectangleEdges {
        width: HANDLE_WIDTH,
        minimum_body: MIN_MOVE_WIDTH,
    }
    .handle_width(clip_width)
}

/// The painted interval and its hit geometry retain canonical endpoints even
/// below handle size; narrow clips yield their edge handles to the move body.
pub(in crate::editor) fn rendered_clip_width(lane_width: f32, width_fraction: f64) -> f32 {
    let width = lane_width * width_fraction as f32;
    if !width.is_finite() || width <= 0.0 || lane_width <= 0.0 || width_fraction <= 0.0 {
        return 0.0;
    }
    width
}

#[cfg(test)]
pub(super) fn clip_fixture(
    frames: u64,
) -> (
    hex_project::Editor,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "track".into(),
            path: "trim.wav".into(),
            sample_rate: 48_000,
            frames,
        })
        .expect("import")
    else {
        panic!("import result")
    };
    let Created::Clip(id) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 100,
            duration: 100,
            source_offset: 20,
            source_length: 100,
            looping: false,
        })
        .expect("place clip")
    else {
        panic!("clip result")
    };
    (editor, track, id)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn pointer_mapping_preserves_handle_offset_and_clamps_before_frame_zero() {
        let lane = LaneBounds {
            left: 10.0,
            width: 100.0,
            top: 0.0,
            height: 44.0,
        };
        let viewport = Viewport {
            start: 100,
            length: 1_000,
        };
        let pointer = frame_at_x(30.0, lane, viewport).expect("pointer frame");
        let delta = i128::from(pointer) - 250;
        assert_eq!(corrected_edge_frame(pointer, delta), 250);
        assert_eq!(corrected_edge_frame(0, 25), 0);
        assert_eq!(corrected_edge_frame(u64::MAX, -1), u64::MAX);
        assert_eq!(frame_at_x(f32::NAN, lane, viewport), None);
    }

    #[test]
    fn edge_frame_uses_inclusive_left_and_exclusive_right_bounds() {
        let (editor, _, id) = clip_fixture(300);
        let clip = editor.project().clip(id).expect("clip");
        assert_eq!(edge_frame(clip, TrimEdge::Left), Some(100));
        assert_eq!(edge_frame(clip, TrimEdge::Right), Some(200));
        let unchanged = clip
            .preview_edge_trim(TrimEdge::Right, 200, 300)
            .expect("unchanged preview");
        assert!(preview_is_noop(clip, &unchanged));
    }

    #[test]
    fn resize_hit_width_stays_inside_clip_and_move_body_remains_available_at_small_sizes() {
        for (clip_width, expected) in [(3.0, 1.0), (10.0, 4.0), (20.0, 4.0), (240.0, 4.0)] {
            let left = handle_width(clip_width);
            let right = handle_width(clip_width);
            assert_eq!(left, expected);
            assert_eq!(right, expected);
            assert!(left + right <= clip_width);
            // Full-height edges must leave the central body available for moves.
            assert!(clip_width - left - right >= 1.0);
        }
        let narrow = rendered_clip_width(720.0, 0.0005);
        assert_eq!(narrow, 0.36);
        assert_eq!(handle_width(narrow), 0.0);
        assert_eq!(rendered_clip_width(f32::NAN, 1.0), 0.0);
        assert_eq!(rendered_clip_width(720.0, f64::INFINITY), 0.0);
        assert_eq!(handle_width(f32::NAN), 0.0);
        assert_eq!(handle_width(0.0), 0.0);
    }
}
