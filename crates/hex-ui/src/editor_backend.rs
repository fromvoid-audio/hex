//! Narrow desktop-to-editor contract for asynchronous native audio ownership.
//!
//! The editor receives a borrowed-control client, document metadata and bounded
//! resource-free meter/MIDI observations. Desktop and Session retain publication,
//! source and stream ownership; these events never transfer those owners into GPUI.

use hex_audio::prepared::{Client, Status};
use hex_engine::AudioSourceContext;
use hex_session::{DocumentAudioMode, SourcePlanReservation, service::Service};
use std::sync::{Arc, mpsc::Receiver};

/// Non-owning command/status access for one published native output.
pub struct EditorAudio {
    pub control: Client,
    pub sample_rate: u32,
    pub label: String,
    pub spectrum_error: Option<String>,
    pub mode: DocumentAudioMode,
}

impl EditorAudio {
    /// Borrow status only while this client still owns its endpoint.
    pub fn status(&self) -> Option<&Status> {
        self.control.status()
    }
}

/// Backend pieces composed by the desktop and consumed by the canonical editor.
pub struct EditorBackend {
    pub service: Service,
    pub audio_events: Receiver<EditorAudioEvent>,
    pub source_reservation: SourcePlanReservation,
}

/// Readiness and publication observations delivered to the UI.
pub enum EditorAudioEvent {
    /// Close only the named old UI endpoint for a preflighted fresh-output
    /// handoff. This is neither native drain proof nor a project edit.
    RelinquishClient {
        old_output_identity: u64,
        target: AudioSourceContext,
    },
    Opened {
        project_generation: u64,
        restart_request: Option<u64>,
        audio: EditorAudio,
    },
    /// No output opened. A `RateMismatch` error is a recoverable paused
    /// state; the UI names both rates and offers retry.
    Unavailable {
        project_generation: u64,
        restart_request: Option<u64>,
        error: hex_audio::OutputOpenError,
    },
    Publication {
        context: AudioSourceContext,
        output_identity: u64,
        state: EditorPublicationState,
    },
    /// Project replacement kept this output for `project_generation`. The UI
    /// keeps the endpoint it held across the replacement when its identity
    /// matches; otherwise it requests a fresh output.
    OutputRebound {
        project_generation: u64,
        output_identity: u64,
    },
    /// Project replacement retires this output for `project_generation`. The
    /// UI closes the endpoint it held across the replacement so the worker can
    /// open a fresh output; the next `Opened`/`Unavailable` follows.
    OutputReleased {
        project_generation: u64,
        old_output_identity: u64,
    },
    /// The default output device or its nominal rate changed after this
    /// output opened. The UI moves to the new route through the same
    /// recovery as restart audio. Sent at most once per output.
    OutputRouteChanged { output_identity: u64 },
    /// A pending SDK restart on this exact ordinary installed graph.
    NativeLatencyRestart {
        source: hex_audio::prepared::AppliedAudioSource,
    },
    /// The complete set of vst3 device ids for this document on this output whose endpoint
    /// faulted and now passes audio through (#3977). Sent when the set
    /// changes; an empty set clears every degraded marker.
    DegradedPlugins {
        project_generation: u64,
        output_identity: u64,
        devices: Vec<u64>,
    },
}

/// Sorted, duplicate-free device ids, so membership is a binary search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DegradedDevices(Vec<u64>);
impl DegradedDevices {
    pub(crate) fn new(mut devices: Vec<u64>) -> Self {
        devices.sort_unstable();
        devices.dedup();
        Self(devices)
    }
    pub(crate) fn contains(&self, device: u64) -> bool {
        self.0.binary_search(&device).is_ok()
    }
    #[cfg(test)]
    fn as_slice(&self) -> &[u64] {
        &self.0
    }
}

/// UI-safe summary of a worker-owned publication attempt.
#[derive(Debug)]
pub enum EditorPublicationState {
    Preparing,
    Busy,
    Submitted {
        audio_revision: u64,
    },
    /// Verified-source preview correlation, distinct from canonical or resident
    /// publication. Actual application still requires output/revision callback status.
    VerifiedPreviewSubmitted {
        audio_revision: u64,
        identity: hex_session::service::DevicePreviewIdentity,
    },
    /// Exact submission correlation and bounded resource-free observation endpoints. Application
    /// still needs the output/revision callback ACK; no graph or native resource owner crosses.
    ResidentSubmitted {
        audio_revision: u64,
        metadata: hex_session::service::ResidentPublicationMetadata,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
    },
    Refused(String),
    Fault(String),
}

#[cfg(test)]
mod tests {
    use super::DegradedDevices;

    #[test]
    fn degraded_devices_are_normalized_for_membership() {
        let devices = DegradedDevices::new(vec![9, 3, 9, 5]);
        assert_eq!(devices.as_slice(), [3, 5, 9]);
        assert!([3, 5, 9].into_iter().all(|device| devices.contains(device)));
        assert!(!devices.contains(4));
        assert_eq!(
            DegradedDevices::new(vec![5, 3]),
            DegradedDevices::new(vec![3, 5, 3])
        );
        assert!(!DegradedDevices::default().contains(0));
    }
}
