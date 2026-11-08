//! Track-header bindings for the shared numeric widget; canonical gain stays on the actor.
use super::{EditorView, device_panel};
use crate::theme;
use gpui::{Context, Div, Entity, Subscription, div, prelude::*, px};
use hex_lattice_contract::ParameterValue;
use hex_project::{ReturnId, ReturnTrack, Track, TrackId};
use hex_session::device_gesture::GestureTarget;
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
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

impl Controls {
    /// Keep an unfinished numeric draft attached to its original track when
    /// the Console selection changes before the draft can be completed.
    pub(super) fn pending_track_gain(&self, generation: u64, cx: &gpui::App) -> Option<TrackId> {
        if self.generation != generation {
            return None;
        }
        self.bindings
            .iter()
            .find_map(|(id, binding)| binding.entity.read(cx).is_interacting().then_some(*id))
    }
    /// Read-only admission check; drafts remain with their numeric owners.
    /// Unlike finalization, it does not collect bindings or commit values.
    pub(super) fn has_pending_draft(&self, cx: &gpui::App) -> bool {
        self.bindings
            .values()
            .chain(self.mixer.values())
            .any(|binding| {
                let control = binding.entity.read(cx);
                control.is_interacting() || control.is_editing()
            })
    }
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
    fn current(self, view: &EditorView) -> bool {
        view.state.as_ref().is_some_and(|state| match self {
            Self::Balance(track) => state.project.track(track).is_some(),
            Self::ReturnGain(id) => state
                .project
                .return_tracks()
                .iter()
                .any(|track| track.id() == id),
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

pub(super) fn retire_stale(view: &mut EditorView, cx: &mut Context<EditorView>) {
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
                binding_target(view, track, &binding.lease),
                &binding.lease,
            ) {
                let _ = view.cancel_device_gesture();
            }
            binding.lease.set(None);
            binding.entity.update(cx, |control, cx| control.cancel(cx));
        }
    }
    let stale = view
        .track_gain_controls
        .mixer
        .keys()
        .copied()
        .filter(|key| {
            view.track_gain_controls.generation != view.project_generation || !key.current(view)
        })
        .collect::<Vec<_>>();
    for key in stale {
        if let Some(binding) = view.track_gain_controls.mixer.remove(&key) {
            if device_panel::owns_numeric_gesture(view, key.target(), &binding.lease) {
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
                binding_target(view, *track, &binding.lease),
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

fn binding_target(
    view: &EditorView,
    track: TrackId,
    lease: &device_panel::NumericGestureLease,
) -> GestureTarget {
    view.device_gesture
        .snapshot()
        .filter(|active| {
            Some(device_panel::NumericGestureKey::Canonical(active.key)) == lease.get()
                && active.key.generation == view.project_generation
                && active.target.track_gain_anchor() == Some(track)
        })
        .map_or(GestureTarget::TrackGain { track }, |active| active.target)
}

fn dispatch_track_gain(
    view: &mut EditorView,
    track: TrackId,
    lease: &device_panel::NumericGestureLease,
    phase: NumericEventPhase,
    value: ParameterValue,
) -> Result<(), &'static str> {
    if phase == NumericEventPhase::Begin
        && let Some(state) = view.state.as_ref()
        && view.track_selection.contains(track)
        && state
            .project
            .track(track)
            .is_some_and(|track| track.gain() > 0.0)
    {
        let selected = view.track_selection.ordered(&state.project);
        if selected.len() > 1 {
            let group = hex_project::RelativeTrackGains::capture(&state.project, track, &selected)
                .map_err(|error| error.0)?;
            if group.len() > 1 {
                view.device_gesture
                    .begin_track_gains(view.project_generation, group)?;
                lease.set(
                    view.device_gesture
                        .snapshot()
                        .map(|active| device_panel::NumericGestureKey::Canonical(active.key)),
                );
                return Ok(());
            }
        }
    }
    let target = binding_target(view, track, lease);
    device_panel::dispatch_bound_numeric_gesture(view, target, lease, phase, value)
}

impl EditorView {
    pub(super) fn track_volume(&mut self, track: &Track, cx: &mut Context<Self>) -> Div {
        retire_stale(self, cx);
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
                    theme::control_style(),
                )
            });
            let Ok(config) = config else { return div() };
            let entity = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    config.with_presentation(NumericControlPresentation::FilledSlider),
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
                        let reason = if view.gain_recorder.blocks_edits()
                            || view.midi_recorder.blocks_edits()
                        {
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
                    if let Err(reason) = dispatch_track_gain(
                        view,
                        id,
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
            .preview_track_gain(self.project_generation, id);
        let enabled = !automated(track)
            && !self.gain_recorder.blocks_edits()
            && !self.midi_recorder.blocks_edits()
            && !self.opening_project
            && !self.close_prompt_open;
        let binding = &self.track_gain_controls.bindings[&id];
        binding.entity.update(cx, |control, cx| {
            control.set_enabled(enabled, cx);
            let _ = control.set_external_style(theme::control_style(), cx);
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

    pub(super) fn return_volume(&mut self, track: &ReturnTrack, cx: &mut Context<Self>) -> Div {
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
            MixerKey::ReturnGain(track.id()),
            format!("return-volume-{}", track.id().value()),
            "return volume · db",
            spec,
            track.gain(),
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
        retire_stale(self, cx);
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
                theme::control_style(),
            ) else {
                return div();
            };
            let config = config
                .with_presentation(NumericControlPresentation::FilledSlider)
                .with_slider_center_mark(!matches!(key, MixerKey::Balance(_)));
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
                            || (view.gain_recorder.blocks_edits()
                                || view.midi_recorder.blocks_edits())
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
            && !self.midi_recorder.blocks_edits()
            && !self.opening_project
            && !self.close_prompt_open;
        let binding = &self.track_gain_controls.mixer[&key];
        binding.entity.update(cx, |control, cx| {
            control.set_enabled(enabled, cx);
            let _ = control.set_external_style(theme::control_style(), cx);
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
