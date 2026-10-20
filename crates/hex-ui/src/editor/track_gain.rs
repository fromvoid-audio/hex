//! Track-header bindings for the shared numeric widget; canonical gain stays on the actor.
use super::{EditorView, device_panel};
use crate::theme;
use gpui::{Context, Div, Entity, Subscription, div, prelude::*, px};
use hex_lattice_contract::ParameterValue;
use hex_project::{ReturnId, Track, TrackId};
use hex_session::device_gesture::GestureTarget;
use hex_session::snapshot_exports::{SnapshotProjectRead, SnapshotRead, SnapshotTransportError};
use hex_widgets::{
    NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlEvent,
    NumericControlKind, NumericDragAxis, NumericEventPhase, NumericScale, NumericSpec,
};
use std::{cell::Cell, collections::BTreeMap, rc::Rc};

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

#[derive(Default)]
pub(super) struct Controls {
    generation: u64,
    revision: Option<u64>,
    bindings: BTreeMap<TrackId, Binding>,
    mixer: BTreeMap<MixerKey, Binding>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MixerKey {
    Balance(TrackId),
    ReturnGain(ReturnId),
}
impl MixerKey {
    fn target(self) -> GestureTarget {
        match self {
            Self::Balance(track) => GestureTarget::TrackBalance { track },
            Self::ReturnGain(return_track) => GestureTarget::ReturnGain { return_track },
        }
    }
    fn current_in(self, project: SnapshotProjectRead<'_>) -> bool {
        match self {
            Self::Balance(track) => project.track(track).is_some(),
            Self::ReturnGain(id) => project.returns().any(|track| track.id() == id),
        }
    }
    fn current(self, view: &EditorView) -> bool {
        view.state.as_ref().is_some_and(|state| {
            state
                .read_with(|snapshot| self.current_in(snapshot.model()))
                .unwrap_or(false)
        })
    }
}

struct Binding {
    entity: Entity<NumericControl>,
    lease: device_panel::NumericGestureLease,
    _subscription: Subscription,
    _observation: Subscription,
}

/// Mixer activators share palette resolution without adding project state.
pub(super) fn activator_colors(active: bool) -> (u32, u32) {
    let background = theme::color(if active {
        theme::PaletteRole::Primary
    } else {
        theme::PaletteRole::Raised
    });
    let preferred = theme::color(if active {
        theme::PaletteRole::Surface
    } else {
        theme::PaletteRole::Muted
    });
    (background, theme::readable_text(background, preferred))
}

fn automated(track: &Track) -> bool {
    !track.gain_envelope().is_empty() || !track.gain_lane().spans().is_empty()
}

pub(super) struct Retirement {
    generation: u64,
    revision: u64,
    gains: Vec<TrackId>,
    mixer: Vec<MixerKey>,
}
pub(super) fn retirement_buffers(view: &EditorView) -> Result<Retirement, &'static str> {
    let mut gains = Vec::new();
    let mut mixer = Vec::new();
    gains
        .try_reserve_exact(view.track_gain_controls.bindings.len())
        .map_err(|_| "could not reserve volume retirement")?;
    mixer
        .try_reserve_exact(view.track_gain_controls.mixer.len())
        .map_err(|_| "could not reserve mixer retirement")?;
    Ok(Retirement {
        generation: view.project_generation,
        revision: 0,
        gains,
        mixer,
    })
}
pub(super) fn retirement(
    view: &EditorView,
    snapshot: SnapshotRead<'_>,
    output: &mut Retirement,
) -> Result<bool, &'static str> {
    // Reconcile the actual read even when its revision is saturated. A different
    // installed source can legitimately carry the same project revision.
    let project = snapshot.model();
    output.revision = snapshot.revision();
    for id in view.track_gain_controls.bindings.keys().copied() {
        if view.track_gain_controls.generation != view.project_generation
            || project.track(id).is_none_or(|track| {
                !track.gain_points().is_empty() || track.gain_spans().next().is_some()
            })
        {
            if output.gains.len() == output.gains.capacity() {
                return Err("volume retirement inventory changed");
            }
            output.gains.push(id);
        }
    }
    for key in view.track_gain_controls.mixer.keys().copied() {
        if view.track_gain_controls.generation != view.project_generation
            || !key.current_in(project)
        {
            if output.mixer.len() == output.mixer.capacity() {
                return Err("mixer retirement inventory changed");
            }
            output.mixer.push(key);
        }
    }
    Ok(true)
}
pub(super) fn apply_retirement(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    prepared: Option<Retirement>,
) {
    let Some(prepared) = prepared else {
        return;
    };
    let stale = prepared.gains;
    for track in stale {
        if let Some(binding) = view.track_gain_controls.bindings.remove(&track) {
            if device_panel::owns_numeric_gesture(
                view,
                GestureTarget::TrackGain { track },
                &binding.lease,
            ) {
                let _ = view.cancel_device_gesture();
            }
            binding.lease.set(None);
            binding.entity.update(cx, |control, cx| control.cancel(cx));
        }
    }
    let stale = prepared.mixer;
    for key in stale {
        if let Some(binding) = view.track_gain_controls.mixer.remove(&key) {
            if device_panel::owns_numeric_gesture(view, key.target(), &binding.lease) {
                let _ = view.cancel_device_gesture();
            }
            binding.lease.set(None);
            binding.entity.update(cx, |control, cx| control.cancel(cx));
        }
    }
    view.track_gain_controls.generation = prepared.generation;
    view.track_gain_controls.revision = Some(prepared.revision);
}
fn retire_stale(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Result<bool, SnapshotTransportError> {
    let Some(state) = view.state.as_ref() else {
        return Ok(true);
    };
    let mut output = match retirement_buffers(view) {
        Ok(output) => output,
        Err(reason) => {
            view.error = Some(reason.into());
            return Ok(false);
        }
    };
    let apply = state.read_with(|snapshot| retirement(view, snapshot, &mut output))?;
    match apply {
        Ok(changed) => apply_retirement(view, cx, changed.then_some(output)),
        Err(reason) => {
            view.error = Some(reason.into());
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn controls_for_finalize(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Vec<(GestureTarget, Entity<NumericControl>)> {
    // Retirement was prepared with the finalizer's single scoped read.
    let _ = cx;
    view.track_gain_controls
        .bindings
        .iter()
        .map(|(track, binding)| {
            (
                GestureTarget::TrackGain { track: *track },
                binding.entity.clone(),
            )
        })
        .chain(
            view.track_gain_controls
                .mixer
                .iter()
                .map(|(key, binding)| (key.target(), binding.entity.clone())),
        )
        .collect()
}

impl EditorView {
    pub(super) fn track_volume(&mut self, track: &Track, cx: &mut Context<Self>) -> Div {
        if !matches!(retire_stale(self, cx), Ok(true)) {
            return div();
        }
        let id = track.id();
        if automated(track) {
            return div()
                .flex_1()
                .min_w(px(60.0))
                .h(px(18.0))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .debug_selector(move || format!("track-volume-auto-{}", id.value()))
                .child(theme::text("auto"));
        }
        let target = GestureTarget::TrackGain { track: id };
        let generation = self.project_generation;
        if let std::collections::btree_map::Entry::Vacant(entry) =
            self.track_gain_controls.bindings.entry(id)
        {
            let config = NumericSpec::new(
                0.0,
                2.0,
                1.0,
                NumericScale::LinearAmplitudeDecibels,
                1,
                Some("db"),
            )
            .and_then(|spec| {
                NumericControlConfig::new(
                    format!("track-volume-{}", id.value()),
                    theme::text("track volume · db"),
                    NumericControlKind::HorizontalSlider,
                    spec,
                    f64::from(track.gain()),
                    device_panel::control_style(),
                )
            });
            let Ok(config) = config else { return div() };
            let entity = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    config.with_appearance(NumericControlAppearance::FilledSlider),
                )
            });
            let lease: device_panel::NumericGestureLease = Rc::new(Cell::new(None));
            let captured = lease.clone();
            let subscription = cx.subscribe(
                &entity,
                move |view, control, event: &NumericControlEvent, cx| {
                    if generation != view.project_generation {
                        return;
                    }
                    if event.phase == NumericEventPhase::Begin {
                        let Some(state) = view.state.as_ref() else {
                            return;
                        };
                        let Ok(owns_automation) = state.read_with(|snapshot| {
                            snapshot.model().track(id).is_none_or(|track| {
                                !track.gain_points().is_empty()
                                    || track.gain_spans().next().is_some()
                            })
                        }) else {
                            return;
                        };
                        // Gesture pumping bypasses ordinary edit admission while recording.
                        let reason = if view.document_audio_mode
                            != hex_session::DocumentAudioMode::Resident
                        {
                            Some("live track volume is unavailable in this project mode")
                        } else if view.gain_recorder.blocks_edits() {
                            Some("finish gain recording before changing track volume")
                        } else if owns_automation {
                            Some("automation owns gain · edit or clear the gain lane")
                        } else if view.edit_pending() {
                            Some("wait for the current project edit before changing volume")
                        } else {
                            None
                        };
                        if let Some(reason) = reason {
                            view.error = Some(reason.into());
                            control.update(cx, |control, cx| control.cancel(cx));
                            cx.notify();
                            return;
                        }
                    }
                    if let Err(reason) = device_panel::dispatch_bound_numeric_gesture(
                        view,
                        target,
                        &captured,
                        event.phase,
                        ParameterValue::Float(event.value as f32),
                    ) {
                        view.error = Some(reason.into());
                        control.update(cx, |control, cx| control.cancel(cx));
                    }
                    cx.notify();
                },
            );
            let observation = cx.observe(&entity, |_, _, cx| cx.notify());
            entry.insert(Binding {
                entity,
                lease,
                _subscription: subscription,
                _observation: observation,
            });
        }
        let active = self
            .device_gesture
            .snapshot()
            .filter(|active| active.target == target)
            .and_then(|active| match active.value {
                ParameterValue::Float(gain) => Some(gain),
                _ => None,
            });
        let enabled = self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && !automated(track)
            && !self.gain_recorder.blocks_edits()
            && !self.opening_project
            && !self.close_prompt_open;
        let binding = &self.track_gain_controls.bindings[&id];
        binding.entity.update(cx, |control, cx| {
            control.set_enabled(enabled, cx);
            let _ = control.set_external_style(device_panel::control_style(), cx);
            let _ = control.set_external_value(f64::from(active.unwrap_or(track.gain())), cx);
        });
        div()
            .flex()
            .flex_1()
            .min_w(px(60.0))
            .h(px(18.0))
            .child(binding.entity.clone())
    }
}

impl EditorView {
    pub(super) fn track_pan(&mut self, track: &Track, cx: &mut Context<Self>) -> Div {
        let Ok(spec) = NumericSpec::new(-1.0, 1.0, 0.0, NumericScale::Bipolar, 2, None::<&str>)
        else {
            return div();
        };
        self.mixer_control(
            MixerKey::Balance(track.id()),
            format!("track-pan-{}", track.id().value()),
            "track pan · original stereo balance",
            device_panel::balance_spec(spec),
            track.balance(),
            cx,
        )
    }

    pub(super) fn return_volume(&mut self, id: ReturnId, gain: f32, cx: &mut Context<Self>) -> Div {
        let Ok(spec) = NumericSpec::new(
            0.0,
            2.0,
            1.0,
            NumericScale::LinearAmplitudeDecibels,
            1,
            Some("db"),
        ) else {
            return div();
        };
        self.mixer_control(
            MixerKey::ReturnGain(id),
            format!("return-volume-{}", id.value()),
            "return volume · db",
            spec,
            gain,
            cx,
        )
    }

    fn mixer_control(
        &mut self,
        key: MixerKey,
        id: String,
        label: &str,
        spec: NumericSpec,
        canonical: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        if !matches!(retire_stale(self, cx), Ok(true)) {
            return div();
        }
        let target = key.target();
        let generation = self.project_generation;
        if let std::collections::btree_map::Entry::Vacant(entry) =
            self.track_gain_controls.mixer.entry(key)
        {
            let Ok(config) = NumericControlConfig::new(
                id,
                theme::text(label),
                NumericControlKind::HorizontalSlider,
                spec,
                f64::from(canonical),
                device_panel::control_style(),
            ) else {
                return div();
            };
            let config = config.with_appearance(NumericControlAppearance::FilledSlider);
            let config = if matches!(key, MixerKey::Balance(_)) {
                config.with_drag_axis(NumericDragAxis::Vertical)
            } else {
                config
            };
            let entity = cx.new(|cx| NumericControl::from_config(cx, config));
            let lease: device_panel::NumericGestureLease = Rc::new(Cell::new(None));
            let captured = lease.clone();
            let subscription = cx.subscribe(
                &entity,
                move |view, control, event: &NumericControlEvent, cx| {
                    if generation != view.project_generation || !key.current(view) {
                        return;
                    }
                    if event.phase == NumericEventPhase::Begin
                        && (view.document_audio_mode != hex_session::DocumentAudioMode::Resident
                            || view.gain_recorder.blocks_edits()
                            || view.edit_pending())
                    {
                        view.error = Some(
                            "finish the current edit · held mixer controls require resident audio"
                                .into(),
                        );
                        control.update(cx, |control, cx| control.cancel(cx));
                        cx.notify();
                        return;
                    }
                    if let Err(reason) = device_panel::dispatch_bound_numeric_gesture(
                        view,
                        target,
                        &captured,
                        event.phase,
                        ParameterValue::Float(event.value as f32),
                    ) {
                        view.error = Some(reason.into());
                        control.update(cx, |control, cx| control.cancel(cx));
                    }
                    cx.notify();
                },
            );
            let observation = cx.observe(&entity, |_, _, cx| cx.notify());
            entry.insert(Binding {
                entity,
                lease,
                _subscription: subscription,
                _observation: observation,
            });
        }
        let active = self
            .device_gesture
            .snapshot()
            .filter(|active| active.target == target)
            .and_then(|active| match active.value {
                ParameterValue::Float(value) => Some(value),
                _ => None,
            });
        let enabled = self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && !self.gain_recorder.blocks_edits()
            && !self.opening_project
            && !self.close_prompt_open;
        let binding = &self.track_gain_controls.mixer[&key];
        binding.entity.update(cx, |control, cx| {
            control.set_enabled(enabled, cx);
            let _ = control.set_external_style(device_panel::control_style(), cx);
            let _ = control.set_external_value(f64::from(active.unwrap_or(canonical)), cx);
        });
        div()
            .flex()
            .flex_1()
            .min_w(px(60.0))
            .h(px(18.0))
            .child(binding.entity.clone())
    }
}
