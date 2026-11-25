//! Analytic O(log N) track-lane hit testing and pointer gesture dispatch.
use super::*;

impl EditorView {
    /// Analytically hit-test resting clips on `track_id` in `O(log N)` using
    /// the revision-cached `ArrangementClipIndex`.
    pub(in crate::editor) fn hit_test_lane_clip(
        &self,
        track_id: TrackId,
        position: Point<Pixels>,
        viewport: Viewport,
        scale: f32,
    ) -> Option<LaneHit> {
        let state = self.state.as_ref()?;
        let &lane = self.drag_lanes.get(&track_id)?;
        let viewport_end = viewport.end()?;
        if viewport.length == 0 || lane.width <= 0.0 || lane.height <= 0.0 {
            return None;
        }
        let px_x = f32::from(position.x);
        let px_y = f32::from(position.y);
        if px_x < lane.left
            || px_x > lane.left + lane.width
            || px_y < lane.top
            || px_y > lane.top + lane.height
        {
            return None;
        }
        let local_x = px_x - lane.left;
        let local_y = px_y - lane.top;
        let track = state.project.track(track_id)?;

        // Ensure index is populated (uses interior mutability on RefCell).
        {
            let mut idx = self.arrangement_clip_index.borrow_mut();
            idx.ensure_current(self.project_generation, state);
        }
        let index_ref = self.arrangement_clip_index.borrow();
        let track_index_data = index_ref.track(track_id)?;

        // Query a narrow frame window around pointer X (margin of 2 logical px)
        // so sub-pixel clips and boundary handles are always included.
        let margin_frames = ((2.0 / f64::from(lane.width)) * viewport.length as f64).ceil() as u64;
        let frac = (f64::from(local_x) / f64::from(lane.width)).clamp(0.0, 1.0);
        let pointer_frame = viewport
            .start
            .saturating_add((frac * viewport.length as f64) as u64);
        let query_start = pointer_frame
            .saturating_sub(margin_frames)
            .max(viewport.start);
        let query_end = pointer_frame
            .saturating_add(margin_frames.max(1))
            .min(viewport_end);

        let candidates = track_index_data.candidate_slice(query_start, query_end);
        let trim_admitted = trim::drag_admitted(self);
        let header_hit_h = super::super::time_selection::header_hit_height(lane.height);
        let edge_trim_h = trim::edge_trim_height(lane.height);
        let track_height = self.track_height(track_id);

        // Reverse order so later-painted clips win ties just like DOM order.
        for &entry in candidates.iter().rev() {
            if entry.end <= viewport.start || entry.start >= viewport_end {
                continue;
            }
            if !self.arrangement_clip_visible(entry.id) {
                continue;
            }
            let vis_start = entry.start.max(viewport.start);
            let vis_end = entry.end.min(viewport_end);
            if vis_start >= vis_end {
                continue;
            }
            let left_frac = (vis_start - viewport.start) as f64 / viewport.length as f64;
            let width_frac = (vis_end - vis_start) as f64 / viewport.length as f64;
            let raw_left = f64::from(lane.left) + left_frac * f64::from(lane.width);
            let raw_right = f64::from(lane.left) + (left_frac + width_frac) * f64::from(lane.width);
            let raw_width = trim::rendered_clip_width(lane.width, width_frac);
            let raw_clip_left = raw_left as f32;
            let raw_clip_width = raw_width;
            let narrow = narrow_clips::is_narrow(raw_width, scale);
            let clip_left = snap_px(raw_left, scale);
            let clip_right = snap_px(raw_right, scale);
            let clip_width = if narrow {
                (clip_right - clip_left).max(1.0 / scale.max(1.0))
            } else {
                (clip_right - clip_left).max(0.0)
            };
            // Allow sub-pixel clips to be clicked at their rendered pixel column.
            let hit_width = clip_width.max(1.0 / scale.max(1.0));
            if px_x < clip_left || px_x > clip_left + hit_width {
                continue;
            }
            let rel_x = px_x - clip_left;
            let is_midi = matches!(entry.kind, IndexedClipKind::Midi(_));

            // 1. Audio clip gain handle (top 4px strip, inset 9px each side, width >= 28).
            if !is_midi
                && !narrow
                && clip_width >= 28.0
                && track_height > super::super::track_layout::HEADER_ONLY_HEIGHT
                && !self.fade_key_held
                && !self.time_selection_key_held
                && local_y < 4.0
                && rel_x >= 9.0
                && rel_x <= clip_width - 9.0
            {
                let cursor = if self.option_held {
                    CursorStyle::ResizeUpDown
                } else if self.drag_available() {
                    CursorStyle::OpenHand
                } else {
                    CursorStyle::Arrow
                };
                return Some(LaneHit {
                    clip_id: entry.id,
                    is_midi,
                    clip_start: entry.start,
                    clip_end: entry.end,
                    visible_start: vis_start,
                    visible_end: vis_end,
                    clip_left,
                    clip_width,
                    raw_clip_left,
                    raw_clip_width,
                    zone: LaneHitZone::GainHandle,
                    repeat_available: false,
                    cursor,
                });
            }

            // 2. Trim & loop handles when admitted and wide enough.
            let hw = if trim_admitted {
                trim::handle_width(clip_width)
            } else {
                0.0
            };
            if hw > 0.0 && local_y < edge_trim_h {
                if vis_start == entry.start && rel_x <= hw {
                    return Some(LaneHit {
                        clip_id: entry.id,
                        is_midi,
                        clip_start: entry.start,
                        clip_end: entry.end,
                        visible_start: vis_start,
                        visible_end: vis_end,
                        clip_left,
                        clip_width,
                        raw_clip_left,
                        raw_clip_width,
                        zone: LaneHitZone::TrimLeft,
                        repeat_available: false,
                        cursor: drag::rectangle_part(Some(TrimEdge::Left)).cursor(),
                    });
                }
                if vis_end == entry.end && rel_x >= clip_width - hw {
                    let repeat_available = match entry.kind {
                        IndexedClipKind::Audio(i) => track.clips().get(i).is_some_and(|c| {
                            loop_handles::audio_repeat_available(c, &state.project)
                        }),
                        IndexedClipKind::Midi(i) => track
                            .midi_clips()
                            .get(i)
                            .is_some_and(loop_handles::midi_repeat_available),
                    };
                    let cursor = if self.option_held && repeat_available {
                        CursorStyle::Loop
                    } else {
                        drag::rectangle_part(Some(TrimEdge::Right)).cursor()
                    };
                    return Some(LaneHit {
                        clip_id: entry.id,
                        is_midi,
                        clip_start: entry.start,
                        clip_end: entry.end,
                        visible_start: vis_start,
                        visible_end: vis_end,
                        clip_left,
                        clip_width,
                        raw_clip_left,
                        raw_clip_width,
                        zone: LaneHitZone::TrimRight,
                        repeat_available,
                        cursor,
                    });
                }
            }

            // 2b. Loop first-repeat-boundary handle in body.
            if hw > 0.0 {
                let controls = match entry.kind {
                    IndexedClipKind::Audio(i) => track.clips().get(i).and_then(|c| {
                        loop_handles::audio_controls(c, &state.project, lane, viewport)
                    }),
                    IndexedClipKind::Midi(i) => track.midi_clips().get(i).and_then(|c| {
                        loop_handles::midi_controls(c, &state.project, lane, viewport)
                    }),
                };
                if let Some(controls) = controls {
                    for handle in controls.handles.into_iter().flatten() {
                        if rel_x >= handle.left
                            && rel_x < handle.left + loop_handles::WIDTH
                            && local_y >= handle.top
                            && local_y < handle.top + handle.height
                        {
                            let cursor = match handle.kind {
                                LoopHandleKind::FirstBoundary => CursorStyle::ResizeLeftRight,
                                LoopHandleKind::Outer => CursorStyle::Loop,
                            };
                            return Some(LaneHit {
                                clip_id: entry.id,
                                is_midi,
                                clip_start: entry.start,
                                clip_end: entry.end,
                                visible_start: vis_start,
                                visible_end: vis_end,
                                clip_left,
                                clip_width,
                                raw_clip_left,
                                raw_clip_width,
                                zone: LaneHitZone::LoopHandle(handle.kind),
                                repeat_available: false,
                                cursor,
                            });
                        }
                    }
                }
            }

            // 3. Clip name header strip vs move surface vs body.
            let grab_cursor = if self.drag_available() {
                CursorStyle::OpenHand
            } else {
                CursorStyle::Arrow
            };
            let (zone, cursor) = if local_y < super::super::clip_name::CLIP_NAME_HEIGHT {
                (LaneHitZone::Header, grab_cursor)
            } else if local_y < header_hit_h {
                (LaneHitZone::MoveSurface, grab_cursor)
            } else {
                (LaneHitZone::Body, CursorStyle::Arrow)
            };
            return Some(LaneHit {
                clip_id: entry.id,
                is_midi,
                clip_start: entry.start,
                clip_end: entry.end,
                visible_start: vis_start,
                visible_end: vis_end,
                clip_left,
                clip_width,
                raw_clip_left,
                raw_clip_width,
                zone,
                repeat_available: false,
                cursor,
            });
        }
        None
    }

    /// Wrap a track lane's `TrackLaneCanvas` + optional live overlays with
    /// lane-level analytic pointer and drag handlers (3 lightweight wrappers
    /// per visible track instead of thousands of per-clip widgets).
    pub(in crate::editor) fn wrap_track_lane_canvas(
        &self,
        track_id: TrackId,
        viewport: Viewport,
        scale_factor: f32,
        canvas: TrackLaneCanvas,
        overlays: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let generation = self.project_generation;
        let lane_cursor = self
            .hovered_arrangement_clip
            .filter(|h| h.generation == generation && h.track == track_id)
            .map_or(CursorStyle::Arrow, |h| h.resolved_cursor(self.option_held));

        let inner = div()
            .id(("lane-canvas-surface", track_id.value()))
            .absolute()
            .inset_0()
            .cursor(lane_cursor)
            .child(canvas)
            .children(overlays)
            .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
                let hit = view.hit_test_lane_clip(track_id, event.position, viewport, scale_factor);
                let next_hover = hit.map(|h| HoveredArrangementClip {
                    generation,
                    track: track_id,
                    clip: h.clip_id,
                    zone: h.zone,
                    repeat_available: h.repeat_available,
                    cursor: h.cursor,
                });
                let mut changed = false;
                if view.hovered_arrangement_clip != next_hover {
                    view.hovered_arrangement_clip = next_hover;
                    changed = true;
                }
                if let Some(h) = hit.filter(|h| !h.is_midi) {
                    if view.update_fade_hover(
                        track_id,
                        h.clip_id,
                        generation,
                        Some(f32::from(event.position.y)),
                    ) {
                        changed = true;
                    }
                } else if let Some((fade_gen, fade_clip)) = view.hovered_fade_clip
                    && fade_gen == generation
                    && view.state.as_ref().is_some_and(|s| {
                        s.project
                            .track(track_id)
                            .is_some_and(|t| t.clips().iter().any(|c| c.id() == fade_clip))
                    })
                    && view.update_fade_hover(track_id, fade_clip, generation, None)
                {
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                if !*hovered {
                    let mut changed = false;
                    if view
                        .hovered_arrangement_clip
                        .is_some_and(|h| h.track == track_id)
                    {
                        view.hovered_arrangement_clip = None;
                        changed = true;
                    }
                    if let Some((fade_gen, fade_clip)) = view.hovered_fade_clip
                        && fade_gen == generation
                        && view.state.as_ref().is_some_and(|s| {
                            s.project
                                .track(track_id)
                                .is_some_and(|t| t.clips().iter().any(|c| c.id() == fade_clip))
                        })
                        && view.update_fade_hover(track_id, fade_clip, generation, None)
                    {
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    if let Some(hit) =
                        view.hit_test_lane_clip(track_id, event.position, viewport, scale_factor)
                    {
                        cx.stop_propagation();
                        view.open_clip_context_menu(
                            event.position,
                            track_id,
                            hit.clip_id,
                            window,
                            cx,
                        );
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    let Some(hit) =
                        view.hit_test_lane_clip(track_id, event.position, viewport, scale_factor)
                    else {
                        return;
                    };
                    match hit.zone {
                        LaneHitZone::GainHandle => {
                            if event.modifiers.alt
                                && view.begin_clip_gain_mouse_down(
                                    track_id,
                                    hit.clip_id,
                                    viewport,
                                    event,
                                    window,
                                    cx,
                                )
                            {
                                cx.stop_propagation();
                            } else {
                                view.press_clip_move_from_lane(
                                    track_id,
                                    hit,
                                    viewport,
                                    event.position,
                                );
                            }
                        }
                        LaneHitZone::TrimLeft
                        | LaneHitZone::TrimRight
                        | LaneHitZone::LoopHandle(_) => {
                            let (edge, loop_kind) = match hit.zone {
                                LaneHitZone::TrimLeft => (TrimEdge::Left, None),
                                LaneHitZone::TrimRight => (TrimEdge::Right, None),
                                LaneHitZone::LoopHandle(kind) => (TrimEdge::Right, Some(kind)),
                                _ => return,
                            };
                            let target = trim::LaneTrimTarget {
                                track: track_id,
                                clip_id: hit.clip_id,
                                edge,
                                loop_kind,
                            };
                            if hit.is_midi {
                                view.press_midi_trim_from_lane(
                                    target,
                                    viewport,
                                    f32::from(event.position.x),
                                    window,
                                );
                            } else {
                                view.press_audio_trim_from_lane(
                                    target,
                                    viewport,
                                    f32::from(event.position.x),
                                    window,
                                );
                            }
                        }
                        LaneHitZone::Header | LaneHitZone::MoveSurface => {
                            view.press_clip_move_from_lane(track_id, hit, viewport, event.position);
                        }
                        LaneHitZone::Body => {}
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, event: &gpui::MouseUpEvent, _, _| {
                    if let Some(hit) =
                        view.hit_test_lane_clip(track_id, event.position, viewport, scale_factor)
                    {
                        let edge = match hit.zone {
                            LaneHitZone::TrimLeft => Some(TrimEdge::Left),
                            LaneHitZone::TrimRight | LaneHitZone::LoopHandle(_) => {
                                Some(TrimEdge::Right)
                            }
                            _ => None,
                        };
                        view.clear_pointer_down(generation, hit.clip_id, edge);
                    }
                }),
            )
            .on_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    if cx.has_active_drag() || view.active_clip_drag.is_some() {
                        return;
                    }
                    let Some(pos) = event.mouse_position() else {
                        return;
                    };
                    let Some(hit) = view.hit_test_lane_clip(track_id, pos, viewport, scale_factor)
                    else {
                        return;
                    };
                    match hit.zone {
                        LaneHitZone::Header
                        | LaneHitZone::MoveSurface
                        | LaneHitZone::GainHandle => {
                            if matches!(hit.zone, LaneHitZone::GainHandle) && event.modifiers().alt
                            {
                                cx.stop_propagation();
                                return;
                            }
                            view.clear_pointer_down(generation, hit.clip_id, None);
                            cx.stop_propagation();
                            if view.clip_name_is_editing(hit.clip_id) {
                                return;
                            }
                            if view.clip_name_edit.is_some() {
                                view.finish_clip_name(true, window, cx);
                            }
                            view.select_clip_header(
                                track_id,
                                hit.clip_id,
                                event.modifiers(),
                                window,
                                cx,
                            );
                            view.focus.focus(window);
                            cx.notify();
                        }
                        LaneHitZone::TrimLeft
                        | LaneHitZone::TrimRight
                        | LaneHitZone::LoopHandle(_) => {
                            let edge = match hit.zone {
                                LaneHitZone::TrimLeft => TrimEdge::Left,
                                LaneHitZone::TrimRight | LaneHitZone::LoopHandle(_) => {
                                    TrimEdge::Right
                                }
                                _ => unreachable!(),
                            };
                            view.clear_pointer_down(generation, hit.clip_id, Some(edge));
                            cx.stop_propagation();
                        }
                        LaneHitZone::Body => {}
                    }
                }),
            );

        if !self.drag_available() {
            return div().size_full().relative().child(inner);
        }

        let audio_trim_entity = cx.entity();
        let midi_trim_entity = cx.entity();
        let move_entity = cx.entity();

        let with_audio_trim = div()
            .id(("lane-audio-trim-drag", track_id.value()))
            .absolute()
            .inset_0()
            .child(inner)
            .on_drag_from_press(move |press, _, window, cx| {
                audio_trim_entity.update(cx, |view, cx| {
                    let hit =
                        view.hit_test_lane_clip(track_id, press.position, viewport, scale_factor)?;
                    if hit.is_midi {
                        return None;
                    }
                    let (edge, loop_kind) = match hit.zone {
                        LaneHitZone::TrimLeft => (TrimEdge::Left, None),
                        LaneHitZone::TrimRight => (TrimEdge::Right, None),
                        LaneHitZone::LoopHandle(kind) => (TrimEdge::Right, Some(kind)),
                        _ => return None,
                    };
                    view.start_audio_trim_drag_from_lane(
                        trim::LaneTrimTarget {
                            track: track_id,
                            clip_id: hit.clip_id,
                            edge,
                            loop_kind,
                        },
                        viewport,
                        window,
                        cx,
                    )
                })
            });

        let with_midi_trim = div()
            .id(("lane-midi-trim-drag", track_id.value()))
            .absolute()
            .inset_0()
            .child(with_audio_trim)
            .on_drag_from_press(move |press, _, window, cx| {
                midi_trim_entity.update(cx, |view, cx| {
                    let hit =
                        view.hit_test_lane_clip(track_id, press.position, viewport, scale_factor)?;
                    if !hit.is_midi {
                        return None;
                    }
                    let (edge, loop_kind) = match hit.zone {
                        LaneHitZone::TrimLeft => (TrimEdge::Left, None),
                        LaneHitZone::TrimRight => (TrimEdge::Right, None),
                        LaneHitZone::LoopHandle(kind) => (TrimEdge::Right, Some(kind)),
                        _ => return None,
                    };
                    view.start_midi_trim_drag_from_lane(
                        trim::LaneTrimTarget {
                            track: track_id,
                            clip_id: hit.clip_id,
                            edge,
                            loop_kind,
                        },
                        viewport,
                        window,
                        cx,
                    )
                })
            });

        let with_clip_move = div()
            .id(("lane-clip-move-drag", track_id.value()))
            .absolute()
            .inset_0()
            .child(with_midi_trim)
            .on_drop(cx.listener(move |view, drag: &ClipDrag, window, cx| {
                view.drop_clip(drag, track_id, window, cx);
            }))
            .on_drag_from_press(move |press, _, window, cx| {
                move_entity.update(cx, |view, cx| {
                    let hit =
                        view.hit_test_lane_clip(track_id, press.position, viewport, scale_factor)?;
                    if !matches!(
                        hit.zone,
                        LaneHitZone::Header | LaneHitZone::MoveSurface | LaneHitZone::GainHandle
                    ) {
                        return None;
                    }
                    view.start_clip_move_drag_from_lane(track_id, hit, viewport, window, cx)
                })
            });

        div().size_full().relative().child(with_clip_move)
    }

    pub(super) fn press_clip_move_from_lane(
        &mut self,
        track_id: TrackId,
        hit: LaneHit,
        viewport: Viewport,
        pos: Point<Pixels>,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(position) = self.clip_position_for(state, track_id, hit.clip_id, hit.is_midi)
        else {
            return;
        };
        let generation = self.project_generation;
        let revision = state.revision;
        let payload_check = ClipDrag {
            project_generation: generation,
            revision,
            source_track: track_id,
            clip: hit.clip_id,
            group: Default::default(),
            position,
            grab_offset: Default::default(),
            snap_edge: Default::default(),
            source_span: Default::default(),
            capture: Default::default(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Default::default(),
            copy_modifier: Default::default(),
            preview: Default::default(),
            destination: Default::default(),
            insertion_line_y: Default::default(),
            ghost_label: String::new(),
            ghost_waveform: None,
            lane_hovered: Default::default(),
            last_pointer: Default::default(),
        };
        if self.drag_start_is_current(&payload_check) {
            self.remember_pointer_down(generation, hit.clip_id, None, f32::from(pos.x));
            let edge = drag::clip_snap_edge(
                f32::from(pos.x),
                hit.clip_left,
                hit.clip_width,
                hit.visible_start,
                hit.visible_end,
                hit.clip_start,
                hit.clip_end,
            );
            if let Some(press) = self.pointer_down.as_mut() {
                press.snap_edge = edge;
            }
        } else {
            self.clear_pointer_down(generation, hit.clip_id, None);
        }
    }

    pub(super) fn clip_position_for(
        &self,
        state: &ViewState,
        track_id: TrackId,
        clip_id: ClipId,
        is_midi: bool,
    ) -> Option<ClipPosition> {
        let track = state.project.track(track_id)?;
        if is_midi {
            let clip = track.midi_clips().iter().find(|c| c.id() == clip_id)?;
            Some(ClipPosition::Midi {
                start: clip.start(),
                length: clip.length(),
            })
        } else {
            let clip = track.clips().iter().find(|c| c.id() == clip_id)?;
            Some(ClipPosition::Audio {
                start: clip.start(),
                duration: clip.duration(),
            })
        }
    }

    pub(super) fn start_clip_move_drag_from_lane(
        &mut self,
        track_id: TrackId,
        hit: LaneHit,
        viewport: Viewport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(ClipDrag, Entity<super::super::tracks::ClipDragGhost>)> {
        let state = self.state.as_ref()?.clone();
        let position = self.clip_position_for(&state, track_id, hit.clip_id, hit.is_midi)?;
        let kind = if hit.is_midi { "midi" } else { "audio" };
        let ghost_waveform = if !hit.is_midi {
            state
                .project
                .track(track_id)
                .and_then(|t| t.clips().iter().find(|c| c.id() == hit.clip_id))
                .filter(|c| c.audio_edit().is_none())
                .and_then(|c| state.waveforms.get(&c.asset()).cloned())
        } else {
            None
        };
        let source_start_beat = match position {
            ClipPosition::Midi { start, .. } => Some(start),
            ClipPosition::Audio { .. } => None,
        };
        let source_span = Rc::new(std::cell::Cell::new(Some((hit.clip_left, hit.clip_width))));
        let payload = ClipDrag {
            project_generation: self.project_generation,
            revision: state.revision,
            source_track: track_id,
            clip: hit.clip_id,
            group: Default::default(),
            position,
            grab_offset: Default::default(),
            snap_edge: Default::default(),
            source_span,
            capture: Default::default(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Default::default(),
            copy_modifier: Default::default(),
            preview: Default::default(),
            destination: Default::default(),
            insertion_line_y: Default::default(),
            ghost_label: format!("{kind} clip {}", hit.clip_id.value()),
            ghost_waveform,
            lane_hovered: Default::default(),
            last_pointer: Default::default(),
        };
        let capture = if self.drag_start_is_current(&payload) {
            self.take_pointer_down(payload.project_generation, payload.clip, None)
        } else {
            self.clear_pointer_down(payload.project_generation, payload.clip, None);
            None
        };
        payload.capture.set(capture);
        payload
            .snap_edge
            .set(capture.and_then(|press| press.snap_edge));
        let original_down_x = capture.map(drag::PointerDown::origin_x);
        let frame_offset = original_down_x.and_then(|x| {
            drag::grab_offset_from_pointer(
                x,
                hit.raw_clip_left,
                hit.raw_clip_width,
                hit.visible_start,
                hit.visible_end,
                hit.clip_start,
            )
        });
        let offset = match (payload.position, frame_offset, source_start_beat) {
            (ClipPosition::Audio { .. }, Some(offset), _) => Some(offset),
            (ClipPosition::Midi { .. }, Some(offset), Some(start_beat)) => hit
                .clip_start
                .checked_add(offset)
                .and_then(|grabbed_frame| {
                    state.project.tempo_map().frame_to_beat(grabbed_frame).ok()
                })
                .map(|beat| beat.raw().saturating_sub(start_beat.raw())),
            _ => None,
        };
        payload.grab_offset.set(offset);
        payload.snap_modifier.set(window.modifiers().platform);
        payload.copy_modifier.set(window.modifiers().alt);
        cx.set_active_drag_cursor_style(drag::copy_cursor(payload.copy_modifier.get()), window);
        let pointer = window.mouse_position();
        let pointer = (f32::from(pointer.x), f32::from(pointer.y));
        payload.last_pointer.set(Some(pointer));
        self.begin_clip_drag(&payload);
        let (preview, lane_hovered) = self.clip_drag_drop_position(&payload, pointer.0, pointer.1);
        payload.preview.set(preview);
        payload.lane_hovered.set(lane_hovered);
        self.sync_held_clip_move_audition();
        cx.notify();
        Some((payload, cx.new(|_| super::super::tracks::ClipDragGhost)))
    }
}
