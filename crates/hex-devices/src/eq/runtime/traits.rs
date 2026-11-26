use super::*;

impl hex_lattice::OwnedProcessorRuntime for EqRuntime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        let (inline, [committed, scratch]) = self.control_storage()?;
        inline.checked_add(committed)?.checked_add(scratch)
    }
    fn begin_block(&mut self) {
        if self.rollback.open {
            self.rollback_block();
        }
        self.rollback.committed_quiescent = self.is_quiescent_live();
        self.rollback.dirty_bands = 0;
        self.rollback.dirty_flags = 0;
        self.rollback.open = true;
    }
    fn commit_block(&mut self) {
        self.rollback.open = false;
        self.rollback.dirty_bands = 0;
        self.rollback.dirty_flags = 0;
    }
    fn rollback_block(&mut self) {
        if !self.rollback.open {
            return;
        }
        let mut bits = self.rollback.dirty_bands;
        while bits != 0 {
            let index = bits.trailing_zeros() as usize;
            self.bands[index] = self.rollback.bands[index];
            bits &= bits - 1;
        }
        let flags = self.rollback.dirty_flags;
        if (flags & EQ_DIRTY_SCALARS) != 0 {
            self.last_frame = self.rollback.last_frame;
            self.fallback_frame = self.rollback.fallback_frame;
            self.refresh_target = self.rollback.refresh_target;
            self.gain = self.rollback.gain;
            self.mode = self.rollback.mode;
            self.effective_enabled = self.rollback.effective_enabled;
            self.effective_banks = self.rollback.effective_banks;
            self.pristine = self.rollback.pristine;
        }
        if (flags & EQ_DIRTY_AUDITION) != 0 {
            self.audition = self.rollback.audition;
            self.audition_request = self.rollback.audition_request;
            self.audition_mix = self.rollback.audition_mix;
            self.audition_history = self.rollback.audition_history;
        }
        if (flags & EQ_DIRTY_MODE_FADE) != 0 {
            self.mode_fade.copy_from(&self.rollback.mode_fade);
        }
        self.rollback.open = false;
        self.rollback.dirty_bands = 0;
        self.rollback.dirty_flags = 0;
    }
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.configuration
            .parameters
            .binary_search_by_key(&id, |grant| grant.descriptor.id)
            .is_ok()
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, clock, values, false)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, clock, values, true)
    }
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, None, false)
    }
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, clock, false)
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, clock, true)
    }
    fn reset(&mut self) {
        // Reset starts from the authored response, even when the previous
        // block left an automated target and the first new block has no clock.
        for ((band, &stages), node) in self
            .bands
            .iter_mut()
            .zip(&self.configuration.authored_stages)
            .zip(&self.configuration.nodes)
        {
            *band = RuntimeBand::new(stages, node.kind);
        }
        self.last_frame = None;
        self.fallback_frame = 0;
        self.refresh_target = false;
        self.mode = self.configuration.mode;
        self.gain = self.configuration.gain;
        self.effective_banks = std::array::from_fn(|index| {
            self.configuration
                .nodes
                .get(index)
                .map_or(0, |node| node.bank)
        });
        self.effective_enabled = std::array::from_fn(|index| {
            self.configuration
                .nodes
                .get(index)
                .is_some_and(|node| node.enabled)
        });
        self.pristine = true;
        self.audition = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.audition_history = [[[0.0; 2]; 4]; 2];
        self.mode_fade.clear();
        self.rollback.open = false;
        self.rollback.committed_quiescent = true;
        self.rollback.dirty_bands = 0;
        self.rollback.dirty_flags = 0;
    }
    fn committed_is_quiescent(&self) -> bool {
        if self.rollback.open {
            self.rollback.committed_quiescent
        } else {
            self.is_quiescent_live()
        }
    }
    fn speculative_is_quiescent(&self) -> bool {
        self.is_quiescent_live()
    }
    /// A channel-mode change still inherits and retargets without a reset:
    /// the previous routing crossfades out (see `ModeFade`). Retargeting keeps
    /// session playback from detaching the old suffix as a ringing tail beside
    /// a fresh chain.
    fn can_inherit_from(&self, previous: &dyn hex_lattice::OwnedProcessorRuntime) -> bool {
        !self.rollback.open
            && previous
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|old| !old.rollback.open && old.sample_rate == self.sample_rate)
    }
    fn can_retarget_parameters_from(
        &self,
        previous: &dyn hex_lattice::OwnedProcessorRuntime,
    ) -> bool {
        self.can_inherit_from(previous)
    }
    fn inherit_from(&mut self, previous: &dyn hex_lattice::OwnedProcessorRuntime) {
        let Some(old) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|old| !old.rollback.open && old.sample_rate == self.sample_rate)
        else {
            return;
        };
        if !self.mode_is_automated() && old.mode != self.configuration.mode {
            // The new routing starts from empty history; the old one fades out.
            self.reset();
            self.mode_fade.capture(Routing::from(old), self.sample_rate);
            self.last_frame = old.last_frame;
            self.fallback_frame = old.fallback_frame;
            self.refresh_target = true;
            self.gain = old.gain;
            self.pristine = old.pristine;
            return;
        }
        self.mode = old.mode;
        self.mode_fade.copy_from(&old.mode_fade);
        if Arc::ptr_eq(&self.configuration, &old.configuration) {
            self.bands.copy_from_slice(&old.bands);
            self.effective_banks = old.effective_banks;
            self.effective_enabled = old.effective_enabled;
            self.last_frame = old.last_frame;
            self.fallback_frame = old.fallback_frame;
            self.refresh_target = old.refresh_target;
            self.audition = old.audition;
            self.audition_request = old.audition_request;
            self.audition_mix = old.audition_mix;
            self.audition_history = old.audition_history;
            self.gain = old.gain;
            self.pristine = old.pristine;
            return;
        }
        // Stable identity owns effective DSP history. Canonical kind/bank can
        // differ behind an authored value; the first refresh compares effective
        // shape/bank and owns any genuine discontinuity exactly once.
        for (index, node) in self.configuration.nodes.iter().enumerate() {
            if let Some(prior) = old.configuration.nodes.iter().position(|n| n.id == node.id) {
                self.bands[index] = old.bands[prior];
                self.effective_banks[index] = old.effective_banks[prior];
                self.effective_enabled[index] = old.effective_enabled[prior];
            }
        }
        if old
            .audition
            .is_none_or(|selection| self.can_select(selection))
            && old
                .audition_request
                .is_none_or(|selection| self.can_select(selection))
        {
            self.audition = old.audition;
            self.audition_request = old.audition_request;
            self.audition_mix = old.audition_mix;
            self.audition_history = old.audition_history;
        }
        self.last_frame = old.last_frame;
        self.fallback_frame = old.fallback_frame;
        self.refresh_target = true;
        self.gain = old.gain;
        self.pristine = old.pristine;
    }
    /// The single `ModeFade` branch is full: a further channel-mode change
    /// waits until the previous switch has finished fading (Pulse #4122).
    fn transition_busy_from(&self, previous: &dyn hex_lattice::OwnedProcessorRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            old.sample_rate == self.sample_rate
                && !self.mode_is_automated()
                && old.mode != self.configuration.mode
                && old.mode_fade.is_active()
        })
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
    fn transient_audition(&mut self) -> Option<&mut dyn NativeProcessorAudition> {
        Some(self)
    }
    fn transient_audition_requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        self.audition_request
    }
}
impl NativeProcessorAudition for EqRuntime {
    fn can_select(&self, s: ProcessorAuditionSelection) -> bool {
        s.lane < 2
            && self
                .configuration
                .nodes
                .iter()
                .enumerate()
                .any(|(index, node)| {
                    node.id.value() == s.index
                        && self.effective_enabled[index]
                        && self.effective_banks[index] == bank(self.mode, usize::from(s.lane))
                })
    }
    fn requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        self.audition_request
    }
    fn select(&mut self, s: Option<ProcessorAuditionSelection>) {
        self.audition_request = s;
        if self.rollback.open && (self.rollback.dirty_flags & EQ_DIRTY_AUDITION) != 0 {
            self.rollback.audition_request = s;
        }
    }
    fn cancel_immediately(&mut self) {
        self.audition = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.audition_history = [[[0.0; 2]; 4]; 2];
        if self.rollback.open && (self.rollback.dirty_flags & EQ_DIRTY_AUDITION) != 0 {
            self.rollback.audition = None;
            self.rollback.audition_request = None;
            self.rollback.audition_mix = 0.0;
            self.rollback.audition_history = [[[0.0; 2]; 4]; 2];
        }
    }
}

/// Infallible after finite/range control validation. Every new shelf uses the
/// ordinary Q alpha law. Imported S shelves are preflighted and cannot automate
/// gain/Q/shape before explicitly converting to the new law.
pub(super) fn design(node: Node, sample_rate: u32) -> BandStages {
    let rate = f64::from(sample_rate);
    let frequency = f64::from(node.frequency_hz).min(rate * 0.49);
    let q = f64::from(node.q);
    let gain = f64::from(node.gain_db);
    let mut result = [Coefficients::IDENTITY; 4];
    let invalid = Coefficients {
        b0: f64::NAN,
        b1: f64::NAN,
        b2: f64::NAN,
        a1: f64::NAN,
        a2: f64::NAN,
    };
    match node.kind {
        NodeKind::Bell => {
            result[0] = Coefficients::peaking(frequency, rate, q, gain).unwrap_or(invalid);
        }
        NodeKind::Notch => {
            result[0] = Coefficients::notch(frequency, rate, q).unwrap_or(invalid);
        }
        NodeKind::LowCut | NodeKind::HighCut => {
            let count = usize::from(node.slope) + 1;
            for (index, stage) in result.iter_mut().take(count).enumerate() {
                let section_q = if count == 1 {
                    q
                } else if count == 4 {
                    let values = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
                    values[index]
                        * if index + 1 == count {
                            q / f64::from(std::f32::consts::FRAC_1_SQRT_2)
                        } else {
                            1.0
                        }
                } else {
                    let butterworth =
                        Coefficients::butterworth_section_q(count, index).unwrap_or(f64::NAN);
                    butterworth
                        * if index + 1 == count {
                            q / f64::from(std::f32::consts::FRAC_1_SQRT_2)
                        } else {
                            1.0
                        }
                };
                *stage = if node.kind == NodeKind::LowCut {
                    Coefficients::high_pass(frequency, rate, section_q)
                } else {
                    Coefficients::low_pass(frequency, rate, section_q)
                }
                .unwrap_or(invalid);
            }
        }
        NodeKind::LowShelf => {
            result[0] = if node.legacy_shelf {
                Coefficients::low_shelf_slope(frequency, rate, q, gain)
            } else {
                Coefficients::low_shelf_q(frequency, rate, q, gain)
            }
            .unwrap_or(invalid);
        }
        NodeKind::HighShelf => {
            result[0] = if node.legacy_shelf {
                Coefficients::high_shelf_slope(frequency, rate, q, gain)
            } else {
                Coefficients::high_shelf_q(frequency, rate, q, gain)
            }
            .unwrap_or(invalid);
        }
    }
    result
}
