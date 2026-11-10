// Runtime consumers use more methods than this unit-only target.
#[allow(dead_code)]
#[path = "../../src/editor/musical_time.rs"]
mod musical_time;
#[allow(dead_code)]
#[path = "../../src/editor/seek.rs"]
mod seek;
#[allow(dead_code)]
#[path = "../../src/editor/signed_frame_viewport.rs"]
mod signed_frame_viewport;
#[allow(dead_code)]
#[path = "../../src/editor/viewport.rs"]
mod viewport;

#[path = "midi_cue.rs"]
mod midi_cue;
