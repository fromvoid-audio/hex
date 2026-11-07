//! Only the original joining owner can approve an isolated canonical base.
//! Callback configuration retains a nonissuing verifier, never the authority.
use super::*;

fn approve(
    authority: &hex_lattice_contract::Vst3StateAuthority,
    configuration: &Configuration,
    state: &NativeDeviceState,
) -> Result<NativeDeviceState, &'static str> {
    validate_identity(configuration, state)?;
    authority
        .approve_on_control(state.clone())
        .map_err(|_| "native captured state approval refused")
}

fn validate_identity(
    configuration: &Configuration,
    state: &NativeDeviceState,
) -> Result<(), &'static str> {
    if Hosting::saved_identity_on_control(state).map_err(|_| "native captured state is invalid")?
        != Hosting::saved_identity_on_control(&configuration.state)
            .map_err(|_| "native source state is invalid")?
    {
        return Err("native captured state identity differs");
    }
    Ok(())
}

fn approve_processing(
    authority: &hex_lattice_contract::Vst3StateAuthority,
    configuration: &Configuration,
    facts: &CapturedCanonicalBases,
) -> Result<NativeDeviceState, &'static str> {
    let state = facts.canonical_state();
    validate_identity(configuration, state)?;
    if facts.binding() != configuration.binding
        || facts.prepared() != configuration.prepared
        || facts.sample_rate() != configuration.sample_rate
        || !(state.processing_eq(&configuration.state)
            || configuration
                .state_verifier
                .is_some_and(|verifier| verifier.approves(state)))
    {
        return Err("native processing approval source or profile differs");
    }
    authority
        .approve_processing_on_control(state.clone())
        .map_err(|_| "native processing approval refused")
}

impl NativeEffect {
    pub(crate) fn approve_canonical_processing_on_control(
        &self,
        facts: &CapturedCanonicalBases,
    ) -> Result<NativeDeviceState, &'static str> {
        approve_processing(&self.state_authority, &self.callback.configuration, facts)
    }
    pub(crate) fn approve_captured_state_on_control(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, &'static str> {
        approve(&self.state_authority, &self.callback.configuration, state)
    }
}

impl NativeEffectControl {
    pub(crate) fn approve_canonical_processing_on_control(
        &self,
        facts: &CapturedCanonicalBases,
    ) -> Result<NativeDeviceState, &'static str> {
        approve_processing(&self.state_authority, &self.configuration, facts)
    }
    pub(crate) fn approve_captured_state_on_control(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, &'static str> {
        approve(&self.state_authority, &self.configuration, state)
    }
}

impl NativeEffectReference<'_> {
    pub(crate) fn approves_processing_state_on_control(&self, state: &NativeDeviceState) -> bool {
        self.configuration()
            .state_verifier
            .is_some_and(|verifier| verifier.approves_processing(state))
    }
    pub(crate) fn approves_captured_state_on_control(&self, state: &NativeDeviceState) -> bool {
        self.configuration()
            .state_verifier
            .is_some_and(|verifier| verifier.approves(state))
    }
}
