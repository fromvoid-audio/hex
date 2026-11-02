//! Declicked Arrangement audio at the transport loop seam.
//!
//! When the transport wraps from the loop end back to its start, an
//! Arrangement audio source would jump from the content at the loop end to the
//! content at the loop start. For the first `F` frames of every wrapped pass
//! (`F` is the transport fade length, 128 frames at 48 kHz) each track's
//! Arrangement audio is an equal-gain linear crossfade instead: the outgoing
//! side keeps reading the timeline past the loop end, `end + k`, at gain
//! `(F-1-k)/F`, while the loop-start content `start + k` ramps in at
//! `(k+1)/F`. The step at the seam between any two signals of peak `A` is at
//! most `A/F`, and content that is continuous across the seam (the material
//! past the loop end equals the material at its start) passes unchanged.
//!
//! The crossfade is added as a correction `out(k) * (past_end - at_start)` to
//! the ordinary source render, through the same clip readers and track source
//! mix, so it is exactly zero for continuous content. It is a pure function of
//! the source's arrangement frame and whether the pass wrapped: no state, no
//! render split, and results do not depend on block size. Corrections use two
//! fixed stack buffers; nothing allocates.
//!
//! Scope: Arrangement audio clips only. Instrument/MIDI sources, Session clips,
//! device chains and their tails are untouched; devices simply process the
//! corrected source, so output latency is followed naturally. The first pass,
//! entered by Play or a seek, keeps its first transient. Loops shorter than
//! `2F` frames repeat at audio rate and are not crossfaded. Where nothing
//! sounds past the loop end (a clip ending there) the outgoing side is silent
//! and the loop-start content simply ramps in.
//!
//! Streamed sources read the overrun `[end, end + F)` through the same page
//! provider. Exact next-chunk stream demand for a wrapped pass's first chunks
//! also enumerates the overrun those chunks read, so the overrun pages are
//! pinned with the chunk and the crossfade never renders from a missing page:
//! when an overrun page is not yet resident, the existing readiness rule holds
//! the transport for that chunk while it loads, exactly as at a linear page
//! crossing, and later wraps find it cached. The overrun always lies in the
//! page holding `end - 1` or the page after it, so the span renderer's bounded
//! lookahead (`MediaPages::prefetch_page`, #4075), issued from the second half
//! of the page the playhead reads before the wrap, normally makes it resident
//! before the first wrap. Pinning the overrun for the whole loop instead would
//! permanently cost up to two pool pages per looped streamed track and starve
//! small pools.

use crate::{Frame, MediaPages, PreparedTrackMix, Track};
use hex_project::LoopRegion;

/// Stack sub-block for the crossfade correction.
const SUB_BLOCK: usize = 64;

/// The seam of one wrapped pass of an active transport loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoopSeam {
    start: u64,
    end: u64,
    ramp: u64,
}

impl LoopSeam {
    /// The seam of a pass that began by wrapping (`loop_pass > 0`), or `None`
    /// for the first pass and for loops shorter than two ramps.
    pub(crate) fn for_pass(region: LoopRegion, loop_pass: u64, sample_rate: u32) -> Option<Self> {
        let ramp = u64::from(crate::playback::fade_frames(sample_rate));
        let length = region.end().checked_sub(region.start())?;
        (loop_pass > 0 && length >= ramp.saturating_mul(2)).then_some(Self {
            start: region.start(),
            end: region.end(),
            ramp,
        })
    }

    /// The overrun range read for a source chunk at `start` of `frames`
    /// frames: its first frame past the loop end and its length, if the chunk
    /// begins inside the crossfade.
    pub(crate) fn overrun(self, start: u64, frames: usize) -> Option<(u64, usize)> {
        let offset = start.checked_sub(self.start).filter(|k| *k < self.ramp)?;
        let count = usize::try_from(self.ramp - offset)
            .unwrap_or(usize::MAX)
            .min(frames);
        let overrun = self.end.checked_add(offset)?;
        overrun.checked_add(count as u64)?;
        start.checked_add(count as u64)?;
        Some((overrun, count))
    }
}

impl Track {
    /// Add the seam crossfade correction to `output`, which already holds this
    /// track's ordinary audio for the chunk at `start` rendered with `mix`.
    /// `capture`, when present, receives the same correction.
    pub(crate) fn add_loop_seam_crossfade<P: MediaPages + ?Sized>(
        &self,
        seam: Option<LoopSeam>,
        start: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        mut capture: Option<&mut [Frame]>,
        mix: PreparedTrackMix,
    ) {
        let Some(seam) = seam else {
            return;
        };
        if self.clips.is_empty() {
            return;
        }
        let Some((overrun, count)) = seam.overrun(start, output.len()) else {
            return;
        };
        let first = start - seam.start;
        let scale = 1.0 / seam.ramp as f32;
        let mut done = 0;
        while done < count {
            let len = (count - done).min(SUB_BLOCK);
            let mut at_start = [[0.0_f32; 2]; SUB_BLOCK];
            let mut past_end = [[0.0_f32; 2]; SUB_BLOCK];
            let offset = done as u64;
            self.render_audio_range_captured_with_mix(
                start + offset,
                start + offset + len as u64,
                &mut at_start[..len],
                pages.as_deref_mut(),
                None,
                mix,
            );
            self.render_audio_range_captured_with_mix(
                overrun + offset,
                overrun + offset + len as u64,
                &mut past_end[..len],
                pages.as_deref_mut(),
                None,
                mix,
            );
            for index in 0..len {
                let k = first + offset + index as u64;
                let out = (seam.ramp - 1 - k) as f32 * scale;
                let (a, b) = (at_start[index], past_end[index]);
                let correction = [out * (b[0] - a[0]), out * (b[1] - a[1])];
                if correction == [0.0; 2] {
                    continue;
                }
                let frame = &mut output[done + index];
                frame[0] += correction[0];
                frame[1] += correction[1];
                if let Some(frame) = capture
                    .as_deref_mut()
                    .and_then(|capture| capture.get_mut(done + index))
                {
                    frame[0] += correction[0];
                    frame[1] += correction[1];
                }
            }
            done += len;
        }
    }
}

#[cfg(test)]
mod tests;
