//! Track-header bindings for the shared numeric widget; canonical gain stays on the actor.
use super::{
    EditorView, device_panel,
    pending_edit::{Disposition, Receipt},
};
use crate::theme;
use gpui::{Context, Div, Entity, Subscription, div, prelude::*, px};
use hex_engine::AudioSourceContext;
use hex_lattice_contract::ParameterValue;
use hex_project::{Command, RelativeTrackGains, ReturnId, ReturnTrack, Track, TrackId};
use hex_session::{
    device_gesture::GestureTarget,
    service::{Event, Request},
};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
};
use std::{cell::Cell, collections::BTreeMap, rc::Rc, sync::atomic::Ordering::Acquire};

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) mod tests;

#[derive(Default)]
pub(super) struct Controls {
    generation: u64,
    revision: Option<u64>,
    bindings: BTreeMap<TrackId, Binding>,
    mixer: BTreeMap<MixerKey, Binding>,
    pending_recovery: Option<PendingRecovery>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlAnchor {
    TrackVolume(TrackId),
    Mixer(MixerKey),
}

#[derive(Clone, PartialEq)]
enum RecoveryTarget {
    TrackGain(TrackId),
    TrackGains(Box<RelativeTrackGains>),
    MainGain,
    ReturnGain(ReturnId),
    TrackBalance(TrackId),
}

impl RecoveryTarget {
    fn anchor(&self) -> ControlAnchor {
        match self {
            Self::TrackGain(track) => ControlAnchor::TrackVolume(*track),
            Self::TrackGains(group) => ControlAnchor::TrackVolume(group.anchor()),
            Self::MainGain => ControlAnchor::Mixer(MixerKey::MainGain),
            Self::ReturnGain(id) => ControlAnchor::Mixer(MixerKey::ReturnGain(*id)),
            Self::TrackBalance(track) => ControlAnchor::Mixer(MixerKey::Balance(*track)),
        }
    }

    fn is_pan(&self) -> bool {
        matches!(self, Self::TrackBalance(_))
    }

    fn references_track(&self, track: TrackId) -> bool {
        match self {
            Self::TrackGain(id) | Self::TrackBalance(id) => *id == track,
            Self::TrackGains(group) => group.starting_gain(track).is_some(),
            Self::MainGain | Self::ReturnGain(_) => false,
        }
    }

    fn displayed_track_gain(&self, track: TrackId, value: f32) -> Option<f32> {
        match self {
            Self::TrackGain(id) if *id == track => Some(value),
            Self::TrackGains(group) => group.gain_for(track, value).ok().flatten(),
            _ => None,
        }
    }

    fn displayed_mixer_value(&self, key: MixerKey, value: f32) -> Option<f32> {
        match (self, key) {
            (Self::MainGain, MixerKey::MainGain) => Some(value),
            (Self::ReturnGain(id), MixerKey::ReturnGain(other)) if *id == other => Some(value),
            (Self::TrackBalance(id), MixerKey::Balance(other)) if *id == other => Some(value),
            _ => None,
        }
    }

    fn command(&self, project: &hex_project::Project, value: f32) -> Result<Command, &'static str> {
        match self {
            Self::TrackGain(track) => {
                if project.track(*track).is_none_or(automated) {
                    return Err("automation owns gain · edit or clear the gain lane");
                }
                Ok(Command::Gain {
                    track: *track,
                    gain: value,
                })
            }
            Self::TrackGains(group) => group.command(project, value).map_err(|error| error.0),
            Self::MainGain => Ok(Command::MasterGain { gain: value }),
            Self::ReturnGain(return_track) => {
                if !project
                    .return_tracks()
                    .iter()
                    .any(|track| track.id() == *return_track)
                {
                    return Err("return track no longer exists");
                }
                Ok(Command::ReturnGain {
                    return_track: *return_track,
                    gain: value,
                })
            }
            Self::TrackBalance(track) => {
                if project.track(*track).is_none() {
                    return Err("track no longer exists");
                }
                Ok(Command::Balance {
                    track: *track,
                    balance: value,
                })
            }
        }
    }

    fn matches_request(&self, request: &Request) -> bool {
        match (self, request) {
            (Self::TrackGain(id), Request::Edit(Command::Gain { track, .. })) => id == track,
            (Self::TrackGains(_), Request::Edit(Command::SetTrackGains { .. })) => true,
            (Self::MainGain, Request::Edit(Command::MasterGain { .. })) => true,
            (Self::ReturnGain(id), Request::Edit(Command::ReturnGain { return_track, .. })) => {
                id == return_track
            }
            (Self::TrackBalance(id), Request::Edit(Command::Balance { track, .. })) => id == track,
            _ => false,
        }
    }
}

struct PendingRecovery {
    receipt: Option<Receipt>,
    context: AudioSourceContext,
    target: RecoveryTarget,
    value: f32,
}

impl Controls {
    pub(super) fn displayed_track_gain(&self, generation: u64, track: TrackId) -> Option<f32> {
        let pending = self.pending_recovery.as_ref()?;
        if pending.context.project_generation != generation {
            return None;
        }
        pending.target.displayed_track_gain(track, pending.value)
    }

    fn displayed_mixer_value(&self, generation: u64, key: MixerKey) -> Option<f32> {
        let pending = self.pending_recovery.as_ref()?;
        if pending.context.project_generation != generation {
            return None;
        }
        pending.target.displayed_mixer_value(key, pending.value)
    }

    pub(super) fn has_queued_recovery(&self) -> bool {
        self.pending_recovery
            .as_ref()
            .is_some_and(|pending| pending.receipt.as_ref().is_some_and(Receipt::is_queued))
    }

    pub(super) fn has_committed_recovery(&self) -> bool {
        self.pending_recovery
            .as_ref()
            .is_some_and(|pending| pending.receipt.is_some())
    }

    pub(super) fn retire_deferred_recovery(&mut self) -> bool {
        if self.has_queued_recovery() {
            self.pending_recovery = None;
            true
        } else {
            false
        }
    }
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
    pub(super) fn pending_return_gain(&self, generation: u64, cx: &gpui::App) -> Option<ReturnId> {
        if self.generation != generation {
            return None;
        }
        self.mixer.iter().find_map(|(key, binding)| match key {
            MixerKey::ReturnGain(id)
                if binding.entity.read(cx).is_interacting()
                    || binding.entity.read(cx).is_editing() =>
            {
                Some(*id)
            }
            _ => None,
        })
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
    MainGain,
    Balance(TrackId),
    ReturnGain(ReturnId),
}
impl MixerKey {
    fn target(self) -> GestureTarget {
        match self {
            Self::MainGain => GestureTarget::MainGain,
            Self::Balance(track) => GestureTarget::TrackBalance { track },
            Self::ReturnGain(return_track) => GestureTarget::ReturnGain { return_track },
        }
    }
    fn canonical_value(self, view: &EditorView) -> Option<f32> {
        let project = &view.state.as_ref()?.project;
        match self {
            Self::MainGain => Some(project.master_gain()),
            Self::Balance(id) => project.track(id).map(Track::balance),
            Self::ReturnGain(id) => project
                .return_tracks()
                .iter()
                .find(|track| track.id() == id)
                .map(ReturnTrack::gain),
        }
    }
    fn current(self, view: &EditorView) -> bool {
        self.canonical_value(view).is_some()
    }
    fn supports_atomic_gain(self) -> bool {
        matches!(self, Self::MainGain | Self::ReturnGain(_))
    }
    fn atomic_gain_command(self, gain: f32) -> Option<hex_project::Command> {
        match self {
            Self::MainGain => Some(hex_project::Command::MasterGain { gain }),
            Self::ReturnGain(return_track) => {
                Some(hex_project::Command::ReturnGain { return_track, gain })
            }
            Self::Balance(_) => None,
        }
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
    if view.track_gain_controls.generation != view.project_generation
        && view
            .track_gain_controls
            .pending_recovery
            .as_ref()
            .is_some_and(|pending| pending.context.project_generation != view.project_generation)
    {
        view.track_gain_controls.pending_recovery = None;
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
        if view
            .track_gain_controls
            .pending_recovery
            .as_ref()
            .is_some_and(|pending| {
                pending.receipt.is_none() && pending.target.references_track(track)
            })
        {
            view.track_gain_controls.pending_recovery = None;
        }
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
        if view
            .track_gain_controls
            .pending_recovery
            .as_ref()
            .is_some_and(|pending| {
                pending.receipt.is_none() && pending.target.anchor() == ControlAnchor::Mixer(key)
            })
        {
            view.track_gain_controls.pending_recovery = None;
        }
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
                view.begin_track_gains_gesture(group)?;
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

fn gain_recovery_blocker(view: &mut EditorView) -> Option<&'static str> {
    if view.device_gesture.is_recovery_pending()
        && view
            .device_gesture
            .snapshot()
            .is_some_and(|active| active.target.is_mixer_gain())
    {
        if !view.device_gesture.has_pending_terminal() {
            let _ = view.cancel_device_gesture();
            view.wake_poll();
        }
        return Some("previous volume edit is canceling · drag again when it finishes");
    }
    if view.recovery.generation().is_some() {
        return Some("audio recovery pending · wait before editing");
    }
    None
}

fn begin_degraded_recovery(
    view: &mut EditorView,
    target: RecoveryTarget,
    value: f32,
) -> Result<(), &'static str> {
    let Some(state) = view.state.as_ref() else {
        return Err("project is still loading");
    };
    let context = AudioSourceContext {
        project_generation: view.project_generation,
        editor_revision: state.revision,
    };
    let playing = view
        .audio_status()
        .is_some_and(|status| status.playing.load(Acquire));
    view.recover_audio();
    if view.recovery.generation().is_none() {
        return Err("audio recovery could not start");
    }
    if playing {
        view.defer_play_until_audio_recovers();
    }
    view.recovery.bind_mixer_edit(context);
    let is_pan = target.is_pan();
    view.track_gain_controls.pending_recovery = Some(PendingRecovery {
        receipt: None,
        context,
        target,
        value,
    });
    view.status = if is_pan {
        "restarting audio · pan will update when ready"
    } else {
        "restarting audio · volume will update when ready"
    }
    .into();
    view.error = None;
    Ok(())
}

fn dispatch_degraded_recovery(
    view: &mut EditorView,
    anchor: ControlAnchor,
    phase: NumericEventPhase,
    value: f32,
) -> Option<Result<(), &'static str>> {
    let owns = view
        .track_gain_controls
        .pending_recovery
        .as_ref()
        .is_some_and(|pending| {
            pending.receipt.is_none()
                && pending.context.project_generation == view.project_generation
                && pending.target.anchor() == anchor
        });
    if !owns {
        return None;
    }
    match phase {
        NumericEventPhase::Begin => Some(Ok(())),
        NumericEventPhase::Change => {
            if let Some(pending) = view.track_gain_controls.pending_recovery.as_mut() {
                pending.value = value;
            }
            Some(Ok(()))
        }
        NumericEventPhase::Cancel => {
            view.track_gain_controls.pending_recovery = None;
            Some(Ok(()))
        }
        NumericEventPhase::Commit => {
            let Some(pending) = view.track_gain_controls.pending_recovery.as_mut() else {
                return Some(Ok(()));
            };
            pending.value = value;
            let context = pending.context;
            let Some(state) = view.state.as_ref() else {
                view.track_gain_controls.pending_recovery = None;
                return Some(Err("project is still loading"));
            };
            if state.revision != context.editor_revision {
                view.track_gain_controls.pending_recovery = None;
                return Some(Err("edit canceled · project changed"));
            }
            let command = match pending.target.command(&state.project, value) {
                Ok(command) => command,
                Err(reason) => {
                    view.track_gain_controls.pending_recovery = None;
                    return Some(Err(reason));
                }
            };
            pending.receipt = Some(Receipt::new(view.project_generation));
            if !view.enqueue_request(Request::Edit(command), false) {
                view.track_gain_controls.retire_deferred_recovery();
            }
            Some(Ok(()))
        }
    }
}

impl EditorView {
    pub(super) fn pending_mixer_edit_matches(&self, request: &Request) -> bool {
        self.track_gain_controls
            .pending_recovery
            .as_ref()
            .is_some_and(|pending| {
                pending.context.project_generation == self.project_generation
                    && pending.receipt.as_ref().is_some_and(Receipt::is_queued)
                    && pending.target.matches_request(request)
            })
    }

    pub(super) fn mixer_edit_submitted(&mut self) {
        if let Some(pending) = self.track_gain_controls.pending_recovery.as_mut()
            && let Some(receipt) = pending.receipt.as_mut()
            && receipt.is_queued()
        {
            receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(super) fn mixer_edit_request_complete(&mut self, event: &Event) {
        let retire = self
            .track_gain_controls
            .pending_recovery
            .as_mut()
            .and_then(|pending| pending.receipt.as_mut())
            .is_some_and(|receipt| {
                receipt.observe(
                    self.project_generation,
                    event.project_replaced,
                    event.request_complete,
                ) != Disposition::Waiting
            });
        if retire {
            self.track_gain_controls.pending_recovery = None;
        }
    }

    pub(super) fn retire_pending_mixer_edit(&mut self) -> bool {
        self.track_gain_controls.pending_recovery.take().is_some()
    }

    pub(super) fn retire_deferred_mixer_edit(&mut self) -> bool {
        self.track_gain_controls.retire_deferred_recovery()
    }

    pub(super) fn retire_abandoned_mixer_edit(&mut self) -> bool {
        let Some(target) = self
            .track_gain_controls
            .pending_recovery
            .as_ref()
            .filter(|pending| pending.receipt.as_ref().is_some_and(Receipt::is_queued))
            .map(|pending| &pending.target)
        else {
            return false;
        };
        let still_queued = self
            .pending_requests
            .iter()
            .chain(self.device_gesture_barriers.iter())
            .chain(
                self.recovery
                    .deferred_edit
                    .as_ref()
                    .map(|edit| edit.request.as_ref()),
            )
            .any(|request| target.matches_request(request));
        if still_queued {
            return false;
        }
        self.track_gain_controls.pending_recovery = None;
        true
    }
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
                    if event.phase != NumericEventPhase::Begin
                        && let Some(result) = dispatch_degraded_recovery(
                            view,
                            ControlAnchor::TrackVolume(id),
                            event.phase,
                            event.value as f32,
                        )
                    {
                        if let Err(reason) = result {
                            view.error = Some(reason.into());
                            control.update(cx, |control, cx| control.cancel(cx));
                        }
                        cx.notify();
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
                        } else if let Some(reason) = gain_recovery_blocker(view) {
                            Some(reason)
                        } else if view.edit_pending()
                            || view.pending_restart_request.is_some()
                            || view.track_gain_controls.pending_recovery.is_some()
                        {
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
                        if view.has_degraded_vst3_output() {
                            let target = match view.state.as_ref() {
                                Some(state)
                                    if view.track_selection.contains(id)
                                        && state
                                            .project
                                            .track(id)
                                            .is_some_and(|track| track.gain() > 0.0) =>
                                {
                                    let selected = view.track_selection.ordered(&state.project);
                                    if selected.len() > 1 {
                                        match RelativeTrackGains::capture(
                                            &state.project,
                                            id,
                                            &selected,
                                        ) {
                                            Ok(group) if group.len() > 1 => {
                                                RecoveryTarget::TrackGains(Box::new(group))
                                            }
                                            Ok(_) => RecoveryTarget::TrackGain(id),
                                            Err(error) => {
                                                view.error = Some(error.0.into());
                                                control
                                                    .update(cx, |control, cx| control.cancel(cx));
                                                cx.notify();
                                                return;
                                            }
                                        }
                                    } else {
                                        RecoveryTarget::TrackGain(id)
                                    }
                                }
                                _ => RecoveryTarget::TrackGain(id),
                            };
                            if let Err(reason) =
                                begin_degraded_recovery(view, target, event.value as f32)
                            {
                                if view.error.is_none() {
                                    view.error = Some(reason.into());
                                }
                                control.update(cx, |control, cx| control.cancel(cx));
                            }
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
            .track_gain_controls
            .displayed_track_gain(self.project_generation, id)
            .or_else(|| {
                self.device_gesture
                    .preview_track_gain(self.project_generation, id)
            });
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

    pub(super) fn main_volume(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(gain) = self.state.as_ref().map(|state| state.project.master_gain()) else {
            return div();
        };
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
            MixerKey::MainGain,
            "main-volume".into(),
            "main volume · db",
            spec,
            gain,
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
            let atomic_admitted = Cell::new(false);
            let subscription = cx.subscribe(
                &entity,
                move |view, control, event: &NumericControlEvent, cx| {
                    if generation != view.project_generation || !key.current(view) {
                        return;
                    }
                    if event.phase != NumericEventPhase::Begin
                        && let Some(result) = dispatch_degraded_recovery(
                            view,
                            ControlAnchor::Mixer(key),
                            event.phase,
                            event.value as f32,
                        )
                    {
                        if let Err(reason) = result {
                            view.error = Some(reason.into());
                            control.update(cx, |control, cx| control.cancel(cx));
                        }
                        cx.notify();
                        return;
                    }
                    if event.phase == NumericEventPhase::Begin {
                        atomic_admitted.set(false);
                    }
                    let explicit_gain = key.supports_atomic_gain() && event.gesture.is_none();
                    if event.phase == NumericEventPhase::Begin
                        && view.document_audio_mode == hex_session::DocumentAudioMode::Resident
                        && !view.gain_recorder.blocks_edits()
                        && !view.midi_recorder.blocks_edits()
                        && let Some(reason) = gain_recovery_blocker(view)
                    {
                        view.error = Some(reason.into());
                        control.update(cx, |control, cx| control.cancel(cx));
                        cx.notify();
                        return;
                    }
                    if event.phase == NumericEventPhase::Begin
                        && ((view.document_audio_mode != hex_session::DocumentAudioMode::Resident
                            && !explicit_gain)
                            || view.gain_recorder.blocks_edits()
                            || view.midi_recorder.blocks_edits()
                            || view.edit_pending()
                            || view.pending_restart_request.is_some()
                            || view.track_gain_controls.pending_recovery.is_some())
                    {
                        view.error = Some(if explicit_gain {
                            "finish the current edit before changing output volume"
                        } else {
                            "finish the current edit · held mixer controls require resident audio"
                        }.into());
                        let rejected_gain = (key.supports_atomic_gain()
                            && view.document_audio_mode
                                != hex_session::DocumentAudioMode::Resident)
                            .then(|| key.canonical_value(view).unwrap_or(canonical));
                        control.update(cx, |control, cx| {
                            control.cancel(cx);
                            if let Some(gain) = rejected_gain {
                                let _ = control.set_external_value(f64::from(gain), cx);
                            }
                        });
                        cx.notify();
                        return;
                    }
                    if event.phase == NumericEventPhase::Begin
                        && view.document_audio_mode == hex_session::DocumentAudioMode::Resident
                        && view.has_degraded_vst3_output()
                    {
                        let recovery_target = match key {
                            MixerKey::MainGain => RecoveryTarget::MainGain,
                            MixerKey::ReturnGain(id) => RecoveryTarget::ReturnGain(id),
                            MixerKey::Balance(track) => RecoveryTarget::TrackBalance(track),
                        };
                        if let Err(reason) =
                            begin_degraded_recovery(view, recovery_target, event.value as f32)
                        {
                            if view.error.is_none() {
                                view.error = Some(reason.into());
                            }
                            control.update(cx, |control, cx| control.cancel(cx));
                        }
                        cx.notify();
                        return;
                    }
                    // Verified sources retain explicit atomic output-gain edits. Held
                    // preview stays resident-only; the actor owns the sole edit.
                    if key.supports_atomic_gain()
                        && view.document_audio_mode != hex_session::DocumentAudioMode::Resident
                    {
                        match event.phase {
                            NumericEventPhase::Begin => atomic_admitted.set(true),
                            NumericEventPhase::Commit if atomic_admitted.replace(false) => {
                                let Some(command) = key.atomic_gain_command(event.value as f32)
                                else {
                                    return;
                                };
                                if view.edit_pending()
                                    || view.gain_recorder.blocks_edits()
                                    || view.midi_recorder.blocks_edits()
                                    || !view.enqueue_device_barrier(
                                        hex_session::service::Request::Edit(command),
                                    )
                                {
                                    view.error = Some(
                                        "finish the current edit before changing output volume"
                                            .into(),
                                    );
                                    let canonical = key.canonical_value(view).unwrap_or(canonical);
                                    control.update(cx, |control, cx| {
                                        control.cancel(cx);
                                        let _ =
                                            control.set_external_value(f64::from(canonical), cx);
                                    });
                                }
                            }
                            NumericEventPhase::Cancel => atomic_admitted.set(false),
                            _ => {}
                        }
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
            .track_gain_controls
            .displayed_mixer_value(self.project_generation, key)
            .or_else(|| {
                self.device_gesture
                    .snapshot()
                    .filter(|active| active.target == target)
                    .and_then(|active| match active.value {
                        ParameterValue::Float(value) => Some(value),
                        _ => None,
                    })
            });
        let enabled = (self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            || key.supports_atomic_gain())
            && !self.gain_recorder.blocks_edits()
            && !self.midi_recorder.blocks_edits()
            && !self.opening_project
            && !self.close_prompt_open;
        let binding = &self.track_gain_controls.mixer[&key];
        binding.entity.update(cx, |control, cx| {
            control.set_enabled(enabled, cx);
            let _ = control.set_external_style(theme::control_style(), cx);
            // The existing worker queue retains an explicit output-gain edit until its
            // acknowledgement; do not flash the old value during that handoff.
            if !(key.supports_atomic_gain()
                && self.document_audio_mode != hex_session::DocumentAudioMode::Resident
                && self.edit_pending())
            {
                let _ = control.set_external_value(f64::from(active.unwrap_or(canonical)), cx);
            }
        });
        div()
            .flex()
            .flex_1()
            .min_w(px(60.0))
            .h(px(18.0))
            .child(binding.entity.clone())
    }
}
