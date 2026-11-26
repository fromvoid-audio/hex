use super::*;

impl Runtime {
    /// Borrow the exact inline owner and its committed/scratch heap storage.
    /// This control-side observation transfers no runtime or buffer ownership.
    pub fn control_storage(&self) -> Option<(usize, [usize; 2])> {
        Some((
            std::mem::size_of::<Self>(),
            [self.primary.bytes(), self.scratch.bytes()],
        ))
    }
    pub(super) fn same_history_layout(&self, previous: &Self) -> bool {
        self.config.delay == previous.config.delay
            && self.config.mid_side == previous.config.mid_side
            && self.config.true_peak == previous.config.true_peak
            && self.primary.same_storage_layout(&previous.primary)
            && self.scratch.same_storage_layout(&previous.scratch)
    }
}
pub(super) struct BlockStats {
    pub(super) post_input_peak: f64,
    pub(super) min_attenuation: [f64; 2],
}
impl BlockStats {
    pub(super) fn new() -> Self {
        Self {
            post_input_peak: 0.0,
            min_attenuation: [1.0; 2],
        }
    }
    pub(super) fn finish(self, history: &mut History) {
        self.finish_meter(&mut history.meter);
    }
    pub(super) fn finish_meter(self, meter: &mut DynamicsMeter) {
        meter.post_input_peak = meter.post_input_peak.max(self.post_input_peak as f32);
        for (lane, attenuation) in self.min_attenuation.into_iter().enumerate() {
            meter.reduction_db[lane] = meter.reduction_db[lane].max(reduction(attenuation));
        }
    }
}
impl Runtime {
    pub(super) fn restore_controls(&mut self) {
        let base = self.scratch.controls.base();
        let changed = base != *self.scratch.controls.effective_ref();
        self.scratch.controls.reset();
        if changed {
            self.scratch.parameters = Parameters::new(base, self.config.rate);
        }
    }
    pub(super) fn process(&mut self, frames: &mut [StereoFrame], bypassed: bool) {
        if !self.open {
            return;
        }
        if !self.releasing && !frames.is_empty() {
            self.restore_controls();
        }
        if !frames.is_empty()
            && self.scratch.gain == self.scratch.parameters.gain
            && self.scratch.output == self.scratch.parameters.output
            && self.scratch.first.is_idle_unity(&self.config)
            && self
                .scratch
                .final_stage
                .as_ref()
                .is_none_or(|stage| stage.is_idle_unity(&self.config))
            && frames
                .iter()
                .all(|f| (f[0].to_bits() | f[1].to_bits()) == 0)
        {
            let window = self.config.delay + 1;
            self.scratch.first.advance_silent(frames.len(), window);
            if let Some(final_stage) = &mut self.scratch.final_stage {
                final_stage.advance_silent(frames.len(), window);
            }
            return;
        }
        let mut stats = BlockStats::new();
        for frame in frames {
            let dry = *frame;
            Self::process_frame(&self.config, &mut self.scratch, frame, &mut stats);
            if bypassed {
                *frame = dry;
            }
        }
        stats.finish(&mut self.scratch);
    }
    pub(super) fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        if !self.open {
            self.begin_block();
        }
        let mut map = self
            .scratch
            .controls
            .resolve(&automation::BINDINGS, values)?;
        let mut stats = BlockStats::new();
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) =
                self.scratch
                    .controls
                    .sample(&automation::BINDINGS, &mut map, values, offset)?
            {
                if next.effective_ref() != self.scratch.controls.effective_ref() {
                    self.scratch.parameters =
                        Parameters::new(*next.effective_ref(), self.config.rate);
                }
                self.scratch.controls.install(next);
            }
            let dry = *frame;
            Self::process_frame(&self.config, &mut self.scratch, frame, &mut stats);
            if bypassed {
                *frame = dry;
            }
        }
        stats.finish(&mut self.scratch);
        Ok(())
    }
    // Keep the canonical block loop optimized while sharing exact frame laws.
    #[inline(always)]
    pub(super) fn process_frame(
        config: &Config,
        history: &mut History,
        frame: &mut StereoFrame,
        stats: &mut BlockStats,
    ) {
        let signal = Self::prepare_input(config, history, *frame, stats);
        Self::process_signal(config, history, signal, frame, stats);
    }
    /// Advance gain ramps once per device input frame and preserve the gained
    /// signal in f64. A buffered consumer may queue it before limiting while
    /// keeping the current ceiling/output controls at the output stage.
    #[inline(always)]
    pub(super) fn prepare_input(
        config: &Config,
        history: &mut History,
        frame: StereoFrame,
        stats: &mut BlockStats,
    ) -> [f64; 2] {
        let parameters = &history.parameters;
        // Compatible edits ramp input/output gain over 5 ms. A falling ceiling
        // is immediate, preserving its safety invariant on the first sample.
        history.gain += (parameters.gain - history.gain) * config.gain_ramp_inv;
        history.output += (parameters.output - history.output) * config.gain_ramp_inv;
        // Output gain is never above the authored target after a downward edit.
        history.output = history.output.min(parameters.output);
        let signal = frame.map(|v| {
            if v.is_finite() {
                f64::from(v) * history.gain
            } else {
                0.0
            }
        });
        stats.post_input_peak = stats
            .post_input_peak
            .max(signal[0].abs())
            .max(signal[1].abs());
        signal
    }
    /// Limit already-gained input without advancing input/output gain ramps.
    /// The caller owns ramp timing and supplies the current output gain through
    /// history; this function preserves the existing limiting and meter laws.
    #[inline(always)]
    pub(super) fn process_signal(
        config: &Config,
        history: &mut History,
        mut signal: [f64; 2],
        frame: &mut StereoFrame,
        stats: &mut BlockStats,
    ) {
        let parameters = &history.parameters;
        if config.mid_side {
            signal = [(signal[0] + signal[1]) * 0.5, (signal[0] - signal[1]) * 0.5];
        }
        let (mut sample, first_gain) = history.first.process(
            Sample {
                signal,
                ..Sample::default()
            },
            config,
            parameters,
            parameters.link,
            false,
        );
        let mut attenuation = first_gain.map(meter_factor);
        if config.mid_side
            && let Some(final_stage) = &mut history.final_stage
        {
            // The final stage delays this exact first-stage factor together
            // with its signal; its own factor is applied only on output.
            sample.attenuation = attenuation.map(|gain| gain as f32);
            sample.signal = [
                sample.signal[0] + sample.signal[1],
                sample.signal[0] - sample.signal[1],
            ];
            let (delayed, final_gain) = final_stage.process(sample, config, parameters, 1.0, true);
            sample = delayed;
            attenuation = std::array::from_fn(|lane| {
                (if sample.attenuation[lane] == 0.0 {
                    1.0
                } else {
                    f64::from(sample.attenuation[lane])
                }) * meter_factor(final_gain[lane])
            });
        }
        if config.soft_clip {
            let ratios = sample.signal.map(|v| {
                if v == 0.0 {
                    1.0
                } else {
                    let normalized = v.abs() / parameters.output_ceiling;
                    (normalized * normalized).tanh().sqrt() / normalized
                }
            });
            // A shared output shaper in m/s preserves encoded lane balance;
            // its common attenuation can be reported truthfully in both lanes.
            let ratios = if config.mid_side {
                [ratios[0].min(ratios[1]); 2]
            } else {
                ratios
            };
            history.meter.clipped |= sample
                .signal
                .iter()
                .any(|v| v.abs() > parameters.output_ceiling);
            for (lane, ratio) in ratios.into_iter().enumerate() {
                sample.signal[lane] *= ratio;
                attenuation[lane] *= meter_factor(ratio);
            }
        }
        *frame = sample.signal.map(|v| (v * history.output) as f32);
        for (lane, attenuation) in attenuation.into_iter().enumerate() {
            stats.min_attenuation[lane] = stats.min_attenuation[lane].min(attenuation);
        }
    }
}
impl OwnedProcessorRuntime for Runtime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        let (inline, [committed, scratch]) = self.control_storage()?;
        inline.checked_add(committed)?.checked_add(scratch)
    }
    fn begin_block(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.scratch.begin_block();
        self.open = true;
    }
    fn supports_parameter_automation(&self, id: hex_lattice_contract::ParameterId) -> bool {
        self.primary.controls.supports(&automation::BINDINGS, id)
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        self.process(frames, false);
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automation);
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, true);
    }
    fn commit_block(&mut self) {
        if self.open {
            self.primary
                .commit_from(&mut self.scratch, self.config.delay + 1);
            self.open = false;
        }
    }
    fn rollback_block(&mut self) {
        if self.open {
            self.scratch
                .rollback_from(&self.primary, self.config.delay + 1);
            self.open = false;
        }
    }
    fn begin_output_tail(&mut self) {
        self.releasing = true;
    }
    fn reset(&mut self) {
        self.releasing = false;
        self.primary.reset(self.config);
        self.scratch.reset(self.config);
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.primary.quiet()
    }
    fn speculative_is_quiescent(&self) -> bool {
        if self.open {
            self.scratch.quiet()
        } else {
            self.primary.quiet()
        }
    }
    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            !self.open
                && !old.open
                && self.config.rate == old.config.rate
                && (self.same_history_layout(old) || old.primary.quiet())
        })
    }
    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(previous) {
            return;
        }
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.releasing = false;
            if self.same_history_layout(old) {
                let mut controls = self.primary.controls;
                controls.inherit(&old.primary.controls);
                self.primary.copy_from(&old.primary);
                self.primary.controls = controls;
                self.scratch.copy_from(&self.primary);
                if self.config.soft_clip != old.config.soft_clip {
                    self.primary.clear_attack();
                    self.scratch.clear_attack();
                }
            } else {
                // Admission proved that no donor audio remains queued. A new
                // layout starts with its own configuration, including both
                // histories even when this candidate was previously rendered.
                self.reset();
            }
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(previous)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
        let result = self.primary.meter;
        self.primary.meter = DynamicsMeter::default();
        Some(result)
    }
}
