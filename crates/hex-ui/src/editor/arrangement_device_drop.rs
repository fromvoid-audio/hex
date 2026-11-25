//! Painted Arrangement device destinations for worker-owned insertions and moves.
use super::{
    EditorMode, EditorView,
    drag::{LaneBounds, NativeDeviceDrag, NativeDeviceDragSource},
};
use crate::theme;
use gpui::{Context, Div, Window, div, prelude::*, px, rgb};
use hex_plugin_catalog::PluginRole;
use hex_session::service::{ArrangementDeviceDestination, PluginEffectDestination};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ArrangementDropTarget {
    ExistingTrack {
        track: hex_project::TrackId,
        lane: LaneBounds,
    },
    NewTrack {
        before_track: Option<hex_project::TrackId>,
        lane: LaneBounds,
        line: f32,
    },
}
#[derive(Clone, Copy, Debug)]
pub(super) struct ArrangementDevicePreview {
    pub(super) destination: ArrangementDeviceDestination,
    pub(super) target: ArrangementDropTarget,
}
pub(super) fn arrangement_existing_track_plugin_destination(
    track: &hex_project::Track,
    role: PluginRole,
) -> Option<PluginEffectDestination> {
    match role {
        PluginRole::Effect => Some(PluginEffectDestination::Track(track.id())),
        PluginRole::Instrument => {
            if track.is_group() {
                return None;
            }
            match track.instrument().map(hex_project::Device::id) {
                Some(instrument) if track.receives_source_audio() => {
                    Some(PluginEffectDestination::ReplaceInstrument {
                        track: track.id(),
                        instrument,
                    })
                }
                Some(_) => None,
                None => Some(PluginEffectDestination::Instrument(track.id())),
            }
        }
    }
}

impl EditorView {
    pub(super) fn painted_arrangement_drop_target(
        &self,
        x: f32,
        y: f32,
    ) -> Option<ArrangementDropTarget> {
        if let Some((before_track, lane, line)) = self.painted_track_insertion_target(x, y) {
            return Some(ArrangementDropTarget::NewTrack {
                before_track,
                lane,
                line,
            });
        }
        let (track, lane) = self.hovered_track_drop_target(x, y)?;
        self.state.as_ref()?.project.track(track)?;
        Some(ArrangementDropTarget::ExistingTrack { track, lane })
    }
    fn painted_arrangement_device_drop_target(
        &self,
        x: f32,
        y: f32,
        allow_new_track: bool,
    ) -> Option<ArrangementDropTarget> {
        if !allow_new_track {
            return self
                .hovered_track_drop_target(x, y)
                .or_else(|| self.hovered_track_automation_header_drop_target(x, y))
                .map(|(track, lane)| ArrangementDropTarget::ExistingTrack { track, lane });
        }
        if let Some(target) = self.painted_arrangement_drop_target(x, y) {
            return Some(target);
        }
        if let Some((before_track, lane, line)) = self.painted_header_track_insertion_target(x, y) {
            return Some(ArrangementDropTarget::NewTrack {
                before_track,
                lane,
                line,
            });
        }
        let (track, lane) = self.hovered_track_automation_header_drop_target(x, y)?;
        self.state.as_ref()?.project.track(track)?;
        Some(ArrangementDropTarget::ExistingTrack { track, lane })
    }
    pub(super) fn update_arrangement_device_drop(
        &mut self,
        drag: &NativeDeviceDrag,
        x: f32,
        y: f32,
    ) {
        let owns = self
            .device_panel
            .active_native_drag
            .as_ref()
            .is_some_and(|active| {
                std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
                    && active.source == drag.source
                    && active.revision == drag.revision
                    && active.project_generation == drag.project_generation
            });
        if !owns {
            return;
        }
        self.device_panel.arrangement_drop_preview = None;
        let plugin_role = match &drag.source {
            NativeDeviceDragSource::Browser { .. } => None,
            NativeDeviceDragSource::Existing { .. }
                if self.device_drag_selection(drag).is_some() =>
            {
                None
            }
            NativeDeviceDragSource::PluginEffect(selection)
                if self.device_panel.pending_plugin_effect.is_none() =>
            {
                Some(selection.role)
            }
            _ => return,
        };
        if !self.native_device_drag_is_current(drag) || self.mode != EditorMode::Arrangement {
            return;
        }
        let allow_new_track = !matches!(drag.source, NativeDeviceDragSource::Existing { .. });
        let Some(target) = self.painted_arrangement_device_drop_target(x, y, allow_new_track)
        else {
            return;
        };
        if let (Some(role), ArrangementDropTarget::ExistingTrack { track, .. }) =
            (plugin_role, target)
        {
            let compatible = self
                .state
                .as_ref()
                .and_then(|state| state.project.track(track))
                .and_then(|target| arrangement_existing_track_plugin_destination(target, role))
                .is_some();
            if !compatible {
                return;
            }
        }
        let destination = match target {
            ArrangementDropTarget::NewTrack { before_track, .. } => {
                ArrangementDeviceDestination::NewTrack { before_track }
            }
            ArrangementDropTarget::ExistingTrack { track, .. } => {
                let instrument = self
                    .state
                    .as_ref()
                    .and_then(|state| state.project.track(track))
                    .and_then(|track| track.instrument())
                    .map(hex_project::Device::id);
                ArrangementDeviceDestination::ExistingTrack { track, instrument }
            }
        };
        self.device_panel.arrangement_drop_preview = Some(ArrangementDevicePreview {
            destination,
            target,
        });
        // Rail/card destinations are distinct from captured Arrangement intent.
        drag.destination.set(None);
    }
    pub(super) fn drop_arrangement_device(
        &mut self,
        drag: &NativeDeviceDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owns = self
            .device_panel
            .active_native_drag
            .as_ref()
            .is_some_and(|active| {
                std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
                    && active.source == drag.source
                    && active.revision == drag.revision
                    && active.project_generation == drag.project_generation
            });
        let captured = self.device_panel.arrangement_drop_preview;
        let pointer = window.mouse_position();
        self.update_arrangement_device_drop(drag, f32::from(pointer.x), f32::from(pointer.y));
        let current = self.device_panel.arrangement_drop_preview;
        let valid = owns
            && self.native_device_drag_is_current(drag)
            && captured.is_some_and(|preview| {
                current.is_some_and(|current| {
                    current.destination == preview.destination && current.target == preview.target
                })
            })
            && !self.persistence.save_dialog_open
            && !self.persistence.export_dialog_open
            && !self.persistence.export_pending
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
            && self.device_panel.gesture_barriers.is_empty()
            && self.device_panel.gesture.snapshot().is_none()
            && !self.device_panel.has_pending_draft(cx);
        let source = &drag.source;
        let copy = window.modifiers().alt;
        let existing_request = match (valid, captured, source) {
            (
                true,
                Some(ArrangementDevicePreview {
                    destination: ArrangementDeviceDestination::ExistingTrack { track, .. },
                    ..
                }),
                NativeDeviceDragSource::Existing { .. },
            ) => Some(self.existing_track_device_append_request(drag, track, copy, cx)),
            _ => None,
        };
        self.clear_native_device_drag(drag);
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        if let Some(request) = existing_request {
            match request {
                Ok(request) => {
                    if self.enqueue_device_barrier(request) {
                        self.error = None;
                        self.status = if copy {
                            "copying devices"
                        } else {
                            "moving devices to track"
                        }
                        .into();
                    }
                }
                Err(reason) => self.error = Some(reason.into()),
            }
        } else if let (true, Some(preview), NativeDeviceDragSource::PluginEffect(selection)) =
            (valid, captured, source)
        {
            match preview.destination {
                ArrangementDeviceDestination::ExistingTrack { track, instrument } => {
                    let destination = match (selection.role, instrument) {
                        (PluginRole::Instrument, Some(instrument)) => {
                            PluginEffectDestination::ReplaceInstrument { track, instrument }
                        }
                        (PluginRole::Instrument, None) => {
                            PluginEffectDestination::Instrument(track)
                        }
                        (PluginRole::Effect, _) => PluginEffectDestination::Track(track),
                    };
                    self.insert_plugin_effect(selection, destination, None, cx);
                }
                ArrangementDeviceDestination::NewTrack { before_track } => {
                    self.insert_plugin_track(selection, before_track, cx);
                }
            }
        } else if let (true, Some(preview), NativeDeviceDragSource::Browser { class, type_id }) =
            (valid, captured, source)
        {
            let gain = match self.device_track_gain(preview.destination) {
                Ok(gain) => gain,
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                    return;
                }
            };
            self.insert_builtin_arrangement_device(
                hex_engine::AudioSourceContext {
                    project_generation: drag.project_generation,
                    editor_revision: drag.revision,
                },
                *class,
                *type_id,
                preview.destination,
                gain,
            );
        } else {
            self.error =
                Some("device drop canceled · target or project changed, or editing is busy".into());
        }
        cx.notify();
    }
    pub(super) fn arrangement_device_drop_preview(&self) -> Option<gpui::Stateful<Div>> {
        let preview = self.device_panel.arrangement_drop_preview?;
        let drag = self.device_panel.active_native_drag.as_ref()?;
        if !self.native_device_drag_is_current(drag) {
            return None;
        }
        let body = self.arrangement_bounds?;
        let (lane, line) = match preview.target {
            ArrangementDropTarget::ExistingTrack { lane, .. } => (lane, None),
            ArrangementDropTarget::NewTrack { lane, line, .. } => (lane, Some(line)),
        };
        let empty = self.state.as_ref()?.project.tracks().is_empty();
        if let Some(line) = line.filter(|_| !empty) {
            return Some(
                div()
                    .id("arrangement-device-drop-preview")
                    .debug_selector(|| "arrangement-device-drop-preview".into())
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(line - body.top))
                    .h(px(2.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
            );
        }

        let (top, height) = (lane.top - body.top, lane.height);
        Some(
            div()
                .id("arrangement-device-drop-preview")
                .debug_selector(|| "arrangement-device-drop-preview".into())
                .absolute()
                .left(px(lane.left - body.left))
                .top(px(top))
                .w(px(lane.width))
                .h(px(height))
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Secondary))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project, TrackSelectionIds};

    #[test]
    fn arrangement_existing_track_plugin_destination_handles_instruments_groups_and_shared_sources()
    {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(empty_track) = editor
            .execute(Command::AddMidiTrack {
                name: "empty".into(),
            })
            .unwrap()
        else {
            panic!("empty track");
        };
        let Created::Track(occupied_track) = editor
            .execute(Command::AddMidiTrack {
                name: "occupied".into(),
            })
            .unwrap()
        else {
            panic!("occupied track");
        };
        let sine = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine"),
        )
        .unwrap();
        let Created::Device(instrument) = editor
            .execute(Command::InsertInstrument {
                track: occupied_track,
                state: sine,
            })
            .unwrap()
        else {
            panic!("instrument");
        };
        let source = editor
            .project()
            .track(occupied_track)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor");
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        let Created::Track(member) = editor
            .execute(Command::AddTrack {
                name: "member".into(),
            })
            .unwrap()
        else {
            panic!("member");
        };
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[member]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group");
        };

        let project = editor.project();
        assert_eq!(
            arrangement_existing_track_plugin_destination(
                project.track(empty_track).unwrap(),
                PluginRole::Instrument,
            ),
            Some(PluginEffectDestination::Instrument(empty_track))
        );
        assert_eq!(
            arrangement_existing_track_plugin_destination(
                project.track(occupied_track).unwrap(),
                PluginRole::Instrument,
            ),
            Some(PluginEffectDestination::ReplaceInstrument {
                track: occupied_track,
                instrument,
            })
        );
        assert_eq!(
            arrangement_existing_track_plugin_destination(
                project.track(contributor).unwrap(),
                PluginRole::Instrument,
            ),
            None
        );
        assert_eq!(
            arrangement_existing_track_plugin_destination(
                project.track(group).unwrap(),
                PluginRole::Instrument,
            ),
            None
        );
        assert_eq!(
            arrangement_existing_track_plugin_destination(
                project.track(group).unwrap(),
                PluginRole::Effect,
            ),
            Some(PluginEffectDestination::Track(group))
        );
    }
}
