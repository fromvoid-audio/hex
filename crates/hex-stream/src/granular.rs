//! Borrowed verified source access for one sequential granular grain.
use crate::{
    StreamFailureKind, VerifiedReferenceChannel, reference::validate_source,
    worker::map_import_error,
};
use hex_engine::{Frame, STREAM_PAGE_FRAMES};
use hex_media::{ResamplePlan, VerifiedWavReader};
use hex_warp::{Error, GranularSource, SequentialGranular};
use std::cell::RefCell;

/// Caller-owned source conversion, two-slot cache and grain scratch.
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

/// Advance at most one grain using neutral, full-source verified project-rate audio.
///
/// The caller must bind the renderer to this reference decision, raw asset,
/// generation, conversion and source interval; fingerprints alone do not grant
/// that binding. Source, plan and buffers are borrowed and remain caller-owned.
/// The 32 MiB check covers renderer backing and supplied slice lengths, not
/// larger backing allocations, the reader manifest, plan, or stack. The caller
/// accounts for all retained capacity and other worker memory.
/// Only pages read on this call are verified. A two-slot cache lives only for
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
    let slot = (buffers.cache.len() / 2)
        .min(STREAM_PAGE_FRAMES)
        .min(plan.max_output_window_frames());
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
        valid: [None, None],
        replace: 0,
        checks: 0,
        cancel: &cancel,
    };
    renderer.advance(&mut source, buffers.grain, &mut || check_cancel(&cancel))
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
    valid: [Option<(u64, usize)>; 2],
    replace: usize,
    checks: usize,
    cancel: &'c RefCell<&'c mut dyn FnMut() -> bool>,
}
impl Source<'_, '_> {
    fn load(&mut self, position: u64) -> Result<Frame, VerifiedGranularStepError> {
        if let Some((index, (start, _))) = self.valid.iter().enumerate().find_map(|(i, item)| {
            item.filter(|(start, count)| position >= *start && position - *start < *count as u64)
                .map(|x| (i, x))
        }) {
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
        let index = self.replace;
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
        self.replace ^= 1;
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
        let Ok(at) = u64::try_from(position) else {
            return Ok([0.0; 2]);
        };
        if at >= self.plan.output_frames() {
            return Ok([0.0; 2]);
        }
        Ok(self.load(at)?.map(f64::from))
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
