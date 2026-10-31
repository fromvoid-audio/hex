//! Display-only clip layout for a pending structural section edit. Built once
//! when the command is submitted and owned by the section receipt, so moved
//! clips and markers retire together with the canonical installation. It never
//! dispatches, prepares audio or replaces canonical state.
//! Gain automation and the overview keep their canonical previews until the
//! acknowledged installation; only arrangement clip rows read this layout.
use super::ViewState;
use hex_project::{Command, Project};
use std::sync::Arc;

/// Preview rows for `command`, or `None` for marker-only/refused commands, which
/// keep drawing canonical clips. Prepared audio peaks follow their clip by ID;
/// a crop moves the visible root offset by the same projected delta.
pub(super) fn build(state: &ViewState, command: &Command) -> Option<Arc<ViewState>> {
    if !command.requires_section_publication() {
        return None;
    }
    let project = state.project.preview_section_layout(command).ok()?;
    let mut audio_clip_waveforms = state.audio_clip_waveforms.clone();
    for (id, prepared) in &mut audio_clip_waveforms {
        let Some(clip) = project
            .clip(*id)
            .filter(|&c| state.project.clip(*id) != Some(c))
        else {
            continue;
        };
        let (Some(before), Some(after)) = (
            projected_offset(&state.project, *id),
            projected_offset(&project, *id),
        ) else {
            continue;
        };
        let offset = if after >= before {
            prepared.offset.checked_add(after - before)
        } else {
            prepared.offset.checked_sub(before - after)
        };
        if let Some(offset) = offset {
            prepared.offset = offset;
            prepared.length = clip.duration();
        }
    }
    let midi_preview = midi_preview(state, &project)?;
    Some(Arc::new(ViewState {
        canonical_audio_source: state.canonical_audio_source.clone(),
        comparison_audio: state.comparison_audio.clone(),
        project: Arc::new(project),
        waveforms: state.waveforms.clone(),
        source_overviews: state.source_overviews.clone(),
        audio_clip_waveforms,
        midi_preview: Arc::new(midi_preview),
        gain_preview: state.gain_preview.clone(),
        timeline_frames: state.timeline_frames,
        can_undo: state.can_undo,
        can_redo: state.can_redo,
        revision: state.revision,
        unsaved: state.unsaved,
        document_path: state.document_path.clone(),
    }))
}

/// MIDI rows draw from bounded summaries. Unchanged clips keep the prepared
/// summary; moved, cropped or copied clips get a display-only summary.
fn midi_preview(
    state: &ViewState,
    project: &Project,
) -> Option<hex_engine::MidiArrangementPreview> {
    let same_tempo = state.project.tempo_map() == project.tempo_map();
    let mut clips = Vec::new();
    for clip in project.tracks().iter().flat_map(|track| track.midi_clips()) {
        let prepared = state
            .midi_preview
            .get(clip.id())
            .filter(|_| same_tempo && state.project.midi_clip(clip.id()) == Some(clip));
        clips.push(match prepared {
            Some(prepared) => prepared.clone(),
            None => hex_engine::midi_clip_preview(project, clip).ok()?,
        });
    }
    hex_engine::MidiArrangementPreview::from_clips(clips).ok()
}

fn projected_offset(project: &Project, clip: hex_project::ClipId) -> Option<u64> {
    let (_, _, offset, _) = project
        .clip(clip)?
        .audio_edit()?
        .projected_domain(project.tempo_map())
        .ok()?;
    Some(offset)
}
