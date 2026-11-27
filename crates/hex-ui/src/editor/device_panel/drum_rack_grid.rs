//! Bounded 4x4 Drum Rack pad grid and drop-target custody.
//!
//! UI captures stable `(project_generation, revision, device, pad_id)` identities
//! and renders accepted drop targets for browser audio samples and built-in leaf
//! native instruments. Canonical state edits, media import, preparation and exact
//! acknowledgement stay with `hex-session`, `hex-project`, and `hex-engine`.
use super::*;
use crate::editor::pending_edit::{Disposition, Receipt};
use hex_session::service::{DrumRackPadDropPayload, Event};
use std::{
    num::NonZeroU64,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) const FRAME_WIDTH: f32 = 280.0;

#[derive(Default)]
pub(in crate::editor) struct State {
    serial: u64,
    pending: Option<PendingPadDrop>,
}

struct PendingPadDrop {
    request_id: u64,
    receipt: Receipt,
    device: DeviceId,
    pad_id: NonZeroU64,
    label: String,
    cancel: Arc<AtomicBool>,
}

impl State {
    pub(super) fn pending_label_for(&self, device: DeviceId, pad_id: NonZeroU64) -> Option<&str> {
        self.pending
            .as_ref()
            .filter(|pending| pending.device == device && pending.pad_id == pad_id)
            .map(|pending| pending.label.as_str())
    }

    pub(in crate::editor) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

pub(super) fn render_body(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let device_id = device.id();
    let pads = match hex_engine::drum_rack_pad_views_on_control(device.state()) {
        Ok(pads) => pads,
        Err(error) => {
            return crate::device_composition::unsupported_body(theme::text(format!(
                "invalid drum rack state · {error}"
            )))
            .id(ElementId::Name(
                format!("device-body-{}", device_id.value()).into(),
            ))
            .debug_selector(move || format!("device-body-{}", device_id.value()));
        }
    };

    let cells = pads
        .into_iter()
        .map(|pad| render_pad_cell(view, cx, device_id, pad))
        .collect::<Vec<_>>();

    let body = div()
        .id(ElementId::Name(
            format!("drum-rack-grid-{}", device_id.value()).into(),
        ))
        .debug_selector(move || format!("drum-rack-grid-{}", device_id.value()))
        .w_full()
        .h_full()
        .grid()
        .grid_cols(4)
        .gap(px(2.0))
        .p(px(4.0))
        .children(cells);

    crate::device_composition::body()
        .id(ElementId::Name(
            format!("device-body-{}", device_id.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device_id.value()))
        .w(px(FRAME_WIDTH))
        .child(body)
}

fn render_pad_cell(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device_id: DeviceId,
    pad: hex_engine::DrumRackPadView,
) -> Stateful<Div> {
    let pad_id = pad.pad_id;
    let selector = format!("drum-rack-pad-{}-{}", device_id.value(), pad_id.get());
    let pending_label = view
        .device_panel
        .drum_rack_drop
        .pending_label_for(device_id, pad_id);
    let note_label = hex_engine::drum_rack_note_label(pad.receive_note);
    let (title_text, bg_role, border_role, text_role) = if let Some(label) = pending_label {
        (
            format!("{label} · loading"),
            theme::PaletteRole::Hover,
            theme::PaletteRole::Secondary,
            theme::PaletteRole::Text,
        )
    } else if pad.occupied {
        let label = if pad.name.trim().is_empty() {
            pad.child_display_name
                .clone()
                .unwrap_or_else(|| format!("pad {}", pad_id.get()))
        } else {
            pad.name.clone()
        };
        (
            label,
            theme::PaletteRole::Raised,
            theme::PaletteRole::Border,
            theme::PaletteRole::Text,
        )
    } else {
        (
            "drop here".to_owned(),
            theme::PaletteRole::Surface,
            theme::PaletteRole::Border,
            theme::PaletteRole::Muted,
        )
    };

    let bg = theme::color(bg_role);
    let text_color = theme::readable_text(bg, theme::color(text_role));
    let note_color = theme::readable_text(bg, theme::color(theme::PaletteRole::SecondaryText));

    let header = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .text_size(px(8.0))
        .line_height(px(10.0))
        .text_color(rgb(note_color))
        .child(theme::text(note_label));

    let mut cell = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .h_full()
        .min_w_0()
        .px(px(3.0))
        .py(px(2.0))
        .flex()
        .flex_col()
        .justify_between()
        .overflow_hidden()
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(theme::color(border_role)))
        .drag_over::<drag::BrowserAudioDrag>(|style, _, _, _| {
            style
                .bg(rgb(theme::color(theme::PaletteRole::Hover)))
                .border_color(rgb(theme::color(theme::PaletteRole::Secondary)))
        })
        .drag_over::<drag::NativeDeviceDrag>(|style, _, _, _| {
            style
                .bg(rgb(theme::color(theme::PaletteRole::Hover)))
                .border_color(rgb(theme::color(theme::PaletteRole::Secondary)))
        })
        .on_drop(
            cx.listener(move |view, payload: &drag::BrowserAudioDrag, _, cx| {
                cx.stop_propagation();
                view.drop_browser_sample_on_drum_rack_pad(payload, device_id, pad_id, cx);
            }),
        )
        .on_drop(
            cx.listener(move |view, payload: &drag::NativeDeviceDrag, window, cx| {
                cx.stop_propagation();
                view.drop_native_instrument_on_drum_rack_pad(
                    payload, device_id, pad_id, window, cx,
                );
            }),
        )
        .child(header)
        .child(
            div()
                .w_full()
                .truncate()
                .text_size(px(9.0))
                .line_height(px(11.0))
                .text_color(rgb(text_color))
                .child(theme::text(title_text)),
        );

    if pending_label.is_some() {
        let pending_selector = format!(
            "drum-rack-pad-pending-{}-{}",
            device_id.value(),
            pad_id.get()
        );
        cell = cell.child(
            div()
                .debug_selector(move || pending_selector.clone())
                .w_full()
                .h(px(1.0))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }

    cell
}

impl EditorView {
    pub(in crate::editor) fn drop_browser_sample_on_drum_rack_pad(
        &mut self,
        drag: &drag::BrowserAudioDrag,
        device: DeviceId,
        pad_id: NonZeroU64,
        cx: &mut Context<Self>,
    ) {
        let ready = (|| -> Result<String, &'static str> {
            if drag.project_asset.is_some() {
                return Err("project sample cannot be dropped onto a drum rack pad yet");
            }
            if !self.browser_audio_drag_is_current(drag) {
                return Err("sample drop canceled · project or browser changed");
            }
            if self.device_panel.drum_rack_drop.has_pending() {
                return Err("drum rack pad drop is already pending");
            }
            if self.persistence.opening_project
                || self.persistence.close_prompt_open
                || self.persistence.export_pending
                || self.edit_pending()
            {
                return Err("wait for the pending edit before dropping onto a drum rack pad");
            }
            if self.audio_pub.document_audio_mode != hex_session::DocumentAudioMode::Resident {
                return Err("drum rack sample loading requires a resident project");
            }
            let current_device = self
                .state
                .as_ref()
                .and_then(|state| state.project.device(device))
                .ok_or("drum rack target no longer exists")?;
            if current_device.state().type_id() != hex_engine::DRUM_RACK_TYPE_ID {
                return Err("drum rack target changed");
            }
            Ok(hex_engine::drum_rack_pad_label_for_path(&drag.path))
        })();

        self.clear_browser_audio_drag(drag);
        let label = match ready {
            Ok(label) => label,
            Err(reason) => {
                self.error = Some(reason.into());
                cx.notify();
                return;
            }
        };

        if self.submit_drum_rack_pad_drop(
            drag.project_generation,
            drag.revision,
            device,
            pad_id,
            DrumRackPadDropPayload::Sample {
                path: drag.path.clone(),
            },
            label,
            cx,
        ) {
            self.stop_browser_preview_for_drop();
        }
    }

    pub(in crate::editor) fn drop_native_instrument_on_drum_rack_pad(
        &mut self,
        drag: &drag::NativeDeviceDrag,
        device: DeviceId,
        pad_id: NonZeroU64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        let owns_drag = self
            .device_panel
            .active_native_drag
            .as_ref()
            .is_some_and(|active| {
                active.project_generation == drag.project_generation
                    && active.revision == drag.revision
                    && active.source == drag.source
                    && std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
            });

        let ready = (|| -> Result<(hex_lattice_contract::DeviceTypeId, String), &'static str> {
            if !owns_drag || !self.native_device_drag_is_current(drag) {
                return Err("device drag canceled · project or source changed");
            }
            if self.device_panel.drum_rack_drop.has_pending() {
                return Err("drum rack pad drop is already pending");
            }
            if self.persistence.opening_project
                || self.persistence.close_prompt_open
                || self.persistence.export_pending
                || self.edit_pending()
            {
                return Err("wait for the pending edit before dropping onto a drum rack pad");
            }
            let drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Instrument,
                type_id,
                ..
            } = drag.source
            else {
                return Err("drop a built-in native instrument or sample onto a drum rack pad");
            };
            if !hex_engine::drum_rack_child_instrument_admitted(type_id) {
                return Err("nested racks and unsupported devices cannot be dropped on a pad");
            }
            let current_device = self
                .state
                .as_ref()
                .and_then(|state| state.project.device(device))
                .ok_or("drum rack target no longer exists")?;
            if current_device.state().type_id() != hex_engine::DRUM_RACK_TYPE_ID {
                return Err("drum rack target changed");
            }
            let label = hex_engine::native_device_catalog_for_class(NativeDeviceClass::Instrument)
                .ok()
                .and_then(|catalog| {
                    catalog
                        .into_iter()
                        .find(|entry| entry.type_id == type_id)
                        .map(|entry| entry.name.to_lowercase())
                })
                .unwrap_or_else(|| type_id.as_str().unwrap_or("instrument").to_lowercase());
            Ok((type_id, label))
        })();

        self.clear_native_device_drag(drag);
        let (type_id, label) = match ready {
            Ok(pair) => pair,
            Err(reason) => {
                self.error = Some(reason.into());
                self.device_panel.message = Some(reason.into());
                cx.notify();
                return;
            }
        };

        let _ = self.submit_drum_rack_pad_drop(
            drag.project_generation,
            drag.revision,
            device,
            pad_id,
            DrumRackPadDropPayload::Instrument { type_id },
            label,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_drum_rack_pad_drop(
        &mut self,
        project_generation: u64,
        revision: u64,
        device: DeviceId,
        pad_id: NonZeroU64,
        payload: DrumRackPadDropPayload,
        label: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(request_id) = self.device_panel.drum_rack_drop.serial.checked_add(1) else {
            self.error = Some("drum rack pad drop request id exhausted".into());
            cx.notify();
            return false;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.device_panel.drum_rack_drop.serial = request_id;
        self.device_panel.drum_rack_drop.pending = Some(PendingPadDrop {
            request_id,
            receipt: Receipt::new(project_generation),
            device,
            pad_id,
            label: label.clone(),
            cancel: Arc::clone(&cancel),
        });
        let request = Request::DropDrumRackPad {
            request_id,
            project_generation,
            revision,
            device,
            pad_id,
            payload,
            cancel,
        };
        if self.enqueue(request) {
            self.status = format!("loading {label} onto drum rack pad");
            self.error = None;
            cx.notify();
            true
        } else {
            self.device_panel.drum_rack_drop.pending = None;
            cx.notify();
            false
        }
    }

    pub(in crate::editor) fn pending_drum_rack_drop_matches(&self, request: &Request) -> bool {
        let Some(pending) = &self.device_panel.drum_rack_drop.pending else {
            return false;
        };
        pending.receipt.is_queued()
            && matches!(
                request,
                Request::DropDrumRackPad {
                    request_id,
                    device,
                    pad_id,
                    ..
                } if *request_id == pending.request_id
                    && *device == pending.device
                    && *pad_id == pending.pad_id
            )
    }

    pub(in crate::editor) fn drum_rack_drop_submitted(&mut self) {
        if let Some(pending) = &mut self.device_panel.drum_rack_drop.pending {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(in crate::editor) fn drum_rack_drop_complete(&mut self, event: &Event) {
        let disposition = self
            .device_panel
            .drum_rack_drop
            .pending
            .as_mut()
            .map(|pending| {
                pending.receipt.observe(
                    self.project_generation,
                    event.project_replaced,
                    event.request_complete,
                )
            });
        match disposition {
            Some(Disposition::Invalidated) => {
                if let Some(pending) = self.device_panel.drum_rack_drop.pending.take() {
                    pending.cancel.store(true, Ordering::Release);
                }
            }
            Some(Disposition::Terminal) => {
                let _ = self.device_panel.drum_rack_drop.pending.take();
            }
            Some(Disposition::Waiting) | None => {}
        }
    }

    pub(in crate::editor) fn cancel_drum_rack_drop(&mut self) {
        if let Some(pending) = self.device_panel.drum_rack_drop.pending.take() {
            pending.cancel.store(true, Ordering::Release);
        }
    }

    pub(in crate::editor) fn retire_drum_rack_drop(&mut self) -> bool {
        if let Some(pending) = self.device_panel.drum_rack_drop.pending.take() {
            pending.cancel.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
}
