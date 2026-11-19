//! View-local analyzer intent and complete mounted-chain membership. Audio
//! retains bounded capture/FFT ownership; focus never owns another EQ's tap.
use super::*;
use gpui::Focusable;
use hex_audio::spectrum_target::{
    SpectrumTarget, SpectrumTargetRequest, SpectrumTargets, TargetError,
};

const WAITING: &str = "spectrum waits for available current project audio";

#[derive(Default)]
pub(in super::super) struct SpectrumSelection {
    generation: u64,
    revision: Option<u64>,
    output_identity: Option<u64>,
    // Protocol lookup key plus canonical identity for revision retirement.
    off: HashMap<u64, DeviceId>,
    status_message: Option<String>,
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in super::super) test_channel: Option<hex_audio::spectrum_target::SpectrumTargetControl>,
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in super::super) test_client: Option<hex_audio::prepared::Client>,
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in super::super) test_now: Option<std::time::Instant>,
}
impl SpectrumSelection {
    fn enabled_raw(&self, device: u64) -> bool {
        !self.off.contains_key(&device)
    }
    pub(super) fn enabled(&self, device: DeviceId) -> bool {
        !self.off.contains_key(&device.value())
    }
}
pub(in crate::editor) fn is_eq(kind: hex_engine::DeviceTypeId) -> bool {
    matches!(kind.as_str(), Ok("hex.eq" | "hex.eq8"))
}

// The shared panel status can also carry save/edit failures. Retire only the
// analyzer message that this intent owns, never an unrelated newer message.
fn clear_status(view: &mut EditorView) -> bool {
    let owned = view.device_panel.eq_spectrum.status_message.take();
    if view.device_panel.message.as_deref() == Some(WAITING)
        || owned
            .as_deref()
            .is_some_and(|message| view.device_panel.message.as_deref() == Some(message))
    {
        view.device_panel.message = None;
        return true;
    }
    owned.is_some()
}

fn target_error_message(error: TargetError) -> &'static str {
    match error {
        TargetError::OutputClosed => "spectrum unavailable: audio output is closed",
        TargetError::InvalidDevice
        | TargetError::InvalidOwner
        | TargetError::WrongDeviceType
        | TargetError::TooManyTargets
        | TargetError::DuplicateTarget
        | TargetError::InconsistentContext => "spectrum unavailable: eq target set is invalid",
        TargetError::InvalidActivationFloor => "spectrum waits for a valid audio publication",
        TargetError::UnsupportedSampleRate(_) => {
            "spectrum unavailable: unsupported audio sample rate"
        }
        TargetError::EpochExhausted | TargetError::TokenExhausted => {
            "spectrum unavailable: target identity exhausted; reopen audio output"
        }
    }
}

fn report_status(view: &mut EditorView, message: String) {
    // Retrying an analyzer may replace its own feedback, never a newer error
    // owned by save/edit controls. Retain the reason so it can be shown once
    // that unrelated owner dismisses its message.
    let current = view.device_panel.message.as_deref();
    let available = current.is_none()
        || current == Some(WAITING)
        || current == view.device_panel.eq_spectrum.status_message.as_deref();
    view.device_panel.eq_spectrum.status_message = Some(message.clone());
    if available {
        view.device_panel.message = Some(message);
    }
}

fn report_error(view: &mut EditorView, error: TargetError) {
    report_status(view, target_error_message(error).into());
}

fn desired(view: &EditorView) -> Option<&SpectrumTargets> {
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(client) = &view.device_panel.eq_spectrum.test_client {
        return client.spectrum_desired_targets();
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(channel) = &view.device_panel.eq_spectrum.test_channel {
        return Some(channel.desired_targets());
    }
    view.audio
        .as_ref()
        .and_then(|audio| audio.control.spectrum_desired_targets())
}
fn output_identity(view: &EditorView) -> Option<u64> {
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(client) = &view.device_panel.eq_spectrum.test_client {
        return client.output_identity();
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    if view.device_panel.eq_spectrum.test_channel.is_some() {
        return Some(0);
    }
    view.audio
        .as_ref()
        .and_then(|audio| audio.control.output_identity())
}
fn publish(view: &mut EditorView, requests: &[SpectrumTargetRequest]) -> Result<(), TargetError> {
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(client) = &mut view.device_panel.eq_spectrum.test_client {
        client.spectrum_set_targets(requests)?;
        return Ok(());
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(channel) = &mut view.device_panel.eq_spectrum.test_channel {
        channel.set_targets(requests)?;
        return Ok(());
    }
    view.audio
        .as_mut()
        .ok_or(TargetError::OutputClosed)?
        .control
        .spectrum_set_targets(requests)?;
    Ok(())
}
fn clear(view: &mut EditorView) {
    let _ = publish(view, &[]);
    view.device_panel.spectrum_results.clear();
}
fn now(_view: &EditorView) -> std::time::Instant {
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(now) = _view.device_panel.eq_spectrum.test_now {
        return now;
    }
    std::time::Instant::now()
}
fn matches(
    result: &hex_audio::spectrum_worker::PublishedSpectra,
    target: SpectrumTarget,
    now: std::time::Instant,
) -> bool {
    result.output.stamp.target == target.token
        && result.output.stamp.generation == target.project_generation
        && result.output.stamp.sample_rate_hz == target.sample_rate_hz
        && result.output.stamp.applied_revision >= target.activation_floor
        && result.output.is_fresh_at(now)
}
pub(in crate::editor) fn result(
    view: &EditorView,
    identity: hex_engine::PostNodeIdentity,
) -> Option<&hex_audio::spectrum_worker::PublishedSpectra> {
    if !view
        .device_panel
        .eq_spectrum
        .enabled_raw(identity.device_id)
    {
        return None;
    }
    let (_, target) = desired(view)?.find(identity)?;
    let result = view.device_panel.spectrum_results.get(&target.token)?;
    (target.project_generation == view.project_generation && matches(result, target, now(view)))
        .then_some(result)
}

pub(in crate::editor) fn reconcile(view: &mut EditorView) -> bool {
    let endpoint = output_identity(view);
    let mut changed = false;
    if view.device_panel.eq_spectrum.generation != view.project_generation {
        clear_status(view);
        view.device_panel.eq_spectrum.generation = view.project_generation;
        view.device_panel.eq_spectrum.revision = None;
        view.device_panel.eq_spectrum.off.clear();
        view.device_panel.spectrum_results.clear();
        changed = true;
    }
    if view.device_panel.eq_spectrum.output_identity != endpoint {
        view.device_panel.eq_spectrum.output_identity = endpoint;
        view.device_panel.spectrum_results.clear();
        clear_status(view);
        changed = true;
    }
    if let Some(state) = view.state.as_ref()
        && view.device_panel.eq_spectrum.revision != Some(state.revision)
    {
        view.device_panel.eq_spectrum.off.retain(|_, id| {
            state
                .project
                .device(*id)
                .is_some_and(|device| is_eq(device.state().type_id()))
        });
        view.device_panel.eq_spectrum.revision = Some(state.revision);
    }
    let before: Vec<_> = desired(view)
        .into_iter()
        .flat_map(SpectrumTargets::iter)
        .map(|(_, target)| target)
        .collect();
    let mut valid: Vec<_> = before
        .iter()
        .copied()
        .filter(|target| {
            view.spectrum_target_is_visible_and_current(*target)
                && view
                    .device_panel
                    .eq_spectrum
                    .enabled_raw(target.identity.device_id)
        })
        .collect();
    if valid.len() != before.len() {
        let requests: Vec<_> = valid
            .iter()
            .copied()
            .map(SpectrumTargetRequest::from)
            .collect();
        if let Err(error) = publish(view, &requests) {
            valid.clear();
            report_error(view, error);
        }
        changed = true;
    }
    let now = now(view);
    let count = view.device_panel.spectrum_results.len();
    view.device_panel
        .spectrum_results
        .retain(|_, result| valid.iter().any(|target| matches(result, *target, now)));
    changed |= count != view.device_panel.spectrum_results.len();
    if !view.clip_view_open || !view.device_panel_active {
        changed |= clear_status(view);
    }
    changed
}

fn context(view: &EditorView) -> Result<Option<(u64, u32)>, String> {
    #[cfg(all(test, feature = "ui-test-support"))]
    if view.device_panel.eq_spectrum.test_channel.is_some()
        || view.device_panel.eq_spectrum.test_client.is_some()
    {
        // Fixture admission only: a real target channel or guarded Client.
        // Neither path opens an OS audio device or supplies fabricated frames.
        return Ok(Some((1, 48_000)));
    }
    let Some(audio) = view.audio.as_ref() else {
        return Ok(None);
    };
    if let Some(error) = &audio.spectrum_error {
        return Err(format!("spectrum unavailable: {error}"));
    }
    if !(8_000..=192_000).contains(&audio.sample_rate) {
        return Err(
            target_error_message(TargetError::UnsupportedSampleRate(audio.sample_rate)).into(),
        );
    }
    let Some(state) = view.state.as_ref() else {
        return Ok(None);
    };
    // The analyzer follows the signal that reaches the output. A pending
    // or refused publication does not advance the installed graph's revision.
    let installed = audio
        .control
        .status()
        .and_then(|status| status.recording_source())
        .filter(|source| {
            source.context.project_generation == view.project_generation
                && audio.control.output_identity() == Some(source.output_identity)
        })
        .map(|source| source.audio_revision);
    let retained = || {
        desired(view)?
            .iter()
            .map(|(_, target)| target)
            .find(|target| {
                target.project_generation == view.project_generation
                    && target.sample_rate_hz == audio.sample_rate
            })
            .map(|target| target.activation_floor)
    };
    Ok(installed
        .or_else(retained)
        .or_else(|| {
            view.audio_admission
                .published_audio_revision(view.project_generation, state.revision)
        })
        .map(|floor| (floor, audio.sample_rate)))
}

/// Called once by the owning chain after its existing mount/culling decision.
/// Publishing one whole set avoids card paint order taking another card's tap.
pub(in crate::editor) fn sync_visible(
    view: &mut EditorView,
    identities: &[hex_engine::PostNodeIdentity],
) {
    reconcile(view);
    let identities: Vec<_> = identities
        .iter()
        .copied()
        .filter(|identity| {
            is_eq(identity.device_type)
                && view.spectrum_identity_is_visible_and_current(*identity, view.project_generation)
                && view
                    .device_panel
                    .eq_spectrum
                    .enabled_raw(identity.device_id)
        })
        .collect();
    if identities.is_empty() {
        clear(view);
        clear_status(view);
        return;
    }
    let (floor, rate) = match context(view) {
        Ok(Some(context)) => context,
        Ok(None) => {
            clear(view);
            if view
                .device_panel
                .eq_spectrum
                .status_message
                .as_deref()
                .is_some_and(|m| m != WAITING)
            {
                clear_status(view);
            }
            return;
        }
        Err(error) => {
            clear(view);
            report_status(view, error);
            return;
        }
    };
    let current = desired(view);
    let requests: Vec<_> = identities
        .into_iter()
        .map(|identity| {
            // Stable taps retain their FFT window across graph revisions. The
            // callback still observes only the exact node on the active graph.
            let previous = current
                .and_then(|targets| targets.find(identity))
                .map(|(_, target)| target)
                .filter(|target| {
                    target.project_generation == view.project_generation
                        && target.sample_rate_hz == rate
                });
            SpectrumTargetRequest {
                identity,
                project_generation: view.project_generation,
                activation_floor: previous
                    .map_or(floor, |target| floor.min(target.activation_floor)),
                sample_rate_hz: rate,
            }
        })
        .collect();
    match publish(view, &requests) {
        Ok(()) => {
            clear_status(view);
        }
        Err(error) => {
            clear(view);
            report_error(view, error);
        }
    }
}

pub(in crate::editor) fn toggle(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
    kind: hex_engine::DeviceTypeId,
) -> Stateful<Div> {
    reconcile(view);
    let enabled = view.device_panel.eq_spectrum.enabled(device);
    let generation = view.project_generation;
    let revision = view.state.as_ref().map(|state| state.revision);
    let identity = hex_engine::PostNodeIdentity::new(owner, device.value(), kind);
    text_segment_button(
        format!("eq8-spectrum-{}", device.value()),
        "spectrum",
        "show live spectrum for this eq; audio output must be available",
        theme::DeviceActionRole::Normal,
        enabled,
    )
    .debug_selector(move || format!("eq8-spectrum-{}", device.value()))
    .on_click(cx.listener(move |view, _, _, cx| {
        if generation != view.project_generation
            || view.state.as_ref().map(|state| state.revision) != revision
            || !view.spectrum_identity_is_visible_and_current(identity, generation)
        {
            view.device_panel.message = Some("spectrum target changed; select it again".into());
            cx.notify();
            return;
        }
        if view.device_panel.eq_spectrum.enabled(device) {
            view.device_panel
                .eq_spectrum
                .off
                .insert(device.value(), device);
            clear_status(view);
        } else {
            view.device_panel.eq_spectrum.off.remove(&device.value());
            if matches!(context(view), Ok(None)) {
                report_status(view, WAITING.into());
            }
        }
        reconcile(view);
        cx.notify();
    }))
}

pub(super) fn focus_listener(
    view: &EditorView,
    cx: &Context<EditorView>,
    device: DeviceId,
) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
    let entity = cx.weak_entity();
    let generation = view.project_generation;
    move |_, window, cx| {
        let _ = entity.update(cx, |view, cx| {
            if generation != view.project_generation
                || view
                    .device_panel
                    .eq_panels
                    .get(&device)
                    .is_none_or(|panel| panel.focus_subscription.is_some())
            {
                return;
            }
            let handle = view.device_panel.chain_window.focus(device, cx);
            let subscription = cx.on_focus_in(&handle, window, move |view, window, cx| {
                if view.project_generation == generation
                    && view.state.as_ref().is_some_and(|state| {
                        state
                            .project
                            .device(device)
                            .is_some_and(|device| device.state().type_id() == eq::EQ_TYPE_ID)
                    })
                {
                    view.device_panel.select_only(Some(device));
                    // Focus is delivered after paint; dirty the next frame,
                    // following the existing Operator focus subscription seam.
                    cx.defer_in(window, |_, _, cx| cx.notify());
                }
            });
            let graph_focus = view.device_panel.eq_panels[&device]
                .entity
                .read(cx)
                .focus_handle(cx);
            let blur = cx.on_blur(&graph_focus, window, move |view, _, _| {
                if view.project_generation == generation {
                    super::scroll_queue::cancel_queued(view, device);
                }
            });
            if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                panel.focus_subscription = Some(subscription);
                panel.scroll_blur_subscription = Some(blur);
            }
        });
    }
}
