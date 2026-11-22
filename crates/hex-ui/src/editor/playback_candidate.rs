//! Pending or applied playback candidate metadata shared between controller and seek admission.
use hex_engine::playback::Playback;
use hex_session::service::ViewState;
use std::sync::Arc;

pub(crate) struct PendingPlayback {
    pub(crate) project_revision: u64,
    pub(crate) project_generation: u64,
    pub(crate) state: Arc<ViewState>,
    pub(crate) playback: Playback,
    pub(crate) install_fresh_runtime: bool,
    pub(crate) fresh_runtime_fence: Option<hex_audio::prepared::FreshRuntimeFence>,
    pub(crate) prepared_revision: Option<Arc<hex_engine::PreparedRevision>>,
    pub(crate) seek_request_id: Option<u64>,
    pub(crate) device_preview_identity: Option<hex_session::service::DevicePreviewIdentity>,
    pub(crate) tuning_preview_identity: Option<hex_session::service::TuningPreviewIdentity>,
}

#[derive(Clone)]
pub(crate) struct PlaybackCandidate {
    pub(crate) clip_preview_identity: Option<hex_session::service::ClipMovePreviewIdentity>,
    pub(crate) worker_requires_preroll: Option<bool>,
    pub(crate) observation: Option<Arc<hex_engine::PreparedPlaybackObservation>>,
    pub(crate) audio_revision: u64,
    pub(crate) project_generation: u64,
    pub(crate) state: Arc<ViewState>,
    pub(crate) prepared_revision: Option<Arc<hex_engine::PreparedRevision>>,
    pub(crate) seek_request_id: Option<u64>,
    pub(crate) device_preview_identity: Option<hex_session::service::DevicePreviewIdentity>,
    pub(crate) tuning_preview_identity: Option<hex_session::service::TuningPreviewIdentity>,
}
