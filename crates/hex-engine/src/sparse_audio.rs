//! Immutable root coordinates for packed, nonoverlapping resident event PCM.
//! A gap consumes no PCM. Readers borrow slices in root order, and the
//! layout and packed PCM are retained and retired with the prepared clip.
use crate::Invalid;
use std::ops::Range;

#[derive(Debug, PartialEq, Eq)]
pub struct SparseFrameMap {
    extent: usize,
    packed_len: usize,
    spans: Vec<Span>,
}

#[derive(Debug, PartialEq, Eq)]
struct Span {
    start: usize,
    end: usize,
    packed_start: usize,
}

impl SparseFrameMap {
    /// Select sparse packing only when it saves at least one sizeable block.
    /// The canonical event inventory is already validated and in root order.
    pub(crate) fn for_events(
        extent: u64,
        events: &[hex_project::AudioEvent],
    ) -> Result<Option<Self>, Invalid> {
        let extent =
            usize::try_from(extent).map_err(|_| Invalid("sparse audio extent overflow"))?;
        let mut packed_len = 0usize;
        let mut count = 0;
        for event in events.iter().filter(|event| event.active) {
            packed_len = packed_len
                .checked_add(
                    usize::try_from(event.length())
                        .map_err(|_| Invalid("sparse event length overflow"))?,
                )
                .ok_or(Invalid("sparse audio length overflow"))?;
            count += 1;
        }
        if packed_len == 0 || extent.saturating_sub(packed_len) < 8_192 {
            return Ok(None);
        }
        let mut spans = Vec::new();
        spans
            .try_reserve_exact(count)
            .map_err(|_| Invalid("sparse audio layout allocation failed"))?;
        let mut packed_start = 0;
        for event in events.iter().filter(|event| event.active) {
            let start =
                usize::try_from(event.start).map_err(|_| Invalid("sparse event start overflow"))?;
            let end =
                usize::try_from(event.end).map_err(|_| Invalid("sparse event end overflow"))?;
            spans.push(Span {
                start,
                end,
                packed_start,
            });
            packed_start += end - start;
        }
        Ok(Some(Self {
            extent,
            packed_len,
            spans,
        }))
    }

    pub fn extent(&self) -> usize {
        self.extent
    }

    pub fn packed_len(&self) -> usize {
        self.packed_len
    }

    pub(crate) fn allocation_bytes(&self) -> usize {
        self.spans.capacity() * size_of::<Span>() + size_of::<Self>()
    }

    pub(crate) fn packed_index(&self, root: usize) -> Option<usize> {
        let span = self
            .spans
            .get(self.spans.partition_point(|span| span.end <= root))?;
        Some(span.packed_start + root.checked_sub(span.start)?)
    }

    /// Visit only occupied intersections. Ranges use logical root coordinates
    /// and packed PCM indices respectively. Cost depends on spans, not gap length.
    pub fn visit(&self, root: Range<usize>, mut visit: impl FnMut(Range<usize>, Range<usize>)) {
        if root.is_empty() {
            return;
        }
        let first = self.spans.partition_point(|span| span.end <= root.start);
        for span in &self.spans[first..] {
            let start = span.start.max(root.start);
            let end = span.end.min(root.end);
            if start >= end {
                break;
            }
            let packed = span.packed_start + (start - span.start);
            visit(start..end, packed..packed + (end - start));
        }
    }
}
