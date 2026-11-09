//! Editor intent for a single retained device take. Session owns the writer,
//! verifies the selected copy, and alone authorizes releasing the original.
use super::EditorView;
use gpui::Context;
use hex_session::service::{
    DeviceAutomationTakeAction as Action, DeviceAutomationTakeProgress as Progress,
    DeviceAutomationTakeScope as Scope, Event, Request,
    automation_draft::{DraftTerminal, RecoveryCopyReceipt},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_EVENTS: u64 = hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64;

#[derive(Default)]
pub(super) struct Recovery {
    phase: Phase,
    request: Option<u64>,
    saved: Option<(Scope, Verified)>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Discover {
        until: Instant,
    },
    NeedsCopy(Scope),
    Choosing {
        scope: Scope,
        picker: u64,
    },
    Preparing {
        scope: Scope,
        destination: PathBuf,
    },
    Copying {
        scope: Scope,
        destination: PathBuf,
    },
    Verifying {
        scope: Scope,
        destination: PathBuf,
    },
    Verified {
        scope: Scope,
        copy: Verified,
    },
    Keeping {
        scope: Scope,
        copy: Verified,
    },
    Applying {
        scope: Scope,
        copy: Verified,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum Resolution {
    Kept(Box<Scope>),
    Applied,
}

struct Verified {
    path: PathBuf,
    receipt: RecoveryCopyReceipt,
}

impl Phase {
    fn unresolved(&self) -> bool {
        !matches!(self, Self::Idle | Self::Discover { .. })
    }

    fn is_work(&self) -> bool {
        matches!(
            self,
            Self::Discover { .. }
                | Self::Preparing { .. }
                | Self::Copying { .. }
                | Self::Verifying { .. }
                | Self::Keeping { .. }
                | Self::Applying { .. }
        )
    }

    fn retry_choice(self) -> Self {
        match self {
            Self::Preparing { scope, .. }
            | Self::Copying { scope, .. }
            | Self::Verifying { scope, .. } => Self::NeedsCopy(scope),
            Self::Keeping { scope, .. } => Self::NeedsCopy(scope),
            _ => Self::Idle,
        }
    }
}

impl Recovery {
    pub(super) fn unresolved(&self) -> bool {
        self.phase.unresolved()
    }

    pub(super) fn polling(&self) -> bool {
        self.request.is_some() || self.phase.is_work()
    }

    fn matches_request(&self, id: u64) -> bool {
        self.request == Some(id)
    }

    fn matches_picker(&self, scope: Scope, picker: u64) -> bool {
        matches!(self.phase, Phase::Choosing { scope: selected, picker: id }
            if selected == scope && id == picker)
    }

    fn finish_picker(&mut self, scope: Scope, picker: u64, destination: Option<PathBuf>) {
        if !self.matches_picker(scope, picker) {
            return;
        }
        self.phase = match destination {
            Some(destination) if scope.target.native().is_some() => {
                Phase::Preparing { scope, destination }
            }
            Some(destination) => Phase::Copying { scope, destination },
            None => Phase::NeedsCopy(scope),
        };
    }

    fn retry_choice(&mut self, phase: Phase) -> Phase {
        match phase {
            Phase::Applying { scope, copy } => {
                self.saved = Some((scope, copy));
                Phase::Idle
            }
            phase => phase.retry_choice(),
        }
    }

    fn retry_current(&mut self) {
        let phase = std::mem::take(&mut self.phase);
        self.phase = self.retry_choice(phase);
    }

    fn settle(&mut self, id: u64, progress: &Progress) -> Option<Resolution> {
        if !self.matches_request(id) {
            return None;
        }
        self.request = None;
        let mut resolution = None;
        self.phase = match (std::mem::take(&mut self.phase), progress) {
            (Phase::Discover { .. }, Progress::FailedPluginCustody { scope, .. }) => {
                Phase::NeedsCopy(**scope)
            }
            (phase @ Phase::Discover { .. }, Progress::FailedPluginPending) => phase,
            (Phase::Preparing { scope, destination }, Progress::Retained(_)) => {
                Phase::Copying { scope, destination }
            }
            (Phase::Copying { scope, destination }, Progress::Pending) => {
                Phase::Verifying { scope, destination }
            }
            (phase @ (Phase::Preparing { .. } | Phase::Verifying { .. }), Progress::Pending) => {
                phase
            }
            (Phase::Verifying { scope, destination }, Progress::VerifiedCopy { path, receipt })
                if destination == *path =>
            {
                Phase::Verified {
                    scope,
                    copy: Verified {
                        path: path.clone(),
                        receipt: *receipt,
                    },
                }
            }
            (Phase::Keeping { scope, copy }, Progress::CopyKept(path)) if copy.path == *path => {
                self.saved = (copy.receipt.events() != 0
                    && matches!(copy.receipt.terminal(), DraftTerminal::Finished(_)))
                .then_some((scope, copy));
                resolution = Some(Resolution::Kept(Box::new(scope)));
                Phase::Idle
            }
            (Phase::Applying { .. }, Progress::Committed) => {
                resolution = Some(Resolution::Applied);
                Phase::Idle
            }
            (phase, _) => self.retry_choice(phase),
        };
        resolution
    }
}

impl EditorView {
    pub(super) fn automation_recovery_status(&self) -> Option<(&'static str, bool)> {
        match &self.automation_take_recovery.phase {
            Phase::Idle if self.retained_native_recovery_scope().is_some() => {
                Some(("save automation recovery copy", true))
            }
            Phase::Idle if self.saved_automation_take_available() => {
                Some(("apply recovered automation", true))
            }
            Phase::NeedsCopy(_) => Some(("save automation recovery copy", true)),
            Phase::Verified { .. } => Some(("keep automation recovery copy", true)),
            Phase::Choosing { .. } => Some(("choose a location for the recovery copy", false)),
            Phase::Preparing { .. } | Phase::Copying { .. } | Phase::Verifying { .. } => {
                Some(("saving automation recovery copy", false))
            }
            Phase::Keeping { .. } => Some(("checking automation recovery copy", false)),
            Phase::Applying { .. } => Some(("applying recovered automation", false)),
            Phase::Idle | Phase::Discover { .. } => None,
        }
    }

    fn saved_automation_take_available(&self) -> bool {
        self.automation_take_recovery.saved.is_some()
            && self.state.is_some()
            && !self.native_automation_recorder.busy()
            && !self.device_gesture_pending()
    }

    pub(super) fn start_automation_recovery(&mut self, cx: &mut Context<Self>) {
        if self.automation_close_preflight.frozen() || self.update_preparing() {
            return;
        }
        let phase = std::mem::take(&mut self.automation_take_recovery.phase);
        if matches!(phase, Phase::Idle)
            && self.saved_automation_take_available()
            && let Some((scope, copy)) = self.automation_take_recovery.saved.take()
        {
            self.automation_take_recovery.phase = Phase::Applying { scope, copy };
            self.wake_poll();
            cx.notify();
            return;
        }
        let scope = match phase {
            Phase::Idle => self.retained_native_recovery_scope(),
            Phase::NeedsCopy(scope) => Some(scope),
            Phase::Verified { scope, copy } => {
                self.automation_take_recovery.phase = Phase::Keeping { scope, copy };
                self.wake_poll();
                cx.notify();
                return;
            }
            phase => {
                self.automation_take_recovery.phase = phase;
                None
            }
        };
        let Some(scope) = scope else { return };
        let Ok(picker) = self.native_automation_recorder.reserve_request() else {
            self.automation_take_recovery.phase = Phase::NeedsCopy(scope);
            self.error = Some("automation request limit reached".into());
            cx.notify();
            return;
        };
        self.automation_take_recovery.phase = Phase::Choosing { scope, picker };
        let fallback = self
            .state
            .as_ref()
            .and_then(|state| state.document_path.as_deref())
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        self.with_project_picker_directory(cx, move |view, directory, cx| {
            if !view.automation_take_recovery.matches_picker(scope, picker) {
                return;
            }
            let prompt = cx.prompt_for_new_path(
                &directory.unwrap_or(fallback),
                Some("automation-recovery.hex-automation-draft"),
            );
            cx.spawn(async move |this, cx| {
                let result = prompt.await;
                let _ = this.update(cx, |view, cx| {
                    if !view.automation_take_recovery.matches_picker(scope, picker) {
                        return;
                    }
                    let destination = match result {
                        Ok(Ok(destination)) if !view.automation_close_preflight.frozen() => {
                            destination
                        }
                        Ok(Err(error)) => {
                            view.error = Some(error.to_string().into());
                            None
                        }
                        _ => None,
                    };
                    view.automation_take_recovery
                        .finish_picker(scope, picker, destination);
                    view.wake_poll();
                    cx.notify();
                });
            })
            .detach();
        });
    }

    pub(super) fn probe_failed_plugin_automation(&mut self) {
        if self.automation_take_recovery.request.is_none()
            && matches!(self.automation_take_recovery.phase, Phase::Idle)
            && !self.native_automation_recorder.busy()
            && let Some(until) = Instant::now().checked_add(Duration::from_secs(5))
        {
            self.automation_take_recovery.phase = Phase::Discover { until };
            self.wake_poll();
        }
    }

    pub(super) fn tick_automation_take_recovery(&mut self) -> bool {
        if matches!(
            self.automation_take_recovery.phase,
            Phase::Discover { until } if Instant::now() >= until
        ) {
            self.automation_take_recovery.phase = Phase::Idle;
            self.automation_take_recovery.request = None;
            return true;
        }
        if self.automation_close_preflight.frozen()
            || self.automation_take_recovery.request.is_some()
            || !self.automation_take_recovery.phase.is_work()
        {
            return false;
        }
        let Ok(request_id) = self.native_automation_recorder.reserve_request() else {
            self.error = Some("automation request limit reached".into());
            self.automation_take_recovery.retry_current();
            return true;
        };
        let take = |scope, action| Request::DeviceAutomationTake {
            request_id,
            scope,
            action,
        };
        let request = match &self.automation_take_recovery.phase {
            Phase::Discover { .. } => Request::QueryFailedPluginAutomation { request_id },
            Phase::Preparing { scope, .. } => take(*scope, Action::PrepareNativeRecovery),
            Phase::Copying { scope, destination } if scope.target.native().is_some() => {
                let Some(continuation) = self.native_recovery_continuation(*scope) else {
                    self.error = Some("retained automation take changed".into());
                    self.automation_take_recovery.phase = Phase::Idle;
                    return true;
                };
                take(
                    *scope,
                    Action::CopyNativeRecovery {
                        destination: destination.clone(),
                        max_events: MAX_EVENTS,
                        continuation,
                    },
                )
            }
            Phase::Copying { scope, destination } => take(
                *scope,
                Action::CopyFailedPlugin {
                    destination: destination.clone(),
                    max_events: MAX_EVENTS,
                },
            ),
            Phase::Verifying { scope, .. } => take(*scope, Action::PollFailedCopy),
            Phase::Keeping { scope, copy } if scope.target.native().is_some() => take(
                *scope,
                Action::KeepNativeRecoveryCopy {
                    path: copy.path.clone(),
                    receipt: copy.receipt,
                    max_events: MAX_EVENTS,
                },
            ),
            Phase::Keeping { scope, copy } => take(
                *scope,
                Action::KeepFailedPluginCopy {
                    path: copy.path.clone(),
                    receipt: copy.receipt,
                    max_events: MAX_EVENTS,
                },
            ),
            Phase::Applying { scope, copy } => {
                let Some(state) = self.state.as_ref() else {
                    self.automation_take_recovery.retry_current();
                    return true;
                };
                Request::RecoverDeviceAutomationTake {
                    request_id,
                    context: hex_engine::AudioSourceContext {
                        project_generation: self.project_generation,
                        editor_revision: state.revision,
                    },
                    target: scope.target.draft_target(),
                    expected: scope.stamp,
                    path: copy.path.clone(),
                    receipt: Some(copy.receipt),
                }
            }
            _ => return false,
        };
        self.automation_take_recovery.request = Some(request_id);
        if !self.enqueue(request) && self.automation_take_recovery.matches_request(request_id) {
            self.automation_take_recovery.request = None;
            self.automation_take_recovery.retry_current();
        }
        true
    }

    pub(super) fn automation_take_recovery_not_submitted(
        &mut self,
        request: &Request,
        closed: bool,
    ) {
        let (Request::DeviceAutomationTake { request_id, .. }
        | Request::QueryFailedPluginAutomation { request_id }
        | Request::RecoverDeviceAutomationTake { request_id, .. }) = request
        else {
            return;
        };
        if self.automation_take_recovery.matches_request(*request_id) {
            self.automation_take_recovery.request = None;
            if closed {
                self.automation_take_recovery.retry_current();
            }
        }
    }

    pub(super) fn observe_automation_take_recovery(&mut self, event: &Event) {
        let Some(completion) = event.device_automation_take_complete.as_ref() else {
            return;
        };
        match self
            .automation_take_recovery
            .settle(completion.request_id, &completion.progress)
        {
            Some(Resolution::Kept(scope)) => {
                self.release_native_recovery(*scope);
                self.error = None;
                self.status = "automation recovery copy kept".into();
            }
            Some(Resolution::Applied) => {
                self.error = None;
                self.status = "recovered automation applied".into();
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests;
