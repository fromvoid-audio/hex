//! Explicit historical-kernel fixture for private analytic/feedback proofs.
use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterValue};

pub fn legacy_operator_state() -> NativeDeviceState {
    let mut state = OPERATOR_METADATA.default_state().unwrap();
    for source in 0..4_u32 {
        for target in 0..4_u32 {
            if source != target {
                state = OPERATOR_METADATA
                    .patch_parameter(
                        &state,
                        ParameterId::from_static(13 + source * 4 + target),
                        ParameterValue::Float(0.0),
                    )
                    .unwrap();
            }
        }
        for (id, value) in [
            (2 + source * 3, 1.0),
            (29 + source, if source == 0 { 1.0 } else { 0.0 }),
        ] {
            state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    ParameterId::from_static(id),
                    ParameterValue::Float(value),
                )
                .unwrap();
        }
    }
    OPERATOR_METADATA
        .patch_parameter(&state, parameter::GLOBAL_MODE, ParameterValue::Choice(0))
        .unwrap()
}
