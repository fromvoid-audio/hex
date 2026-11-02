//! Per-voice fade around streamed page misses.
//!
//! A streamed source voice whose page is missing would otherwise drop to
//! silence on the first missing frame and jump back to full level on the
//! first frame after the page arrives, often in the middle of a page. Both
//! edges click. Instead each voice keeps a miss gain `g`. On every rendered
//! frame `g` moves by `1/F` towards 1 when the frame is present and towards 0
//! when it is missing, where `F` is the shared transport declick length
//! (`transport_fade::fade_frames`, 128 at 48 kHz). A present frame plays as
//! `sample * g`. A missing frame conceals with the voice's last present sample
//! held under `g`, so a miss fades out over at most `F` frames and recovery
//! fades in over at most `F` frames from whatever level the voice reached.
//! For any signal of peak `A` the miss adds at most `A/F` to the step between
//! neighbouring frames. A voice that never misses keeps `g = 1` and is
//! bit-identical to the unfaded render.
//!
//! Ownership (decision for #4073, 2 November 2026): the session runtime
//! (`SessionPlayback`) owns one fixed-capacity [`StreamMissFades`] table by
//! value. For each render it lends the table to the page view handed to the
//! source renderers, through [`MediaPages::page_and_miss_fades`]; the shared
//! streamed span renderer (`render_streamed_page_span_captured`) is the only
//! code that reads or changes it, so Arrangement clips, Session clips and
//! Session audio releases share one implementation. Prepared clips stay
//! read-only. Nothing allocates, locks or waits; the table is a plain array.
//!
//! Voice identity and reset rules: a voice is a contiguous read of one
//! streamed source. A span continues the voice whose next expected source
//! frame equals the span's first source frame for the same source key;
//! anything else starts a fresh voice at full gain with a silent held sample.
//! Seek, transport loop wrap, clip loop wrap, scene launch and any other
//! source discontinuity therefore reset by construction, with no command
//! hook, and leave their own declick (transport fade, loop seam crossfade,
//! Session audio release) in charge of the edge. A graph replacement keeps
//! the table, so a voice that continues the same source key and position
//! across the handoff keeps its fade; a new source generation is a new voice.
//! When more voices sound than the table holds, the least recently touched
//! voice is reused; an evicted voice restarts at full gain, which is the
//! previous unfaded behaviour, never worse.
//!
//! The table is non-canonical smoothing state: a render that fails or is
//! discarded may already have advanced it, which can only start a voice afresh
//! at full gain. Frames past the end of a source's final short page are true
//! silence, not a miss: they end the voice without a concealment tail. Page
//! views without a table (exports, verified offline views, legacy linear
//! playback) keep the previous source-local silence on a miss.

use crate::{Frame, MediaPages, StreamSourceKey};

/// Voices tracked at once. Lookup is a linear scan per streamed span, so the
/// table stays small; more simultaneous streamed voices degrade to the
/// unfaded behaviour for the evicted ones.
pub const STREAM_MISS_FADE_VOICES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Voice {
    key: Option<StreamSourceKey>,
    next_source_frame: u64,
    gain: f32,
    held: Frame,
    touched: u64,
}

const IDLE: Voice = Voice {
    key: None,
    next_source_frame: 0,
    gain: 1.0,
    held: [0.0; 2],
    touched: 0,
};

/// Preallocated per-voice miss-fade state; see the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamMissFades {
    voices: [Voice; STREAM_MISS_FADE_VOICES],
    clock: u64,
    step: f32,
}

impl StreamMissFades {
    /// An empty table fading over the shared declick length at `sample_rate`.
    pub fn new(sample_rate: u32) -> Self {
        let frames = crate::playback::fade_frames(sample_rate).max(1);
        Self {
            voices: [IDLE; STREAM_MISS_FADE_VOICES],
            clock: 0,
            step: 1.0 / frames as f32,
        }
    }

    /// The voice continuing `key` at `source_frame`, or a fresh one.
    pub(crate) fn voice(&mut self, key: StreamSourceKey, source_frame: u64) -> MissVoice<'_> {
        self.clock = self.clock.wrapping_add(1);
        let index = self
            .voices
            .iter()
            .position(|voice| voice.key == Some(key) && voice.next_source_frame == source_frame)
            .unwrap_or_else(|| {
                let reuse = self
                    .voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, voice)| (voice.key.is_some(), voice.touched))
                    .map_or(0, |(index, _)| index);
                self.voices[reuse] = Voice {
                    key: Some(key),
                    next_source_frame: source_frame,
                    ..IDLE
                };
                reuse
            });
        let voice = &mut self.voices[index];
        voice.touched = self.clock;
        MissVoice {
            voice,
            step: self.step,
        }
    }
}

/// One voice's fade, borrowed for a single span.
pub(crate) struct MissVoice<'a> {
    voice: &'a mut Voice,
    step: f32,
}

impl MissVoice<'_> {
    /// A present frame: returns it under the (rising) miss gain.
    pub(crate) fn present(&mut self, sample: Frame) -> Frame {
        let voice = &mut *self.voice;
        voice.held = sample;
        voice.next_source_frame = voice.next_source_frame.saturating_add(1);
        if voice.gain >= 1.0 {
            return sample;
        }
        voice.gain = (voice.gain + self.step).min(1.0);
        [sample[0] * voice.gain, sample[1] * voice.gain]
    }

    /// A missing frame: the held sample under the (falling) miss gain.
    pub(crate) fn missing(&mut self) -> Frame {
        let voice = &mut *self.voice;
        voice.next_source_frame = voice.next_source_frame.saturating_add(1);
        voice.gain = (voice.gain - self.step).max(0.0);
        [voice.held[0] * voice.gain, voice.held[1] * voice.gain]
    }

    /// The source ended: no concealment tail, and the next read is fresh.
    pub(crate) fn end(self) {
        *self.voice = IDLE;
    }
}

/// A page view lending a miss-fade table to the source renderers.
pub(crate) struct MissFadedPages<'a, P: MediaPages + ?Sized> {
    pub(crate) pages: &'a mut P,
    pub(crate) fades: &'a mut StreamMissFades,
}

impl<P: MediaPages + ?Sized> MediaPages for MissFadedPages<'_, P> {
    fn page(
        &mut self,
        key: StreamSourceKey,
        page_index: u64,
    ) -> Option<crate::BorrowedMediaPage<'_>> {
        self.pages.page(key, page_index)
    }

    fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.pages.request_page(key, page_index);
    }

    fn scalar_page(
        &mut self,
        key: crate::ScalarSourceKey,
        page_index: u64,
    ) -> Option<crate::BorrowedScalarPage<'_>> {
        self.pages.scalar_page(key, page_index)
    }

    fn request_scalar_page(&mut self, key: crate::ScalarSourceKey, page_index: u64) {
        self.pages.request_scalar_page(key, page_index);
    }

    fn prefetch_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.pages.prefetch_page(key, page_index);
    }

    fn page_and_miss_fades(
        &mut self,
        key: StreamSourceKey,
        page_index: u64,
    ) -> (
        Option<crate::BorrowedMediaPage<'_>>,
        Option<&mut StreamMissFades>,
    ) {
        (self.pages.page(key, page_index), Some(&mut *self.fades))
    }
}

#[cfg(test)]
mod tests;
