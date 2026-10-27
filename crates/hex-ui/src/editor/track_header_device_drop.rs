//! Header destinations capture the painted TrackId, independent of
//! device-panel selection. Existing worker stamps and prepare-before-commit own
//! insertion/history. Browser effects append; browser instruments require an
//! empty chain. Option-drag copies append through the canonical clipboard edit.
use super::{
    EditorView,
    drag::{NativeDeviceDrag, NativeDeviceDragSource},
};
use crate::theme;
use gpui::{App, Context, Div, Stateful, prelude::*, rgb};
use hex_engine::NativeDeviceClass;
use hex_project::TrackId;
use hex_session::service::{NativeDeviceDropSource, Request};

impl EditorView {
    fn header_device_request(
        &self,
        drag: &NativeDeviceDrag,
        generation: u64,
        revision: Option<u64>,
        track: TrackId,
        copy: bool,
        cx: &App,
    ) -> Result<Request, &'static str> {
        if generation != drag.project_generation
            || revision != Some(drag.revision)
            || !self.native_device_drag_is_current(drag)
            || !self
                .active_native_device_drag
                .as_ref()
                .is_some_and(|active| {
                    std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
                        && active.source == drag.source
                        && active.project_generation == drag.project_generation
                        && active.revision == drag.revision
                })
        {
            return Err("device drop canceled · project or source changed");
        }
        if self.save_dialog_open
            || self.export_dialog_open
            || self.export_pending
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
            || self.device_panel.has_pending_draft(cx)
            || self.track_gain_controls.has_pending_draft(cx)
            || self.gain_numeric_editing(cx)
            || self.gain_lane.draft.is_some()
            || (self.gain_recorder.blocks_edits() || self.midi_recorder.blocks_edits())
        {
            return Err("finish pending edits before dropping a device");
        }
        let target = self
            .state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .ok_or("device drop target is no longer available")?;
        let NativeDeviceDragSource::Browser { class, type_id } = drag.source else {
            if copy {
                return self.duplicate_device_drag_request(
                    drag,
                    hex_project::DeviceChainOwner::Track(track),
                    None,
                );
            }
            return Err("hold option to copy devices onto a track");
        };
        if class == NativeDeviceClass::Instrument {
            if target.instrument().is_some() {
                return Err(
                    "instrument slot is already occupied · header drop does not replace it",
                );
            }
            if !target.devices().is_empty() {
                return Err("header instrument drop requires an empty device chain");
            }
        }
        Ok(Request::DropNativeDevice {
            project_generation: drag.project_generation,
            revision: drag.revision,
            source: NativeDeviceDropSource::Browser { class, type_id },
            track,
            before: None,
            replace: None,
        })
    }

    pub(super) fn device_drop_header(
        &self,
        track: TrackId,
        header: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let generation = self.project_generation;
        let revision = self.state.as_ref().map(|state| state.revision);
        let owner = cx.entity();
        header
            .drag_over::<NativeDeviceDrag>(move |style, drag, window, cx| {
                let valid = owner
                    .read(cx)
                    .header_device_request(
                        drag,
                        generation,
                        revision,
                        track,
                        window.modifiers().alt,
                        cx,
                    )
                    .is_ok();
                let role = if valid {
                    theme::PaletteRole::Secondary
                } else {
                    theme::PaletteRole::Primary
                };
                // Refine paint only: existing header content/hit bounds stay put.
                style
                    .bg(gpui::Hsla::from(rgb(theme::color(role))).alpha(0.18))
                    .border_color(rgb(theme::color(role)))
            })
            .on_drop(
                cx.listener(move |view, drag: &NativeDeviceDrag, window, cx| {
                    cx.stop_propagation();
                    let request = view.header_device_request(
                        drag,
                        generation,
                        revision,
                        track,
                        window.modifiers().alt,
                        cx,
                    );
                    view.clear_native_device_drag(drag);
                    cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                    match request {
                        Ok(request) => {
                            if view.enqueue_device_barrier(request) {
                                view.error = None;
                                view.status = "adding device to track".into();
                            }
                        }
                        Err(reason) => view.error = Some(reason.into()),
                    }
                    cx.notify();
                }),
            )
    }
}
