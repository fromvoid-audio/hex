//! Output open/reopen, route-follow, preference, and rate-negotiation state
//! owned by `NativeAudioSink`.
use super::{document_output, route_follow};
use hex_audio::{AudioLifecycleFailure, OutputOpenError, RateMismatch, WorkerPreparedOutput};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub(super) const STARTUP_OPEN_DEADLINE: Duration = Duration::from_secs(10);
pub(super) const STARTUP_OPEN_TIMEOUT: &str = "audio output did not open in time; editing continues without audio. use restart audio to retry";

/// Refuses an output whose negotiated rate differs from the document rate the
/// open requested, as the same typed recoverable outcome hex-audio reports.
/// The refused output is dropped into its existing lifecycle retirement.
pub(super) fn rate_checked(
    requested_hz: Option<u32>,
    output: WorkerPreparedOutput,
) -> Result<WorkerPreparedOutput, OutputOpenError> {
    match requested_hz {
        Some(project_hz) if project_hz != output.sample_rate => Err(RateMismatch {
            device_hz: output.sample_rate,
            project_hz,
        }
        .into()),
        _ => Ok(output),
    }
}

pub(super) fn lifecycle_failure_error(failure: AudioLifecycleFailure) -> OutputOpenError {
    let reason = match failure {
        AudioLifecycleFailure::Retirement => "audio output shutdown failed",
        AudioLifecycleFailure::Cleanup => "audio resource cleanup failed",
        AudioLifecycleFailure::MismatchedOwner => "audio lifecycle ownership mismatch",
        AudioLifecycleFailure::RetiringEditor => {
            "vst3 editor shutdown is unresolved · original source retained · audio reopen refused"
        }
    };
    if failure == AudioLifecycleFailure::RetiringEditor {
        reason.into()
    } else {
        format!("{reason}; save your project and restart hex").into()
    }
}

pub(crate) struct OutputLifecycle {
    pub(super) output_generation: Option<u64>,
    pub(crate) output_identity: Option<u64>,
    // Last worker-relinquished endpoint, retained until a fresh Lease opens.
    // Sampler preflight may retire it before a replacement arrives. This
    // identity retains no output, media, or native resource ownership.
    pub(super) relinquished_output_identity: Option<u64>,
    pub(super) opening: Option<hex_audio::WorkerRecoveryOpen>,
    pub(super) open_pending: bool,
    pub(super) requested_rate: Option<u32>,
    pub(super) bridge_rate_mismatch: Option<RateMismatch>,
    pub(super) replacement_output: Option<document_output::ReplacementOutput>,
    pub(super) document_rebind: Option<document_output::Rebind>,
    pub(super) restart_request: Option<u64>,
    /// A typed `RateMismatch` stays distinct so the UI can offer retry.
    pub(super) open_error: Option<OutputOpenError>,
    pub(super) initial_open_started: Option<Instant>,
    /// Samples the default output route off this worker; `None` when the
    /// watch thread could not start, which only disables route following.
    pub(super) route_watch: Option<hex_audio::OutputRouteWatch>,
    pub(super) route_seen: u64,
    pub(super) route_follow: route_follow::RouteFollow,
    pub(super) output_preferences_path: Option<PathBuf>,
    pub(super) output_selection: hex_audio::OutputDeviceSelection,
    pub(super) preferred_sample_rate: Option<u32>,
    /// Last scheduled cursor read from a relinquished output.
    pub(super) last_stopped_frame: u64,
    pub(super) suspended_output: Option<u64>,
}

impl OutputLifecycle {
    pub(super) fn new() -> Self {
        Self {
            output_generation: None,
            output_identity: None,
            relinquished_output_identity: None,
            opening: None,
            open_pending: false,
            requested_rate: None,
            bridge_rate_mismatch: None,
            replacement_output: None,
            document_rebind: None,
            restart_request: None,
            open_error: None,
            initial_open_started: None,
            route_watch: None,
            route_seen: 0,
            route_follow: Default::default(),
            output_preferences_path: std::env::var_os("HOME").map(|home| {
                hex_preferences::audio_device_preferences_path(std::path::Path::new(&home))
            }),
            output_selection: hex_audio::OutputDeviceSelection::default(),
            preferred_sample_rate: None,
            last_stopped_frame: 0,
            suspended_output: None,
        }
    }

    pub(super) fn request_open(&mut self, rate: Option<u32>, request: Option<u64>) {
        self.initial_open_started = None;
        self.opening = None;
        self.open_pending = true;
        self.requested_rate = rate;
        self.bridge_rate_mismatch = None;
        self.restart_request = request;
        self.open_error = None;
    }

    pub(super) fn begin_relinquish(&mut self) {
        self.document_rebind = None;
        self.initial_open_started = None;
        if let Some(identity) = self.output_identity {
            self.relinquished_output_identity = Some(identity);
        }
    }

    pub(super) fn finish_relinquish(&mut self) {
        self.opening = None;
        self.open_pending = false;
        self.output_generation = None;
        self.output_identity = None;
    }

    pub(super) fn reload_preferences(&mut self) -> Result<(), String> {
        let Some(path) = self.output_preferences_path.as_deref() else {
            return Ok(());
        };
        match hex_preferences::load_audio_device_preferences(path) {
            Ok(prefs) => {
                self.output_selection = hex_audio::OutputDeviceSelection {
                    device_uid: prefs.output_device_uid,
                    buffer_size_frames: prefs.buffer_size_frames,
                };
                self.preferred_sample_rate = prefs.sample_rate_hz;
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub(super) fn startup_timed_out(&self, deadline: Duration) -> bool {
        self.initial_open_started
            .is_some_and(|started| started.elapsed() >= deadline)
    }

    pub(super) fn is_quiescent(&self) -> bool {
        !self.open_pending
            && self.opening.is_none()
            && self.open_error.is_none()
            && self.document_rebind.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_open_and_relinquish_preserve_identity_custody() {
        let mut lifecycle = OutputLifecycle::new();
        assert!(lifecycle.is_quiescent());

        lifecycle.output_identity = Some(55);
        lifecycle.output_generation = Some(3);
        lifecycle.request_open(Some(48_000), Some(7));
        assert!(lifecycle.open_pending);
        assert_eq!(lifecycle.requested_rate, Some(48_000));
        assert_eq!(lifecycle.restart_request, Some(7));
        assert!(!lifecycle.is_quiescent());

        lifecycle.begin_relinquish();
        lifecycle.finish_relinquish();
        assert_eq!(lifecycle.relinquished_output_identity, Some(55));
        assert!(lifecycle.output_identity.is_none());
        assert!(lifecycle.output_generation.is_none());
        assert!(lifecycle.is_quiescent());
    }

    #[test]
    fn lifecycle_failure_error_maps_all_variants_to_lowercase_recovery_messages() {
        let retiring = lifecycle_failure_error(AudioLifecycleFailure::RetiringEditor);
        assert!(retiring.to_string().contains("vst3 editor shutdown"));
        assert!(!retiring.to_string().contains("restart hex"));

        for failure in [
            AudioLifecycleFailure::Retirement,
            AudioLifecycleFailure::Cleanup,
            AudioLifecycleFailure::MismatchedOwner,
        ] {
            let message = lifecycle_failure_error(failure).to_string();
            assert_eq!(message, message.to_lowercase());
            assert!(message.contains("save your project and restart hex"));
        }
    }

    #[test]
    fn startup_timeout_observes_elapsed_deadline() {
        let mut lifecycle = OutputLifecycle::new();
        assert!(!lifecycle.startup_timed_out(Duration::ZERO));
        lifecycle.initial_open_started = Some(Instant::now());
        assert!(lifecycle.startup_timed_out(Duration::ZERO));
        assert!(!lifecycle.startup_timed_out(Duration::from_secs(60)));
    }
}
