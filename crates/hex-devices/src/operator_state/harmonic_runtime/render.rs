use super::*;

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
        let mut may_expire = self
            .voices
            .iter()
            .any(|voice| voice.id != 0 && voice.gate.is_some());
        let companion = self.factory.settings.companion;
        let needs_companion_beat = companion.needs_clock();
        let has_modulation = self.factory.configuration.has_modulation();
        let needs_companion_transport = companion.needs_running();
        let base_targets = automation::base_target_values(&self.factory.settings);
        let rate_f64 = f64::from(self.factory.rate);
        let step_per_hz = (u64::MAX as f64 + 1.0) / rate_f64;
        let effective_filter_rate = if self.factory.settings.global.active {
            self.factory.rate.saturating_mul(2)
        } else {
            self.factory.rate
        };
        let mut cached_filter: Option<([f32; 3], crate::operator_filter::Frame)> = None;

        for (offset, frame) in frames.iter_mut().enumerate() {
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
            if self.factory.companion_active {
                for voice in &mut self.voices {
                    if voice.id != 0 {
                        for machine in voice.machines.iter_mut().flatten() {
                            let _ = machine.observe(
                                if self.factory.settings.global.active {
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
            let mut voices_changed = false;
            while let Some(event) = events.get(cursor) {
                let at = event.frame_offset();
                if at != offset {
                    break;
                }
                let origin = clock
                    .and_then(|clock| clock.performance_frame_at_offset(offset))
                    .unwrap_or(self.local_frame);
                let _ = self.apply(*event, origin, beat, running);
                voices_changed = true;
                may_expire = true;
                cursor += 1;
            }
            if voices_changed {
                voice_end = self
                    .voices
                    .iter()
                    .rposition(|voice| voice.id != 0)
                    .map_or(0, |i| i + 1);
            }
            let transition = (self.elapsed < self.frames).then(|| self.spectrum());
            let spectrum = transition.as_ref().unwrap_or(&self.factory.target);
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
                        fine_deltas[op] =
                            (f64::from(fine_hz).clamp(-1000.0, 1000.0) - base_fine) * step_per_hz;
                    }
                }
                Some((scales, fine_deltas))
            } else {
                None
            };
            let (automated_filter_triplet, automated_filter_frame) =
                if (mask & FILTER_MASK) != 0 && self.filter.armed {
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
                    (Some(triplet), frame)
                } else {
                    (None, None)
                };
            let filter_frame =
                automated_filter_frame.or_else(|| self.filter.armed.then(|| self.filter.frame()));
            let law = self.factory.settings.lfo;
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
            for voice in self.voices.iter_mut().take(voice_end) {
                if voice.id == 0 {
                    continue;
                }
                let phase = if law.retrigger() || self.factory.settings.global.active {
                    (voice.lfo_fraction, voice.lfo_cycle)
                } else {
                    self.phase
                };
                let multiplier = if law.active() {
                    ((if self.factory.settings.global.active {
                        self.factory.lfo_table.sample(
                            law.0[1],
                            phase.0,
                            phase.1,
                            self.factory.settings.global.interpolation,
                        )
                    } else {
                        lfo::waveform(law.0[1], phase.0, phase.1)
                    }) * (lfo_amount
                        + if self.factory.settings.global.active {
                            voice.effective.lfo_amount_delta / 100.0
                        } else {
                            0.0
                        })
                    .clamp(-1.0, 1.0))
                    .exp2()
                } else {
                    1.0
                };
                let quality = self.factory.settings.global.active;
                let base_steps = if quality {
                    voice.pitch_steps
                } else {
                    PreparedOperatorConfiguration::lfo_steps(&voice.note)
                };
                let steps = if let Some((scales, fine_deltas)) = pitch_mod {
                    std::array::from_fn(|op| {
                        let s = (base_steps[op] as f64) * scales[op] + fine_deltas[op];
                        (s.round() as u64).clamp(1, (1_u64 << 63) - 1)
                    })
                } else {
                    base_steps
                };
                let steps = if voice.pitch_offset_cents != 0
                    || (!quality && self.channel_pitch_bend != 0)
                {
                    let channel_cents = if quality {
                        0.0
                    } else {
                        f64::from(self.channel_pitch_bend)
                            / if self.channel_pitch_bend < 0 {
                                8192.0
                            } else {
                                8191.0
                            }
                            * 200.0
                    };
                    let ratio = 2.0_f64
                        .powf((f64::from(voice.pitch_offset_cents) + channel_cents) / 1200.0);
                    steps.map(|step| {
                        (((step as f64) * ratio).round() as u64).clamp(1, (1_u64 << 63) - 1)
                    })
                } else {
                    steps
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
                let mut amplitudes = if self.factory.companion_active {
                    if self.factory.uniform_companion {
                        [voice.machines[0]
                            .as_ref()
                            .map_or(0.0, |machine| machine.level(age)); 4]
                    } else if self.factory.partial_companion {
                        partial_levels(&voice.machines, self.factory.companion_sources, age)
                    } else {
                        let independent = || {
                            std::array::from_fn(|i| {
                                voice.machines[i]
                                    .as_ref()
                                    .map_or(0.0, |machine| machine.level(age))
                            })
                        };
                        if self.factory.shared_loop_timing {
                            companion::Machine::shared_loop_levels(&voice.machines, age)
                                .unwrap_or_else(independent)
                        } else {
                            independent()
                        }
                    }
                } else if self.factory.uniform_envelopes {
                    self.factory
                        .configuration
                        .uniform_envelope_levels(age, gate)
                } else {
                    self.factory.configuration.envelope_levels(age, gate)
                };
                // Unified MPE/CC74 timbre law (matching FM4): boosts FM modulator
                // depth (operators with carrier weight == 0.0) by up to +50% without
                // inflating carrier output gain.
                let voice_timbre = voice.timbre.max(self.cc_timbre);
                if voice_timbre != 0 {
                    let boost = 1.0 + 0.5 * (f64::from(voice_timbre) / 127.0);
                    for (op, amp) in amplitudes.iter_mut().enumerate() {
                        if parameters[4 + op] == 0.0 {
                            *amp *= boost;
                        }
                    }
                }
                let voice_filter_frame = if let Some(triplet) = automated_filter_triplet {
                    if quality
                        && voice.modulated_filter.is_some_and(|f| f.armed)
                        && (voice.effective.cutoff_octaves != 0.0
                            || voice.effective.resonance_delta != 0.0
                            || voice.effective.morph_delta != 0.0)
                    {
                        let freq = (f64::from(triplet[0]) * voice.effective.cutoff_octaves.exp2())
                            .clamp(20.0, 20_000.0);
                        let res = (f64::from(triplet[1]) + voice.effective.resonance_delta)
                            .clamp(0.0, 100.0);
                        let morph =
                            (f64::from(triplet[2]) + voice.effective.morph_delta).clamp(0.0, 100.0);
                        self.factory
                            .settings
                            .filter
                            .frame_with_overrides(effective_filter_rate, freq, res, morph)
                            .or(automated_filter_frame)
                    } else {
                        automated_filter_frame
                    }
                } else if quality {
                    voice
                        .modulated_filter
                        .filter(|f| f.armed)
                        .map(|f| f.frame())
                } else {
                    filter_frame
                };
                let mut rendered = 0.0;
                for subframe in 0..if quality { 2 } else { 1 } {
                    let (sample, previous) =
                        self.factory.configuration.sample_with_waveform_sampler(
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
                                    self.factory.table,
                                    self.factory.bank,
                                    self.factory.phase_table,
                                    !quality || self.factory.settings.global.interpolation,
                                )
                            },
                        );
                    let filtered = if quality {
                        voice.decimator.sample(sample)
                    } else {
                        sample
                    };
                    rendered = if quality && self.factory.settings.global.antialias {
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
                let left = rendered
                    * pan_gains[0]
                    * if quality {
                        voice.effective.gain as f32
                    } else {
                        1.0
                    };
                let right = rendered
                    * pan_gains[1]
                    * if quality {
                        voice.effective.gain as f32
                    } else {
                        1.0
                    };
                frame[0] += super::occurrence_gain::sample(left, voice.id, offset, gains);
                frame[1] += super::occurrence_gain::sample(right, voice.id, offset, gains);
                if law.active() {
                    let next = voice.lfo_fraction
                        + delta
                            * if quality {
                                voice.lfo_rate_multiplier
                            } else {
                                1.0
                            };
                    voice.lfo_cycle = voice.lfo_cycle.wrapping_add(next.floor() as u64);
                    voice.lfo_fraction = next.fract();
                }
                if quality {
                    voice.envelope_age =
                        (voice.envelope_age + voice.effective.time_rate).min(u64::MAX as f64);
                    if let Some(filter) = &mut voice.modulated_filter {
                        filter.advance();
                    }
                }
                voice.age = voice.age.saturating_add(1);
                if quality {
                    if voice.envelope_gate.is_some()
                        && EventVoice::read(voice, &self.factory).envelope_finished()
                    {
                        voice.drain = voice.drain.saturating_add(1);
                    } else {
                        voice.drain = 0;
                    }
                }
            }
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
            if may_expire {
                self.expire();
            }
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
