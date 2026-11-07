//! Captured-state provenance uses the original source's existing control custody.
use super::*;
use crate::plugin_effects::manual_base_capture::CompletedManualBase;

impl CandidateRoot {
    pub(crate) fn approve_native_canonical_processing(
        &self,
        facts: &crate::plugin_effects::CapturedCanonicalBases,
    ) -> Result<NativeDeviceState, &'static str> {
        self.require_native_source_control_open()?;
        let state = self
            .0
            .eligible
            .with_live_capture(facts.binding(), |owner| match owner {
                Owner::Effect(effect) => effect.approve_canonical_processing_on_control(facts),
                Owner::Loaned(control) => control.approve_canonical_processing_on_control(facts),
                _ => Err("native processing approval source is unavailable"),
            })?;
        self.require_native_source_control_open()?;
        Ok(state)
    }

    pub(crate) fn native_source_approves_processing_state(
        &self,
        binding: Binding,
        state: &NativeDeviceState,
    ) -> Option<bool> {
        self.with_native_source_endpoint(binding, |_, source| {
            source.approves_processing_state_on_control(state)
        })
    }
    pub(crate) fn approve_native_manual_base(
        &self,
        captured: &CompletedManualBase,
    ) -> Result<NativeDeviceState, &'static str> {
        self.require_native_source_control_open()?;
        let state = self
            .0
            .eligible
            .with_live_capture(captured.binding(), |owner| match owner {
                Owner::Effect(effect) => effect.approve_captured_state_on_control(captured.state()),
                Owner::Loaned(control) => {
                    control.approve_captured_state_on_control(captured.state())
                }
                _ => Err("native captured state source is unavailable"),
            })?;
        self.require_native_source_control_open()?;
        Ok(state)
    }

    pub(crate) fn native_source_approves_captured_state(
        &self,
        binding: Binding,
        state: &NativeDeviceState,
    ) -> Option<bool> {
        self.with_native_source_endpoint(binding, |_, source| {
            source.approves_captured_state_on_control(state)
        })
    }
}
