//! Control-side response queries use the same prepared processors as playback.
use hex_lattice_contract::NativeDeviceState;

/// A native response query could not be prepared or returned safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeResponseError {
    InvalidFrequencyRange,
    InvalidRegistry,
    Unavailable,
    AllocationFailed,
    InvalidResponse,
}

impl NativeResponseError {
    /// Stable diagnostic used by the existing engine query contract.
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidFrequencyRange => "invalid native response frequency range",
            Self::InvalidRegistry => "invalid built-in native device registry",
            Self::Unavailable => "native device response is unavailable",
            Self::AllocationFailed => "native device response allocation failed",
            Self::InvalidResponse => "native device response is invalid",
        }
    }
}

impl std::fmt::Display for NativeResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for NativeResponseError {}

/// Sample the two processor-defined response lanes at up to 256 ordered
/// frequencies below Nyquist. `None` means the effect has no response query.
/// Preparation and allocation occur on the control side, never in a callback.
pub fn native_effect_frequency_response(
    state: &NativeDeviceState,
    sample_rate: u32,
    frequencies_hz: &[f32],
) -> Result<Option<Vec<[f32; 2]>>, NativeResponseError> {
    if frequencies_hz.is_empty()
        || frequencies_hz.len() > 256
        || frequencies_hz.iter().any(|frequency| {
            !frequency.is_finite() || *frequency <= 0.0 || *frequency >= sample_rate as f32 * 0.5
        })
        || frequencies_hz.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(NativeResponseError::InvalidFrequencyRange);
    }
    let registry = crate::native_registry().map_err(|_| NativeResponseError::InvalidRegistry)?;
    let Some(processor) = registry
        .prepare_processor(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .map_err(|_| NativeResponseError::Unavailable)?
    else {
        return Ok(None);
    };
    let mut response = Vec::new();
    response
        .try_reserve_exact(frequencies_hz.len())
        .map_err(|_| NativeResponseError::AllocationFailed)?;
    for frequency in frequencies_hz {
        let Some(magnitude) = processor.frequency_response_at_hz(*frequency) else {
            return Ok(None);
        };
        if magnitude
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(NativeResponseError::InvalidResponse);
        }
        response.push(magnitude);
    }
    Ok(Some(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_response_validates_frequencies_and_uses_prepared_eq() {
        let state = crate::native_registry()
            .unwrap()
            .default_state(crate::eq8::EQ8_TYPE_ID)
            .unwrap();
        let response = native_effect_frequency_response(&state, 48_000, &[500.0, 1_000.0])
            .unwrap()
            .unwrap();
        assert_eq!(response.len(), 2);
        assert!(response.iter().flatten().all(|m| (*m - 1.0).abs() < 1e-6));
        for frequencies in [
            &[][..],
            &[0.0][..],
            &[24_000.0][..],
            &[1_000.0, 500.0][..],
            &[1_000.0, 1_000.0][..],
            &[f32::NAN][..],
            &[f32::INFINITY][..],
            &[-1.0][..],
        ] {
            assert_eq!(
                native_effect_frequency_response(&state, 48_000, frequencies),
                Err(NativeResponseError::InvalidFrequencyRange)
            );
        }
        // Keep the count boundary independent of the ordering validation.
        let frequencies: Vec<_> = (1..=257).map(|index| index as f32).collect();
        assert_eq!(
            native_effect_frequency_response(&state, 48_000, &frequencies),
            Err(NativeResponseError::InvalidFrequencyRange)
        );
        assert_eq!(
            native_effect_frequency_response(&state, 48_000, &frequencies[..256])
                .unwrap()
                .unwrap()
                .len(),
            256
        );
    }
}
