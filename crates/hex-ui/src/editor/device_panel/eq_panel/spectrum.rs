//! View-local analyzer intent; the existing audio client still owns the sole tap.
use super::*;
use hex_audio::spectrum_target::{SpectrumTarget, TargetError};

const WAITING: &str = "spectrum waits for available current project audio";

#[derive(Default)]
pub(in super::super) struct SpectrumSelection {
    generation: u64,
    revision: Option<u64>,
    off: HashSet<DeviceId>,
    status_message: Option<&'static str>,
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in super::super) test_channel: Option<hex_audio::spectrum_target::SpectrumTargetControl>,
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in super::super) test_client: Option<hex_audio::prepared::Client>,
}

impl SpectrumSelection {
    pub(super) fn enabled(&self, device: DeviceId) -> bool {
        !self.off.contains(&device)
    }
}

// The shared panel status can also carry save/edit failures. Retire only the
// analyzer message that this intent owns, never an unrelated newer message.
fn clear_status(view: &mut EditorView) {
    let owned = view.device_panel.eq_spectrum.status_message.take();
    if view.device_panel.message.as_deref() == Some(WAITING)
        || owned.is_some_and(|message| view.device_panel.message.as_deref() == Some(message))
    {
        view.device_panel.message = None;
    }
}

fn report_error(view: &mut EditorView, error: TargetError) {
    let message = match error {
        TargetError::OutputClosed => "spectrum unavailable: audio output is closed",
        TargetError::InvalidDevice | TargetError::InvalidOwner | TargetError::WrongDeviceType => {
            "spectrum unavailable: selected eq target is invalid"
        }
        TargetError::InvalidActivationFloor => "spectrum waits for a valid audio publication",
        TargetError::UnsupportedSampleRate(_) => {
            "spectrum unavailable: unsupported audio sample rate"
        }
        TargetError::EpochExhausted | TargetError::TokenExhausted => {
            "spectrum unavailable: target identity exhausted; reopen audio output"
        }
    };
    view.device_panel.eq_spectrum.status_message = Some(message);
    view.device_panel.message = Some(message.into());
}

fn reconcile(view: &mut EditorView) {
    let invalid_target = desired(view).is_some_and(|target| {
        target.identity.device_type == eq::EQ_TYPE_ID
            && (target.project_generation != view.project_generation
                || !view.state.as_ref().is_some_and(|state| {
                    let matches = |device: &hex_project::Device| {
                        device.id().value() == target.identity.device_id
                            && device.state().type_id() == eq::EQ_TYPE_ID
                    };
                    match target.identity.owner {
                        hex_engine::ProcessorChainOwner::Track(id) => state
                            .project
                            .tracks()
                            .iter()
                            .find(|track| track.id().value() == id)
                            .is_some_and(|track| track.devices().iter().any(matches)),
                        hex_engine::ProcessorChainOwner::Return(id) => state
                            .project
                            .return_tracks()
                            .iter()
                            .find(|track| track.id().value() == id)
                            .is_some_and(|track| track.devices().iter().any(matches)),
                    }
                }))
    });
    if invalid_target {
        clear(view);
    }

    let intent = &mut view.device_panel.eq_spectrum;
    if intent.generation != view.project_generation {
        intent.generation = view.project_generation;
        intent.revision = None;
        intent.off.clear();
    }
    let Some(state) = view.state.as_ref() else {
        return;
    };
    if intent.revision != Some(state.revision) {
        intent.off.retain(|device| {
            state
                .project
                .device(*device)
                .is_some_and(|device| device.state().type_id() == eq::EQ_TYPE_ID)
        });
        intent.revision = Some(state.revision);
    }
}

fn desired(view: &EditorView) -> Option<SpectrumTarget> {
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(client) = &view.device_panel.eq_spectrum.test_client {
        return client.spectrum_desired();
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(channel) = &view.device_panel.eq_spectrum.test_channel {
        return channel.desired();
    }
    view.audio
        .as_ref()
        .and_then(|audio| audio.control.spectrum_desired())
}

fn context(view: &EditorView) -> Option<(u64, u32)> {
    #[cfg(all(test, feature = "ui-test-support"))]
    if view.device_panel.eq_spectrum.test_channel.is_some()
        || view.device_panel.eq_spectrum.test_client.is_some()
    {
        // Only the public control channel is injected, never a Client/backend.
        return Some((1, 48_000));
    }
    let audio = view.audio.as_ref()?;
    if audio.spectrum_error.is_some() || !(8_000..=192_000).contains(&audio.sample_rate) {
        return None;
    }
    let revision = view.state.as_ref()?.revision;
    let floor = view
        .audio_admission
        .published_audio_revision(view.project_generation, revision)?;
    Some((floor, audio.sample_rate))
}

fn clear(view: &mut EditorView) {
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(client) = &mut view.device_panel.eq_spectrum.test_client {
        let _ = client.spectrum_clear();
        view.device_panel.spectrum_result = None;
        return;
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(channel) = &mut view.device_panel.eq_spectrum.test_channel {
        let _ = channel.clear();
        view.device_panel.spectrum_result = None;
        return;
    }
    if let Some(audio) = view.audio.as_mut() {
        let _ = audio.control.spectrum_clear();
    }
    view.device_panel.spectrum_result = None;
}

fn select(
    view: &mut EditorView,
    identity: hex_engine::PostNodeIdentity,
    floor: u64,
    rate: u32,
) -> Result<(), TargetError> {
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(client) = &mut view.device_panel.eq_spectrum.test_client {
        client.spectrum_select(identity, view.project_generation, floor, rate)?;
        return Ok(());
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    if let Some(channel) = &mut view.device_panel.eq_spectrum.test_channel {
        channel.select(identity, view.project_generation, floor, rate)?;
        return Ok(());
    }
    let audio = view.audio.as_mut().ok_or(TargetError::OutputClosed)?;
    audio
        .control
        .spectrum_select(identity, view.project_generation, floor, rate)?;
    Ok(())
}

pub(super) fn sync_selected(
    view: &mut EditorView,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
) {
    reconcile(view);
    // Mounting/hovering a panel is never selection. Only the existing explicit
    // card selection or a real focus-entry event may choose this device.
    if view.device_panel.selected_device != Some(device) {
        return;
    }
    if !view.state.as_ref().is_some_and(|state| {
        state
            .project
            .device(device)
            .is_some_and(|device| device.state().type_id() == eq::EQ_TYPE_ID)
    }) {
        return;
    }
    if !view.device_panel.eq_spectrum.enabled(device) {
        if desired(view).is_some() {
            clear(view);
        }
        clear_status(view);
        return;
    }
    let Some((floor, rate)) = context(view) else {
        return;
    };
    let identity = hex_engine::PostNodeIdentity::new(owner, device.value(), eq::EQ_TYPE_ID);
    let current = desired(view);
    if current.is_some_and(|target| {
        target.identity == identity
            && target.project_generation == view.project_generation
            && target.activation_floor == floor
            && target.sample_rate_hz == rate
    }) {
        return;
    }
    // Both successful activation and refusal retire the previous tap's pixels.
    view.device_panel.spectrum_result = None;
    match select(view, identity, floor, rate) {
        Ok(()) => clear_status(view),
        Err(error) => report_error(view, error),
    }
}

pub(super) fn toggle(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
) -> Stateful<Div> {
    reconcile(view);
    let enabled = view.device_panel.eq_spectrum.enabled(device);
    let generation = view.project_generation;
    let revision = view.state.as_ref().map(|state| state.revision);
    text_action_button(
        format!("eq8-spectrum-{}", device.value()),
        "spectrum",
        "show live spectrum for the selected eq; audio output must be available",
        theme::DeviceActionRole::Normal,
        enabled,
    )
    .debug_selector(move || format!("eq8-spectrum-{}", device.value()))
    .on_click(cx.listener(move |view, _, _, cx| {
        if generation != view.project_generation
            || view.state.as_ref().map(|state| state.revision) != revision
        {
            view.device_panel.message = Some("spectrum target changed; select it again".into());
            cx.notify();
            return;
        }
        reconcile(view);
        if view.device_panel.eq_spectrum.enabled(device) {
            view.device_panel.eq_spectrum.off.insert(device);
            if desired(view).is_some_and(|target| {
                target.identity.device_id == device.value()
                    && target.project_generation == generation
            }) {
                clear(view);
            }
        } else {
            view.device_panel.eq_spectrum.off.remove(&device);
            view.device_panel.selected_device = Some(device);
            sync_selected(view, device, owner);
        }
        if view.device_panel.eq_spectrum.enabled(device) && context(view).is_none() {
            view.device_panel.message = Some(WAITING.into());
        } else if !view.device_panel.eq_spectrum.enabled(device) {
            clear_status(view);
        }
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
                    view.device_panel.selected_device = Some(device);
                    // Focus is delivered after paint; dirty the next frame,
                    // following the existing Operator focus subscription seam.
                    cx.defer_in(window, |_, _, cx| cx.notify());
                }
            });
            if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                panel.focus_subscription = Some(subscription);
            }
        });
    }
}
