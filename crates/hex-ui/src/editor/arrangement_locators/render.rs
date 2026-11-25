//! Arrangement locator editing, pointer drag, and strip rendering.
use super::*;

impl EditorView {
    pub(in crate::editor) fn delete_locator(&mut self, window: &Window) -> bool {
        if self.mode != EditorMode::Arrangement || !self.locators.focus.is_focused(window) {
            return false;
        }
        if self.locator_edit_available()
            && let Some((generation, id)) = self.locators.selected
            && generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.project.arrangement_locator(id).is_some())
            && self.enqueue(Request::Edit(Command::RemoveArrangementLocator(id)))
        {
            self.locators.selected = None;
        }
        true
    }

    pub(in crate::editor) fn rename_selected_locator(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.mode != EditorMode::Arrangement || !self.locators.focus.is_focused(window) {
            return false;
        }
        if let Some((generation, id)) = self.locators.selected
            && let Some(state) = self.locator_state()
        {
            self.begin_locator_name(
                Source {
                    generation,
                    revision: state.revision,
                    id,
                },
                window,
                cx,
            );
        }
        true
    }

    pub(super) fn begin_locator_name(
        &mut self,
        source: Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !source.current(self) || !self.locator_committed_action_available() {
            return;
        }
        let Some(locator) = self
            .locator_state()
            .and_then(|state| state.project.arrangement_locator(source.id))
        else {
            return;
        };
        let input = cx.new(|cx| browser_input::name(locator.name().unwrap_or_default().into(), cx));
        let focus = input.read(cx).focus_handle().clone();
        let blur = cx.on_blur(&focus, window, |view, _, cx| {
            view.commit_locator_name(cx);
        });
        self.locators.name = Some(NameEdit {
            source,
            input,
            _blur: blur,
        });
        focus.focus(window);
        cx.notify();
    }

    pub(super) fn commit_locator_name(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(edit) = self.locators.name.take() else {
            return true;
        };
        let accepted = if edit.source.current(self) {
            let text = edit.input.read(cx).text().to_owned();
            self.enqueue(Request::Edit(Command::RenameArrangementLocator {
                locator: edit.source.id,
                name: (!text.trim().is_empty()).then_some(text),
            }))
        } else {
            true
        };
        if !accepted {
            self.locators.name = Some(edit);
        }
        cx.notify();
        accepted
    }

    pub(in crate::editor) fn finalize_locator_name_for_open(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(edit) = &self.locators.name else {
            return true;
        };
        let name = edit.input.read(cx).text();
        if self.edit_pending()
            || !edit.source.current(self)
            || name.len() > hex_project::MAX_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            self.error = Some("finish or cancel the locator name before opening a project".into());
            return false;
        }
        self.finish_locator_name(true, window, cx);
        self.locators.name.is_none()
    }

    pub(in crate::editor) fn finish_locator_name(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.locators.name.is_none() {
            return false;
        }
        if commit {
            if !self.commit_locator_name(cx) {
                return true;
            }
        } else {
            self.locators.name = None;
        }
        self.locators.focus.focus(window);
        cx.notify();
        true
    }

    pub(super) fn update_locator_drag(&mut self, pointer: gpui::Point<gpui::Pixels>, invert: bool) {
        let Some(drag) = &self.locators.drag else {
            return;
        };
        if !drag.source.current(self)
            || !f32::from(pointer.x).is_finite()
            || !f32::from(pointer.y).is_finite()
        {
            self.locators.drag = None;
            return;
        }
        let delta = f32::from(pointer.x - drag.pointer.x);
        let vertical = f32::from(pointer.y - drag.pointer.y).abs();
        let moved = drag.moved || delta.abs() >= DRAG_THRESHOLD;
        let mut preview = drag.frame;
        if moved && let Some(state) = &self.state {
            let candidate = (drag.frame as f64
                + f64::from(delta / drag.width) * drag.viewport.length as f64)
                .round()
                .clamp(
                    drag.viewport.start as f64,
                    drag.viewport.end().unwrap_or(drag.frame).saturating_sub(1) as f64,
                ) as u64;
            if let Some(frame) = self.snap_grid.with_platform_modifier(invert).snap_frame(
                &state.project,
                candidate,
                drag.viewport,
                drag.width,
            ) && frame >= drag.viewport.start
                && drag.viewport.end().is_some_and(|end| frame < end)
                && frame <= hex_project::MAX_SPARSE_AUDIO_TIMELINE_FRAMES
                && !state
                    .project
                    .arrangement_locators()
                    .iter()
                    .any(|locator| locator.id() != drag.source.id && locator.frame() == frame)
            {
                preview = frame;
            }
        }
        if let Some(drag) = &mut self.locators.drag {
            drag.moved = moved;
            drag.click_disqualified |= vertical >= DRAG_THRESHOLD;
            drag.preview = preview;
        }
    }

    pub(super) fn finish_locator_drag(&mut self) {
        let Some(drag) = self.locators.drag.take() else {
            return;
        };
        if !drag.source.current(self)
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
        {
            return;
        }
        if self.locators.committed.is_some() {
            let playing = self
                .audio_status()
                .is_some_and(|status| status.playing.load(Acquire));
            if !drag.moved
                && !drag.click_disqualified
                && let Some(source) = self.locator_play_source()
            {
                let quantization = self.session_grid.quantization;
                if playing && quantization != hex_engine::LaunchQuantization::Immediate {
                    self.request_committed_locator_seek(drag.frame, source, Some(quantization));
                } else {
                    self.queue_applied_source_play(drag.frame, source, true);
                }
                return;
            }
            self.status = "locator playback is waiting for the current edit".into();
            return;
        }
        if !drag.moved && !drag.click_disqualified {
            self.launch_locator(drag.frame);
        } else if drag.preview != drag.frame && self.locator_edit_available() {
            self.locators.pending = Some(Pending {
                receipt: Receipt::new(drag.source.generation),
                id: drag.source.id,
                frame: drag.preview,
            });
            if !self.enqueue(Request::Edit(Command::MoveArrangementLocator {
                locator: drag.source.id,
                frame: drag.preview,
            })) {
                self.locators.refuse();
            }
        }
    }

    pub(super) fn launch_locator(&mut self, frame: u64) {
        let quantization = self.session_grid.quantization;
        if quantization != hex_engine::LaunchQuantization::Immediate
            && self
                .audio_status()
                .is_some_and(|status| status.playing.load(Acquire))
        {
            self.request_quantized_locator_launch(frame, quantization);
            return;
        }
        if let Some(source) = self.normal_start_source() {
            // An immediate marker click can jump an already-playing transport.
            self.queue_applied_source_play(frame, source, true);
        } else {
            self.queue_normal_play(frame);
        }
    }

    pub(in crate::editor) fn locator_lines(
        &self,
        viewport: Viewport,
        header_width: f32,
        prefix: &'static str,
    ) -> Div {
        let line = |frame, color| {
            div()
                .absolute()
                .left(relative(position(frame, viewport)))
                .top_0()
                .bottom_0()
                .w(px(1.0))
                .bg(rgb(color))
                .opacity(0.55)
        };
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right(px(header_width))
            .children(self.locators.visible(viewport).iter().map(|row| {
                let frame = self.locators.preview_frame(row.id, row.frame);
                let id = row.id.value();
                line(frame, self.locator_color(row, frame))
                    .id((prefix, id))
                    .debug_selector(move || format!("{prefix}-{id}"))
            }))
            .children(self.locators.visible_adds(viewport).map(|frame| {
                line(frame, self.locator_color_at_frame(frame))
                    .id(("pending-locator-line", frame))
                    .debug_selector(move || format!("{prefix}-pending-{frame}"))
            }))
    }

    pub(in crate::editor) fn locator_strip(
        &self,
        state: &ViewState,
        viewport: Viewport,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if state.project.arrangement_locators().is_empty() && self.locators.adds.is_empty() {
            return None;
        }
        let entity = cx.entity();
        let pointer = canvas(
            |_, _, _| (),
            move |_, _, window, _cx| {
                let moving = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let handled = moving.update(cx, |view, cx| {
                        if view.locators.drag.is_none() {
                            return false;
                        }
                        if event.pressed_button == Some(MouseButton::Left)
                            && view.locators.focus.is_focused(window)
                        {
                            view.update_locator_drag(event.position, event.modifiers.platform);
                        } else {
                            view.locators.drag = None;
                        }
                        cx.notify();
                        true
                    });
                    if handled {
                        cx.stop_propagation();
                    }
                });
                let releasing = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    let handled = releasing.update(cx, |view, cx| {
                        if view.locators.drag.is_none() {
                            return false;
                        }
                        if view.locators.focus.is_focused(window) {
                            view.update_locator_drag(event.position, event.modifiers.platform);
                            view.finish_locator_drag();
                        } else {
                            view.locators.drag = None;
                        }
                        cx.notify();
                        true
                    });
                    if handled {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        let markers = self.locators.visible(viewport);
        let lane = div()
            .relative()
            .h_full()
            .overflow_hidden()
            .debug_selector(|| "arrangement-locator-strip".into())
            .track_focus(&self.locators.focus)
            .children([pointer])
            .children(self.locators.visible_adds(viewport).map(|frame| {
                div()
                    .id(("pending-locator-play", frame))
                    .debug_selector(move || format!("locator-play-pending-{frame}"))
                    .absolute()
                    .left(relative(position(frame, viewport)))
                    .top_0()
                    .w(px(12.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.0))
                    .text_color(rgb(self.locator_color_at_frame(frame)))
                    .child(theme::text("▶"))
            }))
            .children(markers.iter().enumerate().map(|(index, row)| {
                let source = Source {
                    generation: self.project_generation,
                    revision: state.revision,
                    id: row.id,
                };
                let frame = self.locators.preview_frame(row.id, row.frame);
                let original = row.frame;
                let id = row.id.value();
                let x = position(frame, viewport);
                let next = markers
                    .get(index + 1)
                    .map_or(1.0, |next| position(next.frame, viewport));
                let available = ((next - x) * width - 13.0).clamp(0.0, 160.0);
                let selected = self.locators.selected == Some((source.generation, source.id));
                let name_edit = self
                    .locators
                    .name
                    .as_ref()
                    .filter(|edit| edit.source.id == row.id && edit.source.current(self));
                let label = if let Some(edit) = name_edit {
                    let input_width = width.min(140.0);
                    let input_left = ((1.0 - x) * width - input_width).min(12.0);
                    div()
                        .absolute()
                        .left(px(input_left))
                        .key_context("hex_clip_name")
                        .w(px(input_width))
                        .child(edit.input.clone())
                        .into_any_element()
                } else {
                    div()
                        .id(("locator-name", id))
                        .debug_selector(move || format!("locator-name-{id}"))
                        .w(px(available))
                        .truncate()
                        .child(theme::text(&row.name))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                                if source.current(view) {
                                    view.locators.selected = Some((source.generation, source.id));
                                    view.locators.focus.focus(window);
                                    if event.click_count >= 2 {
                                        view.begin_locator_name(source, window, cx);
                                    }
                                    cx.notify();
                                }
                                cx.stop_propagation();
                            }),
                        )
                        .into_any_element()
                };
                div()
                    .absolute()
                    .left(relative(x))
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(px(10.0))
                    .text_color(rgb(self.locator_color(row, frame)))
                    .child(
                        div()
                            .id(("locator-play", id))
                            .debug_selector(move || format!("locator-play-{id}"))
                            .w(px(12.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .when(selected, |el| {
                                el.bg(rgb(theme::color(theme::PaletteRole::Hover)))
                            })
                            .child(theme::text("▶"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                                    if source.current(view)
                                        && width.is_finite()
                                        && width > 0.0
                                        && !view.persistence.opening_project
                                        && !view.persistence.close_prompt_open
                                        && view.locators.pending.is_none()
                                        && view.locators.name.is_none()
                                    {
                                        view.locators.selected =
                                            Some((source.generation, source.id));
                                        view.locators.focus.focus(window);
                                        view.locators.drag = Some(Drag {
                                            source,
                                            viewport,
                                            width,
                                            pointer: event.position,
                                            frame: original,
                                            preview: original,
                                            moved: false,
                                            click_disqualified: false,
                                        });
                                        cx.notify();
                                    }
                                    cx.stop_propagation();
                                }),
                            ),
                    )
                    .children([label])
            }));
        Some(
            div()
                .h(px(HEIGHT))
                .flex_shrink_0()
                .pr(px(self.track_header_width()))
                .child(lane),
        )
    }
}
