//! Saved playback-start input uses the event editor's existing Window lease and
//! the clip-loop owner's ordinary edit receipt. It never seeks transport.
use super::*;
use crate::theme;
use gpui::{
    Context, Div, MouseButton, MouseDownEvent, PathBuilder, Window, canvas, div, point, prelude::*,
    px, relative, rgb,
};

impl EditorView {
    pub(super) fn audio_play_start_position(&self) -> Option<u64> {
        if let Some(Kind::PlayStart { position }) =
            self.audio_event_editor.gesture.as_ref().map(|g| g.kind)
        {
            return Some(position);
        }
        let state = self.state.as_ref()?;
        let id = self.detail_clip_id()?;
        let clip = self
            .pending_clip_loop_audio(id)
            .or_else(|| state.project.clip(id))?;
        hex_project::AudioViewProjection::new(clip, state.project.tempo_map())
            .ok()?
            .play_start_position()
            .ok()
            .flatten()
    }
    pub(super) fn audio_play_start_at_pointer(
        &self,
        root: &AudioClipEdit,
        gesture: &Gesture,
        x: f32,
        platform: bool,
    ) -> Option<u64> {
        let tempo = self.state.as_ref()?.project.tempo_map();
        let width = f64::from(f32::from(self.audio_event_editor.bounds?.size.width));
        if !x.is_finite() || !width.is_finite() || width <= 0.0 {
            return None;
        }
        let anchor = root.root_frame_at(gesture.anchor, tempo).ok()?;
        let first = root.root_frame_at(root.view_start, tempo).ok()?;
        let end = root.root_frame_at(root.view_end, tempo).ok()?;
        let last_frame = end.checked_sub(1).filter(|frame| *frame >= first)?;
        let viewport = self.event_viewport(root);
        let delta = ((f64::from(x) - f64::from(gesture.origin_x)) * viewport.length as f64 / width)
            .clamp(-(u64::MAX as f64), u64::MAX as f64)
            .round() as i128;
        let frame = u64::try_from(
            (i128::from(anchor) + delta).clamp(i128::from(first), i128::from(last_frame)),
        )
        .ok()?;
        let last = root
            .view_end
            .checked_sub(1)
            .filter(|last| *last >= root.view_start)?;
        // Pointer coordinates are frames. A final raw beat tick can already
        // project past the last sample, so snapping must share this frame cap.
        let last = root
            .root_position_at(last_frame, tempo)
            .ok()?
            .clamp(root.view_start, last);
        let raw = root
            .root_position_at(frame, tempo)
            .ok()?
            .clamp(root.view_start, last);
        let position = self
            .event_snap_position(raw, platform)
            .unwrap_or(raw)
            .clamp(root.view_start, last);
        // The live musical phase can change its processing clock. Reuse the
        // owner's scalar resolution for held, pending and canonical markers.
        let state = self.state.as_ref()?;
        let source = state.project.clip(self.detail_clip_id()?)?;
        let resolved = hex_project::preview_view_resize(
            source,
            tempo,
            hex_project::AudioViewResizeOperation::PlayStart { position },
        )
        .ok()?;
        resolved.play_start_position
    }
    pub(super) fn begin_audio_play_start(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !f32::from(event.position.x).is_finite()
            || !f32::from(event.position.y).is_finite()
            || self.time_selection_key_held
            || self.audio_event_editor.source_view
            || !self.event_context_current()
            || self.edit_pending()
            || self.audio_event_editor.pending.is_some()
            || self.audio_event_editor.gesture.is_some()
            || cx.has_active_drag()
        {
            return;
        }
        let Some((clip, _, _)) = self.event_context() else {
            return;
        };
        let Some(position) = self.audio_play_start_position() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = state.project.clip(clip) else {
            return;
        };
        if let Err(error) = hex_project::preview_view_resize(
            source,
            state.project.tempo_map(),
            hex_project::AudioViewResizeOperation::PlayStart { position },
        ) {
            self.error = Some(error.0.into());
            cx.notify();
            return;
        }
        let kind = Kind::PlayStart { position };
        if !self.capture_audio_event_pointer(kind, window, cx) {
            return;
        }
        self.audio_event_focus.focus(window);
        self.audio_event_editor.gesture = Some(Gesture {
            anchor: position,
            pointer_x: f32::from(event.position.x),
            origin_x: f32::from(event.position.x),
            moved: false,
            kind,
            placement: None,
            operation: None,
        });
        cx.notify();
    }
}

/// Camera held by the admitted edit's ordinary receipt. Creating a Once
/// placement may expose more root frames; live musical timing can also move the
/// grabbed point in that frame clock. Preserve its visible offset at accepted
/// installation, using the terminal project's projection rather than old frames.
#[derive(Clone, Copy)]
pub(in crate::editor) struct Camera {
    prior: Option<Viewport>,
    root: Viewport,
    bounds: (u64, u64),
    position: u64,
    offset: u64,
}
impl EditorView {
    pub(in crate::editor) fn audio_play_start_camera(
        &self,
        clip: ClipId,
        position: u64,
    ) -> Option<Camera> {
        let (owner, root, _) = self.event_context()?;
        if owner != clip || !self.event_context_current() {
            return None;
        }
        let tempo = self.state.as_ref()?.project.tempo_map();
        let first = root.root_frame_at(editor_bounds(&root).0, tempo).ok()?;
        let end = root.root_frame_at(editor_bounds(&root).1, tempo).ok()?;
        let viewport = self.event_viewport(&root);
        Some(Camera {
            prior: self.audio_event_editor.viewport,
            root: Viewport {
                start: first.checked_add(viewport.start)?,
                length: viewport.length,
            },
            bounds: (first, end),
            position,
            offset: root
                .root_frame_at(position, tempo)
                .ok()?
                .saturating_sub(first.checked_add(viewport.start)?),
        })
    }
    /// Called only with the exact terminal's canonical project inside receive.
    pub(in crate::editor) fn install_audio_play_start_camera(
        &mut self,
        clip: ClipId,
        generation: u64,
        camera: Camera,
        project: &hex_project::Project,
    ) {
        if generation != self.project_generation
            || self.audio_event_editor.owner != Some((clip, generation))
        {
            return;
        }
        let Some(source) = project.clip(clip) else {
            return;
        };
        let accepted_position = hex_project::AudioViewProjection::new(source, project.tempo_map())
            .ok()
            .and_then(|projection| projection.play_start_position().ok().flatten());
        if accepted_position != Some(camera.position) {
            return;
        }
        let root = source.audio_event_root();
        let Ok(first) = root.root_frame_at(editor_bounds(&root).0, project.tempo_map()) else {
            return;
        };
        let Ok(end) = root.root_frame_at(editor_bounds(&root).1, project.tempo_map()) else {
            return;
        };
        let Ok(frame) = root.root_frame_at(camera.position, project.tempo_map()) else {
            return;
        };
        let start = frame.saturating_sub(camera.offset).saturating_sub(first);
        self.audio_event_editor
            .install_frame_space(&root, (first, end));
        self.audio_event_editor.viewport = if camera.prior.is_none()
            && camera.bounds == (first, end)
            && frame.saturating_sub(camera.offset) == camera.root.start
        {
            None
        } else {
            Some(Viewport {
                start,
                length: camera.root.length,
            })
        };
        self.audio_event_editor.canonical_start = Some((camera.position, frame));
        self.audio_event_editor.start_grip =
            Some((camera.position, camera.offset, camera.root.length));
    }
}

impl EditorView {
    /// The marker follows the canonical root-clock query. Only the small arrow
    /// is a drag target; the ruler below it still paints time selections.
    pub(super) fn audio_play_start_marker(
        &self,
        root: &AudioClipEdit,
        viewport: Viewport,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.detail_clip_id()?)?;
        let position = self.audio_play_start_position()?;
        let frame = root
            .root_frame_at(position, state.project.tempo_map())
            .ok()?;
        let fraction = viewport.fraction(frame)?;
        let editable = hex_project::preview_view_resize(
            clip,
            state.project.tempo_map(),
            hex_project::AudioViewResizeOperation::PlayStart { position },
        )
        .is_ok();
        let active = self
            .audio_event_editor
            .gesture
            .as_ref()
            .is_some_and(|g| matches!(g.kind, Kind::PlayStart { .. }));
        let selection_mode = self.time_selection_key_held;
        let inward = self
            .audio_event_editor
            .bounds
            .is_some_and(|bounds| f32::from(bounds.size.width) * (1.0 - fraction) < 10.0);
        let handle_left = if inward { -10.0 } else { -3.0 };
        let hovered = editable
            && !selection_mode
            && !window.is_pointer_dragging()
            && self.audio_event_editor.bounds.is_some_and(|bounds| {
                let x = bounds.left() + bounds.size.width * fraction;
                let pointer = window.mouse_position();
                pointer.x >= x + px(handle_left)
                    && pointer.x <= x + px(handle_left + 13.0)
                    && pointer.y >= bounds.top()
                    && pointer.y <= bounds.top() + px(12.0)
            });
        let color = theme::color(if active || hovered {
            theme::PaletteRole::Primary
        } else {
            theme::PaletteRole::Text
        });
        let selector = if editable {
            "audio-play-start-handle"
        } else {
            "audio-play-start-readonly"
        };
        let handle = div().id(selector).debug_selector(move || selector.into())
            .absolute().left(px(handle_left)).top_0().w(px(13.0)).h(px(12.0))
            .cursor(if selection_mode { gpui::CursorStyle::IBeam } else if editable { gpui::CursorStyle::ResizeLeftRight } else { gpui::CursorStyle::Arrow })
            .on_hover(cx.listener(|_, _, _, cx| cx.notify()))
            .tooltip(move |_, cx| cx.new(|_| super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control("play start"),
                description: if editable {
                    "drag saved playback start · hold 2 to select time · cmd/control inverts snap · escape cancels"
                } else { "play start is read-only for this audio view" }.into(),
            }).into())
            .on_mouse_down(MouseButton::Left, cx.listener(move |view, event, window, cx| {
                if view.time_selection_key_held { return }
                if editable { view.begin_audio_play_start(event, window, cx); }
                cx.stop_propagation();
            }));
        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let x = bounds.left();
                let y = bounds.top();
                let mut arrow = PathBuilder::fill();
                arrow.move_to(point(x, y));
                arrow.line_to(point(x + px(7.0), y + px(4.0)));
                arrow.line_to(point(x, y + px(8.0)));
                arrow.close();
                if let Ok(path) = arrow.build() {
                    window.paint_path(path, rgb(color));
                }
            },
        )
        .absolute()
        .top(px(2.0))
        .left(px(if inward { -7.0 } else { 0.0 }))
        .w(px(7.0))
        .h(px(8.0));
        Some(
            div()
                .debug_selector(|| "audio-play-start-marker".into())
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(fraction))
                .w(px(1.0))
                .bg(rgb(color))
                .children([handle])
                .children([arrow]),
        )
    }
}
