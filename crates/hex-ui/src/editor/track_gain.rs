//! Track-header bindings for the shared numeric widget; canonical gain stays on the actor.
use super::{EditorView, device_panel};
use crate::theme;
use gpui::{Context, Div, Entity, Subscription, div, prelude::*, px};
use hex_lattice_contract::ParameterValue;
use hex_project::{Track, TrackId};
use hex_session::device_gesture::GestureTarget;
use hex_widgets::{
    NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlEvent,
    NumericControlKind, NumericEventPhase, NumericScale, NumericSpec,
};
use std::{cell::Cell, collections::BTreeMap, rc::Rc};

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

#[derive(Default)]
pub(super) struct Controls {
    generation: u64,
    revision: Option<u64>,
    bindings: BTreeMap<TrackId, Binding>,
}

struct Binding {
    entity: Entity<NumericControl>,
    lease: device_panel::NumericGestureLease,
    _subscription: Subscription,
    _observation: Subscription,
}

fn automated(track: &Track) -> bool {
    !track.gain_envelope().is_empty() || !track.gain_lane().spans().is_empty()
}

fn retire_stale(view: &mut EditorView, cx: &mut Context<EditorView>) {
    let Some(state) = view.state.as_ref() else {
        return;
    };
    if view.track_gain_controls.generation == view.project_generation
        && view.track_gain_controls.revision == Some(state.revision)
    {
        return;
    }
    let stale = view
        .track_gain_controls
        .bindings
        .keys()
        .copied()
        .filter(|id| {
            view.track_gain_controls.generation != view.project_generation
                || state.project.track(*id).is_none_or(automated)
        })
        .collect::<Vec<_>>();
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
    view.track_gain_controls.generation = view.project_generation;
    view.track_gain_controls.revision = view.state.as_ref().map(|state| state.revision);
}

pub(super) fn controls_for_finalize(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Vec<(GestureTarget, Entity<NumericControl>)> {
    retire_stale(view, cx);
    view.track_gain_controls
        .bindings
        .iter()
        .map(|(track, binding)| {
            (
                GestureTarget::TrackGain { track: *track },
                binding.entity.clone(),
            )
        })
        .collect()
}

impl EditorView {
    pub(super) fn track_volume(&mut self, track: &Track, cx: &mut Context<Self>) -> Div {
        retire_stale(self, cx);
        let id = track.id();
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
                    config.with_appearance(NumericControlAppearance::CompactSlider),
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
                        // Gesture pumping bypasses ordinary edit admission while recording.
                        let reason = if view.document_audio_mode
                            != hex_session::DocumentAudioMode::Resident
                        {
                            Some("live track volume is unavailable in this project mode")
                        } else if view.gain_recorder.blocks_edits() {
                            Some("finish gain recording before changing track volume")
                        } else if view
                            .state
                            .as_ref()
                            .and_then(|s| s.project.track(id))
                            .is_none_or(automated)
                        {
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
            .flex_1()
            .min_w(px(86.0))
            .h(px(21.0))
            .child(binding.entity.clone())
    }
}
