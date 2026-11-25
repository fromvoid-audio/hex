//! Delegates to [`hex_ui_model::audio_admission`]; pure correlation state and unit tests live in `hex-ui-model`.
pub(super) use hex_ui_model::audio_admission::{
    AdmissionState, AudioAdmissionStatus, LatencyRebuild, ProjectRevision, StoppedPlaybackRestore,
};
pub(super) type PausedRepublish =
    hex_ui_model::audio_admission::PausedRepublish<hex_engine::PlaybackHandoffRejection>;
