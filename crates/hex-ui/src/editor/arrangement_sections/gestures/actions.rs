//! Section naming, structural edits (split/merge/duplicate/delete) and context menus.
use super::super::*;

impl EditorView {
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
    pub(in crate::editor::arrangement_sections) fn section_name_commit(
        &mut self,
        cx: &mut Context<Self>,
    ) {
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
            self.interaction
                .time_selection
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
        let (Some(selection), Some(state)) = (self.interaction.time_selection, self.state.as_ref())
        else {
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
    pub(in crate::editor::arrangement_sections) fn section_metadata(&mut self, command: Command) {
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
    pub(in crate::editor::arrangement_sections) fn start_ahead_section_duplicate(&mut self) {
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
            || primary.selection_intent_epoch != self.interaction.selection_intent_epoch
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
        let linked_scene = section.linked_scene().filter(|scene_id| {
            state
                .project
                .scenes()
                .iter()
                .any(|scene| scene.id() == *scene_id)
        });
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
        if let Some(scene_id) = linked_scene {
            items.insert(
                0,
                MenuItem {
                    label: "edit scene in console",
                    swatch: None,
                    checked: false,
                    action: make(MenuKind::EditSceneInConsole(scene_id)),
                },
            );
        }
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
            if matches!(a.kind, MenuKind::EditSceneInConsole(_)) {
                self.error = Some("linked console scene is no longer available".into());
                cx.notify();
            }
            return;
        }
        if let MenuKind::EditSceneInConsole(scene_id) = a.kind {
            let valid = a.section.is_some_and(|section_id| {
                self.state.as_ref().is_some_and(|state| {
                    state
                        .project
                        .arrangement_section(section_id)
                        .and_then(|section| section.linked_scene())
                        == Some(scene_id)
                        && state
                            .project
                            .scenes()
                            .iter()
                            .any(|scene| scene.id() == scene_id)
                })
            });
            if !valid {
                self.error = Some("linked console scene no longer exists".into());
                cx.notify();
                return;
            }
            self.error = None;
            self.focus.focus(window);
            self.select_session_scene(scene_id);
            cx.notify();
            return;
        }
        if let Some(id) = a.section {
            self.sections.selected = Some(id);
        }
        match a.kind {
            MenuKind::EditSceneInConsole(_) => {}
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
