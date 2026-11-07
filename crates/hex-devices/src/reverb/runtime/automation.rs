//! Borrowed output, delay and modulation controls. Authored values bypass
//! canonical edit ramps; room controls use the existing tank evaluation rate.
//! Input filter and shelf-frequency conversions are cached. Boolean overrides
//! switch existing paths without resetting their history or oscillator phase;
//! decay shaping uses the effective mode in the fixed feedback cache. Ring
//! capacity and declared tail bounds remain unchanged.
use super::*;
use crate::parameter_binding::{Binding, SCALAR_ENVELOPE};
use hex_lattice_contract::ParameterSemantics;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct AutomatedSettings {
    lo_cut: bool,
    hi_cut: bool,
    spin: bool,
    high: bool,
    low: bool,
    chorus: bool,
    freeze: bool,
    flat: bool,
    cut: bool,
    decay: f32,
    dry_wet: f32,
    lo_freq: f32,
    hi_freq: f32,
    predelay: f32,
    spin_amount: f32,
    spin_rate: f32,
    shape: f32,
    high_freq: f32,
    high_ratio: f32,
    low_freq: f32,
    low_ratio: f32,
    diffusion: f32,
    scale: f32,
    size: f32,
    chorus_amount: f32,
    chorus_rate: f32,
    density: u32,
    stereo: f32,
    reflect: f32,
    diffuse: f32,
    lo_q: f32,
    hi_q: f32,
}
impl AutomatedSettings {
    pub(super) fn new(settings: Settings) -> Self {
        Self {
            lo_cut: settings.on(p::LO_CUT),
            hi_cut: settings.on(p::HI_CUT),
            spin: settings.on(p::SPIN),
            high: settings.on(p::HIGH),
            low: settings.on(p::LOW),
            chorus: settings.on(p::CHORUS),
            freeze: settings.on(p::FREEZE),
            flat: settings.on(p::FLAT),
            cut: settings.on(p::CUT),
            decay: settings.get(p::DECAY),
            dry_wet: settings.get(p::DRY_WET),
            lo_freq: settings.get(p::LO_FREQ),
            hi_freq: settings.get(p::HI_FREQ),
            predelay: settings.get(p::PREDELAY),
            spin_amount: settings.get(p::SPIN_AMOUNT),
            spin_rate: settings.get(p::SPIN_RATE),
            shape: settings.get(p::SHAPE),
            high_freq: settings.get(p::HIGH_FREQ),
            high_ratio: settings.get(p::HIGH_RATIO),
            low_freq: settings.get(p::LOW_FREQ),
            low_ratio: settings.get(p::LOW_RATIO),
            diffusion: settings.get(p::DIFFUSION),
            scale: settings.get(p::SCALE),
            size: settings.get(p::SIZE),
            chorus_amount: settings.get(p::CHORUS_AMOUNT),
            chorus_rate: settings.get(p::CHORUS_RATE),
            density: settings.get(p::DENSITY) as u32,
            stereo: settings.get(p::STEREO),
            reflect: settings.get(p::REFLECT),
            diffuse: settings.get(p::DIFFUSE),
            lo_q: settings.get(p::LO_Q),
            hi_q: settings.get(p::HI_Q),
        }
    }
    pub(super) fn check_inline(&self) {
        let Self {
            lo_cut,
            hi_cut,
            spin,
            high,
            low,
            chorus,
            freeze,
            flat,
            cut,
            decay,
            dry_wet,
            lo_freq,
            hi_freq,
            predelay,
            spin_amount,
            spin_rate,
            shape,
            high_freq,
            high_ratio,
            low_freq,
            low_ratio,
            diffusion,
            scale,
            size,
            chorus_amount,
            chorus_rate,
            density,
            stereo,
            reflect,
            diffuse,
            lo_q,
            hi_q,
        } = self;
        let _: [&f32; 22] = [
            decay,
            dry_wet,
            lo_freq,
            hi_freq,
            predelay,
            spin_amount,
            spin_rate,
            shape,
            high_freq,
            high_ratio,
            low_freq,
            low_ratio,
            diffusion,
            scale,
            size,
            chorus_amount,
            chorus_rate,
            stereo,
            reflect,
            diffuse,
            lo_q,
            hi_q,
        ];
        let _: &bool = lo_cut;
        let _: &bool = hi_cut;
        let _: &bool = spin;
        let _: &bool = high;
        let _: &bool = low;
        let _: &bool = chorus;
        let _: &bool = freeze;
        let _: &bool = flat;
        let _: &bool = cut;
        let _: &u32 = density;
    }
}
const SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
    ..SCALAR_ENVELOPE
};
const LOG_SEMANTICS: ParameterSemantics = ParameterSemantics {
    normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
    ..SEMANTICS
};
const BOOLEAN_SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
    ..crate::parameter_binding::BOOLEAN_ENVELOPE
};
const CHOICE_SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
    ..crate::parameter_binding::CHOICE_ENVELOPE
};
pub(super) const FREEZE_INDEX: usize = 24;
const BINDINGS: [Binding<AutomatedSettings>; 32] = [
    Binding::float(
        crate::reverb::DESCRIPTORS[0],
        LOG_SEMANTICS,
        |s| s.decay,
        |s, v| s.decay = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[1],
        SEMANTICS,
        |s| s.dry_wet,
        |s, v| s.dry_wet = v,
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[2],
        BOOLEAN_SEMANTICS,
        |s| s.lo_cut,
        |s, v| s.lo_cut = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[3],
        LOG_SEMANTICS,
        |s| s.lo_freq,
        |s, v| s.lo_freq = v,
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[4],
        BOOLEAN_SEMANTICS,
        |s| s.hi_cut,
        |s, v| s.hi_cut = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[5],
        LOG_SEMANTICS,
        |s| s.hi_freq,
        |s, v| s.hi_freq = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[6],
        SEMANTICS,
        |s| s.predelay,
        |s, v| s.predelay = v,
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[7],
        BOOLEAN_SEMANTICS,
        |s| s.spin,
        |s, v| s.spin = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[8],
        SEMANTICS,
        |s| s.spin_amount,
        |s, v| s.spin_amount = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[9],
        LOG_SEMANTICS,
        |s| s.spin_rate,
        |s, v| s.spin_rate = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[10],
        SEMANTICS,
        |s| s.shape,
        |s, v| s.shape = v,
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[11],
        BOOLEAN_SEMANTICS,
        |s| s.high,
        |s, v| s.high = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[12],
        LOG_SEMANTICS,
        |s| s.high_freq,
        |s, v| s.high_freq = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[13],
        LOG_SEMANTICS,
        |s| s.high_ratio,
        |s, v| s.high_ratio = v,
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[14],
        BOOLEAN_SEMANTICS,
        |s| s.low,
        |s, v| s.low = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[15],
        LOG_SEMANTICS,
        |s| s.low_freq,
        |s, v| s.low_freq = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[16],
        LOG_SEMANTICS,
        |s| s.low_ratio,
        |s, v| s.low_ratio = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[17],
        SEMANTICS,
        |s| s.diffusion,
        |s, v| s.diffusion = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[18],
        SEMANTICS,
        |s| s.scale,
        |s, v| s.scale = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[19],
        LOG_SEMANTICS,
        |s| s.size,
        |s, v| s.size = v,
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[20],
        BOOLEAN_SEMANTICS,
        |s| s.chorus,
        |s, v| s.chorus = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[21],
        SEMANTICS,
        |s| s.chorus_amount,
        |s, v| s.chorus_amount = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[22],
        LOG_SEMANTICS,
        |s| s.chorus_rate,
        |s, v| s.chorus_rate = v,
    ),
    Binding::choice(
        crate::reverb::DESCRIPTORS[24],
        CHOICE_SEMANTICS,
        |s| s.density,
        |s, v| {
            s.density = v;
            Ok(())
        },
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[25],
        BOOLEAN_SEMANTICS,
        |s| s.freeze,
        |s, v| s.freeze = v,
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[26],
        BOOLEAN_SEMANTICS,
        |s| s.flat,
        |s, v| s.flat = v,
    ),
    Binding::boolean(
        crate::reverb::DESCRIPTORS[27],
        BOOLEAN_SEMANTICS,
        |s| s.cut,
        |s, v| s.cut = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[28],
        SEMANTICS,
        |s| s.stereo,
        |s, v| s.stereo = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[29],
        SEMANTICS,
        |s| s.reflect,
        |s, v| s.reflect = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[30],
        SEMANTICS,
        |s| s.diffuse,
        |s, v| s.diffuse = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[31],
        LOG_SEMANTICS,
        |s| s.lo_q,
        |s, v| s.lo_q = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[32],
        LOG_SEMANTICS,
        |s| s.hi_q,
        |s, v| s.hi_q = v,
    ),
];
pub(in crate::reverb) fn supports(id: ParameterId) -> bool {
    crate::parameter_binding::binding(&BINDINGS, id).is_ok()
}
pub(in crate::reverb) fn semantics(id: ParameterId) -> Option<ParameterSemantics> {
    crate::parameter_binding::binding(&BINDINGS, id)
        .ok()
        .map(|b| b.semantics)
}
pub(in crate::reverb) fn validate_ids(ids: &[ParameterId]) -> Result<(), DeviceContractError> {
    if ids.is_empty() || ids.windows(2).any(|p| p[0] >= p[1]) || ids.iter().any(|&id| !supports(id))
    {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    Ok(())
}
#[derive(Clone, Copy, Default)]
pub(super) struct AutomatedValues {
    pub lo_cut: Option<f32>,
    pub hi_cut: Option<f32>,
    pub spin: Option<f32>,
    pub high: Option<f32>,
    pub low: Option<f32>,
    pub chorus: Option<f32>,
    pub freeze: Option<f32>,
    pub flat: Option<f32>,
    pub cut: Option<f32>,
    pub decay: Option<f32>,
    pub dry_wet: Option<f32>,
    pub lo_g: Option<f32>,
    pub hi_g: Option<f32>,
    pub predelay: Option<f32>,
    pub spin_amount: Option<f32>,
    pub spin_rate: Option<f32>,
    pub shape: Option<f32>,
    pub high_alpha: Option<f32>,
    pub high_ratio: Option<f32>,
    pub low_alpha: Option<f32>,
    pub low_ratio: Option<f32>,
    pub diffusion: Option<f32>,
    pub scale: Option<f32>,
    pub size: Option<f32>,
    pub chorus_amount: Option<f32>,
    pub chorus_rate: Option<f32>,
    pub density: Option<f32>,
    pub stereo: Option<f32>,
    pub reflect: Option<f32>,
    pub diffuse: Option<f32>,
    pub lo_damping: Option<f32>,
    pub hi_damping: Option<f32>,
}
impl AutomatedValues {
    pub(super) fn check_inline(&self) {
        let Self {
            lo_cut,
            hi_cut,
            spin,
            high,
            low,
            chorus,
            freeze,
            flat,
            cut,
            decay,
            dry_wet,
            lo_g,
            hi_g,
            predelay,
            spin_amount,
            spin_rate,
            shape,
            high_alpha,
            high_ratio,
            low_alpha,
            low_ratio,
            diffusion,
            scale,
            size,
            chorus_amount,
            chorus_rate,
            density,
            stereo,
            reflect,
            diffuse,
            lo_damping,
            hi_damping,
        } = self;
        let _: [&Option<f32>; 32] = [
            lo_cut,
            hi_cut,
            spin,
            high,
            low,
            chorus,
            freeze,
            flat,
            cut,
            decay,
            dry_wet,
            lo_g,
            hi_g,
            predelay,
            spin_amount,
            spin_rate,
            shape,
            high_alpha,
            high_ratio,
            low_alpha,
            low_ratio,
            diffusion,
            scale,
            size,
            chorus_amount,
            chorus_rate,
            density,
            stereo,
            reflect,
            diffuse,
            lo_damping,
            hi_damping,
        ];
    }
}
// Held authored values reuse their conversions. Missing components deliberately
// remain absent so mixed canonical filter ramps keep their existing law.
fn converted(
    present: bool,
    value: f32,
    old: f32,
    previous: Option<f32>,
    convert: impl FnOnce(f32) -> f32,
) -> Option<f32> {
    present.then(|| {
        if value == old {
            previous.unwrap_or_else(|| convert(value))
        } else {
            convert(value)
        }
    })
}
impl ReverbRuntime {
    pub(super) fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn NativeParameterAutomation,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.automation_controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) =
                self.automation_controls
                    .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let settings = next.effective();
                let old = self.automation_controls.effective();
                // Installed presence distinguishes authored values from the
                // canonical ramp fallback without another producer query.
                self.automation_controls.install(next);
                let present = |index| self.automation_controls.has_value(index);
                let g = |hz: f32| {
                    (std::f32::consts::PI * hz.min(self.tank_rate * 0.45) / self.tank_rate).tan()
                };
                let output = AutomatedValues {
                    lo_cut: present(2).then_some(f32::from(settings.lo_cut)),
                    hi_cut: present(4).then_some(f32::from(settings.hi_cut)),
                    spin: present(7).then_some(f32::from(settings.spin)),
                    high: present(11).then_some(f32::from(settings.high)),
                    low: present(14).then_some(f32::from(settings.low)),
                    chorus: present(20).then_some(f32::from(settings.chorus)),
                    freeze: present(24).then_some(f32::from(settings.freeze)),
                    flat: present(25).then_some(f32::from(settings.flat)),
                    cut: present(26).then_some(f32::from(settings.cut)),
                    decay: present(0).then_some(settings.decay),
                    dry_wet: present(1).then_some(settings.dry_wet),
                    lo_g: converted(
                        present(3),
                        settings.lo_freq,
                        old.lo_freq,
                        self.automated_values.lo_g,
                        g,
                    ),
                    hi_g: converted(
                        present(5),
                        settings.hi_freq,
                        old.hi_freq,
                        self.automated_values.hi_g,
                        g,
                    ),
                    predelay: present(6).then_some(settings.predelay),
                    spin_amount: present(8).then_some(settings.spin_amount),
                    spin_rate: present(9).then_some(settings.spin_rate),
                    shape: present(10).then_some(settings.shape),
                    high_alpha: converted(
                        present(12),
                        settings.high_freq,
                        old.high_freq,
                        self.automated_values.high_alpha,
                        |v| alpha(v, self.tank_rate),
                    ),
                    high_ratio: present(13).then_some(settings.high_ratio),
                    low_alpha: converted(
                        present(15),
                        settings.low_freq,
                        old.low_freq,
                        self.automated_values.low_alpha,
                        |v| alpha(v, self.tank_rate),
                    ),
                    low_ratio: present(16).then_some(settings.low_ratio),
                    diffusion: present(17).then_some(settings.diffusion),
                    scale: present(18).then_some(settings.scale),
                    size: present(19).then_some(settings.size),
                    chorus_amount: present(21).then_some(settings.chorus_amount),
                    chorus_rate: present(22).then_some(settings.chorus_rate),
                    density: present(23).then_some(settings.density as f32),
                    stereo: present(27).then_some(settings.stereo),
                    reflect: converted(
                        present(28),
                        settings.reflect,
                        old.reflect,
                        self.automated_values.reflect,
                        |v| 10.0_f32.powf(v / 20.0),
                    ),
                    diffuse: converted(
                        present(29),
                        settings.diffuse,
                        old.diffuse,
                        self.automated_values.diffuse,
                        |v| 10.0_f32.powf(v / 20.0),
                    ),
                    lo_damping: converted(
                        present(30),
                        settings.lo_q,
                        old.lo_q,
                        self.automated_values.lo_damping,
                        |v| 1.0 / v,
                    ),
                    hi_damping: converted(
                        present(31),
                        settings.hi_q,
                        old.hi_q,
                        self.automated_values.hi_damping,
                        |v| 1.0 / v,
                    ),
                };
                self.silence_limit = silence_deadline_frames(
                    self.rate,
                    output.decay.unwrap_or(self.settings.get(p::DECAY)),
                );
                self.automated_values = output;
            }
            self.frame_outputs(frame, self.automated_values);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeDeviceDefinition};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    struct Values {
        ids: Vec<ParameterId>,
        values: Vec<Option<ParameterValue>>,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            self.ids
                .iter()
                .position(|&p| p == id)
                .and_then(|i| self.values[i])
        }
    }
    fn automated_prepared(
        rate: u32,
        settings: Settings,
    ) -> Result<PreparedReverb, DeviceContractError> {
        PreparedReverb::new(rate, settings).map(|p| p.with_automation(&[p::DECAY, p::FREEZE]))
    }
    fn enabled_settings() -> Settings {
        let mut settings = Settings::default_values();
        for id in [p::LO_CUT, p::HI_CUT, p::HIGH, p::LOW] {
            settings.0[crate::reverb::index(id).unwrap()] = 1.0;
        }
        settings
    }
    fn held_values() -> Values {
        let mut values: Vec<_> = [
            2.0, 100.0, 250.0, 3000.0, 50.0, 80.0, 2.0, 0.1, 2000.0, 1.4, 500.0, 1.5, 10.0, 90.0,
            150.0, 3.0, 5.0, 0.0, 0.0, -12.0, -12.0, 2.0, 1.0,
        ]
        .into_iter()
        .map(|v| Some(ParameterValue::Float(v)))
        .collect();
        values[17] = Some(ParameterValue::Choice(1));
        for (index, value) in [
            (2, false),
            (4, false),
            (7, false),
            (11, false),
            (14, false),
            (20, false),
        ] {
            values.insert(index, Some(ParameterValue::Bool(value)));
        }
        for (index, value) in [(24, false), (25, true), (26, true)] {
            values.insert(index, Some(ParameterValue::Bool(value)));
        }
        Values {
            ids: BINDINGS.iter().map(|b| b.descriptor.id).collect(),
            values,
        }
    }
    #[test]
    fn boolean_modes_match_canonical_settings_and_reject_wrong_types() {
        for rate in [8000, 96000] {
            for id in [
                p::LO_CUT,
                p::HI_CUT,
                p::SPIN,
                p::HIGH,
                p::LOW,
                p::CHORUS,
                p::FREEZE,
                p::FLAT,
                p::CUT,
            ] {
                let mut variants = Vec::new();
                for enabled in [false, true] {
                    let mut settings = enabled_settings();
                    settings.0[crate::reverb::index(p::CUT).unwrap()] = 0.0;
                    if matches!(id, p::FLAT | p::CUT) {
                        settings.0[crate::reverb::index(p::FREEZE).unwrap()] = 1.0;
                    }
                    settings.0[crate::reverb::index(p::PREDELAY).unwrap()] = 0.0;
                    settings.0[crate::reverb::index(p::SPIN_AMOUNT).unwrap()] = 100.0;
                    settings.0[crate::reverb::index(p::CHORUS_AMOUNT).unwrap()] = 4.0;
                    settings.0[crate::reverb::index(p::HIGH_RATIO).unwrap()] = 0.2;
                    settings.0[crate::reverb::index(p::LOW_RATIO).unwrap()] = 2.0;
                    settings.0[crate::reverb::index(id).unwrap()] = f32::from(!enabled);
                    let mut actual = automated_prepared(rate, settings).unwrap().runtime();
                    settings.0[crate::reverb::index(id).unwrap()] = f32::from(enabled);
                    let mut reference = automated_prepared(rate, settings).unwrap().runtime();
                    let values = Values {
                        ids: vec![id],
                        values: vec![Some(ParameterValue::Bool(enabled))],
                    };
                    let mut output = signal(8192);
                    let mut expected = output.clone();
                    actual.process_automation(&mut output, &values).unwrap();
                    reference.process_block(&mut expected, &R);
                    assert_eq!(output, expected, "{rate} {id:?} {enabled}");
                    variants.push(output);
                    let wrong = Values {
                        ids: vec![id],
                        values: vec![Some(ParameterValue::Float(1.0))],
                    };
                    assert!(actual.process_automation(&mut [[0.0; 2]], &wrong).is_err());
                }
                assert!(
                    variants[0]
                        .iter()
                        .zip(&variants[1])
                        .any(|(a, b)| (a[0] - b[0]).abs() > 1e-7),
                    "inaudible fixture {rate} {id:?}"
                );
            }
        }
    }
    fn signal(len: usize) -> Vec<StereoFrame> {
        (0..len)
            .map(|i| {
                if i % 97 < 31 {
                    [0.125, -0.25]
                } else {
                    [0.0; 2]
                }
            })
            .collect()
    }
    #[test]
    fn each_output_matches_static_pcm_at_host_and_downsampled_tank_rates() {
        let settings = enabled_settings();
        for rate in [8000, 96000] {
            let mut baseline = signal(8192);
            automated_prepared(rate, settings)
                .unwrap()
                .runtime()
                .process_block(&mut baseline, &R);
            for (id, value) in [
                (p::DRY_WET, 100.0),
                (p::DECAY, 2.0),
                (p::SIZE, 150.0),
                (p::HIGH_RATIO, 1.4),
                (p::LOW_RATIO, 1.5),
                (p::DIFFUSION, 10.0),
                (p::LO_FREQ, 250.0),
                (p::HI_FREQ, 3000.0),
                (p::HIGH_FREQ, 2000.0),
                (p::LOW_FREQ, 500.0),
                (p::LO_Q, 2.0),
                (p::HI_Q, 1.0),
                (p::PREDELAY, 50.0),
                (p::SPIN_AMOUNT, 80.0),
                (p::SPIN_RATE, 2.0),
                (p::SHAPE, 0.1),
                (p::SCALE, 90.0),
                (p::CHORUS_AMOUNT, 3.0),
                (p::CHORUS_RATE, 5.0),
                (p::STEREO, 0.0),
                (p::REFLECT, -12.0),
                (p::DIFFUSE, -12.0),
            ] {
                let input = Values {
                    ids: vec![id],
                    values: vec![Some(ParameterValue::Float(value))],
                };
                let mut changed = settings;
                changed.0[crate::reverb::index(id).unwrap()] = value;
                let mut actual = automated_prepared(rate, settings).unwrap().runtime();
                let mut reference = automated_prepared(rate, changed).unwrap().runtime();
                let mut output = signal(8192);
                let mut expected = output.clone();
                reference.process_block(&mut expected, &R);
                for block in output.chunks_mut(13) {
                    actual.process_automation(block, &input).unwrap();
                }
                assert_eq!(output, expected, "parameter {} at {rate}", id.value());
                if matches!(
                    id,
                    p::LO_FREQ
                        | p::HI_FREQ
                        | p::HIGH_FREQ
                        | p::LOW_FREQ
                        | p::LO_Q
                        | p::HI_Q
                        | p::DIFFUSION
                ) {
                    assert!(
                        output
                            .iter()
                            .zip(&baseline)
                            .any(|(a, b)| (a[0] - b[0]).abs() > 1e-8 || (a[1] - b[1]).abs() > 1e-8),
                        "filter {} must change enabled PCM at {rate}",
                        id.value()
                    );
                }
                assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
            }
        }
    }
    #[test]
    fn dense_controls_match_one_sample_canonical_edits_with_retained_phase() {
        struct Dense {
            ids: Vec<ParameterId>,
        }
        impl NativeParameterAutomation for Dense {
            fn parameter_ids(&self) -> &[ParameterId] {
                &self.ids
            }
            fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
                let binding = BINDINGS.iter().find(|b| b.descriptor.id == id)?;
                match binding.descriptor.kind {
                    hex_lattice_contract::ParameterKind::Float { min, max, .. } => {
                        let fraction = ((offset * 17 + id.value() as usize * 7) % 65) as f32 / 64.0;
                        Some(ParameterValue::Float(min + (max - min) * fraction))
                    }
                    hex_lattice_contract::ParameterKind::Choice { labels } => Some(
                        ParameterValue::Choice(((offset / 17) % labels.len()) as u32),
                    ),
                    hex_lattice_contract::ParameterKind::Bool => {
                        Some(ParameterValue::Bool((offset / 17).is_multiple_of(2)))
                    }
                }
            }
        }
        let values = Dense {
            ids: BINDINGS.iter().map(|b| b.descriptor.id).collect(),
        };
        for rate in [8000, 96000] {
            let mut settings = enabled_settings();
            settings.0[crate::reverb::index(p::SMOOTH).unwrap()] = 0.0;
            let prepared = automated_prepared(rate, settings).unwrap();
            let mut actual = prepared.runtime();
            actual.process_block(&mut signal(8193), &R);
            let mut reference = prepared.runtime();
            reference.copy_history_from(&actual);
            let mut output = signal(128);
            let mut expected = output.clone();
            actual.process_automation(&mut output, &values).unwrap();
            for (offset, frame) in expected.iter_mut().enumerate() {
                let mut changed = settings;
                for id in values.parameter_ids() {
                    let value = values.value_at(*id, offset).unwrap();
                    changed.0[crate::reverb::index(*id).unwrap()] =
                        crate::reverb::state::scalar(value);
                }
                // Public canonical handoff law with one-sample smoothing is
                // an independent delivery oracle; no authored cache is used.
                let mut next = automated_prepared(rate, changed).unwrap().runtime();
                next.copy_history_from(&reference);
                next.process_block(std::slice::from_mut(frame), &R);
                reference = next;
            }
            for (offset, (a, b)) in output.iter().zip(&expected).enumerate() {
                assert!(
                    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6,
                    "rate {rate} frame {offset}: {a:?} != {b:?}"
                );
            }
            assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
        }
    }
    #[test]
    fn decay_admission_reserves_deadline_and_canonical_render_drops_override() {
        use hex_lattice::PreparedNativeProcessor;
        use hex_lattice_contract::ParameterNormalization;
        let state = crate::reverb::REVERB_DEFINITION.default_state().unwrap();
        for rate in [8000, 96000] {
            let canonical =
                crate::reverb::ReverbDefinition::prepare_owned_room(&state, rate).unwrap();
            assert!(!canonical.supports_parameter_automation(p::DECAY));
            let mut unprepared = canonical.runtime();
            assert!(!unprepared.supports_parameter_automation(p::DECAY));
            let values = Values {
                ids: vec![p::DECAY],
                values: vec![Some(ParameterValue::Float(60.0))],
            };
            let mut frame = [[0.125; 2]];
            assert!(unprepared.process_automation(&mut frame, &values).is_err());
            assert_eq!(frame, [[0.125; 2]]);
            assert_eq!(
                canonical.output_tail(),
                hex_lattice::OutputTail::Finite {
                    max_tail_frames: u64::from(rate) * 8 + u64::from(rate).div_ceil(10)
                }
            );
            let admitted = crate::reverb::ReverbDefinition::prepare_owned_room_for_parameters(
                &state,
                rate,
                &[p::DECAY],
            )
            .unwrap();
            assert_eq!(
                admitted.output_tail(),
                hex_lattice::OutputTail::Finite {
                    max_tail_frames: u64::from(rate) * 181 + u64::from(rate).div_ceil(10)
                }
            );
            let mut runtime = admitted.runtime();
            runtime
                .process_automation(&mut signal(8193), &values)
                .unwrap();
            assert_eq!(runtime.silence_limit, u64::from(rate) * 181);
            let mut inherited = admitted.runtime();
            inherited.copy_history_from(&runtime);
            assert_eq!(inherited.silence_limit, runtime.silence_limit);
            // Existing real-render deadline regressions cover clock advance;
            // place this long admitted deadline at its last frame to verify
            // terminal clearing without rendering181seconds in a unit test.
            inherited.silent_frames = inherited.silence_limit - 1;
            inherited
                .process_automation(&mut [[0.0; 2]], &values)
                .unwrap();
            assert!(inherited.is_quiescent());
            runtime.process_block(&mut [[0.0; 2]], &R);
            assert_eq!(runtime.silence_limit, u64::from(rate) * 8);
        }
        for id in [p::DECAY, p::SIZE, p::HIGH_RATIO, p::LOW_RATIO] {
            assert_eq!(
                semantics(id).unwrap().normalization,
                ParameterNormalization::PositiveLogarithmic
            );
        }
    }
    #[test]
    fn freeze_admission_and_effective_clock_preserve_the_tail_promise() {
        use hex_lattice::PreparedNativeProcessor;
        let state = crate::reverb::REVERB_DEFINITION.default_state().unwrap();
        for rate in [8000, 96000] {
            let plain = crate::reverb::ReverbDefinition::prepare_owned_room(&state, rate).unwrap();
            let mut unprepared = plain.runtime();
            let mut values = Values {
                ids: vec![p::FREEZE],
                values: vec![Some(ParameterValue::Bool(true))],
            };
            assert!(!plain.supports_parameter_automation(p::FREEZE));
            assert!(!unprepared.supports_parameter_automation(p::FREEZE));
            let mut input = [[0.125; 2]];
            assert!(unprepared.process_automation(&mut input, &values).is_err());
            assert_eq!(input, [[0.125; 2]]);
            let admitted = crate::reverb::ReverbDefinition::prepare_owned_room_for_parameters(
                &state,
                rate,
                &[p::FREEZE],
            )
            .unwrap();
            assert_eq!(admitted.output_tail(), hex_lattice::OutputTail::Unbounded);
            let mut room = admitted.runtime();
            room.process_block(&mut signal(8193), &R);
            room.silent_frames = room.silence_limit - 1;
            room.process_automation(&mut [[0.0; 2]], &values).unwrap();
            assert!(!room.is_quiescent());
            assert_eq!(room.silent_frames, room.silence_limit - 1);
            room.begin_output_tail();
            room.process_block(&mut [[0.0; 2]; 17], &R);
            assert!(!room.is_quiescent());
            values.values[0] = Some(ParameterValue::Bool(false));
            room.process_automation(&mut [[0.0; 2]], &values).unwrap();
            assert!(room.is_quiescent());
            room.reset();
            assert_eq!(room.automated_values.freeze, None);
        }
    }
    #[test]
    fn authored_feedback_cache_tracks_canonical_ramps_and_reaches_static_law() {
        for rate in [8000, 96000] {
            let settings = enabled_settings();
            let mut old = automated_prepared(rate, settings).unwrap().runtime();
            old.process_block(&mut signal(8193), &R);
            let mut changed = settings;
            changed.0[crate::reverb::index(p::DECAY).unwrap()] = 0.4;
            changed.0[crate::reverb::index(p::HIGH).unwrap()] = 0.0;
            changed.0[crate::reverb::index(p::LOW).unwrap()] = 0.0;
            let prepared = automated_prepared(rate, changed).unwrap();
            let mut whole = prepared.runtime();
            whole.copy_history_from(&old);
            let mut split = prepared.runtime();
            split.copy_history_from(&old);
            let values = Values {
                ids: vec![p::HIGH_RATIO],
                values: vec![Some(ParameterValue::Float(0.2))],
            };
            let mut output = signal(4096);
            let mut expected = output.clone();
            whole.process_automation(&mut output, &values).unwrap();
            for block in expected.chunks_mut(13) {
                split.process_automation(block, &values).unwrap();
            }
            assert_eq!(output, expected);
            changed.0[crate::reverb::index(p::HIGH_RATIO).unwrap()] = 0.2;
            changed.0[crate::reverb::index(p::SMOOTH).unwrap()] = 0.0;
            let mut reference = automated_prepared(rate, changed).unwrap().runtime();
            reference.copy_history_from(&whole);
            output = signal(4096);
            expected = output.clone();
            whole.process_automation(&mut output, &values).unwrap();
            reference.process_block(&mut expected, &R);
            for (a, b) in output.iter().zip(&expected) {
                assert!((a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
            }
        }
    }
    #[test]
    fn partial_filter_automation_preserves_canonical_coefficient_ramps() {
        for rate in [8000, 96000] {
            for (authored, canonical, value) in [
                (p::LO_FREQ, p::LO_Q, 2.0),
                (p::LO_Q, p::LO_FREQ, 250.0),
                (p::HI_FREQ, p::HI_Q, 1.0),
                (p::HI_Q, p::HI_FREQ, 3000.0),
            ] {
                let settings = enabled_settings();
                let mut source = automated_prepared(rate, settings).unwrap().runtime();
                source.process_block(&mut signal(8193), &R);
                let mut expected_settings = settings;
                expected_settings.0[crate::reverb::index(canonical).unwrap()] = value;
                let mut actual_settings = expected_settings;
                actual_settings.0[crate::reverb::index(authored).unwrap()] =
                    if authored == p::LO_Q || authored == p::HI_Q {
                        3.0
                    } else {
                        1500.0
                    };
                let mut actual = automated_prepared(rate, actual_settings).unwrap().runtime();
                actual.copy_history_from(&source);
                let mut reference = automated_prepared(rate, expected_settings)
                    .unwrap()
                    .runtime();
                reference.copy_history_from(&source);
                let values = Values {
                    ids: vec![authored],
                    values: vec![Some(ParameterValue::Float(settings.get(authored)))],
                };
                let mut output = signal(2048);
                let mut expected = output.clone();
                for block in output.chunks_mut(13) {
                    actual.process_automation(block, &values).unwrap();
                }
                reference.process_block(&mut expected, &R);
                assert_eq!(
                    output,
                    expected,
                    "partial filter {} rate {rate}",
                    authored.value()
                );
            }
        }
    }
    #[test]
    fn density_choices_match_static_rooms_and_refuse_malformed_values() {
        for rate in [8000, 96000] {
            let settings = enabled_settings();
            for choice in 0..3 {
                let mut changed = settings;
                changed.0[crate::reverb::index(p::DENSITY).unwrap()] = choice as f32;
                let mut actual = automated_prepared(rate, settings).unwrap().runtime();
                let mut reference = automated_prepared(rate, changed).unwrap().runtime();
                let values = Values {
                    ids: vec![p::DENSITY],
                    values: vec![Some(ParameterValue::Choice(choice))],
                };
                let mut output = signal(8192);
                let mut expected = output.clone();
                actual.process_automation(&mut output, &values).unwrap();
                reference.process_block(&mut expected, &R);
                assert_eq!(output, expected, "density {choice} rate {rate}");
                for invalid in [ParameterValue::Choice(3), ParameterValue::Float(1.0)] {
                    let values = Values {
                        ids: vec![p::DENSITY],
                        values: vec![Some(invalid)],
                    };
                    let mut frame = [[0.125; 2]];
                    assert!(actual.process_automation(&mut frame, &values).is_err());
                    assert_eq!(frame, [[0.125; 2]]);
                }
            }
        }
    }
    #[test]
    fn partial_scatter_automation_tracks_canonical_ramps() {
        for rate in [8000, 96000] {
            for (authored, canonical, target, override_target) in [
                (p::DIFFUSION, p::DENSITY, 2.0, 10.0),
                (p::DENSITY, p::DIFFUSION, 10.0, 2.0),
            ] {
                let settings = enabled_settings();
                let mut source = automated_prepared(rate, settings).unwrap().runtime();
                source.process_block(&mut signal(8193), &R);
                let mut expected_settings = settings;
                expected_settings.0[crate::reverb::index(canonical).unwrap()] = target;
                let mut actual_settings = expected_settings;
                actual_settings.0[crate::reverb::index(authored).unwrap()] = override_target;
                let mut actual = automated_prepared(rate, actual_settings).unwrap().runtime();
                let mut reference = automated_prepared(rate, expected_settings)
                    .unwrap()
                    .runtime();
                actual.copy_history_from(&source);
                reference.copy_history_from(&source);
                let value = if authored == p::DENSITY {
                    ParameterValue::Choice(settings.get(authored) as u32)
                } else {
                    ParameterValue::Float(settings.get(authored))
                };
                let values = Values {
                    ids: vec![authored],
                    values: vec![Some(value)],
                };
                let mut output = signal(2048);
                let mut expected = output.clone();
                for block in output.chunks_mut(13) {
                    actual.process_automation(block, &values).unwrap();
                }
                reference.process_block(&mut expected, &R);
                assert_eq!(
                    output,
                    expected,
                    "partial scatter {} rate {rate}",
                    authored.value()
                );
            }
        }
    }
    #[test]
    fn supported_float_endpoints_match_static_rooms() {
        for binding in &BINDINGS {
            let hex_lattice_contract::ParameterKind::Float { min, max, .. } =
                binding.descriptor.kind
            else {
                continue; // density choices have their own typed PCM regression.
            };
            for value in [min, max] {
                let settings = enabled_settings();
                let mut changed = settings;
                changed.0[crate::reverb::index(binding.descriptor.id).unwrap()] = value;
                let mut actual = automated_prepared(8000, settings).unwrap().runtime();
                let mut reference = automated_prepared(8000, changed).unwrap().runtime();
                let values = Values {
                    ids: vec![binding.descriptor.id],
                    values: vec![Some(ParameterValue::Float(value))],
                };
                let mut output = signal(8192);
                let mut expected = output.clone();
                actual.process_automation(&mut output, &values).unwrap();
                reference.process_block(&mut expected, &R);
                assert_eq!(
                    output,
                    expected,
                    "{} endpoint {value}",
                    binding.descriptor.id.value()
                );
                assert!(
                    output
                        .iter()
                        .flatten()
                        .all(|v| v.is_finite() && v.abs() <= 16.0)
                );
            }
        }
    }
    #[test]
    fn malformed_atomic_delivery_history_bypass_reset_and_storage_are_preserved() {
        let settings = enabled_settings();
        let prepared = automated_prepared(8000, settings).unwrap();
        let mut source = prepared.runtime();
        let mut reference = prepared.runtime();
        let mut values = held_values();
        let filter = values.ids.iter().position(|&id| id == p::LO_FREQ).unwrap();
        values.values[filter] = Some(ParameterValue::Float(f32::NAN));
        let mut output = [[0.125; 2]; 17];
        assert!(source.process_automation(&mut output, &values).is_err());
        assert_eq!(output, [[0.125; 2]; 17]);
        values.values[filter] = Some(ParameterValue::Float(25.0));
        let mut expected = output;
        source.process_automation(&mut output, &values).unwrap();
        reference
            .process_automation(&mut expected, &values)
            .unwrap();
        assert_eq!(output, expected);
        let mut copied = prepared.runtime();
        copied.copy_history_from(&source);
        let mut bypassed = prepared.runtime();
        bypassed.copy_history_from(&source);
        output.fill([0.125; 2]);
        expected = output;
        source.process_automation(&mut output, &values).unwrap();
        copied.process_automation(&mut expected, &values).unwrap();
        assert_eq!(output, expected);
        expected.fill([0.125; 2]);
        bypassed
            .process_bypassed_block_with_automation(&mut expected, None, &values, &R)
            .unwrap();
        assert_eq!(expected, [[0.125; 2]; 17]);
        output.fill([0.125; 2]);
        source.process_automation(&mut output, &values).unwrap();
        bypassed.process_automation(&mut expected, &values).unwrap();
        assert_eq!(output, expected);
        copied.reset();
        reference.reset();
        let absent = Values {
            ids: vec![],
            values: vec![],
        };
        output.fill([0.125; 2]);
        expected = output;
        copied.process_automation(&mut output, &absent).unwrap();
        reference.process_block(&mut expected, &R);
        assert_eq!(output, expected);
        let storage = copied.control_storage().unwrap();
        assert_eq!(storage.inline_bytes, std::mem::size_of::<ReverbRuntime>());
        assert_eq!(storage.heap_bytes.len(), 14);
        assert!(validate_ids(&[p::SMOOTH]).is_err());
        assert!(validate_ids(&[p::STEREO, p::DRY_WET]).is_err());
        assert!(
            crate::reverb::REVERB_DEFINITION
                .supports_parameter_automation(
                    &crate::reverb::REVERB_DEFINITION.default_state().unwrap(),
                    p::REFLECT
                )
                .unwrap()
        );
    }
    #[test]
    fn clock_free_tail_does_not_resurrect_removed_automation() {
        for rate in [8000, 96000] {
            for frozen in [false, true] {
                let mut prepared = automated_prepared(rate, enabled_settings()).unwrap();
                let mut source = prepared.runtime();
                let mut values = held_values();
                // Fill beyond the authored 50ms predelay even at 96kHz.
                source
                    .process_automation(&mut signal(8193), &values)
                    .unwrap();
                if frozen {
                    values.values[values.ids.iter().position(|&id| id == p::FREEZE).unwrap()] =
                        Some(ParameterValue::Bool(true));
                    let mut settings = enabled_settings();
                    for id in [p::FREEZE, p::FLAT, p::CUT] {
                        settings.0[crate::reverb::index(id).unwrap()] = 1.0;
                    }
                    settings.0[crate::reverb::index(p::SMOOTH).unwrap()] = 0.0;
                    prepared = automated_prepared(rate, settings).unwrap();
                    let mut next = prepared.runtime();
                    next.copy_history_from(&source);
                    source = next;
                    source.process_automation(&mut [[0.0; 2]], &values).unwrap();
                }
                let mut reference = prepared.runtime();
                reference.copy_history_from(&source);
                source.begin_output_tail();
                let mut output = vec![[0.0; 2]; 513];
                let mut expected = output.clone();
                for block in output.chunks_mut(13) {
                    source.process_block(block, &R);
                }
                reference
                    .process_automation(&mut expected, &values)
                    .unwrap();
                for (index, (a, b)) in output.iter().zip(&expected).enumerate() {
                    for ch in 0..2 {
                        assert!(
                            (a[ch] - b[ch]).abs() < 1e-7,
                            "clock-free tail rate {rate} frame {index}: {a:?} != {b:?}"
                        );
                    }
                }
                assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
                // Empty canonical calls do not install a new audible control
                // state; a subsequent tail still freezes the last delivery.
                let mut empty = prepared.runtime();
                empty.copy_history_from(&source);
                let mut empty_reference = prepared.runtime();
                empty_reference.copy_history_from(&source);
                empty.process_block(&mut [], &R);
                empty.begin_output_tail();
                empty_reference.begin_output_tail();
                let mut empty_output = [[0.0; 2]; 17];
                let mut empty_expected = empty_output;
                empty.process_block(&mut empty_output, &R);
                empty_reference.process_block(&mut empty_expected, &R);
                assert_eq!(empty_output, empty_expected);
                // A new active graph inherits DSP history, not tail admission or
                // the detached graph's override mode after lane removal.
                let mut canonical = prepared.runtime();
                canonical.copy_history_from(&source);
                let mut canonical_reference = prepared.runtime();
                canonical_reference.copy_history_from(&reference);
                output.fill([0.0; 2]);
                expected.fill([0.0; 2]);
                canonical.process_block(&mut output, &R);
                let absent = Values {
                    ids: vec![],
                    values: vec![],
                };
                canonical_reference
                    .process_automation(&mut expected, &absent)
                    .unwrap();
                assert_eq!(output, expected);
                // Identical authored values must be sampled again after an
                // ordinary canonical render, even if their old cache matches.
                let mut resumed = prepared.runtime();
                resumed.copy_history_from(&canonical);
                let mut resumed_reference = prepared.runtime();
                resumed_reference.copy_history_from(&canonical_reference);
                let mut resumed_output = signal(128);
                let mut resumed_expected = resumed_output.clone();
                resumed
                    .process_automation(&mut resumed_output, &values)
                    .unwrap();
                resumed_reference
                    .process_automation(&mut resumed_expected, &values)
                    .unwrap();
                assert_eq!(
                    resumed_output, resumed_expected,
                    "resumed rate {rate} frozen {frozen}"
                );
                // Removing this now-canonical device must retain its current
                // controls, not resurrect the previously removed lanes.
                canonical.begin_output_tail();
                canonical_reference.begin_output_tail();
                output.fill([0.0; 2]);
                expected.fill([0.0; 2]);
                canonical.process_block(&mut output, &R);
                canonical_reference.process_block(&mut expected, &R);
                assert_eq!(
                    output, expected,
                    "canonical tail rate {rate} frozen {frozen}"
                );
                source.reset();
                output.fill([0.0; 2]);
                source.process_block(&mut output, &R);
                assert!(output.iter().all(|frame| *frame == [0.0; 2]));
            }
        }
    }
}

#[cfg(test)]
mod benchmark {
    use super::*;
    use std::{hint::black_box, time::Instant};
    struct Source {
        ids: Vec<ParameterId>,
        dense: bool,
    }
    impl NativeParameterAutomation for Source {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(
            &self,
            id: ParameterId,
            offset: usize,
        ) -> Option<hex_lattice_contract::ParameterValue> {
            let t = if self.dense {
                offset as f32 / 128.0
            } else {
                0.5
            };
            if [
                p::LO_CUT,
                p::HI_CUT,
                p::SPIN,
                p::HIGH,
                p::LOW,
                p::CHORUS,
                p::FREEZE,
                p::FLAT,
                p::CUT,
            ]
            .contains(&id)
            {
                return Some(hex_lattice_contract::ParameterValue::Bool(
                    !self.dense || (offset / 17).is_multiple_of(2),
                ));
            }
            if id == p::DENSITY {
                return Some(hex_lattice_contract::ParameterValue::Choice(
                    if self.dense {
                        ((offset / 17) % 3) as u32
                    } else {
                        1
                    },
                ));
            }
            Some(hex_lattice_contract::ParameterValue::Float(match id {
                p::DECAY => 0.1 + 59.9 * t,
                p::SIZE => 1.0 + 199.0 * t,
                p::HIGH_RATIO | p::LOW_RATIO => 0.1 + 1.9 * t,
                p::DRY_WET => 25.0 + 50.0 * t,
                p::LO_FREQ | p::HI_FREQ | p::HIGH_FREQ | p::LOW_FREQ => 20.0 + 19980.0 * t,
                p::LO_Q | p::HI_Q => 0.1 + 9.9 * t,
                p::PREDELAY => 250.0 * t,
                p::SPIN_AMOUNT => 100.0 * t,
                p::SPIN_RATE | p::CHORUS_RATE => 0.01 + 9.99 * t,
                p::SHAPE => t,
                p::SCALE | p::DIFFUSION => 100.0 * t,
                p::CHORUS_AMOUNT => 4.0 * t,
                p::STEREO => 120.0 * t,
                p::REFLECT => -12.0 + 12.0 * t,
                p::DIFFUSE => -6.0 * t,
                _ => return None,
            }))
        }
    }
    // The pre-adapter scalar delivery body, measured against the same current
    // DSP/ring implementation. This is a loop baseline, not an old-binary run.
    fn legacy(runtime: &mut ReverbRuntime, frames: &mut [StereoFrame], source: &Source) {
        for (offset, frame) in frames.iter_mut().enumerate() {
            let mut values = [None];
            source.values_at(offset, &mut values).unwrap();
            let wet = match values[0] {
                Some(hex_lattice_contract::ParameterValue::Float(v))
                    if (0.0..=100.0).contains(&v) =>
                {
                    Some(v)
                }
                None => None,
                _ => panic!("bounded benchmark source"),
            };
            runtime.frame(frame, wet);
        }
    }
    // The pre-fix ordinary process_block body, paired with the repaired path
    // in one run to reduce machine-load differences between source revisions.
    fn previous_canonical(runtime: &mut ReverbRuntime, frames: &mut [StereoFrame]) {
        for frame in frames {
            if runtime.tail_controls_frozen {
                runtime.frame_outputs(frame, runtime.automated_values);
            } else {
                runtime.frame(frame, None);
            }
        }
    }
    #[test]
    #[ignore = "release-only targeted delivery microbenchmark"]
    fn output_delivery_cost_against_legacy_scalar_loop() {
        assert!(!black_box(cfg!(debug_assertions)), "run with --release");
        for mode in 0..6 {
            let source = Source {
                ids: if mode < 2 {
                    vec![p::DRY_WET]
                } else {
                    BINDINGS.iter().map(|b| b.descriptor.id).collect()
                },
                dense: mode == 3,
            };
            let mut runtime = PreparedReverb::new(48000, Settings::default_values())
                .unwrap()
                .with_automation(&[p::DECAY, p::FREEZE])
                .runtime();
            let mut block = [[0.125, -0.25]; 128];
            let mut run = || {
                for _ in 0..128 {
                    block.fill([0.125, -0.25]);
                    match mode {
                        0 => legacy(&mut runtime, &mut block, &source),
                        1..=3 => runtime.process_automation(&mut block, &source).unwrap(),
                        4 => {
                            runtime.process_block(&mut block, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                        }
                        5 => previous_canonical(&mut runtime, &mut block),
                        _ => panic!("bounded benchmark mode"),
                    }
                    black_box(&block);
                }
            };
            for _ in 0..4 {
                run();
            }
            let mut timings = Vec::with_capacity(7);
            for _ in 0..7 {
                let start = Instant::now();
                run();
                timings.push(start.elapsed().as_nanos());
            }
            timings.sort_unstable();
            println!(
                "mode={mode} rate=48000 block=128 frames=16384 warmup=4 repeats=7 median_ns_per_frame={:.3}",
                timings[3] as f64 / 16384.0
            );
        }
    }
}
