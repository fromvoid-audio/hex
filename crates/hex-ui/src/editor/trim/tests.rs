//! Pure trim drag validation and preview tests.
use super::{TrimDrag, payload_is_current};
use crate::editor::{drag::LaneBounds, viewport::Viewport};
use hex_project::{Clip, Command, Created, Editor, TrackId, TrimEdge};
use hex_session::service::ViewState;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::Arc,
};

#[test]
fn trim_payload_rejects_stale_generation_revision_viewport_owner_and_asset() {
    let (mut editor, state, track, clip_id) = state_fixture(300);
    let clip = state.project.clip(clip_id).expect("clip").clone();
    let payload = trim_drag(track, clip, 300);
    let viewport = Viewport {
        start: 0,
        length: 500,
    };
    assert!(payload_is_current(
        &payload,
        4,
        &state,
        viewport,
        Some(payload.source_lane)
    ));
    assert!(!payload_is_current(
        &payload,
        5,
        &state,
        viewport,
        Some(payload.source_lane)
    ));

    let stale_revision = ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::clone(&state.project),
        waveforms: BTreeMap::new(),
        source_overviews: Default::default(),
        audio_clip_waveforms: std::collections::BTreeMap::new(),
        midi_preview: Arc::clone(&state.midi_preview),
        gain_preview: state.gain_preview.clone(),
        timeline_frames: state.timeline_frames,
        can_undo: false,
        can_redo: false,
        revision: state.revision + 1,
        unsaved: false,
        document_path: None,
    };
    assert!(!payload_is_current(
        &payload,
        4,
        &stale_revision,
        viewport,
        Some(payload.source_lane)
    ));
    let shifted_viewport = Viewport {
        start: 1,
        length: 500,
    };
    assert!(!payload_is_current(
        &payload,
        4,
        &state,
        shifted_viewport,
        Some(payload.source_lane)
    ));
    assert!(!payload_is_current(&payload, 4, &state, viewport, None));
    let changed_lane = LaneBounds {
        left: payload.source_lane.left,
        width: payload.source_lane.width + 1.0,
        top: payload.source_lane.top,
        height: payload.source_lane.height,
    };
    assert!(!payload_is_current(
        &payload,
        4,
        &state,
        viewport,
        Some(changed_lane)
    ));

    let (_, wrong_asset, _, _) = state_fixture(301);
    assert!(!payload_is_current(
        &payload,
        4,
        &wrong_asset,
        viewport,
        Some(payload.source_lane)
    ));

    editor
        .execute(Command::RemoveClip(clip_id))
        .expect("remove clip");
    let removed = ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::new(editor.project().clone()),
        waveforms: BTreeMap::new(),
        source_overviews: Default::default(),
        audio_clip_waveforms: std::collections::BTreeMap::new(),
        midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
        gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
        timeline_frames: 500,
        can_undo: false,
        can_redo: false,
        revision: state.revision,
        unsaved: false,
        document_path: None,
    };
    assert!(!payload_is_current(
        &payload,
        4,
        &removed,
        viewport,
        Some(payload.source_lane)
    ));

    let (mut moved_editor, moved_state, old_owner, moved_clip) = state_fixture(300);
    let Created::Track(new_owner) = moved_editor
        .execute(Command::AddTrack {
            name: "new owner".into(),
        })
        .expect("new owner")
    else {
        panic!("track result")
    };
    moved_editor
        .execute(Command::MoveClip {
            clip: moved_clip,
            track: new_owner,
            start: 300,
        })
        .expect("move clip");
    let moved_payload = trim_drag(
        old_owner,
        moved_state
            .project
            .clip(moved_clip)
            .expect("old clip")
            .clone(),
        300,
    );
    let moved = ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::new(moved_editor.project().clone()),
        waveforms: BTreeMap::new(),
        source_overviews: Default::default(),
        audio_clip_waveforms: std::collections::BTreeMap::new(),
        midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
        gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
        timeline_frames: 500,
        can_undo: false,
        can_redo: false,
        revision: moved_state.revision,
        unsaved: false,
        document_path: None,
    };
    assert!(!payload_is_current(
        &moved_payload,
        4,
        &moved,
        viewport,
        Some(moved_payload.source_lane)
    ));
}

fn state_fixture(frames: u64) -> (Editor, ViewState, TrackId, hex_project::ClipId) {
    let (editor, track, id) = super::geometry::clip_fixture(frames);
    let state = ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::new(editor.project().clone()),
        waveforms: BTreeMap::new(),
        source_overviews: Default::default(),
        audio_clip_waveforms: std::collections::BTreeMap::new(),
        midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
        gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
        timeline_frames: 500,
        can_undo: false,
        can_redo: false,
        revision: 7,
        unsaved: false,
        document_path: None,
    };
    (editor, state, track, id)
}

pub(super) fn trim_drag(track: TrackId, clip: Clip, asset_playback_frames: u64) -> TrimDrag {
    TrimDrag {
        loop_kind: None,
        repeat_available: false,
        repeat: Rc::new(Cell::new(false)),
        canceled: Rc::new(Cell::new(false)),
        capture: Rc::new(Cell::new(None)),
        stretch_modifier: Rc::new(Cell::new(false)),
        project_generation: 4,
        revision: 7,
        source_track: track,
        clip,
        asset_playback_frames,
        edge: TrimEdge::Left,
        source_lane: LaneBounds {
            left: 10.0,
            width: 100.0,
            top: 0.0,
            height: 44.0,
        },
        viewport_start: 0,
        viewport_length: 500,
        scroll: Default::default(),
        snap_grid: super::super::musical_time::SnapGrid::Off,
        snap_modifier: Rc::new(Cell::new(false)),
        last_pointer_x: Rc::new(Cell::new(None)),
        valid_pointer_x: Rc::new(Cell::new(None)),
        grab_delta: Rc::new(RefCell::new(None)),
        preview: Rc::new(RefCell::new(None)),
        resolution: Default::default(),
    }
}
