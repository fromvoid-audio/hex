//! Shared typed targets reuse EQ8's existing designer, smoother and routing.
//! A fixed Copy model retains every bank. Derived coefficient targets are staged
//! before controls install; immutable canonical coefficient Arcs never change.
use super::*;
use crate::parameter_binding::{BOOLEAN_ENVELOPE, Binding, CHOICE_ENVELOPE, SCALAR_ENVELOPE};
pub(super) const COUNT: usize = 201;
pub(super) const GAIN_SLOT: usize = COUNT - 1;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ControlValues {
    pub gain: f32,
    pub banks: [[Band; BAND_COUNT]; 5],
}
impl ControlValues {
    pub(super) fn new(settings: Settings) -> Self {
        Self {
            gain: settings.output_gain,
            banks: settings.banks,
        }
    }
}
const SEMANTICS: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        ..SCALAR_ENVELOPE
    };
const LOG: hex_lattice_contract::ParameterSemantics = hex_lattice_contract::ParameterSemantics {
    normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
    ..SEMANTICS
};
const CHOICE: hex_lattice_contract::ParameterSemantics = hex_lattice_contract::ParameterSemantics {
    smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
    ..CHOICE_ENVELOPE
};
const BOOLEAN: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        ..BOOLEAN_ENVELOPE
    };
macro_rules! band_bindings {
    ($(($bank:literal, $band:literal)),* $(,)?) => {
        const BINDINGS: [Binding<ControlValues>; COUNT] = [
            $(Binding::boolean(
                DESCRIPTORS[$bank * 40 + $band * 5], BOOLEAN,
                |s| s.banks[$bank][$band].enabled,
                |s, v| s.banks[$bank][$band].enabled = v,
            ),
            Binding::choice(
                DESCRIPTORS[$bank * 40 + $band * 5 + 1], CHOICE,
                |s| u32::from(s.banks[$bank][$band].kind),
                |s, v| {
                    s.banks[$bank][$band].kind = u8::try_from(v)
                        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
                    Ok(())
                },
            ),
            Binding::float(
                DESCRIPTORS[$bank * 40 + $band * 5 + 2], LOG,
                |s| s.banks[$bank][$band].frequency_hz,
                |s, v| s.banks[$bank][$band].frequency_hz = v,
            ),
            Binding::float(
                DESCRIPTORS[$bank * 40 + $band * 5 + 3], SEMANTICS,
                |s| s.banks[$bank][$band].gain_db,
                |s, v| s.banks[$bank][$band].gain_db = v,
            ),
            Binding::float(
                DESCRIPTORS[$bank * 40 + $band * 5 + 4], LOG,
                |s| s.banks[$bank][$band].q,
                |s, v| s.banks[$bank][$band].q = v,
            ),)*
            Binding::float(DESCRIPTORS[201], SEMANTICS, |s| s.gain, |s, v| s.gain = v),
        ];
    };
}
band_bindings!(
    (0, 0),
    (0, 1),
    (0, 2),
    (0, 3),
    (0, 4),
    (0, 5),
    (0, 6),
    (0, 7),
    (1, 0),
    (1, 1),
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
    (1, 7),
    (2, 0),
    (2, 1),
    (2, 2),
    (2, 3),
    (2, 4),
    (2, 5),
    (2, 6),
    (2, 7),
    (3, 0),
    (3, 1),
    (3, 2),
    (3, 3),
    (3, 4),
    (3, 5),
    (3, 6),
    (3, 7),
    (4, 0),
    (4, 1),
    (4, 2),
    (4, 3),
    (4, 4),
    (4, 5),
    (4, 6),
    (4, 7)
);
pub(super) fn slot(id: ParameterId) -> Option<usize> {
    BINDINGS.binary_search_by_key(&id, |b| b.descriptor.id).ok()
}
pub(super) fn semantics(id: ParameterId) -> Option<hex_lattice_contract::ParameterSemantics> {
    slot(id).map(|i| BINDINGS[i].semantics)
}
pub(super) fn descriptor(id: ParameterId) -> Option<ParameterDescriptor> {
    slot(id).map(|i| BINDINGS[i].descriptor)
}
pub(super) fn prepare(
    state: &NativeDeviceState,
    rate: u32,
    ids: &[ParameterId],
) -> Result<PreparedEq8, DeviceContractError> {
    state.require_asset_free_processing()?;
    if ids.is_empty() || ids.windows(2).any(|p| p[0] >= p[1]) {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    let mut prepared = PreparedEq8::new(decode(state)?, rate)?;
    for &id in ids {
        let index = slot(id).ok_or(DeviceContractError::UnsupportedProcessingClass)?;
        prepared.admitted[index] = true;
    }
    Ok(prepared)
}
fn selected(mode: ChannelMode) -> [usize; 2] {
    match mode {
        ChannelMode::Stereo => [0, 0],
        ChannelMode::LeftRight => [1, 2],
        ChannelMode::MidSide => [3, 4],
    }
}
fn design(
    banks: &[[Band; BAND_COUNT]; 5],
    mode: ChannelMode,
    rate: u32,
) -> Result<[[[Coefficients; 4]; BAND_COUNT]; 2], DeviceContractError> {
    let [a, b] = selected(mode);
    let bank = |index: usize| -> Result<[[Coefficients; 4]; BAND_COUNT], DeviceContractError> {
        let mut out = [[Coefficients::IDENTITY; 4]; BAND_COUNT];
        for (slot, band) in out.iter_mut().zip(banks[index]) {
            if band.enabled {
                *slot = design_band(band, rate)?;
            }
        }
        Ok(out)
    };
    let left = bank(a)?;
    let right = if a == b { left } else { bank(b)? };
    Ok([left, right])
}
impl Eq8Runtime {
    pub(super) fn current_target(&self) -> &[[[Coefficients; 4]; BAND_COUNT]; 2] {
        self.automated_target
            .as_ref()
            .unwrap_or(self.target.as_ref())
    }
    fn enable_admitted(&self, bank: usize, band: usize) -> bool {
        slot(parameter::enabled(bank, band)).is_some_and(|i| self.controls.admitted(i))
    }
    pub(super) fn enabled_layout_compatible(&self, previous: &Self) -> bool {
        let base = self.controls.base();
        let old = previous.controls.base();
        selected(self.mode).into_iter().all(|bank| {
            (0..BAND_COUNT).all(|band| {
                base.banks[bank][band].enabled == old.banks[bank][band].enabled
                    || self.enable_admitted(bank, band)
                    || previous.enable_admitted(bank, band)
            })
        })
    }
    pub(super) fn inherit_enabled_bands(&mut self, previous: &Self) {
        for (channel, bank) in selected(self.mode).into_iter().enumerate() {
            for band in 0..BAND_COUNT {
                if self.enable_admitted(bank, band) || previous.enable_admitted(bank, band) {
                    self.enabled_bands[channel][band] = previous.enabled_bands[channel][band];
                }
            }
        }
    }
    pub(super) fn install_enabled_bands(&mut self, banks: &[[Band; BAND_COUNT]; 5]) {
        for (channel, bank) in selected(self.mode).into_iter().enumerate() {
            self.enabled_bands[channel] = std::array::from_fn(|i| banks[bank][i].enabled);
        }
        if self
            .audition_request
            .is_some_and(|s| !self.selection_is_enabled(s))
        {
            // Keep the existing outgoing audition and its history until its
            // ordinary fade reaches zero. New auditions validate the new mask.
            self.audition_request = None;
        }
    }
    pub(super) fn restore_canonical_controls(&mut self) {
        self.controls.reset();
        self.install_enabled_bands(&self.controls.base().banks);
        if let Some(previous) = self.automated_target.take() {
            if previous != *self.target {
                self.smooth_remaining = coefficient_smooth_frames(self.sample_rate);
            }
            self.refresh_active_bands();
        }
        self.band_targets_pending = self.band_automation_admitted;
        self.retarget_gain(f64::from(self.controls.effective().gain));
    }
    pub(super) fn retarget_gain(&mut self, gain: f64) {
        if self.target_gain != gain {
            self.target_gain = gain;
            self.smooth_remaining = coefficient_smooth_frames(self.sample_rate);
        }
    }
    pub(super) fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) = self
                .controls
                .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let next_values = next.effective();
                let old = self.controls.effective();
                let banks = selected(self.mode);
                let changed = banks
                    .into_iter()
                    .any(|bank| old.banks[bank] != next_values.banks[bank]);
                let target = if self.band_targets_pending || changed {
                    Some(design(&next_values.banks, self.mode, self.sample_rate)?)
                } else {
                    None
                };
                self.controls.install(next);
                self.install_enabled_bands(&next_values.banks);
                if let Some(target) = target {
                    if target != *self.current_target() {
                        self.smooth_remaining = coefficient_smooth_frames(self.sample_rate);
                    }
                    self.automated_target = Some(target);
                    self.refresh_active_bands();
                    self.band_targets_pending = false;
                }
                self.retarget_gain(f64::from(next_values.gain));
            }
            let input = *frame;
            let output = self.process_frame(input);
            if !bypassed {
                *frame = output;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeParameterAutomation};
    struct Values {
        ids: Vec<ParameterId>,
        value: Option<ParameterValue>,
        dense: bool,
        start: usize,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
            if self.dense {
                Some(ParameterValue::Float(
                    ((self.start + offset) % 65) as f32 / 16.0,
                ))
            } else {
                self.value
            }
        }
    }
    fn values(value: Option<f32>) -> Values {
        Values {
            ids: vec![parameter::OUTPUT_GAIN],
            value: value.map(ParameterValue::Float),
            dense: false,
            start: 0,
        }
    }
    fn room(state: &NativeDeviceState, rate: u32, admitted: bool) -> Eq8Runtime {
        let mut prepared = PreparedEq8::new(decode(state).unwrap(), rate).unwrap();
        prepared.admitted[GAIN_SLOT] = admitted;
        prepared.runtime()
    }
    fn signal(len: usize) -> Vec<StereoFrame> {
        (0..len)
            .map(|i| {
                if i % 31 < 7 {
                    [0.25, -0.125]
                } else {
                    [0.01, -0.02]
                }
            })
            .collect()
    }
    fn state(mode: u32, filtered: bool) -> NativeDeviceState {
        let mut state = EQ8_DEFINITION.default_state().unwrap();
        state = EQ8_DEFINITION
            .patch_parameter(&state, parameter::MODE, ParameterValue::Choice(mode))
            .unwrap();
        for bank in 0..5 {
            for band in 0..BAND_COUNT {
                state = EQ8_DEFINITION
                    .patch_parameter(
                        &state,
                        parameter::enabled(bank, band),
                        ParameterValue::Bool(filtered && band == 1),
                    )
                    .unwrap();
            }
            if filtered {
                state = EQ8_DEFINITION
                    .patch_parameter(&state, parameter::gain(bank, 1), ParameterValue::Float(6.0))
                    .unwrap();
            }
        }
        state
    }
    fn all_bands(mode: u32) -> NativeDeviceState {
        let mut base = state(mode, true);
        for bank in 0..5 {
            for band in 0..BAND_COUNT {
                base = EQ8_DEFINITION
                    .patch_parameter(
                        &base,
                        parameter::enabled(bank, band),
                        ParameterValue::Bool(true),
                    )
                    .unwrap();
                base = EQ8_DEFINITION
                    .patch_parameter(
                        &base,
                        parameter::gain(bank, band),
                        ParameterValue::Float(2.0),
                    )
                    .unwrap();
            }
        }
        base
    }
    #[test]
    fn every_band_float_matches_its_canonical_handoff_at_both_rates() {
        for rate in [8000, 96000] {
            for mode in 0..3 {
                let base = all_bands(mode);
                let mut source = room(&base, rate, false);
                source.process_block(&mut signal(513), &R);
                for binding in BINDINGS[..GAIN_SLOT]
                    .iter()
                    .filter(|b| matches!(b.descriptor.kind, ParameterKind::Float { .. }))
                {
                    let id = binding.descriptor.id;
                    let bank = ((id.value() - 1) / 40) as usize;
                    if !selected(source.mode).contains(&bank) {
                        continue;
                    }
                    binding.semantics.validate(binding.descriptor).unwrap();
                    let field = (id.value() - 1) % 5;
                    let value = match field {
                        2 => {
                            if EQ8_DEFINITION.read_parameter(&base, id).unwrap()
                                == ParameterValue::Float(1000.0)
                            {
                                1700.0
                            } else {
                                1000.0
                            }
                        }
                        3 => 6.0,
                        _ => 2.0,
                    };
                    let mut actual = prepare(&base, rate, &[id]).unwrap().runtime();
                    actual.inherit_history_from(&source);
                    let changed = EQ8_DEFINITION
                        .patch_parameter(&base, id, ParameterValue::Float(value))
                        .unwrap();
                    let mut reference = room(&changed, rate, false);
                    reference.inherit_history_from(&source);
                    let values = Values {
                        ids: vec![id],
                        value: Some(ParameterValue::Float(value)),
                        dense: false,
                        start: 0,
                    };
                    let mut output = signal(513);
                    let mut expected = output.clone();
                    actual
                        .process_automation(&mut output, &values, false)
                        .unwrap();
                    reference.process_block(&mut expected, &R);
                    assert_eq!(output, expected, "id {id:?} rate {rate}");
                }
            }
        }
    }
    struct BandDense {
        ids: Vec<ParameterId>,
        start: usize,
    }
    impl NativeParameterAutomation for BandDense {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            let f = ((self.start + offset) * 7 + id.value() as usize * 3) % 65;
            let t = f as f32 / 64.0;
            if id != parameter::OUTPUT_GAIN && (id.value() - 1).is_multiple_of(5) {
                return Some(ParameterValue::Bool(f.is_multiple_of(2)));
            }
            if id != parameter::OUTPUT_GAIN && (id.value() - 1) % 5 == 1 {
                return Some(ParameterValue::Choice((f % 8) as u32));
            }
            Some(ParameterValue::Float(if id == parameter::OUTPUT_GAIN {
                0.5 + t
            } else {
                match (id.value() - 1) % 5 {
                    2 => 200.0 + 1400.0 * t,
                    3 => -3.0 + 6.0 * t,
                    _ => 0.5 + 2.0 * t,
                }
            }))
        }
    }
    #[test]
    fn every_kind_choice_matches_canonical_handoffs_in_its_routing() {
        for rate in [8000, 96000] {
            for mode in 0..3 {
                let base = all_bands(mode);
                let mut source = room(&base, rate, false);
                source.process_block(&mut signal(513), &R);
                for binding in BINDINGS
                    .iter()
                    .filter(|b| matches!(b.descriptor.kind, ParameterKind::Choice { .. }))
                {
                    let id = binding.descriptor.id;
                    let bank = ((id.value() - 1) / 40) as usize;
                    assert!(
                        EQ8_DEFINITION
                            .supports_parameter_automation(&base, id)
                            .unwrap()
                    );
                    binding.semantics.validate(binding.descriptor).unwrap();
                    if !selected(source.mode).contains(&bank) {
                        continue;
                    }
                    for choice in 0..8 {
                        let value = ParameterValue::Choice(choice);
                        let authored = Values {
                            ids: vec![id],
                            value: Some(value),
                            dense: false,
                            start: 0,
                        };
                        let changed = EQ8_DEFINITION.patch_parameter(&base, id, value).unwrap();
                        let mut actual = prepare(&base, rate, &[id]).unwrap().runtime();
                        actual.inherit_history_from(&source);
                        let mut canonical = room(&changed, rate, false);
                        canonical.inherit_history_from(&source);
                        let mut out = signal(513);
                        let mut reference = out.clone();
                        actual
                            .process_automation(&mut out, &authored, false)
                            .unwrap();
                        canonical.process_block(&mut reference, &R);
                        assert_eq!(out, reference, "id {id:?} choice {choice} rate {rate}");
                    }
                }
            }
        }
    }
    #[test]
    fn every_enable_matches_canonical_filter_handoffs_in_both_directions() {
        for rate in [8000, 96000] {
            for mode in 0..3 {
                let all = all_bands(mode);
                for bank in selected(decode(&all).unwrap().mode) {
                    for band in 0..BAND_COUNT {
                        let id = parameter::enabled(bank, band);
                        let binding = &BINDINGS[slot(id).unwrap()];
                        binding.semantics.validate(binding.descriptor).unwrap();
                        for enabled in [false, true] {
                            let base = EQ8_DEFINITION
                                .patch_parameter(&all, id, ParameterValue::Bool(!enabled))
                                .unwrap();
                            let mut source = room(&base, rate, false);
                            source.process_block(&mut signal(513), &R);
                            let value = ParameterValue::Bool(enabled);
                            let authored = Values {
                                ids: vec![id],
                                value: Some(value),
                                dense: false,
                                start: 0,
                            };
                            let changed = EQ8_DEFINITION.patch_parameter(&base, id, value).unwrap();
                            let mut actual = prepare(&base, rate, &[id]).unwrap().runtime();
                            actual.inherit_history_from(&source);
                            let mut canonical = room(&changed, rate, false);
                            canonical.inherit_history_from(&source);
                            let mut out = signal(513);
                            let mut reference = out.clone();
                            actual
                                .process_automation(&mut out, &authored, false)
                                .unwrap();
                            canonical.process_block(&mut reference, &R);
                            assert_eq!(out, reference, "id {id:?} enabled {enabled} rate {rate}");
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn enable_handoffs_are_precise_and_audition_cancellation_keeps_its_fade() {
        for rate in [8000, 96000] {
            let base = state(0, false);
            let id = parameter::enabled(0, 1);
            let held = Values {
                ids: vec![id],
                value: Some(ParameterValue::Bool(true)),
                dense: false,
                start: 0,
            };
            let absent = Values {
                ids: vec![id],
                value: None,
                dense: false,
                start: 0,
            };
            let mut source = prepare(&base, rate, &[id]).unwrap().runtime();
            source
                .process_automation(&mut signal(1024), &held, false)
                .unwrap();
            let selection = Eq8AuditionSelection::new(1, 0).unwrap();
            source.set_audition_selection(Some(selection)).unwrap();
            source
                .process_automation(&mut signal(256), &held, false)
                .unwrap();
            let mut copied = prepare(&base, rate, &[id]).unwrap().runtime();
            assert!(copied.can_retarget_parameters_from(&source));
            copied.inherit_history_from(&source);
            assert_eq!(copied.audition_selection, Some(selection));
            copied.process_automation(&mut [], &absent, false).unwrap();
            assert!(copied.selection_is_enabled(selection));
            // Disabling cancels through the existing 256-frame audition fade;
            // bypass advances that history while preserving the dry input.
            let mut first = signal(17);
            let mut reference = first.clone();
            let dry = first.clone();
            copied
                .process_automation(&mut first, &absent, true)
                .unwrap();
            source
                .process_automation(&mut reference, &absent, false)
                .unwrap();
            assert_eq!(first, dry);
            assert_eq!(copied.audition_request, None);
            assert_eq!(copied.audition_selection, Some(selection));
            assert!(copied.audition_mix > 0.0 && copied.audition_mix < 1.0);
            assert!(copied.set_audition_selection(Some(selection)).is_err());
            let mut out = signal(239);
            let mut reference = out.clone();
            copied.process_automation(&mut out, &absent, false).unwrap();
            source
                .process_automation(&mut reference, &absent, false)
                .unwrap();
            assert_eq!(out, reference);
            assert_eq!(copied.audition_selection, None);
            assert_eq!(copied.audition_mix, 0.0);
            copied
                .process_automation(&mut signal(1024), &held, false)
                .unwrap();
            let mut live = prepare(&base, rate, &[id]).unwrap().runtime();
            live.inherit_history_from(&copied);
            copied.begin_output_tail();
            out = vec![[0.0; 2]; 64];
            reference = out.clone();
            copied.process_block(&mut out, &R);
            live.process_automation(&mut reference, &held, false)
                .unwrap();
            assert_eq!(out, reference);
            copied.reset();
            assert!(!copied.selection_is_enabled(selection));
            assert_eq!(copied.audition_selection, None);
            let changed = EQ8_DEFINITION
                .patch_parameter(&base, id, ParameterValue::Bool(true))
                .unwrap();
            let mut bound = prepare(&changed, rate, &[id]).unwrap().runtime();
            assert!(bound.can_retarget_parameters_from(&live));
            let mut plain = room(&changed, rate, false);
            assert!(!plain.can_retarget_parameters_from(&room(&base, rate, false)));
            assert!(plain.can_retarget_parameters_from(&live));
            plain.inherit_history_from(&live);
            let other = EQ8_DEFINITION
                .patch_parameter(
                    &changed,
                    parameter::enabled(0, 2),
                    ParameterValue::Bool(true),
                )
                .unwrap();
            bound = prepare(&other, rate, &[id]).unwrap().runtime();
            assert!(!bound.can_retarget_parameters_from(&live));
        }
    }
    #[test]
    fn dense_all_banks_and_canonical_base_changes_keep_exact_targets() {
        let ids: Vec<_> = BINDINGS.iter().map(|b| b.descriptor.id).collect();
        for rate in [8000, 96000] {
            for mode in 0..3 {
                let base = all_bands(mode);
                let mut values = BandDense {
                    ids: ids.clone(),
                    start: 0,
                };
                let mut actual = prepare(&base, rate, &ids).unwrap().runtime();
                let mut split = prepare(&base, rate, &ids).unwrap().runtime();
                let mut reference = room(&base, rate, false);
                let mut output = signal(65);
                let mut expected = output.clone();
                let mut parts = output.clone();
                actual
                    .process_automation(&mut output, &values, false)
                    .unwrap();
                for (i, block) in parts.chunks_mut(13).enumerate() {
                    values.start = i * 13;
                    split.process_automation(block, &values, false).unwrap();
                }
                assert_eq!(output, parts);
                values.start = 0;
                for (i, frame) in expected.iter_mut().enumerate() {
                    let mut changed = base.clone();
                    for &id in &ids {
                        changed = EQ8_DEFINITION
                            .patch_parameter(&changed, id, values.value_at(id, i).unwrap())
                            .unwrap();
                    }
                    let mut next = room(&changed, rate, false);
                    next.inherit_history_from(&reference);
                    next.process_block(std::slice::from_mut(frame), &R);
                    reference = next;
                }
                assert_eq!(output, expected);
                // A held authored value equal to the former base still overrides a
                // changed canonical field in the incoming factory's coefficient Arc.
                let id = parameter::frequency(selected(actual.mode)[0], 0);
                let old = EQ8_DEFINITION.read_parameter(&base, id).unwrap();
                let held = Values {
                    ids: vec![id],
                    value: Some(old),
                    dense: false,
                    start: 0,
                };
                let mut source = prepare(&base, rate, &[id]).unwrap().runtime();
                source
                    .process_automation(&mut signal(513), &held, false)
                    .unwrap();
                let changed = EQ8_DEFINITION
                    .patch_parameter(&base, id, ParameterValue::Float(1700.0))
                    .unwrap();
                let mut incoming = prepare(&changed, rate, &[id]).unwrap().runtime();
                incoming.inherit_history_from(&source);
                let mut stable = prepare(&base, rate, &[id]).unwrap().runtime();
                stable.inherit_history_from(&source);
                output = signal(128);
                expected = output.clone();
                incoming
                    .process_automation(&mut output, &held, false)
                    .unwrap();
                stable
                    .process_automation(&mut expected, &held, false)
                    .unwrap();
                assert_eq!(output, expected);
                let mut canonical = room(&changed, rate, false);
                canonical.inherit_history_from(&incoming);
                output = signal(128);
                expected = output.clone();
                incoming
                    .process_automation(&mut output, &values_none(id), false)
                    .unwrap();
                canonical.process_block(&mut expected, &R);
                assert_eq!(output, expected);
            }
        }
        fn values_none(id: ParameterId) -> Values {
            Values {
                ids: vec![id],
                value: None,
                dense: false,
                start: 0,
            }
        }
    }
    #[test]
    fn gain_held_dense_and_fallback_match_canonical_rooms_in_all_modes() {
        for rate in [8000, 96000] {
            for mode in 0..3 {
                let base = state(mode, true);
                let mut source = room(&base, rate, false);
                source.process_block(&mut signal(513), &R);
                for gain in [0.0, 0.25, 2.0, 16.0] {
                    let mut actual = room(&base, rate, true);
                    actual.inherit_history_from(&source);
                    let changed = EQ8_DEFINITION
                        .patch_parameter(&base, parameter::OUTPUT_GAIN, ParameterValue::Float(gain))
                        .unwrap();
                    let mut reference = room(&changed, rate, false);
                    reference.inherit_history_from(&source);
                    let values = values(Some(gain));
                    let mut output = signal(1024);
                    let mut expected = output.clone();
                    for block in output.chunks_mut(13) {
                        actual.process_automation(block, &values, false).unwrap();
                    }
                    reference.process_block(&mut expected, &R);
                    assert_eq!(output, expected);
                }
                let mut actual = room(&base, rate, true);
                actual.inherit_history_from(&source);
                let mut split = room(&base, rate, true);
                split.inherit_history_from(&source);
                let mut reference = source;
                let mut dense = values(Some(1.0));
                dense.dense = true;
                let mut output = signal(128);
                let mut expected = output.clone();
                let mut partitioned = output.clone();
                actual
                    .process_automation(&mut output, &dense, false)
                    .unwrap();
                for (i, block) in partitioned.chunks_mut(13).enumerate() {
                    dense.start = i * 13;
                    split.process_automation(block, &dense, false).unwrap();
                }
                assert_eq!(output, partitioned);
                dense.start = 0;
                for (i, frame) in expected.iter_mut().enumerate() {
                    let changed = EQ8_DEFINITION
                        .patch_parameter(
                            &base,
                            parameter::OUTPUT_GAIN,
                            dense.value_at(parameter::OUTPUT_GAIN, i).unwrap(),
                        )
                        .unwrap();
                    let mut next = room(&changed, rate, false);
                    next.inherit_history_from(&reference);
                    next.process_block(std::slice::from_mut(frame), &R);
                    reference = next;
                }
                assert_eq!(output, expected);
                let mut fallback = room(&base, rate, false);
                fallback.inherit_history_from(&reference);
                output = signal(1024);
                expected = output.clone();
                actual
                    .process_automation(&mut output, &values(None), false)
                    .unwrap();
                fallback.process_block(&mut expected, &R);
                assert_eq!(output, expected);
            }
        }
    }
    #[test]
    fn gain_history_bypass_empty_calls_tails_and_canonical_successors_preserve_audio() {
        for rate in [8000, 96000] {
            let base = state(0, true);
            let held = values(Some(2.0));
            let mut source = room(&base, rate, true);
            source
                .process_automation(&mut signal(1024), &held, false)
                .unwrap();
            let mut copied = room(&base, rate, true);
            copied.inherit_history_from(&source);
            let mut bypassed = room(&base, rate, true);
            bypassed.inherit_history_from(&source);
            let mut output = signal(17);
            let mut expected = output.clone();
            source
                .process_automation(&mut output, &held, false)
                .unwrap();
            copied
                .process_automation(&mut [], &values(Some(4.0)), false)
                .unwrap();
            copied
                .process_automation(&mut expected, &held, false)
                .unwrap();
            assert_eq!(output, expected);
            let mut dry = signal(17);
            let original = dry.clone();
            bypassed.process_automation(&mut dry, &held, true).unwrap();
            assert_eq!(dry, original);
            output = signal(13);
            expected = output.clone();
            source
                .process_automation(&mut output, &held, false)
                .unwrap();
            bypassed
                .process_automation(&mut expected, &held, false)
                .unwrap();
            assert_eq!(output, expected);
            let mut tail_reference = room(&base, rate, true);
            tail_reference.inherit_history_from(&source);
            source.begin_output_tail();
            output = vec![[0.0; 2]; 64];
            expected = output.clone();
            source.process_block(&mut output, &R);
            tail_reference
                .process_automation(&mut expected, &held, false)
                .unwrap();
            assert_eq!(output, expected);
            let changed = EQ8_DEFINITION
                .patch_parameter(&base, parameter::OUTPUT_GAIN, ParameterValue::Float(0.5))
                .unwrap();
            let mut canonical = room(&changed, rate, false);
            canonical.inherit_history_from(&source);
            let mut absent = room(&changed, rate, true);
            absent.inherit_history_from(&source);
            output = signal(1024);
            expected = output.clone();
            canonical.process_block(&mut output, &R);
            let empty = Values {
                ids: vec![],
                value: None,
                dense: false,
                start: 0,
            };
            absent
                .process_automation(&mut expected, &empty, false)
                .unwrap();
            assert_eq!(output, expected);
            canonical.begin_output_tail();
            absent.begin_output_tail();
            output.fill([0.0; 2]);
            expected.fill([0.0; 2]);
            canonical.process_block(&mut output, &R);
            absent.process_block(&mut expected, &R);
            assert_eq!(output, expected);
            absent.reset();
            let mut fresh = room(&changed, rate, true);
            output = signal(128);
            expected = output.clone();
            absent
                .process_automation(&mut output, &held, false)
                .unwrap();
            fresh
                .process_automation(&mut expected, &held, false)
                .unwrap();
            assert_eq!(output, expected);
        }
    }
    #[test]
    fn routing_fade_keeps_outgoing_gain_and_initializes_the_incoming_control_law() {
        for rate in [8000, 96000] {
            for gain in [None, Some(2.0)] {
                let base = state(0, false);
                let mut source = room(&base, rate, true);
                source
                    .process_automation(&mut signal(1024), &values(Some(2.0)), false)
                    .unwrap();
                let changed = EQ8_DEFINITION
                    .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(2))
                    .unwrap();
                let mut next = room(&changed, rate, true);
                assert!(!next.inherit_history_from(&source));
                let input = [0.125, -0.0625];
                let mut output = [input];
                next.process_automation(&mut output, &values(gain), false)
                    .unwrap();
                let frames = f64::from(coefficient_smooth_frames(rate));
                let incoming_gain = 1.0 + (gain.unwrap_or(1.0) as f64 - 1.0) / frames;
                for (c, &sample) in input.iter().enumerate() {
                    let incoming = (f64::from(sample) * incoming_gain) as f32;
                    let expected = (f64::from(incoming)
                        + (f64::from(sample) * 2.0 - f64::from(incoming)) * (frames - 1.0) / frames)
                        as f32;
                    assert!((output[0][c] - expected).abs() < 1e-8);
                }
            }
        }
    }
    #[test]
    fn absent_view_after_canonical_enable_reactivates_the_filter_immediately() {
        for rate in [8000, 96000] {
            let base = state(0, false);
            let id = parameter::frequency(0, 1);
            let held = Values {
                ids: vec![id],
                value: Some(ParameterValue::Float(700.0)),
                dense: false,
                start: 0,
            };
            let mut source = prepare(&base, rate, &[id]).unwrap().runtime();
            source
                .process_automation(&mut signal(513), &held, false)
                .unwrap();
            let colored = state(0, true);
            let mut actual = prepare(&colored, rate, &[id]).unwrap().runtime();
            actual.inherit_history_from(&source);
            let mut expected = room(&colored, rate, false);
            expected.inherit_history_from(&source);
            let mut out = signal(129);
            let mut reference = out.clone();
            actual.process_block(&mut out, &R);
            expected.process_block(&mut reference, &R);
            assert_eq!(out, reference, "rate {rate}");
        }
    }
    #[test]
    fn band_ranges_are_rejected_before_audio_or_targets_change() {
        let base = all_bands(0);
        for (id, value) in [
            (parameter::frequency(0, 0), ParameterValue::Float(0.0)),
            (parameter::q(0, 0), ParameterValue::Float(f32::NAN)),
            (parameter::gain(0, 0), ParameterValue::Float(25.0)),
            (parameter::kind(0, 0), ParameterValue::Choice(8)),
            (parameter::kind(0, 0), ParameterValue::Float(0.0)),
            (parameter::enabled(0, 0), ParameterValue::Float(0.0)),
        ] {
            let mut actual = prepare(&base, 8000, &[id]).unwrap().runtime();
            let source = Values {
                ids: vec![id],
                value: Some(value),
                dense: false,
                start: 0,
            };
            let mut output = [[0.25; 2]];
            assert!(
                actual
                    .process_automation(&mut output, &source, false)
                    .is_err()
            );
            assert_eq!(output, [[0.25; 2]]);
        }
    }
    #[test]
    fn gain_requires_admission_and_rejects_bad_values_without_touching_audio() {
        let base = state(0, false);
        let mut plain = room(&base, 8000, false);
        let held = values(Some(2.0));
        let mut output = [[0.25; 2]];
        assert!(plain.process_automation(&mut output, &held, false).is_err());
        assert_eq!(output, [[0.25; 2]]);
        for invalid in [
            ParameterValue::Bool(true),
            ParameterValue::Float(f32::NAN),
            ParameterValue::Float(f32::INFINITY),
            ParameterValue::Float(-1.0),
            ParameterValue::Float(17.0),
        ] {
            let mut actual = room(&base, 8000, true);
            let bad = Values {
                ids: held.ids.clone(),
                value: Some(invalid),
                dense: false,
                start: 0,
            };
            assert!(actual.process_automation(&mut output, &bad, false).is_err());
            assert_eq!(output, [[0.25; 2]]);
        }
        assert!(
            !EQ8_DEFINITION
                .supports_parameter_automation(&base, parameter::MODE)
                .unwrap()
        );
    }
    #[test]
    #[ignore = "release-only targeted delivery microbenchmark"]
    fn eq8_gain_delivery_cost() {
        use std::{hint::black_box, time::Instant};
        assert!(!black_box(cfg!(debug_assertions)), "run with --release");
        let state = state(0, false);
        for mode in 0..6 {
            let mut source = values(Some(2.0));
            source.dense = mode == 2;
            let band_source = BandDense {
                ids: BINDINGS.iter().map(|b| b.descriptor.id).collect(),
                start: 0,
            };
            let band_state = all_bands(1);
            let mut runtime = if mode >= 3 {
                prepare(&band_state, 48000, &band_source.ids)
                    .unwrap()
                    .runtime()
            } else {
                room(&state, 48000, mode != 0)
            };
            let mut block = [[0.125, -0.25]; 128];
            let mut run = || {
                for _ in 0..128 {
                    block.fill([0.125, -0.25]);
                    if mode == 0 || mode == 5 {
                        runtime.process_block(&mut block, &R);
                    } else if mode >= 3 {
                        if mode == 3 {
                            // Frame-independent source for the held program.
                            struct Held<'a>(&'a BandDense);
                            impl NativeParameterAutomation for Held<'_> {
                                fn parameter_ids(&self) -> &[ParameterId] {
                                    self.0.parameter_ids()
                                }
                                fn value_at(
                                    &self,
                                    id: ParameterId,
                                    _: usize,
                                ) -> Option<ParameterValue> {
                                    if id != parameter::OUTPUT_GAIN
                                        && (id.value() - 1).is_multiple_of(5)
                                    {
                                        Some(ParameterValue::Bool(true))
                                    } else {
                                        self.0.value_at(id, 0)
                                    }
                                }
                            }
                            runtime
                                .process_automation(&mut block, &Held(&band_source), false)
                                .unwrap();
                        } else {
                            runtime
                                .process_automation(&mut block, &band_source, false)
                                .unwrap();
                        }
                    } else {
                        runtime
                            .process_automation(&mut block, &source, false)
                            .unwrap();
                    }
                    black_box(&block);
                }
            };
            for _ in 0..4 {
                run();
            }
            let mut times = [0.0; 7];
            for time in &mut times {
                let start = Instant::now();
                run();
                *time = start.elapsed().as_nanos() as f64 / 16384.0;
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "mode={mode} rate=48000 block=128 frames=16384 warmup=4 repeats=7 median_ns_per_frame={:.3}",
                times[3]
            );
        }
    }
}
