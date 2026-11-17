//! Bounded, borrowed reference-channel sampling for future granular workers.
use crate::{StreamFailureKind, worker::map_import_error};
use hex_engine::{Frame, STREAM_PAGE_FRAMES, StreamedSourceMetadata};
use hex_media::{ResamplePlan, ResampleVersion, VerifiedWavReader};
use hex_warp::ReferenceChannelScan;

/// A completed decision carrying the caller's raw source and conversion facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedReferenceChannel {
    metadata: StreamedSourceMetadata,
    channel: usize,
    region: Option<hex_warp::SourceRegion>,
}

impl VerifiedReferenceChannel {
    /// Raw source and conversion facts supplied by the caller and checked here.
    pub fn metadata(&self) -> &StreamedSourceMetadata {
        &self.metadata
    }

    /// Region used for selection, or neutral full-source audio when absent.
    pub fn region(&self) -> Option<&hex_warp::SourceRegion> {
        self.region.as_ref()
    }

    pub(crate) fn with_region(
        metadata: StreamedSourceMetadata,
        channel: usize,
        region: hex_warp::SourceRegion,
    ) -> Self {
        Self {
            metadata,
            channel,
            region: Some(region),
        }
    }

    /// The selected stereo channel: zero for left, one for right.
    pub fn channel(&self) -> usize {
        self.channel
    }
}

/// Scan a neutral, full-source, project-rate signal on a control/analysis worker.
///
/// The caller retains the reader, conversion plan and both scratch buffers; no
/// page lease, file ownership or callback access is transferred. The result is
/// emitted only after every sample succeeds. At most 4096 representative frames
/// are converted. For a sparse source the verified reader may perform one bounded
/// read per sample, and its ordinary metadata/page checks still apply. This does
/// not verify pages that are never read or provide a whole-file snapshot.
/// The caller must establish the asset and generation binding to the reader;
/// matching file fingerprints alone cannot establish that ownership.
/// Cancellation is checked around reads and between conversion windows, each
/// of which contains at most 4096 output frames.
/// This performs blocking file I/O: do not call it from an audio callback or a
/// sole page-service worker that must keep active playback supplied.
/// Scratch may contain partial results on failure; no partial decision is returned.
pub fn scan_reference_channel_on_control(
    metadata: &StreamedSourceMetadata,
    reader: &mut VerifiedWavReader,
    plan: &ResamplePlan,
    source_scratch: &mut [Frame],
    output_scratch: &mut [Frame],
    mut cancelled: impl FnMut() -> bool,
) -> Result<VerifiedReferenceChannel, StreamFailureKind> {
    validate_source(metadata, reader, plan)?;
    if source_scratch.is_empty()
        || output_scratch.is_empty()
        || plan.max_output_window_frames() == 0
    {
        return Err(StreamFailureKind::CapacityExhausted);
    }
    let length = plan.output_frames();
    let count = length.min(4096);
    let stride = length / count.max(1);
    let mut scan =
        ReferenceChannelScan::new(length).map_err(|_| StreamFailureKind::DecodeFailed)?;
    while let Some(offset) = scan.next_offset() {
        if cancelled() {
            return Err(StreamFailureKind::Cancelled);
        }
        // Consecutive sampled offsets can share a conversion window. Sparse
        // samples use one output frame, avoiding duration-dependent work.
        let frames = if stride == 1 {
            usize::try_from(count - offset)
                .map_err(|_| StreamFailureKind::CapacityExhausted)?
                .min(output_scratch.len())
                .min(plan.max_output_window_frames())
                .min(STREAM_PAGE_FRAMES)
        } else {
            1
        };
        let (first, last) = plan
            .required_source_range(offset, frames)
            .map_err(map_import_error)?;
        let needed =
            usize::try_from(last - first).map_err(|_| StreamFailureKind::CapacityExhausted)?;
        if needed > source_scratch.len() || needed > reader.max_window_frames() {
            return Err(StreamFailureKind::CapacityExhausted);
        }
        let source = &mut source_scratch[..needed];
        let read = reader
            .read_frames_into_with_cancel(first, source, &mut cancelled)
            .map_err(map_import_error)?;
        if read != needed {
            return Err(StreamFailureKind::SourceChanged);
        }
        let output = &mut output_scratch[..frames];
        plan.render_window(first, source, offset, output)
            .map_err(map_import_error)?;
        for (i, sample) in output.iter().enumerate() {
            let at = offset + i as u64;
            scan.observe(at, [f64::from(sample[0]), f64::from(sample[1])])
                .map_err(|_| StreamFailureKind::DecodeFailed)?;
        }
    }
    if cancelled() {
        return Err(StreamFailureKind::Cancelled);
    }
    Ok(VerifiedReferenceChannel {
        metadata: metadata.clone(),
        channel: scan.finish().map_err(|_| StreamFailureKind::DecodeFailed)?,
        region: None,
    })
}

/// Check raw source facts without taking custody or reading pages.
pub(crate) fn validate_source(
    metadata: &StreamedSourceMetadata,
    reader: &VerifiedWavReader,
    plan: &ResamplePlan,
) -> Result<(), StreamFailureKind> {
    if !metadata.key().is_raw()
        || metadata.source_rate() != reader.sample_rate()
        || metadata.source_frames() != reader.source_frames()
        || metadata.source_rate() != plan.source_rate()
        || metadata.project_rate() != plan.target_rate()
        || metadata.source_frames() != plan.source_frames()
        || metadata.source_playback_frames() != plan.output_frames()
        || metadata.playback_frames() != plan.output_frames()
        || ResampleVersion::for_conversion(metadata.conversion_version())
            .map_err(map_import_error)?
            != plan.version()
    {
        return Err(StreamFailureKind::UnknownSource);
    }
    if metadata.fingerprint() != *reader.fingerprint() {
        return Err(StreamFailureKind::SourceChanged);
    }
    Ok(())
}
