//! Pending trim custody, audio clip trim/loop handles, and gesture lifecycle.
use super::*;

impl EditorView {
    pub(super) fn pending_trim_current(&self) -> Option<&PendingTrim> {
        let pending = self.pending_trim.as_ref()?;
        let state = self.state.as_ref()?;
        if pending.generation != self.project_generation || pending.revision > state.revision {
            return None;
        }
        // Independent revisions may complete ahead of this queued request.
        // Its exact stable source owner, not revision equality alone, decides
        // whether the accepted prospective geometry still belongs here.
        let track = state.project.track(pending.track)?;
        let current = match &pending.display {
            PendingTrimDisplay::Audio {
                original,
                asset_frames,
                ..
            } => {
                track.clips().iter().any(|clip| clip == original)
                    && state.project.asset(original.asset())?.playback_frames() == *asset_frames
            }
            PendingTrimDisplay::Midi { original, .. } => track
                .midi_clips()
                .iter()
                .any(|clip| clip == original.as_ref()),
        };
        current.then_some(pending)
    }

    pub(in crate::editor) fn audio_trim_display(
        &self,
        track: TrackId,
        only: Option<hex_project::ClipId>,
    ) -> Option<(Clip, Clip)> {
        if let Some(drag) = self.active_trim_drag.as_ref()
            && drag.source_track == track
            && only.is_none_or(|id| id == drag.clip.id())
            && self.trim_drag_current(drag)
            && let Some(preview) = drag.preview.borrow().as_ref()
        {
            return Some((drag.clip.clone(), preview.clip.clone()));
        }
        let pending = self.pending_trim.as_ref()?;
        if pending.track != track {
            return None;
        }
        let PendingTrimDisplay::Audio {
            original,
            prospective,
            ..
        } = &pending.display
        else {
            return None;
        };
        if only.is_some_and(|id| id != original.id()) {
            return None;
        }
        self.pending_trim_current()?;
        Some((original.clone(), prospective.as_ref().clone()))
    }

    pub(in crate::editor) fn midi_trim_display(
        &self,
        track: TrackId,
        only: Option<hex_project::ClipId>,
    ) -> Option<(Rc<MidiClip>, MidiEdgePreview)> {
        if let Some(drag) = self.midi_editor.active_trim_drag.as_ref()
            && drag.source_track == track
            && only.is_none_or(|id| id == drag.clip.id())
            && self.midi_trim_drag_current(drag)
            && let Some(preview) = *drag.preview.borrow()
        {
            return Some((Rc::clone(&drag.clip), preview));
        }
        let pending = self.pending_trim.as_ref()?;
        if pending.track != track {
            return None;
        }
        let PendingTrimDisplay::Midi {
            original,
            prospective,
        } = &pending.display
        else {
            return None;
        };
        if only.is_some_and(|id| id != original.id()) {
            return None;
        }
        self.pending_trim_current()?;
        Some((Rc::clone(original), *prospective))
    }

    pub(in crate::editor) fn pending_trim_matches(&self, request: &Request) -> bool {
        self.pending_trim.as_ref().is_some_and(|pending| {
            pending.generation == self.project_generation
                && pending.remaining.is_none()
                && ResizeCommand::from_request(request) == Some(pending.command)
        })
    }
    pub(in crate::editor) fn trim_request_submitted(&mut self) {
        if let Some(pending) = self.pending_trim.as_mut() {
            pending.remaining = Some(self.worker_requests_pending);
            if let Some(state) = &self.state {
                pending.revision = state.revision;
            }
        }
    }
    /// Take only our own FIFO terminal before receive installs its canonical
    /// state. Earlier completions, including typed gestures, count but do not
    /// normalize. Replacement discards the old document's owner entirely.
    pub(in crate::editor) fn trim_request_complete(
        &mut self,
        event: &hex_session::service::Event,
        retry: &mut Option<Request>,
    ) -> Option<RetiredTrimViewport> {
        let pending = self.pending_trim.as_mut()?;
        if event.project_replaced || pending.generation != self.project_generation {
            self.pending_trim = None;
            self.service.audition_arrangement_stretch(None);
            return None;
        }
        if event.request_complete
            && let Some(remaining) = pending.remaining.as_mut()
        {
            *remaining = remaining.saturating_sub(1);
            if *remaining == 0 {
                if !pending.replayed
                    && let ResizeCommand::AudioLoop { clip, operation } = pending.command
                    && event.resident_edit_source_unavailable
                        == Some(hex_engine::AudioSourceContext {
                            project_generation: pending.generation,
                            editor_revision: pending.revision,
                        })
                    && event.state.revision == pending.revision
                {
                    pending.remaining = None;
                    *retry = Some(Request::Edit(Command::ResizeAudioClipView {
                        clip,
                        operation,
                    }));
                    return None;
                }
                self.service.audition_arrangement_stretch(None);
                return self.pending_trim.take().map(PendingTrim::into_viewport);
            }
        }
        None
    }
    pub(in crate::editor) fn normalize_completed_trim_viewport(
        &mut self,
        terminal: Option<RetiredTrimViewport>,
    ) {
        if let Some(terminal) = terminal {
            self.normalize_canceled_trim_viewport(terminal.generation, terminal.viewport);
        }
    }
    pub(in crate::editor) fn mark_replayed_trim(&mut self, request: &Request) {
        if self.pending_trim_matches(request)
            && let Some(pending) = &mut self.pending_trim
        {
            pending.replayed = true;
        }
    }

    pub(in crate::editor) fn retire_pending_trim(&mut self) -> bool {
        let terminal = self.pending_trim.take().map(PendingTrim::into_viewport);
        let retired = terminal.is_some();
        if retired {
            self.service.audition_arrangement_stretch(None);
        }
        self.normalize_completed_trim_viewport(terminal);
        retired
    }

    /// Record press state for an analytic audio trim/loop hit on a lane canvas.
    pub(in crate::editor) fn press_audio_trim_from_lane(
        &mut self,
        target: LaneTrimTarget,
        viewport: Viewport,
        pointer_x: f32,
        window: &mut Window,
    ) {
        let LaneTrimTarget {
            track,
            clip_id,
            edge,
            loop_kind,
        } = target;
        let generation = self.project_generation;
        let Some(source) = self.audio_trim_source_for(track, clip_id, edge, loop_kind, viewport)
        else {
            self.clear_pointer_down(generation, clip_id, Some(edge));
            return;
        };
        if self.trim_drag_current(&source.payload()) {
            self.focus.focus(window);
            self.remember_pointer_down(generation, clip_id, Some(edge), pointer_x);
        } else {
            self.clear_pointer_down(generation, clip_id, Some(edge));
        }
    }

    /// Record press state for an analytic MIDI trim/loop hit on a lane canvas.
    pub(in crate::editor) fn press_midi_trim_from_lane(
        &mut self,
        target: LaneTrimTarget,
        viewport: Viewport,
        pointer_x: f32,
        window: &mut Window,
    ) {
        let LaneTrimTarget {
            track,
            clip_id,
            edge,
            loop_kind,
        } = target;
        let generation = self.project_generation;
        let Some(payload) = self.midi_trim_drag_for(track, clip_id, edge, loop_kind, viewport)
        else {
            self.clear_pointer_down(generation, clip_id, Some(edge));
            return;
        };
        if self.midi_trim_drag_current(&payload) {
            self.focus.focus(window);
            self.remember_pointer_down(generation, clip_id, Some(edge), pointer_x);
        } else {
            self.clear_pointer_down(generation, clip_id, Some(edge));
        }
    }

    pub(super) fn audio_trim_source_for(
        &self,
        track: TrackId,
        clip_id: hex_project::ClipId,
        edge: TrimEdge,
        loop_kind: Option<LoopHandleKind>,
        viewport: Viewport,
    ) -> Option<AudioTrimSource> {
        let state = self.state.as_ref()?;
        let lane = *self.drag_lanes.get(&track)?;
        let clip = state
            .project
            .track(track)?
            .clips()
            .iter()
            .find(|c| c.id() == clip_id)?;
        let asset = state.project.asset(clip.asset())?;
        let repeat_available = loop_kind.is_none()
            && edge == TrimEdge::Right
            && loop_handles::audio_repeat_available(clip, &state.project);
        Some(AudioTrimSource {
            loop_kind,
            repeat_available,
            generation: self.project_generation,
            revision: state.revision,
            track,
            clip: Rc::new(clip.clone()),
            asset_playback_frames: asset.playback_frames(),
            edge,
            lane,
            viewport,
            snap_grid: self.snap_grid,
        })
    }

    pub(super) fn midi_trim_drag_for(
        &self,
        track: TrackId,
        clip_id: hex_project::ClipId,
        edge: TrimEdge,
        loop_kind: Option<LoopHandleKind>,
        viewport: Viewport,
    ) -> Option<MidiTrimDrag> {
        let state = self.state.as_ref()?;
        let lane = *self.drag_lanes.get(&track)?;
        let clip = state
            .project
            .track(track)?
            .midi_clips()
            .iter()
            .find(|c| c.id() == clip_id)?;
        let repeat_available = loop_kind.is_none()
            && edge == TrimEdge::Right
            && loop_handles::midi_repeat_available(clip);
        Some(MidiTrimDrag {
            loop_kind,
            repeat_available,
            repeat: Rc::new(Cell::new(false)),
            canceled: Rc::new(Cell::new(false)),
            capture: Rc::new(Cell::new(None)),
            project_generation: self.project_generation,
            revision: state.revision,
            source_track: track,
            clip: Rc::new(clip.clone()),
            edge,
            source_lane: lane,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            stretch_modifier: Rc::new(Cell::new(false)),
            snap_modifier: Rc::new(Cell::new(false)),
            last_pointer_x: Rc::new(Cell::new(None)),
            valid_pointer_x: Rc::new(Cell::new(None)),
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
        })
    }

    /// Construct and activate an audio `TrimDrag` from an analytic lane press.
    pub(in crate::editor) fn start_audio_trim_drag_from_lane(
        &mut self,
        target: LaneTrimTarget,
        viewport: Viewport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(TrimDrag, gpui::Entity<TrimPreview>)> {
        let LaneTrimTarget {
            track,
            clip_id,
            edge,
            loop_kind,
        } = target;
        let project = self.state.as_ref()?.project.clone();
        let source = self.audio_trim_source_for(track, clip_id, edge, loop_kind, viewport)?;
        let lane = source.lane;
        let payload = source.payload();
        let capture = if self.trim_drag_current(&payload) {
            self.take_pointer_down(
                payload.project_generation,
                payload.clip.id(),
                Some(payload.edge),
            )
        } else {
            self.clear_pointer_down(
                payload.project_generation,
                payload.clip.id(),
                Some(payload.edge),
            );
            None
        };
        payload.capture.set(capture);
        let original_down_x = capture.map(|capture| capture.origin_x());
        let pointer_frame = original_down_x.and_then(|x| frame_at_x(x, lane, viewport));
        let delta = pointer_frame
            .zip(audio_grab_frame(&payload, &project))
            .map(|(pointer, edge_frame)| i128::from(pointer) - i128::from(edge_frame));
        *payload.grab_delta.borrow_mut() = delta;
        let pointer_x = f32::from(window.mouse_position().x);
        payload.last_pointer_x.set(Some(pointer_x));
        payload.snap_modifier.set(window.modifiers().platform);
        payload.stretch_modifier.set(window.modifiers().shift);
        payload
            .repeat
            .set(payload.repeat_available && window.modifiers().alt);
        let cursor = if payload.repeat.get() {
            gpui::CursorStyle::Loop
        } else if let Some(kind) = loop_kind {
            match kind {
                LoopHandleKind::FirstBoundary => gpui::CursorStyle::ResizeLeftRight,
                LoopHandleKind::Outer => gpui::CursorStyle::Loop,
            }
        } else {
            super::super::drag::rectangle_part(Some(edge)).cursor()
        };
        window.defer(cx, move |window, cx| {
            cx.set_active_drag_cursor_style(cursor, window);
        });
        self.begin_trim_drag(&payload);
        self.update_trim_preview(&payload, pointer_x, cx);
        let ghost = cx.new(|_| TrimPreview {
            data: payload.preview.clone(),
        });
        Some((payload, ghost))
    }

    /// Construct and activate a `MidiTrimDrag` from an analytic lane press.
    pub(in crate::editor) fn start_midi_trim_drag_from_lane(
        &mut self,
        target: LaneTrimTarget,
        viewport: Viewport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(MidiTrimDrag, gpui::Entity<TrimPreview>)> {
        let LaneTrimTarget {
            track,
            clip_id,
            edge,
            loop_kind,
        } = target;
        let project = self.state.as_ref()?.project.clone();
        let payload = self.midi_trim_drag_for(track, clip_id, edge, loop_kind, viewport)?;
        let lane = payload.source_lane;
        let capture = if self.midi_trim_drag_current(&payload) {
            self.take_pointer_down(payload.project_generation, clip_id, Some(edge))
        } else {
            self.clear_pointer_down(payload.project_generation, clip_id, Some(edge));
            None
        };
        payload.capture.set(capture);
        let original_down_x = capture.map(|capture| capture.origin_x());
        let pointer_beat =
            original_down_x.and_then(|x| midi_pointer_beat(&project, x, lane, viewport));
        let delta = pointer_beat
            .zip(midi_grab_beat(&payload))
            .map(|(pointer, edge_beat)| i128::from(pointer.raw()) - i128::from(edge_beat.raw()));
        *payload.grab_delta.borrow_mut() = delta;
        let pointer_x = f32::from(window.mouse_position().x);
        payload.last_pointer_x.set(Some(pointer_x));
        payload.snap_modifier.set(window.modifiers().platform);
        payload.stretch_modifier.set(window.modifiers().shift);
        payload
            .repeat
            .set(payload.repeat_available && window.modifiers().alt);
        let cursor = if payload.repeat.get() {
            gpui::CursorStyle::Loop
        } else if let Some(kind) = loop_kind {
            match kind {
                LoopHandleKind::FirstBoundary => gpui::CursorStyle::ResizeLeftRight,
                LoopHandleKind::Outer => gpui::CursorStyle::Loop,
            }
        } else {
            super::super::drag::rectangle_part(Some(edge)).cursor()
        };
        window.defer(cx, move |window, cx| {
            cx.set_active_drag_cursor_style(cursor, window);
        });
        self.begin_midi_trim_drag(&payload);
        self.update_midi_trim_preview(&payload, pointer_x, cx);
        let ghost = cx.new(|_| TrimPreview {
            data: Rc::new(RefCell::new(None)),
        });
        Some((payload, ghost))
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::editor) fn trim_handles(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &Clip,
        visible_start: u64,
        visible_end: u64,
        clip_width: f32,
        viewport: Viewport,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        if !drag_admitted(self) || handle_width(clip_width) <= 0.0 {
            return Vec::new();
        }
        let Some(lane) = self.drag_lanes.get(&track).copied() else {
            return Vec::new();
        };
        let Some(asset) = state.project.asset(clip.asset()) else {
            return Vec::new();
        };
        let mut handles = Vec::with_capacity(4);
        let source = AudioTrimSource {
            loop_kind: None,
            repeat_available: false,
            generation: self.project_generation,
            revision: state.revision,
            track,
            clip: Rc::new(clip.clone()),
            asset_playback_frames: asset.playback_frames(),
            edge: TrimEdge::Left,
            lane,
            viewport,
            snap_grid: self.snap_grid,
        };
        if visible_start == clip.start() {
            handles.push(self.trim_handle(state, source.clone(), clip_width, None, scale, cx));
        }
        if let Some(end) = clip.start().checked_add(clip.duration())
            && visible_end == end
        {
            let mut right = source.clone();
            right.edge = TrimEdge::Right;
            right.repeat_available = loop_handles::audio_repeat_available(clip, &state.project);
            handles.push(self.trim_handle(state, right, clip_width, None, scale, cx));
        }
        if let Some(controls) = loop_handles::audio_controls(clip, &state.project, lane, viewport) {
            for control in controls.handles.into_iter().flatten() {
                let mut drag = source.clone();
                drag.edge = TrimEdge::Right;
                drag.loop_kind = Some(control.kind);
                handles.push(self.trim_handle(state, drag, clip_width, Some(control), scale, cx));
            }
        }
        handles
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::editor) fn midi_trim_handles(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &MidiClip,
        clip_start_frame: u64,
        clip_end_frame: u64,
        visible_start: u64,
        visible_end: u64,
        clip_width: f32,
        viewport: Viewport,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        if !drag_admitted(self) || handle_width(clip_width) <= 0.0 {
            return Vec::new();
        }
        let Some(lane) = self.drag_lanes.get(&track).copied() else {
            return Vec::new();
        };
        let drag = MidiTrimDrag {
            loop_kind: None,
            repeat_available: false,
            repeat: Rc::new(Cell::new(false)),
            canceled: Rc::new(Cell::new(false)),
            capture: Rc::new(Cell::new(None)),
            project_generation: self.project_generation,
            revision: state.revision,
            source_track: track,
            clip: Rc::new(clip.clone()),
            edge: TrimEdge::Left,
            source_lane: lane,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            stretch_modifier: Rc::new(Cell::new(false)),
            snap_modifier: Rc::new(Cell::new(false)),
            last_pointer_x: Rc::new(Cell::new(None)),
            valid_pointer_x: Rc::new(Cell::new(None)),
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
        };
        let mut handles = Vec::with_capacity(4);
        if visible_start == clip_start_frame {
            handles.push(self.midi_trim_handle(
                state,
                drag.clone(),
                TrimEdge::Left,
                clip_width,
                lane,
                viewport,
                None,
                scale,
                cx,
            ));
        }
        if visible_end == clip_end_frame {
            let mut right = drag.clone();
            right.edge = TrimEdge::Right;
            right.repeat_available = loop_handles::midi_repeat_available(clip);
            handles.push(self.midi_trim_handle(
                state,
                right,
                TrimEdge::Right,
                clip_width,
                lane,
                viewport,
                None,
                scale,
                cx,
            ));
        }
        if let Some(controls) = loop_handles::midi_controls(clip, &state.project, lane, viewport) {
            for control in controls.handles.into_iter().flatten() {
                let mut payload = drag.clone();
                payload.edge = TrimEdge::Right;
                payload.loop_kind = Some(control.kind);
                handles.push(self.midi_trim_handle(
                    state,
                    payload,
                    TrimEdge::Right,
                    clip_width,
                    lane,
                    viewport,
                    Some(control),
                    scale,
                    cx,
                ));
            }
        }
        handles
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn midi_trim_handle(
        &self,
        state: &ViewState,
        payload: MidiTrimDrag,
        edge: TrimEdge,
        clip_width: f32,
        lane: LaneBounds,
        viewport: Viewport,
        control: Option<loop_handles::LoopControl>,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let clip_id = payload.clip.id();
        let clip_generation = payload.project_generation;
        let drag_entity = cx.entity();
        let preview_owner_entity = drag_entity.clone();
        let mouse_down_entity = drag_entity.clone();
        let drag_start_entity = drag_entity.clone();
        let pointer_payload = payload.clone();
        let clear_entity = drag_entity.clone();
        let project_tempo = state.project.clone();
        let (element_prefix, debug_prefix) = match payload.loop_kind {
            Some(LoopHandleKind::Outer) => ("midi-loop-outer", "midi-clip-loop-outer"),
            Some(LoopHandleKind::FirstBoundary) => ("midi-loop-cycle", "midi-clip-loop-cycle"),
            None => match edge {
                TrimEdge::Left => ("midi-trim-left", "midi-clip-trim-left"),
                TrimEdge::Right => ("midi-trim-right", "midi-clip-trim-right"),
            },
        };
        let repeat_hover = control.is_none()
            && edge == TrimEdge::Right
            && payload.repeat_available
            && self.option_held;
        let repeat_available = payload.repeat_available;
        loop_handles::base(edge, clip_width, lane.height, control, repeat_hover, scale)
            .id((element_prefix, clip_id.value()))
            .debug_selector(move || format!("{debug_prefix}-{}", clip_id.value()))
            .map(|surface| loop_handles::decorate(surface, control, edge, repeat_available))
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                mouse_down_entity.update(cx, |view, _| {
                    if view.midi_trim_drag_current(&pointer_payload) {
                        view.focus.focus(window);
                        view.remember_pointer_down(
                            clip_generation,
                            clip_id,
                            Some(edge),
                            f32::from(event.position.x),
                        );
                    } else {
                        view.clear_pointer_down(clip_generation, clip_id, Some(edge));
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                clear_entity.update(cx, |view, _| {
                    view.clear_pointer_down(clip_generation, clip_id, Some(edge));
                });
            })
            .on_drag(payload, move |payload, _, window, cx| {
                let capture = drag_start_entity.update(cx, |view, _| {
                    if view.midi_trim_drag_current(payload) {
                        view.take_pointer_down(clip_generation, clip_id, Some(edge))
                    } else {
                        view.clear_pointer_down(clip_generation, clip_id, Some(edge));
                        None
                    }
                });
                payload.capture.set(capture);
                let original_down_x = capture.map(|capture| capture.origin_x());
                let pointer_beat = original_down_x
                    .and_then(|x| midi_pointer_beat(&project_tempo, x, lane, viewport));
                let delta =
                    pointer_beat
                        .zip(midi_grab_beat(payload))
                        .map(|(pointer, edge_beat)| {
                            i128::from(pointer.raw()) - i128::from(edge_beat.raw())
                        });
                *payload.grab_delta.borrow_mut() = delta;
                let pointer_x = f32::from(window.mouse_position().x);
                payload.last_pointer_x.set(Some(pointer_x));
                payload.snap_modifier.set(window.modifiers().platform);
                payload.stretch_modifier.set(window.modifiers().shift);
                // The mode is fixed here: option repeats, ahead of shift-stretch.
                payload
                    .repeat
                    .set(payload.repeat_available && window.modifiers().alt);
                if payload.repeat.get() {
                    // GPUI installs the drag (with the edge's cursor) after
                    // this callback, so switch its cursor once it exists.
                    window.defer(cx, |window, cx| {
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::Loop, window);
                    });
                }
                preview_owner_entity.update(cx, |view, cx| {
                    view.begin_midi_trim_drag(payload);
                    view.update_midi_trim_preview(payload, pointer_x, cx);
                });
                cx.new(|_| TrimPreview {
                    data: Rc::new(RefCell::new(None)),
                })
            })
    }

    pub(super) fn trim_handle(
        &self,
        state: &ViewState,
        source: AudioTrimSource,
        clip_width: f32,
        control: Option<loop_handles::LoopControl>,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let clip_id = source.clip.id();
        let generation = source.generation;
        let edge = source.edge;
        let lane = source.lane;
        let viewport = source.viewport;
        let project = state.project.clone();
        let drag_entity = cx.entity();
        let preview_owner_entity = drag_entity.clone();
        let mouse_down_entity = drag_entity.clone();
        let drag_start_entity = drag_entity.clone();
        let press_source = source.clone();
        let clear_entity = drag_entity.clone();
        let (element_prefix, debug_prefix) = match source.loop_kind {
            Some(LoopHandleKind::Outer) => ("loop-outer", "clip-loop-outer"),
            Some(LoopHandleKind::FirstBoundary) => ("loop-cycle", "clip-loop-cycle"),
            None => match edge {
                TrimEdge::Left => ("trim-left", "clip-trim-left"),
                TrimEdge::Right => ("trim-right", "clip-trim-right"),
            },
        };
        let repeat_hover = control.is_none()
            && edge == TrimEdge::Right
            && source.repeat_available
            && self.option_held;
        let repeat_available = source.repeat_available;
        loop_handles::base(edge, clip_width, lane.height, control, repeat_hover, scale)
            .id((element_prefix, clip_id.value()))
            .debug_selector(move || format!("{debug_prefix}-{}", clip_id.value()))
            .map(|surface| loop_handles::decorate(surface, control, edge, repeat_available))
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                mouse_down_entity.update(cx, |view, _| {
                    if view.trim_drag_current(&press_source.payload()) {
                        view.focus.focus(window);
                        view.remember_pointer_down(
                            generation,
                            clip_id,
                            Some(edge),
                            f32::from(event.position.x),
                        );
                    } else {
                        view.clear_pointer_down(generation, clip_id, Some(edge));
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                clear_entity.update(cx, |view, _| {
                    view.clear_pointer_down(generation, clip_id, Some(edge));
                });
            })
            .on_drag_from_press(move |_, _, window, cx| {
                let payload = source.payload();
                let capture = drag_start_entity.update(cx, |view, _| {
                    if view.trim_drag_current(&payload) {
                        view.take_pointer_down(
                            payload.project_generation,
                            payload.clip.id(),
                            Some(payload.edge),
                        )
                    } else {
                        view.clear_pointer_down(
                            payload.project_generation,
                            payload.clip.id(),
                            Some(payload.edge),
                        );
                        None
                    }
                });
                payload.capture.set(capture);
                let original_down_x = capture.map(|capture| capture.origin_x());
                let pointer_frame = original_down_x.and_then(|x| frame_at_x(x, lane, viewport));
                let delta = pointer_frame
                    .zip(audio_grab_frame(&payload, &project))
                    .map(|(pointer, edge)| i128::from(pointer) - i128::from(edge));
                *payload.grab_delta.borrow_mut() = delta;
                let pointer_x = f32::from(window.mouse_position().x);
                payload.last_pointer_x.set(Some(pointer_x));
                payload.snap_modifier.set(window.modifiers().platform);
                payload.stretch_modifier.set(window.modifiers().shift);
                // The mode is fixed here: option repeats, ahead of shift-stretch.
                payload
                    .repeat
                    .set(payload.repeat_available && window.modifiers().alt);
                if payload.repeat.get() {
                    // GPUI installs the drag (with the edge's cursor) after
                    // this callback, so switch its cursor once it exists.
                    window.defer(cx, |window, cx| {
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::Loop, window);
                    });
                }
                preview_owner_entity.update(cx, |view, cx| {
                    view.begin_trim_drag(&payload);
                    view.update_trim_preview(&payload, pointer_x, cx);
                });
                let ghost = cx.new(|_| TrimPreview {
                    data: payload.preview.clone(),
                });
                Some((payload, ghost))
            })
    }

    pub(in crate::editor) fn refresh_trim_pointer(
        &mut self,
        drag: &TrimDrag,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.trim_drag_current(drag) {
            replace_audio_trim_preview(&drag.preview, None);
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let pointer_x = f32::from(event.position.x);
        drag.last_pointer_x.set(Some(pointer_x));
        drag.snap_modifier.set(event.modifiers.platform);
        drag.stretch_modifier.set(event.modifiers.shift);
        self.update_trim_preview(drag, pointer_x, cx);
    }

    pub(in crate::editor) fn refresh_midi_trim_pointer(
        &mut self,
        drag: &MidiTrimDrag,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.midi_trim_drag_current(drag) {
            replace_midi_trim_preview(&drag.preview, None);
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let pointer_x = f32::from(event.position.x);
        drag.last_pointer_x.set(Some(pointer_x));
        drag.snap_modifier.set(event.modifiers.platform);
        drag.stretch_modifier.set(event.modifiers.shift);
        self.update_midi_trim_preview(drag, pointer_x, cx);
    }

    pub(in crate::editor) fn cancel_trim_gesture(&mut self, cx: &mut Context<Self>) -> bool {
        self.retire_trim_gesture(true, cx)
    }

    pub(super) fn normalize_canceled_trim_viewport(
        &mut self,
        generation: u64,
        owned: Option<Viewport>,
    ) {
        if generation == self.project_generation
            && owned.is_some()
            && self.viewport == owned
            && self.active_trim_drag.is_none()
            && self.midi_editor.active_trim_drag.is_none()
            && let Some(horizon) = self.state.as_ref().map(|state| state.timeline_frames)
        {
            // The active display-horizon owner is already retired. Persist the
            // actual canonical clamp so later unrelated growth cannot resurrect
            // an offset that was concealed by visible_viewport's current clamp.
            self.viewport = Some(self.visible_viewport(horizon));
        }
    }

    pub(super) fn retire_trim_gesture(
        &mut self,
        canceled_view: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let audio = self.active_trim_drag.take();
        let midi = self.midi_editor.active_trim_drag.take();
        let canceled = audio.is_some() || midi.is_some();
        if canceled {
            self.hovered_arrangement_clip = None;
        }
        if let Some(drag) = audio {
            if canceled_view {
                self.service.audition_arrangement_stretch(None);
            }
            drag.canceled.set(true);
            drag.preview.borrow_mut().take();
            *drag.resolution.borrow_mut() = Default::default();
            if canceled_view {
                self.normalize_canceled_trim_viewport(
                    drag.project_generation,
                    drag.scroll.get().viewport,
                );
            }
        }
        if let Some(drag) = midi {
            drag.canceled.set(true);
            drag.preview.borrow_mut().take();
            if canceled_view {
                self.normalize_canceled_trim_viewport(
                    drag.project_generation,
                    drag.scroll.get().viewport,
                );
            }
        }
        if canceled {
            cx.notify();
        }
        canceled
    }

    pub(in crate::editor) fn begin_trim_drag(&mut self, drag: &TrimDrag) {
        self.active_trim_drag = Some(drag.clone());
    }

    pub(in crate::editor) fn clear_trim_drag(
        &mut self,
        generation: u64,
        clip: hex_project::ClipId,
        cx: &mut Context<Self>,
    ) {
        if self.active_trim_drag.as_ref().is_some_and(|active| {
            active.project_generation == generation && active.clip.id() == clip
        }) {
            self.cancel_trim_gesture(cx);
        }
    }

    pub(in crate::editor) fn update_trim_preview(
        &mut self,
        drag: &TrimDrag,
        pointer_x: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pointer_x) = super::super::drag::PointerDown::pointer(&drag.capture, pointer_x)
        else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(lane) = self.drag_lanes.get(&drag.source_track).copied() else {
            return false;
        };
        if !payload_is_current(drag, self.project_generation, state, viewport, Some(lane)) {
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return false;
        }
        if !pointer_in_lane_x(pointer_x, lane) {
            return false;
        }
        let next = preview_for_pointer(drag, &state.project, pointer_x, lane, viewport);
        if next.is_some() {
            drag.valid_pointer_x.set(Some(pointer_x));
        } else {
            let reason = snapped_edge_frame(drag, &state.project, pointer_x, lane, viewport)
                .and_then(|frame| preview_clip(drag, &state.project, frame).err());
            let held = retained_preview(drag).is_some();
            self.status = match (held, reason) {
                (true, Some(hex_project::Invalid(reason))) => {
                    format!("resize held at the last valid length · {reason}")
                }
                (true, None) => "resize held at the last valid length".into(),
                (false, Some(hex_project::Invalid(reason))) => format!("resize refused · {reason}"),
                (false, None) => self.status.clone(),
            };
            if held {
                // Hold the last valid extent rather than dropping the gesture.
                self.active_trim_drag = Some(drag.clone());
                cx.notify();
                return false;
            }
        }
        let changed = replace_audio_trim_preview(&drag.preview, next);
        self.active_trim_drag = Some(drag.clone());
        self.sync_held_stretch_audition();
        if changed {
            cx.notify();
        }
        changed
    }

    pub(in crate::editor) fn sync_held_stretch_audition(&self) {
        let Some(drag) = self.active_trim_drag.as_ref() else {
            return;
        };
        let playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Acquire));
        let preview = drag.preview.borrow();
        let target = preview
            .as_ref()
            .filter(|preview| {
                playing
                    && self.mode == super::super::EditorMode::Arrangement
                    && self.document_audio_mode == hex_session::DocumentAudioMode::Resident
                    && drag.stretch_modifier.get()
                    && effective_loop_kind(drag).is_none()
                    && preview.stretch
                    && self.trim_drag_current(drag)
                    && !preview_is_noop(&drag.clip, &preview.clip)
            })
            .map(|preview| hex_session::service::ArrangementStretchAudition {
                context: hex_engine::AudioSourceContext {
                    project_generation: drag.project_generation,
                    editor_revision: drag.revision,
                },
                clip: drag.clip.id(),
                edge: drag.edge,
                frame: preview.command_frame,
            });
        self.service.audition_arrangement_stretch(target);
    }

    pub(in crate::editor) fn refresh_trim_snap_modifier(
        &mut self,
        held: bool,
        stretch: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.active_trim_drag.clone()
            && {
                let snap_changed = drag.snap_modifier.replace(held) != held;
                let stretch_changed = drag.stretch_modifier.replace(stretch) != stretch;
                snap_changed || stretch_changed
            }
        {
            // End the temporary preview borrow before the mutable update.
            let pointer = retained_timeline_pointer(
                drag.last_pointer_x.get(),
                drag.valid_pointer_x.get(),
                drag.preview.borrow().is_some(),
                drag.source_lane,
            );
            if let Some(pointer_x) = pointer {
                self.update_trim_preview(&drag, pointer_x, cx);
            }
        }
    }
}
