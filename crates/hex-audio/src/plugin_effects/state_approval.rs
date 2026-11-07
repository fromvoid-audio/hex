//! Only the original joining owner can approve an isolated canonical base.
//! Callback configuration retains a nonissuing verifier, never the authority.
use super::*;

fn approve(
    authority: &hex_lattice_contract::Vst3StateAuthority,
    configuration: &Configuration,
    state: &NativeDeviceState,
) -> Result<NativeDeviceState, &'static str> {
    if Hosting::saved_identity_on_control(state).map_err(|_| "native captured state is invalid")?
        != Hosting::saved_identity_on_control(&configuration.state)
            .map_err(|_| "native source state is invalid")?
    {
        return Err("native captured state identity differs");
    }
    authority
        .approve_on_control(state.clone())
        .map_err(|_| "native captured state approval refused")
}

impl NativeEffect {
    pub(crate) fn approve_captured_state_on_control(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, &'static str> {
        approve(&self.state_authority, &self.callback.configuration, state)
    }
}

impl NativeEffectControl {
    pub(crate) fn approve_captured_state_on_control(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, &'static str> {
        approve(&self.state_authority, &self.configuration, state)
    }
}

impl NativeEffectReference<'_> {
    pub(crate) fn approves_captured_state_on_control(&self, state: &NativeDeviceState) -> bool {
        self.configuration()
            .state_verifier
            .is_some_and(|verifier| verifier.approves(state))
    }
}
