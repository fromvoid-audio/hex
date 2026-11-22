//! Delegates pure trim geometry to [`hex_ui_model::trim_geometry`] while keeping
//! `hex_widgets::RectangleEdges` handle-width sizing in `hex-ui`.
pub(in crate::editor) use hex_ui_model::trim_geometry::rendered_clip_width;
#[allow(unused_imports)] // Re-exported for trim module; pure unit tests live in hex-ui-model.
pub(super) use hex_ui_model::trim_geometry::{
    corrected_edge_frame, edge_frame, frame_at_x, preview_is_noop,
};

const HANDLE_WIDTH: f32 = 4.0;
const MIN_MOVE_WIDTH: f32 = 1.0;

pub(in crate::editor) fn handle_width(clip_width: f32) -> f32 {
    hex_widgets::RectangleEdges {
        width: HANDLE_WIDTH,
        minimum_body: MIN_MOVE_WIDTH,
    }
    .handle_width(clip_width)
}

#[cfg(all(test, any(feature = "ui-unit-tier", not(feature = "ui-test-support"))))]
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
        assert_eq!(handle_width(f32::NAN), 0.0);
        assert_eq!(handle_width(0.0), 0.0);
    }
}
