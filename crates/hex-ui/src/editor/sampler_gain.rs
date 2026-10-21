//! One captured final gain gesture. The widget owns only its local preview;
//! Session owns authored state and the native worker owns safe output replacement.
use super::*;
use gpui::{Entity, Focusable, div, prelude::*};
use hex_widgets::{
    NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlEvent,
    NumericControlKind, NumericEventPhase, NumericScale, NumericSpec,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    generation: u64,
    revision: u64,
    track: TrackId,
    device: hex_project::DeviceId,
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
    control: Option<Control>,
    pending: Option<Pending>,
    retired_focus: Option<gpui::FocusHandle>,
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
    pub(super) fn sampler_gain_pending(&self) -> bool {
        self.sampler_gain.pending.is_some()
    }
    pub(super) fn retire_sampler_gain(&mut self) {
        if let Some(p) = self.sampler_gain.pending.take() {
            p.cancel.store(true, Ordering::Release);
        }
        if let Some(control) = self.sampler_gain.control.take() {
            self.sampler_gain.retired_focus = Some(control.focus);
        }
    }
    pub(super) fn restore_sampler_gain_focus(&mut self, window: &mut gpui::Window) {
        if self
            .sampler_gain
            .retired_focus
            .take()
            .is_some_and(|focus| focus.is_focused(window))
        {
            self.focus.focus(window);
        }
    }
    pub(super) fn tick_sampler_gain(&mut self) -> bool {
        if self
            .sampler_gain
            .pending
            .as_ref()
            .is_some_and(|p| !p.target.current(self))
        {
            self.retire_sampler_gain();
            return true;
        }
        false
    }
    pub(super) fn finish_sampler_gain(&mut self, complete: Option<u64>) {
        if self
            .sampler_gain
            .pending
            .as_ref()
            .is_some_and(|p| Some(p.id) == complete)
        {
            self.sampler_gain.pending = None;
        }
    }
    fn submit_sampler_gain(&mut self, target: Target, gain: f64) {
        if !target.current(self) || self.edit_pending() || !self.first_sampler_current() {
            self.error = Some("sampler gain target changed or audio is not ready".into());
            return;
        }
        let Some(id) = self.sampler_gain.serial.checked_add(1) else {
            self.error = Some("sampler gain request identity exhausted".into());
            return;
        };
        self.sampler_gain.serial = id;
        let cancel = Arc::new(AtomicBool::new(false));
        let request = hex_session::service::SamplerGainEdit {
            request_id: id,
            project_generation: target.generation,
            revision: target.revision,
            track: target.track,
            device: target.device,
            gain,
            cancel: Arc::clone(&cancel),
        };
        if self.enqueue(Request::SetSamplerGain(request)) {
            self.sampler_gain.pending = Some(Pending { target, id, cancel });
        }
    }
    pub(super) fn sampler_gain_control(
        &mut self,
        track: TrackId,
        device: &hex_project::Device,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let gain = match hex_engine::sampler_gain_on_control(device.state()) {
            Ok(gain) => gain,
            Err(error) => {
                return div()
                    .child(theme::text(error.to_string()))
                    .into_any_element();
            }
        };
        let Some(state) = self.state.as_ref() else {
            return div().into_any_element();
        };
        let target = Target {
            generation: self.project_generation,
            revision: state.revision,
            track,
            device: device.id(),
        };
        if self.sampler_gain.control.as_ref().is_none_or(|c| {
            c.target.generation != target.generation
                || c.target.track != target.track
                || c.target.device != target.device
        }) {
            self.sampler_gain.control = None;
            let config = NumericSpec::new(0.0, 4.0, 1.0, NumericScale::Linear, 2, None::<Arc<str>>)
                .and_then(|spec| {
                    NumericControlConfig::new(
                        format!("sampler-gain-{}", device.id().value()),
                        theme::text("gain"),
                        NumericControlKind::HorizontalSlider,
                        spec,
                        gain,
                        device_panel::control_style(),
                    )
                    .map(|config| config.with_appearance(NumericControlAppearance::InlineValue))
                });
            let config = match config {
                Ok(config) => config,
                Err(_) => {
                    return div()
                        .child(theme::text("sampler gain control unavailable"))
                        .into_any_element();
                }
            };
            let entity = cx.new(|cx| NumericControl::from_config(cx, config));
            let focus = entity.read(cx).focus_handle(cx);
            let subscription = cx.subscribe(
                &entity,
                move |view, emitter, event: &NumericControlEvent, cx| {
                    let Some(control) = view
                        .sampler_gain
                        .control
                        .as_mut()
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
                        view.submit_sampler_gain(captured, event.value);
                    }
                    cx.notify();
                },
            );
            self.sampler_gain.control = Some(Control {
                target,
                gesture: None,
                focus,
                entity,
                _subscription: subscription,
            });
        }
        if let Some(control) = self.sampler_gain.control.as_mut() {
            control.target = target;
        }
        let enabled = !self.edit_pending()
            && self.first_sampler_current()
            && self
                .audio
                .as_ref()
                .and_then(|a| a.control.status())
                .is_some_and(|s| !s.playing.load(Ordering::Acquire));
        let Some(control) = self.sampler_gain.control.as_ref() else {
            return div().into_any_element();
        };
        let entity = control.entity.clone();
        entity.update(cx, |value, cx| {
            let _ = value.set_external_value(gain, cx);
            value.set_enabled(enabled, cx);
        });
        div()
            .id(("sampler-gain-control", device.id().value()))
            .debug_selector(|| "sampler-gain-control".to_owned())
            .w(gpui::px(136.0))
            .h(gpui::px(26.0))
            .flex()
            .items_center()
            .gap(gpui::px(4.0))
            .child(theme::text("gain"))
            .child(
                div()
                    .w(gpui::px(92.0))
                    .h(gpui::px(18.0))
                    .child(entity.clone()),
            )
            .into_any_element()
    }
}
