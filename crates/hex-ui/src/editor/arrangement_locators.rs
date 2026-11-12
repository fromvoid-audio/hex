//! Arrangement locator projection, hit targets, and revision-bound pointer input.
//! Only visible markers are painted. A released move retains its final position
//! until its own ordered worker terminal replaces it with canonical state.
use super::{
    EditorMode, EditorView, ViewState, browser_input,
    pending_edit::{Disposition, Receipt},
    viewport::Viewport,
};
use crate::theme;
use gpui::{
    Context, Div, Entity, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Subscription, Window, canvas, div, prelude::*, px, relative, rgb,
};
use hex_project::{ArrangementLocatorAppearance, Command, LocatorId, Project};
use hex_session::service::{Event, Request};
use std::sync::{Arc, Weak, atomic::Ordering::Acquire};

const HEIGHT: f32 = 18.0;
const DRAG_THRESHOLD: f32 = 3.0;

#[derive(Clone, Copy)]
struct Source {
    generation: u64,
    revision: u64,
    id: LocatorId,
}

impl Source {
    fn current(self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && view.mode == EditorMode::Arrangement
            && view.state.as_ref().is_some_and(|state| {
                self.revision == state.revision
                    && state.project.arrangement_locator(self.id).is_some()
            })
    }
}

struct Drag {
    source: Source,
    viewport: Viewport,
    width: f32,
    pointer: gpui::Point<gpui::Pixels>,
    frame: u64,
    preview: u64,
    moved: bool,
    click_disqualified: bool,
}

struct Pending {
    receipt: Receipt,
    id: LocatorId,
    frame: u64,
}

struct NameEdit {
    source: Source,
    input: Entity<hex_widgets::TextInput>,
    _blur: Subscription,
}

pub(super) struct Locators {
    focus: FocusHandle,
    selected: Option<(u64, LocatorId)>,
    cache: Option<(Weak<Project>, Vec<ArrangementLocatorAppearance>)>,
    drag: Option<Drag>,
    pending: Option<Pending>,
    name: Option<NameEdit>,
}

impl Locators {
    pub(super) fn new(cx: &mut Context<EditorView>) -> Self {
        Self {
            focus: cx.focus_handle(),
            selected: None,
            cache: None,
            drag: None,
            pending: None,
            name: None,
        }
    }

    pub(super) fn cancel_drag(&mut self) -> bool {
        self.drag.take().is_some()
    }

    pub(super) fn naming(&self) -> bool {
        self.name.is_some()
    }

    fn visible(&self, viewport: Viewport) -> &[ArrangementLocatorAppearance] {
        let Some((_, rows)) = &self.cache else {
            return &[];
        };
        let Some(end) = viewport.end() else {
            return &[];
        };
        let first = rows.partition_point(|locator| locator.frame < viewport.start);
        let last = rows.partition_point(|locator| locator.frame < end);
        &rows[first..last]
    }

    fn preview_frame(&self, id: LocatorId, frame: u64) -> u64 {
        if let Some(drag) = &self.drag
            && drag.source.id == id
        {
            return drag.preview;
        }
        self.pending
            .as_ref()
            .filter(|pending| pending.id == id)
            .map_or(frame, |pending| pending.frame)
    }

    pub(super) fn matches(&self, request: &Request) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.receipt.is_queued()
                && matches!(request,
                Request::Edit(Command::MoveArrangementLocator { locator, frame })
                if *locator == pending.id && *frame == pending.frame)
        })
    }

    pub(super) fn submitted(&mut self, pending_requests: usize) {
        if let Some(pending) = &mut self.pending {
            pending.receipt.submitted(pending_requests);
        }
    }

    pub(super) fn refuse(&mut self) -> bool {
        self.pending.take().is_some()
    }

    pub(super) fn observe(&mut self, generation: u64, event: &Event) {
        if self.pending.as_mut().is_some_and(|pending| {
            pending
                .receipt
                .observe(generation, event.project_replaced, event.request_complete)
                != Disposition::Waiting
        }) {
            self.pending = None;
        }
        if event.project_replaced {
            self.selected = None;
            self.drag = None;
            self.name = None;
            self.cache = None;
        }
    }
}

fn position(frame: u64, viewport: Viewport) -> f32 {
    (frame.saturating_sub(viewport.start) as f64 / viewport.length.max(1) as f64) as f32
}

impl EditorView {
    /// Navigate strictly past the stopped play start or the scheduled running
    /// cursor. At either end there is no wrap. Running seeks retain transport.
    pub(super) fn navigate_locator(&mut self, forward: bool, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || !(self.focus.is_focused(window) || self.locators.focus.is_focused(window))
            || self.track_header_focused(window)
            || !self.locator_edit_available()
        {
            return;
        }
        let playing = self.desired_playing(
            self.audio_status()
                .is_some_and(|status| status.playing.load(Acquire)),
        );
        let reference = if playing {
            self.cursor_position()
        } else {
            self.playback_start_frame()
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let locators = state.project.arrangement_locators();
        let index = if forward {
            Some(locators.partition_point(|locator| locator.frame() <= reference))
        } else {
            locators
                .partition_point(|locator| locator.frame() < reference)
                .checked_sub(1)
        };
        let Some(frame) = index
            .and_then(|index| locators.get(index))
            .map(|locator| locator.frame())
        else {
            return;
        };
        if playing {
            self.request_seek(frame);
        } else {
            self.place_locator_play_start(frame);
        }
    }

    fn locator_color(&self, row: &ArrangementLocatorAppearance, frame: u64) -> u32 {
        let color = if frame == row.frame {
            row.color
        } else {
            self.state.as_ref().and_then(|state| {
                let sections = state.project.arrangement_sections();
                let tempo = state.project.tempo_map();
                let index = sections.partition_point(|section| {
                    tempo
                        .beat_to_frame(section.end())
                        .is_ok_and(|end| end <= frame)
                });
                let section = sections.get(index)?;
                tempo
                    .beat_to_frame(section.start())
                    .ok()
                    .filter(|start| *start <= frame)
                    .map(|_| section.color())
            })
        };
        color.map_or_else(
            || theme::color(theme::PaletteRole::Muted),
            |color| theme::element_colors(color).accent,
        )
    }

    pub(super) fn refresh_locators(&mut self, state: &ViewState, viewport: Viewport) {
        if self
            .locators
            .cache
            .as_ref()
            .is_none_or(|(previous, _)| !previous.ptr_eq(&Arc::downgrade(&state.project)))
        {
            self.locators.cache = Some((
                Arc::downgrade(&state.project),
                state.project.arrangement_locator_appearances(),
            ));
        }
        if self
            .locators
            .drag
            .as_ref()
            .is_some_and(|drag| !drag.source.current(self) || drag.viewport != viewport)
        {
            self.locators.drag = None;
        }
        if self
            .locators
            .name
            .as_ref()
            .is_some_and(|name| !name.source.current(self))
        {
            self.locators.name = None;
        }
    }

    pub(super) fn add_locator(&mut self, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || !(self.focus.is_focused(window) || self.locators.focus.is_focused(window))
            || self.track_header_focused(window)
            || self.computer_keyboard.accepts_key("i")
            || !self.locator_edit_available()
        {
            return;
        }
        let frame = self
            .normalized_time_selection()
            .map(|range| range.start())
            .or_else(|| {
                self.edit_cursor
                    .filter(|cursor| cursor.generation == self.project_generation)
                    .map(|cursor| cursor.frame)
            })
            .unwrap_or_else(|| self.playback_start_frame());
        if self.state.as_ref().is_some_and(|state| {
            state
                .project
                .arrangement_locators()
                .binary_search_by_key(&frame, |locator| locator.frame())
                .is_ok()
        }) {
            self.status = "locator already exists here".into();
            return;
        }
        self.enqueue(Request::Edit(Command::AddArrangementLocator { frame }));
    }

    fn locator_edit_available(&self) -> bool {
        !self.edit_pending()
            && !self.opening_project
            && !self.close_prompt_open
            && self.locators.pending.is_none()
            && self.locators.name.is_none()
            && self.context_menu.is_none()
            && !self.theme_settings.open
    }

    pub(super) fn delete_locator(&mut self, window: &Window) -> bool {
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

    pub(super) fn rename_selected_locator(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.mode != EditorMode::Arrangement || !self.locators.focus.is_focused(window) {
            return false;
        }
        if let Some((generation, id)) = self.locators.selected
            && let Some(state) = &self.state
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

    fn begin_locator_name(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        if !source.current(self) || !self.locator_edit_available() {
            return;
        }
        let Some(locator) = self
            .state
            .as_ref()
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

    fn commit_locator_name(&mut self, cx: &mut Context<Self>) {
        if let Some(edit) = self.locators.name.take()
            && edit.source.current(self)
        {
            let text = edit.input.read(cx).text();
            self.enqueue(Request::Edit(Command::RenameArrangementLocator {
                locator: edit.source.id,
                name: (!text.trim().is_empty()).then(|| text.to_owned()),
            }));
        }
        cx.notify();
    }

    pub(super) fn finalize_locator_name_for_open(
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
        true
    }

    pub(super) fn finish_locator_name(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.locators.name.is_none() {
            return false;
        }
        if commit {
            self.commit_locator_name(cx);
        } else {
            self.locators.name = None;
        }
        self.locators.focus.focus(window);
        cx.notify();
        true
    }

    fn update_locator_drag(&mut self, pointer: gpui::Point<gpui::Pixels>, invert: bool) {
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

    fn finish_locator_drag(&mut self) {
        let Some(drag) = self.locators.drag.take() else {
            return;
        };
        if !drag.source.current(self) || self.opening_project || self.close_prompt_open {
            return;
        }
        if !drag.moved && !drag.click_disqualified {
            let command = hex_engine::PlaybackCommand::PlayFrom(drag.frame);
            #[cfg(test)]
            {
                self.last_transport_command = Some(command);
            }
            self.queue_audio_command(command);
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

    pub(super) fn locator_lines(
        &self,
        viewport: Viewport,
        header_width: f32,
        prefix: &'static str,
    ) -> Div {
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right(px(header_width))
            .children(self.locators.visible(viewport).iter().map(|row| {
                let frame = self.locators.preview_frame(row.id, row.frame);
                let id = row.id.value();
                div()
                    .id((prefix, id))
                    .debug_selector(move || format!("{prefix}-{id}"))
                    .absolute()
                    .left(relative(position(frame, viewport)))
                    .top_0()
                    .bottom_0()
                    .w(px(1.0))
                    .bg(rgb(self.locator_color(row, frame)))
                    .opacity(0.55)
            }))
    }

    pub(super) fn locator_strip(
        &self,
        state: &ViewState,
        viewport: Viewport,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if state.project.arrangement_locators().is_empty() {
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
                                        && !view.opening_project
                                        && !view.close_prompt_open
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

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use crate::editor::{
        clip_drag_ui_test::{editor_window, pump_move},
        external_audio_drop_ui_test::BatchFiles,
        musical_time::SnapGrid,
    };
    use gpui::{Modifiers, TestAppContext, VisualTestContext, point};

    fn bounds(
        visual: &mut VisualTestContext,
        prefix: &str,
        id: LocatorId,
    ) -> gpui::Bounds<gpui::Pixels> {
        let selector = Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str());
        visual
            .debug_bounds(selector)
            .expect("visible locator element")
    }

    #[gpui::test]
    fn locator_shortcuts_navigate_stopped_and_running_without_stealing_text_focus(
        cx: &mut TestAppContext,
    ) {
        use std::sync::atomic::Ordering::Acquire;

        cx.update(super::super::bind_editor_keys);
        let mut fixture = super::super::worker_ui_fixture::open(
            cx,
            [1_000, 2_000, 3_000]
                .into_iter()
                .map(|frame| Command::AddArrangementLocator { frame })
                .collect(),
        );
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        visual.update(|window, cx| {
            fixture
                .editor
                .update(cx, |view, _| window.focus(&view.focus));
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-alt-right");
        fixture.editor.read_with(&visual, |view, _| {
            assert_eq!(view.playback_start_frame(), 1_000);
            assert!(!view.audio_status().unwrap().playing.load(Acquire));
        });
        visual.simulate_keystrokes("cmd-alt-right");
        visual.simulate_keystrokes("cmd-alt-left");
        visual.simulate_keystrokes("cmd-alt-left");
        fixture.editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.playback_start_frame(),
                1_000,
                "no wrap before first locator"
            );
        });
        fixture
            .editor
            .update(&mut visual, |view, _| view.computer_keyboard.enabled = true);
        visual.simulate_keystrokes("cmd-alt-right");
        visual.simulate_keystrokes("cmd-alt-right");
        visual.simulate_keystrokes("cmd-alt-right");
        fixture.editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.playback_start_frame(),
                3_000,
                "no wrap after last locator"
            );
            assert!(view.computer_keyboard.enabled);
        });
        fixture.editor.update(&mut visual, |view, _| {
            view.computer_keyboard.enabled = false
        });
        visual.update(|window, cx| {
            fixture
                .editor
                .update(cx, |view, _| window.focus(&view.locators.focus));
        });
        visual.simulate_keystrokes("cmd-alt-left");
        fixture.editor.read_with(&visual, |view, _| {
            assert_eq!(view.playback_start_frame(), 2_000)
        });

        fixture.editor.update(&mut visual, |view, _| {
            view.queue_audio_command(hex_engine::PlaybackCommand::PlayFrom(1_500));
        });
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        fixture.editor.update(&mut visual, |view, cx| view.tick(cx));
        fixture.editor.read_with(&visual, |view, _| {
            assert!(!view.seek_pending(), "prior play seek still pending");
            assert!(
                view.locator_edit_available(),
                "locator navigation unavailable"
            );
        });
        let old_ticket = fixture
            .editor
            .read_with(&visual, |view, _| view.pending_seek_ticket);
        visual.simulate_keystrokes("cmd-alt-right");
        let ticket = fixture.editor.read_with(&visual, |view, _| {
            let ticket = view
                .pending_seek_ticket
                .expect("running navigation admitted a seek");
            assert_ne!(
                Some(ticket),
                old_ticket,
                "navigation must submit a fresh seek"
            );
            ticket
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let consumed = fixture.editor.read_with(&visual, |view, _| {
                view.audio_status().unwrap().consumed_command.load(Acquire) >= ticket
            });
            if consumed {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "seek was not consumed"
            );
            std::thread::yield_now();
        }
        // A running seek is admitted before its 128-frame declick completes.
        fixture.backend.render(&mut [[0.0; 2]; 128]).unwrap();
        fixture.editor.read_with(&visual, |view, _| {
            let status = view.audio_status().unwrap();
            assert!(status.playing.load(Acquire));
            let position = status.position.load(Acquire);
            assert!(status.consumed_command.load(Acquire) >= ticket);
            assert!((2_000..=2_192).contains(&position), "position {position}");
        });

        fixture.editor.update(&mut visual, |view, cx| view.tick(cx));
        visual.simulate_keystrokes("cmd-alt-left");
        let previous_ticket = fixture.editor.read_with(&visual, |view, _| {
            let previous = view.pending_seek_ticket.expect("previous locator admitted");
            assert_ne!(previous, ticket);
            previous
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if fixture.editor.read_with(&visual, |view, _| {
                view.audio_status().unwrap().consumed_command.load(Acquire) >= previous_ticket
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "previous seek was not consumed"
            );
            std::thread::yield_now();
        }
        fixture.backend.render(&mut [[0.0; 2]; 128]).unwrap();
        fixture.editor.read_with(&visual, |view, _| {
            let status = view.audio_status().unwrap();
            assert!(status.playing.load(Acquire));
            assert!((2_000..=2_192).contains(&status.position.load(Acquire)));
        });

        let id = fixture.editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.arrangement_locators()[0].id()
        });
        visual.update(|window, cx| {
            fixture.editor.update(cx, |view, cx| {
                let source = Source {
                    generation: view.project_generation,
                    revision: view.state.as_ref().unwrap().revision,
                    id,
                };
                view.begin_locator_name(source, window, cx);
            });
        });
        visual.run_until_parked();
        fixture
            .editor
            .read_with(&visual, |view, _| assert!(view.locators.name.is_some()));
        let before = fixture.editor.read_with(&visual, |view, _| {
            view.audio_status().unwrap().position.load(Acquire)
        });
        visual.simulate_keystrokes("cmd-alt-right");
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        fixture.editor.read_with(&visual, |view, _| {
            assert!(view.locators.name.is_some());
            assert_eq!(
                view.audio_status().unwrap().position.load(Acquire),
                before + 64
            );
        });
        drop(visual);
        fixture.finish(cx);
    }

    #[gpui::test]
    fn rendered_locator_add_play_move_rename_delete_and_cull(cx: &mut TestAppContext) {
        cx.update(super::super::bind_editor_keys);
        let files = BatchFiles::new();
        let path = files.wav("locator.wav", 48_000, 24_000);
        let (window, editor) = editor_window(cx, Project::new(48_000).unwrap(), Arc::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        pump_move(&editor, &mut visual);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Import(path)))
        });
        pump_move(&editor, &mut visual);
        let track = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.tracks()[0].id()
        });
        let lane_selector =
            Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
        let lane = visual.debug_bounds(lane_selector).expect("track lane");
        let cursor = point(lane.origin.x + lane.size.width * 0.25, lane.center().y);
        visual.simulate_mouse_down(cursor, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(cursor, MouseButton::Left, Modifiers::default());
        let cursor_frame = editor.read_with(&visual, |view, _| {
            view.edit_cursor.expect("placed cursor").frame
        });
        visual.simulate_keystrokes("i");
        pump_move(&editor, &mut visual);
        let id = editor.read_with(&visual, |view, _| {
            let locator = &view.state.as_ref().unwrap().project.arrangement_locators()[0];
            assert_eq!(locator.frame(), cursor_frame);
            locator.id()
        });
        visual.simulate_keystrokes("i");
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_locators()
                    .len(),
                1
            )
        });

        let marker = bounds(&mut visual, "locator-play", id).center();
        assert!(bounds(&mut visual, "locator-line", id).size.height > px(18.0));
        visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
        editor.update(&mut visual, |view, _| {
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::PlaybackCommand::PlayFrom(cursor_frame))
            );
            view.error = None;
            view.snap_grid = SnapGrid::Quarter;
        });

        let strip = visual.debug_bounds("arrangement-locator-strip").unwrap();
        let delta = strip.size.width * 0.18;
        let release = point(marker.x + delta, marker.y);
        let expected = editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            let viewport = view.visible_viewport(state.timeline_frames);
            let width = f32::from(strip.size.width);
            let raw = (cursor_frame as f64
                + f64::from(f32::from(delta) / width) * viewport.length as f64)
                .round() as u64;
            let frame = view
                .snap_grid
                .snap_frame(&state.project, raw, viewport, width)
                .unwrap();
            assert_ne!(frame, raw);
            frame
        });
        visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(release, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let preview = bounds(&mut visual, "locator-play", id);
        visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(&mut visual, |view, cx| {
            let pending = view.locators.pending.as_ref().expect("released preview");
            assert_eq!(pending.frame, expected);
            let deadline = crate::test_wait::hang_deadline();
            loop {
                if let Ok(event) = view.service.try_event() {
                    if event.request_complete {
                        assert!(event.error.is_none());
                        cx.notify();
                        return event;
                    }
                    view.receive(event);
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "locator move terminal"
                );
                std::thread::yield_now();
            }
        });
        for _ in 0..2 {
            visual.run_until_parked();
            assert_eq!(bounds(&mut visual, "locator-play", id), preview);
        }
        editor.update(&mut visual, |view, cx| {
            view.receive(terminal);
            assert!(view.locators.pending.is_none());
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_locator(id)
                    .unwrap()
                    .frame(),
                expected
            );
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, "locator-play", id), preview);

        let marker = preview.center();
        let release = point(marker.x - px(19.0), marker.y);
        let inverted_frame = editor.read_with(&visual, |view, _| {
            let viewport = view.visible_viewport(view.state.as_ref().unwrap().timeline_frames);
            let delta = f64::from(f32::from(release.x - marker.x) / f32::from(strip.size.width));
            (expected as f64 + delta * viewport.length as f64).round() as u64
        });
        visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(release, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(
            release,
            MouseButton::Left,
            Modifiers {
                platform: true,
                ..Modifiers::default()
            },
        );
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_locator(id)
                    .unwrap()
                    .frame(),
                inverted_frame
            );
        });

        let name = bounds(&mut visual, "locator-name", id).center();
        visual.simulate_event(MouseDownEvent {
            position: name,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 2,
            first_mouse: false,
        });
        visual.simulate_mouse_up(name, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_input("Verse Hit");
        visual.simulate_keystrokes("enter");
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_locator(id)
                    .unwrap()
                    .name(),
                Some("Verse Hit")
            )
        });

        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddArrangementLocator {
                frame: 1_000_000
            })))
        });
        pump_move(&editor, &mut visual);
        let hidden = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.arrangement_locators()[1].id()
        });
        for prefix in ["locator-play", "locator-line"] {
            let selector = Box::leak(format!("{prefix}-{}", hidden.value()).into_boxed_str());
            assert!(
                visual.debug_bounds(selector).is_none(),
                "offscreen locator was painted"
            );
        }
        visual.simulate_keystrokes("backspace");
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_locator(id)
                    .is_none()
            )
        });
    }
}
