//! Section pointer capture, drag/resize gestures, request submission, menus and keyboard actions.
use super::*;

impl EditorView {
    pub(super) fn section_drag_cursor(&self) -> Option<gpui::CursorStyle> {
        let g = self.sections.gesture.as_ref()?;
        if !self.section_current(g) || !matches!(g.mode, Mode::Reorder(_) | Mode::Move(_)) {
            return None;
        }
        Some(
            if g.command.as_ref().and_then(copied_section_source).is_some() {
                crate::editor::drag::copy_cursor(true)
            } else if g.capture.displaced(4.0) {
                gpui::CursorStyle::ClosedHand
            } else {
                gpui::CursorStyle::OpenHand
            },
        )
    }
    /// The existing Window lease owns body move/copy input and cursor only.
    /// Release still transfers the original section display to its FIFO receipt.
    pub(in crate::editor) fn capture_section_drag(
        &self,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> bool {
        let Some(cursor) = self.section_drag_cursor() else {
            return true;
        };
        let Some(g) = self.sections.gesture.as_ref() else {
            return true;
        };
        let stamp = (g.generation, g.revision, g.capture.key, g.capture.origin_x);
        let matches = move |g: &Gesture| {
            (g.generation, g.revision, g.capture.key, g.capture.origin_x) == stamp
        };
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            gpui::MouseButton::Left,
            cursor,
            move |event, window, cx| {
                let moved = event.downcast_ref::<gpui::MouseMoveEvent>();
                let released = event
                    .downcast_ref::<gpui::MouseUpEvent>()
                    .filter(|e| e.button == gpui::MouseButton::Left);
                let Some((position, modifiers)) = moved
                    .map(|e| (e.position, e.modifiers))
                    .or_else(|| released.map(|e| (e.position, e.modifiers)))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.sections.gesture.as_ref().is_some_and(matches) {
                        return;
                    }
                    if !view.focus.is_focused(window) {
                        crate::diagnostics::record_section_capture(cx, "focus_cancel");
                        view.cancel_section_pointer();
                    } else if released.is_some() {
                        crate::diagnostics::record_section_capture(cx, "release");
                        view.finish_section_pointer(
                            f32::from(position.x),
                            f32::from(position.y),
                            modifiers,
                        );
                    } else {
                        view.update_section_pointer(
                            f32::from(position.x),
                            f32::from(position.y),
                            modifiers,
                        );
                    }
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.sections.gesture.as_ref().is_some_and(matches) {
                        crate::diagnostics::record_section_capture(cx, "cancel");
                        view.cancel_section_pointer();
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    pub(in crate::editor) fn section_pointer_events(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let entity = cx.entity();
        let empty = self
            .state
            .as_ref()
            .is_some_and(|state| state.project.tracks().is_empty());
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let view = entity.read(cx);
                if view.focus.is_focused(window)
                    && let Some(cursor) = view.section_drag_cursor()
                {
                    window.renew_pointer_capture(entity.entity_id(), cursor);
                }
                // Populated arrangements register their shared capture on the
                // existing body canvas; the empty layout has no body capture.
                if !empty {
                    return;
                }
                let down = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, window, cx| {
                    if !phase.capture() || event.button != gpui::MouseButton::Left {
                        return;
                    }
                    let captured = down.update(cx, |view, cx| {
                        if view.context_menu.is_some()
                            || view.gain_numeric_editing(cx)
                            || view.clip_numeric_editing(cx)
                            || view.project_tempo_editing(cx)
                            || view.pointer_hits_section_launch(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            )
                        {
                            return false;
                        }
                        let captured = view.begin_section_pointer(
                            f32::from(event.position.x),
                            f32::from(event.position.y),
                            event.click_count == 2,
                            event.modifiers,
                            cx,
                        );
                        if captured {
                            view.focus.focus(window);
                            if !view.capture_section_drag(window, cx) {
                                view.cancel_section_pointer();
                            }
                            cx.notify();
                        }
                        captured
                    });
                    if captured {
                        cx.stop_propagation();
                    }
                });
                let moved = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let changed = moved.update(cx, |view, cx| {
                        let changed = if event.pressed_button == Some(gpui::MouseButton::Left) {
                            view.update_section_pointer(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                                event.modifiers,
                            )
                        } else {
                            view.cancel_section_pointer()
                        };
                        if changed {
                            cx.notify();
                        }
                        changed
                    });
                    if changed {
                        cx.stop_propagation();
                    }
                });
                let up = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                    if !phase.capture() || event.button != gpui::MouseButton::Left {
                        return;
                    }
                    let captured = up.update(cx, |view, cx| {
                        let captured = view.finish_section_pointer(
                            f32::from(event.position.x),
                            f32::from(event.position.y),
                            event.modifiers,
                        );
                        if captured {
                            cx.notify();
                        }
                        captured
                    });
                    if captured {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    /// Arrangement rows display a frozen structural edit or committed copy or paste
    /// until its exact terminal installs the canonical state.
    pub(in crate::editor) fn arrangement_rows_state(&self) -> Option<Arc<ViewState>> {
        let state = self.state.as_ref()?;
        self.sections
            .ahead
            .as_ref()
            .filter(|p| p.generation == self.project_generation)
            .and_then(|p| p.layout.clone())
            .or_else(|| {
                self.sections
                    .pending
                    .as_ref()
                    .filter(|p| p.generation == self.project_generation)
                    .and_then(|p| p.layout.clone())
            })
            .or_else(|| self.arrangement_duplicate_view().cloned())
            .or_else(|| self.arrangement_paste_view().cloned())
            .or_else(|| self.time_range_copy_view().cloned())
            .or_else(|| Some(Arc::clone(state)))
    }

    /// Replace the projected rows and marker geometry with the exact committed
    /// copy. The ordinary receipt continues to own input, selection and history.
    pub(in crate::editor) fn observe_section_copy_view(
        &mut self,
        observation: ArrangementEditViewObservation,
        copy: SectionId,
    ) -> bool {
        let Some(current) = self.state.as_ref() else {
            return false;
        };
        let Some(primary) = self.sections.pending.as_ref() else {
            return false;
        };
        let base = if primary.revision == observation.previous_revision {
            Arc::clone(current)
        } else if self
            .sections
            .ahead
            .as_ref()
            .is_some_and(|p| p.revision == observation.previous_revision)
            && current.revision == primary.revision
        {
            let Some(base) = primary
                .layout
                .as_ref()
                .filter(|base| base.revision == observation.previous_revision)
            else {
                return false;
            };
            Arc::clone(base)
        } else {
            return false;
        };
        let pending = if primary.revision == observation.previous_revision {
            self.sections.pending.as_mut()
        } else {
            self.sections.ahead.as_mut()
        };
        let Some(pending) = pending else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || self.persistence.opening_project
            || pending.receipt.is_queued()
            || pending.generation != self.project_generation
            || observation.project_generation != pending.generation
            || observation.first_previous_revision != observation.previous_revision
            || observation.previous_revision != pending.revision
            || base.revision != pending.revision
            || !(matches!(
                &pending.command,
                Command::DuplicateArrangementSection(_) | Command::PasteArrangementSection { .. }
            ) || copied_section_source(&pending.command).is_some())
        {
            return false;
        }
        let Some(state) = observation.state.upgrade().filter(|state| {
            pending.revision.checked_add(1) == Some(state.revision)
                && base.project.arrangement_section(copy).is_none()
                && state.project.arrangement_section(copy).is_some()
        }) else {
            return false;
        };
        pending.display.markers = state
            .project
            .arrangement_sections()
            .iter()
            .map(|section| {
                let mut marker = Marker::from(section);
                // New markers keep their preview presentation and remain
                // noninteractive until the terminal installs their IDs.
                if section.id().value() >= current.project.next_identity() {
                    marker.id = None;
                }
                marker
            })
            .collect();
        pending.display.tempo = state.project.tempo_map().clone();
        pending.copied = Some(copy);
        pending.layout = Some(state);
        self.start_ahead_section_duplicate();
        true
    }

    pub(in crate::editor) fn committed_section_copy_view(&self) -> Option<&Arc<ViewState>> {
        let pending = self.sections.pending.as_ref()?;
        if pending.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != pending.revision)
        {
            return None;
        }
        if let Some(ahead) = self.sections.ahead.as_ref()
            && ahead.generation == self.project_generation
            && pending.revision.checked_add(1) == Some(ahead.revision)
            && let Some(layout) = ahead
                .layout
                .as_ref()
                .filter(|layout| ahead.revision.checked_add(1) == Some(layout.revision))
        {
            return Some(layout);
        }
        pending
            .layout
            .as_ref()
            .filter(|layout| pending.revision.checked_add(1) == Some(layout.revision))
    }

    /// An early copy start is safe only for the exact installed graph
    /// and its own non-preroll metadata. The ordinary receipt still owns the view.
    pub(in crate::editor) fn section_copy_play_source(
        &self,
    ) -> Option<hex_audio::prepared::AppliedAudioSource> {
        use std::sync::atomic::Ordering::Acquire;

        let pending = self.sections.pending.as_ref()?;
        let section = match &pending.command {
            Command::DuplicateArrangementSection(section) => *section,
            command => copied_section_source(command)?,
        };
        let current = self.state.as_ref()?;
        let committed = self.committed_section_copy_view()?;
        if self.document_audio_mode != hex_session::DocumentAudioMode::Resident
            || self.selected_clip.is_some()
            || self.sections.selected_ids() != [section]
            || pending.selection_intent_epoch != self.selection_intent_epoch
            || current.project.tempo_map() != committed.project.tempo_map()
            || current.project.loop_region() != committed.project.loop_region()
            || current
                .project
                .arrangement_section(section)
                .map(|s| s.start())
                != committed
                    .project
                    .arrangement_section(section)
                    .map(|s| s.start())
        {
            return None;
        }
        let audio = self.audio.as_ref()?;
        let status = audio.control.status()?;
        if status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
            || status.stopped_edit_pending()
        {
            return None;
        }
        let observation = self.pending_publication_observation.as_ref()?;
        let crate::EditorPublicationState::ResidentSubmitted {
            audio_revision,
            metadata,
            ..
        } = &observation.state
        else {
            return None;
        };
        let source = status.recording_source()?;
        if observation.output_identity != source.output_identity
            || audio.control.output_identity() != Some(source.output_identity)
            || observation.context != source.context
            || metadata.context != source.context
            || committed.audio_authoring_revision(source.context) != Some(committed.revision)
            || *audio_revision != source.audio_revision
            || metadata.requires_preroll
            || metadata.clip_preview.is_some()
            || metadata.device_preview.is_some()
            || metadata.tuning_preview.is_some()
            || metadata.seek_request.is_some()
            || metadata.stopped_restore
            || metadata.computer_keyboard_request.is_some()
            || metadata.gain_recording_track.is_some()
            || status.recording_source() != Some(source)
        {
            return None;
        }
        Some(source)
    }

    /// Use the selection the successful terminal will install, without
    /// installing that selection or the committed project prematurely.
    pub(in crate::editor) fn section_copy_play_start(&self) -> Option<u64> {
        let pending = self.sections.pending.as_ref()?;
        let Some(source) = copied_section_source(&pending.command) else {
            return Some(self.playback_start_frame());
        };
        let selects_copy = self.device_panel.main_selected == Some(self.project_generation)
            && self.sections.selected == Some(source)
            && pending.selection_intent_epoch.is_some()
            && pending.selection_intent_epoch == self.selection_intent_epoch;
        if !selects_copy {
            return Some(self.playback_start_frame());
        }
        let committed = self.committed_section_copy_view()?;
        let copy = committed.project.arrangement_section(pending.copied?)?;
        let frame = committed
            .project
            .tempo_map()
            .beat_to_frame(copy.start())
            .ok()?;
        Some(self.playback_start_frame_with_section_override(Some(frame)))
    }

    pub(in crate::editor) fn section_install_pending(&self) -> bool {
        self.sections.pending.as_ref().is_some_and(|pending| {
            matches!(
                &pending.command,
                Command::ReorderSectionBefore { .. }
                    | Command::ReorderSectionAfter { .. }
                    | Command::ResizeArrangementSection { .. }
                    | Command::ResizeArrangementSectionEdge { .. }
                    | Command::ResizeArrangementSectionBoundary {
                        marker_only: false,
                        ..
                    }
                    | Command::DeleteArrangementSection(_)
                    | Command::DuplicateArrangementSection(_)
                    | Command::DuplicateArrangementSectionAt { .. }
                    | Command::AppendArrangementSectionCopy { .. }
                    | Command::MoveArrangementSectionBeyondEnd { .. }
                    | Command::ReplaceArrangementSection { .. }
                    | Command::PasteArrangementSection { .. }
            )
        }) || self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.status())
            .is_some_and(|status| status.stopped_edit_pending())
    }
    pub(super) fn select_section_metadata(&mut self, id: Option<SectionId>) {
        self.advance_selection_intent();
        self.duplicate_intent.cancel_chain();
        self.sections.selected = id;
        self.sections.selection = id.into_iter().collect();
        self.sections.anchor = id;
        self.selected_track = None;
        self.arrangement_focus.observe_active_track(None);
        self.selected_clip = None;
        self.audio_detail.detail_context = None;
        self.session_grid.focused_return = None;
        // Use Main's existing view owner so ordinary worker snapshots cannot
        // reselect the first clip and open its inspector under a held preview.
        self.device_panel.main_selected = Some(self.project_generation);
        self.session_grid.selected_cell = None;
        self.session_grid.selected_scene = None;
        self.track_selection = Default::default();
        self.time_selection = None;
        self.time_selection_project_scope = false;
        self.cancel_time_selection_gesture();
    }
    pub(in crate::editor) fn place_section_cursor_at_main_point(&mut self, beat: BeatTime) {
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            return;
        };
        self.sections.cursor = Some(SectionCursor {
            beat,
            generation: self.project_generation,
            revision,
        });
        self.selected_track = None;
        self.arrangement_focus.observe_active_track(None);
        self.selected_clip = None;
        self.audio_detail.detail_context = None;
        self.session_grid.focused_return = None;
        self.device_panel.main_selected = Some(self.project_generation);
        self.session_grid.selected_cell = None;
        self.session_grid.selected_scene = None;
        self.track_selection = Default::default();
    }

    pub(in crate::editor) fn retire_section_pointer_context_for_range(&mut self) {
        self.sections.cursor = None;
        if !self.arrangement_focus.projection.enabled {
            self.sections.selected = None;
            self.sections.selection.clear();
        }
    }

    pub(in crate::editor) fn section_has_focus(&self, window: &Window) -> bool {
        self.mode == EditorMode::Arrangement
            && self.focus.is_focused(window)
            // Q may retain section IDs while a painted range owns keyboard
            // editing. A stationary Main header point keeps its existing section
            // cursor contract; the range body explicitly retires that cursor.
            && self.time_selection.is_none_or(|selection| {
                selection.anchor_frame == selection.end_frame && self.sections.cursor.is_some()
            })
            && self.selected_clip.is_none()
            && self.selected_track.is_none()
            && self.session_grid.focused_return.is_none()
            && self.device_panel.main_selected == Some(self.project_generation)
    }
    pub(super) fn sections_available(&self) -> bool {
        self.sections_idle() && self.section_input_open()
    }
    /// No section edit or global modal/recording/device-gesture barrier is in flight.
    /// Unrelated worker requests (such as clip duplication) do not block sections.
    pub(in crate::editor) fn sections_idle(&self) -> bool {
        !self.edit_blocked_outside_request_queue()
            && self.sections.pending.is_none()
            && !self.section_install_pending()
    }
    fn pending_section_is_copy(&self) -> bool {
        self.sections.pending.as_ref().is_some_and(|pending| {
            matches!(
                &pending.command,
                Command::DuplicateArrangementSection(_) | Command::PasteArrangementSection { .. }
            ) || copied_section_source(&pending.command).is_some()
        })
    }
    /// Pointer section gestures are admitted while idle OR while a single
    /// section copy is in flight and no follow-up is queued yet.
    pub(super) fn section_pointer_available(&self) -> bool {
        let stopped_edit_pending = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.status())
            .is_some_and(|status| status.stopped_edit_pending());
        self.section_input_open()
            && !self.edit_blocked_outside_request_queue()
            && !stopped_edit_pending
            && (self.sections.pending.is_none()
                || (self.sections.queued.is_none()
                    && self.sections.ahead.is_none()
                    && self.pending_section_is_copy()))
    }
    /// No modal owner, text field or other gesture holds editor input.
    pub(super) fn section_input_open(&self) -> bool {
        self.mode == EditorMode::Arrangement
            && self.sections.name.is_none()
            && self.clip_name_edit.is_none()
            && !self.track_name.is_editing()
            && self.context_menu.is_none()
            && !crate::editor::device_panel::picker_is_open(self)
            && !self.persistence.save_dialog_open
            && !self.persistence.export_dialog_open
            && !self.persistence.project_open.busy()
            && !self.settings.changelog.is_open()
            && !self.settings.tuning.open
            && !self.settings.theme.open
            && self.active_fade_drag.is_none()
            && !self.fade_key_held
            && self.clip_paint_gesture.is_none()
            && self.midi_editor.note_gesture.is_none()
            && self.midi_editor.group_gesture.is_none()
            && self.active_clip_drag.is_none()
            && self.active_trim_drag.is_none()
            && self.midi_editor.active_trim_drag.is_none()
            && self.browser.active_audio_drag.is_none()
            && self.device_panel.active_native_drag.is_none()
            && self.time_selection_gesture.is_none()
            && self.time_range_move_gesture.is_none()
            && self.gain_lane.draft.is_none()
            && self.settings.tuning.client.snapshot().is_none()
            && !self.settings.tuning.client.has_pending_requests()
            && !self.settings.tuning.client.is_recovery_pending()
    }

    pub(super) fn section_current(&self, g: &Gesture) -> bool {
        self.section_pointer_available()
            && g.generation == self.project_generation
            && self.section_geometry() == Some(g.layout)
            && g.snap == self.snap_grid
            && self.state.as_ref().is_some_and(|s| {
                s.revision == g.revision && self.visible_viewport(s.timeline_frames) == g.viewport
            })
    }

    pub(super) fn section_launch_action(
        &self,
        id: SectionId,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let generation = self.project_generation;
        let focus = self
            .sections
            .launch_focus
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
            .clone();
        let click_focus = focus.clone();
        hex_widgets::action(theme::text("▶"), theme::action_style())
            .id(("section-launch", id.value()))
            .debug_selector(move || format!("section-launch-{}", id.value()))
            .absolute()
            .left(px(crate::editor::section_launch::ACTION_LEFT))
            .top(px(crate::editor::section_launch::ACTION_TOP))
            .size(px(crate::editor::section_launch::ACTION_SIZE))
            .rounded_full()
            .text_size(px(8.0))
            .line_height(px(12.0))
            .track_focus(&focus)
            .key_context("hex_section_launch")
            .focus(|style| {
                style
                    .border_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Text)))
            })
            .tooltip(|_, cx| {
                cx.new(|_| crate::editor::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: "play from section · hold l to loop".into(),
                })
                .into()
            })
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                if generation == view.project_generation
                    && view.sections_available()
                    && !view.sections.held()
                {
                    click_focus.focus(window);
                    let looped = view.use_section_loop_key();
                    view.launch_section(id, looped);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(
                move |view, _: &crate::editor::section_launch::ActivateSectionLaunch, _, cx| {
                    cx.stop_propagation();
                    if generation == view.project_generation
                        && view.sections_available()
                        && !view.sections.held()
                    {
                        view.launch_section(id, false);
                        cx.notify();
                    }
                },
            ))
    }

    pub(in crate::editor) fn begin_section_pointer(
        &mut self,
        x: f32,
        y: f32,
        double: bool,
        modifiers: gpui::Modifiers,
        cx: &Context<Self>,
    ) -> bool {
        let intent = RectangleSelectionPolicy::Section.intent(modifiers);
        let extend = intent != RectangleSelection::Replace;
        let edit = section_input::EditModifiers::from(modifiers);
        let free = edit.invert_snap;
        let marker_only = edit.marker_only;
        let launch_eligible =
            self.section_autoplay && !double && !extend && !marker_only && !free && !modifiers.alt;
        // This capture also sees clicks on track headers and the focus toggle.
        // Let their focus owners finish before reconciliation decides whether
        // to clear section selection; an armed filter retains its driving IDs.
        if self.section_pointer(x, y, free).is_none() {
            return false;
        }
        if self.sections.naming()
            || self.gain_numeric_editing(cx)
            || self.clip_numeric_editing(cx)
            || self.project_tempo_editing(cx)
        {
            return false;
        }
        let Some(anchor) = self.section_pointer(x, y, free) else {
            return false;
        };
        if !self.section_pointer_available() {
            let hit = self.section_hit(x, y).is_some();
            if hit && !self.sections_idle() {
                self.error = Some(SECTION_BUSY.into());
            }
            return hit;
        }
        let edge = self.section_edge_hit(x);
        let Some([_, top, _, height]) = self.section_layout() else {
            return false;
        };
        let id = match edge {
            Some(SectionEdge::Single(id, _)) | Some(SectionEdge::Shared(id, _)) => Some(id),
            None => self.section_hit(x, y),
        };
        let Some(time_range) = section_input::routes_to_time_range(
            edge.is_some(),
            double && id.is_none(),
            y,
            top,
            height,
            crate::editor::clip_name::CLIP_NAME_HEIGHT,
        ) else {
            return false;
        };
        // Section membership takes precedence over the lower-body range owner,
        // edge resizing and autoplay; Shift never creates a time rectangle here.
        if extend && let Some(id) = id {
            let selection = section_selection::extend(
                self.sections.selected_ids().to_vec(),
                self.sections.selected,
                self.sections.anchor,
                id,
                intent == RectangleSelection::Toggle,
                |anchor, clicked| self.section_range(anchor, clicked),
            );
            self.select_section_metadata(Some(id));
            self.sections.selected = selection.selected;
            self.sections.selection = selection.members;
            self.sections.anchor = Some(selection.anchor);
            self.sections.cursor = None;
            self.cancel_section_pointer();
            self.suppress_cursor_click = true;
            return true;
        }
        if time_range {
            // Do not start a competing capture. Main's existing range receiver
            // sees this same press next (including in empty projects): drags
            // keep canonical all-track scope, while a stationary release places
            // the section cursor.
            self.retire_section_pointer_context_for_range();
            return false;
        }
        if !double && self.section_hit(x, y).is_none() && self.main_time_range_grab(x, y).is_some()
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let id = match edge {
            Some(SectionEdge::Shared(left, _)) => Some(left),
            _ => self.section_hit(x, y),
        };
        self.sections.cursor = Some(SectionCursor {
            beat: anchor,
            generation: self.project_generation,
            revision: state.revision,
        });
        if id.is_none() && !double {
            if !extend || !self.time_selection_project_scope {
                self.select_section_metadata(None);
            }
            // The Main range receiver also owns empty projects. A stationary
            // point still uses this lane cursor for section shortcuts.
            return false;
        }
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(layout) = self.section_layout() else {
            return false;
        };
        let mut original = id
            .and_then(|id| state.project.arrangement_section(id))
            .map(Marker::from);
        let mode = if let Some(SectionEdge::Shared(left, right)) = edge {
            if marker_only {
                Mode::End(left)
            } else {
                Mode::Shared {
                    left,
                    right,
                    marker_only: false,
                }
            }
        } else if let (Some(_marker), Some(id)) = (&original, id) {
            if edge == Some(SectionEdge::Single(id, hex_project::TrimEdge::Left)) {
                if marker_only {
                    Mode::Start(id)
                } else {
                    Mode::ContentStart(id)
                }
            } else if edge == Some(SectionEdge::Single(id, hex_project::TrimEdge::Right)) {
                if marker_only {
                    Mode::End(id)
                } else {
                    Mode::ContentEnd(id)
                }
            } else if marker_only {
                Mode::Move(id)
            } else {
                Mode::Reorder(id)
            }
        } else {
            Mode::Paint
        };
        if matches!(mode, Mode::Paint) {
            let (name, color) = section_appearance::new_marker(&state.project);
            original = Some(Marker {
                id: None,
                name,
                color,
                start: anchor,
                end: anchor,
            });
        }
        let display = Display {
            markers: state
                .project
                .arrangement_sections()
                .iter()
                .map(Marker::from)
                .collect(),
            tempo: state.project.tempo_map().clone(),
            pointer_hover: None,
            held: [None; 2],
            marker_only: mode.marker_only(),
            drop_feedback: None,
        };
        let part = match edge {
            Some(SectionEdge::Shared(..)) => RectanglePart::Seam,
            Some(SectionEdge::Single(_, edge)) => crate::editor::drag::rectangle_part(Some(edge)),
            None => RectanglePart::Body,
        };
        let Some(capture) = RectangleCapture::begin((self.project_generation, id), part, x, true)
        else {
            return false;
        };
        self.sections.gesture = Some(Gesture {
            launch_eligible,
            generation: self.project_generation,
            revision: state.revision,
            viewport,
            snap: self.snap_grid,
            layout,
            anchor,
            last_pointer: (x, y),
            capture,
            original,
            mode,
            display,
            command: None,
            gap_appearance: None,
        });
        self.select_section_metadata(id);
        self.status = if matches!(mode, Mode::Reorder(_)) {
            "section drag · insert or replace song content · option copies · option-command edits markers only · command toggles snap"
        } else if matches!(
            mode,
            Mode::ContentStart(_)
                | Mode::ContentEnd(_)
                | Mode::Shared {
                    marker_only: false,
                    ..
                }
        ) {
            "section resize · crossed time becomes empty · option-command preserves music"
        } else {
            "section marker edit · musical content stays in place"
        }
        .into();
        self.update_section_pointer(x, y, modifiers);
        true
    }
    pub(in crate::editor) fn update_section_pointer(
        &mut self,
        x: f32,
        y: f32,
        modifiers: gpui::Modifiers,
    ) -> bool {
        let edit = section_input::EditModifiers::from(modifiers);
        let free = edit.invert_snap;
        let Some(mut g) = self.sections.gesture.take() else {
            return false;
        };
        if !self.section_current(&g) || self.section_layout().is_none() {
            return true;
        }
        if edit.marker_only {
            // One-way marker intent prevents releasing Option-Command from unexpectedly
            // turning a preview into a destructive song-content permutation.
            g.mode = match g.mode {
                Mode::Reorder(id) => Mode::Move(id),
                Mode::ContentStart(id) => Mode::Start(id),
                Mode::ContentEnd(id) => Mode::End(id),
                Mode::Shared { left, .. } => Mode::End(left),
                mode => mode,
            };
            g.launch_eligible = false;
        }
        if edit.copy_content || edit.invert_snap {
            g.launch_eligible = false;
        }
        if g.capture.update(x).is_none() {
            self.sections.gesture = Some(g);
            return true;
        }
        let beat = if let Some(beat) = self.section_pointer(x, y, free) {
            g.last_pointer = (x, y);
            beat
        } else if let Some(beat) = self.section_pointer(g.last_pointer.0, g.last_pointer.1, free) {
            // Captured input may finish over the browser or another panel. Keep
            // the last admitted geometry, but still apply live modifier intent.
            beat
        } else {
            self.sections.gesture = Some(g);
            return true;
        };
        g.command = None;
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        g.display = Display {
            markers: state
                .project
                .arrangement_sections()
                .iter()
                .map(Marker::from)
                .collect(),
            tempo: state.project.tempo_map().clone(),
            pointer_hover: None,
            held: [None; 2],
            marker_only: g.mode.marker_only(),
            drop_feedback: None,
        };
        match g.mode {
            Mode::Reorder(id) => {
                // Interior destinations retain stable edges. Beyond the end,
                // the absolute pointer uses the grid. A click never moves music.
                let raw_beat = self
                    .section_pointer(g.last_pointer.0, g.last_pointer.1, true)
                    .unwrap_or(beat);
                if g.capture.displaced(4.0) {
                    match crate::section_hit::drop_edge_frames(g.viewport.length, g.layout[2])
                        .and_then(|edge_frames| {
                            section_reorder::drop_preview(
                                &state.project,
                                id,
                                raw_beat,
                                beat,
                                edit.copy_content,
                                edge_frames,
                                &mut g.gap_appearance,
                            )
                        }) {
                        Ok(Some(preview)) => {
                            g.display.markers =
                                preview.sections().iter().map(Marker::from).collect();
                            g.display.tempo = preview.tempo_map().clone();
                            g.display.drop_feedback =
                                preview.feedback().map(|feedback| DropDisplay {
                                    feedback,
                                    tempo: state.project.tempo_map().clone(),
                                });
                            g.command = Some(preview.command);
                        }
                        Ok(None) => {}
                        Err(error) => self.error = Some(error.to_string().into()),
                    }
                }
            }
            Mode::Move(id) => {
                if g.capture.displaced(4.0)
                    && let Some(marker) = &g.original
                {
                    let delta = i128::from(beat.raw()) - i128::from(g.anchor.raw());
                    let Ok(start) = u64::try_from(i128::from(marker.start.raw()) + delta) else {
                        self.sections.gesture = Some(g);
                        return true;
                    };
                    let start = BeatTime::from_raw(start);
                    if start != marker.start {
                        let command = Command::MoveArrangementSectionMarker { section: id, start };
                        match state.project.preview_arrangement_section_edit(&command) {
                            Ok(preview) => {
                                g.display.markers =
                                    preview.sections().iter().map(Marker::from).collect();
                                g.display.tempo = preview.tempo_map().clone();
                                g.command = Some(command);
                            }
                            Err(error) => self.error = Some(error.to_string().into()),
                        }
                    }
                }
            }
            Mode::Shared {
                left,
                right,
                marker_only,
            } => {
                if g.capture.displaced(4.0) {
                    let edit = section_resize::Edit::Shared {
                        left,
                        right,
                        boundary: beat,
                        marker_only,
                    };
                    match section_resize::preview(&state.project, edit) {
                        Ok(preview) => {
                            g.display.markers =
                                preview.sections().iter().map(Marker::from).collect();
                            g.display.tempo = preview.tempo_map().clone();
                            g.command = Some(preview.command);
                        }
                        Err(error) => self.error = Some(error.to_string().into()),
                    }
                }
            }
            Mode::ContentStart(id) | Mode::ContentEnd(id) => {
                if g.capture.displaced(4.0)
                    && let Some(marker) = &g.original
                {
                    let edge = if matches!(g.mode, Mode::ContentStart(_)) {
                        hex_project::TrimEdge::Left
                    } else {
                        hex_project::TrimEdge::Right
                    };
                    if beat
                        != match edge {
                            hex_project::TrimEdge::Left => marker.start,
                            hex_project::TrimEdge::Right => marker.end,
                        }
                    {
                        let edit = section_resize::Edit::Content {
                            section: id,
                            edge,
                            boundary: beat,
                        };
                        match section_resize::preview(&state.project, edit) {
                            Ok(preview) => {
                                g.display.markers =
                                    preview.sections().iter().map(Marker::from).collect();
                                g.display.tempo = preview.tempo_map().clone();
                                g.command = Some(preview.command);
                            }
                            Err(error) => self.error = Some(error.to_string().into()),
                        }
                    }
                }
            }
            Mode::Start(id) | Mode::End(id) => {
                if g.capture.displaced(4.0)
                    && let Some(marker) = &g.original
                {
                    let edge = if matches!(g.mode, Mode::Start(_)) {
                        hex_project::TrimEdge::Left
                    } else {
                        hex_project::TrimEdge::Right
                    };
                    let original_boundary = if edge == hex_project::TrimEdge::Left {
                        marker.start
                    } else {
                        marker.end
                    };
                    if beat != original_boundary {
                        let edit = section_resize::Edit::Marker {
                            section: id,
                            edge,
                            boundary: beat,
                        };
                        match section_resize::preview(&state.project, edit) {
                            Ok(preview) => {
                                g.display.markers =
                                    preview.sections().iter().map(Marker::from).collect();
                                g.command = Some(preview.command);
                            }
                            Err(error) => self.error = Some(error.to_string().into()),
                        }
                    }
                }
            }
            Mode::Paint => {
                let minimum = self
                    .snap_grid
                    .with_platform_modifier(free)
                    .adjacent_beat(&state.project, g.anchor, g.viewport, g.layout[2], true)
                    .or_else(|| {
                        g.anchor
                            .raw()
                            .checked_add(BeatTime::ONE.raw())
                            .map(BeatTime::from_raw)
                    });
                let Some(minimum) = minimum else {
                    return true;
                };
                let start = beat.min(g.anchor);
                let end = if beat == g.anchor {
                    minimum
                } else {
                    beat.max(g.anchor)
                };
                if start < end
                    && state
                        .project
                        .arrangement_sections()
                        .iter()
                        .all(|s| end <= s.start() || start >= s.end())
                {
                    let Some(appearance) = g.original.as_ref() else {
                        return true;
                    };
                    g.display.markers.push(Marker {
                        id: None,
                        name: appearance.name.clone(),
                        color: appearance.color,
                        start,
                        end,
                    });
                    g.command = Some(Command::AddArrangementSection {
                        name: appearance.name.clone(),
                        color: appearance.color,
                        start,
                        end,
                    });
                }
            }
        }
        g.display.pointer_hover = g
            .display
            .pointer_hover_at(g.viewport, g.layout, g.last_pointer);
        g.display.held = g.mode.held();
        self.sections.gesture = Some(g);
        true
    }
    pub(in crate::editor) fn finish_section_pointer(
        &mut self,
        x: f32,
        y: f32,
        modifiers: gpui::Modifiers,
    ) -> bool {
        if self.sections.gesture.is_none() {
            return false;
        }
        self.update_section_pointer(x, y, modifiers);
        let Some(g) = self.sections.gesture.take() else {
            return true;
        };
        let Some(_released) = g.capture.finish(x) else {
            return true;
        };
        self.suppress_cursor_click = true;
        if self.section_current(&g) {
            if let Some(command) = g.command {
                if self.sections.pending.is_some() {
                    self.sections.queued = Some(Queued {
                        generation: g.generation,
                        command,
                        display: Some(g.display),
                    });
                    self.status = "section edit queued · applies after the current edit".into();
                } else {
                    self.submit_section(command, g.display, g.generation, g.revision);
                }
            } else if g.launch_eligible
                && !g.capture.displaced(4.0)
                && let Mode::Reorder(id) = g.mode
            {
                self.launch_arrangement_section(id);
            }
        }
        true
    }
    pub(in crate::editor) fn cancel_section_pointer(&mut self) -> bool {
        self.sections.gesture.take().is_some()
    }
    pub(super) fn submit_section(
        &mut self,
        command: Command,
        display: Display,
        generation: u64,
        revision: u64,
    ) {
        let layout = self
            .state
            .as_ref()
            .filter(|state| state.revision == revision)
            .and_then(|state| crate::editor::section_layout_preview::build(state, &command));
        let deferred = (self.worker_requests_pending != 0 || !self.pending_requests.is_empty())
            && self.pending_requests.len() < super::super::UI_REQUEST_CAPACITY;
        self.sections.pending = Some(Pending {
            generation,
            revision,
            command: command.clone(),
            display,
            layout,
            copied: None,
            cut_clipboard: None,
            cut_cursor: None,
            receipt: Receipt::new(generation),
            selection_intent_epoch: self.selection_intent_epoch,
            deferred_submission: deferred,
        });
        if !deferred
            && !self.enqueue(Request::EditArrangementSection {
                project_generation: generation,
                revision,
                command,
            })
        {
            self.sections.pending = None;
        }
    }
    pub(in crate::editor) fn pending_section_matches(&self, request: &Request) -> bool {
        let Request::EditArrangementSection {
            project_generation,
            revision,
            command,
        } = request
        else {
            return false;
        };
        self.sections
            .pending
            .iter()
            .chain(self.sections.ahead.iter())
            .any(|pending| {
                !pending.deferred_submission
                    && pending.receipt.is_queued()
                    && *project_generation == pending.generation
                    && *revision == pending.revision
                    && same_command(command, &pending.command)
            })
    }
    pub(in crate::editor) fn section_submitted(&mut self) {
        if let Some(p) = self
            .sections
            .ahead
            .as_mut()
            .filter(|p| !p.deferred_submission && p.receipt.is_queued())
            .or_else(|| {
                self.sections
                    .pending
                    .as_mut()
                    .filter(|p| !p.deferred_submission && p.receipt.is_queued())
            })
        {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(in crate::editor) fn retire_pending_section(&mut self) -> bool {
        let pending = self.sections.pending.take().is_some();
        self.sections.ahead.take().is_some() || pending
    }
    pub(in crate::editor) fn refuse_section(&mut self, request: &Request) -> bool {
        if let Some(ahead) = self.sections.ahead.as_ref()
            && let Request::EditArrangementSection {
                project_generation,
                revision,
                command,
            } = request
            && ahead.receipt.is_queued()
            && *project_generation == ahead.generation
            && *revision == ahead.revision
            && same_command(command, &ahead.command)
        {
            return self.sections.ahead.take().is_some();
        }
        self.retire_pending_section()
    }
    pub(in crate::editor) fn section_request_complete(&mut self, event: &Event) {
        if self.sections.ahead.as_mut().is_some_and(|ahead| {
            ahead.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) == Disposition::Invalidated
        }) {
            self.sections.ahead = None;
        }
        if self.sections.pending.as_mut().is_some_and(|p| {
            if p.deferred_submission {
                return event.project_replaced || p.generation != self.project_generation;
            }
            p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        }) {
            if event.error.is_none()
                && !event.project_replaced
                && self.device_panel.main_selected == Some(self.project_generation)
                && let Some(Pending {
                    command: Command::MergeArrangementSections(selected),
                    ..
                }) = self.sections.pending.as_ref()
                && self
                    .sections
                    .selected
                    .is_some_and(|id| selected.contains(&id))
                && let Some(kept) = event
                    .state
                    .project
                    .arrangement_sections()
                    .iter()
                    .find(|section| selected.contains(&section.id()))
            {
                self.sections.selected = Some(kept.id());
                self.sections.selection = vec![kept.id()];
            }
            if event.error.is_none()
                && !event.project_replaced
                && let Some(pending) = self.sections.pending.as_mut()
                && let Some(captured) = pending.cut_clipboard.take()
            {
                self.sections.clipboard = Some(captured);
                if let Some(beat) = pending.cut_cursor.take() {
                    self.sections.cursor = Some(SectionCursor {
                        beat,
                        generation: self.project_generation,
                        revision: event.state.revision,
                    });
                }
                self.status = "section cut".into();
            }
            let pasted_match = self.sections.pending.as_ref().is_some_and(|pending| {
                matches!(pending.command, Command::PasteArrangementSection { .. })
                    && pending
                        .selection_intent_epoch
                        .is_some_and(|epoch| Some(epoch) == self.selection_intent_epoch)
            });
            if event.error.is_none()
                && !event.project_replaced
                && pasted_match
                && self.device_panel.main_selected == Some(self.project_generation)
                && let Some(hex_project::Created::ArrangementSection(id)) = event.created.as_ref()
            {
                self.sections.selected = Some(*id);
                self.sections.selection = vec![*id];
                if let Some(pasted) = event.state.project.arrangement_section(*id) {
                    self.sections.cursor = Some(SectionCursor {
                        beat: pasted.end(),
                        generation: self.project_generation,
                        revision: event.state.revision,
                    });
                }
                self.status = "section pasted".into();
            }
            let copy_source = self.sections.pending.as_ref().and_then(|pending| {
                if pending
                    .selection_intent_epoch
                    .is_none_or(|epoch| Some(epoch) != self.selection_intent_epoch)
                {
                    return None;
                }
                copied_section_source(&pending.command)
            });
            let select_copy = event.error.is_none()
                && !event.project_replaced
                && self.device_panel.main_selected == Some(self.project_generation)
                && copy_source.is_some()
                && copy_source == self.sections.selected;
            if let Some(hex_project::Created::ArrangementSection(id)) =
                event.created.as_ref().filter(|_| select_copy)
            {
                self.sections.selected = Some(*id);
                self.sections.selection = vec![*id];
            }
            if (event.error.is_some() || event.project_replaced)
                && self.sections.queued.take().is_some()
            {
                // It was chosen against the refused preview; never apply it
                // to different content. The terminal error stays visible.
                self.status = "queued section edit dropped".into();
            }
            self.sections.pending = self.sections.ahead.take();
        }
        if event.project_replaced {
            self.sections = Sections::default();
        } else if let Some(g) = self.sections.gesture.as_mut()
            && g.revision != event.state.revision
        {
            let sections_unchanged = self.state.as_ref().is_some_and(|old| {
                old.project.arrangement_sections() == event.state.project.arrangement_sections()
                    && old.project.tempo_map() == event.state.project.tempo_map()
            });
            let target_unchanged = g.original.as_ref().and_then(|m| m.id).is_some_and(|id| {
                let Some(orig) = g.original.as_ref() else {
                    return false;
                };
                event
                    .state
                    .project
                    .arrangement_section(id)
                    .is_some_and(|s| s.start() == orig.start && s.end() == orig.end)
            });
            if sections_unchanged || target_unchanged {
                g.revision = event.state.revision;
            } else {
                self.cancel_section_pointer();
            }
        }
    }
    /// Submit any deferred section release (held while an unrelated worker edit
    /// drained) or queued follow-up once its predecessor is acknowledged.
    pub(in crate::editor) fn submit_queued_section(&mut self) -> bool {
        if self
            .sections
            .pending
            .as_ref()
            .is_some_and(|p| p.deferred_submission)
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
            && !self.edit_blocked_outside_request_queue()
        {
            let Some(state) = self.state.as_ref() else {
                return false;
            };
            let revision = state.revision;
            let layout = self
                .sections
                .pending
                .as_ref()
                .and_then(|p| crate::editor::section_layout_preview::build(state, &p.command));
            if let Some(p) = self.sections.pending.as_mut() {
                if p.generation != self.project_generation {
                    self.sections.pending = None;
                    return true;
                }
                p.revision = revision;
                p.layout = layout;
                p.deferred_submission = false;
                let generation = p.generation;
                let command = p.command.clone();
                if !self.enqueue(Request::EditArrangementSection {
                    project_generation: generation,
                    revision,
                    command,
                }) {
                    self.sections.pending = None;
                }
                return true;
            }
        }
        if self
            .sections
            .queued
            .as_ref()
            .is_some_and(|q| q.generation != self.project_generation)
        {
            self.sections.queued = None;
            return true;
        }
        if self.sections.queued.is_none()
            || !self.sections_available()
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
        {
            return false;
        }
        let Some(Queued {
            command, display, ..
        }) = self.sections.queued.take()
        else {
            return false;
        };
        self.status = "applying queued section edit".into();
        if let Some(display) = display
            && let Some(revision) = self.state.as_ref().map(|s| s.revision)
        {
            self.submit_section(command, display, self.project_generation, revision);
        } else {
            self.section_metadata(command);
        }
        true
    }
    /// Undo/Redo order behind the in-flight edit; a queued section follow-up
    /// was aimed at the state they replace, so it is dropped visibly.
    pub(in crate::editor) fn cancel_queued_section(&mut self) {
        if self.sections.queued.take().is_some() {
            self.status = "queued section edit cancelled by undo/redo".into();
        }
    }
    pub(in crate::editor) fn reconcile_sections(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(edit) = &self.sections.name {
            crate::editor::browser_input::restyle(&edit.input, cx);
        }
        let removed_focus = self
            .sections
            .launch_focus
            .borrow()
            .iter()
            .any(|(id, focus)| {
                focus.is_focused(window)
                    && self
                        .state
                        .as_ref()
                        .is_none_or(|state| state.project.arrangement_section(*id).is_none())
            });
        if removed_focus {
            self.focus.focus(window);
        }
        self.sections.launch_focus.borrow_mut().retain(|id, _| {
            self.state
                .as_ref()
                .is_some_and(|s| s.project.arrangement_section(*id).is_some())
        });
        self.sections.selection.retain(|id| {
            self.state
                .as_ref()
                .is_some_and(|s| s.project.arrangement_section(*id).is_some())
        });

        if self
            .sections
            .gesture
            .as_ref()
            .is_some_and(|g| !self.section_current(g) || self.section_layout().is_none())
        {
            self.cancel_section_pointer();
        }
        if (self.sections.selected.is_some() || self.sections.cursor.is_some())
            && (self.mode != EditorMode::Arrangement
                || self.selected_clip.is_some()
                || self.selected_track.is_some()
                || self.session_grid.focused_return.is_some()
                || self.device_panel.main_selected != Some(self.project_generation)
                || (!self.focus.is_focused(window)
                    && !self.section_focus_toggle_focused(window, cx)
                    && !self
                        .sections
                        .launch_focus
                        .borrow()
                        .values()
                        .any(|focus| focus.is_focused(window))
                    && !self.sections.naming()
                    && self.context_menu.is_none()))
        {
            if !self.arrangement_focus.projection.enabled {
                self.sections.selected = None;
                self.sections.selection.clear();
            }
            self.sections.cursor = None;
        }
        if self.sections.selected.is_some_and(|id| {
            self.state
                .as_ref()
                .is_none_or(|s| s.project.arrangement_section(id).is_none())
        }) {
            self.sections.selected = None;
            self.sections.selection.clear();
        }
        if self
            .sections
            .name
            .as_ref()
            .is_some_and(|n| !self.section_stamp_current(n.generation, n.revision, n.id))
        {
            let focused = self
                .sections
                .name
                .as_ref()
                .is_some_and(|n| n.input.read(cx).focus_handle().is_focused(window));
            self.sections.name = None;
            if focused {
                self.focus.focus(window);
            }
        }
    }
    pub(super) fn section_stamp_current(
        &self,
        generation: u64,
        revision: u64,
        id: SectionId,
    ) -> bool {
        generation == self.project_generation
            && self.state.as_ref().is_some_and(|s| {
                s.revision == revision && s.project.arrangement_section(id).is_some()
            })
    }
    pub(in crate::editor) fn section_name_begin(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.section_has_focus(window) {
            return false;
        }
        let Some(id) = self.sections.selected else {
            return false;
        };
        if !self.sections_available() {
            if !self.sections_idle() {
                self.error = Some(SECTION_BUSY.into());
            }
            return true;
        }
        self.cancel_section_pointer();
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        let Some(section) = state.project.arrangement_section(id) else {
            return true;
        };
        let revision = state.revision;
        let input = cx.new(|cx| crate::editor::browser_input::name(section.name().into(), cx));
        let focus = input.read(cx).focus_handle().clone();
        let blur = cx.on_blur(&focus, window, |view, _, cx| {
            view.section_name_commit(cx);
        });
        self.sections.name = Some(Name {
            id,
            generation: self.project_generation,
            revision,
            input,
            _blur: blur,
        });
        focus.focus(window);
        cx.notify();
        true
    }
    pub(super) fn section_name_commit(&mut self, cx: &mut Context<Self>) {
        let Some(n) = self.sections.name.take() else {
            return;
        };
        let name = n.input.read(cx).text().to_owned();
        if self.section_stamp_current(n.generation, n.revision, n.id) {
            self.section_metadata(Command::RenameArrangementSection {
                section: n.id,
                name,
            });
        }
        cx.notify();
    }
    pub(in crate::editor) fn finalize_section_name_for_open(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(name) = self.sections.name.as_ref() else {
            return true;
        };
        let draft = name.input.read(cx).text();
        if self.edit_pending()
            || self.persistence.project_open.busy()
            || self.persistence.pending_open_path.is_some()
            || !self.section_stamp_current(name.generation, name.revision, name.id)
            || draft.trim().is_empty()
            || draft.len() > hex_project::MAX_NAME_BYTES
            || draft.chars().any(char::is_control)
        {
            self.error = Some("finish or cancel the section name before opening a project".into());
            return false;
        }
        self.section_name_finish(true, window, cx);
        true
    }
    pub(in crate::editor) fn section_name_finish(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.sections.name.is_none() {
            return false;
        }
        if commit {
            self.section_name_commit(cx);
        } else {
            self.sections.name = None;
        }
        self.focus.focus(window);
        cx.notify();
        true
    }
    pub(in crate::editor) fn delete_section(&mut self, window: &Window) -> bool {
        self.delete_section_kind(false, window)
    }
    pub(in crate::editor) fn delete_section_kind(
        &mut self,
        marker_only: bool,
        window: &Window,
    ) -> bool {
        let Some(id) = self.sections.selected else {
            return false;
        };
        if !self.section_has_focus(window) {
            return false;
        }
        if self.section_input_open() {
            self.cancel_section_pointer();
            if self.sections.selection.len() > 1 {
                self.error = Some(
                    "select one section to delete, or press g to merge the selected span".into(),
                );
            } else {
                self.section_metadata(if marker_only {
                    Command::RemoveArrangementSection(id)
                } else {
                    Command::DeleteArrangementSection(id)
                });
            }
        }
        true
    }
    pub(in crate::editor) fn duplicate_section(&mut self, window: &Window) -> bool {
        if !self.section_has_focus(window) {
            return false;
        }
        let Some(id) = self.sections.selected else {
            return false;
        };
        if self.sections.selection.len() > 1 {
            self.error = Some(
                "select one section to duplicate, or press g to merge the selected span".into(),
            );
        } else {
            self.section_metadata(Command::DuplicateArrangementSection(id));
        }
        true
    }
    /// Return true when the lane owns E, including refused/stale points.
    /// Never fall through to clip splitting after a section-context refusal.
    pub(in crate::editor) fn split_section_at_cursor(&mut self, window: &Window) -> bool {
        if self.sections.naming() {
            return true;
        }
        if !self.section_has_focus(window) || self.normalized_time_selection().is_some() {
            return false;
        }
        let Some(cursor) = self.sections.cursor else {
            return false;
        };
        if !self.sections_available() || self.sections.held() {
            if !self.sections_idle() {
                // The cursor is revision-bound, so a split cannot be queued.
                self.error = Some(SECTION_BUSY.into());
            }
            return true;
        }
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        if cursor.generation != self.project_generation || cursor.revision != state.revision {
            self.error = Some("section cursor changed; click the lane again".into());
            return true;
        }
        let Some(section) = state
            .project
            .arrangement_sections()
            .iter()
            .find(|s| s.start() < cursor.beat && cursor.beat < s.end())
        else {
            self.error = Some("edit cursor must be strictly inside a section to split".into());
            return true;
        };
        self.section_metadata(Command::SplitArrangementSection {
            section: section.id(),
            at: cursor.beat,
        });
        true
    }
    pub(in crate::editor) fn merge_sections(&mut self, window: &Window) -> bool {
        if !self.section_has_focus(window) || self.sections.selected.is_none() {
            return false;
        }
        self.merge_selected_sections();
        true
    }
    /// Diagnostic projection of the inputs that route `g` (counts and flags only).
    pub(in crate::editor) fn group_selection_route(&self, window: &Window) -> String {
        format!(
            "mode={:?} focused={} section_focus={} event_focus={} midi_focus={} keyboard_g={} \
             sections={} section_primary={} clips={} clip={} track={} time_range={} \
             section_cursor={} main_selected={} return={} edit_pending={}",
            self.mode,
            self.focus.is_focused(window),
            self.section_has_focus(window),
            self.audio_detail.focus.is_focused(window),
            self.midi_editor.focus.is_focused(window),
            self.computer_keyboard.accepts_key("g"),
            self.sections.selection.len(),
            self.sections.selected.is_some(),
            self.selected_clip_ids().len(),
            self.selected_clip.is_some(),
            self.selected_track.is_some(),
            self.time_selection
                .is_some_and(|range| range.anchor_frame != range.end_frame),
            self.sections.cursor.is_some(),
            self.device_panel.main_selected == Some(self.project_generation),
            self.session_grid.focused_return.is_some(),
            self.edit_pending(),
        )
    }
    /// `g` with a section span as the only grouping selection merges it, even
    /// when the stricter section-focus test fails (Pulse #3812). False when
    /// there is no such span, so the caller explains the empty selection.
    pub(in crate::editor) fn merge_section_span_selection(&mut self) -> bool {
        if self.sections.selection.len() < 2 || self.selected_clip.is_some() {
            return false;
        }
        self.merge_selected_sections();
        true
    }
    /// `g` on a Main gap creates a marker without cutting the tracks below it.
    /// Occupied ranges keep the existing project-scope grouping route.
    pub(in crate::editor) fn create_section_in_selected_gap(&mut self) -> bool {
        let (Some(selection), Some(state)) = (self.time_selection, self.state.as_ref()) else {
            return false;
        };
        let start = selection.anchor_frame.min(selection.end_frame);
        let end = selection.anchor_frame.max(selection.end_frame);
        let tempo = state.project.tempo_map();
        let (Ok(start), Ok(end)) = (tempo.frame_to_beat(start), tempo.frame_to_beat(end)) else {
            self.error = Some("section range is outside the timeline".into());
            return true;
        };
        if start >= end {
            self.error = Some("select a longer time range to create a section".into());
            return true;
        }
        if state
            .project
            .arrangement_sections()
            .iter()
            .any(|section| section.start() < end && section.end() > start)
        {
            return false;
        }
        let (name, color) = section_appearance::new_marker(&state.project);
        self.section_metadata(Command::AddArrangementSection {
            name,
            color,
            start,
            end,
        });
        true
    }
    /// Merge the current section selection, or explain why it cannot merge.
    pub(in crate::editor) fn merge_selected_sections(&mut self) {
        if self.sections.selection.len() < 2 {
            self.error = Some("shift-click another section to merge a span".into());
        } else if !self.section_input_open() {
            self.error = Some("applying edit · wait before merging sections".into());
        } else {
            self.section_metadata(Command::MergeArrangementSections(
                self.sections.selection.clone(),
            ));
        }
    }
    pub(super) fn section_metadata(&mut self, command: Command) {
        if !self.section_input_open() {
            return;
        }
        if !self.sections_idle() {
            self.sections.queued = Some(Queued {
                generation: self.project_generation,
                command,
                display: None,
            });
            self.status = "section edit queued · applies after the current edit".into();
            self.start_ahead_section_duplicate();
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let mut display = Display {
            markers: state
                .project
                .arrangement_sections()
                .iter()
                .map(Marker::from)
                .collect(),
            tempo: state.project.tempo_map().clone(),
            pointer_hover: None,
            held: [None; 2],
            marker_only: false,
            drop_feedback: None,
        };
        match &command {
            Command::AddArrangementSection {
                name,
                color,
                start,
                end,
            } => {
                let index = display
                    .markers
                    .partition_point(|marker| marker.start < *start);
                display.markers.insert(
                    index,
                    Marker {
                        id: None,
                        name: name.clone(),
                        color: *color,
                        start: *start,
                        end: *end,
                    },
                );
            }
            Command::RenameArrangementSection { section, name } => {
                if let Some(m) = display.markers.iter_mut().find(|m| m.id == Some(*section)) {
                    m.name = name.clone();
                }
            }
            Command::SetArrangementSectionColor { section, color } => {
                if let Some(m) = display.markers.iter_mut().find(|m| m.id == Some(*section)) {
                    m.color = *color;
                }
            }
            Command::RemoveArrangementSection(id) => display.markers.retain(|m| m.id != Some(*id)),
            Command::ResizeArrangementSection { .. }
            | Command::ResizeArrangementSectionEdge { .. }
            | Command::ResizeArrangementSectionBoundary { .. }
            | Command::DeleteArrangementSection(_)
            | Command::DuplicateArrangementSection(_)
            | Command::DuplicateArrangementSectionAt { .. }
            | Command::AppendArrangementSectionCopy { .. }
            | Command::MoveArrangementSectionBeyondEnd { .. }
            | Command::ReplaceArrangementSection { .. }
            | Command::PasteArrangementSection { .. }
            | Command::SplitArrangementSection { .. }
            | Command::MergeArrangementSections(_) => {
                match state.project.preview_arrangement_section_edit(&command) {
                    Ok(preview) => {
                        display.markers = preview.sections().iter().map(Marker::from).collect();
                        display.tempo = preview.tempo_map().clone();
                    }
                    Err(error) => {
                        self.error = Some(error.to_string().into());
                        return;
                    }
                }
            }
            _ => return,
        }
        self.submit_section(command, display, self.project_generation, state.revision);
    }

    /// Admit one queued duplicate of the same source only against the exact
    /// committed predecessor. Both FIFO receipts retain their terminal custody.
    pub(super) fn start_ahead_section_duplicate(&mut self) {
        if self.sections.ahead.is_some()
            || self.worker_requests_pending != 1
            || !self.pending_requests.is_empty()
            || !self.device_panel.gesture_barriers.is_empty()
            || !self.section_input_open()
        {
            return;
        }
        let Some(primary) = self.sections.pending.as_ref() else {
            return;
        };
        let section = match &primary.command {
            Command::DuplicateArrangementSection(section) => *section,
            command => match copied_section_source(command) {
                Some(section) => section,
                None => return,
            },
        };
        let Some(queued) = self.sections.queued.as_ref() else {
            return;
        };
        if queued.generation != self.project_generation
            || !matches!(queued.command, Command::DuplicateArrangementSection(id) if id == section)
            || primary.generation != self.project_generation
            || primary.selection_intent_epoch != self.selection_intent_epoch
            || self.sections.selected_ids() != [section]
        {
            return;
        }
        let Some(view) = primary.layout.as_ref().filter(|view| {
            primary.revision.checked_add(1) == Some(view.revision)
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == primary.revision)
        }) else {
            return;
        };
        if view.project.arrangement_section(section).is_none() {
            return;
        }
        let revision = view.revision;
        let command = queued.command.clone();
        self.sections.ahead = Some(Pending {
            generation: self.project_generation,
            revision,
            command: command.clone(),
            display: primary.display.clone(),
            layout: None,
            copied: None,
            cut_clipboard: None,
            cut_cursor: None,
            receipt: Receipt::new(self.project_generation),
            selection_intent_epoch: primary.selection_intent_epoch,
            deferred_submission: false,
        });
        self.sections.queued = None;
        if !self.enqueue(Request::EditArrangementSection {
            project_generation: self.project_generation,
            revision,
            command,
        }) {
            self.sections.ahead = None;
        } else {
            self.status = "applying consecutive section copy".into();
        }
    }
    pub(in crate::editor) fn section_menu(
        &mut self,
        id: SectionId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sections_available() {
            if !self.sections_idle() {
                self.error = Some(SECTION_BUSY.into());
            }
            return;
        }
        self.cancel_section_pointer();
        let selection = self.sections.selection.clone();
        self.select_section_metadata(Some(id));
        if selection.contains(&id) {
            self.sections.selection = selection;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(section) = state.project.arrangement_section(id) else {
            return;
        };
        let generation = self.project_generation;
        let revision = state.revision;
        let color = section.color();
        let make = |kind| {
            crate::editor::context_menu::EditorContextAction::Section(MenuAction {
                generation,
                revision,
                section: Some(id),
                kind,
            })
        };
        let mut items = vec![
            MenuItem {
                label: "duplicate section · cmd-d",
                swatch: None,
                checked: false,
                action: make(MenuKind::Duplicate),
            },
            MenuItem {
                label: "cut section · cmd-x",
                swatch: None,
                checked: false,
                action: make(MenuKind::Cut),
            },
            MenuItem {
                label: "copy section · cmd-c",
                swatch: None,
                checked: false,
                action: make(MenuKind::Copy),
            },
            MenuItem {
                label: "paste section at cursor · cmd-v",
                swatch: None,
                checked: false,
                action: make(MenuKind::Paste),
            },
            MenuItem {
                label: "delete section and close time",
                swatch: None,
                checked: false,
                action: make(MenuKind::Delete),
            },
            MenuItem {
                label: "rename section · cmd-r",
                swatch: None,
                checked: false,
                action: make(MenuKind::Rename),
            },
            MenuItem {
                label: "remove marker only · option-command-delete",
                swatch: None,
                checked: false,
                action: make(MenuKind::Remove),
            },
        ];
        if self.sections.selection.len() >= 2 {
            items.insert(
                0,
                MenuItem {
                    label: "merge selected span · g",
                    swatch: None,
                    checked: false,
                    action: make(MenuKind::Merge),
                },
            );
        }
        items.extend(
            ElementColor::TRACK_CHOICES
                .into_iter()
                .map(|choice| MenuItem {
                    label: choice.key(),
                    swatch: Some(theme::element_colors(choice).accent),
                    checked: choice == color,
                    action: make(MenuKind::Color(choice)),
                }),
        );
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            icon_grid: None,
            position,
            selected: 0,
            focus,
            previous_focus,
            items,
        });
        cx.notify();
    }
    pub(in crate::editor) fn section_lane_menu(
        &mut self,
        x: f32,
        y: f32,
        position: gpui::Point<gpui::Pixels>,
        modifiers: gpui::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sections_available() {
            if !self.sections_idle() {
                self.error = Some(SECTION_BUSY.into());
            }
            return;
        }
        let free = section_input::EditModifiers::from(modifiers).invert_snap;
        let Some(beat) = self.section_pointer(x, y, free) else {
            return;
        };
        self.cancel_section_pointer();
        self.select_section_metadata(None);
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let generation = self.project_generation;
        let revision = state.revision;
        self.sections.cursor = Some(SectionCursor {
            beat,
            generation,
            revision,
        });
        let items = vec![MenuItem {
            label: "paste section at cursor · cmd-v",
            swatch: None,
            checked: false,
            action: crate::editor::context_menu::EditorContextAction::Section(MenuAction {
                generation,
                revision,
                section: None,
                kind: MenuKind::Paste,
            }),
        }];
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            icon_grid: None,
            position,
            selected: 0,
            focus,
            previous_focus,
            items,
        });
        cx.notify();
    }
    pub(in crate::editor) fn section_menu_stale(&self, a: &MenuAction) -> bool {
        match a.section {
            Some(id) => !self.section_stamp_current(a.generation, a.revision, id),
            None => {
                a.generation != self.project_generation
                    || self
                        .state
                        .as_ref()
                        .is_none_or(|state| state.revision != a.revision)
            }
        }
    }
    pub(in crate::editor) fn section_menu_action(
        &mut self,
        a: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.section_menu_stale(&a) {
            return;
        }
        if let Some(id) = a.section {
            self.sections.selected = Some(id);
        }
        match a.kind {
            MenuKind::Rename => {
                self.focus.focus(window);
                self.section_name_begin(window, cx);
            }
            MenuKind::Color(color) => {
                if let Some(section) = a.section {
                    self.section_metadata(Command::SetArrangementSectionColor { section, color });
                }
            }
            MenuKind::Remove => {
                self.focus.focus(window);
                self.delete_section_kind(true, window);
            }
            MenuKind::Delete => {
                self.focus.focus(window);
                self.delete_section_kind(false, window);
            }
            MenuKind::Duplicate => {
                self.focus.focus(window);
                self.duplicate_section(window);
            }
            MenuKind::Cut => {
                self.focus.focus(window);
                self.cut_section(window);
            }
            MenuKind::Copy => {
                self.focus.focus(window);
                self.copy_section(window);
            }
            MenuKind::Paste => {
                self.focus.focus(window);
                self.paste_section(window);
            }
            MenuKind::Merge => {
                self.focus.focus(window);
                self.merge_sections(window);
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_default_starts_idle_without_pending_or_cursor() {
        let sections = Sections::default();
        assert!(sections.gesture.is_none());
        assert!(sections.pending.is_none());
        assert!(sections.queued.is_none());
        assert!(sections.cursor.is_none());
        assert!(sections.selected.is_none());
    }
}
