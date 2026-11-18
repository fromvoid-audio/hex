//! Pure borrowed demand traversal, adapted from published177 8d444c06.
//! Extents describe required samples; these scalars never certify installed pages.
use crate::{Clip, Invalid, STREAM_PAGE_FRAMES, StreamedPageSpan, StreamedSourceMetadata, Track};

/// Required half-open extent within one project-rate source page. No ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamPageExtent {
    pub source: crate::StreamSourceKey,
    pub page_index: u64,
    pub start: usize,
    pub end: usize,
}

/// Typed union used by one stream-domain reservation. Audio traversal retains
/// its narrow extent type; scalar demand joins this union before readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamResourceExtent {
    pub source: crate::StreamResourceKey,
    pub page_index: u64,
    pub start: usize,
    pub end: usize,
}
impl From<StreamPageExtent> for StreamResourceExtent {
    fn from(extent: StreamPageExtent) -> Self {
        Self {
            source: extent.source.into(),
            page_index: extent.page_index,
            start: extent.start,
            end: extent.end,
        }
    }
}

/// Append exact scalar demand, clipping only known positions outside the
/// immutable source extent. The catalog/page state still decides readiness.
pub fn visit_scalar_page_range(
    source: &crate::ScalarSourceMetadata,
    start: u64,
    count: usize,
    mut visit: impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
) -> Result<(), Invalid> {
    let end = start
        .checked_add(count as u64)
        .ok_or(Invalid("scalar demand range overflow"))?;
    let mut cursor = start.max(source.start_frame());
    let end = end.min(source.end_frame());
    while cursor < end {
        let offset = (cursor % STREAM_PAGE_FRAMES as u64) as usize;
        let count = (end - cursor).min((STREAM_PAGE_FRAMES - offset) as u64) as usize;
        visit(StreamResourceExtent {
            source: source.key().into(),
            page_index: cursor / STREAM_PAGE_FRAMES as u64,
            start: offset,
            end: offset + count,
        })?;
        cursor = cursor
            .checked_add(count as u64)
            .ok_or(Invalid("scalar demand advance overflow"))?;
    }
    Ok(())
}

/// The next render chunk's clocks and width; demand borrows never change them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamChunkWindow {
    pub performance_frame: u64,
    pub arrangement_frame: u64,
    pub route_generation: u64,
    pub frames: usize,
}

pub(crate) fn visit_page_range(
    source: &StreamedSourceMetadata,
    start: u64,
    count: usize,
    visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
) -> Result<(), Invalid> {
    let end = start
        .checked_add(count as u64)
        .ok_or(Invalid("stream demand range overflow"))?;
    if end > source.playback_frames() {
        return Err(Invalid("stream demand exceeds source"));
    }
    let mut cursor = start;
    while cursor < end {
        let offset = (cursor % STREAM_PAGE_FRAMES as u64) as usize;
        let count = (end - cursor).min((STREAM_PAGE_FRAMES - offset) as u64) as usize;
        visit(StreamPageExtent {
            source: source.key(),
            page_index: cursor / STREAM_PAGE_FRAMES as u64,
            start: offset,
            end: offset + count,
        })?;
        cursor += count as u64;
    }
    Ok(())
}

impl Clip {
    // Returning the actual consumed width preserves legacy rendering semantics.
    pub(crate) fn visit_streamed_spans(
        &self,
        start: u64,
        end: u64,
        mut visit: impl FnMut(&StreamedSourceMetadata, StreamedPageSpan<'_>) -> usize,
    ) {
        if !self.active {
            return;
        }
        let Some(metadata) = self.metadata.as_deref() else {
            return;
        };
        let Some(source) = metadata.streamed.as_deref() else {
            return;
        };
        let span_end = if self.looping {
            end.min(self.end)
        } else {
            end.min(self.end)
                .min(self.start.saturating_add(self.source_length as u64))
        };
        let mut position = start.max(self.start);
        while position < span_end {
            let relative = if self.looping {
                (position - self.start) % self.source_length as u64
            } else {
                position - self.start
            };
            let until_phase_wrap = self.source_length as u64 - metadata.source_phase;
            let phase = if relative >= until_phase_wrap {
                relative - until_phase_wrap
            } else {
                relative + metadata.source_phase
            };
            let source_phase = if metadata.reversed {
                self.source_length as u64 - 1 - phase
            } else {
                phase
            };
            let Some(source_frame) = (self.source_offset as u64).checked_add(source_phase) else {
                break;
            };
            let until_wrap = if self.looping || metadata.reversed {
                self.source_length as u64 - phase
            } else {
                u64::MAX
            };
            let page_remaining = if metadata.reversed {
                source_frame % STREAM_PAGE_FRAMES as u64 + 1
            } else {
                STREAM_PAGE_FRAMES as u64 - source_frame % STREAM_PAGE_FRAMES as u64
            };
            let count = (span_end - position).min(until_wrap).min(page_remaining) as usize;
            let consumed = visit(
                source,
                StreamedPageSpan {
                    source_frame,
                    reversed: metadata.reversed,
                    timeline_frame: position,
                    output_offset: (position - start) as usize,
                    count,
                    gain: metadata.gain,
                    fade: metadata.fade.as_ref().map(|fade| (fade, self.start)),
                },
            );
            if consumed == 0 || consumed > count {
                break;
            }
            position = position.saturating_add(consumed as u64);
        }
    }
}

impl Track {
    /// Allocation-probe access to the same bounded page-demand traversal.
    #[cfg(feature = "instrument-test-support")]
    #[doc(hidden)]
    pub fn probe_stream_demand(
        &self,
        start: u64,
        count: usize,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        self.visit_stream_demand(start, count, visit)
    }

    pub(crate) fn visit_stream_demand(
        &self,
        start: u64,
        count: usize,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        let end = start
            .checked_add(count as u64)
            .ok_or(Invalid("stream demand clock overflow"))?;
        let mut result = Ok(());
        crate::clip_index::visit(&self.clip_index, &self.clips, start, end, |clip| {
            if result.is_err() {
                return;
            }
            clip.visit_streamed_spans(start, end, |source, span| {
                let first = if span.reversed {
                    span.source_frame.checked_sub((span.count - 1) as u64)
                } else {
                    Some(span.source_frame)
                };
                result = first
                    .ok_or(Invalid("reverse stream demand underflow"))
                    .and_then(|first| visit_page_range(source, first, span.count, visit));
                if result.is_ok() { span.count } else { 0 }
            });
        });
        result
    }
}
