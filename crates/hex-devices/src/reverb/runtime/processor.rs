use super::*;

pub(super) type Ramp = hex_dsp::LinearRampF32;
/// Original exclusive requested allocations. This scalar observation is not a
/// custody receipt and never adopts arbitrary provider-owned storage.
#[derive(Clone, Copy)]
pub struct ReverbRuntimeStorage {
    pub inline_bytes: usize,
    pub committed_heap_bytes: [usize; RUNTIME_HEAP_ALLOCATIONS],
    pub scratch_heap_bytes: [usize; RUNTIME_HEAP_ALLOCATIONS],
}
pub const RUNTIME_HEAP_ALLOCATIONS: usize = 14;

#[derive(Clone, Copy)]
pub(super) struct ReverbState {
    pub(super) automation_controls: crate::parameter_binding::Controls<AutomatedSettings, 33>,
    pub(super) automated_values: AutomatedValues,
    pub(super) scatter_cache: ScatterCache,
    pub(super) feedback_cache: FeedbackCache,
    pub(super) parameters: [Ramp; PARAMETER_COUNT],
    pub(super) coefficients: [Ramp; COEFFICIENTS],
    pub(super) tank_ramp_remaining: u32,
    pub(super) silence_limit: u64,
    pub(super) silent_frames: u64,
    pub(super) active: bool,
    pub(super) input_low: [StereoFrame; 2],
    pub(super) input_high: [StereoFrame; 2],
    pub(super) feedback_low: [f32; 8],
    pub(super) feedback_high: [f32; 8],
    pub(super) phase: u32,
    pub(super) sum: StereoFrame,
    pub(super) previous: StereoFrame,
    pub(super) next: StereoFrame,
    pub(super) spin_phase: f32,
    pub(super) chorus_phase: f32,
}
impl ReverbState {
    pub(super) fn check_inline(&self) {
        let Self {
            automation_controls,
            automated_values,
            scatter_cache,
            feedback_cache,
            parameters,
            coefficients,
            tank_ramp_remaining,
            silence_limit,
            silent_frames,
            active,
            input_low,
            input_high,
            feedback_low,
            feedback_high,
            phase,
            sum,
            previous,
            next,
            spin_phase,
            chorus_phase,
        } = self;
        fn inline<T: Copy>(_: &T) {}
        automation_controls.check_inline(AutomatedSettings::check_inline);
        automated_values.check_inline();
        scatter_cache.check_inline();
        feedback_cache.check_inline();
        inline(parameters);
        inline(coefficients);
        inline(tank_ramp_remaining);
        inline(silence_limit);
        inline(silent_frames);
        inline(active);
        inline(input_low);
        inline(input_high);
        inline(feedback_low);
        inline(feedback_high);
        inline(phase);
        inline(sum);
        inline(previous);
        inline(next);
        inline(spin_phase);
        inline(chorus_phase);
    }
    pub(super) fn v(&self, id: hex_lattice_contract::ParameterId) -> f32 {
        self.parameters[(id.value() - 1) as usize].value()
    }
    pub(super) fn retime_canonical_ramps(&mut self, rate: u32, mode: u32) {
        if self.tank_ramp_remaining == 0 {
            return;
        }
        let frames = smoothing_frames(rate, mode);
        let mut remaining = 0;
        for ramp in self
            .parameters
            .iter_mut()
            .chain(self.coefficients.iter_mut())
        {
            if ramp.is_active() {
                ramp.retarget(ramp.target(), frames);
                remaining = frames;
            }
        }
        self.tank_ramp_remaining = remaining;
    }
    pub(super) fn clear_scalars(&mut self) {
        self.input_low = [[0.0; 2]; 2];
        self.input_high = [[0.0; 2]; 2];
        self.feedback_low = [0.0; 8];
        self.feedback_high = [0.0; 8];
        self.phase = 0;
        self.sum = [0.0; 2];
        self.previous = [0.0; 2];
        self.next = [0.0; 2];
        self.spin_phase = 0.0;
        self.chorus_phase = 0.0;
        self.silent_frames = 0;
        self.active = false;
    }
    pub(super) fn reset(&mut self, rate: u32, settings: Settings) {
        self.automation_controls.reset();
        self.automated_values = AutomatedValues::default();
        self.scatter_cache = ScatterCache::default();
        self.feedback_cache = FeedbackCache::default();
        self.silence_limit = silence_deadline_frames(rate, settings.get(p::DECAY));
        self.clear_scalars();
        for v in &mut self.parameters {
            *v = Ramp::settled(v.target());
        }
        for v in &mut self.coefficients {
            *v = Ramp::settled(v.target());
        }
        self.tank_ramp_remaining = 0;
    }
}

pub struct ReverbRuntime {
    pub(super) rate: u32,
    pub(super) factor: u32,
    pub(super) tank_rate: f32,
    pub(super) settled_tank: TankDesign,
    pub(super) settings: Settings,
    pub(super) primary: ReverbState,
    pub(super) speculative: ReverbState,
    pub(super) tail_controls_frozen: bool,
    pub(super) open: bool,
    pub(super) inputs: [RingPair; 2],
    pub(super) lines: [RingPair; 8],
    pub(super) allpasses: [[RingPair; 2]; 2],
}
impl ReverbRuntime {
    /// Original exclusive runtime leaf, including every requested Box payload.
    /// A borrowed declaration never transfers native custody. The closed host
    /// constructor must establish origin before using this bounded observation.
    /// Exhaustive destructuring forces an owning-field audit when this changes.
    pub fn control_storage(&self) -> Option<ReverbRuntimeStorage> {
        let Self {
            rate,
            factor,
            tank_rate,
            settled_tank,
            settings,
            primary,
            speculative,
            tail_controls_frozen,
            open,
            inputs,
            lines,
            allpasses,
        } = self;
        fn inline<T: Copy>(_: &T) {}
        inline(rate);
        inline(factor);
        inline(tank_rate);
        inline(settled_tank);
        inline(settings);
        primary.check_inline();
        speculative.check_inline();
        inline(tail_controls_frozen);
        inline(open);
        let mut committed_heap_bytes = [0; RUNTIME_HEAP_ALLOCATIONS];
        let mut scratch_heap_bytes = [0; RUNTIME_HEAP_ALLOCATIONS];
        let mut observed = 0;
        for (index, ring) in inputs
            .iter()
            .chain(lines.iter())
            .chain(allpasses.iter().flatten())
            .enumerate()
        {
            let RingPair {
                committed,
                scratch,
                committed_cursor,
                committed_filled,
                cursor,
                filled,
                dirty_start,
                dirty_len,
                cleared_in_block,
                post_clear_writes,
                post_clear_max,
            } = ring;
            inline(committed_cursor);
            inline(committed_filled);
            inline(cursor);
            inline(filled);
            inline(dirty_start);
            inline(dirty_len);
            inline(cleared_in_block);
            inline(post_clear_writes);
            inline(post_clear_max);
            *committed_heap_bytes.get_mut(index)? =
                committed.len().checked_mul(std::mem::size_of::<f32>())?;
            *scratch_heap_bytes.get_mut(index)? =
                scratch.len().checked_mul(std::mem::size_of::<f32>())?;
            observed += 1;
        }
        if observed != RUNTIME_HEAP_ALLOCATIONS {
            return None;
        }
        Some(ReverbRuntimeStorage {
            inline_bytes: std::mem::size_of::<Self>(),
            committed_heap_bytes,
            scratch_heap_bytes,
        })
    }
    pub fn control_storage_bytes(&self) -> Option<usize> {
        let storage = self.control_storage()?;
        let committed = storage
            .committed_heap_bytes
            .into_iter()
            .try_fold(storage.inline_bytes, usize::checked_add)?;
        storage
            .scratch_heap_bytes
            .into_iter()
            .try_fold(committed, usize::checked_add)
    }
    pub(super) fn rings_mut(&mut self) -> impl Iterator<Item = &mut RingPair> {
        self.inputs
            .iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.allpasses.iter_mut().flatten())
    }
    pub(super) fn inherit_scalars(&mut self, old: &ReverbState, old_settings: Settings) {
        self.tail_controls_frozen = false;
        self.primary
            .automation_controls
            .inherit(&old.automation_controls);
        self.primary.automated_values = old.automated_values;
        self.primary.scatter_cache = old.scatter_cache;
        self.primary.feedback_cache = old.feedback_cache;
        if self.primary.automation_controls.admitted(0) {
            self.primary.silence_limit = silence_deadline_frames(
                self.rate,
                self.primary
                    .automated_values
                    .decay
                    .unwrap_or(self.settings.get(p::DECAY)),
            );
        }
        let canonical_mode = self.settings.get(p::SMOOTH) as u32;
        let mode = if self
            .primary
            .automation_controls
            .admitted(automation::SMOOTH_INDEX)
        {
            old.automated_values.smooth.unwrap_or(canonical_mode)
        } else {
            canonical_mode
        };
        let frames = smoothing_frames(self.rate, mode);
        let mut ramp_remaining = 0;
        for (v, p) in self.primary.parameters.iter_mut().zip(old.parameters) {
            v.inherit(p, frames);
            ramp_remaining = ramp_remaining.max(v.remaining());
        }
        for (v, p) in self.primary.coefficients.iter_mut().zip(old.coefficients) {
            v.inherit(p, frames);
            ramp_remaining = ramp_remaining.max(v.remaining());
        }
        self.primary.tank_ramp_remaining = ramp_remaining;
        self.primary.input_low = old.input_low;
        self.primary.input_high = old.input_high;
        self.primary.feedback_low = old.feedback_low;
        self.primary.feedback_high = old.feedback_high;
        self.primary.phase = old.phase;
        self.primary.sum = old.sum;
        self.primary.previous = old.previous;
        self.primary.next = old.next;
        self.primary.spin_phase = old.spin_phase;
        self.primary.chorus_phase = old.chorus_phase;
        self.primary.active = old.active;
        self.primary.silent_frames = if self.settings.get(p::DECAY) != old_settings.get(p::DECAY)
            || self.settings.on(p::FREEZE) != old_settings.on(p::FREEZE)
        {
            0
        } else {
            old.silent_frames
        };
        self.speculative = self.primary;
        self.open = false;
    }
    pub(super) fn clear_speculative(&mut self) {
        for ring in self.rings_mut() {
            ring.clear();
        }
        self.speculative.clear_scalars();
    }
    pub(super) fn tank<const MATURE: bool>(&mut self, input: StereoFrame, output: AutomatedValues) {
        let s = &mut self.speculative;
        let freeze = output.freeze.unwrap_or_else(|| s.v(p::FREEZE));
        let flat = output.flat.unwrap_or_else(|| s.v(p::FLAT));
        let cut = output.cut.unwrap_or_else(|| s.v(p::CUT));
        let predelay = output.predelay.unwrap_or_else(|| s.v(p::PREDELAY));
        let spin_amount = output.spin_amount.unwrap_or_else(|| s.v(p::SPIN_AMOUNT));
        let spin_rate = output.spin_rate.unwrap_or_else(|| s.v(p::SPIN_RATE));
        let shape = output.shape.unwrap_or_else(|| s.v(p::SHAPE));
        let scale = output.scale.unwrap_or_else(|| s.v(p::SCALE));
        let chorus_amount = output
            .chorus_amount
            .unwrap_or_else(|| s.v(p::CHORUS_AMOUNT));
        let chorus_rate = output.chorus_rate.unwrap_or_else(|| s.v(p::CHORUS_RATE));
        let diffusion = output.diffusion.unwrap_or_else(|| s.v(p::DIFFUSION));
        let density = output.density.unwrap_or_else(|| s.v(p::DENSITY));
        let size = (output.size.unwrap_or_else(|| s.v(p::SIZE)) / 100.0).sqrt();
        let predelay = predelay * 0.001 * self.tank_rate;
        let excitation = 1.0 - freeze * cut;
        let settled = s.tank_ramp_remaining == 0;
        let [high_filter, low_filter] = if settled {
            self.settled_tank.filters
        } else {
            [
                InputFilter::new(s.coefficients[24].value(), s.coefficients[30].value()),
                InputFilter::new(s.coefficients[25].value(), s.coefficients[31].value()),
            ]
        };
        let high_filter = if output.lo_g.is_some() || output.lo_damping.is_some() {
            InputFilter::new(
                output.lo_g.unwrap_or(s.coefficients[24].value()),
                output.lo_damping.unwrap_or(s.coefficients[30].value()),
            )
        } else {
            high_filter
        };
        let low_filter = if output.hi_g.is_some() || output.hi_damping.is_some() {
            InputFilter::new(
                output.hi_g.unwrap_or(s.coefficients[25].value()),
                output.hi_damping.unwrap_or(s.coefficients[31].value()),
            )
        } else {
            low_filter
        };
        let lo_alpha = output.low_alpha.unwrap_or(s.coefficients[26].value());
        let hi_alpha = output.high_alpha.unwrap_or(s.coefficients[27].value());
        let mut late_input = [0.0; 2];
        let mut early = [0.0; 2];
        let spin_scale =
            output.spin.unwrap_or_else(|| s.v(p::SPIN)) * spin_amount * 0.000025 * self.tank_rate;
        let spin_sc = (spin_scale != 0.0).then(|| (std::f32::consts::TAU * s.spin_phase).sin_cos());
        let tap_scale = size * (0.75 + 0.5 * shape) * self.tank_rate;
        let lo_cut = output.lo_cut.unwrap_or_else(|| s.v(p::LO_CUT));
        let hi_cut = output.hi_cut.unwrap_or_else(|| s.v(p::HI_CUT));
        for c in 0..2 {
            let mut x = input[c] * excitation;
            let high = high_filter.apply(&mut s.input_low[c], x, true);
            x += (high - x) * lo_cut;
            let low = low_filter.apply(&mut s.input_high[c], x, false);
            x += (low - x) * hi_cut;
            self.inputs[c].push(x);
            late_input[c] = self.inputs[c].read::<MATURE>(predelay + 1.0);
            // Channel 1 adds +0.25 turns (+pi/2): sin(a + pi/2 + b) = cos(a)cos(b) - sin(a)sin(b).
            let spin_channel = spin_sc.map(|(si, co)| if c == 0 { (si, co) } else { (co, -si) });
            for (i, seconds) in TAPS.into_iter().enumerate() {
                let spin = if let Some((si, co)) = spin_channel {
                    let (tap_sin, tap_cos) = SPIN_TAP_SIN_COS[i];
                    (si * tap_cos + co * tap_sin) * spin_scale
                } else {
                    0.0
                };
                let delay = predelay + (seconds * tap_scale + spin).max(0.0) + 1.0;
                let weight = (1.0 - shape + shape * (i as f32 / 7.0)) * 0.125;
                early[c] += self.inputs[c].read::<MATURE>(delay)
                    * weight
                    * if i % 3 == 2 { -1.0 } else { 1.0 };
            }
            late_input[c] += early[c] * 0.25;
        }
        let depth = output.chorus.unwrap_or_else(|| s.v(p::CHORUS))
            * chorus_amount
            * 0.001
            * self.tank_rate
            * (1.0 - freeze);
        let chorus_sc = (depth != 0.0).then(|| (std::f32::consts::TAU * s.chorus_phase).sin_cos());
        let delayed = std::array::from_fn(|i| {
            let modulation = if let Some((si, co)) = chorus_sc {
                let (tap_sin, tap_cos) = CHORUS_TAP_SIN_COS[i];
                (si * tap_cos + co * tap_sin) * depth
            } else {
                0.0
            };
            let base = TIMES[i] * size * self.tank_rate;
            let delay = base + freeze * (base.round() - base);
            self.lines[i].read::<MATURE>(delay + modulation)
        });
        let rotations = if output.diffusion.is_some() || output.density.is_some() {
            s.scatter_cache.resolve(diffusion, density)
        } else if settled {
            self.settled_tank.rotations
        } else {
            scatter_rotations(s.v(p::DIFFUSION) * 0.01, s.v(p::DENSITY) + 1.0)
        };
        let feedback = scatter(delayed, rotations);
        let authored_feedback = if output.decay.is_some()
            || output.size.is_some()
            || output.high_ratio.is_some()
            || output.low_ratio.is_some()
            || output.high.is_some()
            || output.low.is_some()
            || output.freeze.is_some()
            || output.flat.is_some()
        {
            let feedback_parameters = [
                output.decay.unwrap_or_else(|| s.v(p::DECAY)),
                size,
                output.high_ratio.unwrap_or_else(|| s.v(p::HIGH_RATIO)),
                output.low_ratio.unwrap_or_else(|| s.v(p::LOW_RATIO)),
                freeze,
                flat,
                output.low.unwrap_or_else(|| s.v(p::LOW)),
                output.high.unwrap_or_else(|| s.v(p::HIGH)),
            ];
            Some(*s.feedback_cache.resolve(feedback_parameters))
        } else {
            None
        };
        for (i, sample) in feedback.into_iter().enumerate() {
            s.feedback_low[i] = clean(lo_alpha * s.feedback_low[i] + (1.0 - lo_alpha) * sample);
            // Cascaded shelves have a conservative product-gain bound. Keep
            // even crossed cutoffs and in-flight mode/ratio edits passive;
            // ordinary feedback stays below unity, flat freeze reaches unity.
            let (mid, low, high_ratio, normalizer) = if let Some(gains) = authored_feedback {
                gains[i]
            } else if settled {
                self.settled_tank.feedback[i]
            } else {
                shaping_gains(
                    s.coefficients[i * 3].value(),
                    s.coefficients[i * 3 + 1].value(),
                    s.coefficients[i * 3 + 2].value(),
                )
            };
            let low_shaped = sample * mid + s.feedback_low[i] * (low - mid);
            s.feedback_high[i] =
                clean(hi_alpha * s.feedback_high[i] + (1.0 - hi_alpha) * low_shaped);
            let shaped =
                (s.feedback_high[i] + (low_shaped - s.feedback_high[i]) * high_ratio) * normalizer;
            let excitation = (late_input[0] * if i & 1 == 0 { 1.0 } else { -1.0 }
                + late_input[1] * if i & 2 == 0 { 1.0 } else { -1.0 })
                * 0.25;
            self.lines[i].push(shaped + excitation);
        }
        s.previous = s.next;
        for (c, reflection) in early.into_iter().enumerate() {
            let mut diffuse = delayed
                .iter()
                .enumerate()
                .map(|(i, v)| v * if (i >> (c + 1)) & 1 == 0 { 0.25 } else { -0.25 })
                .sum::<f32>();
            for (i, seconds) in [0.005, 0.0017].into_iter().enumerate() {
                let a = diffusion * 0.006;
                let old = self.allpasses[c][i]
                    .read::<MATURE>(seconds * (0.5 + 1.5 * scale * 0.01) * self.tank_rate);
                let out = old - a * diffuse;
                self.allpasses[c][i].push(diffuse + a * out);
                diffuse = out;
            }
            s.next[c] = reflection * output.reflect.unwrap_or(s.coefficients[28].value())
                + diffuse * output.diffuse.unwrap_or(s.coefficients[29].value());
        }
        let mid = (s.next[0] + s.next[1]) * 0.5;
        let side =
            (s.next[0] - s.next[1]) * 0.5 * output.stereo.unwrap_or_else(|| s.v(p::STEREO)) * 0.01;
        s.next = [mid + side, mid - side];
        s.spin_phase = (s.spin_phase + spin_rate / self.tank_rate).fract();
        s.chorus_phase = (s.chorus_phase + chorus_rate / self.tank_rate).fract();
    }
    pub(super) fn frame(&mut self, frame: &mut StereoFrame, dry_wet: Option<f32>) {
        self.frame_outputs(
            frame,
            AutomatedValues {
                dry_wet,
                ..AutomatedValues::default()
            },
        );
    }
    pub(super) fn frame_outputs(&mut self, frame: &mut StereoFrame, output: AutomatedValues) {
        if self.speculative.tank_ramp_remaining != 0 {
            for ramp in &mut self.speculative.parameters {
                ramp.advance();
            }
            for ramp in &mut self.speculative.coefficients {
                ramp.advance();
            }
            self.speculative.tank_ramp_remaining -= 1;
        }
        let input = frame.map(|v| {
            if v.is_finite() {
                v.clamp(-16.0, 16.0)
            } else {
                0.0
            }
        });
        let freeze = output
            .freeze
            .unwrap_or_else(|| self.speculative.v(p::FREEZE));
        let cut = output.cut.unwrap_or_else(|| self.speculative.v(p::CUT));
        if input != [0.0; 2] && freeze * cut < 1.0 {
            self.speculative.active = true;
            self.speculative.silent_frames = 0;
        } else if self.speculative.active && freeze == 0.0 {
            self.speculative.silent_frames += 1;
            if self.speculative.silent_frames >= self.speculative.silence_limit {
                self.clear_speculative();
            }
        }
        if self.speculative.active {
            for (sum, v) in self.speculative.sum.iter_mut().zip(input) {
                *sum += v;
            }
            self.speculative.phase += 1;
            if self.speculative.phase == self.factor {
                self.speculative.phase = 0;
                let averaged = self.speculative.sum.map(|v| v / self.factor as f32);
                if self.inputs[0].filled {
                    self.tank::<true>(averaged, output);
                } else {
                    self.tank::<false>(averaged, output);
                }
                self.speculative.sum = [0.0; 2];
            }
        }
        let fraction = (self.speculative.phase + 1) as f32 / self.factor as f32;
        let wet = output
            .dry_wet
            .unwrap_or_else(|| self.speculative.v(p::DRY_WET))
            * 0.01;
        for c in 0..2 {
            let room = self.speculative.previous[c]
                + fraction * (self.speculative.next[c] - self.speculative.previous[c]);
            frame[c] = ((1.0 - wet) * input[c] + wet * room).clamp(-16.0, 16.0);
        }
    }
    pub(super) fn process(&mut self, frames: &mut [StereoFrame], bypassed: bool) {
        if !self.open {
            self.begin_block();
        }
        if !frames.is_empty() && !self.tail_controls_frozen {
            // This active render has no provider automation view. Discard
            // inherited delivery state without resetting the room; a later
            // tail must freeze canonical controls, and resumed identical
            // authored values must be sampled into a fresh cache.
            let canonical_mode = self.settings.get(p::SMOOTH) as u32;
            if self
                .speculative
                .automated_values
                .smooth
                .is_some_and(|mode| mode != canonical_mode)
            {
                self.speculative
                    .retime_canonical_ramps(self.rate, canonical_mode);
            }
            self.speculative.automation_controls.reset();
            self.speculative.automated_values = AutomatedValues::default();
            self.speculative.silence_limit =
                silence_deadline_frames(self.rate, self.settings.get(p::DECAY));
        }
        for frame in frames {
            let dry = *frame;
            if self.tail_controls_frozen {
                self.frame_outputs(frame, self.speculative.automated_values);
            } else {
                self.frame(frame, None);
            }
            if bypassed {
                *frame = dry;
            }
        }
    }
}
pub(super) fn shaping_gains(low: f32, mid: f32, high: f32) -> FeedbackGains {
    let mid = mid.clamp(1e-8, 1.0);
    let envelope = mid.sqrt();
    let low = low.clamp(0.0, envelope);
    let high = high.clamp(0.0, envelope);
    let high_ratio = high / mid;
    let product = low.max(mid) * high_ratio.max(1.0);
    let normalizer = (mid.max(low).max(high) / product).min(1.0);
    (mid, low, high_ratio, normalizer)
}
pub(super) fn scatter_rotations(amount: f32, stages: f32) -> Rotations {
    std::array::from_fn(|stage| {
        let weight = (stages - stage as f32).clamp(0.0, 1.0);
        (weight != 0.0).then(|| (amount * weight * std::f32::consts::FRAC_PI_4).sin_cos())
    })
}
pub(super) fn scatter(mut output: [f32; 8], rotations: Rotations) -> [f32; 8] {
    for (stage, rotation) in rotations.into_iter().enumerate() {
        let Some((sin, cos)) = rotation else {
            continue;
        };
        let stride = 1 << stage;
        for base in (0..8).step_by(stride * 2) {
            for offset in 0..stride {
                let a = output[base + offset];
                let b = output[base + offset + stride];
                output[base + offset] = a * cos + b * sin;
                output[base + offset + stride] = b * cos - a * sin;
            }
        }
    }
    output
}
impl OwnedProcessorRuntime for ReverbRuntime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        self.control_storage_bytes()
    }
    fn begin_block(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.speculative = self.primary;
        for ring in self.rings_mut() {
            ring.begin_block();
        }
        self.open = true;
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn DeviceResources) {
        self.process(frames, false);
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        _: &dyn DeviceResources,
    ) {
        self.process(frames, true);
    }
    fn commit_block(&mut self) {
        if !self.open {
            return;
        }
        for ring in self.rings_mut() {
            ring.commit();
        }
        self.primary = self.speculative;
        self.open = false;
    }
    fn rollback_block(&mut self) {
        if !self.open {
            return;
        }
        for ring in self.rings_mut() {
            ring.rollback();
        }
        self.speculative = self.primary;
        self.open = false;
    }
    fn begin_output_tail(&mut self) {
        self.tail_controls_frozen = true;
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        automation::supports(parameter)
            && (parameter != p::DECAY || self.primary.automation_controls.admitted(0))
            && (parameter != p::FREEZE
                || self
                    .primary
                    .automation_controls
                    .admitted(automation::FREEZE_INDEX))
            && (parameter != p::SMOOTH
                || self
                    .primary
                    .automation_controls
                    .admitted(automation::SMOOTH_INDEX))
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, automation, false)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, automation, true)
    }
    fn reset(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.tail_controls_frozen = false;
        for ring in self.rings_mut() {
            ring.reset();
        }
        self.primary.reset(self.rate, self.settings);
        self.speculative = self.primary;
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        !self.primary.active
    }
    fn speculative_is_quiescent(&self) -> bool {
        if self.open {
            !self.speculative.active
        } else {
            !self.primary.active
        }
    }
    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|p| !self.open && !p.open && p.rate == self.rate)
    }
    fn can_retarget_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(previous)
    }
    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(previous) {
            return;
        }
        let Some(old) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        for (to, from) in self
            .inputs
            .iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.allpasses.iter_mut().flatten())
            .zip(
                old.inputs
                    .iter()
                    .chain(old.lines.iter())
                    .chain(old.allpasses.iter().flatten()),
            )
        {
            to.copy_committed_from(from);
        }
        self.inherit_scalars(&old.primary, old.settings);
    }
    fn transfer_history_from(&mut self, previous: &mut dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(previous) {
            return;
        }
        let Some(old) = previous.as_any_mut().and_then(|v| v.downcast_mut::<Self>()) else {
            return;
        };
        for (to, from) in self
            .inputs
            .iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.allpasses.iter_mut().flatten())
            .zip(
                old.inputs
                    .iter_mut()
                    .chain(old.lines.iter_mut())
                    .chain(old.allpasses.iter_mut().flatten()),
            )
        {
            to.swap_committed_from(from);
        }
        let old_primary = old.primary;
        let old_settings = old.settings;
        self.inherit_scalars(&old_primary, old_settings);
        old.reset();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}
