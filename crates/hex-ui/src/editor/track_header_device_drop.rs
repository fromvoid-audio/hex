//! Header destinations capture the painted TrackId, independent of
//! device-panel selection. Existing worker stamps and prepare-before-commit own
//! insertion/history. Browser effects append; browser instruments replace the
//! existing instrument or precede effects. Track-chain moves append the frozen
//! selection as one edit. Option-drag copies use clipboard edits.
use super::{
    EditorView,
    device_panel::DevicePanelOwner,
    drag::{BrowserAudioDrag, NativeDeviceDrag, NativeDeviceDragSource},
};
use crate::theme;
use gpui::{App, Context, Div, Stateful, prelude::*, rgb};
use hex_engine::NativeDeviceClass;
use hex_project::{DeviceChainOwner, TrackId};
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
                .device_panel
                .active_native_drag
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
        match drag.source {
            NativeDeviceDragSource::PluginEffect(ref selection) => {
                let target = self.check_device_append_target(track, cx)?;
                if self.device_panel.pending_plugin_effect.is_some() {
                    return Err("wait for the pending vst3 insertion");
                }
                if selection.role == hex_plugin_catalog::PluginRole::Instrument
                    && (target.is_group()
                        || (target.instrument().is_some() && !target.receives_source_audio()))
                {
                    return Err("select an ordinary receiving track for the vst3 instrument");
                }
                Ok(None)
            }
            NativeDeviceDragSource::Existing { .. } => self
                .existing_track_device_append_request(drag, track, copy, cx)
                .map(Some),
            NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Instrument,
                type_id,
            } => {
                let target = self.check_device_append_target(track, cx)?;
                if target.is_group() {
                    return Err("group tracks accept audio effects, not clips or instruments");
                }
                Ok(Some(Request::DropNativeDevice {
                    project_generation: generation,
                    revision: drag.revision,
                    source: NativeDeviceDropSource::Browser {
                        class: NativeDeviceClass::Instrument,
                        type_id,
                    },
                    track,
                    before: None,
                    replace: target.instrument().map(hex_project::Device::id),
                    control: None,
                }))
            }
            NativeDeviceDragSource::Browser { class, type_id } => self
                .browser_device_append_request(track, class, type_id, cx)
                .map(Some),
        }
    }

    /// Resolve the held device selection against the same append contract for
    /// headers and Arrangement lanes. Callers validate their own painted target.
    pub(super) fn existing_track_device_append_request(
        &self,
        drag: &NativeDeviceDrag,
        track: TrackId,
        copy: bool,
        cx: &App,
    ) -> Result<Request, &'static str> {
        self.check_device_append_target(track, cx)?;
        if copy {
            return self.duplicate_device_drag_request(drag, DeviceChainOwner::Track(track), None);
        }
        let (owner, selected) = self
            .device_drag_selection(drag)
            .ok_or("device move canceled · selection changed")?;
        let DevicePanelOwner::Track(source) = owner else {
            return Err("move from a track device chain");
        };
        let project = &self
            .state
            .as_ref()
            .ok_or("device move canceled · project changed")?
            .project;
        project
            .validate_track_device_move(source, selected, track)
            .map_err(|error| error.0)?;
        Ok(Request::MoveTrackDevices {
            project_generation: drag.project_generation,
            revision: drag.revision,
            source,
            selected: selected.clone(),
            destination: track,
        })
    }

    pub(super) fn check_device_append_target(
        &self,
        track: TrackId,
        cx: &App,
    ) -> Result<&hex_project::Track, &'static str> {
        self.check_device_append_ready(cx)?;
        self.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .ok_or("device drop target is no longer available")
    }

    pub(super) fn check_device_append_ready(&self, cx: &App) -> Result<(), &'static str> {
        if self.persistence.save_dialog_open
            || self.persistence.export_dialog_open
            || self.persistence.export_pending
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_panel.gesture_barriers.is_empty()
            || self.device_panel.gesture.snapshot().is_some()
            || self.device_panel.has_pending_draft(cx)
            || self.track_gain_controls.has_pending_draft(cx)
            || self.gain_numeric_editing(cx)
            || self.gain_lane.draft.is_some()
            || (self.gain_recorder.blocks_edits() || self.midi_editor.recorder.blocks_edits())
        {
            return Err("finish pending edits before dropping a device");
        }
        Ok(())
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
            if target.is_group() {
                return Err("group tracks accept audio effects, not clips or instruments");
            }
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
            control: None,
        })
    }

    fn header_device_insertion_gap_active(&self, drag: &NativeDeviceDrag) -> bool {
        self.mode == super::EditorMode::Arrangement
            && !matches!(drag.source, NativeDeviceDragSource::Existing { .. })
            && self
                .device_panel
                .arrangement_drop_preview
                .is_some_and(|preview| {
                    matches!(
                        preview.target,
                        super::arrangement_device_drop::ArrangementDropTarget::NewTrack { .. }
                    )
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
                let view = sample_owner.read(cx);
                let is_gap = view.mode == super::EditorMode::Arrangement
                    && view
                        .device_panel
                        .sampler_load
                        .header_insertion_line_y
                        .is_some();
                if !is_gap
                    && view
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
            .on_drop(
                cx.listener(move |view, drag: &BrowserAudioDrag, window, cx| {
                    if let Some((before_track, _)) =
                        view.painted_sampler_track_gap(window.mouse_position())
                    {
                        view.drop_browser_sample_on_new_track(drag, before_track, cx);
                    } else {
                        view.drop_browser_sample_on_track(
                            drag,
                            Some(track),
                            hex_project::InstrumentLoadPlacement::First,
                            cx,
                        );
                    }
                }),
            )
            .drag_over::<NativeDeviceDrag>(move |style, drag, window, cx| {
                let view = owner.read(cx);
                if view.header_device_insertion_gap_active(drag) {
                    return style;
                }
                let valid = view
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
                    let pointer = window.mouse_position();
                    if view.mode == super::EditorMode::Arrangement
                        && !matches!(drag.source, NativeDeviceDragSource::Existing { .. })
                        && view
                            .painted_header_track_insertion_target(
                                f32::from(pointer.x),
                                f32::from(pointer.y),
                            )
                            .is_some()
                    {
                        view.drop_arrangement_device(drag, window, cx);
                        return;
                    }
                    let copy = window.modifiers().alt;
                    let request =
                        view.header_device_request(drag, generation, revision, track, copy, cx);
                    view.clear_native_device_drag(drag);
                    cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                    match request {
                        Ok(Some(request)) => {
                            if view.enqueue_catalog_device(request) {
                                view.error = None;
                                view.status = match drag.source {
                                    NativeDeviceDragSource::Existing { .. } if copy => {
                                        "copying devices"
                                    }
                                    NativeDeviceDragSource::Existing { .. } => {
                                        "moving devices to track"
                                    }
                                    _ => "adding device to track",
                                }
                                .into();
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
                                    (hex_plugin_catalog::PluginRole::Instrument, None) => {
                                        PluginEffectDestination::Instrument(track)
                                    }
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

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{Command, Created, Editor, Project, TrackSelectionIds};
    use std::{cell::Cell, rc::Rc, sync::Arc};

    #[gpui::test]
    fn header_device_request_refuses_browser_native_instrument_on_group_track(
        cx: &mut TestAppContext,
    ) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(ordinary) = editor
            .execute(Command::AddTrack {
                name: "ordinary".into(),
            })
            .unwrap()
        else {
            panic!("ordinary track");
        };
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[ordinary]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group track");
        };
        let (handle, view) = crate::editor::rendered_test_fixture::editor_window(
            cx,
            editor.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();

        view.update(&mut visual, |view, cx| {
            let generation = view.project_generation;
            let revision = view.state.as_ref().unwrap().revision;
            let inst_drag = NativeDeviceDrag {
                project_generation: generation,
                revision,
                source: NativeDeviceDragSource::Browser {
                    class: NativeDeviceClass::Instrument,
                    type_id: DeviceTypeId::from_static("hex.instrument.sine"),
                },
                destination: Rc::new(Cell::new(None)),
                return_destination: Rc::new(Cell::new(None)),
            };
            view.device_panel.active_native_drag = Some(inst_drag.clone());

            assert_eq!(
                view.header_device_request(
                    &inst_drag,
                    generation,
                    Some(revision),
                    group,
                    false,
                    cx
                )
                .unwrap_err(),
                "group tracks accept audio effects, not clips or instruments",
                "group track header must reject browser native instruments on drag-over and drop"
            );
            assert!(
                view.browser_device_append_request(
                    group,
                    NativeDeviceClass::Instrument,
                    DeviceTypeId::from_static("hex.instrument.sine"),
                    cx
                )
                .is_err()
            );
            assert!(
                view.header_device_request(
                    &inst_drag,
                    generation,
                    Some(revision),
                    ordinary,
                    false,
                    cx
                )
                .is_ok(),
                "ordinary track header must still accept browser native instruments"
            );

            let fx_drag = NativeDeviceDrag {
                project_generation: generation,
                revision,
                source: NativeDeviceDragSource::Browser {
                    class: NativeDeviceClass::Effect,
                    type_id: DeviceTypeId::from_static("hex.utility"),
                },
                destination: Rc::new(Cell::new(None)),
                return_destination: Rc::new(Cell::new(None)),
            };
            view.device_panel.active_native_drag = Some(fx_drag.clone());
            assert!(
                view.header_device_request(&fx_drag, generation, Some(revision), group, false, cx)
                    .is_ok(),
                "group track header must still accept browser native effects"
            );
        });
    }
}
