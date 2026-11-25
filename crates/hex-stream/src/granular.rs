//! Borrowed verified source access for one sequential granular grain.
use crate::{
    StreamFailureKind, VerifiedReferenceChannel, reference::validate_source,
    worker::map_import_error,
};
use hex_engine::{Frame, STREAM_PAGE_FRAMES, StreamedSourceMetadata};
use hex_media::{ResamplePlan, VerifiedWavReader};
use hex_warp::{
    Error, GranularSource, ReferenceChannelScan, SequentialGranular, SequentialWarp, SourceRegion,
};
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
/// on source replacement. After partial failure, discard a poisoned renderer or
/// restore an instance-bound checkpoint only after revalidating source identity.
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

/// Advance at most one bounded warp step (`Tones`, `Texture`, `Beats`, `Complex`,
/// or `ComplexPro`) using verified project-rate audio.
pub fn advance_verified_warp_on_control(
    renderer: &mut SequentialWarp,
    reference: &VerifiedReferenceChannel,
    reader: &mut VerifiedWavReader,
    plan: &ResamplePlan,
    buffers: VerifiedGranularBuffers<'_>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<bool, VerifiedGranularStepError> {
    validate_source(reference.metadata(), reader, plan)
        .map_err(VerifiedGranularStepError::Source)?;
    let (rate, required_channel) = renderer.source_parameters();
    if rate != reference.metadata().project_rate()
        || required_channel.is_some_and(|ch| ch != reference.channel())
    {
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
pub(crate) fn region_min_slot(rate: u32) -> usize {
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
    fn load_slot(&mut self, position: u64) -> Result<(usize, usize), VerifiedGranularStepError> {
        self.clock = self.clock.saturating_add(1);
        if let Some((index, (start, count))) =
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
            let avail = count.saturating_sub(offset);
            let cache_offset = index * self.slot + offset;
            if cache_offset + avail <= self.cache.len() {
                return Ok((cache_offset, avail));
            }
            return Err(VerifiedGranularStepError::Source(
                StreamFailureKind::CapacityExhausted,
            ));
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
        let avail = count.saturating_sub(at);
        if offset + at + avail <= self.cache.len() {
            Ok((offset + at, avail))
        } else {
            Err(VerifiedGranularStepError::Source(
                StreamFailureKind::CapacityExhausted,
            ))
        }
    }

    fn load(&mut self, position: u64) -> Result<Frame, VerifiedGranularStepError> {
        let (cache_offset, _) = self.load_slot(position)?;
        self.cache
            .get(cache_offset)
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
        if output.is_empty() {
            return Ok(());
        }
        // Validate end position upfront to match sample-by-sample overflow behavior.
        start
            .checked_add((output.len() - 1) as i128)
            .ok_or(Error::Invalid("granular source position overflow"))?;
        let mut cursor = 0usize;
        while cursor < output.len() {
            let prev_bucket = self.checks / 1024;
            let pos = start + cursor as i128;
            let remaining = output.len() - cursor;
            if let Some(region) = self.region {
                let shifted = pos
                    .checked_add(i128::from(region.phase()))
                    .ok_or(Error::Invalid("source region position overflow"))?;
                let length = region.length();
                let local = if region.looping() {
                    match (i64::try_from(shifted), i64::try_from(length)) {
                        (Ok(p), Ok(l)) => p.rem_euclid(l) as u64,
                        _ => shifted.rem_euclid(i128::from(length)) as u64,
                    }
                } else if shifted < 0 {
                    let zeros =
                        usize::try_from((-shifted).min(remaining as i128)).unwrap_or(remaining);
                    output[cursor..cursor + zeros].fill([0.0; 2]);
                    self.checks = self.checks.wrapping_add(zeros);
                    if self.checks / 1024 != prev_bucket && check_cancel(self.cancel) {
                        return Err(VerifiedGranularStepError::Source(
                            StreamFailureKind::Cancelled,
                        ));
                    }
                    cursor += zeros;
                    continue;
                } else if shifted >= i128::from(length) {
                    output[cursor..].fill([0.0; 2]);
                    self.checks = self.checks.wrapping_add(remaining);
                    if self.checks / 1024 != prev_bucket && check_cancel(self.cancel) {
                        return Err(VerifiedGranularStepError::Source(
                            StreamFailureKind::Cancelled,
                        ));
                    }
                    break;
                } else {
                    shifted as u64
                };
                let region_rem = usize::try_from(length - local).unwrap_or(remaining);
                let source_idx = region
                    .start()
                    .checked_add(local)
                    .ok_or(Error::Invalid("source region index overflow"))?;
                let (cache_off, avail) = self.load_slot(source_idx)?;
                let run = remaining.min(region_rem).min(avail).min(1024);
                self.checks = self.checks.wrapping_add(run);
                if self.checks / 1024 != prev_bucket && check_cancel(self.cancel) {
                    return Err(VerifiedGranularStepError::Source(
                        StreamFailureKind::Cancelled,
                    ));
                }
                let src_slice = &self.cache[cache_off..cache_off + run];
                let dst_slice = &mut output[cursor..cursor + run];
                let envelope = region.envelope();
                if envelope.is_empty() {
                    for (dst, &src) in dst_slice.iter_mut().zip(src_slice) {
                        *dst = [f64::from(src[0]), f64::from(src[1])];
                    }
                } else {
                    for (k, (dst, &src)) in dst_slice.iter_mut().zip(src_slice).enumerate() {
                        let gain = region.gain_at(source_idx + k as u64);
                        *dst = [f64::from(src[0]) * gain, f64::from(src[1]) * gain];
                    }
                }
                cursor += run;
            } else {
                let total_frames = self.plan.output_frames();
                if pos < 0 {
                    let zeros = usize::try_from((-pos).min(remaining as i128)).unwrap_or(remaining);
                    output[cursor..cursor + zeros].fill([0.0; 2]);
                    self.checks = self.checks.wrapping_add(zeros);
                    if self.checks / 1024 != prev_bucket && check_cancel(self.cancel) {
                        return Err(VerifiedGranularStepError::Source(
                            StreamFailureKind::Cancelled,
                        ));
                    }
                    cursor += zeros;
                    continue;
                }
                let Ok(at) = u64::try_from(pos) else {
                    output[cursor..].fill([0.0; 2]);
                    break;
                };
                if at >= total_frames {
                    output[cursor..].fill([0.0; 2]);
                    self.checks = self.checks.wrapping_add(remaining);
                    if self.checks / 1024 != prev_bucket && check_cancel(self.cancel) {
                        return Err(VerifiedGranularStepError::Source(
                            StreamFailureKind::Cancelled,
                        ));
                    }
                    break;
                }
                let (cache_off, avail) = self.load_slot(at)?;
                let run = remaining.min(avail).min(1024);
                self.checks = self.checks.wrapping_add(run);
                if self.checks / 1024 != prev_bucket && check_cancel(self.cancel) {
                    return Err(VerifiedGranularStepError::Source(
                        StreamFailureKind::Cancelled,
                    ));
                }
                let src_slice = &self.cache[cache_off..cache_off + run];
                let dst_slice = &mut output[cursor..cursor + run];
                for (dst, &src) in dst_slice.iter_mut().zip(src_slice) {
                    *dst = [f64::from(src[0]), f64::from(src[1])];
                }
                cursor += run;
            }
        }
        Ok(())
    }
}
