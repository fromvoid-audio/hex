//! Compact, ID-backed Session grid and guarded playback requests.
mod actions;
pub(crate) use actions::PendingPlaybackAction;
use actions::with_quantization;
pub(super) use actions::{DeleteTarget, action_is_current, launch_action, resolve_delete_target};

use super::console_columns::ColumnFrame;
use super::console_track_page::TrackPage;
use super::session_route_view::cell_route_markers;
use super::track_proposals::{TrackProposal, anchor_ordinal};
use super::{EditorMode, EditorView};
use crate::theme;
use gpui::{Context, Div, ElementId, Window, div, prelude::*, px, rgb, uniform_list};
use hex_audio::prepared::{
    SessionRouteEntry, SessionRouteOwner, SessionRouteSnapshot, SessionRouteTarget,
};
use hex_engine::{LaunchQuantization, SessionPlaybackCommand};
use hex_project::{
    BeatTime, ClipId, Command, SceneId, SessionCellContent, SessionSceneTarget, TrackId,
};
use hex_session::{import_batch::ConsoleDestination, service::Request};
use std::sync::Arc;

const TRACK_COLUMN_WIDTH: f32 = 148.0;
const SCENE_ROW_HEIGHT: f32 = 31.0;
// Shared by the pinned Return bank so scene and mixer columns stay aligned.
pub(super) const TRACK_HEADER_HEIGHT: f32 = 28.0;
// Eight nested ancestors fit while preserving a usable header label.
const GROUP_BAND_HEIGHT: f32 = 1.0;
const SCENE_HEADER_WIDTH: f32 = 110.0;

/// The proposed column has no cell, mixer, or drop actions until preparation ends.
pub(super) fn pending_device_column(width: f32) -> Div {
    div()
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .border_l_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
}

/// Paint occupies an inset surface; the cell keeps its original IDs, layout,
/// rename/launch controls and drop targets. Console clips are one caption row,
/// so their entire surface uses the header fill without an arrangement divider.
fn session_clip_backdrop(clip: ClipId, fill: u32, stroke: f32) -> Div {
    let outline = theme::color(theme::PaletteRole::ClipOutline);
    div()
        .debug_selector(move || format!("console-clip-{}", clip.value()))
        .absolute()
        .inset(px(2.0))
        .bottom(px(3.0))
        .child(super::clip_boundary::backdrop_shell(
            div(),
            0.0,
            1.0,
            super::clip_boundary::Backdrop {
                body: fill,
                header: fill,
                outline,
                hover_outline: outline,
                stroke,
                header_height: SCENE_ROW_HEIGHT,
                header_gradient: None,
                notch: super::clip_boundary::Notch::new(true),
            },
            true,
            move || format!("console-clip-trailing-{}", clip.value()),
        ))
}

#[derive(Clone)]
struct SessionRowContext {
    frame: ColumnFrame,
    revision: u64,
    generation: u64,
    page: Arc<TrackPage>,
    route_snapshot: Option<SessionRouteSnapshot>,
    routes_ready: bool,
    transport_playing: bool,
    stroke: f32,
    return_width: f32,
}

#[derive(Clone, Copy)]
enum CellExpectation {
    Empty,
    Stop,
    Midi(ClipId),
    Audio(ClipId),
}

mod gestures;
mod paint;

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::launch_action;
    use hex_engine::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn queued_scene_launch_retains_quantization_selected_before_launch() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 3).unwrap();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };

        let mut selected_quantization = LaunchQuantization::FourQuarter;
        let command = super::with_quantization(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            },
            selected_quantization,
        );
        let queued = launch_action(command, 19, 3);
        selected_quantization = LaunchQuantization::Immediate;
        assert_eq!(selected_quantization, LaunchQuantization::Immediate);
        assert!(matches!(
            queued.action,
            PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
                quantization: LaunchQuantization::FourQuarter,
                ..
            })
        ));
    }
}
