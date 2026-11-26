use super::*;

/// Maximum event-free sub-block size for voice-outer / sample-inner rendering.
const SUB_BLOCK: usize = 64;

/// Automated filter coefficient update cadence (in frames). Updates sample 0 of
/// every sub-block and every 16 frames thereafter, cutting per-sample `tan()`
/// computations by 16x while keeping static and stepped changes exact.
const AUTOMATED_FILTER_STRIDE: usize = 16;

#[derive(Clone, Copy)]
struct SubBlockSample {
    beat: Option<u64>,
    running: Option<bool>,
    spectrum: Option<Spectrum>,
    parameters: [f64; 12],
    pitch_mod: Option<([f64; OPERATORS], [f64; OPERATORS])>,
    automated_filter_triplet: Option<[f32; 3]>,
    automated_filter_frame: Option<crate::operator_filter::Frame>,
    filter_frame: Option<crate::operator_filter::Frame>,
    phase: (f64, u64),
    lfo_amount: f64,
    delta: f64,
}

impl Default for SubBlockSample {
    fn default() -> Self {
        Self {
            beat: None,
            running: None,
            spectrum: None,
            parameters: [0.0; 12],
            pitch_mod: None,
            automated_filter_triplet: None,
            automated_filter_frame: None,
            filter_frame: None,
            phase: (0.0, 0),
            lfo_amount: 0.0,
            delta: 0.0,
        }
    }
}

#[inline]
fn channel_pitch_bend_cents(channel_pitch_bend: i16) -> f64 {
    if channel_pitch_bend == 0 {
        0.0
    } else {
        f64::from(channel_pitch_bend)
            / if channel_pitch_bend < 0 {
                8192.0
            } else {
                8191.0
            }
            * 200.0
    }
}

#[inline]
fn voice_is_finished(
    voice: &Voice,
    quality: bool,
    companion_active: bool,
    release_frames: u64,
) -> bool {
    if voice.gate.is_none() {
        return false;
    }
    if quality {
        voice.drain >= quality::TAIL as u32
    } else if companion_active {
        voice.machines.iter().all(|machine| {
            machine
                .as_ref()
                .is_some_and(|machine| machine.finished(voice.age))
        })
    } else {
        voice
            .gate
            .is_some_and(|gate| voice.age >= gate.saturating_add(release_frames))
    }
}

impl Runtime {
    pub(super) fn read_target(
        values: Option<&dyn NativeParameterAutomation>,
        mask: u32,
        index: usize,
        offset: usize,
    ) -> Option<f32> {
        if (mask & (1_u32 << index)) == 0 {
            return None;
        }
        match values?.value_at(automation::TARGET_IDS[index], offset) {
            Some(ParameterValue::Float(v)) if v.is_finite() => Some(v),
            _ => None,
        }
    }

    pub(super) fn process_inner(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[hex_lattice::NativeInstrumentOccurrenceGain],
        values: Option<&dyn NativeParameterAutomation>,
        mask: u32,
    ) {
        const LEVEL_MASK: u32 = (1 << 1) | (1 << 3) | (1 << 5) | (1 << 7);
        const FEEDBACK_MASK: u32 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11);
        const PITCH_MASK: u32 = (1 << 0)
            | (1 << 2)
            | (1 << 4)
            | (1 << 6)
            | (1 << 14)
            | (1 << 15)
            | (1 << 16)
            | (1 << 17);
        const FILTER_MASK: u32 = (1 << 18) | (1 << 19) | (1 << 20);

        let mut cursor = 0;
        let mut voice_end = self
            .voices
            .iter()
            .rposition(|voice| voice.id != 0)
            .map_or(0, |i| i + 1);
        let mut has_fading = self.fading.iter().any(|fade| fade.remaining > 0);
        let companion = self.factory.settings.companion;
        let companion_active = self.factory.companion_active;
        let needs_companion_beat = companion.needs_clock();
        let has_modulation = self.factory.configuration.has_modulation();
        let needs_companion_transport = companion.needs_running();
        let base_targets = automation::base_target_values(&self.factory.settings);
        let rate_f64 = f64::from(self.factory.rate);
        let step_per_hz = (u64::MAX as f64 + 1.0) / rate_f64;
        let quality = self.factory.settings.global.active;
        let release_frames = self.factory.configuration.release_frames();
        let effective_filter_rate = if quality {
            self.factory.rate.saturating_mul(2)
        } else {
            self.factory.rate
        };
        let law = self.factory.settings.lfo;
        let mut cached_filter: Option<([f32; 3], crate::operator_filter::Frame)> = None;
        let mut sub_samples = [SubBlockSample::default(); SUB_BLOCK];

        let mut sub_start = 0;
        while sub_start < frames.len() {
            // Apply all MIDI events arriving at `sub_start` at their exact offset.
            let mut voices_changed = false;
            if events
                .get(cursor)
                .is_some_and(|event| event.frame_offset() == sub_start)
            {
                let beat = if needs_companion_beat {
                    clock.and_then(|clock| clock.beat_at_offset(sub_start))
                } else {
                    None
                };
                let running = if needs_companion_transport {
                    clock.and_then(|clock| clock.transport_is_running())
                } else {
                    None
                };
                if companion_active {
                    for voice in self.voices.iter_mut().take(voice_end) {
                        if voice.id != 0 {
                            for machine in voice.machines.iter_mut().flatten() {
                                let _ = machine.observe(
                                    if quality {
                                        voice.envelope_age as u64
                                    } else {
                                        voice.age
                                    },
                                    beat,
                                    running,
                                );
                            }
                        }
                    }
                }
                while let Some(event) = events.get(cursor) {
                    if event.frame_offset() != sub_start {
                        break;
                    }
                    let origin = clock
                        .and_then(|clock| clock.performance_frame_at_offset(sub_start))
                        .unwrap_or(self.local_frame);
                    let _ = self.apply(*event, origin, beat, running);
                    voices_changed = true;
                    cursor += 1;
                }
                if voices_changed {
                    voice_end = self
                        .voices
                        .iter()
                        .rposition(|voice| voice.id != 0)
                        .map_or(0, |i| i + 1);
                    has_fading = self.fading.iter().any(|fade| fade.remaining > 0);
                }
            }

            let next_event = events
                .get(cursor)
                .map_or(frames.len(), |event| event.frame_offset());
            let sub_end = next_event.min(sub_start + SUB_BLOCK).min(frames.len());
            if sub_end <= sub_start {
                break;
            }
            let sub_len = sub_end - sub_start;

            // Pre-evaluate block-wide parameters, LFO phase, and sub-block filter
            // coefficients once across `sub_start..sub_end` before the voice-outer loop.
            let mut sub_filter_triplet: Option<[f32; 3]> = None;
            let mut sub_filter_frame: Option<crate::operator_filter::Frame> = None;
            for (local, slot) in sub_samples[..sub_len].iter_mut().enumerate() {
                let offset = sub_start + local;
                let beat = if needs_companion_beat {
                    clock.and_then(|clock| clock.beat_at_offset(offset))
                } else {
                    None
                };
                let running = if needs_companion_transport {
                    clock.and_then(|clock| clock.transport_is_running())
                } else {
                    None
                };
                let spectrum = (self.elapsed < self.frames).then(|| self.spectrum());
                let mut parameters = self.current_parameters();
                if (mask & LEVEL_MASK) != 0 {
                    for (op, parameter) in parameters.iter_mut().enumerate().take(OPERATORS) {
                        if let Some(level) = Self::read_target(values, mask, op * 2 + 1, offset) {
                            *parameter = if self.factory.settings.enabled.flags()[op] {
                                f64::from(level).clamp(0.0, 8.0)
                            } else {
                                0.0
                            };
                        }
                    }
                }
                if (mask & FEEDBACK_MASK) != 0 {
                    for op in 0..OPERATORS {
                        if let Some(fb) = Self::read_target(values, mask, 8 + op, offset) {
                            parameters[8 + op] = if self.factory.settings.feedback_enabled(op) {
                                f64::from(fb).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                        }
                    }
                }
                let pitch_mod = if (mask & PITCH_MASK) != 0 {
                    let mut scales = [1.0_f64; OPERATORS];
                    let mut fine_deltas = [0.0_f64; OPERATORS];
                    for op in 0..OPERATORS {
                        if let Some(ratio) = Self::read_target(values, mask, op * 2, offset) {
                            let base_ratio = f64::from(base_targets[op * 2]).max(0.5);
                            scales[op] = f64::from(ratio).clamp(0.5, 32.0) / base_ratio;
                        }
                        if let Some(fine_hz) = Self::read_target(values, mask, 14 + op, offset) {
                            let base_fine = f64::from(base_targets[14 + op]);
                            fine_deltas[op] = (f64::from(fine_hz).clamp(-1000.0, 1000.0)
                                - base_fine)
                                * step_per_hz;
                        }
                    }
                    Some((scales, fine_deltas))
                } else {
                    None
                };
                if (mask & FILTER_MASK) != 0 && self.filter.armed {
                    if local.is_multiple_of(AUTOMATED_FILTER_STRIDE) {
                        let freq = Self::read_target(values, mask, 18, offset)
                            .unwrap_or(base_targets[18])
                            .clamp(20.0, 20_000.0);
                        let res = Self::read_target(values, mask, 19, offset)
                            .unwrap_or(base_targets[19])
                            .clamp(0.0, 100.0);
                        let morph = Self::read_target(values, mask, 20, offset)
                            .unwrap_or(base_targets[20])
                            .clamp(0.0, 100.0);
                        let triplet = [freq, res, morph];
                        let frame = if let Some((prev, frame)) = cached_filter
                            && prev == triplet
                        {
                            Some(frame)
                        } else {
                            self.factory
                                .settings
                                .filter
                                .frame_with_overrides(
                                    effective_filter_rate,
                                    f64::from(freq),
                                    f64::from(res),
                                    f64::from(morph),
                                )
                                .inspect(|&frame| {
                                    cached_filter = Some((triplet, frame));
                                })
                        };
                        sub_filter_triplet = Some(triplet);
                        sub_filter_frame = frame;
                    }
                } else {
                    sub_filter_triplet = None;
                    sub_filter_frame = None;
                }
                let filter_frame =
                    sub_filter_frame.or_else(|| self.filter.armed.then(|| self.filter.frame()));
                let lfo_rate_hz = Self::read_target(values, mask, 12, offset)
                    .map_or_else(|| law.rate(), |v| f64::from(v).clamp(0.01, 100.0));
                let lfo_amount = Self::read_target(values, mask, 13, offset)
                    .map_or_else(|| law.amount(), |v| f64::from(v).clamp(0.0, 100.0) / 100.0);
                let delta = if !law.active() {
                    0.0
                } else if law.sync() {
                    clock
                        .and_then(|c| c.beat_at_offset(offset).zip(c.beat_at_offset(offset + 1)))
                        .map_or(0.0, |(a, b)| {
                            b.saturating_sub(a) as f64 / 4_294_967_296.0 / law.beats()
                        })
                } else {
                    lfo_rate_hz / rate_f64
                };
                *slot = SubBlockSample {
                    beat,
                    running,
                    spectrum,
                    parameters,
                    pitch_mod,
                    automated_filter_triplet: sub_filter_triplet,
                    automated_filter_frame: sub_filter_frame,
                    filter_frame,
                    phase: self.phase,
                    lfo_amount,
                    delta,
                };
                if law.active() {
                    let next = self.phase.0 + delta;
                    self.phase.1 = self.phase.1.wrapping_add(next.floor() as u64);
                    self.phase.0 = next.fract();
                }
                self.elapsed = self.elapsed.saturating_add(1).min(self.frames);
                if let Some(r) = &mut self.parameters {
                    r.elapsed += 1;
                    if r.elapsed >= r.frames {
                        self.parameters = None;
                    }
                }
                self.filter.advance();
                self.local_frame = self.local_frame.saturating_add(1);
            }

            // Voice-outer, sample-inner render pass: keeps each voice's hot state
            // in registers/L1, hoists pitch-bend `powf` and timbre/pan math out of
            // the inner sample loop, and retires finished voices inline without
            // scanning all 64 voices per sample.
            let channel_cents = if quality {
                0.0
            } else {
                channel_pitch_bend_cents(self.channel_pitch_bend)
            };
            let cc_timbre = self.cc_timbre;
            let factory = &self.factory;
            let sub_frames = &mut frames[sub_start..sub_end];
            let sub_slice = &sub_samples[..sub_len];
            let mut any_expired = false;

            for voice in self.voices.iter_mut().take(voice_end) {
                if voice.id == 0 {
                    continue;
                }
                let pitch_ratio = if voice.pitch_offset_cents != 0
                    || (!quality && self.channel_pitch_bend != 0)
                {
                    Some(
                        2.0_f64
                            .powf((f64::from(voice.pitch_offset_cents) + channel_cents) / 1200.0),
                    )
                } else {
                    None
                };
                let voice_timbre = voice.timbre.max(cc_timbre);
                let timbre_boost = if voice_timbre != 0 {
                    Some(1.0 + 0.5 * (f64::from(voice_timbre) / 127.0))
                } else {
                    None
                };
                let mut voice_cached_filter: Option<([f32; 3], crate::operator_filter::Frame)> =
                    None;
                let voice_id = voice.id;
                let base_steps = if quality {
                    voice.pitch_steps
                } else {
                    PreparedOperatorConfiguration::lfo_steps(&voice.note)
                };
                let static_steps = if let Some(ratio) = pitch_ratio {
                    base_steps.map(|step| {
                        (((step as f64) * ratio).round() as u64).clamp(1, (1_u64 << 63) - 1)
                    })
                } else {
                    base_steps
                };

                for (local, (frame, sample_ctx)) in
                    sub_frames.iter_mut().zip(sub_slice.iter()).enumerate()
                {
                    if companion_active && !(local == 0 && voices_changed) {
                        for machine in voice.machines.iter_mut().flatten() {
                            let _ = machine.observe(
                                if quality {
                                    voice.envelope_age as u64
                                } else {
                                    voice.age
                                },
                                sample_ctx.beat,
                                sample_ctx.running,
                            );
                        }
                    }
                    let [left, right] = render_voice_sample(
                        voice,
                        factory,
                        sample_ctx,
                        base_steps,
                        static_steps,
                        pitch_ratio,
                        timbre_boost,
                        has_modulation,
                        effective_filter_rate,
                        &mut voice_cached_filter,
                    );
                    let offset = sub_start + local;
                    frame[0] += super::occurrence_gain::sample(left, voice_id, offset, gains);
                    frame[1] += super::occurrence_gain::sample(right, voice_id, offset, gains);
                    if voice_is_finished(voice, quality, companion_active, release_frames) {
                        *voice = Voice::default();
                        any_expired = true;
                        break;
                    }
                }
            }

            if any_expired {
                voice_end = self
                    .voices
                    .iter()
                    .rposition(|voice| voice.id != 0)
                    .map_or(0, |i| i + 1);
            }

            if has_fading {
                let mut any_fading = false;
                for fade in &mut self.fading {
                    if fade.remaining == 0 {
                        continue;
                    }
                    let pitch_ratio = if fade.voice.pitch_offset_cents != 0
                        || (!quality && self.channel_pitch_bend != 0)
                    {
                        Some(2.0_f64.powf(
                            (f64::from(fade.voice.pitch_offset_cents) + channel_cents) / 1200.0,
                        ))
                    } else {
                        None
                    };
                    let voice_timbre = fade.voice.timbre.max(cc_timbre);
                    let timbre_boost = if voice_timbre != 0 {
                        Some(1.0 + 0.5 * (f64::from(voice_timbre) / 127.0))
                    } else {
                        None
                    };
                    let mut voice_cached_filter: Option<([f32; 3], crate::operator_filter::Frame)> =
                        None;
                    let base_steps = if quality {
                        fade.voice.pitch_steps
                    } else {
                        PreparedOperatorConfiguration::lfo_steps(&fade.voice.note)
                    };
                    let static_steps = if let Some(ratio) = pitch_ratio {
                        base_steps.map(|step| {
                            (((step as f64) * ratio).round() as u64).clamp(1, (1_u64 << 63) - 1)
                        })
                    } else {
                        base_steps
                    };
                    let denom = f32::from(fade.total.max(1));
                    for (frame, sample_ctx) in sub_frames.iter_mut().zip(sub_slice.iter()) {
                        let gain = f32::from(fade.remaining) / denom;
                        let [left, right] = render_voice_sample(
                            &mut fade.voice,
                            factory,
                            sample_ctx,
                            base_steps,
                            static_steps,
                            pitch_ratio,
                            timbre_boost,
                            has_modulation,
                            effective_filter_rate,
                            &mut voice_cached_filter,
                        );
                        frame[0] += left * gain;
                        frame[1] += right * gain;
                        fade.remaining -= 1;
                        if fade.remaining == 0 {
                            *fade = StealFade::default();
                            break;
                        }
                    }
                    if fade.remaining > 0 {
                        any_fading = true;
                    }
                }
                has_fading = any_fading;
            }

            sub_start = sub_end;
        }

        if !frames.is_empty() {
            if mask != 0 {
                let last = frames.len() - 1;
                for (index, (last_target, &base_target)) in self
                    .last_targets
                    .iter_mut()
                    .zip(base_targets.iter())
                    .enumerate()
                {
                    *last_target =
                        Self::read_target(values, mask, index, last).unwrap_or(base_target);
                }
            } else {
                self.last_targets = base_targets;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn render_voice_sample(
    voice: &mut Voice,
    factory: &Factory,
    ctx: &SubBlockSample,
    base_steps: [u64; OPERATORS],
    static_steps: [u64; OPERATORS],
    pitch_ratio: Option<f64>,
    timbre_boost: Option<f64>,
    has_modulation: bool,
    effective_filter_rate: u32,
    voice_cached_filter: &mut Option<([f32; 3], crate::operator_filter::Frame)>,
) -> StereoFrame {
    let law = factory.settings.lfo;
    let quality = factory.settings.global.active;
    let spectrum = ctx.spectrum.as_ref().unwrap_or(&factory.target);
    let parameters = ctx.parameters;
    let phase = if law.retrigger() || quality {
        (voice.lfo_fraction, voice.lfo_cycle)
    } else {
        ctx.phase
    };
    let multiplier = if law.active() {
        ((if quality {
            factory.lfo_table.sample(
                law.0[1],
                phase.0,
                phase.1,
                factory.settings.global.interpolation,
            )
        } else {
            lfo::waveform(law.0[1], phase.0, phase.1)
        }) * (ctx.lfo_amount
            + if quality {
                voice.effective.lfo_amount_delta / 100.0
            } else {
                0.0
            })
        .clamp(-1.0, 1.0))
        .exp2()
    } else {
        1.0
    };
    let steps = if let Some((scales, fine_deltas)) = ctx.pitch_mod {
        let modulated = std::array::from_fn(|op| {
            let s = (base_steps[op] as f64) * scales[op] + fine_deltas[op];
            (s.round() as u64).clamp(1, (1_u64 << 63) - 1)
        });
        if let Some(ratio) = pitch_ratio {
            modulated
                .map(|step| (((step as f64) * ratio).round() as u64).clamp(1, (1_u64 << 63) - 1))
        } else {
            modulated
        }
    } else {
        static_steps
    };
    let age = if quality {
        voice.envelope_frame()
    } else {
        voice.age
    };
    let gate = if quality {
        voice.envelope_gate.unwrap_or(u64::MAX)
    } else {
        voice.gate.unwrap_or(u64::MAX)
    };
    let mut amplitudes = if factory.companion_active {
        if factory.uniform_companion {
            [voice.machines[0]
                .as_ref()
                .map_or(0.0, |machine| machine.level(age)); 4]
        } else if factory.partial_companion {
            partial_levels(&voice.machines, factory.companion_sources, age)
        } else {
            let independent = || {
                std::array::from_fn(|i| {
                    voice.machines[i]
                        .as_ref()
                        .map_or(0.0, |machine| machine.level(age))
                })
            };
            if factory.shared_loop_timing {
                companion::Machine::shared_loop_levels(&voice.machines, age)
                    .unwrap_or_else(independent)
            } else {
                independent()
            }
        }
    } else if factory.uniform_envelopes {
        factory.configuration.uniform_envelope_levels(age, gate)
    } else {
        factory.configuration.envelope_levels(age, gate)
    };
    if let Some(boost) = timbre_boost {
        for (op, amp) in amplitudes.iter_mut().enumerate() {
            if parameters[4 + op] == 0.0 {
                *amp *= boost;
            }
        }
    }
    let voice_filter_frame = if let Some(triplet) = ctx.automated_filter_triplet {
        if quality
            && voice.modulated_filter.is_some_and(|f| f.armed)
            && (voice.effective.cutoff_octaves != 0.0
                || voice.effective.resonance_delta != 0.0
                || voice.effective.morph_delta != 0.0)
        {
            if let Some((prev, frame)) = *voice_cached_filter
                && prev == triplet
            {
                Some(frame)
            } else {
                let freq = (f64::from(triplet[0]) * voice.effective.cutoff_octaves.exp2())
                    .clamp(20.0, 20_000.0);
                let res =
                    (f64::from(triplet[1]) + voice.effective.resonance_delta).clamp(0.0, 100.0);
                let morph = (f64::from(triplet[2]) + voice.effective.morph_delta).clamp(0.0, 100.0);
                factory
                    .settings
                    .filter
                    .frame_with_overrides(effective_filter_rate, freq, res, morph)
                    .inspect(|&frame| {
                        *voice_cached_filter = Some((triplet, frame));
                    })
                    .or(ctx.automated_filter_frame)
            }
        } else {
            ctx.automated_filter_frame
        }
    } else if quality {
        voice
            .modulated_filter
            .filter(|f| f.armed)
            .map(|f| f.frame())
    } else {
        ctx.filter_frame
    };
    let mut rendered = 0.0;
    for subframe in 0..if quality { 2 } else { 1 } {
        let (sample, previous) = factory.configuration.sample_with_waveform_sampler(
            &voice.note,
            amplitudes,
            &voice.phases,
            voice.previous,
            std::array::from_fn(|i| {
                (parameters[i]
                    * if quality {
                        voice.effective.level_gain[i]
                    } else {
                        1.0
                    })
                .clamp(0.0, 8.0)
            }),
            std::array::from_fn(|i| parameters[4 + i]),
            std::array::from_fn(|i| parameters[8 + i]),
            has_modulation,
            voice_filter_frame.map(|frame| (&mut voice.filters, frame)),
            |operator, phase, base_step| {
                spectrum.sample_policy(
                    operator,
                    phase,
                    voice.age,
                    || {
                        let step = steps[operator];
                        let render_step = match (law.active(), quality) {
                            (true, false) => step.saturating_mul(2),
                            (false, true) => step / 2,
                            _ => step,
                        };
                        (render_step, base_step)
                    },
                    factory.table,
                    factory.bank,
                    factory.phase_table,
                    !quality || factory.settings.global.interpolation,
                )
            },
        );
        let filtered = if quality {
            voice.decimator.sample(sample)
        } else {
            sample
        };
        rendered = if quality && factory.settings.global.antialias {
            filtered
        } else {
            sample
        };
        voice.previous = previous;
        if quality && !law.active() {
            for (phase, step) in voice.phases.iter_mut().zip(steps) {
                let step = step / 2 + if subframe == 1 { step % 2 } else { 0 };
                *phase = phase.wrapping_add(step);
            }
        } else {
            for (i, step) in steps.iter().enumerate() {
                let step = if law.active() && law.selected(i) {
                    (*step as f64 * multiplier).round() as u64
                } else {
                    *step
                };
                let step = if quality {
                    step / 2 + if subframe == 1 { step % 2 } else { 0 }
                } else {
                    step
                };
                voice.phases[i] = voice.phases[i].wrapping_add(step);
            }
        }
    }
    let pan_gains = if quality { voice.pan_gains } else { [1.0, 1.0] };
    let gain_scale = if quality {
        voice.effective.gain as f32
    } else {
        1.0
    };
    let left = rendered * pan_gains[0] * gain_scale;
    let right = rendered * pan_gains[1] * gain_scale;
    if law.active() {
        let next = voice.lfo_fraction
            + ctx.delta
                * if quality {
                    voice.lfo_rate_multiplier
                } else {
                    1.0
                };
        voice.lfo_cycle = voice.lfo_cycle.wrapping_add(next.floor() as u64);
        voice.lfo_fraction = next.fract();
    }
    if quality {
        voice.envelope_age = (voice.envelope_age + voice.effective.time_rate).min(u64::MAX as f64);
        if let Some(filter) = &mut voice.modulated_filter {
            filter.advance();
        }
    }
    voice.age = voice.age.saturating_add(1);
    if quality {
        if voice.envelope_gate.is_some() && EventVoice::read(voice, factory).envelope_finished() {
            voice.drain = voice.drain.saturating_add(1);
        } else {
            voice.drain = 0;
        }
    }
    [left, right]
}
