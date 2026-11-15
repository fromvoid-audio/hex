use super::EditorView;
use gpui::{App, AppContext, Context, Global, PromptButton, PromptLevel, Window};
use std::cell::Cell;
use std::time::Duration;

/// One UI-owned close decision. Freeze gesture admission while draining the
/// existing FIFO, awaiting Session's ordered custody read, and showing consent.
/// The five-second drain/read bound refuses close; it never discards a take.
#[derive(Default)]
pub(super) struct AutomationClosePreflight {
    state: PreflightState,
    window: Option<gpui::AnyWindowHandle>,
    update: Option<u64>,
}

#[derive(Clone, Copy, Default)]
enum PreflightState {
    #[default]
    Idle,
    Draining {
        quit: bool,
        deadline: std::time::Instant,
    },
    Waiting {
        request: u64,
        quit: bool,
        generation: u64,
        revision: u64,
        deadline: std::time::Instant,
    },
    Ready {
        request: u64,
        quit: bool,
        generation: u64,
        revision: u64,
        safe: bool,
    },
    Dispatching {
        request: u64,
        quit: bool,
        generation: u64,
        revision: u64,
        safe: bool,
    },
    Held {
        quit: bool,
    },
}

impl AutomationClosePreflight {
    pub(super) fn frozen(&self) -> bool {
        !matches!(self.state, PreflightState::Idle)
    }
    pub(super) fn waiting(&self) -> bool {
        matches!(
            self.state,
            PreflightState::Draining { .. } | PreflightState::Waiting { .. }
        )
    }
    pub(super) fn blocks_update_approval(&self) -> bool {
        self.frozen() && !matches!(self.state, PreflightState::Held { .. })
    }
    pub(super) fn awaiting_update(&self) -> bool {
        self.update.is_some() && self.blocks_update_approval()
    }
    pub(super) fn update_safe(&self, id: u64) -> bool {
        matches!(self.state, PreflightState::Held { .. })
            && self.update.is_none_or(|expected| expected == id)
    }
    pub(super) fn cancel_update(&mut self, id: u64) {
        if self.update == Some(id) {
            self.update = None;
            self.state = PreflightState::Idle;
        }
    }

    fn observe(
        &mut self,
        request_id: u64,
        safe: bool,
        observed_generation: u64,
        observed_revision: u64,
    ) {
        let PreflightState::Waiting {
            request,
            quit,
            generation,
            revision,
            ..
        } = self.state
        else {
            return;
        };
        if request_id != request {
            return;
        }
        self.state = PreflightState::Ready {
            request,
            quit,
            generation,
            revision,
            safe: safe && observed_generation == generation && observed_revision == revision,
        };
    }
}

/// Bound for joining the project workers when hex quits. macOS termination
/// exits without dropping the editor, so a cancelled export's cleanup would
/// otherwise be cut off; the UI never waits longer than this.
const QUIT_SHUTDOWN_BOUND: Duration = Duration::from_secs(1);

/// Drive the actual worker and window events until close reaches its prompt.
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn wait_for_close_prompt(
    editor: &gpui::Entity<EditorView>,
    cx: &mut gpui::TestAppContext,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    loop {
        editor.update(cx, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
        });
        cx.run_until_parked();
        if cx.has_pending_prompt() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "close prompt did not arrive"
        );
        std::thread::yield_now();
    }
}

/// App-wide quit custody shared by Hex's own Quit paths and OS termination
/// requests (Dock Quit, logout, restart, shutdown), which macOS delivers through
/// the patched GPUI `App::on_should_quit` hook.
///
/// An OS request routes through [`EditorView::request_quit`], so it gets the
/// same refusals and unsaved-work prompt as menu Quit; it stays deferred until
/// that flow either quits ([`quit_confirmed`]: reply terminate) or does not
/// ([`quit_refused`]: reply cancel). Once quitting is confirmed, Hex's own
/// `cx.quit()` re-enters the hook and is answered at once, so the user is never
/// prompted twice. Without this global (tests, profiling tools) both helpers
/// fall back to plain `cx.quit()` / nothing.
#[derive(Default)]
struct QuitGate {
    confirmed: Cell<bool>,
    quit_issued: Cell<bool>,
    os_request_pending: Cell<bool>,
}

impl Global for QuitGate {}

/// Route OS termination requests through the editor's quit policy.
pub(super) fn install_quit_gate(cx: &mut App) {
    cx.set_global(QuitGate::default());
    cx.on_should_quit(|cx| {
        let gate = cx.global::<QuitGate>();
        if gate.confirmed.get() {
            cx.reply_to_quit(true);
            return;
        }
        gate.os_request_pending.set(true);
        let editor = cx.windows().into_iter().find_map(|handle| {
            handle
                .update(cx, |_, window, _| window.root::<EditorView>().flatten())
                .ok()
                .flatten()
                .map(|editor| (handle, editor))
        });
        let Some((handle, editor)) = editor else {
            // No editor holds work that a prompt could protect.
            quit_confirmed(cx);
            return;
        };
        if handle
            .update(cx, |_, window, cx| {
                editor.update(cx, |view, cx| view.request_quit(window, cx))
            })
            .is_err()
        {
            quit_refused(cx);
        }
    });
}

/// The user's quit is settled: answer a pending OS request with terminate, or
/// start Hex's own quit once.
pub(super) fn quit_confirmed(cx: &App) {
    let Some(gate) = cx.try_global::<QuitGate>() else {
        cx.quit();
        return;
    };
    gate.confirmed.set(true);
    // Either answer starts termination; later confirmations are no-ops.
    let issued = gate.quit_issued.replace(true);
    if gate.os_request_pending.replace(false) {
        cx.reply_to_quit(true);
    } else if !issued {
        cx.quit();
    }
}

/// Quitting was refused or cancelled: keep hex open and answer a pending OS
/// request with cancel.
pub(super) fn quit_refused(cx: &App) {
    if cx
        .try_global::<QuitGate>()
        .is_some_and(|gate| gate.os_request_pending.replace(false))
    {
        cx.reply_to_quit(false);
    }
}

pub(super) fn requires_close_confirmation(
    unsaved: bool,
    worker_requests_pending: usize,
    local_requests_pending: usize,
    duplicate_pending: bool,
    opening_project: bool,
    export_pending: bool,
    device_gesture_pending: bool,
) -> bool {
    unsaved
        || worker_requests_pending > 0
        || local_requests_pending > 0
        || duplicate_pending
        || opening_project
        || export_pending
        || device_gesture_pending
}

impl EditorView {
    fn release_automation_close(&mut self, cx: &Context<Self>) {
        let update = self.automation_close_preflight.update.take();
        self.automation_close_preflight.state = PreflightState::Idle;
        if let Some(id) = update {
            self.cancel_update(id, cx);
        }
    }

    pub(super) fn begin_update_automation_close(
        &mut self,
        id: u64,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        if self.automation_close_preflight.frozen() {
            return Err("another close decision is pending");
        }
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("automation safety deadline unavailable")?;
        self.automation_close_preflight.update = Some(id);
        self.automation_close_preflight.window = Some(window.window_handle());
        self.automation_close_preflight.state = PreflightState::Draining {
            quit: false,
            deadline,
        };
        self.wake_poll();
        cx.notify();
        Ok(())
    }
    pub(super) fn request_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.begin_automation_close(false, window, cx);
        false
    }

    pub(super) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_automation_close(true, window, cx);
    }

    fn begin_automation_close(&mut self, quit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.automation_close_preflight.frozen() {
            let quit_already_pending = matches!(
                self.automation_close_preflight.state,
                PreflightState::Draining { quit: true, .. }
                    | PreflightState::Waiting { quit: true, .. }
                    | PreflightState::Ready { quit: true, .. }
                    | PreflightState::Dispatching { quit: true, .. }
                    | PreflightState::Held { quit: true }
            );
            if quit && !quit_already_pending {
                quit_refused(cx);
            }
            return;
        }
        if self.update_blocks_close()
            || super::device_panel::source_naming(self)
            || self.gain_recorder.blocks_edits()
            || self.midi_recorder.blocks_edits()
            || self.native_automation_recorder.busy()
            || self.automation_take_recovery.unresolved()
            || self.close_prompt_open
            || self.save_dialog_open
            || self.project_open.busy()
            || self.pending_open_path.is_some()
        {
            self.error =
                Some("finish pending work or recover the automation take before closing".into());
            cx.notify();
            if quit {
                quit_refused(cx);
            }
            return;
        }
        if !self.prepare_theme_window_close(window, cx) {
            if quit {
                quit_refused(cx);
            }
            return;
        }
        let _ = super::device_panel::finalize_controls(self, cx);
        self.finalize_device_gesture_for_close();
        if let Err(reason) = self.finalize_tuning_gesture() {
            self.error = Some(reason.into());
        }
        let Some(deadline) = std::time::Instant::now().checked_add(Duration::from_secs(5)) else {
            self.error = Some("close safety deadline unavailable".into());
            if quit {
                quit_refused(cx);
            }
            return;
        };
        self.automation_close_preflight.window = Some(window.window_handle());
        self.automation_close_preflight.state = PreflightState::Draining { quit, deadline };
        self.wake_poll();
        cx.notify();
    }

    fn submit_automation_close_query(
        &mut self,
        quit: bool,
        deadline: std::time::Instant,
        cx: &mut Context<Self>,
    ) {
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            self.release_automation_close(cx);
            self.error = Some("project is still loading".into());
            if quit {
                quit_refused(cx);
            }
            cx.notify();
            return;
        };
        let Ok(request) = self.native_automation_recorder.reserve_request() else {
            self.release_automation_close(cx);
            self.error = Some("automation request limit reached".into());
            if quit {
                quit_refused(cx);
            }
            cx.notify();
            return;
        };
        self.automation_close_preflight.state = PreflightState::Waiting {
            request,
            quit,
            generation: self.project_generation,
            revision,
            deadline,
        };
        if self
            .service
            .submit(
                hex_session::service::Request::QueryDeviceAutomationCloseSafety {
                    request_id: request,
                },
            )
            .is_err()
        {
            self.release_automation_close(cx);
            self.error = Some("automation safety check unavailable · close cancelled".into());
            if quit {
                quit_refused(cx);
            }
        } else {
            self.wake_poll();
        }
        cx.notify();
    }

    pub(super) fn observe_automation_close(&mut self, event: &hex_session::service::Event) {
        let Some(completion) = event.device_automation_take_complete.as_ref() else {
            return;
        };
        use hex_session::service::DeviceAutomationTakeProgress as Progress;
        let safe = event.error.is_none() && completion.progress == Progress::CloseSafe;
        self.automation_close_preflight.observe(
            completion.request_id,
            safe,
            self.project_generation,
            event.state.revision,
        );
    }

    pub(super) fn tick_automation_close(&mut self, cx: &mut Context<Self>) -> bool {
        if let PreflightState::Draining { quit, deadline }
        | PreflightState::Waiting { quit, deadline, .. } = self.automation_close_preflight.state
            && std::time::Instant::now() >= deadline
        {
            self.release_automation_close(cx);
            self.error = Some("automation safety check timed out · close cancelled".into());
            if quit {
                quit_refused(cx);
            }
            return true;
        }
        if let PreflightState::Draining { quit, deadline } = self.automation_close_preflight.state
            && self.pending_requests.is_empty()
            && self.worker_requests_pending == 0
            && !self.device_gesture_pending()
            && !self.native_automation_recorder.busy()
            && !self.automation_take_recovery.unresolved()
            && self.tuning.client.snapshot().is_none()
        {
            self.submit_automation_close_query(quit, deadline, cx);
            return true;
        }
        if let PreflightState::Ready {
            request,
            quit,
            generation,
            revision,
            safe,
        } = self.automation_close_preflight.state
        {
            let Some(handle) = self.automation_close_preflight.window else {
                self.release_automation_close(cx);
                if quit {
                    quit_refused(cx);
                }
                self.error = Some("close window unavailable · close cancelled".into());
                return true;
            };
            self.automation_close_preflight.state = PreflightState::Dispatching {
                request,
                quit,
                generation,
                revision,
                safe,
            };
            let editor = cx.entity();
            let error_editor = editor.clone();
            cx.spawn(async move |_, cx| {
                if cx
                    .update_window(handle, move |_, window, cx| {
                        editor.update(cx, |view, cx| {
                            view.finish_automation_close(request, window, cx)
                        });
                    })
                    .is_err()
                {
                    let _ = cx.update(|cx| {
                        error_editor.update(cx, |view, cx| {
                            view.release_automation_close(cx);
                            view.error = Some("close window unavailable · close cancelled".into());
                            cx.notify();
                        });
                        if quit {
                            quit_refused(cx);
                        }
                    });
                }
            })
            .detach();
            return true;
        }
        if let PreflightState::Held { quit } = self.automation_close_preflight.state
            && !self.close_prompt_open
            && !self.update_preparing()
        {
            self.release_automation_close(cx);
            if quit {
                quit_refused(cx);
            }
        }
        false
    }

    pub(super) fn automation_close_disconnected(&mut self, cx: &mut Context<Self>) -> bool {
        let quit = match self.automation_close_preflight.state {
            PreflightState::Draining { quit, .. }
            | PreflightState::Waiting { quit, .. }
            | PreflightState::Ready { quit, .. }
            | PreflightState::Dispatching { quit, .. } => quit,
            _ => return false,
        };
        self.release_automation_close(cx);
        self.error = Some("project worker stopped · close cancelled".into());
        if quit {
            quit_refused(cx);
        }
        true
    }

    fn finish_automation_close(
        &mut self,
        request: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PreflightState::Dispatching {
            request: current,
            quit,
            generation,
            revision,
            safe,
        } = self.automation_close_preflight.state
        else {
            return;
        };
        if current != request {
            return;
        }
        let valid = safe
            && self.project_generation == generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == revision)
            && self.pending_requests.is_empty()
            && self.worker_requests_pending == 0
            && !self.native_automation_recorder.busy()
            && !self.automation_take_recovery.unresolved()
            && !self.device_gesture_pending();
        if !valid {
            self.release_automation_close(cx);
            if !safe {
                self.probe_failed_plugin_automation();
            }
            self.error = Some(
                "automation take needs recovery or pending work changed · close cancelled".into(),
            );
            if quit {
                quit_refused(cx);
            }
            cx.notify();
            return;
        }
        self.automation_close_preflight.state = PreflightState::Held { quit };
        if self.automation_close_preflight.update.is_some() {
            self.reconcile_update_ready(window, cx);
            cx.notify();
            return;
        }
        if self.requires_close_confirmation(cx) {
            self.show_close_prompt(window, cx, quit);
        } else if self.prepare_update_quit(false, cx) {
            // Keep the admission freeze until update preparation succeeds or fails.
        } else {
            self.cancel_browser_sample_playback();
            self.stop();
            if quit {
                self.shutdown_service_for_quit(cx);
            }
            window.remove_window();
            if quit {
                quit_confirmed(cx);
            }
        }
    }

    /// Join the session and export workers within [`QUIT_SHUTDOWN_BOUND`]
    /// before a confirmed quit. A timeout is logged and quitting continues;
    /// the workers stay signalled.
    fn shutdown_service_for_quit(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.service.shutdown(QUIT_SHUTDOWN_BOUND) {
            let error = error.to_string();
            eprintln!("hex: {error}");
            if let Some(unavailable) =
                crate::diagnostics::record_state(cx, "quit shutdown incomplete", Some(&error))
            {
                eprintln!("{unavailable}");
            }
        }
    }

    fn requires_close_confirmation(&self, cx: &Context<Self>) -> bool {
        requires_close_confirmation(
            self.state.as_ref().is_some_and(|state| state.unsaved),
            self.worker_requests_pending,
            self.pending_requests.len(),
            self.duplicate_intent.is_pending(),
            self.opening_project,
            self.export_pending,
            self.device_gesture_pending()
                || self.pending_plugin_effect.is_some()
                || self.device_panel.has_pending_draft(cx)
                || self.tuning.client.snapshot().is_some(),
        )
    }

    fn finalize_device_gesture_for_close(&mut self) {
        if self.device_gesture.has_pending_terminal() {
            return;
        }
        if self.device_gesture.snapshot().is_some()
            && let Err(error) = self.device_gesture.finish_current()
        {
            self.error = Some(error.into());
        }
    }

    fn show_close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>, quit: bool) {
        self.invalidate_prepared_seek();
        self.close_prompt_open = true;
        let answer = window.prompt(
            PromptLevel::Warning,
            "discard unsaved work?",
            Some(if self.export_pending {
                "pending edits or project changes will be discarded. an export may not finish."
            } else {
                "pending edits or project changes will be discarded."
            }),
            &[PromptButton::cancel("cancel"), PromptButton::new("discard")],
            cx,
        );
        // The window can retain its bootstrap handle type after replace_root.
        // Update the editor entity directly instead of downcasting that
        // handle, otherwise the dialog opens but its answer is never consumed.
        let handle = window.window_handle();
        // Keep the prompt's editor alive until its answer is handled. Closing
        // the confirmed window must not depend on a weak entity update succeeding.
        let editor = cx.entity();
        cx.spawn(async move |_, cx| {
            let discard = answer.await.ok() == Some(1);
            if let Err(error) = cx.update_window(handle, move |_, window, cx| {
                let preparing_update = editor.update(cx, |view, cx| {
                    view.close_prompt_open = false;
                    if discard {
                        if view.update_armed() {
                            return view.prepare_update_quit(true, cx);
                        }
                        view.cancel_plugin_effect();
                        view.cancel_browser_sample_playback();
                        if let Some(control) = &view.export_control {
                            control.cancel();
                        }
                        view.stop();
                        if quit {
                            view.shutdown_service_for_quit(cx);
                        }
                    } else {
                        view.release_automation_close(cx);
                        if quit {
                            quit_refused(cx);
                        }
                        cx.notify();
                    }
                    false
                });
                // Release the prompt's extra handle inside this app update,
                // so removing the last window also flushes editor/Service drops.
                drop(editor);
                if discard && !preparing_update {
                    window.remove_window();
                    if quit {
                        quit_confirmed(cx);
                    }
                }
            }) {
                // A window that was already removed needs no second close.
                // Other lifecycle failures must be observable during diagnosis.
                eprintln!("hex: could not handle close prompt: {error}");
                if quit {
                    // Keep hex running rather than leave an OS request unanswered.
                    let _ = cx.update(|cx| quit_refused(cx));
                }
            }
        })
        .detach();
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::{AutomationClosePreflight, PreflightState, requires_close_confirmation};

    #[test]
    fn automation_preflight_correlates_responses_and_keeps_admission_frozen() {
        let mut gate = AutomationClosePreflight::default();
        assert!(!gate.frozen());
        gate.state = PreflightState::Waiting {
            request: 7,
            quit: true,
            generation: 2,
            revision: 3,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(1),
        };
        assert!(gate.frozen());
        gate.observe(6, true, 2, 3);
        assert!(matches!(gate.state, PreflightState::Waiting { .. }));
        gate.observe(7, true, 2, 4);
        assert!(matches!(
            gate.state,
            PreflightState::Ready {
                safe: false,
                quit: true,
                ..
            }
        ));
        assert!(gate.frozen());
        gate.state = PreflightState::Idle;
        gate.observe(7, true, 2, 3);
        assert!(!gate.frozen());
    }

    #[test]
    fn update_readiness_belongs_to_one_update_and_cancellation_releases_it() {
        let mut gate = AutomationClosePreflight {
            update: Some(7),
            state: PreflightState::Held { quit: false },
            ..Default::default()
        };
        assert!(gate.update_safe(7));
        assert!(!gate.update_safe(8));
        gate.cancel_update(8);
        assert!(gate.frozen());
        gate.cancel_update(7);
        assert!(!gate.frozen());
        assert!(!gate.update_safe(7));
    }

    #[test]
    fn close_confirmation_covers_dirty_and_pending_work() {
        assert!(requires_close_confirmation(
            false, 0, 0, false, false, true, false
        ));
        assert!(!requires_close_confirmation(
            false, 0, 0, false, false, false, false
        ));
        assert!(requires_close_confirmation(
            true, 0, 0, false, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 1, 0, false, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 1, false, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 0, true, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 0, false, true, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 0, false, false, false, true
        ));
    }
}

// Run with --features ui-test-support. These exercise the actual asynchronous
// prompt completion and window lifecycle without opening a user's project.
#[cfg(all(test, feature = "ui-test-support"))]
mod window_tests {
    use super::*;
    use gpui::{
        AnyWindowHandle, Empty, Entity, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
        VisualTestContext, point,
    };
    use hex_session::service::{Event, Request, Service, ViewState};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn settle_close(cx: &mut TestAppContext) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            cx.run_until_parked();
            let mut pending = false;
            for handle in cx.windows() {
                let _ = cx.update_window(handle, |_, window, cx| {
                    if let Some(editor) = window.root::<EditorView>().flatten() {
                        editor.update(cx, |view, cx| {
                            if matches!(
                                view.automation_close_preflight.state,
                                PreflightState::Draining { .. }
                                    | PreflightState::Waiting { .. }
                                    | PreflightState::Ready { .. }
                                    | PreflightState::Dispatching { .. }
                            ) {
                                pending = true;
                                view.tick(cx);
                            }
                        });
                    }
                });
            }
            if !pending {
                break;
            }
            assert!(Instant::now() < deadline, "close preflight did not settle");
            std::thread::yield_now();
        }
        cx.run_until_parked();
    }

    fn next_service_event(service: &Service) -> Event {
        let start = Instant::now();
        loop {
            match service.try_event() {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty)
                    if start.elapsed() < Duration::from_secs(2) =>
                {
                    std::thread::yield_now();
                }
                Err(error) => panic!("project worker event unavailable: {error}"),
            }
        }
    }

    fn next_editor_service_event(editor: &Entity<EditorView>, cx: &mut TestAppContext) -> Event {
        let start = Instant::now();
        loop {
            match editor.read_with(cx, |view, _| view.service.try_event()) {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty)
                    if start.elapsed() < Duration::from_secs(3) =>
                {
                    std::thread::yield_now();
                }
                Err(error) => panic!("project worker event unavailable: {error}"),
            }
        }
    }

    fn dirty_replaced_window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<EditorView>) {
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let service = Service::new(48_000, 32, 8_000_000).expect("disposable service");
                let _ = next_service_event(&service);
                service
                    .submit(Request::Edit(hex_project::Command::AddTrack {
                        name: "close fixture".into(),
                    }))
                    .unwrap();
                let dirty = next_service_event(&service);
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("test offline output".into()))
                });
                super::super::install_close_guard(window, cx, editor.downgrade());
                editor.update(cx, |view, cx| {
                    view.receive(dirty);
                    cx.notify();
                });
                editor
            })
            .expect("bootstrap window");
        (handle, editor)
    }

    #[gpui::test]
    fn automation_close_freezes_admission_until_prompt_cancellation(cx: &mut TestAppContext) {
        let (handle, editor) = dirty_replaced_window(cx);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.request_quit(window, cx);
                assert!(view.automation_close_preflight.frozen());
                let track = view.state.as_ref().unwrap().project.tracks()[0].id();
                let target = hex_session::device_gesture::GestureTarget::TrackGain { track };
                assert!(
                    view.begin_device_gesture(
                        target,
                        hex_lattice_contract::ParameterValue::Float(0.5)
                    )
                    .is_err()
                );
                assert!(
                    view.begin_native_automation_touch(
                        target,
                        hex_lattice_contract::ParameterValue::Float(0.5)
                    )
                    .is_err()
                );
            });
        })
        .unwrap();
        settle_close(cx);
        assert!(cx.has_pending_prompt());
        editor.read_with(cx, |view, _| {
            assert!(view.automation_close_preflight.frozen())
        });
        cx.simulate_prompt_answer("cancel");
        settle_close(cx);
        editor.read_with(cx, |view, _| {
            assert!(!view.automation_close_preflight.frozen())
        });
        assert_eq!(cx.windows().len(), 1);
        editor.update(cx, |view, cx| {
            let revision = view.state.as_ref().unwrap().revision;
            view.automation_close_preflight.state = PreflightState::Waiting {
                request: 1000,
                quit: false,
                generation: view.project_generation,
                revision,
                deadline: Instant::now(),
            };
            assert!(view.tick_automation_close(cx));
            assert!(!view.automation_close_preflight.frozen());
            view.automation_close_preflight
                .observe(1000, true, view.project_generation, revision);
            assert!(
                !view.automation_close_preflight.frozen(),
                "late safe response stays retired"
            );
            view.automation_close_preflight.state = PreflightState::Waiting {
                request: 1001,
                quit: false,
                generation: view.project_generation,
                revision,
                deadline: Instant::now() + Duration::from_secs(1),
            };
            view.automation_close_preflight
                .observe(1001, false, view.project_generation, revision);
            view.tick_automation_close(cx);
        });
        settle_close(cx);
        assert!(!cx.has_pending_prompt());
        editor.read_with(cx, |view, _| {
            assert!(!view.automation_close_preflight.frozen())
        });
        assert_eq!(cx.windows().len(), 1);
    }

    fn click_custom_close(visual: &mut VisualTestContext) {
        let bounds = visual
            .debug_bounds("window-control-close")
            .expect("custom close hitbox");
        let position = point(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        visual.simulate_event(MouseDownEvent {
            position,
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        visual.simulate_event(MouseUpEvent {
            position,
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
    }

    #[gpui::test]
    fn custom_close_button_removes_a_clean_editor_window(cx: &mut TestAppContext) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let (handle, editor) = dirty_replaced_window(cx);
        mark_clean(&editor, cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click_custom_close(&mut visual);
        visual.run_until_parked();
        settle_close(cx);
        assert!(cx.windows().is_empty());
    }

    #[gpui::test]
    fn custom_close_button_keeps_cancelled_dirty_window_and_discards_on_second_close(
        cx: &mut TestAppContext,
    ) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let (handle, _) = dirty_replaced_window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click_custom_close(&mut visual);
        settle_close(cx);
        assert!(cx.has_pending_prompt());
        click_custom_close(&mut visual);
        settle_close(cx);
        assert!(cx.has_pending_prompt());
        settle_close(cx);
        cx.simulate_prompt_answer("cancel");
        settle_close(cx);
        assert!(!cx.has_pending_prompt());
        assert_eq!(cx.windows().len(), 1);

        visual.run_until_parked();
        click_custom_close(&mut visual);
        settle_close(cx);
        assert!(cx.has_pending_prompt());
        settle_close(cx);
        cx.simulate_prompt_answer("discard");
        settle_close(cx);
        assert!(cx.windows().is_empty());
    }

    #[gpui::test]
    fn cancel_then_discard_closes_replaced_bootstrap_window(cx: &mut TestAppContext) {
        let (handle, editor) = dirty_replaced_window(cx);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(!view.request_window_close(window, cx));
            });
        })
        .expect("dirty window");
        settle_close(cx);
        assert!(cx.has_pending_prompt());
        settle_close(cx);
        cx.simulate_prompt_answer("cancel");
        settle_close(cx);
        assert_eq!(cx.windows().len(), 1);
        editor.read_with(cx, |view, _| {
            assert!(!view.close_prompt_open);
            assert!(view.state.as_ref().is_some_and(|state| state.unsaved));
            assert_eq!(view.worker_requests_pending, 0);
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(!view.request_window_close(window, cx));
                // A second close while the sheet is open must not create a second prompt.
                assert!(!view.request_window_close(window, cx));
            });
        })
        .expect("cancel preserved window");
        settle_close(cx);
        cx.simulate_prompt_answer("discard");
        settle_close(cx);
        assert!(!cx.has_pending_prompt());
        assert!(cx.windows().is_empty());
        assert!(cx.update_window(handle, |_, _, _| ()).is_err());
    }

    #[gpui::test]
    fn quit_cancel_then_discard_removes_confirmed_window(cx: &mut TestAppContext) {
        let (handle, editor) = dirty_replaced_window(cx);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| view.request_quit(window, cx));
        })
        .expect("dirty window");
        settle_close(cx);
        cx.simulate_prompt_answer("cancel");
        settle_close(cx);
        assert_eq!(cx.windows().len(), 1);
        assert!(!editor.read_with(cx, |view, _| view.close_prompt_open));
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| view.request_quit(window, cx));
        })
        .expect("cancel preserved window");
        settle_close(cx);
        cx.simulate_prompt_answer("discard");
        settle_close(cx);
        assert!(!cx.has_pending_prompt());
        assert!(cx.windows().is_empty());
    }

    #[gpui::test]
    fn clean_base_device_draft_is_committed_before_cancel_and_discard_prompt(
        cx: &mut TestAppContext,
    ) {
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let service = Service::new(48_000, 32, 8_000_000).expect("disposable service");
        let _ = next_service_event(&service);
        service
            .submit(Request::Edit(hex_project::Command::AddTrack {
                name: "device draft".into(),
            }))
            .expect("add track");
        let track = next_service_event(&service).state.project.tracks()[0].id();
        let type_id =
            hex_session::device_gesture::DeviceTypeId::new("hex.utility").expect("type id");
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("add utility");
        let device_event = next_service_event(&service);
        let device = device_event.state.project.tracks()[0].devices()[0].id();
        let path =
            std::env::temp_dir().join(format!("hex-clean-device-draft-{}.hex", std::process::id()));
        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save clean base");
        let device_event = next_service_event(&service);
        assert!(!device_event.state.unsaved);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("test offline output".into()))
                });
                super::super::install_close_guard(window, cx, editor.downgrade());
                editor.update(cx, |view, cx| {
                    let state = &device_event.state;
                    view.state = Some(Arc::new(ViewState {
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: Arc::clone(&state.project),
                        waveforms: state.waveforms.clone(),
                        source_overviews: state.source_overviews.clone(),
                        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
                        midi_preview: Arc::clone(&state.midi_preview),
                        gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                        timeline_frames: state.timeline_frames,
                        can_undo: false,
                        can_redo: false,
                        revision: state.revision,
                        unsaved: false,
                        document_path: None,
                    }));
                    view.begin_device_gesture(
                        hex_session::device_gesture::GestureTarget::DeviceParameter {
                            device,
                            parameter: hex_session::device_gesture::ParameterId::new(1)
                                .expect("gain parameter"),
                        },
                        hex_session::device_gesture::ParameterValue::Float(1.0),
                    )
                    .expect("begin clean-base gesture");
                    view.change_device_gesture(hex_session::device_gesture::ParameterValue::Float(
                        2.0,
                    ))
                    .expect("draft parameter change");
                    cx.notify();
                });
                editor
            })
            .expect("device editor window");
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(!view.request_window_close(window, cx));
            });
        })
        .expect("close with draft");
        settle_close(cx);
        assert!(cx.has_pending_prompt());
        settle_close(cx);
        cx.simulate_prompt_answer("cancel");
        settle_close(cx);
        assert_eq!(cx.windows().len(), 1);
        editor.read_with(cx, |view, _| {
            assert!(view.state.as_ref().is_some_and(|state| state.unsaved));
            assert!(
                view.device_gesture.snapshot().is_none(),
                "close finalized the draft"
            );
            view.service.submit(Request::Undo).unwrap();
        });
        let undone = next_editor_service_event(&editor, cx);
        assert!(undone.error.is_none());
        assert_eq!(undone.state.project, device_event.state.project);
        editor.update(cx, |view, _| {
            view.receive(undone);
            view.service.submit(Request::Redo).unwrap();
        });
        let redone = next_editor_service_event(&editor, cx);
        assert!(redone.error.is_none());
        editor.update(cx, |view, _| {
            view.receive(redone);
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(!view.request_window_close(window, cx));
            });
        })
        .expect("second close");
        settle_close(cx);
        cx.simulate_prompt_answer("discard");
        settle_close(cx);
        assert!(cx.windows().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[gpui::test]
    fn failed_gesture_compile_refuses_deferred_save_and_export(cx: &mut TestAppContext) {
        let service = Service::new(48_000, 64, 8_000_000).expect("disposable service");
        let _ = next_service_event(&service);
        service
            .submit(Request::Edit(hex_project::Command::AddTrack {
                name: "compile-limited chain".into(),
            }))
            .expect("add track");
        let mut state = next_service_event(&service).state;
        let track = state.project.tracks()[0].id();
        let type_id =
            hex_session::device_gesture::DeviceTypeId::new("hex.utility").expect("type id");
        let mut devices = Vec::new();
        for _ in 0..24 {
            service
                .submit(Request::AddNativeDevice {
                    track,
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: Box::new(type_id),
                    before: None,
                })
                .expect("add utility");
            let event = next_service_event(&service);
            devices.push(
                event.state.project.tracks()[0]
                    .devices()
                    .last()
                    .unwrap()
                    .id(),
            );
            state = event.state;
        }
        let gain = hex_session::device_gesture::ParameterId::new(1).expect("gain parameter");
        let invalid_value = hex_session::device_gesture::ParameterValue::Float(56.234_13);
        let mut rejected_device = None;
        for device in devices {
            service
                .submit(Request::SetDeviceParameter {
                    device,
                    parameter: gain,
                    value: invalid_value,
                })
                .expect("probe device capacity");
            let event = next_service_event(&service);
            if event.error.is_some() {
                rejected_device = Some(device);
                state = event.state;
                break;
            }
            state = event.state;
        }
        let device = rejected_device.expect("dense chain rejects one candidate");
        let save_path =
            std::env::temp_dir().join(format!("hex-refused-gesture-{}.hex", std::process::id()));
        let export_path =
            std::env::temp_dir().join(format!("hex-refused-gesture-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&save_path);
        let _ = std::fs::remove_file(&export_path);

        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("test offline output".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(Arc::clone(&state));
                    view.begin_device_gesture(
                        hex_session::device_gesture::GestureTarget::DeviceParameter {
                            device,
                            parameter: gain,
                        },
                        hex_session::device_gesture::ParameterValue::Float(1.0),
                    )
                    .expect("begin parameter gesture");
                    cx.notify();
                });
                editor
            })
            .expect("device editor window");

        editor.update(cx, |view, _| assert!(view.pump_device_gesture()));
        let begin = next_editor_service_event(&editor, cx);
        let begin_ack = begin.gesture_ack.expect("begin acknowledgement");
        assert!(begin_ack.accepted);
        editor.update(cx, |view, _| {
            assert!(view.acknowledge_device_gesture(&begin_ack))
        });

        editor.update(cx, |view, _| {
            view.change_device_gesture(invalid_value)
                .expect("stage invalid final value");
            assert!(view.pump_device_gesture());
        });
        let preview = next_editor_service_event(&editor, cx);
        let preview_ack = preview.gesture_ack.expect("preview acknowledgement");
        assert!(!preview_ack.accepted);
        editor.update(cx, |view, _| {
            assert!(view.acknowledge_device_gesture(&preview_ack))
        });

        let export_control = hex_session::service::ExportControl::default();
        editor.update(cx, |view, _| {
            assert!(view.enqueue_device_barrier(Request::SaveNew(save_path.clone())));
            let owner = hex_session::service::preset::Owner {
                project_generation: view.project_generation,
                revision: view.state.as_ref().unwrap().revision,
                device,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id,
            };
            assert!(view.enqueue_device_barrier(Request::SaveDevicePreset {
                owner,
                path: save_path.with_extension("hex-device-preset.json"),
            }));
            assert!(view.enqueue_device_barrier(Request::LoadDevicePreset {
                owner,
                path: save_path.with_extension("hex-device-preset.json"),
            }));
            assert!(view.enqueue_device_barrier(Request::ExportRange {
                path: export_path.clone(),
                start: 0,
                frames: Some(256),
                signal: hex_session::service::ExportSignal::Main,
                control: export_control.clone(),
                format: hex_session::service::ExportFormat::Pcm16,
                overwrite: false,
            }));
            view.export_control = Some(export_control.clone());
            view.export_window = Some(handle);
            view.saving_project = true;
            view.export_pending = true;
            assert!(view.pump_device_gesture());
        });
        let commit = next_editor_service_event(&editor, cx);
        let commit_ack = commit.gesture_ack.expect("commit acknowledgement");
        assert!(!commit_ack.accepted);
        editor.update(cx, |view, _| {
            assert!(view.acknowledge_device_gesture(&commit_ack));
            assert!(!view.saving_project);
            assert!(!view.export_pending);
            assert!(view.export_control.is_none());
            assert!(view.export_window.is_none());
            assert!(view.device_gesture_barriers.is_empty());
            assert!(view.pending_requests.is_empty());
            assert!(view.error.is_some());
        });

        assert!(export_control.is_cancelled());
        assert!(
            !save_path.exists(),
            "failed gesture must not save old value"
        );
        assert!(
            !export_path.exists(),
            "failed gesture must not export old value"
        );
        assert!(matches!(
            editor.read_with(cx, |view, _| view.service.try_export_event()),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        cx.update_window(handle, |_, window, _| window.remove_window())
            .expect("remove test window");
    }

    fn mark_clean(editor: &Entity<EditorView>, cx: &mut TestAppContext) {
        let path = std::env::temp_dir().join(format!(
            "hex-close-clean-{}-{}.hex",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        editor.update(cx, |view, _| {
            view.service.submit(Request::SaveNew(path.clone())).unwrap()
        });
        let event = next_editor_service_event(editor, cx);
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(cx, |view, _| {
            view.receive(event);
        });
        let _ = std::fs::remove_file(path);
    }

    #[gpui::test]
    fn os_quit_with_unsaved_work_waits_on_the_prompt(cx: &mut TestAppContext) {
        cx.update(install_quit_gate);
        let (_, editor) = dirty_replaced_window(cx);
        assert!(cx.simulate_should_quit());
        settle_close(cx);
        assert!(cx.has_pending_prompt());
        assert!(cx.quit_request_pending());
        assert!(cx.quit_replies().is_empty(), "no answer before the prompt");
        settle_close(cx);
        cx.simulate_prompt_answer("cancel");
        settle_close(cx);
        assert_eq!(cx.quit_replies(), [false]);
        assert_eq!(cx.windows().len(), 1);
        assert!(!editor.read_with(cx, |view, _| view.close_prompt_open));

        assert!(cx.simulate_should_quit());
        settle_close(cx);
        cx.simulate_prompt_answer("discard");
        settle_close(cx);
        assert_eq!(cx.quit_replies(), [false, true]);
        assert!(cx.windows().is_empty());
    }

    #[gpui::test]
    fn os_quit_of_a_clean_project_terminates_at_once(cx: &mut TestAppContext) {
        cx.update(install_quit_gate);
        let (_, editor) = dirty_replaced_window(cx);
        mark_clean(&editor, cx);
        assert!(cx.simulate_should_quit());
        settle_close(cx);
        assert!(!cx.has_pending_prompt());
        assert_eq!(cx.quit_replies(), [true]);
        // The project workers were joined before terminate was answered.
        assert!(editor.read_with(cx, |view, _| matches!(
            view.service.submit(Request::Undo),
            Err(hex_session::service::SubmitError::Closed(_))
        )));
        assert!(cx.windows().is_empty());
    }

    #[gpui::test]
    fn menu_quit_prompts_once_and_answers_its_own_terminate_request(cx: &mut TestAppContext) {
        cx.update(install_quit_gate);
        let (handle, editor) = dirty_replaced_window(cx);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| view.request_quit(window, cx));
        })
        .expect("dirty window");
        settle_close(cx);
        cx.simulate_prompt_answer("discard");
        settle_close(cx);
        assert!(cx.windows().is_empty());
        // On macOS that confirmed cx.quit() re-enters as a termination request.
        assert!(cx.simulate_should_quit());
        settle_close(cx);
        assert!(
            !cx.has_pending_prompt(),
            "confirmed quit must not prompt again"
        );
        assert_eq!(cx.quit_replies(), [true]);
    }
}
