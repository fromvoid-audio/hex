//! Fixed worker constructors using existing qualified original aliases only.
use super::*;
use hex_engine::native_device_resources::ResourcePreparationError;
use std::{alloc::Layout, mem::size_of, sync::atomic::AtomicUsize};

#[derive(Clone, Copy, Debug)]
pub struct NativeOriginalWaveformLimits {
    pub bins: usize,
    pub scan_frames: usize,
    /// Local requested outer/header/body and all summary backings. PCM and the
    /// full old/new graph construction reservation remain separately required.
    pub metadata_bytes: usize,
}

fn capacity_error() -> DeviceSourceImportError {
    ResourcePreparationError::MetadataCapacity.into()
}
fn add_array(total: usize, count: usize, element: usize) -> Result<usize, DeviceSourceImportError> {
    count
        .checked_mul(element)
        .and_then(|bytes| total.checked_add(bytes))
        .ok_or_else(capacity_error)
}
fn outer_bytes() -> Result<usize, DeviceSourceImportError> {
    Layout::array::<AtomicUsize>(2)
        .map_err(|_| capacity_error())?
        .extend(Layout::new::<Waveform>())
        .map(|(layout, _)| layout.pad_to_align().size())
        .map_err(|_| capacity_error())
}
/// Quote BEFORE any native alias/peak/paired-owner birth. Includes the actual
/// fixed16 outer pyramid capacity even for one leaf. No PCM bytes or historical
/// construction overlap is inferred from this local requested-allocation quote.
pub fn native_original_metadata_bytes(
    frames: usize,
    bins: usize,
) -> Result<usize, DeviceSourceImportError> {
    if frames == 0 || bins == 0 || bins > MAX_BINS {
        return Err(ImportError::LimitExceeded("invalid waveform extent").into());
    }
    let mut count = bins.min(frames);
    let mut bytes = add_array(
        outer_bytes()?,
        PYRAMID_LEVEL_CAPACITY,
        size_of::<Vec<[Peak; 2]>>(),
    )?;
    bytes = add_array(bytes, count, size_of::<[Peak; 2]>())?;
    while count > 1 {
        count = count.div_ceil(2);
        bytes = add_array(bytes, count, size_of::<[Peak; 2]>())?;
    }
    Ok(bytes)
}
fn actual_metadata_bytes(waveform: &Waveform) -> Result<usize, DeviceSourceImportError> {
    let mut bytes = add_array(
        outer_bytes()?,
        waveform.bins.capacity(),
        size_of::<[Peak; 2]>(),
    )?;
    bytes = add_array(
        bytes,
        waveform.levels.capacity(),
        size_of::<Vec<[Peak; 2]>>(),
    )?;
    for level in &waveform.levels {
        bytes = add_array(bytes, level.capacity(), size_of::<[Peak; 2]>())?;
    }
    Ok(bytes)
}
fn preflight(
    frames: usize,
    limits: NativeOriginalWaveformLimits,
    cancel: &dyn hex_engine::CancellationQuery,
) -> Result<(), DeviceSourceImportError> {
    if cancel.is_cancelled() {
        return Err(ImportError::Cancelled.into());
    }
    if frames > limits.scan_frames {
        return Err(ImportError::LimitExceeded("waveform source scan frames").into());
    }
    if native_original_metadata_bytes(frames, limits.bins)? > limits.metadata_bytes {
        return Err(capacity_error());
    }
    Ok(())
}
fn finish(
    source: OwnedAudioSource,
    limits: NativeOriginalWaveformLimits,
    cancel: &dyn hex_engine::CancellationQuery,
) -> Result<SharedWaveform, DeviceSourceImportError> {
    let frames = borrow_native(&source)?;
    let mut waveform = summarize_borrowed(frames, limits.bins, || {
        if cancel.is_cancelled() {
            Err(ImportError::Cancelled)
        } else {
            Ok(())
        }
    })?;
    // Actual capacities verify the prior allowance; they do not reconstruct a
    // past construction peak. All local children have closed scalar destructors.
    if actual_metadata_bytes(&waveform)? > limits.metadata_bytes {
        return Err(capacity_error());
    }
    waveform.samples = Some(SampleBacking::NativeOriginal(source));
    // No later query/refusal/clone: move the finished value into its one paired
    // outer constructor. Caller must immediately retain this in pending custody.
    Ok(SharedWaveform::new_on_control(waveform))
}
/// Borrow a frozen actual native original. The caller reserves/retains the
/// pending destination and enclosing old/new peak, and latches abnormal exit in
/// its actual construction custody. Local cancellation drops only scalar Vecs
/// and a source alias while this original borrow prevents the final PCM drop.
pub fn summarize_native_source_on_worker(
    source: &OwnedAudioSource,
    limits: NativeOriginalWaveformLimits,
    cancel: &dyn hex_engine::CancellationQuery,
) -> Result<SharedWaveform, DeviceSourceImportError> {
    let frames = borrow_native(source)?;
    preflight(frames.len(), limits, cancel)?;
    let alias = source
        .alias_for_resource_plan_on_control(size_of::<OwnedAudioSource>())
        .map_err(DeviceSourceImportError::Storage)?;
    finish(alias, limits, cancel)
}
/// Actual native stereo counterpart; raw/derived Audio fails the EXISTING
/// checked alias constructor. No raw owner is adopted or copied into a family.
pub fn summarize_native_audio_on_worker(
    source: &Audio,
    limits: NativeOriginalWaveformLimits,
    cancel: &dyn hex_engine::CancellationQuery,
) -> Result<SharedWaveform, DeviceSourceImportError> {
    preflight(source.frames().len(), limits, cancel)?;
    let alias =
        OwnedAudioSource::alias_native_audio_on_control(source, size_of::<OwnedAudioSource>())
            .map_err(DeviceSourceImportError::Storage)?;
    finish(alias, limits, cancel)
}

#[cfg(test)]
mod tests;
