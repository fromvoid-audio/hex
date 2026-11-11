//! Header destinations capture the painted TrackId, independent of
//! device-panel selection. Existing worker stamps and prepare-before-commit own
//! insertion/history. Browser effects append; browser instruments replace the
//! existing instrument or precede effects. Option-drag copies use clipboard edits.
use super::{
    EditorView,
    drag::{BrowserAudioDrag, NativeDeviceDrag, NativeDeviceDragSource},
};
use crate::theme;
use gpui::{App, Context, Div, Stateful, prelude::*, rgb};
use hex_engine::NativeDeviceClass;
use hex_project::TrackId;
use hex_session::service::{NativeDeviceDropSource, Request};

impl EditorView {
    // A validated plugin uses the separate asynchronous insertion path;
    // native devices return their existing typed request.
    fn header_device_request(
        &self,
        drag: &NativeDeviceDrag,
        generation: u64,
        revision: Option<u64>,
        track: TrackId,
        copy: bool,
        cx: &App,
    ) -> Result<Option<Request>, &'static str> {
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
        if let NativeDeviceDragSource::PluginEffect(selection) = &drag.source {
            let target = self.check_device_append_target(track, cx)?;
            if self.pending_plugin_effect.is_some() {
                return Err("wait for the pending vst3 insertion");
            }
            if selection.role == hex_plugin_catalog::PluginRole::Instrument
                && (target.is_group()
                    || (target.instrument().is_some() && !target.receives_source_audio()))
            {
                return Err("select an ordinary receiving track for the vst3 instrument");
            }
            return Ok(None);
        }
        let NativeDeviceDragSource::Browser { class, type_id } = drag.source else {
            self.check_device_append_target(track, cx)?;
            if copy {
                return self
                    .duplicate_device_drag_request(
                        drag,
                        hex_project::DeviceChainOwner::Track(track),
                        None,
                    )
                    .map(Some);
            }
            return Err("hold option to copy devices onto a track");
        };
        if class != NativeDeviceClass::Instrument {
            return self
                .browser_device_append_request(track, class, type_id, cx)
                .map(Some);
        }
        let target = self.check_device_append_target(track, cx)?;
        Ok(Some(Request::DropNativeDevice {
            project_generation: generation,
            revision: drag.revision,
            source: NativeDeviceDropSource::Browser { class, type_id },
            track,
            before: None,
            replace: target.instrument().map(hex_project::Device::id),
        }))
    }

    pub(super) fn check_device_append_target(
        &self,
        track: TrackId,
        cx: &App,
    ) -> Result<&hex_project::Track, &'static str> {
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
        Ok(target)
    }

    pub(super) fn browser_device_append_request(
        &self,
        track: TrackId,
        class: NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        cx: &App,
    ) -> Result<Request, &'static str> {
        let target = self.check_device_append_target(track, cx)?;
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
        let state = self
            .state
            .as_ref()
            .ok_or("device drop target is no longer available")?;
        Ok(Request::DropNativeDevice {
            project_generation: self.project_generation,
            revision: state.revision,
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
        let sample_owner = owner.clone();
        header
            .drag_over::<BrowserAudioDrag>(move |style, drag, _, cx| {
                if sample_owner
                    .read(cx)
                    .browser_sampler_drop_track(drag, Some(track), cx)
                    .is_ok()
                {
                    let color = rgb(theme::color(theme::PaletteRole::Secondary));
                    style
                        .bg(gpui::Hsla::from(color).alpha(0.18))
                        .border_color(color)
                } else {
                    style
                }
            })
            .on_drop(super::sampler_load::track_drop_listener(cx, Some(track)))
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
                        Ok(Some(request)) => {
                            if view.enqueue_device_barrier(request) {
                                view.error = None;
                                view.status = "adding device to track".into();
                            }
                        }
                        Ok(None) => {
                            if let NativeDeviceDragSource::PluginEffect(selection) = &drag.source {
                                use hex_session::service::PluginEffectDestination;
                                let instrument = view
                                    .state
                                    .as_ref()
                                    .and_then(|state| state.project.track(track))
                                    .and_then(hex_project::Track::instrument)
                                    .map(hex_project::Device::id);
                                let destination = match (selection.role, instrument) {
                                    (
                                        hex_plugin_catalog::PluginRole::Instrument,
                                        Some(instrument),
                                    ) => PluginEffectDestination::ReplaceInstrument {
                                        track,
                                        instrument,
                                    },
                                    _ => PluginEffectDestination::Track(track),
                                };
                                view.insert_plugin_effect(selection, destination, None, cx);
                            } else {
                                view.error = Some("device drop source changed".into());
                            }
                        }
                        Err(reason) => view.error = Some(reason.into()),
                    }
                    cx.notify();
                }),
            )
    }
}
