//! Pending section submission, queue continuation and canonical reconciliation.
use super::super::*;

impl EditorView {
    pub(in crate::editor::arrangement_sections) fn submit_section(
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
            && self.pending_requests.len() < crate::editor::UI_REQUEST_CAPACITY;
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
            selection_intent_epoch: self.interaction.selection_intent_epoch,
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
                        .is_some_and(|epoch| Some(epoch) == self.interaction.selection_intent_epoch)
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
                    .is_none_or(|epoch| Some(epoch) != self.interaction.selection_intent_epoch)
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
    pub(in crate::editor::arrangement_sections) fn section_stamp_current(
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
}
