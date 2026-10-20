//! Scalar candidate bindings only; no PCM, owner clone or admission authority.
use super::*;
use hex_project::{Device, Project};

fn devices(project: &Project) -> impl Iterator<Item = &Device> {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.devices().iter().chain(track.instrument()))
        .chain(
            project
                .return_tracks()
                .iter()
                .flat_map(|track| track.devices()),
        )
}

fn scratch_bytes(capacity: usize) -> Result<usize, ResourcePreparationError> {
    capacity
        .checked_mul(size_of::<AssetId>())
        .and_then(|bytes| bytes.checked_add(size_of::<Vec<AssetId>>()))
        .ok_or(ResourcePreparationError::MetadataCapacity)
}

/// Count before allocation. Duplicate references remain in the construction
/// quote: the unique output length cannot describe the temporary backing.
fn reference_count(project: &Project) -> Result<usize, ResourcePreparationError> {
    devices(project).try_fold(0usize, |count, device| {
        count
            .checked_add(device.state().asset_references().len())
            .ok_or(ResourcePreparationError::MetadataCapacity)
    })
}

/// Check the complete actual candidate, including muted/bypassed owners and
/// Return effects. Extra explicitly retained inputs remain allowed. This
/// temporary is destroyed before the bank producer begins; the enclosing
/// worker must still reserve its old/new union and complete construction peak.
pub(crate) fn validate_candidate_assets(
    project: &Project,
    supplied: &[AssetId],
    max_scratch_bytes: usize,
    cancel: &dyn hex_engine::CancellationQuery,
) -> Result<(), ResourcePreparationError> {
    check(cancel)?;
    if supplied.len() > hex_project::MAX_ASSETS
        || supplied
            .windows(2)
            .any(|pair| pair[0].value() >= pair[1].value())
    {
        return Err(ResourcePreparationError::InvalidSource);
    }
    let count = reference_count(project)?;
    if scratch_bytes(count)? > max_scratch_bytes {
        return Err(ResourcePreparationError::MetadataCapacity);
    }
    let mut required = Vec::new();
    required
        .try_reserve_exact(count)
        .map_err(|_| AudioStorageError::Allocation)?;
    if scratch_bytes(required.capacity())? > max_scratch_bytes {
        return Err(ResourcePreparationError::MetadataCapacity);
    }
    let assets = project.asset_entries_on_control().as_slice();
    for device in devices(project) {
        check(cancel)?;
        for reference in device.state().asset_references() {
            let index = assets
                .binary_search_by_key(&reference.asset.value(), |(id, _)| id.value())
                .map_err(|_| ResourcePreparationError::InvalidSource)?;
            required.push(assets[index].0);
        }
    }
    required.sort_unstable_by_key(|id| id.value());
    required.dedup();
    if required.len() > hex_project::MAX_ASSETS
        || required.iter().any(|id| {
            supplied
                .binary_search_by_key(&id.value(), |asset| asset.value())
                .is_err()
        })
    {
        return Err(ResourcePreparationError::InvalidSource);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
