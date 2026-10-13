//! Additive live analytic sampling; authored samplers retain their existing ABI.
use crate::{ANALYTIC_NOTE_PAYLOAD_WORDS, StereoFrame};

/// A genuinely open note gate or the actual note age at its NoteOff boundary.
/// Held has no fabricated duration or release deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteGate {
    Held,
    Released { at_age_frames: u64 },
}

/// Control-prepared, fixed-storage live analytic note. The provider explicitly
/// opts into an open-gate sampler; this is not an authored-sampler fallback.
#[derive(Clone, Copy)]
pub struct PreparedLiveAnalyticNote {
    payload: [u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    release_frames: u64,
    sample_fn: fn(&[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], u64, NoteGate, u64) -> StereoFrame,
}
impl PreparedLiveAnalyticNote {
    pub const fn new(
        payload: [u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
        release_frames: u64,
        sample_fn: fn(&[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], u64, NoteGate, u64) -> StereoFrame,
    ) -> Self {
        Self {
            payload,
            release_frames,
            sample_fn,
        }
    }
    pub const fn release_frames(&self) -> u64 {
        self.release_frames
    }
    pub fn sample(&self, age_frames: u64, gate: NoteGate) -> StereoFrame {
        (self.sample_fn)(&self.payload, age_frames, gate, self.release_frames)
    }
}
