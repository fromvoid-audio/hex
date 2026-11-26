use super::*;

impl Eq8Runtime {
    /// Control-side storage observation: inline runtime bytes plus preallocated
    /// rollback buffer bytes.
    pub fn control_storage(&self) -> Option<(usize, [usize; 2])> {
        Some((
            std::mem::size_of::<Self>(),
            [std::mem::size_of::<Eq8Rollback>(), 0],
        ))
    }

    pub(in super::super) fn snapshot_scalars(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_SCALARS) == 0 {
            self.rollback.mode = self.mode;
            self.rollback.current_gain = self.current_gain;
            self.rollback.target_gain = self.target_gain;
            self.rollback.smooth_remaining = self.smooth_remaining;
            self.rollback.active_bands = self.active_bands;
            self.rollback.enabled_bands = self.enabled_bands;
            self.rollback.band_targets_pending = self.band_targets_pending;
            self.rollback.tail_controls_frozen = self.tail_controls_frozen;
            self.rollback.dirty_flags |= DIRTY_SCALARS;
        }
    }

    pub(in super::super) fn snapshot_band_history(&mut self, channel: usize, band: usize) {
        let bit = 1_u16 << (channel * BAND_COUNT + band);
        if self.rollback.open && (self.rollback.dirty_history & bit) == 0 {
            self.rollback.history[channel][band] = self.history[channel][band];
            self.rollback.dirty_history |= bit;
        }
    }

    pub(in super::super) fn snapshot_all_history(&mut self) {
        if !self.rollback.open || self.rollback.dirty_history == u16::MAX {
            return;
        }
        for channel in 0..2 {
            for band in 0..BAND_COUNT {
                self.snapshot_band_history(channel, band);
            }
        }
    }

    pub(in super::super) fn snapshot_coeffs(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_COEFFS) == 0 {
            self.rollback.coefficients = self.coefficients;
            self.rollback.dirty_flags |= DIRTY_COEFFS;
        }
    }

    pub(in super::super) fn snapshot_audition(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_AUDITION) == 0 {
            self.rollback.audition_selection = self.audition_selection;
            self.rollback.audition_request = self.audition_request;
            self.rollback.audition_mix = self.audition_mix;
            self.rollback.audition_history = self.audition_history;
            self.rollback.dirty_flags |= DIRTY_AUDITION;
        }
    }

    pub(in super::super) fn snapshot_mode_fade(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_MODE_FADE) == 0 {
            self.rollback.mode_fade = self.mode_fade;
            self.rollback.dirty_flags |= DIRTY_MODE_FADE;
        }
    }

    pub(in super::super) fn snapshot_controls(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_CONTROLS) == 0 {
            self.rollback.controls = self.controls;
            self.rollback.dirty_flags |= DIRTY_CONTROLS;
        }
    }

    pub(in super::super) fn snapshot_automated_target(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_AUTOMATED_TARGET) == 0 {
            self.rollback.automated_target = self.automated_target;
            self.rollback.dirty_flags |= DIRTY_AUTOMATED_TARGET;
        }
    }

    pub(in super::super) fn snapshot_pending_routing(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_PENDING_ROUTING) == 0 {
            self.rollback.pending_routing = self.routing.pending();
            self.rollback.dirty_flags |= DIRTY_PENDING_ROUTING;
        }
    }

    /// Request a transient band audition. Invalid or disabled bands leave the
    /// current audition request untouched. Mode- or lane changes crossfade
    /// through the normal cascade and clear the parallel history at the switch.
    pub fn set_audition_selection(
        &mut self,
        selection: Option<Eq8AuditionSelection>,
    ) -> Result<(), DeviceContractError> {
        if selection.is_some_and(|selection| !self.selection_is_enabled(selection)) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        self.audition_request = selection;
        Ok(())
    }

    pub fn process_frame(&mut self, input: StereoFrame) -> StereoFrame {
        self.advance_routing();
        let current = self.process_current_routing(input);
        if self.mode_fade.is_active() {
            self.snapshot_mode_fade();
        }
        self.mode_fade.blend(input, current)
    }

    pub(in super::super) fn process_current_routing(&mut self, input: StereoFrame) -> StereoFrame {
        if self.smooth_remaining > 0 {
            self.snapshot_scalars();
            self.snapshot_coeffs();
            self.advance_coefficients();
        }
        if self.audition_selection != self.audition_request || self.audition_selection.is_some() {
            self.snapshot_audition();
            self.advance_audition();
        }
        let source = route_input(self.mode, input);
        let mut filtered = source;
        for (channel, filtered_channel) in filtered.iter_mut().enumerate() {
            for band in 0..BAND_COUNT {
                if !self.active_bands[channel][band] {
                    continue;
                }
                self.snapshot_band_history(channel, band);
                *filtered_channel = process_band(
                    *filtered_channel,
                    &self.coefficients[channel][band],
                    &mut self.history[channel][band],
                );
                let target = self
                    .routing
                    .effective_target(self.automated_target.as_ref(), self.canonical_mode)[channel]
                    [band];
                let still_active = (0..4).any(|stage| {
                    self.coefficients[channel][band][stage] != Coefficients::IDENTITY
                        || target[stage] != Coefficients::IDENTITY
                        || self.history[channel][band][stage]
                            .iter()
                            .any(|value| *value != 0.0)
                });
                if !still_active {
                    self.snapshot_scalars();
                    self.active_bands[channel][band] = false;
                }
            }
        }
        let output = route_output(self.mode, filtered);
        let normal = [
            finite_f32(output[0] * self.current_gain),
            finite_f32(output[1] * self.current_gain),
        ];
        let Some(selection) = self.audition_selection else {
            return normal;
        };
        let audition = audition_signal(
            self.mode,
            selection,
            &self.coefficients,
            &mut self.audition_history,
            source,
        );
        mix_audition(normal, audition, self.current_gain, self.audition_mix)
    }

    pub fn process(&mut self, frames: &mut [StereoFrame]) {
        for frame in frames {
            *frame = self.process_frame(*frame);
        }
    }

    pub fn reset(&mut self) {
        self.controls.reset();
        self.mode = self.controls.base().mode;
        self.reset_routing();
        self.target_gain = f64::from(self.controls.effective().gain);
        self.automated_target = None;
        self.band_targets_pending = self.band_automation_admitted;
        self.tail_controls_frozen = false;
        self.history = [[[[0.0; 2]; 4]; BAND_COUNT]; 2];
        self.audition_history = [[[0.0; 2]; 4]; 2];
        self.coefficients = self.routing.canonical_target(self.mode).map(|bank| *bank);
        self.current_gain = self.target_gain;
        self.smooth_remaining = 0;
        self.active_bands = active_band_flags(&self.coefficients);
        self.audition_selection = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.mode_fade = ModeFade::INACTIVE;
        self.install_enabled_bands(&self.controls.base().banks);
        self.rollback.open = false;
        self.rollback.committed_quiescent = true;
        self.rollback.dirty_flags = 0;
        self.rollback.dirty_history = 0;
    }

    pub fn inherit_history_from(&mut self, previous: &Self) -> bool {
        self.inherit_routing(previous);
        if self.sample_rate == previous.sample_rate && self.mode == previous.mode {
            self.inherit_enabled_bands(previous);
            if (self.band_automation_admitted || self.controls.admitted(automation::GAIN_SLOT))
                && (previous.band_automation_admitted
                    || previous.controls.admitted(automation::GAIN_SLOT))
            {
                self.controls.inherit(&previous.controls);
            }
            if self.controls.admitted(automation::GAIN_SLOT)
                && previous.controls.admitted(automation::GAIN_SLOT)
                && previous.controls.has_value(automation::GAIN_SLOT)
            {
                self.target_gain = previous.target_gain;
            }
            if self.band_automation_admitted {
                self.automated_target = Some(previous.current_target().map(|bank| *bank));
                self.band_targets_pending = true;
            }
        }
        if self.sample_rate != previous.sample_rate {
            self.reset();
            false
        } else if self.mode != previous.mode {
            // The lane interpretation changes (stereo/LR/MS), so previous
            // histories cannot be reused in the new routing. Start it empty
            // and crossfade out of the previous routing (see `ModeFade`).
            self.reset();
            // Lane histories are not inherited (false); only the fade is.
            self.mode_fade = ModeFade::from_outgoing(previous, self.sample_rate);
            false
        } else {
            // Copy the fading branch only while it is active; an inactive
            // branch's storage is never read.
            if previous.mode_fade.is_active() {
                self.mode_fade = previous.mode_fade;
            } else {
                self.mode_fade.remaining = 0;
            }
            self.current_gain = previous.current_gain;
            let same_target = previous.current_target() == self.current_target()
                && previous.target_gain == self.target_gain;
            self.smooth_remaining = if same_target {
                previous.smooth_remaining
            } else {
                coefficient_smooth_frames(self.sample_rate)
            };
            self.coefficients = previous.coefficients;
            self.history = previous.history;
            self.refresh_active_bands();
            if previous
                .audition_selection
                .is_none_or(|selection| self.selection_is_enabled(selection))
                && previous
                    .audition_request
                    .is_none_or(|selection| self.selection_is_enabled(selection))
            {
                self.audition_selection = previous.audition_selection;
                self.audition_request = previous.audition_request;
                self.audition_mix = previous.audition_mix;
                self.audition_history = previous.audition_history;
            } else {
                self.clear_audition();
            }
            true
        }
    }

    pub(in super::super) fn selection_is_enabled(&self, selection: Eq8AuditionSelection) -> bool {
        match self.mode {
            ChannelMode::Stereo => {
                self.enabled_bands[0][selection.band()] && self.enabled_bands[1][selection.band()]
            }
            ChannelMode::LeftRight | ChannelMode::MidSide => {
                self.enabled_bands[selection.edit_lane()][selection.band()]
            }
        }
    }

    pub(in super::super) fn advance_audition(&mut self) {
        let step = 1.0 / f64::from(eq8_audition_crossfade_frames(self.sample_rate));
        if self.audition_selection != self.audition_request {
            if self.audition_mix > 0.0 {
                self.audition_mix = hex_dsp::flush_subnormal((self.audition_mix - step).max(0.0));
                if self.audition_mix == 0.0 {
                    self.audition_selection = self.audition_request;
                    self.audition_history = [[[0.0; 2]; 4]; 2];
                }
                return;
            }
            self.audition_selection = self.audition_request;
            self.audition_history = [[[0.0; 2]; 4]; 2];
        }
        if self.audition_selection.is_some() {
            self.audition_mix = (self.audition_mix + step).min(1.0);
        } else {
            self.audition_mix = 0.0;
        }
    }

    pub(in super::super) fn clear_audition(&mut self) {
        self.audition_selection = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.audition_history = [[[0.0; 2]; 4]; 2];
    }

    pub(in super::super) fn advance_coefficients(&mut self) {
        if self.smooth_remaining == 0 {
            return;
        }
        let divisor = f64::from(self.smooth_remaining);
        let targets = self
            .routing
            .effective_target(self.automated_target.as_ref(), self.canonical_mode);
        for (current, target) in self.coefficients.iter_mut().zip(targets) {
            for (current, target) in current.iter_mut().flatten().zip(target.iter().flatten()) {
                current.step_toward(*target, divisor);
            }
        }
        self.smooth_remaining -= 1;
        if self.smooth_remaining == 0 {
            self.coefficients = targets.map(|bank| *bank);
            self.current_gain = self.target_gain;
            self.refresh_active_bands();
        } else {
            self.current_gain = hex_dsp::flush_subnormal(
                self.current_gain + (self.target_gain - self.current_gain) / divisor,
            );
        }
    }

    pub(in super::super) fn refresh_active_bands(&mut self) {
        let targets = self
            .routing
            .effective_target(self.automated_target.as_ref(), self.canonical_mode);
        for (channel, target) in targets.into_iter().enumerate() {
            for (band, target) in target.iter().enumerate() {
                self.active_bands[channel][band] = (0..4).any(|stage| {
                    self.coefficients[channel][band][stage] != Coefficients::IDENTITY
                        || target[stage] != Coefficients::IDENTITY
                        || self.history[channel][band][stage]
                            .iter()
                            .any(|value| *value != 0.0)
                });
            }
        }
    }

    pub fn is_quiescent(&self) -> bool {
        !self.mode_fade.is_active()
            && self.audition_selection.is_none()
            && self.audition_request.is_none()
            && self.audition_mix == 0.0
            && self
                .audition_history
                .iter()
                .flatten()
                .flatten()
                .all(|value| *value == 0.0)
            && self
                .history
                .iter()
                .flatten()
                .flatten()
                .flatten()
                .all(|value| *value == 0.0)
    }
}

pub(in super::super) fn process_band(
    mut input: f64,
    coefficients: &BandStages,
    history: &mut [[f64; 2]; 4],
) -> f64 {
    for stage in 0..4 {
        input = coefficients[stage].process_tdf2_with_floor(
            input,
            &mut history[stage],
            hex_dsp::POWER_FLOOR,
        );
    }
    input
}

pub(in super::super) fn active_band_flags(
    coefficients: &[[[Coefficients; 4]; BAND_COUNT]; 2],
) -> [[bool; BAND_COUNT]; 2] {
    let mut active = [[false; BAND_COUNT]; 2];
    for channel in 0..2 {
        for band in 0..BAND_COUNT {
            active[channel][band] =
                (0..4).any(|stage| coefficients[channel][band][stage] != Coefficients::IDENTITY);
        }
    }
    active
}

impl OwnedProcessorRuntime for Eq8Runtime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>() + std::mem::size_of::<Eq8Rollback>())
    }
    fn begin_block(&mut self) {
        if self.rollback.open {
            self.rollback_block();
        }
        self.rollback.committed_quiescent = self.is_quiescent();
        self.rollback.dirty_flags = 0;
        self.rollback.dirty_history = 0;
        self.rollback.open = true;
    }
    fn commit_block(&mut self) {
        self.rollback.open = false;
        self.rollback.dirty_flags = 0;
        self.rollback.dirty_history = 0;
    }
    fn rollback_block(&mut self) {
        if !self.rollback.open {
            return;
        }
        let flags = self.rollback.dirty_flags;
        let mut bits = self.rollback.dirty_history;
        while bits != 0 {
            let slot = bits.trailing_zeros() as usize;
            let channel = slot / BAND_COUNT;
            let band = slot % BAND_COUNT;
            self.history[channel][band] = self.rollback.history[channel][band];
            bits &= bits - 1;
        }
        if (flags & DIRTY_SCALARS) != 0 {
            self.mode = self.rollback.mode;
            self.current_gain = self.rollback.current_gain;
            self.target_gain = self.rollback.target_gain;
            self.smooth_remaining = self.rollback.smooth_remaining;
            self.active_bands = self.rollback.active_bands;
            self.enabled_bands = self.rollback.enabled_bands;
            self.band_targets_pending = self.rollback.band_targets_pending;
            self.tail_controls_frozen = self.rollback.tail_controls_frozen;
        }
        if (flags & DIRTY_COEFFS) != 0 {
            self.coefficients = self.rollback.coefficients;
        }
        if (flags & DIRTY_AUDITION) != 0 {
            self.audition_selection = self.rollback.audition_selection;
            self.audition_request = self.rollback.audition_request;
            self.audition_mix = self.rollback.audition_mix;
            self.audition_history = self.rollback.audition_history;
        }
        if (flags & DIRTY_MODE_FADE) != 0 {
            self.mode_fade = self.rollback.mode_fade;
        }
        if (flags & DIRTY_CONTROLS) != 0 {
            self.controls = self.rollback.controls;
        }
        if (flags & DIRTY_AUTOMATED_TARGET) != 0 {
            self.automated_target = self.rollback.automated_target;
        }
        if (flags & DIRTY_PENDING_ROUTING) != 0 {
            self.routing.install(self.rollback.pending_routing);
        }
        self.rollback.open = false;
        self.rollback.dirty_flags = 0;
        self.rollback.dirty_history = 0;
    }
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        if !frames.is_empty() && !self.tail_controls_frozen {
            self.restore_canonical_controls();
        }
        self.process(frames);
    }
    fn begin_output_tail(&mut self) {
        self.tail_controls_frozen = true;
        if self.rollback.open {
            self.rollback.tail_controls_frozen = true;
        }
    }
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        automation::slot(id).is_some_and(|i| self.controls.admitted(i))
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automation);
    fn reset(&mut self) {
        Eq8Runtime::reset(self);
    }
    fn committed_is_quiescent(&self) -> bool {
        if self.rollback.open {
            self.rollback.committed_quiescent
        } else {
            Eq8Runtime::is_quiescent(self)
        }
    }
    fn speculative_is_quiescent(&self) -> bool {
        Eq8Runtime::is_quiescent(self)
    }
    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        !self.rollback.open
            && previous
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|previous| {
                    !previous.rollback.open && self.sample_rate == previous.sample_rate
                })
    }
    /// A channel-mode change retargets through `ModeFade` instead of a reset,
    /// so session playback keeps one chain rather than detaching the old
    /// suffix as a ringing tail beside a fresh EQ. Within one mode, the
    /// canonical enabled-band set must match except where either runtime has
    /// admitted that exact enable control. Its fixed-stage smoother owns the
    /// transition, including lane removal and canonical fallback.
    fn can_retarget_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        !self.rollback.open
            && previous
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|previous| {
                    !previous.rollback.open
                        && self.sample_rate == previous.sample_rate
                        && if self.preserves_routing(previous) {
                            self.enabled_layout_compatible(previous, previous.mode)
                        } else {
                            self.mode != previous.mode
                                || self.enabled_layout_compatible(previous, self.mode)
                        }
                })
    }
    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            let _ = self.inherit_history_from(previous);
        }
    }
    /// Ordinary canonical mode changes wait for the occupied fade (Pulse #4106).
    /// Two admitted routing runtimes preserve that branch instead; their own
    /// latest validated target waits in fixed storage beside it.
    fn transition_busy_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| {
                self.sample_rate == previous.sample_rate
                    && !self.preserves_routing(previous)
                    && self.mode != previous.mode
                    && previous.mode_fade.is_active()
            })
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
    fn transient_audition(&mut self) -> Option<&mut dyn NativeProcessorAudition> {
        Some(self)
    }
    fn transient_audition_requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        NativeProcessorAudition::requested_selection(self)
    }
}

impl NativeProcessorAudition for Eq8Runtime {
    fn can_select(&self, selection: ProcessorAuditionSelection) -> bool {
        Eq8AuditionSelection::new(selection.index as usize, usize::from(selection.lane))
            .is_ok_and(|selection| self.selection_is_enabled(selection))
    }

    fn requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        self.audition_request
            .map(|selection| ProcessorAuditionSelection {
                index: u32::from(selection.band),
                lane: selection.edit_lane,
            })
    }

    fn select(&mut self, selection: Option<ProcessorAuditionSelection>) {
        // `can_select` preflights this exact stable mapping; update both live
        // and any open rollback snapshot atomically.
        let selection = selection.map(|selection| Eq8AuditionSelection {
            band: selection.index as u8,
            edit_lane: selection.lane,
        });
        self.audition_request = selection;
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_AUDITION) != 0 {
            self.rollback.audition_request = selection;
        }
    }

    fn cancel_immediately(&mut self) {
        self.clear_audition();
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_AUDITION) != 0 {
            self.rollback.audition_selection = None;
            self.rollback.audition_request = None;
            self.rollback.audition_mix = 0.0;
            self.rollback.audition_history = [[[0.0; 2]; 4]; 2];
        }
    }
}

pub(in super::super) fn quiet(value: f64) -> f64 {
    hex_dsp::flush_below(value, hex_dsp::POWER_FLOOR)
}

pub(in super::super) fn finite_f32(value: f64) -> f32 {
    if !value.is_finite() {
        0.0
    } else {
        value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
    }
}

pub(in super::super) fn response_magnitude(
    coefficients: Coefficients,
    frequency_hz: f64,
    sample_rate: f64,
) -> f64 {
    coefficients.magnitude_at_omega(std::f64::consts::TAU * frequency_hz / sample_rate)
}

pub(in super::super) fn design_band(
    band: Band,
    sample_rate: u32,
) -> Result<BandStages, DeviceContractError> {
    let rate = f64::from(sample_rate);
    let nyquist = rate * 0.5;
    let frequency = f64::from(band.frequency_hz).min(nyquist * 0.98);
    if frequency <= 0.0 || !frequency.is_finite() {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let q = f64::from(band.q);
    let gain_db = f64::from(band.gain_db);
    let mut stages = [Coefficients::IDENTITY; 4];
    let map_err = |_| DeviceContractError::InvalidParameterValue;
    match band.kind {
        0 => stages[0] = Coefficients::peaking(frequency, rate, q, gain_db).map_err(map_err)?,
        1 => {
            stages[0] =
                Coefficients::low_shelf_slope(frequency, rate, q, gain_db).map_err(map_err)?
        }
        2 => {
            stages[0] =
                Coefficients::high_shelf_slope(frequency, rate, q, gain_db).map_err(map_err)?
        }
        3 => stages[0] = Coefficients::high_pass(frequency, rate, q).map_err(map_err)?,
        4 => {
            let mut section_qs = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
            section_qs[3] *= q / f64::from(std::f32::consts::FRAC_1_SQRT_2);
            for (stage, section_q) in section_qs.into_iter().enumerate() {
                stages[stage] =
                    Coefficients::high_pass(frequency, rate, section_q).map_err(map_err)?;
            }
        }
        5 => stages[0] = Coefficients::low_pass(frequency, rate, q).map_err(map_err)?,
        6 => {
            let mut section_qs = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
            section_qs[3] *= q / f64::from(std::f32::consts::FRAC_1_SQRT_2);
            for (stage, section_q) in section_qs.into_iter().enumerate() {
                stages[stage] =
                    Coefficients::low_pass(frequency, rate, section_q).map_err(map_err)?;
            }
        }
        7 => stages[0] = Coefficients::notch(frequency, rate, q).map_err(map_err)?,
        _ => return Err(DeviceContractError::InvalidParameterValue),
    }
    Ok(stages)
}
