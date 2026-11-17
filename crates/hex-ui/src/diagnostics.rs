//! Control-side event forwarding; filesystem ownership stays in hex-diagnostics.
//!
//! Coverage: existing action handlers (no new dispatch listeners), pre-dispatch
//! editor keys including consumed shortcuts, root pointer down/up, and changes
//! observed at the existing 16 ms poll in track/clip/scene selection, mode,
//! viewport and browser query length. Loop refusals record bounded gate names
//! and section pointer captures record release/cancellation outcomes. Semantic
//! widget edits use exhaustive session request/command projections; transport
//! attempts retain actual tickets.
//! This is a causal aid, not lossless input replay: intermediate view states,
//! pointer moves/scroll deltas, OS file dialogs, text contents, and deferred-work
//! UI parents are not captured. Unknown parents remain None. Callback application
//! is a separate audio trace joined by output, context, revision and ticket.
//! Existing general status/error logs may include source-provided text/paths;
//! the new action projections exclude them. Loss counters and bounded retention
//! apply to every category; handlers never wait for file I/O.
use gpui::{App, BorrowAppContext, Global, PromptLevel, Window};
use hex_audio::audio_diagnostics::{AudioDiagnosticsFlags, MAX_DRAIN_PER_POLL};
use hex_diagnostics::{Category, DiagnosticsHandle, Level};
use hex_session::service::diagnostics::DiagnosticObserver;
use std::sync::{
    Arc,
    atomic::{
        AtomicU64,
        Ordering::{Acquire, Relaxed},
    },
};
static NEXT_ACTION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AudioSnapshot {
    output: u64,
    applied: u64,
    command: u64,
    stream_applied_command: u64,
    rejected_action: Option<(u64, hex_engine::PlaybackActionRejection)>,
    playing: bool,
    rejected: Option<(u64, hex_engine::PlaybackHandoffRejection)>,
    buffering: bool,
    readiness_refused: bool,
}

struct SessionDiagnostics {
    audio_flags: AudioDiagnosticsFlags,
    action_observer: Option<Arc<DiagnosticObserver>>,
    audio_snapshot: Option<AudioSnapshot>,
    audio_losses: Option<(u64, u64, u64)>,
    handle: Result<DiagnosticsHandle, String>,
    last_view: String,
    /// Retained buffer for the next view snapshot; reused on every tick.
    view_scratch: String,
    last_status: String,
    last_error: Option<String>,
    reported_failure: bool,
}
impl Global for SessionDiagnostics {}

/// Inject the desktop-owned session logger without making views own its worker.
pub fn install_session_log(cx: &mut App, handle: Result<DiagnosticsHandle, String>) {
    cx.set_global(SessionDiagnostics {
        audio_flags: AudioDiagnosticsFlags::OFF,
        action_observer: None,
        audio_snapshot: None,
        audio_losses: None,
        handle,
        last_view: String::new(),
        view_scratch: String::new(),
        last_status: String::new(),
        last_error: None,
        reported_failure: false,
    });
}

/// Select detailed audio tracing after installing the desktop-owned log.
/// Startup-only application configuration; it grants no audio/source authority.
/// `--audio-diagnostics` supplies these flags in the supported desktop entrypoint.
pub fn install_audio_diagnostics(cx: &mut App, flags: AudioDiagnosticsFlags) {
    if cx.try_global::<SessionDiagnostics>().is_none() {
        return;
    }
    let diagnostics = cx.global_mut::<SessionDiagnostics>();
    diagnostics.audio_flags = flags;
    if flags.enabled()
        && let Ok(handle) = &diagnostics.handle
    {
        let _ = handle.record_typed(Level::Info, Category::Audio, &format!("audio_diag v=1 stage=configuration build={:?} flags={flags:?} capacity={} drain_budget={} receipt_time=control", crate::build_info::LABEL,
            hex_audio::audio_diagnostics::TRACE_CAPACITY, MAX_DRAIN_PER_POLL));
    }
}

/// Opt-in action intent and canonical service outcomes. No user text, paths or
/// clipboard content is captured. Direct widgets still produce canonical requests;
/// this wrapper observes existing handlers without changing action availability.
pub fn install_action_diagnostics(cx: &mut App, enabled: bool) {
    let Some(config) = cx.try_global::<SessionDiagnostics>() else {
        return;
    };
    let observer = enabled.then(|| config.handle.as_ref().ok().cloned()).flatten().map(|handle| {
        Arc::new(DiagnosticObserver::new(move |event| {
            let _ = handle.record_typed(Level::Debug, Category::Session, &format!("action_diag v=1 stage=session service={} request={:?} observation={:?} details_truncated={}{}",
                event.service_id, event.request, event.stage, event.details_truncated, event.details));
        }))
    });
    cx.global_mut::<SessionDiagnostics>().action_observer = observer;
}

pub(crate) fn attach_service(cx: &App, service: &hex_session::service::Service) {
    if let Some(observer) = cx
        .try_global::<SessionDiagnostics>()
        .and_then(|d| d.action_observer.clone())
    {
        let _ = service.set_diagnostic_observer(observer);
    }
}

pub(crate) trait ActionTraceContext<T> {
    fn trace<A: gpui::Action>(
        &self,
        handler: impl Fn(&mut T, &A, &mut Window, &mut gpui::Context<T>) + 'static,
    ) -> impl Fn(&A, &mut Window, &mut App) + 'static;
}
impl<T: 'static> ActionTraceContext<T> for gpui::Context<'_, T> {
    fn trace<A: gpui::Action>(
        &self,
        handler: impl Fn(&mut T, &A, &mut Window, &mut gpui::Context<T>) + 'static,
    ) -> impl Fn(&A, &mut Window, &mut App) + 'static {
        self.listener(move |view, action: &A, window, cx| {
            let _scope = action_scope(cx, action.name());
            handler(view, action, window, cx);
        })
    }
}

pub(crate) fn action_scope(
    cx: &App,
    name: &'static str,
) -> Option<hex_session::service::diagnostics::DiagnosticActionScope> {
    let config = cx.try_global::<SessionDiagnostics>()?;
    let observer = config.action_observer.as_ref()?;
    let id = NEXT_ACTION
        .fetch_update(Relaxed, Relaxed, |n| n.checked_add(1))
        .ok()?
        .checked_add(1)?;
    if let Ok(handle) = &config.handle {
        let _ = handle.record_typed(
            Level::Debug,
            Category::Ui,
            &format!("action_diag v=1 stage=handler action_id={id} action={name:?}"),
        );
    }
    Some(observer.enter_ui_action(id))
}

/// Called by the pre-dispatch interceptor, including keys consumed by custom
/// editors. Ordinary text is counted, never recorded; action names are static.
pub(crate) fn record_key(cx: &App, event: &gpui::KeystrokeEvent) {
    let Some(config) = cx
        .try_global::<SessionDiagnostics>()
        .filter(|d| d.action_observer.is_some())
    else {
        return;
    };
    if let Ok(handle) = &config.handle {
        let key = match event.keystroke.key.as_str() {
            "space" | "enter" | "escape" | "tab" | "backspace" | "delete" | "left" | "right"
            | "up" | "down" => event.keystroke.key.as_str(),
            _ => "redacted",
        };
        let _ = handle.record_typed(Level::Debug, Category::Ui, &format!("action_diag v=1 stage=key key={key:?} key_bytes={} modifiers={:?} resolved_action={:?} context_depth={}",
            event.keystroke.key.len(), event.keystroke.modifiers, event.action.as_ref().map(|a| a.name()), event.context_stack.len()));
    }
}

/// Control-thread time to install one received session outcome into the view
/// (`Editor::receive`), logged after that event's `ClientReceived` line. Paint
/// and present are not included.
pub(crate) fn record_ui_install(cx: &App, revision: u64, install: std::time::Duration) {
    if let Some(Ok(handle)) = cx
        .try_global::<SessionDiagnostics>()
        .filter(|d| d.action_observer.is_some())
        .map(|d| &d.handle)
    {
        let _ = handle.record_typed(
            Level::Debug,
            Category::Ui,
            &format!(
                "action_diag v=1 stage=ui_installed revision={revision} install_us={}",
                install.as_micros()
            ),
        );
    }
}

/// Selection counts and focus flags that route `g`; no names or project content.
pub(crate) fn record_group_selection(cx: &App, route: &str) {
    if let Some(Ok(handle)) = cx
        .try_global::<SessionDiagnostics>()
        .filter(|d| d.action_observer.is_some())
        .map(|d| &d.handle)
    {
        let _ = handle.record_typed(
            Level::Debug,
            Category::Ui,
            &format!("action_diag v=1 stage=group_selection {route}"),
        );
    }
}

/// Correlates the catalog class with its insertion terminal using the original
/// request context. Includes bounded plug-in metadata, never its path or state.
pub(crate) struct PluginLoad {
    handle: DiagnosticsHandle,
    details: String,
}

impl PluginLoad {
    pub(crate) fn new(
        cx: &App,
        name: &str,
        context: hex_engine::AudioSourceContext,
        destination: hex_session::service::PluginEffectDestination,
        rate: u32,
    ) -> Option<Self> {
        let config = cx.try_global::<SessionDiagnostics>()?;
        config.action_observer.as_ref()?;
        let handle = config.handle.as_ref().ok()?.clone();
        let name: String = name.chars().take(128).collect();
        let details = format!(
            "class={name:?} generation={} revision={} rate_hz={rate} destination={destination:?}",
            context.project_generation, context.editor_revision,
        );
        Some(Self { handle, details })
    }

    pub(crate) fn submitted(&self) {
        self.record("submitted");
    }

    pub(crate) fn complete(&self, accepted: bool) {
        self.record(if accepted { "accepted" } else { "refused" });
    }

    fn record(&self, stage: &'static str) {
        let _ = self.handle.record_typed(
            Level::Debug,
            Category::Ui,
            &format!("vst3_load v=1 stage={stage} {}", self.details),
        );
    }
}

/// One bounded control-side record per refused loop action; labels are static
/// and contain no project names, paths, or user-entered text.
pub(crate) fn record_loop_refusal(
    cx: &App,
    gates: &[(&'static str, bool)],
    section_view_blocked: bool,
    worker_pending: usize,
    queued_pending: usize,
    section_pending: bool,
    stopped_edit_pending: bool,
) {
    if let Some(diagnostics) = cx.try_global::<SessionDiagnostics>()
        && let (Ok(handle), Some(observer)) = (&diagnostics.handle, &diagnostics.action_observer)
    {
        let active = gates
            .iter()
            .filter_map(|(name, active)| active.then_some(*name))
            .collect::<Vec<_>>()
            .join(",");
        let _ = handle.record_typed(Level::Debug, Category::Ui, &format!(
            "action_diag v=1 stage=loop_refused gates={active} section_view_blocked={section_view_blocked} worker_pending={worker_pending} queued_pending={queued_pending} section_pending={section_pending} stopped_edit_pending={stopped_edit_pending} ui_action_id={:?}",
            observer.current_ui_action()
        ));
    }
}

/// Capture terminal events omit pointer coordinates and source data.
pub(crate) fn record_section_capture(cx: &App, outcome: &'static str) {
    if let Some(Ok(handle)) = cx
        .try_global::<SessionDiagnostics>()
        .filter(|d| d.action_observer.is_some())
        .map(|d| &d.handle)
    {
        let _ = handle.record_typed(
            Level::Debug,
            Category::Ui,
            &format!("action_diag v=1 stage=section_capture outcome={outcome}"),
        );
    }
}

pub(crate) fn actions_enabled(cx: &App) -> bool {
    cx.try_global::<SessionDiagnostics>()
        .is_some_and(|d| d.action_observer.is_some())
}

/// Record the editor view snapshot when it changes. `write` fills a retained
/// buffer, so an unchanged steady-state tick allocates nothing here.
pub(crate) fn record_view(cx: &mut App, write: impl FnOnce(&mut String)) {
    if !actions_enabled(cx) {
        return;
    }
    let config = cx.global_mut::<SessionDiagnostics>();
    let mut snapshot = std::mem::take(&mut config.view_scratch);
    snapshot.clear();
    write(&mut snapshot);
    if config.last_view != snapshot
        && let Ok(handle) = &config.handle
        && handle
            .record_typed(
                Level::Debug,
                Category::Ui,
                &format!("action_diag v=1 stage=view_observed{snapshot}"),
            )
            .is_ok()
    {
        std::mem::swap(&mut config.last_view, &mut snapshot);
    }
    config.view_scratch = snapshot;
}

pub(crate) fn record_pointer(
    cx: &App,
    stage: &str,
    button: gpui::MouseButton,
    position: gpui::Point<gpui::Pixels>,
    modifiers: gpui::Modifiers,
) {
    if !actions_enabled(cx) {
        return;
    }
    if let Some(config) = cx.try_global::<SessionDiagnostics>()
        && let Ok(handle) = &config.handle
    {
        let _ = handle.record_typed(Level::Debug, Category::Ui, &format!("action_diag v=1 stage={stage} button={button:?} position={position:?} modifiers={modifiers:?}"));
    }
}

#[derive(Clone)]
pub(crate) struct TransportTrace(DiagnosticsHandle, Arc<DiagnosticObserver>);
impl TransportTrace {
    pub(crate) fn from_app(cx: &App) -> Option<Self> {
        let config = cx.try_global::<SessionDiagnostics>()?;
        let observer = config.action_observer.as_ref()?;
        config
            .handle
            .as_ref()
            .ok()
            .cloned()
            .map(|handle| Self(handle, Arc::clone(observer)))
    }
    pub(crate) fn attempt(
        &self,
        audio: &mut crate::EditorAudio,
        action: hex_engine::PlaybackAction,
    ) -> Result<u64, hex_engine::PlaybackAction> {
        let result = audio.control.action_with_ticket(action);
        let _ = self.0.record_typed(if result.is_ok() { Level::Debug } else { Level::Warn }, Category::Transport, &format!("action_diag v=1 stage=transport_attempt ui_action_id={:?} output={:?} action={action:?} ticket={:?} accepted={}", self.1.current_ui_action(), audio.control.output_identity(), result.as_ref().ok(), result.is_ok()));
        result
    }
    pub(crate) fn stop(&self, audio: &crate::EditorAudio) {
        let _ = self.0.record_typed(
            Level::Debug,
            Category::Transport,
            &format!(
                "action_diag v=1 stage=priority_stop_requested ui_action_id={:?} output={:?}",
                self.1.current_ui_action(),
                audio.control.output_identity()
            ),
        );
    }
}

/// Consume actual callback observations on the existing UI/control poll. No
/// native/project owner escapes and a slow log can only lose diagnostics.
pub(crate) fn record_audio(cx: &mut App, audio: Option<&mut crate::EditorAudio>) {
    let Some(config) = cx.try_global::<SessionDiagnostics>() else {
        return;
    };
    let flags = config.audio_flags;
    if !flags.enabled() {
        return;
    }
    let Some(audio) = audio else {
        return;
    };
    audio.control.set_audio_diagnostics(flags);
    let Some(status) = audio.status() else {
        return;
    };
    let output = audio.control.output_identity().unwrap_or(0);
    let snapshot = AudioSnapshot {
        output,
        applied: status.applied_revision.load(Acquire),
        command: status.consumed_command.load(Acquire),
        stream_applied_command: status.stream_applied_command_prefix.load(Acquire),
        rejected_action: status.rejected_action(),
        playing: status.playing.load(Acquire),
        rejected: status.rejected_revision(),
        buffering: status.stream_buffering.load(Acquire),
        readiness_refused: status.stream_readiness_refused.load(Acquire),
    };
    let losses = status.audio_diagnostics_dropped();
    let diagnostics = cx.global_mut::<SessionDiagnostics>();
    let Ok(handle) = &diagnostics.handle else {
        for _ in 0..MAX_DRAIN_PER_POLL {
            if audio.control.take_audio_diagnostic().is_none() {
                break;
            }
        }
        return;
    };
    for _ in 0..MAX_DRAIN_PER_POLL {
        let Some(event) = audio.control.take_audio_diagnostic() else {
            break;
        };
        let _ = handle.record_typed(
            Level::Debug,
            Category::Audio,
            &format!("audio_diag v=1 stage=callback event={event:?}"),
        );
    }
    if diagnostics.audio_snapshot != Some(snapshot)
        && handle
            .record_typed(
                Level::Debug,
                Category::Audio,
                &format!("audio_diag v=1 stage=observed_status snapshot={snapshot:?}"),
            )
            .is_ok()
    {
        diagnostics.audio_snapshot = Some(snapshot);
    }
    let lost = (output, losses, handle.overflow_count());
    if diagnostics.audio_losses != Some(lost)
        && handle
            .record_typed(if lost.1 > 0 || lost.2 > 0 { Level::Warn } else { Level::Debug }, Category::Audio, &format!(
                "audio_diag v=1 stage=loss output={output} callback_dropped={} logger_dropped={}",
                lost.1, lost.2
            ))
            .is_ok()
    {
        diagnostics.audio_losses = Some(lost);
    }
}

/// Worker event receipt is distinct from actual callback application. Retain
/// exact resource-free correlation and avoid formatting private paths/errors.
pub(crate) fn record_audio_backend_event(cx: &mut App, event: &crate::EditorAudioEvent) {
    use crate::{EditorAudioEvent as E, EditorPublicationState as P};
    let Some(diagnostics) = cx.try_global::<SessionDiagnostics>() else {
        return;
    };
    if !diagnostics
        .audio_flags
        .contains(AudioDiagnosticsFlags::PUBLICATION)
    {
        return;
    }
    let Ok(handle) = &diagnostics.handle else {
        return;
    };
    let level = match event {
        E::Unavailable { .. }
        | E::Publication {
            state: P::Fault(_), ..
        } => Level::Error,
        E::PluginParameterTouched { .. } => Level::Debug,
        E::DegradedPlugins { .. }
        | E::Publication {
            state: P::Refused(_) | P::Busy,
            ..
        } => Level::Warn,
        _ => Level::Info,
    };
    let details = match event {
        E::Opened {
            project_generation,
            restart_request,
            audio,
        } => format!(
            "stage=output_opened generation={project_generation} restart={restart_request:?} output={:?} rate={} mode={:?}",
            audio.control.output_identity(),
            audio.sample_rate,
            audio.mode
        ),
        E::Unavailable {
            project_generation,
            restart_request,
            ..
        } => format!(
            "stage=output_unavailable generation={project_generation} restart={restart_request:?}"
        ),
        E::RelinquishClient {
            old_output_identity,
            target,
        } => format!("stage=output_relinquish output={old_output_identity} target={target:?}"),
        E::OutputRebound {
            project_generation,
            output_identity,
        } => {
            format!("stage=output_rebound generation={project_generation} output={output_identity}")
        }
        E::OutputReleased {
            project_generation,
            old_output_identity,
        } => format!(
            "stage=output_released generation={project_generation} output={old_output_identity}"
        ),
        E::OutputRouteChanged { output_identity } => {
            format!("stage=output_route_changed output={output_identity}")
        }
        E::NativeLatencyRestart { source } => {
            format!("stage=native_latency_restart source={source:?}")
        }
        E::PluginParameterTouched {
            source,
            binding,
            device,
            parameter,
            sequence,
            ..
        } => {
            format!(
                "stage=plugin_parameter_touched source={source:?} binding={binding:?} device={device} parameter={parameter} sequence={sequence}"
            )
        }
        E::PluginTransportToggle { project_generation } => {
            format!("stage=plugin_transport_toggle generation={project_generation}")
        }
        E::PluginEditorWindows {
            project_generation,
            output_identity,
            devices,
        } => {
            format!(
                "stage=plugin_editor_windows generation={project_generation} output={output_identity} devices={devices:?}"
            )
        }
        E::DegradedPlugins {
            project_generation,
            output_identity,
            devices,
        } => format!(
            "stage=degraded_plugins generation={project_generation} output={output_identity} devices={devices:?}"
        ),
        E::Publication {
            context,
            output_identity,
            state,
        } => {
            let stage = match state {
                P::Preparing => "preparing",
                P::Busy => "backpressure",
                P::Submitted { .. }
                | P::VerifiedPreviewSubmitted { .. }
                | P::VerifiedArrangementGainSubmitted { .. }
                | P::ResidentSubmitted { .. } => "submitted",
                P::Refused(_) | P::VerifiedArrangementGainRefused(_) => "refused",
                P::Fault(_) => "fault",
            };
            let (revision, preview, seek) = match state {
                P::Submitted { audio_revision }
                | P::VerifiedArrangementGainSubmitted { audio_revision } => {
                    (Some(*audio_revision), None, None)
                }
                P::VerifiedPreviewSubmitted {
                    audio_revision,
                    identity,
                } => (Some(*audio_revision), Some(*identity), None),
                P::ResidentSubmitted {
                    audio_revision,
                    metadata,
                    ..
                } => (
                    Some(*audio_revision),
                    metadata.device_preview,
                    metadata.seek_request,
                ),
                _ => (None, None, None),
            };
            format!(
                "stage=worker_{stage} output={output_identity} context={context:?} audio_revision={revision:?} preview={preview:?} seek={seek:?}"
            )
        }
    };
    let _ = handle.record_typed(level, Category::Audio, &format!("audio_diag v=1 {details}"));
}

/// Open the active session in the native, control-side log viewer.
pub(crate) fn open_session_log(window: &mut Window, cx: &mut App) {
    let Some(diagnostics) = cx.try_global::<SessionDiagnostics>() else {
        show_open_error(window, cx, "session logging is unavailable");
        return;
    };
    let path = match &diagnostics.handle {
        Ok(handle) => handle.path(),
        Err(error) => {
            let error = error.clone();
            show_open_error(window, cx, &error);
            return;
        }
    };
    if let Err(error) = crate::log_viewer::open(path, cx) {
        show_open_error(window, cx, &error);
    }
}

fn show_open_error(window: &mut Window, cx: &mut App, error: &str) {
    let answer = crate::theme::text("ok");
    let answers: [&str; 1] = [answer.as_ref()];
    drop(window.prompt(
        PromptLevel::Warning,
        &crate::theme::text("could not open session log"),
        Some(&crate::theme::text(error)),
        &answers,
        cx,
    ));
}

/// The installed session log file, when logging started.
pub(crate) fn log_path(cx: &App) -> Option<std::path::PathBuf> {
    cx.try_global::<SessionDiagnostics>()?
        .handle
        .as_ref()
        .ok()
        .map(DiagnosticsHandle::path)
}

pub(crate) fn record_state(cx: &mut App, status: &str, error: Option<&str>) -> Option<String> {
    cx.try_global::<SessionDiagnostics>()?;
    cx.update_global::<SessionDiagnostics, _>(|diagnostics, _| {
        let failure = match &diagnostics.handle {
            Ok(handle) => {
                if status != diagnostics.last_status || error != diagnostics.last_error.as_deref() {
                    let event = format!("status={status}; error={}", error.unwrap_or("none"));
                    if handle
                        .record_typed(
                            if error.is_some() {
                                Level::Error
                            } else {
                                Level::Info
                            },
                            Category::Ui,
                            &event,
                        )
                        .is_ok()
                    {
                        diagnostics.last_status = status.to_owned();
                        diagnostics.last_error = error.map(str::to_owned);
                    }
                }
                handle.failure()
            }
            Err(error) => Some(error.clone()),
        };
        if diagnostics.reported_failure {
            None
        } else {
            diagnostics.reported_failure = failure.is_some();
            failure.map(|error| format!("session logging unavailable: {error}"))
        }
    })
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;

    #[gpui::test]
    fn loop_and_capture_diagnostics_respect_configuration_and_report_active_gates(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory =
            std::env::temp_dir().join(format!("hex-ui-loop-gate-test-{}", std::process::id()));
        let mut log = hex_diagnostics::SessionLog::start(directory).unwrap();
        cx.update(|cx| {
            install_session_log(cx, Ok(log.handle()));
            install_action_diagnostics(cx, false);
            record_loop_refusal(cx, &[("disabled", true)], false, 0, 0, false, false);
            record_section_capture(cx, "disabled");
            install_action_diagnostics(cx, true);
            record_loop_refusal(
                cx,
                &[
                    ("section_held", true),
                    ("scrub", false),
                    ("edit_pending", true),
                ],
                true,
                2,
                3,
                true,
                false,
            );
            record_section_capture(cx, "cancel");
        });
        log.shutdown().unwrap();
        let contents = std::fs::read_to_string(log.path()).unwrap();
        assert!(!contents.contains("disabled"));
        assert!(contents.contains("stage=loop_refused gates=section_held,edit_pending"));
        assert!(contents.contains("section_view_blocked=true worker_pending=2 queued_pending=3 section_pending=true stopped_edit_pending=false"));
        assert!(contents.contains("stage=section_capture outcome=cancel"));
    }

    #[gpui::test]
    fn view_snapshot_reuses_its_buffer_and_records_only_changes(cx: &mut gpui::TestAppContext) {
        let directory = std::env::temp_dir().join(format!(
            "hex-ui-view-diagnostic-test-{}",
            std::process::id()
        ));
        let mut log = hex_diagnostics::SessionLog::start(directory).unwrap();
        cx.update(|cx| {
            install_session_log(cx, Ok(log.handle()));
            install_action_diagnostics(cx, true);
            let write = |text: &'static str| move |buffer: &mut String| buffer.push_str(text);
            record_view(cx, write(" view=a"));
            record_view(cx, write(" view=a"));
            let warmed = cx.global::<SessionDiagnostics>().view_scratch.as_ptr();
            for _ in 0..3 {
                record_view(cx, write(" view=a"));
            }
            assert_eq!(
                cx.global::<SessionDiagnostics>().view_scratch.as_ptr(),
                warmed,
                "unchanged snapshots fill the same retained buffer"
            );
            record_view(cx, write(" view=b"));
            record_view(cx, write(" view=b"));
            assert_eq!(cx.global::<SessionDiagnostics>().last_view, " view=b");
        });
        log.shutdown().unwrap();
        let contents = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(contents.matches("stage=view_observed view=a").count(), 1);
        assert_eq!(contents.matches("stage=view_observed view=b").count(), 1);
    }

    #[gpui::test]
    fn real_refused_gain_is_recorded_with_request_terminal_and_error(
        cx: &mut gpui::TestAppContext,
    ) {
        fn next(service: &hex_session::service::Service) -> hex_session::service::Event {
            let deadline = crate::test_wait::hang_deadline();
            loop {
                if let Ok(event) = service.try_event() {
                    return event;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "session event timed out"
                );
                std::thread::yield_now();
            }
        }
        let directory = std::env::temp_dir().join(format!(
            "hex-ui-gain-diagnostic-test-{}",
            std::process::id()
        ));
        let mut log = hex_diagnostics::SessionLog::start(directory.clone()).unwrap();
        let service = hex_session::service::Service::new(48_000, 8, 0).unwrap();
        next(&service);
        service
            .submit(hex_session::service::Request::Edit(
                hex_project::Command::AddTrack {
                    name: "fixture".into(),
                },
            ))
            .unwrap();
        let initial = next(&service);
        let track = initial.state.project.tracks()[0].id();
        cx.update(|cx| {
            install_session_log(cx, Ok(log.handle()));
            install_action_diagnostics(cx, false);
            assert!(action_scope(cx, "gain_fixture").is_none());
            install_action_diagnostics(cx, true);
            attach_service(cx, &service);
            let _action = action_scope(cx, "gain_fixture").unwrap();
            service
                .submit(hex_session::service::Request::Edit(
                    hex_project::Command::Gain {
                        track,
                        gain: f32::NAN,
                    },
                ))
                .unwrap();
        });
        let refused = next(&service);
        assert!(refused.request_complete);
        assert_eq!(refused.state.revision, initial.state.revision);
        let error = refused.error.as_deref().expect("gain refusal");
        cx.update(|cx| assert!(record_state(cx, "gain edit refused", Some(error)).is_none()));
        let stats = log.handle().statistics();
        assert_eq!((stats.queue_full, stats.closed), (0, 0));
        log.shutdown().unwrap();
        let contents = std::fs::read_to_string(log.path()).unwrap();
        let action = contents
            .split("stage=handler action_id=")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        assert!(contents.contains(&format!("ui_action_id: Some({action})")));
        assert!(contents.contains("operation: Some(\\\"gain\\\")"));
        assert!(contents.contains("SubmitAttempt"));
        assert!(contents.contains("WorkerBegin"));
        assert!(contents.contains("ClientReceived"));
        assert!(contents.contains("error: true"));
        assert!(contents.contains(error));
        assert!(contents.contains("gain edit refused"));
        assert!(
            contents
                .lines()
                .any(|line| line.contains("level=debug category=session")
                    && line.contains("stage=session"))
        );
        assert!(
            contents
                .lines()
                .any(|line| line.contains("level=debug category=ui")
                    && line.contains("stage=handler"))
        );
        assert!(
            contents
                .lines()
                .any(|line| line.contains("level=error category=ui")
                    && line.contains("gain edit refused"))
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn key_diagnostics_redact_typed_unicode_and_keep_control_keys(cx: &mut gpui::TestAppContext) {
        let directory =
            std::env::temp_dir().join(format!("hex-ui-key-redaction-test-{}", std::process::id()));
        let mut log = hex_diagnostics::SessionLog::start(directory.clone()).unwrap();
        cx.update(|cx| {
            install_session_log(cx, Ok(log.handle()));
            install_action_diagnostics(cx, true);
            for (key, text) in [("q", Some("秘密🎵")), ("enter", None)] {
                record_key(
                    cx,
                    &gpui::KeystrokeEvent {
                        keystroke: gpui::Keystroke {
                            key: key.into(),
                            key_char: text.map(str::to_owned),
                            modifiers: gpui::Modifiers::default(),
                        },
                        action: None,
                        context_stack: Vec::new(),
                    },
                );
            }
        });
        log.shutdown().unwrap();
        let contents = std::fs::read_to_string(log.path()).unwrap();
        assert!(contents.contains("key=\\\"redacted\\\""));
        assert!(contents.contains("key=\\\"enter\\\""));
        assert!(!contents.contains("秘密"));
        assert!(!contents.contains("🎵"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn detailed_audio_logs_are_opt_in_correlated_and_category_filtered(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory =
            std::env::temp_dir().join(format!("hex-ui-audio-diag-test-{}", std::process::id()));
        let mut log = hex_diagnostics::SessionLog::start(directory.clone()).unwrap();
        let event = crate::EditorAudioEvent::Publication {
            context: hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 42,
            },
            output_identity: 99,
            state: crate::EditorPublicationState::Submitted {
                audio_revision: 315,
            },
        };
        cx.update(|cx| {
            install_session_log(cx, Ok(log.handle()));
            record_audio_backend_event(cx, &event);
            install_audio_diagnostics(cx, AudioDiagnosticsFlags::TIMING);
            record_audio_backend_event(cx, &event);
            install_audio_diagnostics(cx, AudioDiagnosticsFlags::PUBLICATION);
            record_audio_backend_event(cx, &event);
            record_audio_backend_event(
                cx,
                &crate::EditorAudioEvent::Publication {
                    context: hex_engine::AudioSourceContext {
                        project_generation: 7,
                        editor_revision: 43,
                    },
                    output_identity: 99,
                    state: crate::EditorPublicationState::Refused("private fixture path".into()),
                },
            );
            install_audio_diagnostics(cx, AudioDiagnosticsFlags::OFF);
            record_audio_backend_event(cx, &event);
        });
        log.shutdown().unwrap();
        let contents = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(contents.matches("stage=worker_submitted").count(), 1);
        assert!(contents.contains("output=99"));
        assert!(contents.contains("project_generation: 7, editor_revision: 42"));
        assert!(contents.contains("audio_revision=Some(315)"));
        assert!(contents.contains("stage=worker_refused"));
        assert!(
            contents
                .lines()
                .any(|line| line.contains("level=info category=audio")
                    && line.contains("stage=worker_submitted"))
        );
        assert!(
            contents
                .lines()
                .any(|line| line.contains("level=warn category=audio")
                    && line.contains("stage=worker_refused"))
        );
        assert!(!contents.contains("private fixture path"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn fault_context_reaches_session_log_once_and_failure_is_reported_once(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory =
            std::env::temp_dir().join(format!("hex-ui-session-log-test-{}", std::process::id()));
        let mut log = hex_diagnostics::SessionLog::start(directory.clone()).expect("test log");
        let fault =
            Some("audio stopped · invalid event batch · frame 128 · audio rev 3".to_owned());
        cx.update(|cx| {
            install_session_log(cx, Ok(log.handle()));
            assert!(record_state(cx, "audio render fault · stopped", fault.as_deref()).is_none());
            assert!(record_state(cx, "audio render fault · stopped", fault.as_deref()).is_none());
        });
        log.shutdown().expect("flush log");
        let contents = std::fs::read_to_string(log.path()).expect("read log");
        assert_eq!(contents.matches("invalid event batch").count(), 1);
        assert!(contents.contains("frame 128 · audio rev 3"));
        cx.update(|cx| {
            install_session_log(cx, Err("disk unavailable".to_owned()));
            assert_eq!(
                record_state(cx, "stopped", fault.as_deref()).as_deref(),
                Some("session logging unavailable: disk unavailable")
            );
            assert!(record_state(cx, "stopped", fault.as_deref()).is_none());
        });
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }
}
