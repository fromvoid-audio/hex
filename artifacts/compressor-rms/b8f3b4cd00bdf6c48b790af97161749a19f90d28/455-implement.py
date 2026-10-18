from pathlib import Path
p=Path('crates/hex-devices/src/compressor.rs');s=p.read_text()
def rep(a,b,n=1):
 global s
 assert s.count(a)==n,(a,s.count(a));s=s.replace(a,b)
rep('COMPRESSOR_STATE_VERSION: u32 = 2','COMPRESSOR_STATE_VERSION: u32 = 3');rep('STATE_BYTES: usize = 28','STATE_BYTES: usize = 32')
rep('const GAIN_SMOOTH_MS: f64 = 5.0;','const GAIN_SMOOTH_MS: f64 = 5.0;\nconst RMS_INTEGRATION_SECONDS: f64 = 0.010;')
rep('    pub const DRY_WET: ParameterId = ParameterId::from_static(7);','    pub const DRY_WET: ParameterId = ParameterId::from_static(7);\n    pub const DETECTOR: ParameterId = ParameterId::from_static(8);')
rep('];\n\n#[derive(Clone, Copy, Debug, PartialEq)]', '''    ParameterDescriptor {
        id: parameter::DETECTOR,
        label: "detector",
        kind: ParameterKind::Choice { labels: &["peak", "rms"] },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum Detector { Peak, Rms }
impl Detector {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value { 0 => Ok(Self::Peak), 1 => Ok(Self::Rms), _ => Err(DeviceContractError::InvalidParameterValue) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]''')
rep('    dry_wet: f32,','    dry_wet: f32,\n    detector: Detector,')
rep('            dry_wet: 1.0,','            dry_wet: 1.0,\n            detector: Detector::Peak,')
rep('        dry_wet: read(24),','        dry_wet: read(24),\n        detector: Detector::from_choice(u32::from_le_bytes(bytes[28..32].try_into().map_err(|_| DeviceContractError::MalformedState)?))?,')
rep('    NativeDeviceState::new(COMPRESSOR_TYPE_ID, COMPRESSOR_STATE_VERSION, payload)','    payload.extend_from_slice(&(settings.detector as u32).to_le_bytes());\n    NativeDeviceState::new(COMPRESSOR_TYPE_ID, COMPRESSOR_STATE_VERSION, payload)')
rep('        let value = match id {','        if id == parameter::DETECTOR { return Ok(ParameterValue::Choice(settings.detector as u32)); }\n        let value = match id {')
rep('        descriptor.validate(value)?;','''        descriptor.validate(value)?;
        if id == parameter::DETECTOR {
            let ParameterValue::Choice(value) = value else { return Err(DeviceContractError::InvalidParameterValue); };
            settings.detector = Detector::from_choice(value)?;
            return encode(settings);
        }''')
rep('        let gain_alpha = (-1.0 / (GAIN_SMOOTH_MS * samples_per_ms)).exp();','        let gain_alpha = (-1.0 / (GAIN_SMOOTH_MS * samples_per_ms)).exp();\n        let rms_alpha = (-1.0 / (RMS_INTEGRATION_SECONDS * f64::from(sample_rate))).exp();')
rep('            gain_alpha,','            gain_alpha,\n            rms_alpha,')
rep('    gain_alpha: f64,','    gain_alpha: f64,\n    rms_alpha: f64,',2)
rep('            gain_alpha: self.gain_alpha,','            gain_alpha: self.gain_alpha,\n            rms_alpha: self.rms_alpha,\n            energy: [0.0; 2],\n            detector_mix: f64::from(self.settings.detector == Detector::Rms),\n            detector_remaining: 0,')
rep('    envelope: f64,','    energy: [f64; 2],\n    detector_mix: f64,\n    detector_remaining: u32,\n    envelope: f64,')
rep('        self.envelope = 0.0;','        self.energy = [0.0; 2];\n        self.detector_mix = f64::from(self.settings.detector == Detector::Rms);\n        self.detector_remaining = 0;\n        self.envelope = 0.0;')
rep('        let coefficient = if peak > self.envelope {','''        // Independent mean-square histories stay warm even while Peak is selected.
        for (energy, input) in self.energy.iter_mut().zip([left, right]) {
            *energy = self.rms_alpha * *energy + (1.0 - self.rms_alpha) * input * input;
        }
        if self.detector_remaining > 0 {
            let target = f64::from(self.settings.detector == Detector::Rms);
            self.detector_mix += (target - self.detector_mix) / f64::from(self.detector_remaining);
            self.detector_remaining -= 1;
            if self.detector_remaining == 0 { self.detector_mix = target; }
        }
        // Exact Peak fast path retains the original amplitude-follower arithmetic.
        let detector = if self.detector_mix == 0.0 { peak } else {
            let rms = self.energy[0].max(self.energy[1]).sqrt();
            if self.detector_mix == 1.0 { rms } else { (1.0 - self.detector_mix) * peak + self.detector_mix * rms }
        };
        let coefficient = if detector > self.envelope {''')
rep('(1.0 - coefficient) * peak;','(1.0 - coefficient) * detector;')
rep('            self.envelope = previous.envelope;','''            self.energy = previous.energy;
            self.detector_mix = previous.detector_mix;
            self.detector_remaining = if self.settings.detector == previous.settings.detector {
                previous.detector_remaining
            } else if self.detector_mix != f64::from(self.settings.detector == Detector::Rms) {
                self.sample_rate.div_ceil(200)
            } else { 0 };
            self.envelope = previous.envelope;''')
p.write_text(s)
# A declaration-approved TEST fixture only.
p=Path('crates/hex-storage/src/files.rs');s=p.read_text();start=s.index('    fn opaque_compressor_state_survives_project_save_and_load()');end=s.index('    #[test]',start);before=s[start:end];after=before.replace('with_capacity(28)','with_capacity(32)').replace('        let track_snapshot =','        payload.extend_from_slice(&1_u32.to_le_bytes());\n        let track_snapshot =').replace('"hex.compressor"), 2, payload','"hex.compressor"), 3, payload').replace('state_version(), 2','state_version(), 3').replace('        assert_eq!(&device.state().payload()[24..28]', '        assert_eq!(&device.state().payload()[28..32], &1_u32.to_le_bytes());\n        assert_eq!(&device.state().payload()[24..28]');assert before!=after;s=s[:start]+after+s[end:];p.write_text(s)
