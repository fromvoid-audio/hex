//! Capture-local point admission. Invalid requested points never become display
//! or commands; this deliberately makes no monotonic/nearest-boundary claim.
use super::{TrimDrag, TrimPreviewState, edge_frame, preview_clip};
use hex_project::{Clip, Project};

#[path = "stretch_preview/resolver.rs"]
mod resolver;
pub(super) use resolver::Resolution;
use resolver::{ResolutionRequest, resolve as resolve_request};

pub(super) fn resolve(drag: &TrimDrag, project: &Project, frame: u64) -> Option<TrimPreviewState> {
    let stretch = drag.stretch_modifier.get();
    let request = ResolutionRequest { frame, stretch };
    let mut resolution = drag.resolution.borrow_mut();
    resolve_request(
        &mut resolution,
        request,
        || edge_frame(&drag.clip, drag.edge),
        |candidate_frame| admitted(drag, project, candidate_frame, stretch),
    )
    .map(|accepted| TrimPreviewState {
        clip: accepted.clip,
        command_frame: accepted.command_frame,
        stretch: accepted.stretch,
    })
}

fn admitted(drag: &TrimDrag, project: &Project, frame: u64, stretch: bool) -> Option<Clip> {
    let clip = preview_clip(drag, project, frame).ok()?;
    if stretch {
        hex_engine::audio_transform::validate_clip_warp_preview(&clip, project.tempo_map()).ok()?;
    }
    Some(clip)
}
