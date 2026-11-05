//! One bounded scalar job turn; partial pages never enter the ready queue.
use super::*;
use crate::{ResourcePageKey, ScalarSourceDefinition};
use hex_storage::automation_draft::{ProjectedSpanCursor, SpanReadState};

pub(crate) struct ScalarJob {
    pub(crate) source: usize,
    pub(crate) key: ResourcePageKey,
    pub(crate) page: Box<PageBuffer>,
    cursor: ProjectedSpanCursor,
    start: u64,
}
impl ScalarJob {
    pub(crate) fn begin(
        source: usize,
        definition: &mut ScalarSourceDefinition,
        key: ResourcePageKey,
        mut page: Box<PageBuffer>,
    ) -> Result<Self, (Box<PageBuffer>, StreamFailureKind)> {
        let Some(start) = key
            .page_index
            .checked_mul(hex_engine::STREAM_PAGE_FRAMES as u64)
        else {
            return Err((page, StreamFailureKind::DecodeFailed));
        };
        let Some(end) = start.checked_add(hex_engine::STREAM_PAGE_FRAMES as u64) else {
            return Err((page, StreamFailureKind::DecodeFailed));
        };
        if start >= definition.metadata().end_frame()
            || end <= definition.metadata().start_frame()
            || page.scalar_values.len() != hex_engine::STREAM_PAGE_FRAMES
        {
            return Err((page, StreamFailureKind::UnknownSource));
        }
        let cursor = match definition.source.begin_projected_spans_on_control(
            Arc::clone(&definition.projection),
            start..end.min(definition.metadata().end_frame()),
        ) {
            Ok(cursor) => cursor,
            Err(_) => return Err((page, StreamFailureKind::SourceChanged)),
        };
        page.scalar_values.fill(0.0);
        page.scalar_coverage.fill(0);
        page.key = None;
        page.valid_frames = (definition.metadata().end_frame() - start)
            .min(hex_engine::STREAM_PAGE_FRAMES as u64) as usize;
        Ok(Self {
            source,
            key,
            page,
            cursor,
            start,
        })
    }
    /// At most 64 fetched and 64 visited data records plus the source's fixed
    /// integrity overhead. Pending may consume many turns under collisions.
    pub(crate) fn step(
        &mut self,
        definition: &mut ScalarSourceDefinition,
    ) -> Result<bool, StreamFailureKind> {
        let mut spans = [None; 64];
        let progress = definition
            .source
            .read_projected_spans_on_control(
                &definition.projection,
                &mut self.cursor,
                64,
                &mut spans,
            )
            .map_err(|_| StreamFailureKind::SourceChanged)?;
        for span in spans[..progress.written].iter().flatten() {
            let start = usize::try_from(
                span.start
                    .checked_sub(self.start)
                    .ok_or(StreamFailureKind::DecodeFailed)?,
            )
            .map_err(|_| StreamFailureKind::DecodeFailed)?;
            let end = usize::try_from(
                span.end
                    .checked_sub(self.start)
                    .ok_or(StreamFailureKind::DecodeFailed)?,
            )
            .map_err(|_| StreamFailureKind::DecodeFailed)?;
            self.page
                .scalar_values
                .get_mut(start..end)
                .ok_or(StreamFailureKind::DecodeFailed)?
                .fill(span.value.get());
            for offset in start..end {
                *self
                    .page
                    .scalar_coverage
                    .get_mut(offset / 64)
                    .ok_or(StreamFailureKind::DecodeFailed)? |= 1 << (offset % 64);
            }
        }
        if progress.state == SpanReadState::Finished {
            self.page.key = Some(self.key);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
