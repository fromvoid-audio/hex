// The production controller also owns GPUI and service lifecycles; include only
// its source-owned, value-only correlation boundary in this target.
#[path = "../../src/editor/controller/audio_event_correlation.rs"]
mod audio_event_correlation;

#[path = "../../src/editor/controller/session_feedback.rs"]
mod session_feedback;
