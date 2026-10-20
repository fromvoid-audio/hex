//! View-local analyzer intent; the existing audio client still owns the sole tap.
use super::*;
use gpui::Focusable;
use hex_audio::spectrum_target::{SpectrumTarget, TargetError};
use hex_session::snapshot_exports::{
    SnapshotDeviceRead, SnapshotProjectRead, SnapshotTransportError,
};

const WAITING: &str = "spectrum waits for available current project audio";

#[derive(Default)]
pub(in super::super) struct SpectrumSelection {
    generation: u64,
    revision: Option<u64>,
    off: HashSet<DeviceId>,
    status_message: Option<String>,
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

#[derive(Debug)]
pub(in crate::editor) enum ReadError {
    Transport(SnapshotTransportError),
    ScratchUnavailable,
}
impl From<SnapshotTransportError> for ReadError {
    fn from(error: SnapshotTransportError) -> Self {
        Self::Transport(error)
    }
}

fn is_eq(device: SnapshotDeviceRead<'_>) -> bool {
    device.class() == NativeDeviceClass::Effect && device.type_id() == eq::EQ_TYPE_ID
}

// Exact membership in the authored effect chain, independent of audio liveness.
fn effect_identity_is_current(
    project: SnapshotProjectRead<'_>,
    identity: hex_engine::PostNodeIdentity,
) -> bool {
    let matches = |device: SnapshotDeviceRead<'_>| {
        device.class() == NativeDeviceClass::Effect
            && device.id().value() == identity.device_id
            && device.type_id() == identity.device_type
    };
    match identity.owner {
        hex_engine::ProcessorChainOwner::Main => project.main_devices().any(matches),
        hex_engine::ProcessorChainOwner::Track(id) => project
            .tracks()
            .find(|track| track.id().value() == id)
            .is_some_and(|track| track.devices().any(matches)),
        hex_engine::ProcessorChainOwner::Return(id) => project
            .returns()
            .find(|track| track.id().value() == id)
            .is_some_and(|track| track.devices().any(matches)),
    }
}

pub(in crate::editor) fn focused_effect_identity_is_current(
    view: &EditorView,
    project: SnapshotProjectRead<'_>,
    identity: hex_engine::PostNodeIdentity,
) -> bool {
    let focused = match (super::super::selected_owner(view, project), identity.owner) {
        (Some(DevicePanelOwner::Main), hex_engine::ProcessorChainOwner::Main) => true,
        (Some(DevicePanelOwner::Track(track)), hex_engine::ProcessorChainOwner::Track(id)) => {
            track.value() == id
        }
        (Some(DevicePanelOwner::Return(track)), hex_engine::ProcessorChainOwner::Return(id)) => {
            track.value() == id
        }
        _ => false,
    };
    focused && effect_identity_is_current(project, identity)
}

struct Prepared {
    revision: u64,
    reset_generation: bool,
    selected: Option<DeviceId>,
    invalid_target: bool,
    invalid_off: Vec<DeviceId>,
    action_current: bool,
}

// Allocate only enough scratch for the existing off set before entering the
// retained read. The guard fills Copy IDs without growth or HashSet mutation.
fn prepare(
    view: &EditorView,
    action: Option<hex_engine::PostNodeIdentity>,
) -> Result<Prepared, ReadError> {
    let off = &view.device_panel.eq_spectrum.off;
    if off.len() > hex_project::MAX_DEVICES {
        return Err(ReadError::ScratchUnavailable);
    }
    let mut invalid_off = Vec::new();
    invalid_off
        .try_reserve_exact(off.len())
        .map_err(|_| ReadError::ScratchUnavailable)?;
    let state = view.state.as_ref().ok_or(SnapshotTransportError::Stale)?;
    let target = desired(view);
    let reset_generation = view.device_panel.eq_spectrum.generation != view.project_generation;
    let facts = state.read_with(|snapshot| {
        let project = snapshot.model();
        let revision = snapshot.revision();
        let selected = view.device_panel.selected_device.filter(|device| {
            (reset_generation || !off.contains(device))
                && project.device(*device).is_some_and(is_eq)
        });
        let invalid_target = target.is_some_and(|target| {
            target.identity.device_type == eq::EQ_TYPE_ID
                && (selected.map(DeviceId::value) != Some(target.identity.device_id)
                    || target.project_generation != view.project_generation
                    || !effect_identity_is_current(project, target.identity))
        });
        if !reset_generation && view.device_panel.eq_spectrum.revision != Some(revision) {
            for device in off {
                if !project.device(*device).is_some_and(is_eq) {
                    if invalid_off.len() == off.len() || invalid_off.len() == invalid_off.capacity()
                    {
                        return None;
                    }
                    invalid_off.push(*device);
                }
            }
        }
        Some((
            revision,
            selected,
            invalid_target,
            action.is_some_and(|identity| {
                focused_effect_identity_is_current(view, project, identity)
            }),
        ))
    })?;
    let (revision, selected, invalid_target, action_current) =
        facts.ok_or(ReadError::ScratchUnavailable)?;
    Ok(Prepared {
        revision,
        reset_generation,
        selected,
        invalid_target,
        invalid_off,
        action_current,
    })
}

fn apply_reconcile(view: &mut EditorView, prepared: &Prepared) -> bool {
    let mut changed = false;
    if prepared.reset_generation {
        clear_status(view);
        let intent = &mut view.device_panel.eq_spectrum;
        intent.generation = view.project_generation;
        intent.revision = None;
        intent.off.clear();
        changed = true;
    }
    if prepared.selected.is_none() {
        changed |= clear_status(view);
    }
    if prepared.invalid_target {
        clear(view);
        changed = true;
    }
    let intent = &mut view.device_panel.eq_spectrum;
    for device in &prepared.invalid_off {
        intent.off.remove(device);
    }
    intent.revision = Some(prepared.revision);
    changed
}

pub(in crate::editor) fn reconcile_checked(view: &mut EditorView) -> Result<bool, ReadError> {
    let prepared = prepare(view, None)?;
    Ok(apply_reconcile(view, &prepared))
}

pub(in crate::editor) fn reconcile(view: &mut EditorView) -> bool {
    // A refused read cannot authorize cleanup. Production target retirement
    // uses the checked form before any destructive audio/UI effects.
    reconcile_checked(view).unwrap_or(false)
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

fn context(view: &EditorView, revision: u64) -> Result<Option<(u64, u32)>, String> {
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
    Ok(view
        .audio_admission
        .published_audio_revision(view.project_generation, revision)
        .map(|floor| (floor, audio.sample_rate)))
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
    let identity = hex_engine::PostNodeIdentity::new(owner, device.value(), eq::EQ_TYPE_ID);
    let Ok(prepared) = prepare(view, Some(identity)) else {
        return;
    };
    // Prepare audio-publication context too, before cleanup can change intent.
    let source_context = context(view, prepared.revision);
    apply_reconcile(view, &prepared);
    if prepared.action_current {
        sync_prepared(view, device, identity, source_context);
    }
}

fn sync_prepared(
    view: &mut EditorView,
    device: DeviceId,
    identity: hex_engine::PostNodeIdentity,
    source_context: Result<Option<(u64, u32)>, String>,
) {
    // Mounting/hovering a panel is never selection. Only the existing explicit
    // card selection or a real focus-entry event may choose this device.
    if view.device_panel.selected_device != Some(device) {
        return;
    }
    if !view.device_panel.eq_spectrum.enabled(device) {
        if desired(view).is_some_and(|target| {
            target.identity == identity && target.project_generation == view.project_generation
        }) {
            clear(view);
        }
        clear_status(view);
        return;
    }
    let (floor, rate) = match source_context {
        Ok(Some(context)) => context,
        Ok(None) => {
            // Recovery can retire this endpoint without replacing the project
            // or its selected EQ. Pending publication is not the old output's
            // setup/closed failure; retire only feedback owned by this analyzer.
            view.device_panel.spectrum_result = None;
            if view
                .device_panel
                .eq_spectrum
                .status_message
                .as_deref()
                .is_some_and(|message| message != WAITING)
            {
                clear_status(view);
            }
            return;
        }
        Err(error) => {
            view.device_panel.spectrum_result = None;
            report_status(view, error);
            return;
        }
    };
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
        Ok(()) => {
            clear_status(view);
        }
        Err(error) => report_error(view, error),
    }
}

pub(super) fn toggle(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
) -> Stateful<Div> {
    let identity = hex_engine::PostNodeIdentity::new(owner, device.value(), eq::EQ_TYPE_ID);
    let revision = match prepare(view, Some(identity)) {
        Ok(prepared) => {
            let revision = prepared.revision;
            apply_reconcile(view, &prepared);
            Some(revision)
        }
        Err(_) => None,
    };
    let enabled = view.device_panel.eq_spectrum.enabled(device);
    let generation = view.project_generation;
    text_action_button(
        format!("eq8-spectrum-{}", device.value()),
        "spectrum",
        "show live spectrum for the selected eq; audio output must be available",
        theme::DeviceActionRole::Normal,
        enabled,
    )
    .debug_selector(move || format!("eq8-spectrum-{}", device.value()))
    .on_click(cx.listener(move |view, _, _, cx| {
        // An initial render refusal does not bake a disabled callback into the
        // panel. The explicit click can capture fresh actual facts instead.
        let prepared = match prepare(view, Some(identity)) {
            Ok(prepared) => prepared,
            Err(ReadError::Transport(SnapshotTransportError::Busy)) => return,
            Err(_) => return,
        };
        if generation != view.project_generation
            || revision.is_some_and(|revision| prepared.revision != revision)
            || !prepared.action_current
        {
            view.device_panel.message = Some("spectrum target changed; select it again".into());
            cx.notify();
            return;
        }
        let source_context = context(view, prepared.revision);
        let waiting = matches!(&source_context, Ok(None));
        apply_reconcile(view, &prepared);
        if view.device_panel.eq_spectrum.enabled(device) {
            view.device_panel.eq_spectrum.off.insert(device);
            if desired(view).is_some_and(|target| {
                target.identity == identity && target.project_generation == generation
            }) {
                clear(view);
            }
        } else {
            view.device_panel.eq_spectrum.off.remove(&device);
            view.device_panel.selected_device = Some(device);
            sync_prepared(view, device, identity, source_context);
        }
        if view.device_panel.eq_spectrum.enabled(device) && waiting {
            report_status(view, WAITING.into());
        } else if !view.device_panel.eq_spectrum.enabled(device) {
            clear_status(view);
        }
        cx.notify();
    }))
}

// Capture the actual effect owner at prepaint, before installing subscriptions.
// A Busy capture leaves the subscription empty so a later prepaint can retry.
fn capture_owner(
    view: &EditorView,
    device: DeviceId,
) -> Result<Option<hex_engine::ProcessorChainOwner>, SnapshotTransportError> {
    let state = view.state.as_ref().ok_or(SnapshotTransportError::Stale)?;
    state.read_with(|snapshot| {
        let project = snapshot.model();
        if project
            .main_devices()
            .any(|item| item.id() == device && is_eq(item))
        {
            return Some(hex_engine::ProcessorChainOwner::Main);
        }
        if let Some(track) = project.tracks().find(|track| {
            track
                .devices()
                .any(|item| item.id() == device && is_eq(item))
        }) {
            return Some(hex_engine::ProcessorChainOwner::Track(track.id().value()));
        }
        project
            .returns()
            .find(|track| {
                track
                    .devices()
                    .any(|item| item.id() == device && is_eq(item))
            })
            .map(|track| hex_engine::ProcessorChainOwner::Return(track.id().value()))
    })
}

fn focus_owner_is_current(
    view: &EditorView,
    generation: u64,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
) -> Result<bool, SnapshotTransportError> {
    let state = view.state.as_ref().ok_or(SnapshotTransportError::Stale)?;
    state.read_with(|snapshot| {
        generation == view.project_generation
            && focused_effect_identity_is_current(
                view,
                snapshot.model(),
                hex_engine::PostNodeIdentity::new(owner, device.value(), eq::EQ_TYPE_ID),
            )
    })
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
            let Ok(Some(owner)) = capture_owner(view, device) else {
                return;
            };
            if !matches!(
                focus_owner_is_current(view, generation, device, owner),
                Ok(true)
            ) {
                return;
            }
            let handle = view.device_panel.chain_window.focus(device, cx);
            let subscription = cx.on_focus_in(&handle, window, move |view, window, cx| {
                // Devices can move between effect owners without changing their
                // ID/generation; do not cache an obsolete owner in this watch.
                let Ok(Some(owner)) = capture_owner(view, device) else {
                    return;
                };
                if matches!(
                    focus_owner_is_current(view, generation, device, owner),
                    Ok(true)
                ) {
                    view.device_panel.selected_device = Some(device);
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
                if generation == view.project_generation
                    && focus_owner_is_current(view, generation, device, owner).is_ok()
                {
                    // Both live and actually stale owners lose their queued
                    // scroll on blur. Refused reads cannot authorize cleanup.
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
