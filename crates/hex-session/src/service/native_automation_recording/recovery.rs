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
    let limits = DraftReadLimits {
        max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
    };
    let selected = (|| {
        let reader = DraftReader::open_bounded(&path, limits, MAX_RECOVERY_EVENTS)
            .map_err(|error| error.to_string())?;
        let target = reader.target();
        let (provenance, identity) = match target {
            DraftTarget::Vst3Group(_) => {
                let take = reader
                    .into_completed_group()
                    .map_err(|error| error.to_string())?;
                (take.provenance(), take.content_identity())
            }
            DraftTarget::Device { .. } => {
                let completed = reader.into_completed().map_err(|error| error.to_string())?;
                let take = CompletedAutomationTake::from_completed_on_control(completed)?;
                if take.event_count() == 0 {
                    return Err("automation recovery file has no recorded applications".into());
                }
                (take.provenance(), take.content_identity())
            }
            _ => return Err("automation recovery file does not target a device".into()),
        };
        Ok((target, provenance, identity))
    })()
    .map_err(|reason: String| retained_error(&path, &reason))?;
    Ok(SealedAutomationRecoveryFile {
        path,
        target: selected.0,
        provenance: selected.1,
        identity: selected.2,
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
        if max_events > MAX_RECOVERY_EVENTS {
            return Err("device automation recovery exceeds the event budget".into());
        }
        let limits = DraftReadLimits {
            max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
        };
        if let DeviceCaptureTarget::Vst3Group(target) = scope.target {
            let mut take = receipt.open_completed_group_on_control(path, limits, max_events)?;
            if take.target() != target.draft {
                return Err("verified recovery belongs to another vst3 group".into());
            }
            return prepare_group_take_adoption_on_control(
                session,
                document_base,
                &mut take,
                scope.stamp,
            );
        }
        let DeviceCaptureTarget::Vst3(target) = scope.target else {
            return Err("verified plugin recovery requires a vst3 target".into());
        };
        let mut take = receipt.open_completed_on_control(path, limits, max_events)?;
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
        if let DraftTarget::Vst3Group(target) = expected_target {
            let mut take = match identity {
                AutomationRecoveryFileIdentity::Copy(receipt) => {
                    receipt.open_completed_group_on_control(path, limits, MAX_RECOVERY_EVENTS)?
                }
                AutomationRecoveryFileIdentity::Sealed(selected) => {
                    let take = CompletedGroupedDraft::open(path, limits, MAX_RECOVERY_EVENTS)
                        .map_err(|error| error.to_string())?;
                    if take.content_identity() != selected {
                        return Err("automation recovery file changed after selection".into());
                    }
                    take
                }
            };
            if take.target() != target {
                return Err("sealed recovery belongs to another vst3 group".into());
            }
            let command = prepare_group_take_adoption_on_control(
                session,
                document_base,
                &mut take,
                expected,
            )?;
            return Ok((Box::new(command), TakePublication::Vst3));
        }
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
