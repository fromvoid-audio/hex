//! Browser intent and cancellation only. Session retains the admitted request;
//! the audio owner retains native resources through its exact terminal.
use super::track_proposals::{Anchor, TrackProposal};
use super::*;
use gpui::{Div, div, prelude::*, rgb};
use hex_session::service::{
    ArrangementDeviceDestination, DeviceInsertConfirmation, GraphEditOperation,
    PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
    ResidentGraphSubmission,
};
use std::rc::Rc;

#[cfg(all(test, feature = "ui-test-support"))]
mod confirmation_tests;

fn device_confirmation_title(operation: GraphEditOperation) -> &'static str {
    match operation {
        GraphEditOperation::InsertDevice => "interrupt playback to insert this device?",
        GraphEditOperation::ReplaceDevice => "interrupt playback to replace this device?",
        GraphEditOperation::RemoveDevice => "interrupt playback to remove this device?",
        GraphEditOperation::MoveDevice => "interrupt playback to move devices?",
        GraphEditOperation::MoveTrack => "interrupt playback to move this track?",
        GraphEditOperation::PowerDevice => "interrupt playback to change device power?",
        GraphEditOperation::RemoveTrack => "interrupt playback to remove this track?",
    }
}

gpui::actions!(
    hex_plugin_proposal,
    [ActivatePluginProposal, CancelDeviceConfirmation]
);

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui::KeyBinding::new("enter", ActivatePluginProposal, Some("hex_plugin_proposal")),
        gpui::KeyBinding::new("space", ActivatePluginProposal, Some("hex_plugin_proposal")),
        gpui::KeyBinding::new(
            "escape",
            CancelDeviceConfirmation,
            Some("hex_device_confirmation"),
        ),
    ]);
}

#[cfg(feature = "ui-test-support")]
pub(crate) fn fixture_view(
    window: gpui::AnyWindowHandle,
    cx: &gpui::App,
) -> Result<crate::PluginEffectTestView, String> {
    window
        .read(cx, |view: gpui::Entity<EditorView>, cx| {
            let view = view.read(cx);
            let (parameter_controls, parameter_query_pending) =
                device_panel::vst3_parameters::test_state(view);
            let devices: std::collections::BTreeSet<_> = parameter_controls
                .iter()
                .map(|(device, _, _)| *device)
                .collect();
            let parameter_touches = devices
                .into_iter()
                .filter_map(|device| {
                    let (row, control, lane) =
                        device_panel::vst3_parameters::test_touch_state(view, device, cx);
                    row.map(|(parameter, normalized)| {
                        (device, parameter, normalized, control, lane)
                    })
                })
                .collect();
            crate::PluginEffectTestView {
                state: view.state.clone(),
                pending: view.device_panel.pending_plugin_effect.is_some(),
                queued_requests: view.worker_requests_pending
                    + view.pending_requests.len()
                    + view.device_panel.gesture_barriers.len(),
                parameter_controls,
                parameter_query_pending,
                parameter_touches,
                parameter_gesture: view
                    .device_panel
                    .gesture
                    .snapshot()
                    .and_then(|snapshot| snapshot.plugin_value)
                    .map(|value| value.normalized()),
                operation: view
                    .device_panel
                    .pending_plugin_effect
                    .as_ref()
                    .map(|pending| Arc::downgrade(&pending.control)),
                error: view.error.as_ref().map(ToString::to_string),
                selected_owner: view
                    .state
                    .as_ref()
                    .and_then(|state| device_panel::selected_owner(view, state))
                    .and_then(|owner| match owner {
                        device_panel::DevicePanelOwner::Track(id) => {
                            Some(PluginEffectDestination::Track(id))
                        }
                        device_panel::DevicePanelOwner::Return(id) => {
                            Some(PluginEffectDestination::Return(id))
                        }
                        device_panel::DevicePanelOwner::Main => Some(PluginEffectDestination::Main),
                        device_panel::DevicePanelOwner::Source(_) => None,
                    }),
                status: view.status.clone(),
                applied: view
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.status())
                    .and_then(|status| status.native_effect_source()),
                transport: view
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.status())
                    .map(|status| {
                        use std::sync::atomic::Ordering::Acquire;
                        (status.position.load(Acquire), status.playing.load(Acquire))
                    }),
            }
        })
        .map_err(|error| error.to_string())
}

pub(super) struct PendingPluginEffect {
    pub(super) status: String,
    pub(super) proposal: PluginProposal,
    control: Arc<PluginEffectControl>,
    pub(super) trace: Option<Rc<crate::diagnostics::PluginLoad>>,
}

pub(super) struct DeviceConfirmation {
    offer: DeviceInsertConfirmation,
    modal_focus: Option<gpui::FocusHandle>,
    action_focus: Option<[gpui::FocusHandle; 2]>,
    focused_once: bool,
    previous_focus: Option<gpui::FocusHandle>,
}

pub(super) struct DeviceConfirmationFocusRestore {
    target: gpui::FocusHandle,
    retired: Vec<gpui::FocusHandle>,
}

impl DeviceConfirmation {
    pub(super) fn new(offer: DeviceInsertConfirmation) -> Self {
        Self {
            offer,
            modal_focus: None,
            action_focus: None,
            focused_once: false,
            previous_focus: None,
        }
    }

    fn owns_focus_handle(&self, handle: &gpui::FocusHandle) -> bool {
        self.modal_focus.as_ref().is_some_and(|own| own == handle)
            || self
                .action_focus
                .as_ref()
                .is_some_and(|handles| handles.iter().any(|own| own == handle))
    }

    fn has_focus(&self, window: &Window) -> bool {
        self.modal_focus
            .as_ref()
            .is_some_and(|handle| handle.is_focused(window))
            || self
                .action_focus
                .as_ref()
                .is_some_and(|handles| handles.iter().any(|handle| handle.is_focused(window)))
    }
}

impl Drop for DeviceConfirmation {
    fn drop(&mut self) {
        // Closing or replacing the editor discards an unanswered choice.
        if self.offer.is_pending() {
            self.offer.cancel();
        }
    }
}
impl Drop for PendingPluginEffect {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

#[derive(Clone)]
pub(super) struct PluginProposal {
    pub(super) destination: PluginEffectDestination,
    pub(super) before: Option<hex_project::DeviceId>,
    name: String,
    loading: &'static str,
    before_empty: u64,
}

pub(super) struct FailedPluginProposal {
    proposal: PluginProposal,
    control: Arc<PluginEffectControl>,
    error: String,
}

#[derive(Clone, Copy)]
enum ProposalSurface {
    ArrangementTrack,
    SessionTrack,
    Slot,
}

enum SlotPosition {
    First,
    Before(hex_project::DeviceId),
    End,
}

impl PluginProposal {
    fn slot(&self, owner: device_panel::DevicePanelOwner) -> Option<SlotPosition> {
        use PluginEffectDestination as Destination;
        use device_panel::DevicePanelOwner as Owner;
        let effect_position = || Some(self.before.map_or(SlotPosition::End, SlotPosition::Before));
        match (self.destination, owner) {
            (Destination::Track(track), Owner::Track(id)) if track == id => effect_position(),
            (Destination::Instrument(track), Owner::Track(id)) if track == id => {
                Some(SlotPosition::First)
            }
            (
                Destination::ReplaceInstrument {
                    track,
                    instrument: replacing,
                }
                | Destination::ReplaceTrackEffect { track, replacing },
                Owner::Track(id),
            ) if track == id => Some(SlotPosition::Before(replacing)),
            (Destination::Return(target), Owner::Return(id)) if target == id => effect_position(),
            (Destination::Main, Owner::Main) => effect_position(),
            (Destination::ReplaceMainEffect { replacing }, Owner::Main) => {
                Some(SlotPosition::Before(replacing))
            }
            (
                Destination::ReplaceReturnEffect {
                    return_track,
                    replacing,
                },
                Owner::Return(id),
            ) if return_track == id => Some(SlotPosition::Before(replacing)),
            _ => None,
        }
    }
}

impl EditorView {
    fn plugin_proposal(
        &self,
    ) -> Option<(&PluginProposal, &Arc<PluginEffectControl>, Option<&str>)> {
        if let Some(pending) = self
            .device_panel
            .pending_plugin_effect
            .as_ref()
            .filter(|pending| !pending.control.is_cancelled())
        {
            return Some((&pending.proposal, &pending.control, None));
        }
        self.device_panel
            .failed_plugin_proposal
            .as_ref()
            .map(|failed| {
                (
                    &failed.proposal,
                    &failed.control,
                    Some(failed.error.as_str()),
                )
            })
    }

    pub(super) fn proposed_plugin_track(&self) -> Option<Anchor> {
        let proposal = self.plugin_proposal()?.0;
        match proposal.destination {
            PluginEffectDestination::NewTrack { before_track, .. } => Some((
                before_track,
                TrackProposal::Device {
                    before_empty: proposal.before_empty,
                },
            )),
            _ => None,
        }
    }

    pub(super) fn proposed_plugin_slot_index(
        &self,
        owner: device_panel::DevicePanelOwner,
        mut chain: impl ExactSizeIterator<Item = hex_project::DeviceId>,
    ) -> Option<usize> {
        let (proposal, _, _) = self.plugin_proposal()?;
        let len = chain.len();
        Some(match proposal.slot(owner)? {
            SlotPosition::First => 0,
            SlotPosition::Before(id) => chain.position(|device| device == id).unwrap_or(len),
            SlotPosition::End => len,
        })
    }

    pub(super) fn restore_proposed_plugin_focus(&self, window: &mut Window) {
        if self
            .device_panel
            .proposed_plugin_action_focus
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window))
            && !self.plugin_proposal_is_visible()
        {
            self.focus.focus(window);
        }
    }

    fn plugin_proposal_is_visible(&self) -> bool {
        let Some((proposal, _, _)) = self.plugin_proposal() else {
            return false;
        };
        match proposal.destination {
            PluginEffectDestination::NewTrack { .. } => match self.mode {
                EditorMode::Arrangement => true,
                EditorMode::Session => self.state.as_ref().is_some_and(|state| {
                    self.session_track_page(&state.project, self.session_grid.visible_columns)
                        .proposal_index(TrackProposal::Device {
                            before_empty: proposal.before_empty,
                        })
                        .is_some()
                }),
            },
            _ => {
                self.audio_detail.clip_view_open
                    && self.device_panel.active
                    && self
                        .state
                        .as_ref()
                        .and_then(|state| device_panel::selected_owner(self, state))
                        .is_some_and(|owner| proposal.slot(owner).is_some())
            }
        }
    }

    pub(super) fn proposed_plugin_track_row(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .debug_selector(|| "arrangement-pending-device-track".into())
            .w_full()
            .h(px(super::tracks::TRACK_ROW_HEIGHT))
            .flex()
            .justify_end()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .children(
                self.proposed_plugin_surface(ProposalSurface::ArrangementTrack, cx)
                    .map(|header| header.w(px(self.track_header_width()))),
            )
    }

    pub(super) fn proposed_session_track(
        &mut self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id("session-pending-device-track")
            .debug_selector(|| "session-pending-device-track".into())
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .children(self.proposed_plugin_surface(ProposalSurface::SessionTrack, cx))
    }

    pub(super) fn proposed_plugin_slot_card(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(card) = self.proposed_plugin_surface(ProposalSurface::Slot, cx) else {
            return div();
        };
        card.debug_selector(|| "device-chain-loading-slot".into())
            .w(px(device_panel::PROPOSED_SLOT_WIDTH))
            .h(px(device_panel::DEVICE_FRAME_HEIGHT))
            .mr(px(device_panel::DEVICE_CHAIN_PADDING))
            .flex_shrink_0()
    }

    fn proposed_plugin_surface(
        &mut self,
        surface: ProposalSurface,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let (proposal, control, error) = self.plugin_proposal()?;
        let title = hex_session::service::text::verbatim(&proposal.name);
        let failed = error.is_some();
        let detail = error.unwrap_or(proposal.loading).to_owned();
        let caption = if matches!(surface, ProposalSurface::SessionTrack) {
            if failed { "load failed" } else { "loading" }.to_owned()
        } else {
            detail.clone()
        };
        let tooltip = super::display_error::DisplayError::from(detail);
        let control = Arc::clone(control);
        let focus = self
            .device_panel
            .proposed_plugin_action_focus
            .get_or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
            .clone();
        let action = move |view: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            if failed {
                if view
                    .device_panel
                    .failed_plugin_proposal
                    .as_ref()
                    .is_some_and(|track| Arc::ptr_eq(&track.control, &control))
                {
                    view.device_panel.failed_plugin_proposal = None;
                }
            } else if let Some(pending) = &view.device_panel.pending_plugin_effect
                && Arc::ptr_eq(&pending.control, &control)
            {
                pending.control.cancel();
            }
            view.restore_proposed_plugin_focus(window);
            cx.stop_propagation();
            cx.notify();
        };
        let key_action = action.clone();
        let (action_id, status_id) = match (surface, failed) {
            (ProposalSurface::ArrangementTrack, true) => (
                "arrangement-pending-device-dismiss",
                "arrangement-pending-device-status",
            ),
            (ProposalSurface::ArrangementTrack, false) => (
                "arrangement-pending-device-cancel",
                "arrangement-pending-device-status",
            ),
            (ProposalSurface::SessionTrack, true) => (
                "session-pending-device-dismiss",
                "session-pending-device-status",
            ),
            (ProposalSurface::SessionTrack, false) => (
                "session-pending-device-cancel",
                "session-pending-device-status",
            ),
            (ProposalSurface::Slot, true) => (
                "device-chain-loading-dismiss",
                "device-chain-loading-status",
            ),
            (ProposalSurface::Slot, false) => {
                ("device-chain-loading-cancel", "device-chain-loading-status")
            }
        };
        let button = hex_widgets::action(
            theme::text(if failed { "dismiss" } else { "cancel" }),
            theme::action_style(),
        )
        .id(action_id)
        .debug_selector(move || action_id.into())
        .h(px(22.0))
        .px_2()
        .text_xs()
        .track_focus(&focus)
        .key_context("hex_plugin_proposal")
        .focus(|style| {
            style
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Text)))
        })
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |view, _, window, cx| action(view, window, cx)))
        .on_action(
            cx.listener(move |view, _: &ActivatePluginProposal, window, cx| {
                key_action(view, window, cx);
            }),
        );
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .when(matches!(surface, ProposalSurface::SessionTrack), |header| {
                    header.h_full().line_height(px(11.0))
                })
                .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .child(div().truncate().child(theme::text(title)))
                        .child(
                            div()
                                .text_color(rgb(theme::color(if failed {
                                    theme::PaletteRole::Error
                                } else {
                                    theme::PaletteRole::Muted
                                })))
                                .truncate()
                                .id(status_id)
                                .debug_selector(move || status_id.into())
                                .tooltip(move |_, cx| {
                                    cx.new(|_| super::display_error::ErrorTooltip(tooltip.clone()))
                                        .into()
                                })
                                .child(theme::text(caption)),
                        ),
                )
                .children([button]),
        )
    }

    pub(super) fn install_device_confirmation(&mut self, offer: DeviceInsertConfirmation) {
        if !self.live_mode_enabled() {
            if offer.is_pending() {
                let _ = offer.proceed();
            }
            return;
        }
        let previous_focus = self
            .device_panel
            .insert_confirmation
            .as_ref()
            .and_then(|current| current.previous_focus.clone());
        let mut next = DeviceConfirmation::new(offer);
        next.previous_focus = previous_focus;
        self.device_panel.confirmation_restore_focus = None;
        self.device_panel.insert_confirmation = Some(next);
    }

    pub(super) fn clear_device_confirmation_on_project_replace(&mut self) {
        if let Some(confirmation) = self.device_panel.insert_confirmation.take() {
            let mut retired = Vec::with_capacity(3);
            if let Some(modal) = &confirmation.modal_focus {
                retired.push(modal.clone());
            }
            if let Some(actions) = &confirmation.action_focus {
                retired.extend(actions.iter().cloned());
            }
            self.device_panel.confirmation_restore_focus = Some(DeviceConfirmationFocusRestore {
                target: self.focus.clone(),
                retired,
            });
        }
    }

    pub(super) fn restore_device_confirmation_focus(&mut self, window: &mut Window, cx: &App) {
        if self.device_panel.insert_confirmation.is_none()
            && let Some(restore) = self.device_panel.confirmation_restore_focus.take()
            && (window.focused(cx).is_none()
                || restore
                    .retired
                    .iter()
                    .any(|handle| handle.is_focused(window)))
        {
            restore.target.focus(window);
        }
    }

    pub(super) fn device_insert_confirmation_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let pending = self
            .device_panel
            .insert_confirmation
            .as_ref()?
            .offer
            .is_pending();
        if !pending {
            let retired = self.device_panel.insert_confirmation.take();
            if let Some(confirmation) = retired
                && (confirmation.has_focus(window) || window.focused(cx).is_none())
            {
                let target = confirmation
                    .previous_focus
                    .clone()
                    .unwrap_or_else(|| self.focus.clone());
                target.focus(window);
            }
            self.device_panel.confirmation_restore_focus = None;
            return None;
        }
        if self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.persistence.save_dialog_open
            || self.update_preparing()
        {
            if let Some(confirmation) = self.device_panel.insert_confirmation.as_mut() {
                confirmation.focused_once = false;
            }
            return None;
        }
        let confirmation = self.device_panel.insert_confirmation.as_mut()?;
        let title = device_confirmation_title(confirmation.offer.operation());
        // Keep the choice on the editor through repaint and native focus
        // changes. Only the matching button/key action answers the worker's offer.
        let submission = confirmation.offer.submission();
        let modal_focus = confirmation
            .modal_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let [cancel_focus, proceed_focus] = confirmation
            .action_focus
            .get_or_insert_with(|| {
                std::array::from_fn(|_| cx.focus_handle().tab_index(0).tab_stop(true))
            })
            .clone();
        if !confirmation.focused_once {
            if confirmation.previous_focus.is_none() {
                confirmation.previous_focus = window
                    .focused(cx)
                    .filter(|handle| !confirmation.owns_focus_handle(handle));
            }
            cancel_focus.focus(window);
            confirmation.focused_once = true;
        }
        let actions = [false, true].map(|proceed| {
            let (label, id, focus) = if proceed {
                (
                    "proceed",
                    "device-insert-confirmation-proceed",
                    &proceed_focus,
                )
            } else {
                ("cancel", "device-insert-confirmation-cancel", &cancel_focus)
            };
            hex_widgets::action(theme::text(label), theme::action_style())
                .id(id)
                .debug_selector(move || id.into())
                .h(px(24.0))
                .px_3()
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                .track_focus(focus)
                .key_context("hex_plugin_proposal")
                .focus(|style| {
                    style
                        .border_1()
                        .border_color(rgb(theme::color(theme::PaletteRole::Text)))
                })
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.answer_device_confirmation(submission, proceed, window, cx);
                }))
                .on_action(
                    cx.listener(move |view, _: &ActivatePluginProposal, window, cx| {
                        view.answer_device_confirmation(submission, proceed, window, cx);
                    }),
                )
        });
        let key_cancel_focus = cancel_focus.clone();
        let key_proceed_focus = proceed_focus.clone();
        let card = div()
            .id("device-insert-confirmation")
            .debug_selector(|| "device-insert-confirmation".into())
            .w(px(420.0))
            .max_w_full()
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .text_xs()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(theme::text(title)),
            )
            .child(
                div()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(
                        "proceeding will noticeably interrupt playback: audio fades briefly \
                         and resumes from the same position, and held notes or effect tails \
                         may reset.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .children(actions),
            );
        Some(
            div()
                .modal_mouse()
                .id("device-insert-confirmation-modal")
                .debug_selector(|| "device-insert-confirmation-modal".into())
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .bg(gpui::rgba(0x00000077))
                .track_focus(&modal_focus)
                .key_context("hex_device_confirmation")
                .on_action(
                    cx.listener(move |view, _: &CancelDeviceConfirmation, window, cx| {
                        view.answer_device_confirmation(submission, false, window, cx);
                    }),
                )
                .on_key_down(
                    cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                view.answer_device_confirmation(submission, false, window, cx);
                            }
                            "tab" => {
                                if key_cancel_focus.is_focused(window) {
                                    key_proceed_focus.focus(window);
                                } else {
                                    key_cancel_focus.focus(window);
                                }
                                cx.notify();
                                cx.stop_propagation();
                            }
                            "enter" | "space" => {
                                if key_proceed_focus.is_focused(window) {
                                    view.answer_device_confirmation(submission, true, window, cx);
                                } else if key_cancel_focus.is_focused(window) {
                                    view.answer_device_confirmation(submission, false, window, cx);
                                } else {
                                    cx.stop_propagation();
                                }
                            }
                            _ => {
                                cx.stop_propagation();
                            }
                        }
                    }),
                )
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(gpui::MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(gpui::MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(card),
        )
    }

    fn answer_device_confirmation(
        &mut self,
        submission: ResidentGraphSubmission,
        proceed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(confirmation) = self.device_panel.insert_confirmation.as_ref()
            && confirmation.offer.submission() == submission
            && confirmation.offer.is_pending()
        {
            if proceed {
                confirmation.offer.proceed();
            } else {
                confirmation.offer.cancel();
            }
            let retired = self.device_panel.insert_confirmation.take();
            self.device_panel.confirmation_restore_focus = None;
            let target = retired
                .and_then(|confirmation| confirmation.previous_focus.clone())
                .unwrap_or_else(|| self.focus.clone());
            target.focus(window);
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn cancel_plugin_effect(&self) {
        if let Some(pending) = &self.device_panel.pending_plugin_effect {
            pending.control.cancel();
        }
    }

    pub(super) fn plugin_effect_complete(&mut self, event: &hex_session::service::Event) {
        let Some(terminal) = self
            .device_panel
            .pending_plugin_effect
            .as_ref()
            .and_then(|pending| {
                event
                    .plugin_effect_complete
                    .iter()
                    .chain(event.device_insert_proposal_complete.iter())
                    .find(|terminal| Arc::ptr_eq(&pending.control, &terminal.control))
            })
        else {
            return;
        };
        if terminal.accepted
            && !event.project_replaced
            && self.mode == EditorMode::Session
            && let Some((_, proposal)) = self.proposed_plugin_track()
        {
            self.retain_session_track_proposal_page(proposal);
        }
        if let Some(intent) = self.deferred_toggle_play.as_mut()
            && let TogglePlayTarget::DeviceProposal { output } = intent.target
            && terminal.accepted
            && !event.project_replaced
            && event.device_insert_complete.as_ref().is_none_or(|done| {
                done.outcome == hex_session::service::DeviceInsertOutcome::Installed
                    && done.submission.output_identity == output
                    && done.submission.context.project_generation == intent.generation
                    && done.submission.context.editor_revision == event.state.revision
            })
        {
            intent.target = TogglePlayTarget::CommittedDevice {
                output,
                revision: event.state.revision,
                audio_revision: event
                    .device_insert_complete
                    .as_ref()
                    .map(|done| done.submission.audio_revision),
            };
        }
        self.retire_plugin_effect(&terminal.control, terminal.accepted, event.error.as_deref());
    }

    pub(super) fn insert_plugin_track(
        &mut self,
        selection: &hex_plugin_catalog::PluginSelection,
        before_track: Option<TrackId>,
        cx: &App,
    ) {
        use hex_plugin_catalog::PluginRole;
        use hex_project::TrackDeviceSlot;
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let gain = match self.settings.track_defaults.captured_gain() {
            Ok(gain) => gain,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        self.insert_plugin_effect(
            selection,
            PluginEffectDestination::NewTrack {
                before_track,
                slot: match selection.role {
                    PluginRole::Instrument => TrackDeviceSlot::Instrument,
                    PluginRole::Effect => TrackDeviceSlot::Effect,
                },
                gain,
                track_identity: state.project.next_identity(),
            },
            None,
            cx,
        );
    }

    pub(super) fn retire_plugin_effect(
        &mut self,
        control: &Arc<PluginEffectControl>,
        installed: bool,
        error: Option<&str>,
    ) -> bool {
        let settled = Self::settle_device_proposal(
            &mut self.device_panel.pending_plugin_effect,
            &mut self.device_panel.failed_plugin_proposal,
            control,
            installed,
            error,
        );
        if settled
            && self.deferred_toggle_play.is_some_and(|intent| {
                matches!(intent.target, TogglePlayTarget::DeviceProposal { .. })
            })
        {
            self.deferred_toggle_play = None;
        }
        settled
    }

    pub(super) fn settle_device_proposal(
        pending: &mut Option<PendingPluginEffect>,
        failed: &mut Option<FailedPluginProposal>,
        control: &Arc<PluginEffectControl>,
        installed: bool,
        error: Option<&str>,
    ) -> bool {
        if pending
            .as_ref()
            .is_none_or(|pending| !Arc::ptr_eq(&pending.control, control))
        {
            return false;
        }
        let Some(pending) = pending.take() else {
            return false;
        };
        if let Some(trace) = &pending.trace {
            trace.complete(installed);
        }
        if !installed && !pending.control.is_cancelled() {
            *failed = Some(FailedPluginProposal {
                proposal: pending.proposal.clone(),
                control: Arc::clone(control),
                error: error.unwrap_or("device load failed").into(),
            });
        }
        true
    }

    pub(super) fn insert_builtin_arrangement_device(
        &mut self,
        context: hex_engine::AudioSourceContext,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        target: ArrangementDeviceDestination,
        new_track_gain: Option<f32>,
    ) {
        self.enqueue_catalog_device(Request::DropBrowserDeviceOnArrangement {
            project_generation: context.project_generation,
            revision: context.editor_revision,
            class,
            type_id,
            destination: target,
            new_track_gain,
            control: None,
        });
    }

    /// Capture only catalog insertions. Moves, copies and preset replacement
    /// retain their original request and independently owned terminal.
    pub(super) fn enqueue_catalog_device(&mut self, mut request: Request) -> bool {
        use PluginEffectDestination as Destination;
        use hex_engine::NativeDeviceClass;
        use hex_project::TrackDeviceSlot;
        use hex_session::service::{
            EffectBusDestination, NativeDeviceDropSource, TrackCatalogPlacement,
        };

        let Some(state) = &self.state else {
            return self.enqueue_device_barrier(request);
        };
        let new_track = |before_track, class, gain: Option<f32>| Destination::NewTrack {
            before_track,
            slot: match class {
                NativeDeviceClass::Effect => TrackDeviceSlot::Effect,
                NativeDeviceClass::Instrument => TrackDeviceSlot::Instrument,
            },
            gain: gain.unwrap_or(1.0),
            track_identity: state.project.next_identity(),
        };
        let track_slot = |track, class, replace: Option<hex_project::DeviceId>| match class {
            NativeDeviceClass::Effect => Destination::Track(track),
            NativeDeviceClass::Instrument => replace
                .map_or(Destination::Instrument(track), |instrument| {
                    Destination::ReplaceInstrument { track, instrument }
                }),
        };
        if let Request::LoadTrackCatalogDevice {
            track,
            class,
            type_id,
            placement,
            control,
            ..
        } = &request
        {
            let (before, replace) = match *placement {
                TrackCatalogPlacement::Insert { before } => (before, None),
                TrackCatalogPlacement::ReplaceInstrument(device) => (None, Some(device)),
            };
            let (class, type_id, destination, control) = (
                *class,
                *type_id,
                track_slot(*track, *class, replace),
                Arc::clone(control),
            );
            return self.enqueue_builtin_proposal(
                request,
                control,
                class,
                type_id,
                destination,
                before,
            );
        }
        let (class, type_id, destination, before, slot_control) = match &mut request {
            Request::DropBrowserDeviceOnArrangement {
                class,
                type_id,
                destination,
                new_track_gain,
                control,
                ..
            } => {
                let position = match *destination {
                    ArrangementDeviceDestination::NewTrack { before_track } => {
                        new_track(before_track, *class, *new_track_gain)
                    }
                    ArrangementDeviceDestination::ExistingTrack { track, instrument } => {
                        track_slot(track, *class, instrument)
                    }
                };
                (*class, *type_id, position, None, control)
            }
            Request::InsertBrowserInstrumentTrack {
                class,
                type_id,
                new_track_gain,
                control,
                ..
            } => (
                *class,
                *type_id,
                new_track(None, *class, *new_track_gain),
                None,
                control,
            ),
            Request::InsertEffectBusDevice {
                class,
                type_id,
                destination,
                before,
                control,
                ..
            } => {
                let position = match *destination {
                    EffectBusDestination::Main => Destination::Main,
                    EffectBusDestination::Return(id) => Destination::Return(id),
                };
                (*class, *type_id, position, *before, control)
            }
            Request::DropNativeDevice {
                source: NativeDeviceDropSource::Browser { class, type_id },
                track,
                replace,
                before,
                control,
                ..
            } => (
                *class,
                *type_id,
                track_slot(*track, *class, *replace),
                *before,
                control,
            ),
            _ => return self.enqueue_device_barrier(request),
        };
        let control = Arc::new(PluginEffectControl::default());
        *slot_control = Some(Arc::clone(&control));
        self.enqueue_builtin_proposal(request, control, class, type_id, destination, before)
    }

    fn enqueue_builtin_proposal(
        &mut self,
        request: Request,
        control: Arc<PluginEffectControl>,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        destination: PluginEffectDestination,
        before: Option<hex_project::DeviceId>,
    ) -> bool {
        if self.device_panel.pending_plugin_effect.is_some() {
            self.error = Some("wait for the pending device insertion".into());
            return false;
        }
        let name = hex_engine::native_device_catalog_for_class(class)
            .ok()
            .and_then(|entries| entries.into_iter().find(|entry| entry.type_id == type_id))
            .map_or_else(
                || type_id.as_str().unwrap_or("device").to_owned(),
                |entry| entry.name,
            );
        if !self.enqueue_device_barrier(request) {
            return false;
        }
        let status = format!("loading device · {name}");
        self.device_panel.pending_plugin_effect = Some(PendingPluginEffect {
            control,
            trace: None,
            status,
            proposal: PluginProposal {
                destination,
                before,
                name,
                loading: "loading device",
                before_empty: self.next_empty_track_id,
            },
        });
        self.device_panel.failed_plugin_proposal = None;
        self.error = None;
        true
    }

    pub(super) fn insert_browser_plugin_effect(
        &mut self,
        selection: hex_plugin_catalog::PluginSelection,
        cx: &App,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let destination = match device_panel::selected_owner(self, state) {
            Some(device_panel::DevicePanelOwner::Track(id)) => PluginEffectDestination::Track(id),
            Some(device_panel::DevicePanelOwner::Return(id)) => PluginEffectDestination::Return(id),
            Some(device_panel::DevicePanelOwner::Main) => PluginEffectDestination::Main,
            None | Some(device_panel::DevicePanelOwner::Source(_)) => {
                self.error =
                    Some("select a track, return or main before inserting an effect".into());
                return;
            }
        };
        self.insert_plugin_effect(&selection, destination, None, cx);
    }

    pub(super) fn resolve_plugin_destination(
        project: &hex_project::Project,
        role: hex_plugin_catalog::PluginRole,
        destination: PluginEffectDestination,
        before: Option<hex_project::DeviceId>,
    ) -> Result<(PluginEffectDestination, Option<hex_project::DeviceId>), &'static str> {
        match (role, destination) {
            (hex_plugin_catalog::PluginRole::Instrument, PluginEffectDestination::Track(track)) => {
                let Some(target) = project.track(track) else {
                    return Err("select a track for the vst3 instrument");
                };
                if target.is_group() {
                    return Err("select an ordinary receiving track for the vst3 instrument");
                }
                if let Some(instrument) = target.instrument().map(hex_project::Device::id) {
                    if !target.receives_source_audio() {
                        return Err("select an ordinary receiving track for the vst3 instrument");
                    }
                    Ok((
                        PluginEffectDestination::ReplaceInstrument { track, instrument },
                        None,
                    ))
                } else {
                    Ok((PluginEffectDestination::Instrument(track), None))
                }
            }
            (
                hex_plugin_catalog::PluginRole::Instrument,
                PluginEffectDestination::ReplaceInstrument { .. }
                | PluginEffectDestination::NewTrack {
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    ..
                },
            ) => Ok((destination, None)),
            (hex_plugin_catalog::PluginRole::Instrument, _) => {
                Err("select a track for the vst3 instrument")
            }
            _ => Ok((destination, before)),
        }
    }

    pub(super) fn insert_plugin_effect(
        &mut self,
        selection: &hex_plugin_catalog::PluginSelection,
        destination: PluginEffectDestination,
        before: Option<hex_project::DeviceId>,
        cx: &App,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let (destination, before) = match Self::resolve_plugin_destination(
            &state.project,
            selection.role,
            destination,
            before,
        ) {
            Ok(resolved) => resolved,
            Err(message) => {
                self.error = Some(message.into());
                return;
            }
        };
        if self.device_panel.pending_plugin_effect.is_some() {
            self.error = Some("wait for the pending device insertion".into());
            return;
        }
        let control = Arc::new(PluginEffectControl::opening_editor());
        let request = PluginEffectInsertion {
            context: hex_engine::AudioSourceContext {
                project_generation: self.project_generation,
                editor_revision: state.revision,
            },
            selection: PluginEffectSelection {
                module: selection.module.clone(),
                executable: selection.executable_relative_path.clone(),
                cid: selection.cid,
                executable_sha256: selection.executable_sha256.clone(),
                info_sha256: selection.info_sha256.clone(),
            },
            destination,
            before,
            track_device_name: matches!(destination, PluginEffectDestination::NewTrack { .. })
                .then(|| selection.class_name.clone()),
            control: Arc::clone(&control),
        };
        let trace = crate::diagnostics::PluginLoad::new(
            cx,
            &selection.class_name,
            request.context,
            destination,
            state.project.sample_rate(),
        );
        if self.enqueue_device_barrier(Request::InsertPluginEffect(request)) {
            if let Some(trace) = &trace {
                trace.submitted();
            }
            let loading = if matches!(
                destination,
                PluginEffectDestination::ReplaceTrackEffect { .. }
                    | PluginEffectDestination::ReplaceMainEffect { .. }
                    | PluginEffectDestination::ReplaceReturnEffect { .. }
            ) {
                "loading replacement"
            } else {
                "loading vst3"
            };
            let status = format!("{loading} · {}", selection.class_name);
            self.status.clone_from(&status);
            self.device_panel.pending_plugin_effect = Some(PendingPluginEffect {
                control,
                trace: trace.map(Rc::new),
                status,
                proposal: PluginProposal {
                    destination,
                    before,
                    name: selection.class_name.clone(),
                    loading,
                    before_empty: self.next_empty_track_id,
                },
            });
            self.device_panel.failed_plugin_proposal = None;
            self.error = None;
        }
    }
}

// Catalog transport receipts below are synthetic fixture metadata only. Actual
// source revalidation and all native preparation/ACK remain with the worker.
#[cfg(feature = "ui-test-support")]
pub(crate) fn fixture_catalog(
    root: &std::path::Path,
    selected: &PluginEffectSelection,
    subcategories: &str,
    factory_flags: u32,
) -> Result<(), String> {
    selected.validate()?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let temporary = std::env::temp_dir()
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !root.starts_with(&temporary) {
        return Err("plugin fixture requires an owned temporary root".into());
    }
    let report = serde_json::from_value(serde_json::json!({
        "profile":"explicit-user-trusted-macos-factory-only","request":1,"generation":1,
        "receipt_sha256":"a".repeat(64),"helper_sha256":"b".repeat(64),
        "bundle":{"supplied_path":selected.module,"canonical_path":selected.module.canonicalize().map_err(|error| error.to_string())?,"info_sha256":selected.info_sha256},
        "executable":{"relative_path":selected.executable,"sha256":selected.executable_sha256,"image":"thin_arm64"},
        "bundle_identifier":"org.hex.original","interruption":null,"error":null,"stdout":"","stderr":"",
        "cleanup":{"direct_reaped":true,"direct_exit_code":0,"direct_exit_signal":null,"custody_lost":false,"cleanup_unconfirmed":false,
          "exit_observed_before_signals":true,"original_group_term":{"status":"absent"},"original_group_kill":{"status":"absent"},"direct_kill":null,"descendants":"unknown","pending":false},
        "discovery":{"request":1,"sdk":"3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96","public_sdk":"586dc5e6c8012c3e4b01c79389375cbe96bdb1da","platform":"darwin-arm64",
          "factory_flags":factory_flags,"vendor":"original fixture","url":"","email":"","classes":[{"cid":selected.cid,"cardinality":1,"category":"Audio Module Class","name":"original processor","class_flags":0,"subcategories":subcategories,"class_vendor":"original fixture","class_version":"1","class_sdk_version":"3.8"}]}
    })).map_err(|error| error.to_string())?;
    hex_plugin_catalog::storage::update(&root.join("plugin-catalog.json"), |catalog| {
        catalog.record(selected.module.clone(), Ok((report, "e".repeat(40))))
    })
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_plugin_catalog::PluginRole;
    use hex_project::{Command, Created, Editor, Project, TrackSelectionIds};

    #[test]
    fn resolve_plugin_destination_replaces_occupied_instruments_and_respects_track_ownership() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(empty_track) = editor
            .execute(Command::AddMidiTrack {
                name: "empty".into(),
            })
            .unwrap()
        else {
            panic!("empty track");
        };
        let Created::Track(native_track) = editor
            .execute(Command::AddMidiTrack {
                name: "native".into(),
            })
            .unwrap()
        else {
            panic!("native track");
        };
        let sine = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine"),
        )
        .unwrap();
        let Created::Device(native_instrument) = editor
            .execute(Command::InsertInstrument {
                track: native_track,
                state: sine,
            })
            .unwrap()
        else {
            panic!("native instrument");
        };
        let source = editor
            .project()
            .track(native_track)
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
        let Created::Track(vst3_track) = editor
            .execute(Command::AddMidiTrack {
                name: "vst3".into(),
            })
            .unwrap()
        else {
            panic!("vst3 track");
        };
        let vst3_state = hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::VST3_DEVICE_TYPE_ID,
            1,
            vec![1],
        )
        .unwrap();
        let Created::Device(vst3_instrument) = editor
            .execute(Command::InsertInstrument {
                track: vst3_track,
                state: vst3_state,
            })
            .unwrap()
        else {
            panic!("vst3 instrument");
        };
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[empty_track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group");
        };
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return");
        };

        let project = editor.project();
        // Empty track -> Instrument(track).
        assert_eq!(
            EditorView::resolve_plugin_destination(
                project,
                PluginRole::Instrument,
                PluginEffectDestination::Track(empty_track),
                None,
            ),
            Ok((PluginEffectDestination::Instrument(empty_track), None))
        );
        // Occupied track (native or VST3) -> ReplaceInstrument { track, instrument }.
        assert_eq!(
            EditorView::resolve_plugin_destination(
                project,
                PluginRole::Instrument,
                PluginEffectDestination::Track(native_track),
                None,
            ),
            Ok((
                PluginEffectDestination::ReplaceInstrument {
                    track: native_track,
                    instrument: native_instrument,
                },
                None,
            ))
        );
        assert_eq!(
            EditorView::resolve_plugin_destination(
                project,
                PluginRole::Instrument,
                PluginEffectDestination::Track(vst3_track),
                None,
            ),
            Ok((
                PluginEffectDestination::ReplaceInstrument {
                    track: vst3_track,
                    instrument: vst3_instrument,
                },
                None,
            ))
        );
        // Group track, non-receiving contributor track, Return, and Main refuse VST3 instrument.
        assert!(
            EditorView::resolve_plugin_destination(
                project,
                PluginRole::Instrument,
                PluginEffectDestination::Track(group),
                None,
            )
            .is_err()
        );
        assert!(
            EditorView::resolve_plugin_destination(
                project,
                PluginRole::Instrument,
                PluginEffectDestination::Track(contributor),
                None,
            )
            .is_err()
        );
        assert!(
            EditorView::resolve_plugin_destination(
                project,
                PluginRole::Instrument,
                PluginEffectDestination::Return(bus),
                None,
            )
            .is_err()
        );
        assert!(
            EditorView::resolve_plugin_destination(
                project,
                PluginRole::Instrument,
                PluginEffectDestination::Main,
                None,
            )
            .is_err()
        );
        // VST3 effect keeps Track(track) and preserves before == Some(instrument).
        assert_eq!(
            EditorView::resolve_plugin_destination(
                project,
                PluginRole::Effect,
                PluginEffectDestination::Track(native_track),
                Some(native_instrument),
            ),
            Ok((
                PluginEffectDestination::Track(native_track),
                Some(native_instrument),
            ))
        );
    }
}
