//! Interactive transport click; samples are allocated/prepared on control.
//! The callback owns only a sample cursor and expected Arrangement position.
//! Two 20 ms mono buffers require at most 30,720 sample bytes at 192 kHz.
//! They are retained by Playback through callback retirement and freed on
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
        } = self;
        ledger.vec(normal)?;
        ledger.vec(accent)
    }
    pub(super) fn prepare(rate: u32) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(Invalid("metronome sample rate must be 8000..192000 hz"));
        }
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

    pub(super) fn reset(&mut self) {
        self.sample = self.normal.len();
        self.expected_frame = None;
    }

    pub(super) fn inherit(&mut self, previous: &Self, same_timing: bool) {
        self.enabled = previous.enabled;
        if same_timing && self.normal.len() == previous.normal.len() {
            self.sample = previous.sample;
            self.accented = previous.accented;
            self.expected_frame = previous.expected_frame;
        } else {
            self.reset();
        }
    }

    /// Arrangement transport beat is the click clock, including Session routes.
    /// Loop wraps cut the old tail; exact loop-start pulses retrigger. Meter
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
        let region = region.filter(|region| region.enabled());
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
                self.reset();
            }
            self.mix_span(timing, meter, frame, &mut output[offset..offset + count]);
            offset += count;
            let Some(end) = frame.checked_add(count as u64) else {
                self.reset();
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
            if let Some(sample) = samples.get(self.sample) {
                output[0] += sample;
                output[1] += sample;
                self.sample += 1;
            }
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
mod tests;
