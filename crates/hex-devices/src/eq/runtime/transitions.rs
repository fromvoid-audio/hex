use super::*;

#[derive(Clone, Copy)]
pub(super) struct RuntimeBand {
    pub(super) current: BandStages,
    pub(super) target: BandStages,
    pub(super) history: [[[f64; 2]; 4]; 2],
    pub(super) remaining: u16,
    pub(super) shape: u8,
    // A stage that has processed a non-identity filter remains in the bound
    // until its histories are reset, including through coefficient ramps.
    pub(super) stages_used: u8,
}
#[cfg(test)]
impl Default for RuntimeBand {
    fn default() -> Self {
        Self::new([Coefficients::IDENTITY; 4], NodeKind::Bell)
    }
}
impl RuntimeBand {
    pub(super) fn new(stages: BandStages, kind: NodeKind) -> Self {
        Self {
            current: stages,
            target: stages,
            history: [[[0.0; 2]; 4]; 2],
            remaining: 0,
            shape: kind as u8,
            stages_used: stages_used(&stages),
        }
    }

    #[inline]
    pub(super) fn filter_channel(&mut self, channel: usize, input: f64) -> f64 {
        let used = usize::from(self.stages_used);
        let history = &mut self.history[channel];
        let output = filter(input, &self.current[..used], history);
        // Even identity arithmetic can change history on a non-finite input.
        // Preserve that state until reset.
        if used < self.current.len() && !output.is_finite() {
            self.stages_used = self.current.len() as u8;
            filter(output, &self.current[used..], &mut history[used..])
        } else {
            output
        }
    }
}
/// A five-millisecond transition independent of device sample rate.
pub(super) fn coefficient_smooth_frames(sample_rate: u32) -> u16 {
    u16::try_from(sample_rate.div_ceil(200).max(1)).unwrap_or(u16::MAX)
}
/// Frames for a channel-mode crossfade: 256 at 48 kHz, scaled by rate.
pub(super) fn mode_fade_frames(sample_rate: u32) -> u32 {
    let frames = (u64::from(sample_rate) * 256).div_ceil(48_000).max(1);
    u32::try_from(frames).unwrap_or(u32::MAX)
}
/// One band of the outgoing routing: frozen effective coefficients, its
/// history and the bank it filtered.
#[derive(Clone, Copy)]
pub(super) struct OutgoingBand {
    pub(super) stages: BandStages,
    pub(super) history: [[[f64; 2]; 4]; 2],
    pub(super) bank: u8,
}
impl Default for OutgoingBand {
    fn default() -> Self {
        Self {
            stages: [Coefficients::IDENTITY; 4],
            history: [[[0.0; 2]; 4]; 2],
            bank: 0,
        }
    }
}
/// The outgoing routing's band audition: the auditioned band's frozen
/// coefficients and bank, its isolated history and the frozen audition mix.
#[derive(Clone, Copy)]
pub(super) struct OutgoingAudition {
    pub(super) stages: BandStages,
    pub(super) history: [[[f64; 2]; 4]; 2],
    pub(super) bank: u8,
    pub(super) mix: f64,
}
/// Borrowed effective routing only; capture copies into the existing outgoing
/// owner and retains no borrow or resource custody.
pub(super) struct Routing<'a> {
    pub(super) mode: u8,
    pub(super) nodes: &'a [Node],
    pub(super) bands: &'a [RuntimeBand],
    pub(super) banks: &'a [u8; MAX_NODES],
    pub(super) audition: Option<ProcessorAuditionSelection>,
    pub(super) audition_mix: f64,
    pub(super) audition_history: &'a [[[f64; 2]; 4]; 2],
    pub(super) gain: f64,
}
impl<'a> From<&'a EqRuntime> for Routing<'a> {
    fn from(source: &'a EqRuntime) -> Self {
        Self {
            mode: source.mode,
            nodes: &source.configuration.nodes,
            bands: &source.bands,
            banks: &source.effective_banks,
            audition: source.audition,
            audition_mix: source.audition_mix,
            audition_history: &source.audition_history,
            gain: source.gain,
        }
    }
}
/// The outgoing routing after a stereo/left-right/mid-side switch (Pulse
/// #4122, the same law as EQ8's #4106). Band histories cannot be reinterpreted
/// under a new routing, so the previous routing keeps filtering with its
/// frozen effective coefficients, history, gain and band audition (at its
/// frozen audition mix) while the output crossfades linearly into the new
/// routing, which starts from empty history. The crossfade ends exactly on the
/// new routing, then the outgoing state is cleared.
///
/// Capacity is one outgoing branch. Its band storage holds `MAX_NODES` bands,
/// the existing node limit, so any outgoing routing fits regardless of the
/// incoming node count (a mode switch combined with node deletion, such as a
/// state replacement, still crossfades). It is allocated with the runtime on
/// control; capturing, carrying and processing only copy into it. Same-mode
/// handoffs carry an active branch. A further mode change while the branch is
/// still fading is refused as transiently busy (`transition_busy_from`), so
/// the host defers it for at most one fade (256 frames at 48 kHz, 16/3 ms)
/// instead of dropping the older routing. A transport discontinuity or reset
/// ends the crossfade. Shared mode readings instead coalesce to the latest
/// target at the first frame after this branch finishes.
pub(super) struct ModeFade {
    pub(super) bands: Box<[OutgoingBand]>,
    pub(super) len: usize,
    pub(super) audition: Option<OutgoingAudition>,
    pub(super) mode: u8,
    pub(super) gain: f64,
    pub(super) remaining: u32,
    pub(super) total: u32,
}
impl ModeFade {
    pub(super) fn new() -> Self {
        Self {
            bands: vec![OutgoingBand::default(); MAX_NODES].into_boxed_slice(),
            len: 0,
            audition: None,
            mode: 0,
            gain: 1.0,
            remaining: 0,
            total: 0,
        }
    }
    pub(super) fn is_active(&self) -> bool {
        self.remaining > 0
    }
    pub(super) fn clear(&mut self) {
        for band in &mut self.bands[..self.len] {
            band.history = [[[0.0; 2]; 4]; 2];
        }
        self.len = 0;
        self.audition = None;
        self.remaining = 0;
        self.total = 0;
    }
    /// Starts a crossfade out of `previous`'s routing.
    pub(super) fn capture(&mut self, previous: Routing<'_>, sample_rate: u32) {
        self.clear();
        let mode = previous.mode;
        let active = |bank_id: u8| bank_id == bank(mode, 0) || bank_id == bank(mode, 1);
        let outgoing = previous
            .bands
            .iter()
            .zip(previous.banks)
            .filter(|(_, bank_id)| active(**bank_id));
        // `previous.bands` holds at most `MAX_NODES` bands, so every outgoing
        // band has a slot.
        for ((band, bank_id), slot) in outgoing.zip(self.bands.iter_mut()) {
            *slot = OutgoingBand {
                stages: band.current,
                history: band.history,
                bank: *bank_id,
            };
            self.len += 1;
        }
        self.audition = previous
            .audition
            .filter(|_| previous.audition_mix > 0.0)
            .and_then(|selection| {
                previous
                    .nodes
                    .iter()
                    .position(|node| node.id.value() == selection.index)
            })
            .and_then(|index| {
                Some(OutgoingAudition {
                    stages: previous.bands.get(index)?.current,
                    history: *previous.audition_history,
                    bank: *previous.banks.get(index)?,
                    mix: previous.audition_mix,
                })
            });
        self.mode = mode;
        self.gain = previous.gain;
        self.total = mode_fade_frames(sample_rate);
        self.remaining = self.total;
    }
    /// Carries an in-progress crossfade across a same-routing handoff, copying
    /// only the occupied bands.
    pub(super) fn copy_from(&mut self, other: &ModeFade) {
        self.clear();
        if !other.is_active() {
            return;
        }
        self.bands[..other.len].copy_from_slice(&other.bands[..other.len]);
        self.len = other.len;
        self.audition = other.audition;
        self.mode = other.mode;
        self.gain = other.gain;
        self.remaining = other.remaining;
        self.total = other.total;
    }
    pub(super) fn blend(
        &mut self,
        input: StereoFrame,
        current: [f64; 2],
        mapped_gain: Option<f64>,
    ) -> [f64; 2] {
        if !self.is_active() {
            return current;
        }
        let mut working = [f64::from(input[0]), f64::from(input[1])];
        if self.mode == 2 {
            working = [
                (working[0] + working[1]) * 0.5,
                (working[0] - working[1]) * 0.5,
            ];
        }
        let audition_input = working;
        for band in &mut self.bands[..self.len] {
            for (channel, value) in working.iter_mut().enumerate() {
                if band.bank == bank(self.mode, channel) {
                    *value = filter(*value, &band.stages, &mut band.history[channel]);
                }
            }
        }
        if let Some(audition) = &mut self.audition {
            for (channel, value) in working.iter_mut().enumerate() {
                let isolated = if audition.bank == bank(self.mode, channel) {
                    filter(
                        audition_input[channel],
                        &audition.stages,
                        &mut audition.history[channel],
                    )
                } else {
                    0.0
                };
                *value += (isolated - *value) * audition.mix;
            }
        }
        if self.mode == 2 {
            working = [working[0] + working[1], working[0] - working[1]];
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.clear();
            return current;
        }
        let weight = f64::from(self.remaining) / f64::from(self.total);
        let gain = mapped_gain.unwrap_or(self.gain);
        std::array::from_fn(|channel| {
            current[channel] + (working[channel] * gain - current[channel]) * weight
        })
    }
}
