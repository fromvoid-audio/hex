//! Original source queries through bounded scalar coefficients and PCM borrows.
use super::*;
use hex_lattice::DeviceResourcePcm;

/// Copyable scalar-only voice coefficients. Gate duration and age use output frames. Releasing
/// a gated voice freezes its ADS level at note-off and ramps to zero; source
/// playback (and a Classic loop) continues during release. Age queries may be
/// made in any order, enabling seek/block-independent original render fixtures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceCoefficients {
    pub(super) expected_frames: usize,
    pub(super) channels: SampleChannels,
    pub(super) region: Region,
    pub(super) loop_offsets: Option<(usize, usize)>,
    pub(super) reverse: bool,
    pub(super) step: u64,
    /// Band-limited reconstruction above unity rate; derived from `step`.
    pub(super) kernel: Option<super::interpolation::Kernel>,
    pub(super) channel_gain: [f64; 2],
    pub(super) gate: GateMode,
    pub(super) envelope: envelope::PreparedEnvelope,
}

impl VoiceCoefficients {
    /// Zero fixed buffering/lookahead; source resampling and envelope timing
    /// do not introduce a block of host latency.
    pub const fn algorithmic_latency_frames(&self) -> u32 {
        0
    }

    pub const fn release_frames(&self) -> u64 {
        match self.gate {
            GateMode::Gate => self.envelope.release_frames(),
            GateMode::Trigger => 0,
        }
    }

    /// Length in output frames for a nonlooping source, excluding envelope
    /// truncation. A loop is unbounded until its gate/release ends. Trigger
    /// voices must be retained to this natural end even after MIDI note-off.
    pub fn natural_frames(&self) -> Option<u64> {
        if self.loop_offsets.is_some() {
            return None;
        }
        let length = (self.region.end - self.region.start) as u128 * ONE;
        u64::try_from(length.div_ceil(u128::from(self.step))).ok()
    }

    /// Exclusive onset-relative lifetime bound for this voice and gate duration.
    /// Trigger ignores note-off; gated playback ends at the earlier of source
    /// exhaustion and envelope release. Loops require a representable gate plus
    /// release. This is not a silence detector or authority to retain resources;
    /// the host must separately check onset addition, cancellation and leases.
    pub fn end_age_frames(&self, gate_frames: u64) -> Result<u64, Invalid> {
        let natural = self.natural_frames();
        if self.gate == GateMode::Trigger {
            return natural.ok_or(Invalid("trigger sample needs a finite natural end"));
        }
        match (natural, gate_frames.checked_add(self.release_frames())) {
            (Some(source), Some(release)) => Ok(source.min(release)),
            (Some(source), None) => Ok(source),
            (None, Some(release)) => Ok(release),
            (None, None) => Err(Invalid("sample gate and release end age overflow")),
        }
    }

    #[inline]
    pub fn sample(
        &self,
        pcm: DeviceResourcePcm<'_>,
        age_frames: u64,
        gate_frames: u64,
    ) -> StereoFrame {
        self.sample_inner::<true>(pcm, age_frames, gate_frames)
    }

    #[inline]
    fn sample_inner<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        age_frames: u64,
        gate_frames: u64,
    ) -> StereoFrame {
        // u64 * u64 fits u128 exactly, even at u64::MAX age. Stateless
        // public queries retain their original age-relative source law.
        let position = u128::from(age_frames) * u128::from(self.step);
        self.sample_position_inner::<CHECK_FINITE>(pcm, age_frames, gate_frames, position)
    }

    /// Mutable gated voices carry actual source distance through tuning changes;
    /// envelope age and gate remain independent output-frame clocks.
    #[inline]
    pub(super) fn sample_at_position(
        &self,
        pcm: DeviceResourcePcm<'_>,
        age_frames: u64,
        gate_frames: u64,
        position: u128,
    ) -> StereoFrame {
        self.sample_position_inner::<true>(pcm, age_frames, gate_frames, position)
    }

    #[inline]
    fn sample_position_inner<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        age_frames: u64,
        gate_frames: u64,
        position: u128,
    ) -> StereoFrame {
        let channels = match pcm {
            DeviceResourcePcm::Mono(_) => SampleChannels::Mono,
            DeviceResourcePcm::Stereo(_) => SampleChannels::Stereo,
        };
        if pcm.frames() != self.expected_frames || channels != self.channels {
            return [0.0; 2];
        }
        let envelope = match self.gate {
            GateMode::Gate => self.envelope.sample(age_frames, gate_frames),
            GateMode::Trigger => self.envelope.held(age_frames),
        };
        if envelope == 0.0 {
            return [0.0; 2];
        }
        let Some(value) = self.source_value::<CHECK_FINITE>(pcm, position) else {
            return [0.0; 2];
        };
        // Gain in f64 so finite f32 sources do not overflow. Saturation at
        // the representation boundary avoids NaN/Inf from otherwise valid
        // finite PCM and the bounded gain domain.
        std::array::from_fn(|channel| {
            let value = value[channel] * self.channel_gain[channel] * envelope;
            value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
        })
    }

    /// Reconstructed source at a Q32 playback position, before gain and
    /// envelope. `None` means silence (past a nonlooping end, a missing frame
    /// or, when checked, non-finite PCM).
    #[inline]
    pub(super) fn source_value<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        mut position: u128,
    ) -> Option<[f64; 2]> {
        let length = self.region.end - self.region.start;
        // Whether playback has wrapped at least once: the loop end then
        // precedes the loop start in playback history.
        let mut wrapped = false;
        // Integer modulo preserves loop phase without converting the carried
        // source cursor (or a stateless age-derived position) to f64.
        if let Some((start, end)) = self.loop_offsets {
            let end = end as u128 * ONE;
            let start = start as u128 * ONE;
            if position >= end {
                position = start + (position - end) % (end - start);
                wrapped = true;
            }
        } else if position >= length as u128 * ONE {
            return None;
        }
        let index = (position >> FRACTION_BITS) as usize;
        let fraction = (position % ONE) as f64 / ONE as f64;
        let source_index = |offset: usize| {
            if self.reverse {
                self.region.end - 1 - offset
            } else {
                self.region.start + offset
            }
        };
        if let Some(kernel) = self.kernel {
            // Pitched-up path; see `interpolation` for the law. Taps follow
            // playback history: past a Classic loop end they continue at its
            // start; before the loop start of a wrapped pass they come from
            // the loop end. Other taps outside the region read silence.
            let index = i64::try_from(index).ok()?;
            return kernel.reconstruct::<CHECK_FINITE>(index, fraction, |offset| {
                let offset = match self.loop_offsets {
                    Some((start, end)) => {
                        let (start, end) = (start as i64, end as i64);
                        let span = end - start;
                        if offset >= end {
                            start + (offset - end) % span
                        } else if wrapped && offset < start {
                            end - 1 - (start - 1 - offset) % span
                        } else {
                            offset
                        }
                    }
                    None => offset,
                };
                let offset = usize::try_from(offset).ok()?;
                if offset >= length {
                    return None;
                }
                pcm.frame(source_index(offset))
            });
        }
        let mut next = index + 1;
        if let Some((start, end)) = self.loop_offsets {
            if next == end {
                next = start;
            }
        }
        next = next.min(length - 1);
        let (Some(a), Some(b)) = (
            pcm.frame(source_index(index)),
            pcm.frame(source_index(next)),
        ) else {
            return None;
        };
        if CHECK_FINITE && a.iter().chain(&b).any(|value| !value.is_finite()) {
            return None;
        }
        // Interpolate in f64 so finite f32 endpoints do not overflow the
        // subtraction.
        Some(std::array::from_fn(|channel| {
            f64::from(a[channel]) * (1.0 - fraction) + f64::from(b[channel]) * fraction
        }))
    }

    /// Fill caller-owned output. Overflowed ages are defined silence; they do
    /// not wrap to frame zero. No cursor, heap, ownership or block-size state.
    pub fn render(
        &self,
        pcm: DeviceResourcePcm<'_>,
        start_age: u64,
        gate_frames: u64,
        output: &mut [StereoFrame],
    ) {
        self.render_inner::<true>(pcm, start_age, gate_frames, output);
    }

    fn render_inner<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        start_age: u64,
        gate_frames: u64,
        output: &mut [StereoFrame],
    ) {
        for (offset, frame) in output.iter_mut().enumerate() {
            *frame = u64::try_from(offset)
                .ok()
                .and_then(|offset| start_age.checked_add(offset))
                .map_or([0.0; 2], |age| {
                    self.sample_inner::<CHECK_FINITE>(pcm, age, gate_frames)
                });
        }
    }
}

/// Original stereo borrowed-voice API. It delegates to the same scalar plan;
/// existing PCM, envelope, pitch and checked lifetime laws remain unchanged.
#[derive(Clone, Copy, Debug)]
pub struct Voice<'a> {
    pub(super) frames: &'a [StereoFrame],
    pub(super) coefficients: VoiceCoefficients,
}
impl Voice<'_> {
    pub fn coefficients(&self) -> VoiceCoefficients {
        self.coefficients
    }
    pub const fn algorithmic_latency_frames(&self) -> u32 {
        self.coefficients.algorithmic_latency_frames()
    }
    pub const fn release_frames(&self) -> u64 {
        self.coefficients.release_frames()
    }
    pub fn natural_frames(&self) -> Option<u64> {
        self.coefficients.natural_frames()
    }
    pub fn end_age_frames(&self, gate_frames: u64) -> Result<u64, Invalid> {
        self.coefficients.end_age_frames(gate_frames)
    }
    #[inline]
    pub fn sample(&self, age_frames: u64, gate_frames: u64) -> StereoFrame {
        // Sample::new already validated every immutable frame on control.
        // Keep that original path free of redundant per-frame finite checks;
        // public raw-PCM queries still use the checked specialization above.
        self.coefficients.sample_inner::<false>(
            DeviceResourcePcm::Stereo(self.frames),
            age_frames,
            gate_frames,
        )
    }
    pub fn render(&self, start_age: u64, gate_frames: u64, output: &mut [StereoFrame]) {
        self.coefficients.render_inner::<false>(
            DeviceResourcePcm::Stereo(self.frames),
            start_age,
            gate_frames,
            output,
        )
    }
}
