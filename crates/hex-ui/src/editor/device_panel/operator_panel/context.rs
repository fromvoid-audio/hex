//! View selection guarded by canonical track or retained-source instrument identity.
use super::super::{DevicePanelOwner, EditorView};
use super::{OperatorPage, OperatorPanelState};
use gpui::{App, Bounds, Context, FocusHandle, Pixels, Window};
use hex_project::{DeviceId, SourceId, TrackId};

#[derive(Clone, Copy, Debug)]
enum InstrumentOwner {
    Track(TrackId),
    Source(SourceId),
}

#[derive(Clone, Copy, Debug)]
pub(in crate::editor::device_panel) struct Owner {
    generation: u64,
    owner: InstrumentOwner,
    device: DeviceId,
}

impl Owner {
    pub(in crate::editor::device_panel) fn capture(
        view: &EditorView,
        device: DeviceId,
    ) -> Option<Self> {
        let state = view.state.as_ref()?;
        if let Some(DevicePanelOwner::Source(source)) = view.device_panel.owner {
            let definition = state.project.native_source(source)?;
            let instrument = definition.instrument();
            if instrument.id() != device
                || instrument.state().type_id().as_str() != Ok("hex.instrument.operator")
                || state
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.source_id() == Some(source))
            {
                return None;
            }
            return Some(Self {
                generation: view.project_generation,
                owner: InstrumentOwner::Source(source),
                device,
            });
        }
        let track = state.project.tracks().iter().find(|track| {
            track.instrument().is_some_and(|instrument| {
                instrument.id() == device
                    && instrument.state().type_id().as_str() == Ok("hex.instrument.operator")
            })
        })?;
        Some(Self {
            generation: view.project_generation,
            owner: InstrumentOwner::Track(track.id()),
            device,
        })
    }

    pub(super) fn is_alive(self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && view
                .state
                .as_ref()
                .and_then(|state| match self.owner {
                    InstrumentOwner::Track(track) => state.project.track(track)?.instrument(),
                    InstrumentOwner::Source(source) => {
                        if state
                            .project
                            .tracks()
                            .iter()
                            .any(|track| track.source_id() == Some(source))
                        {
                            return None;
                        }
                        Some(state.project.native_source(source)?.instrument())
                    }
                })
                .is_some_and(|instrument| {
                    instrument.id() == self.device
                        && instrument.state().type_id().as_str() == Ok("hex.instrument.operator")
                })
    }

    pub(super) fn is_current(self, view: &EditorView) -> bool {
        self.is_alive(view)
            && match self.owner {
                InstrumentOwner::Track(track) => {
                    view.selected_track == Some(track)
                        && view.focused_return.is_none()
                        && view.device_panel.owner == Some(DevicePanelOwner::Track(track))
                }
                InstrumentOwner::Source(source) => {
                    view.device_panel.owner == Some(DevicePanelOwner::Source(source))
                        && view.state.as_ref().is_some_and(|state| {
                            view.device_panel.source_manager.source_view.is_some_and(
                                |(generation, selected, anchor)| {
                                    generation == self.generation
                                        && selected == source
                                        && anchor
                                            == super::super::effect_bus::selected_track_owner(
                                                view, state,
                                            )
                                },
                            )
                        })
                }
            }
    }

    /// Immutable canonical state for this currently selected instrument owner.
    pub(in crate::editor::device_panel) fn state(
        self,
        view: &EditorView,
    ) -> Option<&hex_lattice_contract::NativeDeviceState> {
        if !self.is_current(view) {
            return None;
        }
        let project = &view.state.as_ref()?.project;
        Some(match self.owner {
            InstrumentOwner::Track(track) => project.track(track)?.instrument()?.state(),
            InstrumentOwner::Source(source) => project.native_source(source)?.instrument().state(),
        })
    }
}

/// Context changes are explicit and cannot abandon a pending invalid draft.
pub(in crate::editor::device_panel) fn select_oscillator(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    owner: Owner,
    operator: usize,
) -> bool {
    if operator >= 4 || !owner.is_current(view) || !super::super::finalize_controls(view, cx) {
        return false;
    }
    let current = view
        .device_panel
        .operator_pages
        .get(&owner.device)
        .copied()
        .unwrap_or_default();
    let target = OperatorPanelState {
        selected_operator: operator,
        page: if current.page == OperatorPage::Oscillator {
            OperatorPage::Oscillator
        } else {
            OperatorPage::Envelope
        },
        ..current
    };
    if target != current {
        view.device_panel
            .operator_pages
            .insert(owner.device, target);
        cx.notify();
    }
    true
}

pub(super) fn select_page(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    owner: Owner,
    page: OperatorPage,
) -> bool {
    if !owner.is_current(view) || !super::super::finalize_controls(view, cx) {
        return false;
    }
    let state = view
        .device_panel
        .operator_pages
        .entry(owner.device)
        .or_default();
    let page = if page == OperatorPage::Oscillator && state.page == OperatorPage::Oscillator {
        OperatorPage::Envelope
    } else {
        page
    };
    if state.page != page {
        state.page = page;
        cx.notify();
    }
    true
}

pub(super) fn select_routing(view: &mut EditorView, cx: &mut Context<EditorView>, owner: Owner) {
    select_page(view, cx, owner, OperatorPage::Routing);
}

// Oscillator section backgrounds are explicit keyboard selection targets.
// Other containers only track descendant focus.
pub(in crate::editor::device_panel) struct SectionFocus {
    pub(in crate::editor::device_panel) handle: FocusHandle,
    pub(in crate::editor::device_panel) focused: bool,
    subscriptions: Option<(gpui::Subscription, gpui::Subscription)>,
}

pub(super) fn section_focus(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    section: super::Section,
) -> (FocusHandle, bool) {
    let state = view
        .device_panel
        .operator_section_focus
        .entry((device, section))
        .or_insert_with(|| SectionFocus {
            handle: cx
                .focus_handle()
                .tab_index(0)
                .tab_stop(matches!(section, super::Section::Oscillator(_))),
            focused: false,
            subscriptions: None,
        });
    (state.handle.clone(), state.focused)
}

pub(super) fn section_listener(
    view: &EditorView,
    cx: &Context<EditorView>,
    device: DeviceId,
    section: super::Section,
) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
    let entity = cx.weak_entity();
    let owner = Owner::capture(view, device);
    move |_, window, cx| {
        let _ = entity.update(cx, |view, cx| {
            let key = (device, section);
            let Some(state) = view.device_panel.operator_section_focus.get(&key) else {
                return;
            };
            if state.subscriptions.is_some() {
                return;
            }
            let handle = state.handle.clone();
            let focus_in = cx.on_focus_in(&handle, window, move |view, window, cx| {
                if !owner.is_some_and(|owner| owner.is_alive(view)) {
                    return;
                }
                if let Some(state) = view.device_panel.operator_section_focus.get_mut(&key) {
                    state.focused = true;
                }
                // GPUI delivers focus events after painting, when immediate
                // notifications cannot dirty the window. Schedule the changed
                // section/context for the next frame without requiring input.
                cx.defer_in(window, |_, _, cx| cx.notify());
            });
            let focus_out = cx.on_focus_out(&handle, window, move |view, _, window, cx| {
                if let Some(state) = view.device_panel.operator_section_focus.get_mut(&key) {
                    state.focused = false;
                    cx.defer_in(window, |_, _, cx| cx.notify());
                }
            });
            if let Some(state) = view.device_panel.operator_section_focus.get_mut(&key) {
                state.subscriptions = Some((focus_in, focus_out));
            }
        });
    }
}

/// View-only expansion. Never hide an unfinished invalid native parameter edit.
pub(in crate::editor::device_panel) fn toggle_options(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    owner: Owner,
) -> bool {
    if !owner.is_current(view) || !super::super::finalize_controls(view, cx) {
        return false;
    }
    let state = view
        .device_panel
        .operator_pages
        .entry(owner.device)
        .or_default();
    state.options_open = !state.options_open;
    cx.notify();
    true
}
