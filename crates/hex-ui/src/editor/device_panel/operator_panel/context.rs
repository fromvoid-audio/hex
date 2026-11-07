//! View selection guarded by the canonical instrument owner, independent of DSP edits.
use super::super::{DevicePanelOwner, EditorView};
use super::{OperatorPage, OperatorPanelState};
use gpui::{App, Bounds, Context, FocusHandle, Pixels, Window};
use hex_project::{DeviceId, TrackId};

#[derive(Clone, Copy, Debug)]
pub(in crate::editor::device_panel) struct Owner {
    generation: u64,
    track: TrackId,
    device: DeviceId,
}

impl Owner {
    pub(in crate::editor::device_panel) fn capture(
        view: &EditorView,
        device: DeviceId,
    ) -> Option<Self> {
        let state = view.state.as_ref()?;
        let track = state.project.tracks().iter().find(|track| {
            track.instrument().is_some_and(|instrument| {
                instrument.id() == device
                    && instrument.state().type_id().as_str() == Ok("hex.instrument.operator")
            })
        })?;
        Some(Self {
            generation: view.project_generation,
            track: track.id(),
            device,
        })
    }

    pub(super) fn is_alive(self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && view
                .state
                .as_ref()
                .and_then(|state| state.project.track(self.track))
                .and_then(hex_project::Track::instrument)
                .is_some_and(|instrument| {
                    instrument.id() == self.device
                        && instrument.state().type_id().as_str() == Ok("hex.instrument.operator")
                })
    }

    pub(super) fn is_current(self, view: &EditorView) -> bool {
        self.is_alive(view)
            && view.selected_track == Some(self.track)
            && view.focused_return.is_none()
            && view.device_panel.owner == Some(DevicePanelOwner::Track(self.track))
    }
}

/// Context changes are explicit and cannot abandon a pending invalid draft.
pub(in crate::editor::device_panel) fn select_oscillator(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    owner: Owner,
    operator: usize,
) -> bool {
    if operator >= 4
        || !owner.is_current(view)
        || !super::preview::finalize_controls(view, cx, owner.device)
        || !super::super::finalize_controls(view, cx)
    {
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
    if !owner.is_current(view)
        || !super::preview::finalize_controls(view, cx, owner.device)
        || !super::super::finalize_controls(view, cx)
    {
        return false;
    }
    let state = view
        .device_panel
        .operator_pages
        .entry(owner.device)
        .or_default();
    if state.page != page {
        state.page = page;
        cx.notify();
    }
    true
}

pub(super) fn select_routing(view: &mut EditorView, cx: &mut Context<EditorView>, owner: Owner) {
    select_page(view, cx, owner, OperatorPage::Routing);
}

// Section containers track descendant focus, but are never keyboard targets.
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
            handle: cx.focus_handle().tab_stop(false),
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
