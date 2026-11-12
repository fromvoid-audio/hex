//! Painted Console column boundaries capture stable anchors. GPUI owns hover
//! paint and hit testing; no model allocation, guessed IDs or retained ghost is
//! needed. The same destination is bound to hover and release. Canonical birth,
//! preparation and history stay with project/session, including async imports.
use super::{
    EditorMode, EditorView,
    drag::{BrowserAudioDrag, ClipDrag, NativeDeviceDrag, NativeDeviceDragSource},
};
use crate::theme;
use gpui::{Context, Div, IntoElement, Stateful, Window, WindowId, div, prelude::*, px, rgb};
use hex_project::{SessionSceneTarget, TrackId};
use hex_session::{
    import_batch::ConsoleDestination,
    service::{ArrangementDeviceDestination, Request},
};

/// Regular columns may clip at very narrow sizes; Return/Main columns stay
/// pinned and operable. Header, scene and mixer rows share this exact viewport.
pub(super) fn pinned_row(
    content: impl IntoElement,
    return_width: f32,
    main: impl IntoElement,
    height: f32,
) -> Div {
    div()
        .relative()
        .w_full()
        .h(px(height))
        .min_w_0()
        .flex_shrink_0()
        .flex()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .overflow_hidden()
                .child(content),
        )
        .child(div().w(px(return_width)).flex_shrink_0())
        .child(main)
}

#[cfg(all(test, feature = "ui-test-support"))]
mod layout_tests {
    use super::*;
    use gpui::{
        AppContext, Empty, Modifiers, Render, TestAppContext, VisualTestContext, point, size,
    };
    use std::{cell::Cell, rc::Rc};

    struct Fixture(Rc<Cell<usize>>, Rc<Cell<usize>>);
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let track_clicks = self.0.clone();
            let main_clicks = self.1.clone();
            pinned_row(
                div().w(px(160.0)).h_full().flex_shrink_0().on_mouse_down(
                    gpui::MouseButton::Left,
                    move |_, _, _| {
                        track_clicks.set(track_clicks.get() + 1);
                    },
                ),
                100.0,
                div()
                    .debug_selector(|| "pinned-main".into())
                    .w(px(150.0))
                    .h_full()
                    .flex_shrink_0()
                    .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                        main_clicks.set(main_clicks.get() + 1);
                    }),
                40.0,
            )
        }
    }

    #[gpui::test]
    fn narrow_regular_viewport_cannot_displace_or_intercept_pinned_main(cx: &mut TestAppContext) {
        let track = Rc::new(Cell::new(0));
        let main = Rc::new(Cell::new(0));
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        cx.update_window(handle, |_, window, cx| {
            window.replace_root(cx, |_, _| Fixture(track.clone(), main.clone()));
        })
        .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        for width in [397.0, 640.0] {
            visual.simulate_resize(size(px(width), px(40.0)));
            visual.run_until_parked();
            let bounds = visual.debug_bounds("pinned-main").unwrap();
            assert_eq!(bounds.right(), px(width));
            assert_eq!(bounds.size.width, px(150.0));
            let track_before = track.get();
            visual.simulate_click(
                point(bounds.left() - px(99.0), px(20.0)),
                Modifiers::default(),
            );
            assert_eq!(
                track.get(),
                track_before,
                "regular content cannot intercept the return bank"
            );
            let main_before = main.get();
            visual.simulate_click(bounds.center(), Modifiers::default());
            assert_eq!(main.get(), main_before + 1);
            visual.simulate_click(point(px(10.0), px(20.0)), Modifiers::default());
            assert_eq!(
                track.get(),
                track_before + 1,
                "regular interior remains operable"
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct ColumnFrame {
    generation: u64,
    revision: u64,
    window: WindowId,
    size: gpui::Size<gpui::Pixels>,
    page: usize,
    fold_revision: u64,
}
impl ColumnFrame {
    pub(super) fn capture(view: &EditorView, window: &Window, revision: u64) -> Self {
        Self {
            generation: view.project_generation,
            revision,
            window: window.window_handle().window_id(),
            size: window.bounds().size,
            page: view.session_track_start,
            fold_revision: view.arrangement_focus.projection.fold_revision(),
        }
    }
    pub(super) fn current(self, view: &EditorView, window: &Window) -> bool {
        self.current_view(view)
            && self.window == window.window_handle().window_id()
            && self.size == window.bounds().size
    }
    pub(super) fn current_view(self, view: &EditorView) -> bool {
        self.current_view_without_page(view) && self.page == view.session_track_start
    }
    fn current_view_without_page(self, view: &EditorView) -> bool {
        view.mode == EditorMode::Session
            && self.generation == view.project_generation
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.revision == self.revision)
            && self.fold_revision == view.arrangement_focus.projection.fold_revision()
    }
    pub(super) fn current_transition(
        self,
        view: &EditorView,
        window: &Window,
        page: usize,
    ) -> bool {
        self.current_view_without_page(view)
            && view.session_track_start == page
            && self.window == window.window_handle().window_id()
            && self.size == window.bounds().size
    }
    pub(super) fn current_transition_view(self, view: &EditorView, page: usize) -> bool {
        self.current_view_without_page(view) && view.session_track_start == page
    }
}
impl EditorView {
    pub(super) fn console_column_edge(
        &self,
        before: Option<TrackId>,
        scene: SessionSceneTarget,
        frame: ColumnFrame,
        header: bool,
        id: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let edge = div()
            .id(gpui::ElementId::Name(id.clone().into()))
            .debug_selector(move || id.clone())
            .absolute()
            .w(px(6.0))
            .top(px(if header { 0.0 } else { 4.0 }))
            .bottom(px(if header { 0.0 } else { 4.0 }))
            .left_0();
        self.console_column_surface(edge, before, scene, frame, header, cx)
    }
    pub(super) fn console_column_surface(
        &self,
        surface: Stateful<Div>,
        before_track: Option<TrackId>,
        scene: SessionSceneTarget,
        frame: ColumnFrame,
        header: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let target = ConsoleDestination::NewTrack {
            before_track,
            scene,
        };
        let browser_owner = cx.entity();
        let clip_owner = cx.entity();
        let device_owner = cx.entity();
        surface
            .when(!header && self.sample_drop_target() == Some(super::drag::BrowserDropDestination::Console(target)), |surface| {
                surface.bg(rgb(theme::color(theme::PaletteRole::Secondary)))
            })
            .drag_over::<BrowserAudioDrag>(move |style, drag, window, cx| {
                let view = browser_owner.read(cx);
                if !header && frame.current(view, window) && view.browser_audio_drag_is_current(drag) && view.console_target_exists(target) {
                    style.bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                } else { style }
            })
            .drag_over::<ClipDrag>(move |style, drag, window, cx| {
                let view = clip_owner.read(cx);
                if !header && frame.current(view, window) && view.console_column_clip_valid(drag) && view.console_target_exists(target) {
                    style.bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                } else { style }
            })
            .drag_over::<NativeDeviceDrag>(move |style, drag, window, cx| {
                let view = device_owner.read(cx);
                if frame.current(view, window) && view.console_column_device_valid(drag, before_track, cx) {
                    style.bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                } else { style }
            })
            .on_drag_move::<BrowserAudioDrag>(cx.listener(move |view, event: &gpui::DragMoveEvent<BrowserAudioDrag>, window, cx| {
                if !event.bounds.contains(&event.event.position) { return; }
                cx.stop_propagation();
                let Some(drag) = event.dragged_item().downcast_ref::<BrowserAudioDrag>() else { return; };
                drag.preview.set(None); drag.lane_hovered.set(false); drag.insertion_line_y.set(None);
                drag.destination.set((!header && frame.current(view, window) && view.browser_audio_drag_is_current(drag) && view.console_target_exists(target)).then_some(super::drag::BrowserDropDestination::Console(target)));
                cx.notify();
            }))
            .on_drag_move::<ClipDrag>(cx.listener(move |_, event: &gpui::DragMoveEvent<ClipDrag>, _, cx| {
                if !event.bounds.contains(&event.event.position) { return; }
                cx.stop_propagation();
                if let Some(drag) = event.dragged_item().downcast_ref::<ClipDrag>() {
                    drag.preview.set(None); drag.destination.set(None); drag.insertion_line_y.set(None); drag.lane_hovered.set(false); drag.group.borrow_mut().invalidate();
                }
                cx.notify();
            }))
            .on_drag_move::<NativeDeviceDrag>(cx.listener(move |view, event: &gpui::DragMoveEvent<NativeDeviceDrag>, _, cx| {
                if !event.bounds.contains(&event.event.position) { return; }
                cx.stop_propagation();
                if let Some(drag) = event.dragged_item().downcast_ref::<NativeDeviceDrag>() {
                    drag.destination.set(None); drag.return_destination.set(None); view.arrangement_device_preview = None;
                }
                cx.notify();
            }))
            .on_drop(cx.listener(move |view, drag: &BrowserAudioDrag, window, cx| {
                cx.stop_propagation();
                if !header && frame.current(view, window) { view.drop_console_browser_audio(drag, target, cx); }
                else { view.clear_browser_audio_drag(drag); view.error = Some("sample drop canceled · console layout changed or no scene target".into()); cx.notify(); }
            }))
            .on_drop(cx.listener(move |view, drag: &ClipDrag, window, cx| {
                cx.stop_propagation();
                if !header && frame.current(view, window) { view.drop_console_arrangement_clip(drag, target, cx); }
                else { view.retire_console_clip_drag(drag); view.error = Some("clip drop canceled · console layout changed or no scene target".into()); cx.notify(); }
            }))
            .on_drop(cx.listener(move |view, drag: &NativeDeviceDrag, window, cx| {
                cx.stop_propagation();
                let valid = frame.current(view, window) && view.console_column_device_valid(drag, before_track, cx);
                view.clear_native_device_drag(drag);
                cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                if valid && let NativeDeviceDragSource::Browser {class, type_id} = drag.source {
                    let destination = ArrangementDeviceDestination::NewTrack {before_track};
                    match view.device_track_gain(destination) {
                        Ok(new_track_gain) => { view.enqueue_device_barrier(Request::DropBrowserDeviceOnArrangement {project_generation: drag.project_generation, revision: drag.revision, class, type_id, destination, new_track_gain}); }
                        Err(error) => view.error = Some(error.into()),
                    }
                } else { view.error = Some("device drop canceled · target changed, editing is busy, or source is not a browser device".into()); }
                cx.notify();
            }))
    }
    pub(super) fn console_column_clip_valid(&self, drag: &ClipDrag) -> bool {
        self.console_clip_drag_owned(drag)
            && self.drag_start_is_current(drag)
            && drag.group.borrow().ids.len() <= 1
            && self
                .state
                .as_ref()
                .is_some_and(|state| match drag.position {
                    super::drag::ClipPosition::Audio { .. } => state
                        .project
                        .clip(drag.clip)
                        .is_some_and(|clip| clip.audio_edit().is_none()),
                    super::drag::ClipPosition::Midi { .. } => {
                        state.project.midi_clip(drag.clip).is_some()
                    }
                })
    }
    fn console_column_device_valid(
        &self,
        drag: &NativeDeviceDrag,
        before: Option<TrackId>,
        cx: &gpui::App,
    ) -> bool {
        self.native_device_drag_is_current(drag)
            && matches!(drag.source, NativeDeviceDragSource::Browser { .. })
            && self
                .active_native_device_drag
                .as_ref()
                .is_some_and(|active| std::rc::Rc::ptr_eq(&active.destination, &drag.destination))
            && self.state.as_ref().is_some_and(|state| {
                state.project.tracks().len() < hex_project::MAX_TRACKS
                    && before.is_none_or(|id| state.project.track(id).is_some())
            })
            && !self.edit_pending()
            && !self.save_dialog_open
            && !self.export_dialog_open
            && !self.export_pending
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
            && self.device_gesture_barriers.is_empty()
            && self.device_gesture.snapshot().is_none()
            && !self.device_panel.has_pending_draft(cx)
            && !self.track_gain_controls.has_pending_draft(cx)
            && !self.gain_numeric_editing(cx)
            && self.gain_lane.draft.is_none()
            && !self.gain_recorder.blocks_edits()
            && !self.midi_recorder.blocks_edits()
    }
}
