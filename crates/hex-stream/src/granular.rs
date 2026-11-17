//! Borrowed verified source access for one sequential granular grain.
use crate::{
    StreamFailureKind, VerifiedReferenceChannel, reference::validate_source,
    worker::map_import_error,
};
use hex_engine::{Frame, STREAM_PAGE_FRAMES, StreamedSourceMetadata};
use hex_media::{ResamplePlan, VerifiedWavReader};
use hex_warp::{Error, GranularSource, ReferenceChannelScan, SequentialGranular, SourceRegion};
use std::cell::RefCell;

/// Caller-owned source conversion, bounded cache and grain scratch.
pub struct VerifiedGranularBuffers<'a> {
    pub source: &'a mut [Frame],
    pub cache: &'a mut [Frame],
    pub grain: &'a mut [[f64; 2]],
}

/// The verified source and warp owners retain distinct failure details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifiedGranularStepError {
    Source(StreamFailureKind),
    Warp(Error),
}
impl From<Error> for VerifiedGranularStepError {
    fn from(error: Error) -> Self {
        Self::Warp(error)
    }
}

/// Advance at most one grain using verified project-rate audio.
///
/// The caller must bind the renderer to this reference decision, raw asset,
/// generation, conversion and source interval; fingerprints alone do not grant
/// that binding. Source, plan and buffers are borrowed and remain caller-owned.
/// The 32 MiB check covers renderer backing and supplied slice lengths, not
/// larger backing allocations, the reader manifest, plan, or stack. The caller
/// accounts for all retained capacity and other worker memory. Region-backed
/// reads require four cache slots of at least 2048 project-rate frames (4096
/// above 55,999 Hz) to limit loop-boundary cache churn; smaller plans refuse.
/// Only pages read on this call are verified. A bounded cache lives only for
/// this grain; a cached value can outlive a file change during the same call,
/// so this is not a whole-file snapshot. The caller must invalidate the result
/// on source replacement and discard a poisoned renderer after partial failure.
/// This blocks on I/O: use a separate control/analysis worker, never an audio
/// callback or the sole worker serving active playback. Cancellation is checked
/// during verified reads, conversion-window boundaries and sample iteration.
pub fn advance_verified_granular_on_control(
    renderer: &mut SequentialGranular,
    reference: &VerifiedReferenceChannel,
    reader: &mut VerifiedWavReader,
    plan: &ResamplePlan,
    buffers: VerifiedGranularBuffers<'_>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<bool, VerifiedGranularStepError> {
    validate_source(reference.metadata(), reader, plan)
        .map_err(VerifiedGranularStepError::Source)?;
    if renderer.source_parameters() != (reference.metadata().project_rate(), reference.channel()) {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::UnknownSource,
        ));
    }
    let aggregate = renderer
        .backing_bytes()
        .and_then(|n| {
            reference.region().map_or(Some(n), |region| {
                region.backing_bytes().and_then(|v| n.checked_add(v))
            })
        })
        .and_then(|n| {
            buffers
                .grain
                .len()
                .checked_mul(std::mem::size_of::<[f64; 2]>())
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| {
            buffers
                .source
                .len()
                .checked_mul(std::mem::size_of::<Frame>())
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| {
            buffers
                .cache
                .len()
                .checked_mul(std::mem::size_of::<Frame>())
                .and_then(|v| n.checked_add(v))
        });
    if aggregate.is_none_or(|n| n > hex_warp::MAX_SCRATCH_BYTES) {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::CapacityExhausted,
        ));
    }
    let slots = if reference.region().is_some() { 4 } else { 2 };
    let slot = (buffers.cache.len() / slots)
        .min(STREAM_PAGE_FRAMES)
        .min(plan.max_output_window_frames());
    if reference.region().is_some() && slot < region_min_slot(plan.target_rate()) {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::CapacityExhausted,
        ));
    }
    if slot == 0 || buffers.source.is_empty() {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::CapacityExhausted,
        ));
    }
    let cancel: RefCell<&mut dyn FnMut() -> bool> = RefCell::new(&mut cancelled);
    let mut source = Source {
        reader,
        plan,
        source: buffers.source,
        cache: buffers.cache,
        slot,
        valid: [None; 4],
        ages: [0; 4],
        slots,
        clock: 0,
        checks: 0,
        region: reference.region(),
        cancel: &cancel,
    };
    renderer.advance(&mut source, buffers.grain, &mut || check_cancel(&cancel))
}

/// Select the granular reference channel from a verified project-rate region.
///
/// The region is moved into the result and is reused by granular synthesis.
/// The caller binds the asset/generation to this borrowed reader; only pages
/// read by this call are verified, and the per-call cache is not a file snapshot.
/// This may block and must run off the audio callback and sole playback worker.
/// Region scans require the same four-slot minimum as granular advances.
pub fn scan_verified_granular_region_on_control(
    metadata: &StreamedSourceMetadata,
    region: SourceRegion,
    reader: &mut VerifiedWavReader,
    plan: &ResamplePlan,
    source_scratch: &mut [Frame],
    cache_scratch: &mut [Frame],
    mut cancelled: impl FnMut() -> bool,
) -> Result<VerifiedReferenceChannel, VerifiedGranularStepError> {
    validate_source(metadata, reader, plan).map_err(VerifiedGranularStepError::Source)?;
    if region.source_frames() != plan.output_frames() {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::UnknownSource,
        ));
    }
    let memory = region
        .backing_bytes()
        .and_then(|n| {
            source_scratch
                .len()
                .checked_mul(std::mem::size_of::<Frame>())
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| {
            cache_scratch
                .len()
                .checked_mul(std::mem::size_of::<Frame>())
                .and_then(|v| n.checked_add(v))
        });
    let slots = 4;
    let slot = (cache_scratch.len() / slots)
        .min(STREAM_PAGE_FRAMES)
        .min(plan.max_output_window_frames());
    if slot < region_min_slot(plan.target_rate()) {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::CapacityExhausted,
        ));
    }
    if memory.is_none_or(|n| n > hex_warp::MAX_SCRATCH_BYTES)
        || slot == 0
        || source_scratch.is_empty()
    {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::CapacityExhausted,
        ));
    }
    let cancel: RefCell<&mut dyn FnMut() -> bool> = RefCell::new(&mut cancelled);
    if check_cancel(&cancel) {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::Cancelled,
        ));
    }
    let mut scan = ReferenceChannelScan::new(region.length())?;
    {
        let mut source = Source {
            reader,
            plan,
            source: source_scratch,
            cache: cache_scratch,
            slot,
            valid: [None; 4],
            ages: [0; 4],
            slots,
            clock: 0,
            checks: 0,
            region: Some(&region),
            cancel: &cancel,
        };
        while let Some(offset) = scan.next_offset() {
            scan.observe(offset, source.frame(i128::from(offset))?)?;
        }
    }
    if check_cancel(&cancel) {
        return Err(VerifiedGranularStepError::Source(
            StreamFailureKind::Cancelled,
        ));
    }
    Ok(VerifiedReferenceChannel::with_region(
        metadata.clone(),
        scan.finish()?,
        region,
    ))
}

// Four windows of this size cover the common analysis working set across a
// wrap; smaller regions can still require both sides on every comparison.
fn region_min_slot(rate: u32) -> usize {
    if (rate / 8_000).max(1) <= 6 {
        2048
    } else {
        4096
    }
}

fn check_cancel(cancel: &RefCell<&mut dyn FnMut() -> bool>) -> bool {
    cancel
        .try_borrow_mut()
        .map_or(true, |mut callback| (*callback)())
}

struct Source<'a, 'c> {
    reader: &'a mut VerifiedWavReader,
    plan: &'a ResamplePlan,
    source: &'a mut [Frame],
    cache: &'a mut [Frame],
    slot: usize,
    valid: [Option<(u64, usize)>; 4],
    ages: [u64; 4],
    slots: usize,
    clock: u64,
    checks: usize,
    region: Option<&'a SourceRegion>,
    cancel: &'c RefCell<&'c mut dyn FnMut() -> bool>,
}
impl Source<'_, '_> {
    fn load(&mut self, position: u64) -> Result<Frame, VerifiedGranularStepError> {
        self.clock = self.clock.saturating_add(1);
        if let Some((index, (start, _))) =
            self.valid[..self.slots]
                .iter()
                .enumerate()
                .find_map(|(i, item)| {
                    item.filter(|(start, count)| {
                        position >= *start && position - *start < *count as u64
                    })
                    .map(|x| (i, x))
                })
        {
            self.ages[index] = self.clock;
            let offset = usize::try_from(position - start).map_err(|_| {
                VerifiedGranularStepError::Source(StreamFailureKind::CapacityExhausted)
            })?;
            return self.cache.get(index * self.slot + offset).copied().ok_or(
                VerifiedGranularStepError::Source(StreamFailureKind::CapacityExhausted),
            );
        }
        if check_cancel(self.cancel) {
            return Err(VerifiedGranularStepError::Source(
                StreamFailureKind::Cancelled,
            ));
        }
        let start = position / self.slot as u64 * self.slot as u64;
        let count = usize::try_from((self.plan.output_frames() - start).min(self.slot as u64))
            .map_err(|_| VerifiedGranularStepError::Source(StreamFailureKind::CapacityExhausted))?;
        let (first, last) = self
            .plan
            .required_source_range(start, count)
            .map_err(map_import_error)
            .map_err(VerifiedGranularStepError::Source)?;
        let needed = usize::try_from(last - first)
            .map_err(|_| VerifiedGranularStepError::Source(StreamFailureKind::CapacityExhausted))?;
        if needed > self.source.len() || needed > self.reader.max_window_frames() {
            return Err(VerifiedGranularStepError::Source(
                StreamFailureKind::CapacityExhausted,
            ));
        }
        let index = (0..self.slots)
            .min_by_key(|&i| {
                if self.valid[i].is_none() {
                    0
                } else {
                    self.ages[i]
                }
            })
            .ok_or(VerifiedGranularStepError::Source(
                StreamFailureKind::CapacityExhausted,
            ))?;
        self.valid[index] = None;
        let input = &mut self.source[..needed];
        let read = self
            .reader
            .read_frames_into_with_cancel(first, input, || check_cancel(self.cancel))
            .map_err(map_import_error)
            .map_err(VerifiedGranularStepError::Source)?;
        if read != needed {
            return Err(VerifiedGranularStepError::Source(
                StreamFailureKind::SourceChanged,
            ));
        }
        let offset = index * self.slot;
        let output = &mut self.cache[offset..offset + count];
        self.plan
            .render_window(first, input, start, output)
            .map_err(map_import_error)
            .map_err(VerifiedGranularStepError::Source)?;
        if check_cancel(self.cancel) {
            return Err(VerifiedGranularStepError::Source(
                StreamFailureKind::Cancelled,
            ));
        }
        self.valid[index] = Some((start, count));
        self.ages[index] = self.clock;
        let at = usize::try_from(position - start)
            .map_err(|_| VerifiedGranularStepError::Source(StreamFailureKind::CapacityExhausted))?;
        output
            .get(at)
            .copied()
            .ok_or(VerifiedGranularStepError::Source(
                StreamFailureKind::CapacityExhausted,
            ))
    }
}
impl GranularSource for Source<'_, '_> {
    type Error = VerifiedGranularStepError;
    fn frame(&mut self, position: i128) -> Result<[f64; 2], Self::Error> {
        self.checks = self.checks.wrapping_add(1);
        if self.checks.is_multiple_of(1024) && check_cancel(self.cancel) {
            return Err(VerifiedGranularStepError::Source(
                StreamFailureKind::Cancelled,
            ));
        }
        let (at, gain) = if let Some(region) = self.region {
            let Some(resolved) = region.resolve(position)? else {
                return Ok([0.0; 2]);
            };
            resolved
        } else {
            let Ok(at) = u64::try_from(position) else {
                return Ok([0.0; 2]);
            };
            if at >= self.plan.output_frames() {
                return Ok([0.0; 2]);
            }
            (at, 1.0)
        };
        Ok(self.load(at)?.map(|value| f64::from(value) * gain))
    }
    fn window(&mut self, start: i128, output: &mut [[f64; 2]]) -> Result<(), Self::Error> {
        for (index, frame) in output.iter_mut().enumerate() {
            let position = start
                .checked_add(index as i128)
                .ok_or(Error::Invalid("granular source position overflow"))?;
            *frame = self.frame(position)?;
        }
        Ok(())
    }
}
