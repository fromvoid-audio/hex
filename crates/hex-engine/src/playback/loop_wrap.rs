//! Declicked Arrangement loop wrap for `Playback`.
//!
//! When the transport wraps from the loop end back to its start, the project
//! mix jumps between unrelated content. The heard seam is ramped out over the
//! `F` frames before it and back in over the `F` frames after it, with the
//! same length and linear shape as `transport_fade` (`F` is its
//! `fade_frames`): `(F-1)/F` down to `0` on the last frame before the seam,
//! then `1/F` up to `1`. A seam between any two signals of peak `A` therefore
//! steps by at most `A/F`. This is an out/in dip, not a crossfade: content
//! that is already continuous across the seam also dips for `2F` frames. Clip
//! edges are not faded here.
//!
//! A loop shorter than `2F` frames (about 5 ms) is left unramped: its wrap
//! repeats at audio rate, and a dip there would modulate every period and
//! keep its content from ever reaching unity gain.
//!
//! The ramp follows the heard seam, which trails the wrap of the transport
//! cursor by the graph's main output latency. Before the wrap the gain is a
//! pure function of the cursor and the active loop region; after it, a
//! counter of output frames since the wrap carries the remaining ramp across
//! callbacks and graph handoff. No render part has to be split, so the result
//! does not depend on block size.
//!
//! Gain is applied to the project mix with the transport fade, before
//! monitor-only metronome and tuning audition audio; device and project-output
//! taps observe the pre-ramp signal. Loop off, a cursor at or after the loop
//! end and a stopped transport are unchanged. Session clips mixed with the
//! looping Arrangement dip with it. The state is one integer: no allocation,
//! locking or ownership transfer.

use crate::Frame;
use hex_project::LoopRegion;

/// Loop-wrap ramp state for one contiguous playing render. `Playback` owns
/// one; offline renderers and oracles that must match `Playback` output keep
/// their own and call `ramp` once per rendered block, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoopWrapRamp {
    /// Output frames since the latest wrap, until its heard seam's ramp-in ends.
    since_wrap: Option<u64>,
}

impl LoopWrapRamp {
    /// Ramp `output`, rendered by a playing transport from cursor `position`.
    /// `region` is the configured loop (inactive once the cursor is at or
    /// after its end), `latency_frames` the main output latency and
    /// `sample_rate` sets `F`. Never allocates.
    pub fn ramp(
        &mut self,
        output: &mut [Frame],
        position: u64,
        region: Option<LoopRegion>,
        latency_frames: u32,
        sample_rate: u32,
    ) {
        let frames = super::transport_fade::fade_frames(sample_rate);
        self.apply(output, true, position, region, latency_frames, frames);
    }

    pub(crate) fn clear(&mut self) {
        self.since_wrap = None;
    }

    /// Ramp one rendered part. `position` is the transport cursor before the
    /// part rendered and `region` its loop, active while the cursor is before
    /// the end; `latency` is the main output latency and `frames` is `F`.
    pub(crate) fn apply(
        &mut self,
        output: &mut [Frame],
        playing: bool,
        position: u64,
        region: Option<LoopRegion>,
        latency: u32,
        frames: u32,
    ) {
        if !playing {
            self.clear();
            return;
        }
        let ramp = u64::from(frames.max(1));
        let latency = u64::from(latency);
        let reach = ramp.saturating_add(latency);
        let region = region.filter(|region| {
            position < region.end() && region.end() - region.start() >= ramp.saturating_mul(2)
        });
        let seam_near = region.is_some_and(|region| {
            region.end() - position <= (output.len() as u64).saturating_add(reach)
        });
        if self.since_wrap.is_none() && !seam_near {
            return;
        }
        let mut position = position;
        for frame in output.iter_mut() {
            let mut gain = 1.0_f32;
            if let Some(region) = region {
                // The cursor is before the end, so the seam is at least one frame away.
                let until_seam = (region.end() - position).saturating_add(latency);
                if until_seam <= ramp {
                    gain = gain.min((until_seam - 1) as f32 / ramp as f32);
                }
            }
            if let Some(elapsed) = self.since_wrap {
                let steps = if elapsed < latency {
                    latency - 1 - elapsed
                } else {
                    elapsed - latency + 1
                };
                if steps < ramp {
                    gain = gain.min(steps as f32 / ramp as f32);
                }
                self.since_wrap = Some(elapsed + 1).filter(|next| *next < reach);
            }
            if gain < 1.0 {
                frame[0] *= gain;
                frame[1] *= gain;
            }
            if let Some(region) = region {
                position += 1;
                if position == region.end() {
                    position = region.start();
                    self.since_wrap = Some(0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
