//! Captured final sampler controls. Widgets own local previews; Session owns
//! authored state and the existing native stopped-edit transaction owns audio.
use super::*;
use gpui::{Entity, Focusable, div, prelude::*};
use hex_engine::SamplerParameter;
use hex_widgets::{
    NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlEvent,
    NumericControlKind, NumericEventPhase, NumericScale, NumericSpec,
};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    generation: u64,
    revision: u64,
    track: TrackId,
    device: hex_project::DeviceId,
    parameter: SamplerParameter,
}
struct Control {
    target: Target,
    gesture: Option<Target>,
    focus: gpui::FocusHandle,
    entity: Entity<NumericControl>,
    _subscription: gpui::Subscription,
}
struct Pending {
    target: Target,
    id: u64,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
pub(super) struct State {
    serial: u64,
    // The closed parameter enum bounds this map to seven entries.
    controls: BTreeMap<SamplerParameter, Control>,
    pending: Option<Pending>,
    // Retired controls from this panel cycle only; drained at every render.
    retired_focus: Vec<gpui::FocusHandle>,
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(p) = &self.pending {
            p.cancel.store(true, Ordering::Release);
        }
    }
}
impl Target {
    fn current(self, view: &EditorView) -> bool {
        view.project_generation == self.generation
            && view.selected_track == Some(self.track)
            && view.device_panel.owner == Some(device_panel::DevicePanelOwner::Track(self.track))
            && view.state.as_ref().is_some_and(|s| {
                s.revision == self.revision
                    && s.project
                        .track(self.track)
                        .and_then(|t| t.instrument())
                        .is_some_and(|d| d.id() == self.device && !d.bypassed())
            })
    }
}
impl EditorView {
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
            .is_some_and(|p| !p.target.current(self))
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
        if !target.current(self) || self.edit_pending() || !self.first_sampler_current() {
            self.error = Some("sampler edit target changed or audio is not ready".into());
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
            track: target.track,
            device: target.device,
            parameter: target.parameter,
            value,
            cancel: Arc::clone(&cancel),
        };
        if self.enqueue(Request::SetSamplerParameter(request)) {
            self.sampler_parameter.pending = Some(Pending { target, id, cancel });
        }
    }
    pub(super) fn sampler_parameter_control(
        &mut self,
        track: TrackId,
        device: &hex_project::Device,
        parameter: SamplerParameter,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let value = match hex_engine::sampler_parameter_value_on_control(parameter, device.state())
        {
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
                .filter(|asset| asset.original_source().is_some())
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
        let target = Target {
            generation: self.project_generation,
            revision: state.revision,
            track,
            device: device.id(),
            parameter,
        };
        if self
            .sampler_parameter
            .controls
            .get(&parameter)
            .is_none_or(|c| {
                c.target.generation != target.generation
                    || c.target.track != target.track
                    || c.target.device != target.device
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
                if parameter.is_integer() { 0 } else { 2 },
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
                    device_panel::control_style(),
                )
                .map(|config| config.with_appearance(NumericControlAppearance::InlineValue))
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
                    gesture: None,
                    focus,
                    entity,
                    _subscription: subscription,
                },
            );
        }
        if let Some(control) = self.sampler_parameter.controls.get_mut(&parameter) {
            control.target = target;
        }
        let enabled = !self.edit_pending()
            && self.first_sampler_current()
            && self
                .audio
                .as_ref()
                .and_then(|a| a.control.status())
                .is_some_and(|s| !s.playing.load(Ordering::Acquire));
        let Some(control) = self.sampler_parameter.controls.get(&parameter) else {
            return div().into_any_element();
        };
        let entity = control.entity.clone();
        entity.update(cx, |widget, cx| {
            let _ = widget.set_external_value(value, cx);
            widget.set_enabled(enabled, cx);
        });
        div()
            .id((
                match parameter {
                    SamplerParameter::Gain => "sampler-gain-control",
                    SamplerParameter::Transpose => "sampler-pitch-control",
                    SamplerParameter::StartFrame => "sampler-start-control",
                    SamplerParameter::EndFrame => "sampler-end-control",
                    SamplerParameter::LoopEnabled => "sampler-loop-control",
                    SamplerParameter::LoopStartFrame => "sampler-loop-start-control",
                    SamplerParameter::LoopEndFrame => "sampler-loop-end-control",
                },
                device.id().value(),
            ))
            .debug_selector(move || format!("sampler-{}-control", parameter.key()))
            .w_full()
            .min_w_0()
            .h(gpui::px(26.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(gpui::px(4.0))
            .child(theme::text(parameter.label()))
            .child(
                div()
                    .w(gpui::px(92.0))
                    .h(gpui::px(18.0))
                    .child(entity.clone()),
            )
            .into_any_element()
    }
}
