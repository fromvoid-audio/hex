//! Interactive transport click; samples are allocated/prepared on control.
//! The callback owns only a sample cursor and expected Arrangement position.
//! Two 20 ms mono buffers require at most 30,720 sample bytes at 192 kHz.
//! The click is mixed after Main processing, so it passes through a mono delay
//! ring of exactly the graph's Main output latency: a click scheduled at content
//! frame F is heard with the music of F. The ring holds at most
//! `MAX_COMPENSATION_FRAMES` samples (256 KiB) and is empty at zero latency.
//! It delays the output stream itself, so loop wraps need no special case.
//! All buffers are retained by Playback through callback retirement and freed on
//! control with the owning graph, separately from Session runtime/preroll
//! scratch. Preroll and linear export do not allocate or mix this click.
use crate::{Frame, Invalid, PreparedTempoMap};
use hex_project::{BeatTime, LoopRegion, MeterMap};

pub(super) struct Metronome {
    normal: Vec<f32>,
    accent: Vec<f32>,
    enabled: bool,
    sample: usize,
    accented: bool,
    expected_frame: Option<u64>,
    /// Main-latency delay ring; `delay.len()` is the latency in frames.
    delay: Vec<f32>,
    delay_cursor: usize,
    /// The ring may hold nonzero samples, so a reset must clear it. This avoids
    /// a full clear on every stopped block.
    delay_dirty: bool,
}

impl Metronome {
    pub(super) fn visit_candidate_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self {
            normal,
            accent,
            enabled: _,
            sample: _,
            accented: _,
            expected_frame: _,
            delay,
            delay_cursor: _,
            delay_dirty: _,
        } = self;
        ledger.vec(normal)?;
        ledger.vec(accent)?;
        if delay.capacity() > 0 {
            ledger.vec(delay)?;
        }
        Ok(())
    }
    /// Prepare the click for a graph whose Main output arrives `latency_frames`
    /// after its content frame.
    pub(super) fn prepare(rate: u32, latency_frames: u32) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(Invalid("metronome sample rate must be 8000..192000 hz"));
        }
        if latency_frames > crate::compensation::MAX_COMPENSATION_FRAMES {
            return Err(Invalid(
                "metronome latency exceeds the compensation frame limit",
            ));
        }
        let mut delay = Vec::new();
        delay
            .try_reserve_exact(latency_frames as usize)
            .map_err(|_| Invalid("metronome delay allocation refused"))?;
        delay.resize(latency_frames as usize, 0.0);
        let length = (rate / 50) as usize;
        let mut normal = Vec::new();
        let mut accent = Vec::new();
        normal
            .try_reserve_exact(length)
            .map_err(|_| Invalid("metronome sample allocation refused"))?;
        accent
            .try_reserve_exact(length)
            .map_err(|_| Invalid("metronome sample allocation refused"))?;
        for index in 0..length {
            let time = index as f64 / f64::from(rate);
            // A finite 20 ms tapered click, bounded at 0.16 per channel.
            let envelope = (1.0 - index as f64 / length as f64).powi(3);
            normal.push((0.10 * envelope * (std::f64::consts::TAU * 1_000.0 * time).cos()) as f32);
            accent.push((0.16 * envelope * (std::f64::consts::TAU * 1_500.0 * time).cos()) as f32);
        }
        Ok(Self {
            normal,
            accent,
            enabled: false,
            sample: length,
            accented: false,
            expected_frame: None,
            delay,
            delay_cursor: 0,
            delay_dirty: false,
        })
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.reset();
        }
        self.enabled = enabled;
    }

    /// Bounded: clears at most the latency ring, and only when it may hold a
    /// click. Stop, seek and graph epoch changes reset the Main delays too.
    pub(super) fn reset(&mut self) {
        self.reset_generator();
        if self.delay_dirty {
            self.delay.fill(0.0);
            self.delay_cursor = 0;
            self.delay_dirty = false;
        }
    }

    fn reset_generator(&mut self) {
        self.sample = self.normal.len();
        self.expected_frame = None;
    }

    /// A compatible handoff keeps the in-flight click and its delayed samples.
    /// Copying the ring is bounded by the latency and does not allocate.
    pub(super) fn inherit(&mut self, previous: &Self, same_timing: bool) {
        self.enabled = previous.enabled;
        if same_timing
            && self.normal.len() == previous.normal.len()
            && self.delay.len() == previous.delay.len()
        {
            self.sample = previous.sample;
            self.accented = previous.accented;
            self.expected_frame = previous.expected_frame;
            self.delay.copy_from_slice(&previous.delay);
            self.delay_cursor = previous.delay_cursor;
            self.delay_dirty = previous.delay_dirty;
        } else {
            self.delay_dirty = true;
            self.reset();
        }
    }

    /// Push one undelayed click sample and return the one now audible.
    fn delayed(&mut self, sample: f32) -> f32 {
        let Some(slot) = self.delay.get_mut(self.delay_cursor) else {
            return sample;
        };
        let audible = std::mem::replace(slot, sample);
        self.delay_cursor += 1;
        if self.delay_cursor == self.delay.len() {
            self.delay_cursor = 0;
        }
        if sample != 0.0 {
            self.delay_dirty = true;
        }
        audible
    }

    /// Arrangement transport beat is the click clock, including Session routes.
    /// Loop wraps cut the old tail before delay; exact loop-start pulses retrigger. Meter
    /// denominator beats start at each marker, with a stronger first beat/bar.
    pub(super) fn mix(
        &mut self,
        timing: &PreparedTempoMap,
        meter: &MeterMap,
        mut frame: u64,
        region: Option<LoopRegion>,
        output: &mut [Frame],
    ) {
        if !self.enabled || output.is_empty() {
            return;
        }
        let region = region.filter(|region| region.enabled() && frame < region.end());
        let mut offset = 0;
        while offset < output.len() {
            if let Some(region) = region
                && frame >= region.end()
            {
                frame = region.start() + (frame - region.start()) % (region.end() - region.start());
            }
            let remaining = output.len() - offset;
            let count = region.map_or(remaining, |region| {
                remaining.min(usize::try_from(region.end() - frame).unwrap_or(usize::MAX))
            });
            if self.expected_frame != Some(frame) {
                // A loop wrap or other content jump restarts the click clock,
                // but already-delayed clicks still sound with the music that
                // is in flight through Main processing.
                self.reset_generator();
            }
            self.mix_span(timing, meter, frame, &mut output[offset..offset + count]);
            offset += count;
            let Some(end) = frame.checked_add(count as u64) else {
                self.reset_generator();
                return;
            };
            frame = end;
            self.expected_frame = Some(frame);
        }
    }

    fn mix_span(
        &mut self,
        timing: &PreparedTempoMap,
        meter: &MeterMap,
        start: u64,
        output: &mut [Frame],
    ) {
        // One lower-bound lookup per span, then one map lookup per beat/marker.
        // At the supported 1000 BPM and 1/64 meter, pulses are at least 30
        // frames apart at 8 kHz, except for at most 1024 meter-marker boundaries.
        let mut next = timing
            .beat_lower_bound_for_frame(start)
            .ok()
            .and_then(|raw| u64::try_from(raw).ok())
            .and_then(|raw| next_pulse(timing, meter, raw));
        for (offset, output) in output.iter_mut().enumerate() {
            let Some(frame) = start.checked_add(offset as u64) else {
                break;
            };
            while let Some((beat, event_frame, accented)) = next {
                if event_frame > frame {
                    break;
                }
                if event_frame == frame {
                    self.sample = 0;
                    self.accented = accented;
                }
                next = beat
                    .raw()
                    .checked_add(1)
                    .and_then(|raw| next_pulse(timing, meter, raw));
            }
            let samples = if self.accented {
                &self.accent
            } else {
                &self.normal
            };
            let click = match samples.get(self.sample) {
                Some(sample) => {
                    self.sample += 1;
                    *sample
                }
                None => 0.0,
            };
            let audible = self.delayed(click);
            output[0] += audible;
            output[1] += audible;
        }
    }
}

fn next_pulse(
    timing: &PreparedTempoMap,
    meter: &MeterMap,
    raw: u64,
) -> Option<(BeatTime, u64, bool)> {
    let points = meter.points();
    let index = points
        .partition_point(|point| point.beat().raw() <= raw)
        .saturating_sub(1);
    let point = points.get(index)?;
    let step = (4 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
    let delta = u128::from(raw - point.beat().raw());
    let step = u128::from(step);
    let ordinal = delta.div_ceil(step);
    let candidate = u128::from(point.beat().raw()) + ordinal * step;
    let next_marker = points.get(index + 1).map(|point| point.beat().raw());
    let (raw, accented) =
        if let Some(marker) = next_marker.filter(|marker| u128::from(*marker) <= candidate) {
            (marker, true)
        } else {
            (
                u64::try_from(candidate).ok()?,
                ordinal % u128::from(point.meter().numerator()) == 0,
            )
        };
    let beat = BeatTime::from_raw(raw);
    Some((beat, timing.beat_to_frame(beat).ok()?, accented))
}

#[cfg(test)]
mod latency_tests;
#[cfg(test)]
mod tests;
