//! Explicit sealed-file adoption reuses the existing take/history transaction.
//! Durable capture identity and current publication intent remain distinct.
//! Current SDK/provider metadata still admit the canonical command; no historical
//! live worker binding is stored, reconstructed or accepted as capability.
use super::*;
use hex_storage::automation_draft::{AutomationContentIdentity, DraftReader, DraftTarget};

const MAX_RECOVERY_EVENTS: u64 = hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64;

/// Opaque proof selected on control. Both forms are revalidated before applying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationRecoveryFileIdentity {
    Copy(RecoveryCopyReceipt),
    Sealed(AutomationContentIdentity),
}

/// Read-only selected device draft; owns neither writer custody nor an edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedAutomationRecoveryFile {
    pub path: PathBuf,
    pub target: DraftTarget,
    pub provenance: DraftProvenance,
    pub identity: AutomationContentIdentity,
}

fn open_bounded_sealed(
    path: &Path,
    limits: DraftReadLimits,
) -> Result<CompletedAutomationTake, String> {
    let reader = DraftReader::open_bounded(path, limits, MAX_RECOVERY_EVENTS)
        .map_err(|error| error.to_string())?;
    let completed = reader.into_completed().map_err(|error| error.to_string())?;
    CompletedAutomationTake::from_completed_on_control(completed)
}

pub(in crate::service) fn inspect_sealed_file(
    path: &Path,
) -> Result<SealedAutomationRecoveryFile, String> {
    let path = super::super::recovery::absolute_document_path(path)?;
    let take = open_bounded_sealed(
        &path,
        DraftReadLimits {
            max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
        },
    )
    .map_err(|reason| retained_error(&path, &reason))?;
    if take.event_count() == 0 {
        return Err(retained_error(
            &path,
            "automation recovery file has no recorded applications",
        ));
    }
    let target = take.target();
    if !matches!(target, DraftTarget::Device { .. }) {
        return Err("automation recovery file does not target a device".into());
    }
    Ok(SealedAutomationRecoveryFile {
        path,
        target,
        provenance: take.provenance(),
        identity: take.content_identity(),
    })
}

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
    identity: AutomationRecoveryFileIdentity,
) -> Result<(Box<hex_project::Command>, TakePublication), String> {
    let result = (|| {
        let limits = DraftReadLimits {
            max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
        };
        let mut take = match identity {
            AutomationRecoveryFileIdentity::Copy(receipt) => {
                receipt.open_completed_on_control(path, limits, MAX_RECOVERY_EVENTS)?
            }
            AutomationRecoveryFileIdentity::Sealed(selected) => {
                let take = open_bounded_sealed(path, limits)?;
                if take.content_identity() != selected {
                    return Err("automation recovery file changed after selection".into());
                }
                take
            }
        };
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
