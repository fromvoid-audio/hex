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
    time_scale: f64,
    tone_alpha: Option<f64>,
    volume_gain: f32,
    transpose_st: f64,
    spread_pct: f64,
    glide_decay: f64,
    pitch_env_amount_pct: f64,
    filter_env_amount_st: f64,
    shaper_drive_pct: f64,
    shaper_wet_pct: f64,
    filter_drive_db: f64,
    pan_override: Option<f32>,
    base_filter_triplet: [f32; 3],
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
            time_scale: 1.0,
            tone_alpha: None,
            volume_gain: 1.0,
            transpose_st: 0.0,
            spread_pct: 0.0,
            glide_decay: 0.0,
            pitch_env_amount_pct: 0.0,
            filter_env_amount_st: 0.0,
            shaper_drive_pct: 0.0,
            shaper_wet_pct: 100.0,
            filter_drive_db: 0.0,
            pan_override: None,
            base_filter_triplet: [1000.0, 0.0, 0.0],
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
        mask: u64,
        index: usize,
        offset: usize,
    ) -> Option<f32> {
        if (mask & (1_u64 << index)) == 0 {
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
        mask: u64,
    ) {
        const LEVEL_MASK: u64 = (1 << 1) | (1 << 3) | (1 << 5) | (1 << 7);
        const FEEDBACK_MASK: u64 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11);
        const PITCH_MASK: u64 = (1 << 0)
            | (1 << 2)
            | (1 << 4)
            | (1 << 6)
            | (1 << automation::FINE_OFFSET_BASE_IDX)
            | (1 << (automation::FINE_OFFSET_BASE_IDX + 1))
            | (1 << (automation::FINE_OFFSET_BASE_IDX + 2))
            | (1 << (automation::FINE_OFFSET_BASE_IDX + 3));
        const PAN_MASK: u64 = 1 << automation::PAN_IDX;
        const FILTER_MASK: u64 = (1 << automation::FILTER_FREQ_IDX)
            | (1 << automation::FILTER_RES_IDX)
            | (1 << automation::FILTER_MORPH_IDX);
        const TIME_MASK: u64 = 1 << automation::TIME_IDX;
        const TONE_MASK: u64 = 1 << automation::TONE_IDX;
        const VOLUME_MASK: u64 = 1 << automation::VOLUME_IDX;
        const TRANSPOSE_MASK: u64 = 1 << automation::TRANSPOSE_IDX;
        const SPREAD_MASK: u64 = 1 << automation::SPREAD_IDX;
        const GLIDE_TIME_MASK: u64 = 1 << automation::GLIDE_TIME_IDX;
        const PITCH_ENV_AMOUNT_MASK: u64 = 1 << automation::PITCH_ENV_AMOUNT_IDX;
        const FILTER_ENV_AMOUNT_MASK: u64 = 1 << automation::FILTER_ENV_AMOUNT_IDX;
        const SHAPER_DRIVE_MASK: u64 = 1 << automation::SHAPER_DRIVE_IDX;
        const SHAPER_DRY_WET_MASK: u64 = 1 << automation::SHAPER_DRY_WET_IDX;
        const FILTER_DRIVE_MASK: u64 = 1 << automation::FILTER_DRIVE_IDX;
        let base_time_scale = self.factory.settings.performance.time_rate();
        let base_tone_alpha = performance::tone_alpha_for(
            f64::from(self.factory.settings.performance.tone),
            self.factory.rate,
        );
        let base_volume_gain =
            performance::volume_gain_for(f64::from(self.factory.settings.performance.volume));
        let base_transpose_st = f64::from(self.factory.settings.pitch_perf.transpose);
        let base_spread_pct = f64::from(self.factory.settings.pitch_perf.spread);
        let base_glide_decay = pitch_perf::glide_decay_coeff(
            f64::from(self.factory.settings.pitch_perf.glide_time),
            self.factory.rate,
        );
        let base_pitch_env_amount_pct = f64::from(self.factory.settings.pitch_env.amount());
        let base_filter_env_amount_st = f64::from(self.factory.settings.filter_env.amount());
        let base_shaper_drive_pct = f64::from(self.factory.settings.filter_shaper.shaper_drive);
        let base_shaper_wet_pct = f64::from(self.factory.settings.filter_shaper.shaper_dry_wet);
        let base_filter_drive_db = f64::from(self.factory.settings.filter_shaper.filter_drive);
        let pitch_env_active = self.factory.settings.pitch_env.enabled();
        let filter_env_active =
            self.factory.settings.filter.armed && self.factory.settings.filter_env.amount() != 0.0;
        let lfo_env_active =
            self.factory.settings.lfo.active() && self.factory.settings.lfo_ext.env_enabled();
        let output_stage_active = base_tone_alpha.is_some()
            || base_volume_gain != 1.0
            || base_spread_pct > 0.0
            || (mask & (TONE_MASK | VOLUME_MASK | SPREAD_MASK)) != 0;

        let mut cursor = 0;
        let mut voice_end = self
            .voices
            .iter()
            .rposition(|voice| voice.id != 0)
            .map_or(0, |i| i + 1);
        let mut has_fading = self.fading.iter().any(|fade| fade.remaining > 0);
        let companion = self.factory.settings.companion;
        let companion_active = self.factory.companion_active;
        let needs_companion_beat = companion.needs_clock()
            || self.factory.settings.pitch_env.needs_clock()
            || self
                .factory
                .settings
                .filter_env
                .needs_clock(self.factory.settings.filter.armed)
            || self
                .factory
                .settings
                .lfo_ext
                .needs_clock(self.factory.settings.lfo.active());
        let has_modulation = self.factory.configuration.has_modulation();
        let needs_companion_transport = companion.needs_running()
            || self.factory.settings.pitch_env.needs_running()
            || self
                .factory
                .settings
                .filter_env
                .needs_running(self.factory.settings.filter.armed)
            || self
                .factory
                .settings
                .lfo_ext
                .needs_running(self.factory.settings.lfo.active());
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
                if companion_active || pitch_env_active || filter_env_active || lfo_env_active {
                    for voice in self.voices.iter_mut().take(voice_end) {
                        if voice.id != 0 {
                            let obs_age = if quality {
                                voice.envelope_age as u64
                            } else {
                                voice.age
                            };
                            if companion_active {
                                for machine in voice.machines.iter_mut().flatten() {
                                    let _ = machine.observe(obs_age, beat, running);
                                }
                            }
                            if let Some(pe) = &mut voice.pitch_env {
                                let _ = pe.observe(obs_age, beat, running);
                            }
                            if let Some(fe) = &mut voice.filter_env {
                                let _ = fe.observe(obs_age, beat, running);
                            }
                            if let Some(le) = &mut voice.lfo_env {
                                let _ = le.observe(obs_age, beat, running);
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
                        if let Some(fine_hz) = Self::read_target(
                            values,
                            mask,
                            automation::FINE_OFFSET_BASE_IDX + op,
                            offset,
                        ) {
                            let base_fine =
                                f64::from(base_targets[automation::FINE_OFFSET_BASE_IDX + op]);
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
                        let freq =
                            Self::read_target(values, mask, automation::FILTER_FREQ_IDX, offset)
                                .unwrap_or(base_targets[automation::FILTER_FREQ_IDX])
                                .clamp(20.0, 20_000.0);
                        let res =
                            Self::read_target(values, mask, automation::FILTER_RES_IDX, offset)
                                .unwrap_or(base_targets[automation::FILTER_RES_IDX])
                                .clamp(0.0, 100.0);
                        let morph =
                            Self::read_target(values, mask, automation::FILTER_MORPH_IDX, offset)
                                .unwrap_or(base_targets[automation::FILTER_MORPH_IDX])
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
                } else if self.factory.settings.lfo_ext.is_high() {
                    ((lfo_rate_hz * 100.0).min(0.45 * rate_f64)) / rate_f64
                } else if law.sync() || self.factory.settings.lfo_ext.is_sync() {
                    clock
                        .and_then(|c| c.beat_at_offset(offset).zip(c.beat_at_offset(offset + 1)))
                        .map_or(0.0, |(a, b)| {
                            b.saturating_sub(a) as f64 / 4_294_967_296.0 / law.beats()
                        })
                } else {
                    lfo_rate_hz / rate_f64
                };
                let time_scale = if (mask & TIME_MASK) != 0 {
                    Self::read_target(values, mask, automation::TIME_IDX, offset)
                        .map_or(base_time_scale, |v| {
                            performance::time_rate_for(f64::from(v))
                        })
                } else {
                    base_time_scale
                };
                let tone_alpha = if (mask & TONE_MASK) != 0 {
                    Self::read_target(values, mask, automation::TONE_IDX, offset)
                        .map_or(base_tone_alpha, |v| {
                            performance::tone_alpha_for(f64::from(v), self.factory.rate)
                        })
                } else {
                    base_tone_alpha
                };
                let volume_gain = if (mask & VOLUME_MASK) != 0 {
                    Self::read_target(values, mask, automation::VOLUME_IDX, offset)
                        .map_or(base_volume_gain, |v| {
                            performance::volume_gain_for(f64::from(v))
                        })
                } else {
                    base_volume_gain
                };
                let transpose_st = if (mask & TRANSPOSE_MASK) != 0 {
                    Self::read_target(values, mask, automation::TRANSPOSE_IDX, offset).map_or(
                        base_transpose_st,
                        |v| {
                            f64::from(v).clamp(
                                f64::from(pitch_perf::MIN_TRANSPOSE_ST),
                                f64::from(pitch_perf::MAX_TRANSPOSE_ST),
                            )
                        },
                    )
                } else {
                    base_transpose_st
                };
                let spread_pct = if (mask & SPREAD_MASK) != 0 {
                    Self::read_target(values, mask, automation::SPREAD_IDX, offset)
                        .map_or(base_spread_pct, |v| f64::from(v).clamp(0.0, 100.0))
                } else {
                    base_spread_pct
                };
                let glide_decay = if (mask & GLIDE_TIME_MASK) != 0 {
                    Self::read_target(values, mask, automation::GLIDE_TIME_IDX, offset)
                        .map_or(base_glide_decay, |v| {
                            pitch_perf::glide_decay_coeff(f64::from(v), self.factory.rate)
                        })
                } else {
                    base_glide_decay
                };
                let pitch_env_amount_pct = if (mask & PITCH_ENV_AMOUNT_MASK) != 0 {
                    Self::read_target(values, mask, automation::PITCH_ENV_AMOUNT_IDX, offset)
                        .map_or(base_pitch_env_amount_pct, |v| {
                            f64::from(v).clamp(-100.0, 100.0)
                        })
                } else {
                    base_pitch_env_amount_pct
                };
                let filter_env_amount_st = if (mask & FILTER_ENV_AMOUNT_MASK) != 0 {
                    Self::read_target(values, mask, automation::FILTER_ENV_AMOUNT_IDX, offset)
                        .map_or(base_filter_env_amount_st, |v| {
                            f64::from(v).clamp(-96.0, 96.0)
                        })
                } else {
                    base_filter_env_amount_st
                };
                let shaper_drive_pct = if (mask & SHAPER_DRIVE_MASK) != 0 {
                    Self::read_target(values, mask, automation::SHAPER_DRIVE_IDX, offset)
                        .map_or(base_shaper_drive_pct, |v| f64::from(v).clamp(0.0, 100.0))
                } else {
                    base_shaper_drive_pct
                };
                let shaper_wet_pct = if (mask & SHAPER_DRY_WET_MASK) != 0 {
                    Self::read_target(values, mask, automation::SHAPER_DRY_WET_IDX, offset)
                        .map_or(base_shaper_wet_pct, |v| f64::from(v).clamp(0.0, 100.0))
                } else {
                    base_shaper_wet_pct
                };
                let filter_drive_db = if (mask & FILTER_DRIVE_MASK) != 0 {
                    Self::read_target(values, mask, automation::FILTER_DRIVE_IDX, offset)
                        .map_or(base_filter_drive_db, |v| f64::from(v).clamp(0.0, 24.0))
                } else {
                    base_filter_drive_db
                };
                let pan_override = if (mask & PAN_MASK) != 0 {
                    Self::read_target(values, mask, automation::PAN_IDX, offset)
                        .map(|v| v.clamp(-50.0, 50.0))
                } else {
                    None
                };
                let base_filter_triplet = [
                    base_targets[automation::FILTER_FREQ_IDX],
                    base_targets[automation::FILTER_RES_IDX],
                    base_targets[automation::FILTER_MORPH_IDX],
                ];
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
                    time_scale,
                    tone_alpha,
                    volume_gain,
                    transpose_st,
                    spread_pct,
                    glide_decay,
                    pitch_env_amount_pct,
                    filter_env_amount_st,
                    shaper_drive_pct,
                    shaper_wet_pct,
                    filter_drive_db,
                    pan_override,
                    base_filter_triplet,
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
            let mut stage_mix = [[0.0_f32; 2]; SUB_BLOCK];
            let sub_frames: &mut [StereoFrame] = if output_stage_active {
                &mut stage_mix[..sub_len]
            } else {
                &mut frames[sub_start..sub_end]
            };
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
                    if (companion_active || pitch_env_active || filter_env_active || lfo_env_active)
                        && !(local == 0 && voices_changed)
                    {
                        let obs_age = if quality {
                            voice.envelope_age as u64
                        } else {
                            voice.age
                        };
                        if companion_active {
                            for machine in voice.machines.iter_mut().flatten() {
                                let _ =
                                    machine.observe(obs_age, sample_ctx.beat, sample_ctx.running);
                            }
                        }
                        if let Some(pe) = &mut voice.pitch_env {
                            let _ = pe.observe(obs_age, sample_ctx.beat, sample_ctx.running);
                        }
                        if let Some(fe) = &mut voice.filter_env {
                            let _ = fe.observe(obs_age, sample_ctx.beat, sample_ctx.running);
                        }
                        if let Some(le) = &mut voice.lfo_env {
                            let _ = le.observe(obs_age, sample_ctx.beat, sample_ctx.running);
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

            if output_stage_active {
                let dst = &mut frames[sub_start..sub_end];
                for ((out_frame, &mixed), sample_ctx) in dst
                    .iter_mut()
                    .zip(stage_mix[..sub_len].iter())
                    .zip(sub_slice.iter())
                {
                    let spread_out = self.spread_stage.process_sample(
                        mixed,
                        sample_ctx.spread_pct,
                        self.factory.rate,
                    );
                    let toned = self
                        .tone_filter
                        .process_sample(spread_out, sample_ctx.tone_alpha);
                    let gain = sample_ctx.volume_gain;
                    out_frame[0] += toned[0] * gain;
                    out_frame[1] += toned[1] * gain;
                }
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
    let age = if quality {
        voice.envelope_frame()
    } else {
        voice.age
    };
    let lfo_ext = factory.settings.lfo_ext;
    let phase = if law.retrigger() || quality {
        (voice.lfo_fraction, voice.lfo_cycle)
    } else {
        ctx.phase
    };
    let (lfo_signal, multiplier) = if law.active() {
        let wave = if quality {
            factory.lfo_table.sample(
                law.0[1],
                phase.0,
                phase.1,
                factory.settings.global.interpolation,
            )
        } else {
            lfo::waveform(law.0[1], phase.0, phase.1)
        };
        let raw_depth = (ctx.lfo_amount
            + if quality {
                voice.effective.lfo_amount_delta / 100.0
            } else {
                0.0
            })
        .clamp(-1.0, 1.0);
        let depth = if lfo_ext.is_neutral() {
            raw_depth
        } else {
            let vel_scale = lfo_ext.amount_vel_scale(voice.note.midi_origin().1);
            let env_scale = voice
                .lfo_env
                .as_ref()
                .map_or(1.0, |le| (le.value(age) / 100.0).clamp(0.0, 1.0));
            (raw_depth * vel_scale * env_scale).clamp(-1.0, 1.0)
        };
        let sig = wave * depth;
        (sig, sig.exp2())
    } else {
        (0.0, 1.0)
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
    let pitch_env_st = voice.pitch_env.as_ref().map_or(0.0, |pe| {
        factory.settings.pitch_env.semitone_offset(
            pe,
            age,
            voice.note.midi_origin().1,
            ctx.pitch_env_amount_pct,
        )
    });
    let pitch_perf_st = ctx.transpose_st + voice.glide_semitones;
    let steps = if pitch_perf_st != 0.0 || pitch_env_st != 0.0 {
        let keytrack = factory.settings.pitch_keytrack.values();
        let base_scaled = if pitch_perf_st != 0.0 {
            pitch_perf::scale_steps_by_semitones(steps, pitch_perf_st, keytrack)
        } else {
            steps
        };
        if pitch_env_st != 0.0 {
            let pe_settings = factory.settings.pitch_env;
            std::array::from_fn(|op| {
                if !pe_settings.dest_operator(op) {
                    return base_scaled[op];
                }
                let kt = f64::from(keytrack[op]);
                if kt <= 0.0 {
                    base_scaled[op]
                } else {
                    let ratio = (pitch_env_st * (kt / 100.0) / 12.0).exp2();
                    (((base_scaled[op] as f64) * ratio).round() as u64).clamp(1, (1_u64 << 63) - 1)
                }
            })
        } else {
            base_scaled
        }
    } else {
        steps
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
    if quality && voice.drain > 0 {
        amplitudes = [0.0; 4];
    } else if let Some(boost) = timbre_boost {
        for (op, amp) in amplitudes.iter_mut().enumerate() {
            if parameters[4 + op] == 0.0 {
                *amp *= boost;
            }
        }
    }
    let lfo_filter_oct = if law.active() && lfo_ext.dest_filter() {
        lfo_signal * 4.0
    } else {
        0.0
    };
    let filter_env_oct = voice.filter_env.as_ref().map_or(0.0, |fe| {
        filter_env::Settings::envelope_octaves(fe, age, ctx.filter_env_amount_st)
    }) + lfo_filter_oct;
    let voice_filter_frame = if filter_env_oct != 0.0 && factory.settings.filter.armed {
        let triplet = ctx
            .automated_filter_triplet
            .unwrap_or(ctx.base_filter_triplet);
        let mod_oct = if quality {
            voice.effective.cutoff_octaves
        } else {
            0.0
        };
        let freq =
            (f64::from(triplet[0]) * (mod_oct + filter_env_oct).exp2()).clamp(20.0, 20_000.0);
        let res = (f64::from(triplet[1])
            + if quality {
                voice.effective.resonance_delta
            } else {
                0.0
            })
        .clamp(0.0, 100.0);
        let morph = (f64::from(triplet[2])
            + if quality {
                voice.effective.morph_delta
            } else {
                0.0
            })
        .clamp(0.0, 100.0);
        factory
            .settings
            .filter
            .frame_with_overrides(effective_filter_rate, freq, res, morph)
            .or(ctx.filter_frame)
    } else if let Some(triplet) = ctx.automated_filter_triplet {
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
    let filter_enabled = factory.settings.filter.enabled();
    let filter_circuit = factory.settings.filter.circuit();
    let shaper_type = factory.settings.filter_shaper.shaper_type;
    let nonlinear_active = (filter_enabled && filter_circuit != 0) || shaper_type != 0;
    for subframe in 0..if quality { 2 } else { 1 } {
        let (mut sample, previous) = factory.configuration.sample_with_waveform_sampler(
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
        if nonlinear_active {
            sample = filter_shaper::process_subframe(
                sample,
                filter_enabled,
                filter_circuit,
                ctx.filter_drive_db,
                shaper_type,
                ctx.shaper_drive_pct,
                ctx.shaper_wet_pct,
            );
        }
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
    let pan_gains = if let Some(pan) = ctx.pan_override {
        let mut g = factory.settings.global;
        g.pan = (f64::from(pan)
            + if quality {
                voice.effective.pan_delta
            } else {
                0.0
            })
        .clamp(-50.0, 50.0) as f32;
        g.gains(voice.note.midi_origin().0, voice.sequence)
    } else if quality {
        voice.pan_gains
    } else {
        [1.0, 1.0]
    };
    let gain_scale = if quality {
        voice.effective.gain as f32
    } else {
        1.0
    };
    let left = rendered * pan_gains[0] * gain_scale;
    let right = rendered * pan_gains[1] * gain_scale;
    if law.active() {
        let pe_lfo_mult = if pitch_env_st != 0.0 && factory.settings.pitch_env.dest_lfo() {
            (pitch_env_st / 12.0).exp2().clamp(0.0625, 16.0)
        } else {
            1.0
        };
        let step_delta = (ctx.delta
            * pe_lfo_mult
            * if quality {
                voice.lfo_rate_multiplier
            } else {
                1.0
            })
        .clamp(0.0, 0.45);
        let next = voice.lfo_fraction + step_delta;
        voice.lfo_cycle = voice.lfo_cycle.wrapping_add(next.floor() as u64);
        voice.lfo_fraction = next.fract();
    }
    if quality {
        let rate = if ctx.time_scale == 1.0 {
            voice.effective.time_rate
        } else {
            (voice.effective.time_rate * ctx.time_scale).clamp(0.25, 16.0)
        };
        voice.envelope_age = (voice.envelope_age + rate).min(u64::MAX as f64);
        if let Some(filter) = &mut voice.modulated_filter {
            filter.advance();
        }
    }
    if voice.glide_semitones != 0.0 {
        voice.glide_semitones *= ctx.glide_decay;
        if voice.glide_semitones.abs() < 1e-4 {
            voice.glide_semitones = 0.0;
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
