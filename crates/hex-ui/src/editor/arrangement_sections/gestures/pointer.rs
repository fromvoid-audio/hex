//! Section drag cursor, capture and pointer lifecycle handling.
use super::super::*;

impl EditorView {
    pub(in crate::editor::arrangement_sections) fn section_drag_cursor(
        &self,
    ) -> Option<gpui::CursorStyle> {
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
                            || view.pointer_hits_review_handle(event.position)
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
        if self.audio_pub.document_audio_mode != hex_session::DocumentAudioMode::Resident
            || self.selected_clip.is_some()
            || self.sections.selected_ids() != [section]
            || pending.selection_intent_epoch != self.interaction.selection_intent_epoch
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
        let observation = self.audio_pub.pending_publication_observation.as_ref()?;
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
            && pending.selection_intent_epoch == self.interaction.selection_intent_epoch;
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
    pub(in crate::editor::arrangement_sections) fn select_section_metadata(
        &mut self,
        id: Option<SectionId>,
    ) {
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
        self.interaction.time_selection = None;
        self.interaction.time_selection_project_scope = false;
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
            && self.interaction.time_selection.is_none_or(|selection| {
                selection.anchor_frame == selection.end_frame && self.sections.cursor.is_some()
            })
            && self.selected_clip.is_none()
            && self.selected_track.is_none()
            && self.session_grid.focused_return.is_none()
            && self.device_panel.main_selected == Some(self.project_generation)
    }
    pub(in crate::editor::arrangement_sections) fn sections_available(&self) -> bool {
        self.sections_idle() && self.section_input_open()
    }
    /// No section edit or global modal/recording/device-gesture barrier is in flight.
    /// Unrelated worker requests (such as clip duplication) do not block sections.
    pub(in crate::editor) fn sections_idle(&self) -> bool {
        !self.edit_blocked_outside_request_queue()
            && self.sections.pending.is_none()
            && !self.section_install_pending()
    }
    pub(super) fn pending_section_is_copy(&self) -> bool {
        self.sections.pending.as_ref().is_some_and(|pending| {
            matches!(
                &pending.command,
                Command::DuplicateArrangementSection(_) | Command::PasteArrangementSection { .. }
            ) || copied_section_source(&pending.command).is_some()
        })
    }
    /// Pointer section gestures are admitted while idle OR while a single
    /// section copy is in flight and no follow-up is queued yet.
    pub(in crate::editor::arrangement_sections) fn section_pointer_available(&self) -> bool {
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
    pub(in crate::editor::arrangement_sections) fn section_input_open(&self) -> bool {
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
            && self.interaction.active_fade_drag.is_none()
            && !self.interaction.fade_key_held
            && self.interaction.clip_paint_gesture.is_none()
            && self.midi_editor.note_gesture.is_none()
            && self.midi_editor.group_gesture.is_none()
            && self.interaction.active_clip_drag.is_none()
            && self.interaction.active_trim_drag.is_none()
            && self.midi_editor.active_trim_drag.is_none()
            && self.browser.active_audio_drag.is_none()
            && self.device_panel.active_native_drag.is_none()
            && self.interaction.time_selection_gesture.is_none()
            && self.interaction.time_range_move_gesture.is_none()
            && self.gain_lane.draft.is_none()
            && self.settings.tuning.client.snapshot().is_none()
            && !self.settings.tuning.client.has_pending_requests()
            && !self.settings.tuning.client.is_recovery_pending()
    }

    pub(in crate::editor::arrangement_sections) fn section_current(&self, g: &Gesture) -> bool {
        self.section_pointer_available()
            && g.generation == self.project_generation
            && self.section_geometry() == Some(g.layout)
            && g.snap == self.snap_grid
            && self.state.as_ref().is_some_and(|s| {
                s.revision == g.revision && self.visible_viewport(s.timeline_frames) == g.viewport
            })
    }

    pub(in crate::editor::arrangement_sections) fn section_launch_action(
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
            self.interaction.suppress_cursor_click = true;
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
            if !extend || !self.interaction.time_selection_project_scope {
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
        self.interaction.suppress_cursor_click = true;
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
        let gesture = self.sections.gesture.take().is_some();
        let scene_drag = self.sections.scene_drag.take().is_some();
        gesture || scene_drag
    }
}
