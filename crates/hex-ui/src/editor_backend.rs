//! Narrow desktop-to-editor contract for asynchronous native audio ownership.
//!
//! The editor receives only a borrowed-control client and scalar/document
//! metadata. The desktop and Session worker retain publication and stream
//! ownership; these events report readiness and publication progress without
//! transferring those owners into GPUI.

use hex_audio::prepared::{Client, Status};
use hex_engine::AudioSourceContext;
use hex_session::{DocumentAudioMode, SourcePlanReservation, service::Service};
use std::sync::mpsc::Receiver;

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
    Unavailable {
        project_generation: u64,
        restart_request: Option<u64>,
        error: String,
    },
    Publication {
        context: AudioSourceContext,
        output_identity: u64,
        state: EditorPublicationState,
    },
}

/// UI-safe summary of a worker-owned publication attempt.
#[derive(Debug)]
pub enum EditorPublicationState {
    Preparing,
    Busy,
    Submitted { audio_revision: u64 },
    Refused(String),
    Fault(String),
}
