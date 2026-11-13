//! Captured final sampler controls. Widgets own local previews; Session owns
//! authored state and the ordinary prepared-device transaction owns audio.
use super::*;
use gpui::{Entity, Focusable, div, prelude::*};
use hex_engine::SamplerParameter;
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

/// Inline value width of each sampler control. Twelve classic-mode controls
/// (including the loop crossfade) plus 2 px gaps must fit one row of the
/// 1024 px device body: 12 * 80 + 11 * 2 = 982 px.
const SAMPLER_INLINE_VALUE_WIDTH: f32 = 80.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Target {
    generation: u64,
    revision: u64,
    owner: hex_session::service::SamplerParameterOwner,
    device: hex_project::DeviceId,
    parameter: SamplerParameter,
}
struct Control {
    target: Target,
    bounds: (f64, f64),
    gesture: Option<Target>,
    focus: gpui::FocusHandle,
    entity: Entity<NumericControl>,
    edit_state: hex_widgets::NumericEditState,
    _subscription: gpui::Subscription,
}
struct Pending {
    target: PendingTarget,
    id: u64,
    cancel: Arc<AtomicBool>,
    value: Option<f64>,
}
enum PendingTarget {
    Parameter(Target),
    Replacement {
        generation: u64,
        revision: u64,
        track: TrackId,
        device: hex_project::DeviceId,
    },
}
impl PendingTarget {
    fn current(&self, view: &EditorView) -> bool {
        match self {
            Self::Parameter(target) => target.current(view),
            Self::Replacement {
                generation,
                revision,
                track,
                device,
            } => {
                view.project_generation == *generation
                    && view.state.as_ref().is_some_and(|state| {
                        state.revision == *revision
                            && state
                                .project
                                .track(*track)
                                .and_then(|track| track.instrument())
                                .is_some_and(|entry| entry.id() == *device)
                    })
            }
        }
    }
}
#[derive(Default)]
pub(super) struct State {
    serial: u64,
    // The closed parameter enum bounds this map to eleven entries.
    controls: BTreeMap<SamplerParameter, Control>,
    pending: Option<Pending>,
    // Retired controls from this panel cycle only; drained at every render.
    retired_focus: Vec<gpui::FocusHandle>,
}
impl State {
    pub(super) fn numeric_draft_active(&self) -> bool {
        self.controls
            .values()
            .any(|control| control.edit_state.is_editing() || control.gesture.is_some())
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(p) = &self.pending {
            p.cancel.store(true, Ordering::Release);
        }
    }
}
impl Target {
    pub(super) fn new(
        generation: u64,
        revision: u64,
        owner: hex_session::service::SamplerParameterOwner,
        device: hex_project::DeviceId,
        parameter: SamplerParameter,
    ) -> Self {
        Self {
            generation,
            revision,
            owner,
            device,
            parameter,
        }
    }

    fn device(self, view: &EditorView) -> Option<&hex_project::Device> {
        let project = &view.state.as_ref()?.project;
        match self.owner {
            hex_session::service::SamplerParameterOwner::Track(track) => {
                project.track(track)?.instrument()
            }
            hex_session::service::SamplerParameterOwner::Source(source) => {
                Some(project.native_source(source)?.instrument())
            }
        }
        .filter(|device| device.id() == self.device)
    }

    pub(super) fn current(self, view: &EditorView) -> bool {
        if view.project_generation != self.generation
            || view
                .state
                .as_ref()
                .is_none_or(|state| state.revision != self.revision)
        {
            return false;
        }
        let owner_current = match self.owner {
            hex_session::service::SamplerParameterOwner::Track(track) => {
                view.selected_track == Some(track)
                    && view.device_panel.owner == Some(device_panel::DevicePanelOwner::Track(track))
            }
            hex_session::service::SamplerParameterOwner::Source(source) => {
                device_panel::source_owner_current(view, source)
            }
        };
        owner_current
            && self.device(view).is_some_and(|device| {
                (matches!(
                    self.owner,
                    hex_session::service::SamplerParameterOwner::Track(_)
                ) || device.state().type_id().as_str() == Ok("hex.instrument.sampler"))
                    && (!self.parameter.is_source_frame()
                        || view.sampler_source_target_current(device))
            })
    }
}

impl EditorView {
    /// Geometry owners capture this existing adapter's revision-bound target;
    /// they never publish an independent sampler edit or receipt.
    pub(super) fn sampler_boundary_target(
        &mut self,
        owner: hex_session::service::SamplerParameterOwner,
        device: hex_project::DeviceId,
        parameter: SamplerParameter,
    ) -> Option<Target> {
        if !matches!(
            parameter,
            SamplerParameter::StartFrame | SamplerParameter::EndFrame
        ) || self.edit_pending()
        {
            self.error =
                Some("sampler boundary is unavailable while another edit is pending".into());
            return None;
        }
        let target = Target::new(
            self.project_generation,
            self.state.as_ref()?.revision,
            owner,
            device,
            parameter,
        );
        target.current(self).then_some(target)
    }

    pub(super) fn validate_sampler_boundary(
        &self,
        target: Target,
        frame: u64,
    ) -> Result<(), hex_engine::Invalid> {
        if !target.current(self) || frame > target.parameter.bounds().1 as u64 {
            return Err(hex_engine::Invalid(
                "sampler boundary target changed or frame exceeds supported limits",
            ));
        }
        let device = target.device(self).ok_or(hex_engine::Invalid(
            "sampler boundary target is unavailable",
        ))?;
        let frames = device
            .state()
            .asset_references()
            .first()
            .and_then(|reference| {
                self.state
                    .as_ref()?
                    .project
                    .assets()
                    .find(|a| a.id().value() == reference.asset.value())
            })
            .map(|asset| asset.source_frames())
            .ok_or(hex_engine::Invalid(
                "sampler original source extent is unavailable",
            ))?;
        if frame > frames {
            return Err(hex_engine::Invalid(
                "sampler boundary exceeds original source",
            ));
        }
        hex_engine::patch_sampler_parameter_on_control(
            target.parameter,
            device.state(),
            frame as f64,
        )
        .map(|_| ())
    }

    pub(super) fn submit_sampler_boundary(&mut self, target: Target, frame: u64) {
        if let Err(error) = self.validate_sampler_boundary(target, frame) {
            self.error = Some(error.to_string().into());
            return;
        }
        let current = target.device(self).and_then(|d| {
            hex_engine::sampler_parameter_value_on_control(target.parameter, d.state()).ok()
        });
        if current != Some(frame as f64) {
            self.submit_sampler_parameter(target, frame as f64);
        }
    }

    /// A source boundary draft and its accepted keyed request share one display
    /// value with numeric controls. Only the exact terminal retires the latter.
    pub(super) fn sampler_display_value(
        &self,
        owner: hex_session::service::SamplerParameterOwner,
        device: &hex_project::Device,
        parameter: SamplerParameter,
        cx: &gpui::App,
    ) -> Result<f64, hex_engine::Invalid> {
        if parameter.is_source_frame()
            && let Some(value) =
                super::device_panel::sampler_boundary_preview(self, owner, device.id(), parameter)
        {
            return Ok(value as f64);
        }
        if let Some(pending) = self.sampler_parameter.pending.as_ref()
            && let PendingTarget::Parameter(target) = pending.target
            && target.owner == owner
            && target.device == device.id()
            && target.parameter == parameter
            && target.current(self)
            && let Some(value) = pending.value
        {
            return Ok(value);
        }
        if let Some(control) = self.sampler_parameter.controls.get(&parameter)
            && control.target.owner == owner
            && control.target.device == device.id()
            && control.target.current(self)
            && control.entity.read(cx).is_interacting()
        {
            return Ok(control.entity.read(cx).value());
        }
        hex_engine::sampler_parameter_value_on_control(parameter, device.state())
    }
    fn sampler_source_target_current(&self, device: &hex_project::Device) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(reference) = device
            .state()
            .asset_references()
            .first()
            .filter(|_| device.state().asset_references().len() == 1)
        else {
            return false;
        };
        state
            .project
            .assets()
            .find(|a| a.id().value() == reference.asset.value())
            .is_some_and(|asset| self.sampler_source_overview(asset).is_some())
    }
    /// Same committed view, source namespace, original coordinates and optional
    /// saved content facts. Fresh import facts come from the staged native bank.
    pub(super) fn sampler_source_overview(
        &self,
        asset: &hex_project::Asset,
    ) -> Option<&hex_session::OriginalSourceOverview> {
        let state = self.state.as_ref()?;
        if !state
            .source_overviews
            .matches_context(hex_engine::AudioSourceContext {
                project_generation: self.project_generation,
                editor_revision: state.revision,
            })
        {
            return None;
        }
        state.source_overviews.get(asset.id()).filter(|overview| {
            overview.source_frames() == asset.source_frames()
                && overview.sample_rate() == asset.source_sample_rate()
                && asset
                    .original_source()
                    .is_none_or(|facts| facts == overview.content())
        })
    }
    pub(super) fn sampler_parameter_pending(&self) -> bool {
        self.sampler_parameter.pending.is_some()
    }
    pub(super) fn retire_sampler_parameter(&mut self) {
        if let Some(p) = self.sampler_parameter.pending.take() {
            p.cancel.store(true, Ordering::Release);
        }
        for control in std::mem::take(&mut self.sampler_parameter.controls).into_values() {
            self.sampler_parameter.retired_focus.push(control.focus);
        }
    }
    pub(super) fn restore_sampler_parameter_focus(&mut self, window: &mut gpui::Window) {
        if self
            .sampler_parameter
            .retired_focus
            .drain(..)
            .any(|focus| focus.is_focused(window))
        {
            self.focus.focus(window);
        }
    }
    pub(super) fn tick_sampler_parameter(&mut self) -> bool {
        if self
            .sampler_parameter
            .pending
            .as_ref()
            .is_some_and(|p| p.cancel.load(Ordering::Acquire) || !p.target.current(self))
        {
            self.retire_sampler_parameter();
            return true;
        }
        false
    }
    pub(super) fn finish_sampler_parameter(&mut self, complete: Option<u64>) {
        if self
            .sampler_parameter
            .pending
            .as_ref()
            .is_some_and(|p| Some(p.id) == complete)
        {
            self.sampler_parameter.pending = None;
        }
    }
    fn submit_sampler_parameter(&mut self, target: Target, value: f64) {
        if !target.current(self) || self.edit_pending() {
            self.error = Some("sampler edit target changed or another edit is pending".into());
            return;
        }
        let Some(id) = self.sampler_parameter.serial.checked_add(1) else {
            self.error = Some("sampler edit request identity exhausted".into());
            return;
        };
        self.sampler_parameter.serial = id;
        let cancel = Arc::new(AtomicBool::new(false));
        let request = hex_session::service::SamplerParameterEdit {
            request_id: id,
            project_generation: target.generation,
            revision: target.revision,
            owner: target.owner,
            device: target.device,
            parameter: target.parameter,
            value,
            cancel: Arc::clone(&cancel),
        };
        if self.enqueue(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(request),
        )) {
            self.sampler_parameter.pending = Some(Pending {
                target: PendingTarget::Parameter(target),
                id,
                cancel,
                value: Some(value),
            });
        }
    }
    /// A local numeric draft is not canonical history. Cancel it and consume the
    /// shortcut; a subsequent press may move the global history edge.
    pub(super) fn cancel_sampler_drafts(&mut self, cx: &mut Context<Self>) -> bool {
        let boundary_cancelled = super::device_panel::cancel_sampler_boundary(self);
        let controls = self
            .sampler_parameter
            .controls
            .values_mut()
            .filter_map(|control| {
                if control.entity.read(cx).is_interacting() {
                    control.gesture = None;
                    Some(control.entity.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let cancelled = boundary_cancelled || !controls.is_empty();
        for entity in controls {
            entity.update(cx, |widget, cx| widget.cancel(cx));
        }
        cancelled
    }
    pub(super) fn sampler_replacement_pending_id(&self) -> Option<u64> {
        self.sampler_parameter
            .pending
            .as_ref()
            .filter(|p| matches!(p.target, PendingTarget::Replacement { .. }))
            .map(|p| p.id)
    }
    pub(super) fn cancel_sampler_replacement(&mut self, id: Option<u64>) {
        if let Some(pending) = self.sampler_parameter.pending.as_ref().filter(|p| {
            matches!(p.target, PendingTarget::Replacement { .. }) && id.is_none_or(|id| id == p.id)
        }) {
            pending.cancel.store(true, Ordering::Release);
            self.status = "cancelling sample replacement".into();
        }
    }
    pub(super) fn submit_sampler_replacement(&mut self, target: super::sampler_load::Target) {
        let super::sampler_load::Target {
            generation,
            revision,
            track,
            path,
            slot,
        } = target;
        let hex_session::service::SamplerLoadSlot::Existing {
            device,
            expected_state,
        } = slot
        else {
            self.error = Some("sampler replacement target is unavailable".into());
            return;
        };
        if self.edit_pending() {
            self.error = Some("wait for the pending edit before replacing its sample".into());
            return;
        }
        let Some(id) = self.sampler_parameter.serial.checked_add(1) else {
            self.error = Some("sampler edit request identity exhausted".into());
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let request = hex_session::service::SamplerReplacementEdit {
            request_id: id,
            project_generation: generation,
            revision,
            track,
            device,
            expected_state,
            path,
            cancel: Arc::clone(&cancel),
        };
        self.sampler_parameter.serial = id;
        if self.enqueue(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Replacement(request),
        )) {
            self.sampler_parameter.pending = Some(Pending {
                target: PendingTarget::Replacement {
                    generation,
                    revision,
                    track,
                    device,
                },
                id,
                cancel,
                value: None,
            });
        }
    }
    pub(super) fn sampler_parameter_control(
        &mut self,
        owner: hex_session::service::SamplerParameterOwner,
        device: &hex_project::Device,
        parameter: SamplerParameter,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let value = match self.sampler_display_value(owner, device, parameter, cx) {
            Ok(value) => value,
            Err(error) => {
                return div()
                    .child(theme::text(error.to_string()))
                    .into_any_element();
            }
        };
        let Some(state) = self.state.as_ref() else {
            return div().into_any_element();
        };
        // Original-file frames only: project-rate playback frames are a
        // different coordinate system. This hint never grants admission; the
        // actual retained PCM preparer validates the candidate before commit.
        let (min, mut max) = parameter.bounds();
        if parameter.is_source_frame() {
            let frames = device
                .state()
                .asset_references()
                .first()
                .and_then(|reference| {
                    state
                        .project
                        .assets()
                        .find(|asset| asset.id().value() == reference.asset.value())
                })
                .filter(|asset| self.sampler_source_overview(asset).is_some())
                .map(|asset| asset.source_frames());
            let Some(frames) =
                frames.filter(|n| *n > 0 && *n <= SamplerParameter::EndFrame.bounds().1 as u64)
            else {
                return div()
                    .child(theme::text("source frame bounds unavailable"))
                    .into_any_element();
            };
            max = match parameter {
                SamplerParameter::StartFrame | SamplerParameter::LoopStartFrame => {
                    (frames - 1) as f64
                }
                _ => frames as f64,
            };
            if min == max {
                // A one-frame source has no editable endpoints.
                return div()
                    .w_full()
                    .min_w_0()
                    .h(gpui::px(26.0))
                    .child(theme::text(format!(
                        "{} {value:.0} frames",
                        parameter.label()
                    )))
                    .into_any_element();
            }
        }
        let target = Target::new(
            self.project_generation,
            state.revision,
            owner,
            device.id(),
            parameter,
        );
        if self
            .sampler_parameter
            .controls
            .get(&parameter)
            .is_none_or(|c| {
                c.target.generation != target.generation
                    || c.target.owner != target.owner
                    || c.target.device != target.device
                    || c.bounds != (min, max)
            })
        {
            if let Some(control) = self.sampler_parameter.controls.remove(&parameter) {
                self.sampler_parameter.retired_focus.push(control.focus);
            }
            let config = NumericSpec::new(
                min,
                max,
                if matches!(
                    parameter,
                    SamplerParameter::EndFrame | SamplerParameter::LoopEndFrame
                ) {
                    max
                } else {
                    parameter.default_value()
                },
                if min < 0.0 {
                    NumericScale::Bipolar
                } else {
                    NumericScale::Linear
                },
                if parameter.is_integer() {
                    0
                } else if parameter.is_envelope() {
                    6
                } else {
                    2
                },
                parameter.unit().map(Arc::<str>::from),
            )
            .and_then(|spec| {
                if parameter.is_integer() {
                    spec.with_integer_values()
                } else if parameter == SamplerParameter::Transpose {
                    spec.with_keyboard_steps(1.0, 0.1)
                } else {
                    Ok(spec)
                }
            })
            .map(|spec| {
                if parameter == SamplerParameter::LoopEnabled {
                    spec.with_codec(
                        |value| theme::text(if value == 1.0 { "on" } else { "off" }).to_string(),
                        |text| {
                            let text = text.trim();
                            if text.eq_ignore_ascii_case("on") || text == "1" {
                                Some(1.0)
                            } else if text.eq_ignore_ascii_case("off") || text == "0" {
                                Some(0.0)
                            } else {
                                None
                            }
                        },
                    )
                } else {
                    spec
                }
            })
            .and_then(|spec| {
                NumericControlConfig::new(
                    format!("sampler-{}-{}", parameter.key(), device.id().value()),
                    theme::text(parameter.label()),
                    NumericControlKind::HorizontalSlider,
                    spec,
                    value,
                    crate::theme::control_style(),
                )
                .and_then(|config| {
                    let config = config.with_presentation(NumericControlPresentation::InlineValue);
                    if parameter.is_envelope() {
                        config.with_value_drag_steps(1.0, 0.1)
                    } else {
                        Ok(config)
                    }
                })
            });
            let config = match config {
                Ok(config) => config,
                Err(_) => {
                    return div()
                        .child(theme::text("sampler control unavailable"))
                        .into_any_element();
                }
            };
            let entity = cx.new(|cx| NumericControl::from_config(cx, config));
            let edit_state = entity.read(cx).edit_state();
            let focus = entity.read(cx).focus_handle(cx);
            let subscription = cx.subscribe(
                &entity,
                move |view, emitter, event: &NumericControlEvent, cx| {
                    let Some(control) = view
                        .sampler_parameter
                        .controls
                        .get_mut(&parameter)
                        .filter(|c| c.entity.entity_id() == emitter.entity_id())
                    else {
                        return;
                    };
                    let captured = match event.phase {
                        NumericEventPhase::Begin => {
                            control.gesture = Some(control.target);
                            None
                        }
                        NumericEventPhase::Cancel => {
                            control.gesture = None;
                            None
                        }
                        NumericEventPhase::Commit => {
                            control.gesture.take().or(Some(control.target))
                        }
                        NumericEventPhase::Change => None,
                    };
                    if let Some(captured) = captured {
                        view.submit_sampler_parameter(captured, event.value);
                    }
                    cx.notify();
                },
            );
            self.sampler_parameter.controls.insert(
                parameter,
                Control {
                    target,
                    bounds: (min, max),
                    gesture: None,
                    focus,
                    entity,
                    edit_state,
                    _subscription: subscription,
                },
            );
        }
        if let Some(control) = self.sampler_parameter.controls.get_mut(&parameter) {
            control.target = target;
        }
        let active = hex_engine::sampler_parameter_active_on_control(parameter, device.state())
            .unwrap_or(false);
        let busy = self.edit_pending()
            || self.gain_lane.draft.is_some()
            || self.gain_lane.numeric_draft_active();
        let availability = hex_widgets::ControlAvailability::for_edit(active, busy);
        let Some(control) = self.sampler_parameter.controls.get(&parameter) else {
            return div().into_any_element();
        };
        let entity = control.entity.clone();
        entity.update(cx, |widget, cx| {
            let _ = widget.set_external_value(value, cx);
            let _ = widget.set_inline_value_width(SAMPLER_INLINE_VALUE_WIDTH, cx);
            widget.set_availability(availability, cx);
        });
        div()
            .id((
                match parameter {
                    SamplerParameter::AttackMs => "sampler-attack-control",
                    SamplerParameter::DecayMs => "sampler-decay-control",
                    SamplerParameter::SustainPercent => "sampler-sustain-control",
                    SamplerParameter::ReleaseMs => "sampler-release-control",
                    SamplerParameter::Gain => "sampler-gain-control",
                    SamplerParameter::Transpose => "sampler-pitch-control",
                    SamplerParameter::StartFrame => "sampler-start-control",
                    SamplerParameter::EndFrame => "sampler-end-control",
                    SamplerParameter::LoopEnabled => "sampler-loop-control",
                    SamplerParameter::LoopStartFrame => "sampler-loop-start-control",
                    SamplerParameter::LoopEndFrame => "sampler-loop-end-control",
                    SamplerParameter::LoopCrossfadeMs => "sampler-loop-crossfade-control",
                },
                device.id().value(),
            ))
            .debug_selector(move || format!("sampler-{}-control", parameter.key()))
            .w_full()
            .min_w_0()
            .h(gpui::px(28.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_start()
            .child(
                div()
                    .h(gpui::px(12.0))
                    .text_size(gpui::px(10.0))
                    .line_height(gpui::px(12.0))
                    .child(theme::text(if active {
                        parameter.label()
                    } else {
                        "release*"
                    })),
            )
            .child(div().w_full().h(gpui::px(16.0)).child(entity.clone()))
            .into_any_element()
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) mod tests;
