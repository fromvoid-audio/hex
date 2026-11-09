//! Explicit sealed-file adoption reuses the existing take/history transaction.
//! Durable capture identity and current publication intent remain distinct.
//! Current SDK/provider metadata still admit the canonical command; no historical
//! live worker binding is stored, reconstructed or accepted as capability.
use super::*;
use hex_storage::automation_draft::DraftTarget;

/// Prepare a canonical edit from the verified source without publishing it or
/// releasing original custody. Call on control, never the audio callback.
pub fn prepare_verified_vst3_recovery_on_control(
    session: &Session,
    document_base: Option<&Path>,
    scope: DeviceAutomationTakeScope,
    path: &Path,
    receipt: RecoveryCopyReceipt,
    max_events: u64,
) -> Result<hex_project::Command, String> {
    let result = (|| {
        let DeviceCaptureTarget::Vst3(target) = scope.target else {
            return Err("verified plugin recovery requires a vst3 target".into());
        };
        if max_events > hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64 {
            return Err("device automation recovery exceeds the event budget".into());
        }
        let mut take = receipt.open_completed_on_control(
            path,
            DraftReadLimits {
                max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
            },
            max_events,
        )?;
        let expected = DraftTarget::Device {
            device: target.device,
            target: hex_project::DeviceAutomationTarget::Vst3 {
                parameter: target.parameter,
            },
        };
        if take.target() != expected
            || take
                .target()
                .vst3_class_in_source(take.source())
                .map_err(|error| error.to_string())?
                != Some(target.class)
        {
            return Err("verified recovery belongs to another device, parameter or class".into());
        }
        prepare_device_take_adoption_on_control(session, document_base, &mut take, scope.stamp)
    })();
    result.map_err(|reason: String| retained_error(path, &reason))
}

pub(in crate::service) fn recover_sealed(
    session: &Session,
    document_base: Option<&Path>,
    expected_target: DraftTarget,
    expected: DraftProvenance,
    path: &Path,
) -> Result<(Box<hex_project::Command>, TakePublication), String> {
    let result = (|| {
        let mut take = CompletedAutomationTake::open_on_control(
            path,
            DraftReadLimits {
                max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
            },
        )
        .map_err(|error| error.to_string())?;
        if take.target() != expected_target {
            return Err("sealed recovery belongs to another device or parameter".into());
        }
        let DraftTarget::Device { device, target } = expected_target else {
            return Err("sealed recovery does not target a device".into());
        };
        let publication = if matches!(target, hex_project::DeviceAutomationTarget::Vst3 { .. }) {
            TakePublication::Vst3
        } else {
            TakePublication::Native(
                hex_engine::native_parameter_input_target(session.project(), device, target)
                    .map_err(|error| error.0.to_owned())?,
            )
        };
        // The shared adoption owner verifies complete source equivalence,
        // provenance, exact saved class and bounded sample/loop conversion.
        let command =
            prepare_device_take_adoption_on_control(session, document_base, &mut take, expected)?;
        Ok((Box::new(command), publication))
    })();
    result.map_err(|reason: String| retained_error(path, &reason))
}

#[cfg(test)]
mod tests;
