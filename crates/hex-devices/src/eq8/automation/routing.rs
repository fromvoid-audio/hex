//! Bounded authored routing: one outgoing native fade and one latest target.
//! Each accepted sample updates the desired mode. An occupied fade completes
//! before the latest target starts, so the last change starts within one fade
//! and settles within two (16/3 ms per fade, scaled by rate). Intermediate modes
//! can be superseded while waiting; authored lanes are retained unchanged.
//!
//! Control prepares all five canonical coefficient templates. Both templates
//! and the one pending authored target are fixed Copy storage, with no new Arc,
//! allocation, queue, worker or signal-history branch. Plain fallback and frozen
//! tails therefore advance pending routing without fallible callback design.
use super::*;

type Target = CoefficientTarget;

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingRouting {
    mode: ChannelMode,
    target: Target,
    enabled: [[bool; BAND_COUNT]; 2],
}
impl PendingRouting {
    fn new(values: ControlValues, target: Target) -> Self {
        Self {
            mode: values.mode,
            target,
            enabled: selected(values.mode)
                .map(|bank| std::array::from_fn(|i| values.banks[bank][i].enabled)),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::eq8) struct RoutingAutomation {
    canonical: [CoefficientBank; 5],
    pending: Option<PendingRouting>,
}
impl RoutingAutomation {
    pub(in crate::eq8) fn new(canonical: [CoefficientBank; 5]) -> Self {
        Self {
            canonical,
            pending: None,
        }
    }
    pub(in crate::eq8) fn canonical_target(&self, mode: ChannelMode) -> [&CoefficientBank; 2] {
        selected(mode).map(|bank| &self.canonical[bank])
    }
    pub(in crate::eq8) fn effective_target<'a>(
        &'a self,
        automated: Option<&'a Target>,
        canonical_mode: ChannelMode,
    ) -> [&'a CoefficientBank; 2] {
        automated
            .map(channels)
            .unwrap_or_else(|| self.canonical_target(canonical_mode))
    }
    pub(super) fn stage(
        &self,
        values: ControlValues,
        current: ChannelMode,
        rate: u32,
    ) -> Result<Option<PendingRouting>, DeviceContractError> {
        if values.mode == current {
            Ok(None)
        } else {
            Ok(Some(PendingRouting::new(
                values,
                design(&values.banks, values.mode, rate)?,
            )))
        }
    }
    pub(super) fn install(&mut self, pending: Option<PendingRouting>) {
        self.pending = pending;
    }
    pub(super) fn restore_canonical(
        &mut self,
        base: ControlValues,
        current: ChannelMode,
    ) -> Option<Target> {
        self.pending = (base.mode != current)
            .then(|| PendingRouting::new(base, self.canonical_target(base.mode).map(|bank| *bank)));
        (base.mode != current).then(|| self.canonical_target(current).map(|bank| *bank))
    }
}

impl Eq8Runtime {
    pub(in crate::eq8) fn preserves_routing(&self, previous: &Self) -> bool {
        self.sample_rate == previous.sample_rate
            && self.controls.admitted(MODE_SLOT)
            && previous.controls.admitted(MODE_SLOT)
    }
    pub(in crate::eq8) fn inherit_routing(&mut self, previous: &Self) {
        if !self.preserves_routing(previous) {
            return;
        }
        // The incoming factory retains its own canonical templates. Actual
        // routing, active fade and cached desired target belong to playback.
        self.mode = previous.mode;
        self.install_enabled_bands(&self.controls.base().banks);
        self.routing.pending = previous.routing.pending;
    }
    pub(in crate::eq8) fn reset_routing(&mut self) {
        self.routing.pending = None;
    }
    pub(in crate::eq8) fn advance_routing(&mut self) {
        if self.mode_fade.is_active() {
            return;
        }
        let Some(pending) = self
            .routing
            .pending
            .take()
            .filter(|pending| pending.mode != self.mode)
        else {
            return;
        };
        let canonical = self
            .routing
            .canonical_target(pending.mode)
            .map(|bank| *bank);
        let outgoing = ModeFade::from_outgoing(self, self.sample_rate);
        self.mode = pending.mode;
        self.coefficients = canonical;
        self.automated_target = Some(pending.target);
        self.current_gain = f64::from(self.controls.base().gain);
        self.target_gain = f64::from(self.controls.effective().gain);
        self.smooth_remaining =
            if canonical != pending.target || self.current_gain != self.target_gain {
                coefficient_smooth_frames(self.sample_rate)
            } else {
                0
            };
        self.history = [[[[0.0; 2]; 4]; BAND_COUNT]; 2];
        self.clear_audition();
        self.enabled_bands = pending.enabled;
        self.mode_fade = outgoing;
        self.band_targets_pending = false;
        self.refresh_active_bands();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{BandDense, all_bands, room, signal};
    use super::*;
    use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeParameterAutomation};
    struct Values {
        ids: Vec<ParameterId>,
        mode: Option<u32>,
        gain: Option<f32>,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            if id == parameter::MODE {
                self.mode.map(ParameterValue::Choice)
            } else if id == parameter::OUTPUT_GAIN {
                self.gain.map(ParameterValue::Float)
            } else {
                Some(ParameterValue::Float(700.0))
            }
        }
    }
    fn mode(value: Option<u32>) -> Values {
        Values {
            ids: vec![parameter::MODE],
            mode: value,
            gain: None,
        }
    }
    fn colorful(mode: u32) -> NativeDeviceState {
        let mut state = all_bands(mode);
        for (bank, gain) in [3.0, -6.0, 8.0, -2.0, 6.0].into_iter().enumerate() {
            state = EQ8_DEFINITION
                .patch_parameter(
                    &state,
                    parameter::gain(bank, 0),
                    ParameterValue::Float(gain),
                )
                .unwrap();
            state = EQ8_DEFINITION
                .patch_parameter(
                    &state,
                    parameter::frequency(bank, 0),
                    ParameterValue::Float(200.0 + bank as f32 * 400.0),
                )
                .unwrap();
        }
        state
    }
    #[test]
    fn every_mode_handoff_matches_canonical_routing_with_authored_gain_and_banks() {
        for rate in [8000, 96000] {
            for from in 0..3 {
                let base = colorful(from);
                let mut source = room(&base, rate, false);
                source.process_block(&mut signal(1024), &R);
                for to in 0..3 {
                    let mut ids: Vec<_> =
                        (0..5).map(|bank| parameter::frequency(bank, 0)).collect();
                    ids.extend([parameter::MODE, parameter::OUTPUT_GAIN]);
                    let values = Values {
                        ids: ids.clone(),
                        mode: Some(to),
                        gain: Some(2.0),
                    };
                    let mut actual = prepare(&base, rate, &ids).unwrap().runtime();
                    actual.inherit_history_from(&source);
                    let changed = EQ8_DEFINITION
                        .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(to))
                        .unwrap();
                    ids.retain(|&id| id != parameter::MODE);
                    let mut canonical = prepare(&changed, rate, &ids).unwrap().runtime();
                    canonical.inherit_history_from(&source);
                    let reference_values = Values {
                        ids,
                        mode: None,
                        gain: Some(2.0),
                    };
                    let mut out = signal(2 * mode_fade_frames(rate) as usize + 97);
                    let mut reference = out.clone();
                    actual.process_automation(&mut out, &values, false).unwrap();
                    canonical
                        .process_automation(&mut reference, &reference_values, false)
                        .unwrap();
                    assert_eq!(out, reference, "rate {rate}, {from}->{to}");
                }
            }
        }
    }
    #[test]
    fn busy_routing_uses_the_latest_target_after_exactly_one_existing_fade() {
        for rate in [8000, 96000] {
            let base = colorful(0);
            let n = mode_fade_frames(rate) as usize;
            let mut source = room(&base, rate, false);
            source.process_block(&mut signal(1024), &R);
            let mut actual = prepare(&base, rate, &[parameter::MODE]).unwrap().runtime();
            actual.inherit_history_from(&source);
            let first = EQ8_DEFINITION
                .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(1))
                .unwrap();
            let mut reference = room(&first, rate, false);
            reference.inherit_history_from(&source);
            let mut out = signal(2 * n + 13);
            let mut expected = out.clone();
            for (i, frame) in out.iter_mut().enumerate() {
                let wanted = match i {
                    0 => 1,
                    1 => 0,
                    _ => 2,
                };
                actual
                    .process_automation(std::slice::from_mut(frame), &mode(Some(wanted)), false)
                    .unwrap();
                assert_eq!(
                    actual.mode,
                    if i < n {
                        ChannelMode::LeftRight
                    } else {
                        ChannelMode::MidSide
                    }
                );
            }
            reference.process_block(&mut expected[..n], &R);
            let last = EQ8_DEFINITION
                .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(2))
                .unwrap();
            let mut final_room = room(&last, rate, false);
            final_room.inherit_history_from(&reference);
            final_room.process_block(&mut expected[n..], &R);
            assert_eq!(out, expected);
            assert!(!actual.mode_fade.is_active());
            assert!(actual.routing.pending.is_none());
        }
    }
    #[test]
    fn copied_pending_routing_fallback_tail_bypass_and_reset_preserve_their_laws() {
        for rate in [8000, 96000] {
            let base = colorful(0);
            let n = mode_fade_frames(rate) as usize;
            let ids = [parameter::MODE];
            let mut source = prepare(&base, rate, &ids).unwrap().runtime();
            source
                .process_automation(&mut signal(3), &mode(Some(1)), false)
                .unwrap();
            source
                .process_automation(&mut signal(1), &mode(Some(2)), false)
                .unwrap();
            let changed = EQ8_DEFINITION
                .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(2))
                .unwrap();
            let mut copied = prepare(&changed, rate, &ids).unwrap().runtime();
            assert!(copied.can_retarget_parameters_from(&source));
            assert!(!copied.transition_busy_from(&source));
            copied.inherit_history_from(&source);
            assert_eq!(copied.mode, ChannelMode::LeftRight);
            copied
                .process_automation(&mut [], &mode(Some(0)), false)
                .unwrap();
            let mut out = signal(31);
            let mut reference = out.clone();
            source
                .process_automation(&mut out, &mode(Some(2)), false)
                .unwrap();
            copied
                .process_automation(&mut reference, &mode(Some(2)), false)
                .unwrap();
            assert_eq!(out, reference, "copy rate {rate}");
            let mut bypassed = source.clone();
            out = signal(17);
            let dry = out.clone();
            reference = out.clone();
            bypassed
                .process_automation(&mut out, &mode(Some(2)), true)
                .unwrap();
            source
                .process_automation(&mut reference, &mode(Some(2)), false)
                .unwrap();
            assert_eq!(out, dry);
            out = signal(17);
            reference = out.clone();
            bypassed
                .process_automation(&mut out, &mode(Some(2)), false)
                .unwrap();
            source
                .process_automation(&mut reference, &mode(Some(2)), false)
                .unwrap();
            assert_eq!(out, reference);
            let mut tail = source.clone();
            let mut live = source.clone();
            tail.begin_output_tail();
            out = vec![[0.0; 2]; 2 * n + 17];
            reference = out.clone();
            tail.process_block(&mut out, &R);
            live.process_automation(&mut reference, &mode(Some(2)), false)
                .unwrap();
            assert_eq!(out, reference);
            assert_eq!(tail.mode, ChannelMode::MidSide);
            let mut absent = source.clone();
            let mut split = source.clone();
            out = signal(2 * n + 17);
            reference = out.clone();
            absent.process_block(&mut out, &R);
            for block in reference.chunks_mut(13) {
                split.process_block(block, &R);
            }
            assert_eq!(out, reference);
            assert_eq!(absent.mode, ChannelMode::Stereo);
            assert!(!absent.mode_fade.is_active());
            absent.reset();
            assert_eq!(absent.mode, ChannelMode::Stereo);
            assert!(absent.routing.pending.is_none());
            let mut fresh = prepare(&base, rate, &ids).unwrap().runtime();
            out = signal(97);
            reference = out.clone();
            absent
                .process_automation(&mut out, &mode(Some(2)), false)
                .unwrap();
            fresh
                .process_automation(&mut reference, &mode(Some(2)), false)
                .unwrap();
            assert_eq!(out, reference);
        }
    }
    #[test]
    fn dense_all_202_controls_are_finite_and_partition_independent() {
        let ids: Vec<_> = BINDINGS.iter().map(|b| b.descriptor.id).collect();
        assert_eq!(ids.len(), 202);
        for rate in [8000, 96000] {
            let base = colorful(0);
            let mut whole = prepare(&base, rate, &ids).unwrap().runtime();
            let mut split = prepare(&base, rate, &ids).unwrap().runtime();
            let mut values = BandDense {
                ids: ids.clone(),
                start: 0,
            };
            let mut out = signal(2 * mode_fade_frames(rate) as usize + 77);
            let mut reference = out.clone();
            whole.process_automation(&mut out, &values, false).unwrap();
            for (i, block) in reference.chunks_mut(13).enumerate() {
                values.start = i * 13;
                split.process_automation(block, &values, false).unwrap();
            }
            assert_eq!(out, reference);
            assert!(
                whole
                    .history
                    .iter()
                    .flatten()
                    .flatten()
                    .flatten()
                    .all(|v| v.is_finite())
            );
            assert!(out.iter().flatten().all(|v| v.is_finite()));
        }
    }
}
