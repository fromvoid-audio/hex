use super::*;

impl EqRuntime {
    /// Control-side storage observation: inline runtime bytes plus committed
    /// and preallocated rollback heap slices.
    pub fn control_storage(&self) -> Option<(usize, [usize; 2])> {
        let committed = std::mem::size_of_val(self.bands.as_ref())
            .checked_add(std::mem::size_of_val(self.mode_fade.bands.as_ref()))?;
        let scratch = std::mem::size_of_val(self.rollback.bands.as_ref()).checked_add(
            std::mem::size_of_val(self.rollback.mode_fade.bands.as_ref()),
        )?;
        Some((EQ_RUNTIME_BYTES, [committed, scratch]))
    }

    pub(super) fn snapshot_scalars(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & EQ_DIRTY_SCALARS) == 0 {
            self.rollback.last_frame = self.last_frame;
            self.rollback.fallback_frame = self.fallback_frame;
            self.rollback.refresh_target = self.refresh_target;
            self.rollback.gain = self.gain;
            self.rollback.mode = self.mode;
            self.rollback.effective_enabled = self.effective_enabled;
            self.rollback.effective_banks = self.effective_banks;
            self.rollback.pristine = self.pristine;
            self.rollback.dirty_flags |= EQ_DIRTY_SCALARS;
        }
    }

    pub(super) fn snapshot_band(&mut self, index: usize) {
        let bit = 1_u64 << index;
        if self.rollback.open && (self.rollback.dirty_bands & bit) == 0 {
            self.rollback.bands[index] = self.bands[index];
            self.rollback.dirty_bands |= bit;
        }
    }

    pub(super) fn snapshot_all_bands(&mut self) {
        if !self.rollback.open {
            return;
        }
        let count = self.bands.len();
        for index in 0..count {
            self.snapshot_band(index);
        }
    }

    pub(super) fn snapshot_audition(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & EQ_DIRTY_AUDITION) == 0 {
            self.rollback.audition = self.audition;
            self.rollback.audition_request = self.audition_request;
            self.rollback.audition_mix = self.audition_mix;
            self.rollback.audition_history = self.audition_history;
            self.rollback.dirty_flags |= EQ_DIRTY_AUDITION;
        }
    }

    pub(super) fn snapshot_mode_fade(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & EQ_DIRTY_MODE_FADE) == 0 {
            self.rollback.mode_fade.copy_from(&self.mode_fade);
            self.rollback.dirty_flags |= EQ_DIRTY_MODE_FADE;
        }
    }

    pub(super) fn is_quiescent_live(&self) -> bool {
        !self.mode_fade.is_active()
            && self.audition.is_none()
            && self.audition_request.is_none()
            && self.audition_mix == 0.0
            && self
                .bands
                .iter()
                .all(|b| b.history.iter().flatten().flatten().all(|v| *v == 0.0))
            && self
                .audition_history
                .iter()
                .flatten()
                .flatten()
                .all(|v| *v == 0.0)
    }

    pub(super) fn mode_is_automated(&self) -> bool {
        self.configuration
            .parameters
            .first()
            .is_some_and(|grant| grant.descriptor.id == MODE)
    }
    pub(super) fn select_routing(&mut self, target: u8, discontinuity: bool) {
        if discontinuity || (self.pristine && self.mode_is_automated()) {
            if self.mode != target {
                self.snapshot_scalars();
                self.snapshot_audition();
                self.audition = None;
                self.audition_request = None;
                self.audition_mix = 0.0;
                self.audition_history = [[[0.0; 2]; 4]; 2];
                self.refresh_target = true;
            }
            if self.mode != target {
                self.snapshot_scalars();
                self.mode = target;
            }
            if self.pristine && self.mode_is_automated() && self.mode_fade.is_active() {
                self.snapshot_mode_fade();
                self.mode_fade.clear();
            }
            return;
        }
        // One frozen outgoing branch. New readings coalesce while occupied;
        // the first frame after its fade samples the latest current target.
        if self.mode == target || self.mode_fade.is_active() {
            return;
        }
        self.snapshot_mode_fade();
        self.snapshot_scalars();
        self.snapshot_all_bands();
        self.snapshot_audition();
        self.mode_fade.capture(
            Routing {
                mode: self.mode,
                nodes: &self.configuration.nodes,
                bands: &self.bands,
                banks: &self.effective_banks,
                audition: self.audition,
                audition_mix: self.audition_mix,
                audition_history: &self.audition_history,
                gain: self.gain,
            },
            self.sample_rate,
        );
        self.mode = target;
        for band in &mut self.bands {
            band.history = [[[0.0; 2]; 4]; 2];
        }
        self.audition = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.audition_history = [[[0.0; 2]; 4]; 2];
        // Routing is independent of node coefficients. Keep their existing
        // target cadence and in-flight trajectories through a mode switch.
    }
    pub(super) fn process(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        bypassed: bool,
    ) {
        self.process_at(
            frames,
            clock,
            bypassed,
            0,
            automation::Targets {
                gain: self.configuration.gain,
                mode: self.configuration.mode,
                mapped_gain: false,
            },
            None,
        );
    }
    // Callers pass either a whole block at offset zero or one frame at its
    // original block offset; clock and legacy lane positions stay unchanged.
    pub(super) fn process_at(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        bypassed: bool,
        first_offset: usize,
        targets: automation::Targets,
        node_overrides: Option<&automation::Nodes>,
    ) {
        let automated = !self.configuration.lanes.is_empty() || node_overrides.is_some();
        if !automated
            && !bypassed
            && !frames.is_empty()
            && frames.len() <= 256
            && !self.refresh_target
            && !self.pristine
            && targets.mode == self.mode
            && !self.mode_fade.is_active()
            && self.audition.is_none()
            && self.audition_request.is_none()
            && self.try_process_steady_block(frames, clock, first_offset, targets)
        {
            return;
        }
        if !frames.is_empty() {
            self.snapshot_scalars();
        }
        for (offset, frame) in frames.iter_mut().enumerate() {
            let offset = first_offset + offset;
            let running = clock.is_some_and(|c| c.transport_is_running() == Some(true));
            // Stopped monitoring can repeat its Arrangement origin. While
            // running, Arrangement wraps refresh authored targets but preserve
            // DSP history on the continuous performance clock. Legacy clocks
            // without performance origins keep Arrangement discontinuities.
            let origin = if running {
                clock.and_then(|c| c.arrangement_frame_at_offset(offset))
            } else {
                None
            };
            let position = origin.unwrap_or(self.fallback_frame);
            let current_frame = origin.map(|arrangement| {
                let continuity = clock
                    .and_then(|clock| clock.performance_frame_at_offset(offset))
                    .unwrap_or(arrangement);
                (arrangement, continuity)
            });
            let arrangement_changed = origin.is_some_and(|position| {
                self.last_frame
                    .is_none_or(|(last, _)| last.checked_add(1) != Some(position))
            });
            let discontinuity = current_frame.is_some_and(|(_, position)| {
                self.last_frame
                    .is_none_or(|(_, last)| last.checked_add(1) != Some(position))
            });
            self.select_routing(targets.mode, discontinuity);
            if discontinuity {
                self.snapshot_all_bands();
                for band in &mut self.bands {
                    band.history = [[[0.0; 2]; 4]; 2];
                }
                self.snapshot_audition();
                self.audition_history = [[[0.0; 2]; 4]; 2];
                if self.mode_fade.is_active() {
                    self.snapshot_mode_fade();
                    self.mode_fade.clear();
                }
            }
            let refresh_all = self.refresh_target || discontinuity;
            if refresh_all
                || arrangement_changed
                || (automated && position % self.configuration.cadence == 0)
            {
                self.refresh_target = false;
                let beat = if origin.is_some() && automated {
                    clock.and_then(|clock| clock.arrangement_beat_at_offset(offset))
                } else {
                    None
                };
                let mut dynamic_mask = self.configuration.automated_nodes;
                if node_overrides.is_some() {
                    dynamic_mask |= self.configuration.granted_nodes;
                }
                for index in 0..self.configuration.nodes.len() {
                    let is_dynamic = (dynamic_mask & (1_u64 << index)) != 0;
                    if !refresh_all && !is_dynamic {
                        continue;
                    }
                    let mut node = self.configuration.node_at(index, beat);
                    if let Some(overrides) = node_overrides {
                        overrides.apply(index, &mut node);
                    }
                    let bank_changed = self.effective_banks[index] != node.bank;
                    self.effective_banks[index] = node.bank;
                    self.effective_enabled[index] = node.enabled;
                    let target = if !is_dynamic {
                        self.configuration.authored_stages[index]
                    } else if node.enabled {
                        design(node, self.sample_rate)
                    } else {
                        [Coefficients::IDENTITY; 4]
                    };
                    self.snapshot_band(index);
                    let band = &mut self.bands[index];
                    if bank_changed || band.shape != node.kind as u8 {
                        band.history = [[[0.0; 2]; 4]; 2];
                        band.shape = node.kind as u8;
                    }
                    if discontinuity || (self.pristine && node_overrides.is_some()) {
                        band.target = target;
                        band.current = target;
                        band.remaining = 0;
                        band.stages_used = stages_used(&target);
                    } else if band.target != target {
                        band.target = target;
                        band.stages_used = band.stages_used.max(stages_used(&target));
                        band.remaining = if band.current == target {
                            0
                        } else {
                            coefficient_smooth_frames(self.sample_rate)
                        };
                    }
                }
            }
            let input = *frame;
            let mut working = [f64::from(frame[0]), f64::from(frame[1])];
            if self.mode == 2 {
                working = [
                    (working[0] + working[1]) * 0.5,
                    (working[0] - working[1]) * 0.5,
                ];
            }
            let audition_input = working;
            let node_count = self.configuration.nodes.len();
            let bank_l = bank(self.mode, 0);
            let bank_r = bank(self.mode, 1);
            for index in 0..node_count {
                let run_l = self.effective_banks[index] == bank_l;
                let run_r = self.effective_banks[index] == bank_r;
                if self.bands[index].remaining == 0 && !run_l && !run_r {
                    continue;
                }
                self.snapshot_band(index);
                let band = &mut self.bands[index];
                if band.remaining > 0 {
                    let t = 1.0 / f64::from(band.remaining);
                    for (current, target) in band.current.iter_mut().zip(&band.target) {
                        current.b0 += (target.b0 - current.b0) * t;
                        current.b1 += (target.b1 - current.b1) * t;
                        current.b2 += (target.b2 - current.b2) * t;
                        current.a1 += (target.a1 - current.a1) * t;
                        current.a2 += (target.a2 - current.a2) * t;
                    }
                    band.remaining -= 1;
                    if band.remaining == 0 {
                        band.current = band.target;
                    }
                }
                if run_l {
                    working[0] = band.filter_channel(0, working[0]);
                }
                if run_r {
                    working[1] = band.filter_channel(1, working[1]);
                }
            }
            if self.audition != self.audition_request || self.audition.is_some() {
                self.snapshot_audition();
            }
            if self.audition != self.audition_request {
                self.audition_mix = hex_dsp::flush_subnormal(
                    (self.audition_mix - self.configuration.audition_step).max(0.0),
                );
                if self.audition_mix == 0.0 {
                    self.audition = self.audition_request;
                    self.audition_history = [[[0.0; 2]; 4]; 2];
                }
            } else if self.audition.is_some() {
                self.audition_mix = (self.audition_mix + self.configuration.audition_step).min(1.0);
            }
            if let Some(index) = self.audition.and_then(|selection| {
                self.configuration
                    .nodes
                    .iter()
                    .position(|node| node.id.value() == selection.index)
            }) {
                for channel in 0..2 {
                    let isolated = if self.effective_banks[index] == bank(self.mode, channel) {
                        filter(
                            audition_input[channel],
                            &self.bands[index].current,
                            &mut self.audition_history[channel],
                        )
                    } else {
                        0.0
                    };
                    working[channel] += (isolated - working[channel]) * self.audition_mix;
                }
            }
            if targets.mapped_gain {
                self.gain = targets.gain;
            } else {
                let delta = targets.gain - self.gain;
                if delta.abs() <= hex_dsp::QUIET_FLOOR {
                    self.gain = targets.gain;
                } else {
                    self.gain =
                        hex_dsp::flush_subnormal(self.gain + delta * self.configuration.gain_alpha);
                }
            }
            if self.mode == 2 {
                working = [working[0] + working[1], working[0] - working[1]];
            }
            if self.mode_fade.is_active() {
                self.snapshot_mode_fade();
            }
            let output = self.mode_fade.blend(
                input,
                [working[0] * self.gain, working[1] * self.gain],
                targets.mapped_gain.then_some(self.gain),
            );
            *frame = if bypassed {
                input
            } else {
                [finite_f32(output[0]), finite_f32(output[1])]
            };
            self.last_frame = current_frame;
            self.fallback_frame = self.fallback_frame.wrapping_add(1);
            self.pristine = false;
        }
    }

    pub(super) fn try_process_steady_block(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        first_offset: usize,
        targets: automation::Targets,
    ) -> bool {
        let len = frames.len();
        let next_last_frame = if let Some(clock) = clock {
            if clock.transport_is_running() == Some(true) {
                let Some((arr0, perf0)) = clock.contiguous_running_frames(first_offset, len) else {
                    return false;
                };
                let Some((last_arr, last_perf)) = self.last_frame else {
                    return false;
                };
                if last_arr.checked_add(1) != Some(arr0) || last_perf.checked_add(1) != Some(perf0)
                {
                    return false;
                }
                Some((arr0 + (len - 1) as u64, perf0 + (len - 1) as u64))
            } else {
                None
            }
        } else {
            None
        };
        let gain_steady =
            targets.mapped_gain || (self.gain + (targets.gain - self.gain) * 0.015625) == self.gain;
        if !gain_steady {
            return false;
        }
        let node_count = self.configuration.nodes.len();
        if self.bands[..node_count]
            .iter()
            .any(|band| band.remaining > 0)
        {
            return false;
        }
        let mut all_zero = true;
        for frame in frames.iter() {
            if !frame[0].is_finite()
                || !frame[1].is_finite()
                || frame[0].abs() >= 1e10
                || frame[1].abs() >= 1e10
            {
                return false;
            }
            if (frame[0].to_bits() | frame[1].to_bits()) != 0 {
                all_zero = false;
            }
        }
        self.snapshot_scalars();
        if targets.mapped_gain {
            self.gain = targets.gain;
        }
        if all_zero {
            let history_quiet = self.bands[..node_count].iter().all(|band| {
                let used = usize::from(band.stages_used);
                band.history[0][..used]
                    .iter()
                    .chain(&band.history[1][..used])
                    .all(|h| h[0] == 0.0 && h[1] == 0.0)
            });
            if history_quiet {
                self.last_frame = next_last_frame;
                self.fallback_frame = self.fallback_frame.wrapping_add(len as u64);
                return true;
            }
        }

        let mut working = [[0.0_f64; 2]; 256];
        let working = &mut working[..len];
        if self.mode == 2 {
            for (dst, src) in working.iter_mut().zip(frames.iter()) {
                let l = f64::from(src[0]);
                let r = f64::from(src[1]);
                *dst = [(l + r) * 0.5, (l - r) * 0.5];
            }
        } else {
            for (dst, src) in working.iter_mut().zip(frames.iter()) {
                *dst = [f64::from(src[0]), f64::from(src[1])];
            }
        }

        let bank_l = bank(self.mode, 0);
        let bank_r = bank(self.mode, 1);
        let mut quieted = [false; 2];
        for index in 0..node_count {
            let target_bank = self.effective_banks[index];
            let run_l = target_bank == bank_l;
            let run_r = target_bank == bank_r;
            if !run_l && !run_r {
                continue;
            }
            let used = usize::from(self.bands[index].stages_used);
            if used == 1 && self.bands[index].current[0] == Coefficients::IDENTITY {
                let mut touched = false;
                if run_l && self.bands[index].history[0][0] != [0.0; 2] {
                    touched = true;
                }
                if run_r && self.bands[index].history[1][0] != [0.0; 2] {
                    touched = true;
                }
                if touched {
                    self.snapshot_band(index);
                }
                let band = &mut self.bands[index];
                if run_l && band.history[0][0] == [0.0; 2] && !quieted[0] {
                    for frame in working.iter_mut() {
                        frame[0] = quiet(frame[0]);
                    }
                    quieted[0] = true;
                } else if run_l && band.history[0][0] != [0.0; 2] {
                    let h = &mut band.history[0][0];
                    for frame in working.iter_mut() {
                        let output = frame[0] + h[0];
                        h[0] = quiet(h[1]);
                        h[1] = 0.0;
                        frame[0] = quiet(output);
                    }
                    quieted[0] = true;
                }
                if run_r && band.history[1][0] == [0.0; 2] && !quieted[1] {
                    for frame in working.iter_mut() {
                        frame[1] = quiet(frame[1]);
                    }
                    quieted[1] = true;
                } else if run_r && band.history[1][0] != [0.0; 2] {
                    let h = &mut band.history[1][0];
                    for frame in working.iter_mut() {
                        let output = frame[1] + h[0];
                        h[0] = quiet(h[1]);
                        h[1] = 0.0;
                        frame[1] = quiet(output);
                    }
                    quieted[1] = true;
                }
                continue;
            }
            self.snapshot_band(index);
            let band = &mut self.bands[index];
            if run_l && run_r {
                for stage in 0..used {
                    let c = band.current[stage];
                    let mut h0_l = band.history[0][stage][0];
                    let mut h1_l = band.history[0][stage][1];
                    let mut h0_r = band.history[1][stage][0];
                    let mut h1_r = band.history[1][stage][1];
                    for frame in working.iter_mut() {
                        let x_l = frame[0];
                        let y_l = c.b0 * x_l + h0_l;
                        h0_l = quiet(c.b1 * x_l - c.a1 * y_l + h1_l);
                        h1_l = quiet(c.b2 * x_l - c.a2 * y_l);
                        frame[0] = quiet(y_l);

                        let x_r = frame[1];
                        let y_r = c.b0 * x_r + h0_r;
                        h0_r = quiet(c.b1 * x_r - c.a1 * y_r + h1_r);
                        h1_r = quiet(c.b2 * x_r - c.a2 * y_r);
                        frame[1] = quiet(y_r);
                    }
                    band.history[0][stage] = [h0_l, h1_l];
                    band.history[1][stage] = [h0_r, h1_r];
                }
                quieted = [true; 2];
            } else {
                let ch = usize::from(run_r);
                for stage in 0..used {
                    let c = band.current[stage];
                    let mut h0 = band.history[ch][stage][0];
                    let mut h1 = band.history[ch][stage][1];
                    for frame in working.iter_mut() {
                        let x = frame[ch];
                        let y = c.b0 * x + h0;
                        h0 = quiet(c.b1 * x - c.a1 * y + h1);
                        h1 = quiet(c.b2 * x - c.a2 * y);
                        frame[ch] = quiet(y);
                    }
                    band.history[ch][stage] = [h0, h1];
                }
                quieted[ch] = true;
            }
        }

        let gain = self.gain;
        if self.mode == 2 {
            for (dst, src) in frames.iter_mut().zip(working.iter()) {
                let l = (src[0] + src[1]) * gain;
                let r = (src[0] - src[1]) * gain;
                *dst = [finite_f32(l), finite_f32(r)];
            }
        } else {
            for (dst, src) in frames.iter_mut().zip(working.iter()) {
                *dst = [finite_f32(src[0] * gain), finite_f32(src[1] * gain)];
            }
        }
        self.last_frame = next_last_frame;
        self.fallback_frame = self.fallback_frame.wrapping_add(len as u64);
        true
    }
}
pub(super) fn stages_used(stages: &BandStages) -> u8 {
    // Always run one section so even an identity band applies the existing
    // finite-value and denormal law. Further untouched identity sections
    // have zero history and would repeat that same operation.
    stages
        .iter()
        .rposition(|stage| *stage != Coefficients::IDENTITY)
        .map_or(1, |index| (index + 1) as u8)
}

pub(super) fn filter(mut input: f64, stages: &[Coefficients], history: &mut [[f64; 2]]) -> f64 {
    for (c, h) in stages.iter().zip(history) {
        input = c.process_tdf2_with_floor(input, h, hex_dsp::POWER_FLOOR);
    }
    input
}
pub(super) fn bank(mode: u8, channel: usize) -> u8 {
    match mode {
        1 => 1 + channel as u8,
        2 => 3 + channel as u8,
        _ => 0,
    }
}
