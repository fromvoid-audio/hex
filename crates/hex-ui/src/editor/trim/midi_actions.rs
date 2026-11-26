//! MIDI clip trim gestures, pan viewports, and canonical trim release dispatch.
use super::*;

impl EditorView {
    pub(in crate::editor) fn begin_midi_trim_drag(&mut self, drag: &MidiTrimDrag) {
        self.midi_editor.active_trim_drag = Some(drag.clone());
    }

    pub(in crate::editor) fn clear_midi_trim_drag(
        &mut self,
        generation: u64,
        clip: hex_project::ClipId,
        cx: &mut Context<Self>,
    ) {
        if self
            .midi_editor
            .active_trim_drag
            .as_ref()
            .is_some_and(|active| {
                active.project_generation == generation && active.clip.id() == clip
            })
        {
            if let Some(drag) = self.midi_editor.active_trim_drag.take() {
                drag.canceled.set(true);
                drag.preview.borrow_mut().take();
                self.normalize_canceled_trim_viewport(
                    drag.project_generation,
                    drag.scroll.get().viewport,
                );
            }
            cx.notify();
        }
    }

    pub(in crate::editor) fn update_midi_trim_preview(
        &mut self,
        drag: &MidiTrimDrag,
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
        let Some(lane) = self
            .timeline_caches
            .drag_lanes
            .get(&drag.source_track)
            .copied()
        else {
            return false;
        };
        if !midi_payload_is_current(drag, self.project_generation, state, viewport, Some(lane)) {
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return false;
        }
        if !pointer_in_lane_x(pointer_x, lane) {
            return false;
        }
        let next = midi_preview_for_pointer(drag, &state.project, pointer_x, lane, viewport);
        if next.is_some() {
            drag.valid_pointer_x.set(Some(pointer_x));
        }
        let changed = replace_midi_trim_preview(&drag.preview, next);
        self.midi_editor.active_trim_drag = Some(drag.clone());
        if changed {
            cx.notify();
        }
        changed
    }

    pub(in crate::editor) fn refresh_midi_trim_snap_modifier(
        &mut self,
        held: bool,
        stretch: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.midi_editor.active_trim_drag.clone() {
            let snap_changed = drag.snap_modifier.replace(held) != held;
            let stretch_changed = drag.stretch_modifier.replace(stretch) != stretch;
            if snap_changed || stretch_changed {
                let pointer = retained_timeline_pointer(
                    drag.last_pointer_x.get(),
                    drag.valid_pointer_x.get(),
                    drag.preview.borrow().is_some(),
                    drag.source_lane,
                );
                if let Some(pointer_x) = pointer {
                    self.update_midi_trim_preview(&drag, pointer_x, cx);
                }
            }
        }
    }

    pub(in crate::editor) fn midi_trim_drag_current(&self, drag: &MidiTrimDrag) -> bool {
        if !drag_admitted(self) {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        midi_payload_is_current(
            drag,
            self.project_generation,
            state,
            self.visible_viewport(state.timeline_frames),
            self.timeline_caches
                .drag_lanes
                .get(&drag.source_track)
                .copied(),
        )
    }

    pub(in crate::editor) fn drop_midi_trim(
        &mut self,
        drag: &MidiTrimDrag,
        target_track: TrackId,
        cx: &mut Context<Self>,
    ) {
        if target_track != drag.source_track || !drag_admitted(self) {
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(lane) = self.timeline_caches.drag_lanes.get(&target_track).copied() else {
            return;
        };
        if !midi_payload_is_current(drag, self.project_generation, state, viewport, Some(lane)) {
            self.error = Some("midi clip trim canceled · project changed".into());
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            cx.notify();
            return;
        }
        let Some(pointer_x) = drag.last_pointer_x.get() else {
            return;
        };
        let Some(beat) = snapped_midi_edge_beat(drag, &state.project, pointer_x, lane, viewport)
        else {
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        };
        let Some(preview) = midi_edge_preview(drag, beat) else {
            return;
        };
        if (midi_loop_kind(drag).is_some() && midi_grab_beat(drag) == Some(beat))
            || (midi_loop_kind(drag).is_none()
                && preview.start() == drag.clip.start()
                && preview.length() == drag.clip.length()
                && preview.source_start() == drag.clip.source_start()
                && preview.playback_offset() == drag.clip.playback_offset())
        {
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let stretch = drag.stretch_modifier.get();
        let request = Request::Edit(if let Some(kind) = midi_loop_kind(drag) {
            Command::ResizeMidiClipView {
                clip: drag.clip.id(),
                operation: kind.midi_operation(beat),
            }
        } else if stretch {
            Command::StretchMidiClipEdge {
                clip: drag.clip.id(),
                edge: drag.edge,
                beat,
            }
        } else {
            Command::TrimMidiClipEdge {
                clip: drag.clip.id(),
                edge: drag.edge,
                beat,
            }
        });
        let Some(command) = ResizeCommand::from_request(&request) else {
            return;
        };
        self.interaction.pending_trim = Some(PendingTrim {
            replayed: false,
            generation: drag.project_generation,
            revision: drag.revision,
            track: drag.source_track,
            command,
            remaining: None,
            scroll_viewport: drag.scroll.get().viewport,
            display: PendingTrimDisplay::Midi {
                original: Rc::clone(&drag.clip),
                prospective: preview,
            },
        });
        let accepted = self.enqueue(request);
        if !accepted {
            self.interaction.pending_trim = None;
        }
        self.retire_trim_gesture(!accepted, cx);
        cx.notify();
    }

    /// Finalize the captured source, regardless of the release's hovered panel.
    /// Foreign horizontal coordinates retain the last valid timeline point;
    /// canonical validation and the existing pending-trim owner stay authoritative.
    pub(in crate::editor) fn finish_trim_pointer_release(
        &mut self,
        event: &gpui::MouseUpEvent,
        _browser_release: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != MouseButton::Left
            || (self.interaction.active_trim_drag.is_none()
                && self.midi_editor.active_trim_drag.is_none())
        {
            return false;
        }
        if self.mode != super::super::EditorMode::Arrangement || !self.focus.is_focused(window) {
            self.cancel_trim_gesture(cx);
            return true;
        }
        let x = f32::from(event.position.x);
        if !x.is_finite() || !f32::from(event.position.y).is_finite() {
            self.cancel_trim_gesture(cx);
            return true;
        }
        if let Some(drag) = self.interaction.active_trim_drag.clone() {
            drag.snap_modifier.set(event.modifiers.platform);
            drag.stretch_modifier.set(event.modifiers.shift);
            let pointer = retained_timeline_pointer(
                Some(x),
                drag.valid_pointer_x.get(),
                drag.preview.borrow().is_some(),
                drag.source_lane,
            );
            if let Some(pointer) = pointer {
                drag.last_pointer_x.set(Some(pointer));
                self.update_trim_preview(&drag, pointer, cx);
                if drag.preview.borrow().is_some() {
                    self.drop_trim(&drag, drag.source_track, window, cx);
                }
            }
        } else if let Some(drag) = self.midi_editor.active_trim_drag.clone() {
            drag.snap_modifier.set(event.modifiers.platform);
            drag.stretch_modifier.set(event.modifiers.shift);
            let pointer = retained_timeline_pointer(
                Some(x),
                drag.valid_pointer_x.get(),
                drag.preview.borrow().is_some(),
                drag.source_lane,
            );
            if let Some(pointer) = pointer {
                drag.last_pointer_x.set(Some(pointer));
                self.update_midi_trim_preview(&drag, pointer, cx);
                if drag.preview.borrow().is_some() {
                    self.drop_midi_trim(&drag, drag.source_track, cx);
                }
            }
        }
        // A refused/stale/invalid release retires its own capture; an accepted
        // release already transferred display ownership to PendingTrim.
        self.cancel_trim_gesture(cx);
        true
    }

    pub(in crate::editor) fn trim_pan_viewport(
        &self,
        drag: &TrimDrag,
        x: f32,
        next: Viewport,
    ) -> Option<Viewport> {
        let state = self.state.as_ref()?;
        let lane = self
            .timeline_caches
            .drag_lanes
            .get(&drag.source_track)
            .copied()?;
        let delta = (*drag.grab_delta.borrow())?;
        let candidate = |viewport| {
            let pointer_frame = frame_at_x(x, lane, viewport)?;
            let requested = i128::from(pointer_frame) - delta;
            let frame = corrected_edge_frame(pointer_frame, delta);
            let clip = preview_clip(drag, &state.project, frame).ok()?;
            let endpoint = if drag.loop_kind == Some(LoopHandleKind::FirstBoundary) {
                u64::try_from(
                    hex_project::AudioViewProjection::new(&clip, state.project.tempo_map())
                        .ok()?
                        .logical_first_boundary(),
                )
                .ok()?
            } else {
                edge_frame(&clip, drag.edge)?
            };
            let unclamped = requested == i128::from(frame) && endpoint == frame;
            Some((clip, unclamped))
        };
        resize_pan_prefix(drag.viewport(), next, candidate)
    }

    pub(in crate::editor) fn midi_trim_pan_viewport(
        &self,
        drag: &MidiTrimDrag,
        x: f32,
        next: Viewport,
    ) -> Option<Viewport> {
        let state = self.state.as_ref()?;
        let lane = self
            .timeline_caches
            .drag_lanes
            .get(&drag.source_track)
            .copied()?;
        let delta = (*drag.grab_delta.borrow())?;
        let candidate = |viewport| {
            let beat = midi_pointer_beat(&state.project, x, lane, viewport)?;
            let requested = i128::from(beat.raw()) - delta;
            let beat = BeatTime::from_raw(requested.clamp(0, i128::from(u64::MAX)) as u64);
            let clip = midi_edge_preview(drag, beat)?;
            let edge = if drag.loop_kind == Some(LoopHandleKind::FirstBoundary) {
                clip.boundary(&drag.clip)?.raw()
            } else {
                match drag.edge {
                    TrimEdge::Left => clip.start().raw(),
                    TrimEdge::Right => clip.start().raw().checked_add(clip.length().raw())?,
                }
            };
            Some((
                clip,
                requested == i128::from(beat.raw()) && edge == beat.raw(),
            ))
        };
        resize_pan_prefix(drag.viewport(), next, candidate)
    }

    /// A private resize may navigate beyond the current display extent, never
    /// beyond its checked viewport or into a stale owner's project/geometry.
    pub(in crate::editor) fn trim_display_end(&self) -> Option<u64> {
        if let Some(pending) = self.pending_trim_current()
            && let Some(viewport) = pending.scroll_viewport
            && self.viewport == Some(viewport)
        {
            return viewport.end();
        }
        if !drag_admitted(self) {
            return None;
        }
        let state = self.state.as_ref()?;
        if let Some(drag) = &self.interaction.active_trim_drag {
            let viewport = drag.scroll.get().viewport?;
            if self.viewport == Some(viewport)
                && payload_is_current(
                    drag,
                    self.project_generation,
                    state,
                    viewport,
                    self.timeline_caches
                        .drag_lanes
                        .get(&drag.source_track)
                        .copied(),
                )
            {
                return viewport.end();
            }
        }
        if let Some(drag) = &self.midi_editor.active_trim_drag {
            let viewport = drag.scroll.get().viewport?;
            if self.viewport == Some(viewport)
                && midi_payload_is_current(
                    drag,
                    self.project_generation,
                    state,
                    viewport,
                    self.timeline_caches
                        .drag_lanes
                        .get(&drag.source_track)
                        .copied(),
                )
            {
                return viewport.end();
            }
        }
        None
    }

    pub(in crate::editor) fn trim_drag_current(&self, drag: &TrimDrag) -> bool {
        if !drag_admitted(self) {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        payload_is_current(
            drag,
            self.project_generation,
            state,
            self.visible_viewport(state.timeline_frames),
            self.timeline_caches
                .drag_lanes
                .get(&drag.source_track)
                .copied(),
        )
    }

    pub(in crate::editor) fn drop_trim(
        &mut self,
        drag: &TrimDrag,
        target_track: TrackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            drag.canceled.set(true);
            self.cancel_trim_gesture(cx);
            return;
        }
        if target_track != drag.source_track || !drag_admitted(self) {
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(lane) = self.timeline_caches.drag_lanes.get(&target_track).copied() else {
            return;
        };
        if !payload_is_current(drag, self.project_generation, state, viewport, Some(lane)) {
            self.error = Some("clip trim canceled · project changed".into());
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            cx.notify();
            return;
        }
        // GPUI's MouseUpEvent carries the authoritative release point. Window's
        // cached pointer can still be the previous move-event location.
        let pointer_x = drag
            .last_pointer_x
            .get()
            .unwrap_or_else(|| f32::from(window.mouse_position().x));
        drag.last_pointer_x.set(Some(pointer_x));
        #[cfg(test)]
        {
            self.test_probes.last_trim_release_snap = Some(drag.snap_modifier.get());
        }
        let Some(frame) = snapped_edge_frame(drag, &state.project, pointer_x, lane, viewport)
        else {
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        };
        let Some(resolved) =
            resolve_preview(drag, &state.project, frame).or_else(|| retained_preview(drag))
        else {
            return;
        };
        let frame = resolved.command_frame;
        let preview = resolved.clip;
        if preview_is_noop(&drag.clip, &preview)
            || (effective_loop_kind(drag).is_some()
                && audio_grab_frame(drag, &state.project) == Some(frame))
        {
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let stretch = drag.stretch_modifier.get();
        let request = Request::Edit(if let Some(kind) = effective_loop_kind(drag) {
            Command::ResizeAudioClipView {
                clip: drag.clip.id(),
                operation: kind.audio_operation(frame),
            }
        } else if stretch {
            Command::StretchAudioClipEdge {
                clip: drag.clip.id(),
                edge: drag.edge,
                frame,
            }
        } else {
            Command::TrimClipEdge {
                clip: drag.clip.id(),
                edge: drag.edge,
                frame,
            }
        });
        let Some(command) = ResizeCommand::from_request(&request) else {
            return;
        };
        self.interaction.pending_trim = Some(PendingTrim {
            replayed: false,
            generation: drag.project_generation,
            revision: drag.revision,
            track: drag.source_track,
            command,
            remaining: None,
            scroll_viewport: drag.scroll.get().viewport,
            display: PendingTrimDisplay::Audio {
                original: drag.clip.clone(),
                prospective: Box::new(preview),
                asset_frames: drag.asset_playback_frames,
            },
        });
        self.service.finish_arrangement_stretch_audition();
        let accepted = self.enqueue(request);
        if !accepted {
            self.interaction.pending_trim = None;
        }
        self.retire_trim_gesture(!accepted, cx);
        cx.notify();
    }
}
