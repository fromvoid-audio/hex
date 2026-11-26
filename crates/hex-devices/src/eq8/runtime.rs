use super::*;

mod process;
pub(super) use process::{
    active_band_flags, design_band, finite_f32, process_band, quiet, response_magnitude,
};

pub(super) type Coefficients = hex_dsp::BiquadCoefficients;

pub(super) type BandStages = [Coefficients; 4];
pub(super) type CoefficientBank = [BandStages; BAND_COUNT];
pub(super) type CoefficientTarget = [CoefficientBank; 2];

pub(super) fn channels(target: &CoefficientTarget) -> [&CoefficientBank; 2] {
    let [left, right] = target;
    [left, right]
}

/// Prepared coefficients and admitted bindings for a closed EQ8 instance.
#[derive(Clone)]
pub struct PreparedEq8 {
    pub(super) admitted: [bool; automation::COUNT],
    pub(super) control_values: automation::ControlValues,
    pub(super) canonical_banks: [CoefficientBank; 5],
    pub(super) sample_rate: u32,
    pub(super) mode: ChannelMode,
    pub(super) output_gain: f64,
    pub(super) enabled_bands: [[bool; BAND_COUNT]; 2],
}

impl PreparedEq8 {
    pub(super) fn new(settings: Settings, sample_rate: u32) -> Result<Self, DeviceContractError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut banks = [[[Coefficients::IDENTITY; 4]; BAND_COUNT]; 5];
        for (bank, bank_stages) in banks.iter_mut().enumerate() {
            for (band_index, stages) in bank_stages.iter_mut().enumerate() {
                let band = settings.banks[bank][band_index];
                if band.enabled {
                    *stages = design_band(band, sample_rate)?;
                }
            }
        }
        let selected = automation::selected(settings.mode);
        let mut enabled_bands = [[false; BAND_COUNT]; 2];
        for (channel, bank_index) in selected.into_iter().enumerate() {
            for (band, enabled) in enabled_bands[channel].iter_mut().enumerate() {
                *enabled = settings.banks[bank_index][band].enabled;
            }
        }
        Ok(Self {
            admitted: [false; automation::COUNT],
            control_values: automation::ControlValues::new(settings),
            canonical_banks: banks,
            sample_rate,
            mode: settings.mode,
            output_gain: f64::from(settings.output_gain),
            enabled_bands,
        })
    }

    pub(super) fn runtime(&self) -> Eq8Runtime {
        let coefficients = automation::selected(self.mode).map(|bank| self.canonical_banks[bank]);
        let controls = crate::parameter_binding::Controls::new(self.control_values, self.admitted);
        let active_bands = active_band_flags(&coefficients);
        let band_automation_admitted = self.admitted[..automation::GAIN_SLOT].iter().any(|&v| v);
        Eq8Runtime {
            controls,
            tail_controls_frozen: false,
            routing: automation::routing::RoutingAutomation::new(self.canonical_banks),
            automated_target: None,
            band_automation_admitted,
            band_targets_pending: band_automation_admitted,
            sample_rate: self.sample_rate,
            mode: self.mode,
            canonical_mode: self.mode,
            current_gain: self.output_gain,
            target_gain: self.output_gain,
            coefficients,
            smooth_remaining: 0,
            active_bands,
            history: [[[[0.0; 2]; 4]; BAND_COUNT]; 2],
            enabled_bands: self.enabled_bands,
            audition_selection: None,
            audition_request: None,
            audition_mix: 0.0,
            audition_history: [[[0.0; 2]; 4]; 2],
            mode_fade: ModeFade::INACTIVE,
            rollback: Box::new(Eq8Rollback::new(
                controls,
                self.mode,
                self.output_gain,
                coefficients,
                active_bands,
                self.enabled_bands,
                band_automation_admitted,
            )),
        }
    }
}

impl PreparedOwnedProcessorRuntime for PreparedEq8 {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        automation::slot(id).is_some_and(|i| self.admitted[i])
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        self.supports_parameter_automation(id)
            .then(|| automation::descriptor(id))
            .flatten()
    }
    fn parameter_modulation_semantics(
        &self,
        id: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        automation::semantics(id).filter(|semantics| {
            self.supports_parameter_automation(id)
                && semantics.modulation == hex_lattice_contract::ParameterModulation::SampleRate
        })
    }
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }

    fn supports_power_off(&self) -> bool {
        true
    }

    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<Eq8Runtime>() + std::mem::size_of::<Eq8Rollback>()
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        Ok(Box::new(self.runtime()))
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn frequency_response_at_hz(&self, frequency_hz: f32) -> Option<[f32; 2]> {
        if !frequency_hz.is_finite()
            || frequency_hz <= 0.0
            || frequency_hz >= self.sample_rate as f32 * 0.5
        {
            return None;
        }
        let selected = automation::selected(self.mode);
        let response = std::array::from_fn(|channel| {
            let mut magnitude = self.output_gain;
            for band in 0..BAND_COUNT {
                for stage in 0..4 {
                    magnitude *= response_magnitude(
                        self.canonical_banks[selected[channel]][band][stage],
                        f64::from(frequency_hz),
                        f64::from(self.sample_rate),
                    );
                }
            }
            finite_f32(magnitude)
        });
        Some(response)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }
}

pub(super) const DIRTY_SCALARS: u8 = 1 << 0;
pub(super) const DIRTY_COEFFS: u8 = 1 << 1;
pub(super) const DIRTY_AUDITION: u8 = 1 << 2;
pub(super) const DIRTY_MODE_FADE: u8 = 1 << 3;
pub(super) const DIRTY_CONTROLS: u8 = 1 << 4;
pub(super) const DIRTY_AUTOMATED_TARGET: u8 = 1 << 5;
pub(super) const DIRTY_PENDING_ROUTING: u8 = 1 << 6;

/// Preallocated bounded dirty-range rollback journal for `Eq8Runtime`.
/// Immutable canonical coefficient banks live once on `Eq8Runtime` and are never
/// duplicated or copied across speculative block boundaries.
#[derive(Clone, Debug)]
pub(super) struct Eq8Rollback {
    pub(super) open: bool,
    pub(super) committed_quiescent: bool,
    pub(super) dirty_flags: u8,
    pub(super) dirty_history: u16,
    pub(super) mode: ChannelMode,
    pub(super) current_gain: f64,
    pub(super) target_gain: f64,
    pub(super) smooth_remaining: u16,
    pub(super) active_bands: [[bool; BAND_COUNT]; 2],
    pub(super) enabled_bands: [[bool; BAND_COUNT]; 2],
    pub(super) band_targets_pending: bool,
    pub(super) tail_controls_frozen: bool,
    pub(super) history: [[[[f64; 2]; 4]; BAND_COUNT]; 2],
    pub(super) coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
    pub(super) audition_selection: Option<Eq8AuditionSelection>,
    pub(super) audition_request: Option<Eq8AuditionSelection>,
    pub(super) audition_mix: f64,
    pub(super) audition_history: [[[f64; 2]; 4]; 2],
    pub(super) mode_fade: ModeFade,
    pub(super) controls:
        crate::parameter_binding::Controls<automation::ControlValues, { automation::COUNT }>,
    pub(super) automated_target: Option<CoefficientTarget>,
    pub(super) pending_routing: Option<automation::routing::PendingRouting>,
}

impl Eq8Rollback {
    pub(super) fn new(
        controls: crate::parameter_binding::Controls<
            automation::ControlValues,
            { automation::COUNT },
        >,
        mode: ChannelMode,
        gain: f64,
        coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
        active_bands: [[bool; BAND_COUNT]; 2],
        enabled_bands: [[bool; BAND_COUNT]; 2],
        band_targets_pending: bool,
    ) -> Self {
        Self {
            open: false,
            committed_quiescent: true,
            dirty_flags: 0,
            dirty_history: 0,
            mode,
            current_gain: gain,
            target_gain: gain,
            smooth_remaining: 0,
            active_bands,
            enabled_bands,
            band_targets_pending,
            tail_controls_frozen: false,
            history: [[[[0.0; 2]; 4]; BAND_COUNT]; 2],
            coefficients,
            audition_selection: None,
            audition_request: None,
            audition_mix: 0.0,
            audition_history: [[[0.0; 2]; 4]; 2],
            mode_fade: ModeFade::INACTIVE,
            controls,
            automated_target: None,
            pending_routing: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Eq8Runtime {
    pub(super) controls:
        crate::parameter_binding::Controls<automation::ControlValues, { automation::COUNT }>,
    pub(super) tail_controls_frozen: bool,
    // One fixed canonical bank inventory serves both normal playback and
    // mode changes; never duplicated in rollback storage.
    pub(super) routing: automation::routing::RoutingAutomation,
    pub(super) automated_target: Option<CoefficientTarget>,
    pub(super) band_automation_admitted: bool,
    pub(super) band_targets_pending: bool,
    pub(super) sample_rate: u32,
    pub(super) mode: ChannelMode,
    pub(super) canonical_mode: ChannelMode,
    pub(super) current_gain: f64,
    pub(super) target_gain: f64,
    pub(super) coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
    pub(super) smooth_remaining: u16,
    pub(super) active_bands: [[bool; BAND_COUNT]; 2],
    pub(super) history: [[[[f64; 2]; 4]; BAND_COUNT]; 2],
    pub(super) enabled_bands: [[bool; BAND_COUNT]; 2],
    pub(super) audition_selection: Option<Eq8AuditionSelection>,
    pub(super) audition_request: Option<Eq8AuditionSelection>,
    pub(super) audition_mix: f64,
    pub(super) audition_history: [[[f64; 2]; 4]; 2],
    pub(super) mode_fade: ModeFade,
    pub(super) rollback: Box<Eq8Rollback>,
}

/// Coefficient and output-gain smoothing lasts 256 frames at the 48 kHz
/// reference rate (16/3 ms). Other rates scale the frame count so the
/// smoothing time is the same everywhere (43 frames at 8 kHz, 1024 at 192 kHz).
pub(super) const COEFFICIENT_SMOOTH_REFERENCE_FRAMES: u32 = 256;
pub(super) const COEFFICIENT_SMOOTH_REFERENCE_RATE: u32 = 48_000;

/// Frames for one coefficient handoff at `sample_rate`; at least one frame.
pub(super) fn coefficient_smooth_frames(sample_rate: u32) -> u16 {
    let frames = u64::from(sample_rate) * u64::from(COEFFICIENT_SMOOTH_REFERENCE_FRAMES);
    let frames = frames.div_ceil(u64::from(COEFFICIENT_SMOOTH_REFERENCE_RATE));
    u16::try_from(frames).unwrap_or(u16::MAX).max(1)
}

/// Frames for a channel-mode crossfade: 256 at 48 kHz, scaled by rate.
pub(super) fn mode_fade_frames(sample_rate: u32) -> u32 {
    let frames = (u64::from(sample_rate) * 256).div_ceil(48_000).max(1);
    u32::try_from(frames).unwrap_or(u32::MAX)
}

/// The outgoing routing after a stereo/left-right/mid-side switch (Pulse
/// #4106). Lane histories cannot be reinterpreted under a new routing, so the
/// previous routing keeps filtering with its frozen effective coefficients,
/// history, gain and band audition (at its frozen audition mix) while the
/// output crossfades linearly into the new routing, which starts from empty
/// history. The crossfade ends exactly on the new routing, then the outgoing
/// history is cleared.
///
/// Capacity is one outgoing branch, in fixed inline storage with no
/// allocation. Same-mode handoffs carry an active branch unchanged. A further
/// ordinary canonical mode change while the branch is still fading is refused as transiently busy
/// (`transition_busy_from`), so the host defers it for at most one fade
/// (256 frames at 48 kHz, 16/3 ms) instead of dropping the older routing.
#[derive(Clone, Copy, Debug)]
pub(super) struct ModeFade {
    pub(super) mode: ChannelMode,
    pub(super) gain: f64,
    pub(super) coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
    pub(super) history: [[[[f64; 2]; 4]; BAND_COUNT]; 2],
    pub(super) audition_selection: Option<Eq8AuditionSelection>,
    pub(super) audition_mix: f64,
    pub(super) audition_history: [[[f64; 2]; 4]; 2],
    pub(super) remaining: u32,
    pub(super) total: u32,
}

impl ModeFade {
    const INACTIVE: Self = Self {
        mode: ChannelMode::Stereo,
        gain: 1.0,
        coefficients: [[[Coefficients::IDENTITY; 4]; BAND_COUNT]; 2],
        history: [[[[0.0; 2]; 4]; BAND_COUNT]; 2],
        audition_selection: None,
        audition_mix: 0.0,
        audition_history: [[[0.0; 2]; 4]; 2],
        remaining: 0,
        total: 0,
    };

    pub(super) fn from_outgoing(previous: &Eq8Runtime, sample_rate: u32) -> Self {
        let total = mode_fade_frames(sample_rate);
        Self {
            mode: previous.mode,
            gain: previous.current_gain,
            coefficients: previous.coefficients,
            history: previous.history,
            audition_selection: previous.audition_selection,
            audition_mix: previous.audition_mix,
            audition_history: previous.audition_history,
            remaining: total,
            total,
        }
    }

    pub(super) fn is_active(&self) -> bool {
        self.remaining > 0
    }

    pub(super) fn blend(&mut self, input: StereoFrame, current: StereoFrame) -> StereoFrame {
        if !self.is_active() {
            return current;
        }
        let source = route_input(self.mode, input);
        let mut filtered = source;
        for (channel, value) in filtered.iter_mut().enumerate() {
            for band in 0..BAND_COUNT {
                *value = process_band(
                    *value,
                    &self.coefficients[channel][band],
                    &mut self.history[channel][band],
                );
            }
        }
        let output = route_output(self.mode, filtered);
        let normal = [
            finite_f32(output[0] * self.gain),
            finite_f32(output[1] * self.gain),
        ];
        let outgoing = match self.audition_selection {
            Some(selection) => {
                let audition = audition_signal(
                    self.mode,
                    selection,
                    &self.coefficients,
                    &mut self.audition_history,
                    source,
                );
                mix_audition(normal, audition, self.gain, self.audition_mix)
            }
            None => normal,
        };
        self.remaining -= 1;
        if self.remaining == 0 {
            self.history = [[[[0.0; 2]; 4]; BAND_COUNT]; 2];
            self.audition_history = [[[0.0; 2]; 4]; 2];
            self.audition_selection = None;
            self.audition_mix = 0.0;
            return current;
        }
        let weight = f64::from(self.remaining) / f64::from(self.total);
        std::array::from_fn(|channel| {
            let incoming = f64::from(current[channel]);
            finite_f32(incoming + (f64::from(outgoing[channel]) - incoming) * weight)
        })
    }
}

/// The solo signal of one auditioned band under `mode`, before output gain.
pub(super) fn audition_signal(
    mode: ChannelMode,
    selection: Eq8AuditionSelection,
    coefficients: &[[[Coefficients; 4]; BAND_COUNT]; 2],
    history: &mut [[[f64; 2]; 4]; 2],
    source: [f64; 2],
) -> [f64; 2] {
    let mut audition = source;
    let channels: &[usize] = match mode {
        ChannelMode::Stereo => &[0, 1],
        ChannelMode::LeftRight | ChannelMode::MidSide => {
            if selection.edit_lane() == 0 {
                &[0]
            } else {
                &[1]
            }
        }
    };
    for &channel in channels {
        audition[channel] = process_band(
            audition[channel],
            &coefficients[channel][selection.band()],
            &mut history[channel],
        );
    }
    match mode {
        ChannelMode::Stereo | ChannelMode::LeftRight => audition,
        ChannelMode::MidSide => [audition[0] + audition[1], audition[0] - audition[1]],
    }
}

/// Crossfade the normal output toward the gained audition by `mix`.
pub(super) fn mix_audition(
    normal: StereoFrame,
    audition: [f64; 2],
    gain: f64,
    mix: f64,
) -> StereoFrame {
    std::array::from_fn(|channel| {
        let normal = f64::from(normal[channel]);
        finite_f32(
            (normal + (audition[channel] * gain - normal) * mix)
                .clamp(-f64::from(f32::MAX), f64::from(f32::MAX)),
        )
    })
}

pub(super) fn route_input(mode: ChannelMode, input: StereoFrame) -> [f64; 2] {
    match mode {
        ChannelMode::Stereo | ChannelMode::LeftRight => [f64::from(input[0]), f64::from(input[1])],
        ChannelMode::MidSide => [
            (f64::from(input[0]) + f64::from(input[1])) * 0.5,
            (f64::from(input[0]) - f64::from(input[1])) * 0.5,
        ],
    }
}

pub(super) fn route_output(mode: ChannelMode, filtered: [f64; 2]) -> [f64; 2] {
    match mode {
        ChannelMode::Stereo | ChannelMode::LeftRight => filtered,
        ChannelMode::MidSide => [filtered[0] + filtered[1], filtered[0] - filtered[1]],
    }
}
