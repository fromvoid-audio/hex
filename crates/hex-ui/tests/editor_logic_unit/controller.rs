// The production controller also owns GPUI and service lifecycles; include only
// its source-owned, value-only correlation, feedback, and publication boundaries.
#[path = "../../src/editor/controller/audio_event_correlation.rs"]
mod audio_event_correlation;

#[allow(dead_code)]
#[path = "../../src/editor/controller/publication.rs"]
mod publication;

#[path = "../../src/editor/controller/session_feedback.rs"]
mod session_feedback;
