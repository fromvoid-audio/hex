//! Shared preview-curve cache and effective-parameter patching for native effect panels
//! (`compressor_panel`, `overdrive_panel`, `saturator_panel`).
use super::{EditorView, ParameterKey, effective_parameter_value};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::DeviceId;
use std::collections::HashMap;

/// Cached preview curve keyed by canonical device state, effective parameter values, and any
/// extra context (such as sample rate for frequency-response previews).
pub(super) struct PreviewCache<T, const N: usize, Extra = ()> {
    state: NativeDeviceState,
    extra: Extra,
    values: [ParameterValue; N],
    curve: Result<T, String>,
}

impl<T, const N: usize, Extra> PreviewCache<T, N, Extra> {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn ok_curve(&self) -> Option<&T> {
        self.curve.as_ref().ok()
    }
}

/// Inputs describing one native effect preview curve lookup and fallback computation.
pub(super) struct PreviewQuery<'a, const N: usize, Extra = ()> {
    pub(super) device: DeviceId,
    pub(super) state: &'a NativeDeviceState,
    pub(super) device_view: &'a NativeDeviceView,
    pub(super) parameters: [ParameterId; N],
    pub(super) missing_error: &'static str,
    pub(super) extra: Extra,
}

/// Resolve a cached effect preview curve, computing and memoizing a fresh curve on cache miss.
pub(super) fn resolve<T, const N: usize, Extra>(
    view: &mut EditorView,
    query: PreviewQuery<'_, N, Extra>,
    cache_map: impl Fn(&mut EditorView) -> &mut HashMap<DeviceId, PreviewCache<T, N, Extra>>,
    evaluate: impl FnOnce(&NativeDeviceState) -> Result<T, String>,
) -> Result<T, String>
where
    T: Clone,
    Extra: Copy + PartialEq,
{
    let PreviewQuery {
        device,
        state,
        device_view,
        parameters,
        missing_error,
        extra,
    } = query;
    let mut values = [ParameterValue::Float(0.0); N];
    for (index, id) in parameters.iter().enumerate() {
        let parameter = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == *id)
            .ok_or_else(|| missing_error.to_owned())?;
        values[index] = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: *id,
            },
            parameter.value,
        );
    }
    if let Some(cached) = cache_map(view).get(&device)
        && cached.state == *state
        && cached.extra == extra
        && cached.values == values
    {
        return cached.curve.clone();
    }
    let curve = patch_effect_state(state, parameters, values).and_then(|draft| evaluate(&draft));
    cache_map(view).insert(
        device,
        PreviewCache {
            state: state.clone(),
            extra,
            values,
            curve: curve.clone(),
        },
    );
    curve
}

fn patch_effect_state<const N: usize>(
    state: &NativeDeviceState,
    parameters: [ParameterId; N],
    values: [ParameterValue; N],
) -> Result<NativeDeviceState, String> {
    let mut draft = state.clone();
    for (id, value) in parameters.into_iter().zip(values) {
        if hex_engine::native_device_parameter_for_class(NativeDeviceClass::Effect, &draft, id)
            .map_err(|error| error.to_string())?
            != value
        {
            draft = hex_engine::patch_native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                &draft,
                id,
                value,
            )
            .map_err(|error| error.to_string())?;
        }
    }
    Ok(draft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::DeviceTypeId;

    #[test]
    fn patch_effect_state_updates_only_changed_parameters_and_preserves_identity() {
        let saturator = DeviceTypeId::from_static("hex.saturator");
        let base =
            hex_engine::default_native_device_state_for_class(NativeDeviceClass::Effect, saturator)
                .expect("saturator default state");
        let drive_id = ParameterId::from_static(1);
        let original = hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            &base,
            drive_id,
        )
        .expect("drive parameter");

        // Unchanged value returns an equivalent state without error.
        let same = patch_effect_state(&base, [drive_id], [original]).expect("unchanged patch");
        assert_eq!(same, base);

        // Changed value updates the target parameter cleanly.
        let updated = patch_effect_state(&base, [drive_id], [ParameterValue::Float(12.0)])
            .expect("updated patch");
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                &updated,
                drive_id,
            )
            .expect("patched drive"),
            ParameterValue::Float(12.0)
        );
    }
}
