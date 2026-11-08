//! Bounded read-only carrier-filter response, separate from instrument rendering.
use super::OperatorSettings;
use crate::NativeResponseError;
use hex_lattice_contract::NativeDeviceState;

/// Query up to 256 strictly increasing positive frequencies below host Nyquist.
/// Uses validated authored state and the native filter preparation rate (host
/// rate for legacy, 2x for Operator mode). This is the carrier filter alone:
/// oscillator/decimator transfer, live modulation and smoothing are excluded.
/// Preparation/allocation belongs on control, never in an audio callback.
pub fn operator_filter_frequency_response(
    state: &NativeDeviceState,
    sample_rate: u32,
    frequencies_hz: &[f32],
) -> Result<Vec<f32>, NativeResponseError> {
    if frequencies_hz.is_empty()
        || frequencies_hz.len() > 256
        || frequencies_hz.iter().any(|frequency| {
            !frequency.is_finite() || *frequency <= 0.0 || *frequency >= sample_rate as f32 * 0.5
        })
        || frequencies_hz.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(NativeResponseError::InvalidFrequencyRange);
    }
    let settings =
        OperatorSettings::from_state(state).map_err(|_| NativeResponseError::Unavailable)?;
    let prepared = if settings.global.active {
        settings.filter.prepare_oversampled(sample_rate)
    } else {
        settings.filter.prepare(sample_rate)
    }
    .map_err(|_| NativeResponseError::Unavailable)?;
    let mut response = Vec::new();
    response
        .try_reserve_exact(frequencies_hz.len())
        .map_err(|_| NativeResponseError::AllocationFailed)?;
    for &frequency in frequencies_hz {
        let magnitude = if settings.filter.armed {
            prepared.magnitude_at_hz(frequency)
        } else {
            1.0
        };
        if !magnitude.is_finite() || magnitude < 0.0 {
            return Err(NativeResponseError::InvalidResponse);
        }
        response.push(magnitude);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

    fn settings() -> OperatorSettings {
        let mut settings = OperatorSettings::defaults();
        settings
            .filter
            .patch(ParameterId::from_static(20_001), ParameterValue::Bool(true))
            .unwrap();
        settings
            .filter
            .patch(
                ParameterId::from_static(20_005),
                ParameterValue::Float(20_000.0),
            )
            .unwrap();
        settings
    }

    #[test]
    fn full_state_query_preserves_authored_bytes_and_uses_the_native_working_rate() {
        let mut settings = settings();
        let frequencies = [100.0, 1000.0, 3000.0];
        let mut curves = Vec::new();
        for active in [false, true] {
            settings.global.active = active;
            let state = settings.to_state().unwrap();
            let bytes = state.payload().to_vec();
            let prepared = if active {
                settings.filter.prepare_oversampled(8000)
            } else {
                settings.filter.prepare(8000)
            }
            .unwrap();
            let curve = operator_filter_frequency_response(&state, 8000, &frequencies).unwrap();
            assert_eq!(curve, frequencies.map(|hz| prepared.magnitude_at_hz(hz)));
            assert_eq!(state.payload(), bytes);
            curves.push(curve);
        }
        assert_ne!(
            curves[0], curves[1],
            "rate-specific ceiling must follow the actual factory"
        );
        // Version11's codec ends at the filter block and predates Operator mode.
        let current = settings.to_state().unwrap();
        let legacy = NativeDeviceState::new(
            current.type_id(),
            11,
            current.payload()[..super::super::FILTER_PAYLOAD_BYTES].to_vec(),
        )
        .unwrap();
        assert_eq!(
            operator_filter_frequency_response(&legacy, 8000, &frequencies).unwrap(),
            curves[0]
        );
        settings
            .filter
            .patch(
                ParameterId::from_static(20_001),
                ParameterValue::Bool(false),
            )
            .unwrap();
        assert_eq!(
            operator_filter_frequency_response(&settings.to_state().unwrap(), 8000, &frequencies)
                .unwrap(),
            [1.0; 3]
        );
        assert_eq!(
            operator_filter_frequency_response(
                &OperatorSettings::defaults().to_state().unwrap(),
                8000,
                &frequencies
            )
            .unwrap(),
            [1.0; 3]
        );
    }

    #[test]
    fn query_refuses_invalid_state_rate_and_bounded_frequency_inputs() {
        let state = settings().to_state().unwrap();
        for frequencies in [
            &[][..],
            &[0.0],
            &[-1.0],
            &[24_000.0],
            &[100.0, 100.0],
            &[200.0, 100.0],
            &[f32::NAN],
            &[f32::INFINITY],
        ] {
            assert_eq!(
                operator_filter_frequency_response(&state, 48_000, frequencies),
                Err(NativeResponseError::InvalidFrequencyRange)
            );
        }
        let frequencies: Vec<_> = (1..=257).map(|hz| hz as f32).collect();
        assert_eq!(
            operator_filter_frequency_response(&state, 48_000, &frequencies),
            Err(NativeResponseError::InvalidFrequencyRange)
        );
        assert_eq!(
            operator_filter_frequency_response(&state, 48_000, &frequencies[..256])
                .unwrap()
                .len(),
            256
        );
        for rate in [7999, 192_001] {
            assert_eq!(
                operator_filter_frequency_response(&state, rate, &[100.0]),
                Err(NativeResponseError::Unavailable)
            );
        }
        let mut malformed = state.payload().to_vec();
        malformed[..4].copy_from_slice(&f32::NAN.to_le_bytes());
        for invalid in [
            NativeDeviceState::new(
                DeviceTypeId::new("hex.effect.utility").unwrap(),
                1,
                Vec::new(),
            )
            .unwrap(),
            NativeDeviceState::new(
                state.type_id(),
                state.state_version() + 1,
                state.payload().to_vec(),
            )
            .unwrap(),
            NativeDeviceState::new(state.type_id(), state.state_version(), Vec::new()).unwrap(),
            NativeDeviceState::new(state.type_id(), state.state_version(), malformed).unwrap(),
        ] {
            assert_eq!(
                operator_filter_frequency_response(&invalid, 48_000, &[100.0]),
                Err(NativeResponseError::Unavailable)
            );
        }
    }
}
