//! Finite effect-only owners reuse the existing Return rail and worker commands.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum EffectBusOwner {
    Main,
    Return(hex_project::ReturnId),
}
impl EffectBusOwner {
    pub(super) fn devices(self, project: &hex_project::Project) -> Option<&[hex_project::Device]> {
        match self {
            Self::Main => Some(project.main_devices()),
            Self::Return(id) => project
                .return_tracks()
                .iter()
                .find(|r| r.id() == id)
                .map(|r| r.devices()),
        }
    }
    pub(super) fn title(self, project: &hex_project::Project) -> Option<String> {
        match self {
            Self::Main => Some("main effects".into()),
            Self::Return(id) => project
                .return_tracks()
                .iter()
                .find(|r| r.id() == id)
                .map(|r| format!("{} · return effects", r.name())),
        }
    }
    pub(super) fn processor(self) -> hex_engine::ProcessorChainOwner {
        match self {
            Self::Main => hex_engine::ProcessorChainOwner::Main,
            Self::Return(id) => hex_engine::ProcessorChainOwner::Return(id.value()),
        }
    }
    pub(super) fn prefix(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Return(_) => "return",
        }
    }
    pub(super) fn end_selector(self) -> String {
        match self {
            Self::Main => "main-device-drop-end".into(),
            Self::Return(id) => format!("return-device-drop-end-{}", id.value()),
        }
    }
    pub(super) fn insert_request(
        self,
        project_generation: u64,
        revision: u64,
        before: Option<DeviceId>,
        type_id: hex_lattice_contract::DeviceTypeId,
    ) -> Request {
        Request::InsertEffectBusDevice {
            project_generation,
            revision,
            destination: match self {
                Self::Main => hex_session::service::EffectBusDestination::Main,
                Self::Return(id) => hex_session::service::EffectBusDestination::Return(id),
            },
            class: NativeDeviceClass::Effect,
            type_id,
            before,
        }
    }
    pub(super) fn reorder(self, device: DeviceId, before: Option<DeviceId>) -> Command {
        match self {
            Self::Main => Command::MoveMainDevice { device, before },
            Self::Return(_) => Command::MoveReturnDevice { device, before },
        }
    }
}

pub(in crate::editor) fn select_main(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if view.state.is_none() || !finalize_controls(view, cx) {
        return;
    }
    view.focused_return = None;
    view.selected_track = None;
    view.track_selection = Default::default();
    view.selected_clip = None;
    view.detail_context = None;
    view.selected_cell = None;
    view.selected_scene = None;
    view.device_panel.main_selected = Some(view.project_generation);
    view.clip_view_open = true;
    view.device_panel_active = true;
    view.focus.focus(window);
    cx.notify();
}

pub(in crate::editor) fn selected_owner(
    view: &EditorView,
    state: &ViewState,
) -> Option<DevicePanelOwner> {
    let owner = selected_track_owner(view, state);
    if let Some((generation, source, anchor)) = view.device_panel.source_manager.source_view
        && generation == view.project_generation
        && anchor == owner
        && state
            .project
            .native_source(source)
            .is_some_and(|definition| {
                definition.instrument().state().type_id().as_str() == Ok("hex.instrument.fm4")
            })
        && !state
            .project
            .tracks()
            .iter()
            .any(|track| track.source_id() == Some(source))
    {
        return Some(DevicePanelOwner::Source(source));
    }
    owner
}

pub(super) fn selected_track_owner(
    view: &EditorView,
    state: &ViewState,
) -> Option<DevicePanelOwner> {
    view.focused_return
        .filter(|id| state.project.return_tracks().iter().any(|r| r.id() == *id))
        .map(DevicePanelOwner::Return)
        .or_else(|| {
            view.selected_track
                .filter(|id| state.project.track(*id).is_some())
                .map(DevicePanelOwner::Track)
        })
        .or_else(|| {
            (view.device_panel.main_selected == Some(view.project_generation))
                .then_some(DevicePanelOwner::Main)
        })
}
