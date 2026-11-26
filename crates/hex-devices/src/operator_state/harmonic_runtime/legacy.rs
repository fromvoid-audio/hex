use super::*;

impl VoiceHistory for Runtime {
    fn voice(&self, index: usize) -> Option<Voice> {
        self.voices.get(index).copied()
    }
    fn admission_voice(&self, index: usize) -> Option<(u64, PreparedOperatorNote)> {
        self.voices.get(index).map(|voice| (voice.id, voice.note))
    }
}

pub(in super::super) struct Legacy<'a> {
    pub fine_offsets: [f32; 4],
    pub pitch_keytrack: [f32; 4],
    pub tuning: Option<&'a hex_lattice::TuningTable>,
    pub rate: u32,
    pub base: [u8; ENABLE_PAYLOAD_BYTES],
    pub voices: &'a dyn VoiceHistory,
    pub parameters: [f64; 12],
    pub ramp: Option<ParameterRamp>,
    pub phase: (f64, u64),
    pub source: Spectrum,
    pub target: Spectrum,
    pub elapsed: u32,
    pub frames: u32,
    pub filter: FilterTrajectory,
    pub companion: companion::Settings,
    pub local_frame: u64,
}

pub(in super::super) struct Admission<'a> {
    pub(super) fine_offsets: [f32; 4],
    pub(super) pitch_keytrack: [f32; 4],
    pub(super) tuning: Option<&'a hex_lattice::TuningTable>,
    pub(super) rate: u32,
    pub(super) base: [u8; ENABLE_PAYLOAD_BYTES],
    pub(super) voices: &'a dyn VoiceHistory,
    pub(super) companion: companion::Settings,
}

impl<'a> Admission<'a> {
    pub(in super::super) fn admission_voices(
        &self,
    ) -> impl Iterator<Item = (u64, PreparedOperatorNote)> + '_ {
        (0..VOICES).filter_map(|index| self.voices.admission_voice(index))
    }

    pub(in super::super) fn read(previous: &'a dyn MutableInstrumentRuntime) -> Option<Self> {
        if let Some(p) = previous.as_any().downcast_ref::<Runtime>() {
            return Some(Self {
                fine_offsets: p.factory.settings.fine.values(),
                pitch_keytrack: p.factory.settings.pitch_keytrack.values(),
                tuning: p.factory.tuning.as_ref(),
                rate: p.factory.rate,
                base: p.factory.base,
                voices: p,
                companion: p.factory.settings.companion,
            });
        }
        let p = Legacy::read(previous)?;
        Some(Self {
            fine_offsets: p.fine_offsets,
            pitch_keytrack: p.pitch_keytrack,
            tuning: p.tuning,
            rate: p.rate,
            base: p.base,
            voices: p.voices,
            companion: p.companion,
        })
    }
}
pub(in super::super) fn builtin_spectrum(base: &[u8; ENABLE_PAYLOAD_BYTES]) -> Option<Spectrum> {
    let mut out = Spectrum {
        coefficients: [[0.0; 64]; 4],
        builtin: [[0.0; crate::operator_waveform::BUILTIN_CHOICE_COUNT]; 4],
        builtin_active: [0; 4],
        custom_active: [false; 4],
    };
    for operator in 0..4 {
        let offset = LEGACY_PAYLOAD_BYTES + operator * 4;
        let shape = u32::from_le_bytes(base[offset..offset + 4].try_into().ok()?);
        OperatorWaveform::from_choice(shape)?;
        let weight = out.builtin[operator].get_mut(shape as usize)?;
        *weight = 1.0;
        out.builtin_active[operator] = 1 << shape;
    }
    Some(out)
}
pub(in super::super) fn parameter_words(p: crate::fm4::OperatorFeedbackParameters) -> [f64; 12] {
    std::array::from_fn(|i| match i {
        0..=3 => p.oscillator_levels()[i],
        4..=7 => p.carrier_weights()[i - 4],
        _ => p.feedback_turns()[i - 8],
    })
}
impl<'a> Legacy<'a> {
    pub(in super::super) fn voices(&self) -> impl Iterator<Item = Voice> + '_ {
        (0..VOICES).filter_map(|index| self.voices.voice(index))
    }
    pub(in super::super) fn read(previous: &'a dyn MutableInstrumentRuntime) -> Option<Self> {
        if let Some(p) = previous.as_any().downcast_ref::<Runtime>() {
            return Some(Self {
                fine_offsets: p.factory.settings.fine.values(),
                pitch_keytrack: p.factory.settings.pitch_keytrack.values(),
                tuning: p.factory.tuning.as_ref(),
                rate: p.factory.rate,
                base: p.factory.base,
                voices: p,
                parameters: p.factory.parameters(),
                ramp: p.parameters,
                phase: p.phase,
                source: p.source,
                target: p.factory.target,
                elapsed: p.elapsed,
                frames: p.frames,
                filter: p.filter,
                companion: p.factory.settings.companion,
                local_frame: p.local_frame,
            });
        }
        #[cfg(test)]
        {
            super::lfo_runtime::export_harmonic_history(previous)
                .or_else(|| super::feedback_runtime::export_harmonic_history(previous))
        }
        #[cfg(not(test))]
        {
            None
        }
    }
}
