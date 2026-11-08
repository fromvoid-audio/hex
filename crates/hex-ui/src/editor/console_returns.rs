//! Bounded Console Return presentation. This transient page owns no musical
//! state: existing numeric bindings, Return commands and observation caches do.
//! The bank reserves matching space in every grid row; its body has no fake
//! clip/drop targets. Name/numeric drafts remain on their Return on resize;
//! keyboard header selection reuses the shared focus owner across pages.
use super::{
    EditorView,
    console_columns::ColumnFrame,
    waveform::{button, toggle_button_with_style},
};
use crate::theme;
use gpui::{App, Context, Div, KeyBinding, Stateful, Window, div, prelude::*, px, rgb};
use hex_project::{Project, ReturnId};
use std::ops::Range;

pub(super) const COLUMN_WIDTH: f32 = 100.0;

#[derive(Default)]
pub(super) struct State {
    generation: Option<u64>,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Page {
    pub range: Range<usize>,
    total: usize,
    generation: u64,
}
impl Page {
    pub fn width(&self) -> f32 {
        self.range.len() as f32 * COLUMN_WIDTH
    }
    fn previous(&self) -> usize {
        self.range.start.saturating_sub(self.range.len())
    }
    fn next(&self) -> usize {
        self.range
            .end
            .min(self.total.saturating_sub(self.range.len()))
    }
    fn paged(&self) -> bool {
        self.total > self.range.len()
    }
}
impl State {
    pub fn page(
        &mut self,
        project: &Project,
        generation: u64,
        available: f32,
        pending: Option<ReturnId>,
    ) -> Page {
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.start = 0;
        }
        // The grid supplies the space left after one regular track, Main and
        // the insertion gutter. Limit wider banks to two working columns.
        let count = ((available / COLUMN_WIDTH).floor() as usize).clamp(1, 2);
        let total = project.return_tracks().len();
        self.start = self.start.min(total.saturating_sub(count));
        if let Some(index) =
            pending.and_then(|id| project.return_tracks().iter().position(|r| r.id() == id))
        {
            if index < self.start {
                self.start = index;
            }
            if index >= self.start + count {
                self.start = index + 1 - count;
            }
        }
        self.end = (self.start + count).min(total);
        Page {
            range: self.start..self.end,
            total,
            generation,
        }
    }
    fn current(&self, page: &Page) -> bool {
        self.generation == Some(page.generation)
            && self.start == page.range.start
            && self.end == page.range.end
    }
}

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "left",
            super::SelectPreviousReturn,
            Some("hex_console_return_header && !hex_text_editing && !hex_text_input"),
        ),
        KeyBinding::new(
            "right",
            super::SelectNextReturn,
            Some("hex_console_return_header && !hex_text_editing && !hex_text_input"),
        ),
    ]);
}

impl EditorView {
    pub(super) fn select_adjacent_console_return(
        &mut self,
        next: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode != super::EditorMode::Session
            || self.track_reorder_active()
            || !self.track_view_change_available(cx)
        {
            return;
        }
        let Some(super::track_name::Target::Return(current)) = self.focused_header_target(window)
        else {
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        let returns = state.project.return_tracks();
        let total = returns.len();
        let Some(index) = returns.iter().position(|track| track.id() == current) else {
            return;
        };
        let index = if next {
            index.saturating_add(1).min(returns.len().saturating_sub(1))
        } else {
            index.saturating_sub(1)
        };
        let Some(id) = returns.get(index).map(hex_project::ReturnTrack::id) else {
            return;
        };
        if !self.focus_return_details(id, window, cx) {
            return;
        }
        let count = self
            .console_returns
            .end
            .saturating_sub(self.console_returns.start)
            .max(1);
        if index < self.console_returns.start {
            self.console_returns.start = index;
        }
        if index >= self.console_returns.end {
            self.console_returns.start = index + 1 - count;
        }
        // Keep the desired range coherent between key events, including
        // several presses before the next page has painted.
        self.console_returns.end = self.console_returns.start.saturating_add(count).min(total);
        self.header_name_focus(super::track_name::Target::Return(id), cx)
            .focus(window);
        cx.notify();
    }

    pub(super) fn console_return_paging(
        &self,
        page: &Page,
        frame: ColumnFrame,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !page.paged() {
            return None;
        }
        let mut row = div().flex().items_center().gap(px(2.0));
        for (previous, glyph, title, selector, destination, enabled) in [
            (
                true,
                "‹",
                "previous returns",
                "console-return-previous",
                page.previous(),
                page.range.start > 0,
            ),
            (
                false,
                "›",
                "next returns",
                "console-return-next",
                page.next(),
                page.range.end < page.total,
            ),
        ] {
            let captured = page.clone();
            row = row.child(
                button(glyph, title, (selector, 0))
                    .size(px(18.0))
                    .opacity(if enabled { 1.0 } else { 0.4 })
                    .debug_selector(move || {
                        if previous {
                            "console-return-previous".into()
                        } else {
                            "console-return-next".into()
                        }
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        cx.stop_propagation();
                        if enabled
                            && frame.current(view, window)
                            && view.console_returns.current(&captured)
                            && !view.track_reorder_active()
                            && view.track_view_change_available(cx)
                        {
                            view.console_returns.start = destination;
                        }
                        cx.notify();
                    })),
            );
        }
        Some(row)
    }

    pub(super) fn console_return_bank(
        &mut self,
        page: &Page,
        frame: ColumnFrame,
        scene_width: f32,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if page.range.is_empty() {
            return None;
        }
        let state = self.state.clone()?;
        let mut bank = div()
            .id("console-return-bank")
            .debug_selector(|| "console-return-bank".into())
            .absolute()
            .right(px(scene_width))
            .top_0()
            .bottom_0()
            .w(px(page.width()))
            .flex()
            .bg(rgb(theme::color(theme::PaletteRole::Background)));
        for track in state
            .project
            .return_tracks()
            .get(page.range.clone())
            .unwrap_or_default()
        {
            let id = track.id();
            let captured = page.clone();
            let selected = self.focused_return == Some(id);
            let context_page = page.clone();
            let header = div()
                .id(("console-return-header", id.value()))
                .debug_selector(move || format!("console-return-header-{}", id.value()))
                .h(px(34.0))
                .flex_shrink_0()
                .px(px(6.0))
                .flex()
                .items_center()
                .text_size(px(11.0))
                .border_b_1()
                .border_color(rgb(theme::control_style().border))
                .bg(rgb(theme::color(if selected {
                    theme::PaletteRole::Raised
                } else {
                    theme::PaletteRole::Surface
                })))
                .tooltip(|_, cx| {
                    cx.new(|_| super::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: "return · left/right select · cmd+r rename".into(),
                    })
                    .into()
                })
                .child(self.track_name_label(
                    super::track_name::Target::Return(id),
                    track.name(),
                    cx,
                ))
                .on_click(cx.listener(move |view, _, window, cx| {
                    if frame.current(view, window) && view.console_returns.current(&captured) {
                        view.focus_return_details(id, window, cx);
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        if frame.current(view, window)
                            && view.console_returns.current(&context_page)
                        {
                            view.open_return_name_context_menu(event.position, id, window, cx);
                        }
                    }),
                );
            let muted = track.muted();
            let (fill, text) = super::track_gain::activator_colors(!muted);
            let captured = page.clone();
            let mute = toggle_button_with_style(
                "on",
                if muted {
                    "activate return"
                } else {
                    "deactivate return"
                },
                ("console-return-mute", id.value()),
                theme::action_style().with_state_colors(fill, text),
            )
            .debug_selector(move || format!("console-return-mute-{}", id.value()))
            .w(px(24.0))
            .h(px(14.0))
            .rounded(px(2.0))
            .border_1()
            .border_color(rgb(theme::control_style().border))
            .on_click(cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                if frame.current(view, window) && view.console_returns.current(&captured) {
                    super::device_panel::enqueue_return_edit(
                        view,
                        hex_project::Command::ReturnMute {
                            return_track: id,
                            muted: !muted,
                        },
                        cx,
                    );
                    cx.notify();
                }
            }));
            let gain = self.return_volume(track, cx);
            let controls = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(div().h(px(18.0)).flex_shrink_0().flex().child(gain))
                .child(
                    div()
                        .flex()
                        .gap(px(3.0))
                        .child(mute)
                        .child(self.sample_arm_button(
                            super::sample_recorder::Arm::Output(
                                hex_engine::OutputCaptureId::Return(id),
                            ),
                            cx,
                        )),
                );
            let mixer = div()
                .debug_selector(move || format!("console-return-mixer-{}", id.value()))
                .h(px(super::console_mixer::height(page.range.len())))
                .flex_shrink_0()
                .flex()
                .items_end()
                .pb(px(6.0))
                .px(px(6.0))
                .gap(px(6.0))
                .border_t_1()
                .border_color(rgb(theme::control_style().border))
                .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                .child(controls)
                .child(super::track_meters::render_return(
                    id,
                    self.track_meters.return_level(id),
                    50.0,
                ));
            bank = bank.child(
                div()
                    .debug_selector(move || format!("console-return-column-{}", id.value()))
                    .w(px(COLUMN_WIDTH))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(rgb(theme::control_style().border))
                    .child(header)
                    .child(div().flex_1().min_h_0())
                    .child(mixer),
            );
        }
        Some(bank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Editor};
    fn project_with_returns(count: usize) -> Project {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        for index in 0..count {
            editor
                .execute(Command::AddReturnTrack {
                    name: format!("bus {index}"),
                })
                .unwrap();
        }
        editor.project().clone()
    }
    #[test]
    fn page_bounds_preserve_all_returns_and_reset_with_the_project() {
        let project = project_with_returns(16);
        let mut state = State::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..8 {
            let page = state.page(&project, 1, 250.0, None);
            seen.extend(page.range.clone());
            assert_eq!(page.width(), 200.0);
            if page.range.end == 16 {
                break;
            }
            state.start = page.next();
        }
        assert_eq!(seen.len(), 16);
        let wide = state.page(&project, 2, 250.0, None);
        assert!(state.current(&wide));
        assert_eq!(state.page(&project, 2, 106.0, None).range, 0..1);
        assert!(
            !state.current(&wide),
            "old page callbacks cannot skip a Return after a width change"
        );
        let narrow = state.page(&project, 2, 106.0, None);
        state.page(&project, 3, 106.0, None);
        assert!(
            !state.current(&narrow),
            "project replacement retires old page callbacks"
        );
        assert_eq!(
            state.page(&project_with_returns(0), 2, 250.0, None).range,
            0..0
        );
        assert_eq!(
            state.page(&project_with_returns(1), 2, 250.0, None).range,
            0..1
        );
    }
    #[test]
    fn a_pending_return_stays_visible_when_the_bank_shrinks_or_is_repaged() {
        let project = project_with_returns(16);
        let id = project.return_tracks()[1].id();
        let mut state = State::default();
        assert_eq!(state.page(&project, 1, 250.0, None).range, 0..2);
        assert_eq!(state.page(&project, 1, 106.0, Some(id)).range, 1..2);
        state.start = 15;
        assert_eq!(state.page(&project, 1, 250.0, Some(id)).range, 1..3);
        assert_eq!(state.page(&project, 1, f32::NAN, None).range.len(), 1);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests {
    use super::*;
    use crate::editor::{
        EditorMode,
        clip_drag_ui_test::{audio_project, editor_window},
    };
    use gpui::{Modifiers, TestAppContext, VisualTestContext, size};
    fn selector(prefix: &str, id: ReturnId, suffix: &str) -> &'static str {
        Box::leak(format!("{prefix}-{}{suffix}", id.value()).into_boxed_str())
    }
    fn fixture(
        cx: &mut TestAppContext,
    ) -> (VisualTestContext, gpui::Entity<EditorView>, Vec<ReturnId>) {
        let (project, _, _, _) = audio_project();
        let mut model = hex_project::Editor::new(project, 32).unwrap();
        for index in 0..16 {
            model
                .execute(hex_project::Command::AddReturnTrack {
                    name: format!("bus {index}"),
                })
                .unwrap();
        }
        let ids = model
            .project()
            .return_tracks()
            .iter()
            .map(|r| r.id())
            .collect();
        let (handle, editor) = editor_window(cx, model.project().clone(), Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(880.0), px(800.0)));
        editor.update(&mut visual, |view, cx| {
            view.mode = EditorMode::Session;
            cx.notify();
        });
        visual.run_until_parked();
        (visual, editor, ids)
    }
    #[gpui::test]
    fn return_columns_align_and_page_through_all_sixteen_buses(cx: &mut TestAppContext) {
        let (mut visual, editor, ids) = fixture(cx);
        let mut seen = std::collections::BTreeSet::new();
        for page_index in 0..8 {
            let range = editor.read_with(&visual, |view, _| {
                let start = view.console_returns.start;
                start..(start + 2).min(16)
            });
            assert_eq!(
                range.start,
                page_index * 2,
                "the previous click must advance the Return bank"
            );
            let main = visual.debug_bounds("console-main-header").unwrap();
            let main_mixer = visual.debug_bounds("console-main-mixer-column").unwrap();
            let bank = visual.debug_bounds("console-return-bank").unwrap();
            let tail = visual.debug_bounds("console-column-header-tail").unwrap();
            assert!(tail.right() <= bank.left());
            assert_eq!(bank.right(), main.left());
            assert_eq!(bank.top(), main.top());
            assert_eq!(bank.bottom(), main_mixer.bottom());
            assert!(main.right() <= px(880.0));
            for index in range.clone() {
                let header = visual
                    .debug_bounds(selector("console-return-header", ids[index], ""))
                    .unwrap();
                let mixer = visual
                    .debug_bounds(selector("console-return-mixer", ids[index], ""))
                    .unwrap();
                let gain = visual
                    .debug_bounds(selector("return-volume", ids[index], "-slider"))
                    .unwrap();
                let meter = visual
                    .debug_bounds(selector("return-output-meter", ids[index], ""))
                    .unwrap();
                assert_eq!(header.left(), mixer.left());
                let column = visual
                    .debug_bounds(selector("console-return-column", ids[index], ""))
                    .unwrap();
                assert_eq!(column.size.width, px(COLUMN_WIDTH));
                assert_eq!(header.size.width + px(1.0), column.size.width);
                assert_eq!(mixer.bottom(), main_mixer.bottom());
                assert!(mixer.contains(&gain.center()) && mixer.contains(&meter.center()));
                seen.insert(ids[index]);
            }
            if range.end == 16 {
                break;
            }
            let next = visual.debug_bounds("console-return-next").unwrap().center();
            visual.simulate_click(next, Modifiers::default());
            visual.run_until_parked();
        }
        assert_eq!(seen.len(), 16);
        let header = visual
            .debug_bounds(selector("console-return-header", ids[14], ""))
            .unwrap();
        visual.simulate_click(header.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.focused_return, Some(ids[14]))
        });
        visual.simulate_mouse_down(
            header.center(),
            gpui::MouseButton::Right,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            header.center(),
            gpui::MouseButton::Right,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some()));
    }
    #[gpui::test]
    fn return_draft_blocks_paging_and_stays_visible_after_a_narrow_resize(cx: &mut TestAppContext) {
        cx.update(crate::editor::bind_editor_keys);
        let (mut visual, editor, ids) = fixture(cx);
        let field = visual
            .debug_bounds(selector("return-volume", ids[1], "-value"))
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("+100 db");
        let next = visual.debug_bounds("console-return-next").unwrap().center();
        visual.simulate_click(next, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert_eq!(view.console_returns.start, 0));
        visual.simulate_resize(size(px(640.0), px(800.0)));
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("return-volume", ids[1], "-value"))
                .is_some()
        );
        assert!(
            visual
                .debug_bounds(selector("console-return-header", ids[0], ""))
                .is_none()
        );
        let main = visual.debug_bounds("console-main-header").unwrap();
        assert!(main.right() <= px(640.0));
        let field = visual
            .debug_bounds(selector("return-volume", ids[1], "-value"))
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        let next = visual.debug_bounds("console-return-next").unwrap().center();
        visual.simulate_click(next, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert_eq!(view.console_returns.start, 2));
    }
    #[gpui::test]
    fn console_return_controls_use_the_existing_actor_and_history(cx: &mut TestAppContext) {
        use crate::editor::track_gain::tests::{actor_fixture, pump};
        use hex_project::Command;
        use hex_session::service::Request;
        let (_directory, mut visual, editor, tracks) = actor_fixture(cx);
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
                name: "bus".into()
            })))
        });
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().unwrap().project.return_tracks().len() == 1
        });
        editor.update(cx, |view, cx| {
            view.mode = EditorMode::Session;
            cx.notify();
        });
        visual.run_until_parked();
        let id = editor.read_with(cx, |view, _| {
            view.state.as_ref().unwrap().project.return_tracks()[0].id()
        });
        let field = visual
            .debug_bounds(selector("return-volume", id, "-value"))
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("-6 db");
        visual.simulate_keystrokes("enter");
        let expected = 10_f32.powf(-6.0 / 20.0);
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && (view.state.as_ref().unwrap().project.return_tracks()[0].gain() - expected).abs()
                    < 1e-5
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().unwrap().project.return_tracks()[0].gain() == 1.0
        });
        visual.run_until_parked();
        let mute = visual
            .debug_bounds(selector("console-return-mute", id, ""))
            .unwrap()
            .center();
        visual.simulate_click(mute, Modifiers::default());
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().unwrap().project.return_tracks()[0].muted()
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && !view.state.as_ref().unwrap().project.return_tracks()[0].muted()
        });
        visual.run_until_parked();
        let arm = visual
            .debug_bounds(selector("sample-arm-return", id, ""))
            .unwrap()
            .center();
        visual.simulate_click(arm, Modifiers::default());
        visual.simulate_mouse_move(gpui::point(px(0.0), px(0.0)), None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_background(selector("sample-arm-return", id, "")),
            Some(rgb(theme::color(theme::PaletteRole::Recording)).into())
        );
        // The Return bank must not become a regular-track insertion or group
        // destination, even while the existing global reorder capture is held.
        let before = editor.read_with(cx, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        let track_header = visual
            .debug_bounds(Box::leak(
                format!("session-track-{}", tracks[0].value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        let bank = visual.debug_bounds("console-return-bank").unwrap();
        let bank_center = gpui::point(bank.center().x, track_header.y);
        visual.simulate_mouse_down(track_header, gpui::MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(bank_center, gpui::MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| assert!(view.track_reorder_active()));
        visual.simulate_mouse_up(bank_center, gpui::MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0 && !view.track_reorder_active()
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
        });
    }

    #[gpui::test]
    fn keyboard_selection_crosses_return_pages_without_canonical_edits(cx: &mut TestAppContext) {
        cx.update(crate::editor::bind_editor_keys);
        let (mut visual, editor, ids) = fixture(cx);
        editor.update(&mut visual, |view, cx| {
            view.selected_track = Some(view.state.as_ref().unwrap().project.tracks()[0].id());
            cx.notify();
        });
        visual.run_until_parked();
        let (before, selected) = editor.read_with(&visual, |view, _| {
            (
                view.state.as_ref().unwrap().project.snapshot(),
                view.selected_track,
            )
        });
        let first = visual
            .debug_bounds(selector("console-return-header", ids[0], ""))
            .unwrap()
            .center();
        visual.simulate_click(first, Modifiers::default());
        visual.run_until_parked();
        for &id in ids.iter().skip(1) {
            visual.simulate_keystrokes("right");
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| assert_eq!(view.focused_return, Some(id)));
            assert!(
                visual
                    .debug_bounds(selector("console-return-header", id, ""))
                    .is_some()
            );
        }
        visual.simulate_keystrokes("right");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.focused_return, Some(ids[15]))
        });
        for index in (0..15).rev() {
            visual.simulate_keystrokes("left");
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.focused_return, Some(ids[index]))
            });
            assert!(
                visual
                    .debug_bounds(selector("console-return-header", ids[index], ""))
                    .is_some()
            );
        }
        visual.simulate_keystrokes("left");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.focused_return, Some(ids[0]))
        });
        // Repeated keys crossing an unpainted page must preserve the header's
        // keyboard context rather than falling through to clip navigation.
        visual.simulate_keystrokes("right");
        visual.run_until_parked();
        visual.simulate_keystrokes("right");
        visual.simulate_keystrokes("right");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.focused_return, Some(ids[3]));
            assert_eq!(
                (view.console_returns.start, view.console_returns.end),
                (2, 4)
            );
            assert_eq!(view.selected_track, selected);
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            assert!(view.last_enqueued_edit.is_none());
        });
    }

    #[gpui::test]
    fn return_name_and_numeric_arrows_keep_their_editing_context(cx: &mut TestAppContext) {
        cx.update(crate::editor::bind_editor_keys);
        let (mut visual, editor, ids) = fixture(cx);
        let before = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        let header = visual
            .debug_bounds(selector("console-return-header", ids[1], ""))
            .unwrap()
            .center();
        visual.simulate_click(header, Modifiers::default());
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        assert!(visual.debug_bounds("return-name-input").is_some());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("draft bus");
        visual.simulate_keystrokes("left");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.focused_return, Some(ids[1]));
            assert_eq!(
                view.track_name.pending_return(view.project_generation),
                Some(ids[1])
            );
        });
        visual.simulate_resize(size(px(640.0), px(800.0)));
        visual.run_until_parked();
        assert!(visual.debug_bounds("return-name-input").is_some());
        assert!(
            visual
                .debug_bounds(selector("console-return-header", ids[1], ""))
                .is_some()
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        visual.simulate_keystrokes("left");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.focused_return, Some(ids[0]))
        });
        let field = visual
            .debug_bounds(selector("return-volume", ids[0], "-value"))
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("-8 db");
        visual.simulate_keystrokes("right");
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert_eq!(view.focused_return, Some(ids[0]));
            assert_eq!(
                view.track_gain_controls
                    .pending_return_gain(view.project_generation, cx),
                Some(ids[0])
            );
        });
        // A delayed menu answer must not create a second text draft over an
        // unfinished numeric edit, even if the canonical revision is unchanged.
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("+100 db");
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.begin_captured_name(
                    super::super::track_name::Target::Return(ids[0]),
                    view.project_generation,
                    view.state.as_ref().unwrap().revision,
                    window,
                    cx,
                );
            });
        });
        editor.read_with(&visual, |view, cx| {
            assert!(!view.track_name.is_editing());
            assert_eq!(
                view.track_gain_controls
                    .pending_return_gain(view.project_generation, cx),
                Some(ids[0])
            );
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|message| message.contains("numeric edit"))
            );
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
        });
        // Ordinary Console track naming keeps its existing admission boundary.
        let track = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.tracks()[0].id()
        });
        let header = visual
            .debug_bounds(Box::leak(
                format!("session-track-{}", track.value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        visual.simulate_click(header, Modifiers::default());
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        assert!(visual.debug_bounds("return-name-input").is_none());
        assert!(visual.debug_bounds("track-name-input").is_none());
    }

    #[gpui::test]
    fn console_return_rename_commits_one_actor_edit_and_escape_cancels(cx: &mut TestAppContext) {
        use crate::editor::track_gain::tests::{actor_fixture, pump};
        use hex_project::Command;
        use hex_session::service::Request;
        let (_directory, mut visual, editor, _) = actor_fixture(cx);
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
                name: "bus".into()
            })))
        });
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().unwrap().project.return_tracks().len() == 1
        });
        editor.update(cx, |view, cx| {
            view.mode = EditorMode::Session;
            cx.notify();
        });
        visual.run_until_parked();
        let (id, tracks) = editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            (
                project.return_tracks()[0].id(),
                project
                    .tracks()
                    .iter()
                    .map(|t| (t.id(), t.name().to_owned()))
                    .collect::<Vec<_>>(),
            )
        });
        let header = visual
            .debug_bounds(selector("console-return-header", id, ""))
            .unwrap()
            .center();
        visual.simulate_click(header, Modifiers::default());
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        assert!(visual.debug_bounds("return-name-input").is_some());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("delay bus");
        visual.simulate_keystrokes("enter");
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().unwrap().project.return_tracks()[0].name() == "delay bus"
        });
        editor.read_with(cx, |view, _| {
            assert!(matches!(view.last_enqueued_edit.as_ref(), Some(Command::RenameReturnTrack { return_track, name }) if *return_track == id && name == "delay bus"));
            assert_eq!(view.state.as_ref().unwrap().project.tracks().iter().map(|t| (t.id(), t.name().to_owned())).collect::<Vec<_>>(), tracks);
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().unwrap().project.return_tracks()[0].name() == "bus"
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().unwrap().project.return_tracks()[0].name() == "delay bus"
        });
        visual.run_until_parked();
        let revision = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().revision);
        let header = visual
            .debug_bounds(selector("console-return-header", id, ""))
            .unwrap()
            .center();
        visual.simulate_click(header, Modifiers::default());
        visual.simulate_keystrokes("cmd-r");
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("discard me");
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(!view.track_name.is_editing());
            assert_eq!(view.worker_requests_pending, 0);
            assert_eq!(view.state.as_ref().unwrap().revision, revision);
            assert_eq!(
                view.state.as_ref().unwrap().project.return_tracks()[0].name(),
                "delay bus"
            );
        });
    }
}
