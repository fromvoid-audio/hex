//! Bounded sequential projected scalar runs. Construction seeks at most three
//! piece predecessors; stepping then reads fixed record batches, never a disk
//! predecessor for each output sample. Every cursor freezes one map Arc and is
//! bound to its original reader. This is a control/offline producer, not a stream
//! lease or callback API. The complete checkpoint/token remain reader-owned;
//! their native resource/peak accounting is deliberately not claimed here.
use super::*;
use hex_project::AutomationValue;

pub const MAX_SPANS_PER_STEP: usize = 64;
pub const MAX_SCALAR_RANGE_FRAMES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectedAutomationSpan {
    pub start: u64,
    pub end: u64,
    pub value: AutomationValue,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanReadState {
    Pending,
    Finished,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanReadProgress {
    pub written: usize,
    /// Data records fetched during this step, excluding at most three fixed
    /// integrity-predecessor reads (one per nonempty piece refill).
    pub records_read: usize,
    /// Cached or newly fetched records processed during this step.
    pub records_visited: usize,
    /// Pending can accompany zero spans when collapsed changes consumed the
    /// input budget. It is never outside coverage or end-of-source.
    pub state: SpanReadState,
}
#[derive(Clone, Copy)]
struct ContentPiece {
    start: u64,
    end: u64,
    performance_start: u64,
}
#[derive(Clone, Copy)]
struct Piece {
    source_start: u64,
    performance_start: u64,
    performance_end: u64,
    next_ordinal: u64,
    held: AutomationValue,
    destination_cursor: u64,
    destination_end: u64,
}
/// Fixed state for one requested destination range. At most three source pieces
/// and 64 prefetched records are retained, independent of duration/pass count.
/// Reader/map identity is checked on every step. No self-reference or joining
/// owner is retained; dropping this cursor never modifies or deletes its file.
pub struct ProjectedSpanCursor {
    owner: Arc<()>,
    destination: Arc<TempoMap>,
    pieces: [Option<Piece>; 3],
    piece_count: usize,
    piece_index: usize,
    batch: [Option<AppliedEvent>; MAX_APPEND_EVENTS],
    batch_count: usize,
    batch_at: usize,
    failed: bool,
}
impl ClockMapping {
    fn pieces(self) -> Result<[Option<ContentPiece>; 3], String> {
        let mut output = [None; 3];
        if self.first.performance_frame == self.end.performance_frame {
            return Ok(output);
        }
        let Some(region) = self.region else {
            output[0] = Some(ContentPiece {
                start: self.first.content_frame,
                end: self.end.content_frame,
                performance_start: self.first.performance_frame,
            });
            return Ok(output);
        };
        let mut boundaries = [
            self.first.content_frame,
            self.end.content_frame,
            region.start(),
            region.end(),
        ];
        boundaries.sort_unstable();
        let mut count = 0;
        for pair in boundaries.windows(2) {
            let [start, end] = [pair[0], pair[1]];
            if start == end {
                continue;
            }
            if let Some(performance_start) = self.latest_at(start) {
                let last = self
                    .latest_at(end - 1)
                    .ok_or("automation coverage piece lost its end")?;
                if performance_start.checked_add(end - start - 1) != Some(last) {
                    return Err("automation coverage piece is not continuous".into());
                }
                let slot = output
                    .get_mut(count)
                    .ok_or("automation coverage has too many pieces")?;
                *slot = Some(ContentPiece {
                    start,
                    end,
                    performance_start,
                });
                count += 1;
            }
        }
        Ok(output)
    }
}
impl<R: TakeReader> TakeCore<R> {
    /// Construction performs at most three logarithmic predecessor seeks and
    /// forward range-location searches. The source has already been fully
    /// validated by open. Stepping has a separate explicit sequential budget.
    pub fn begin_projected_spans_on_control(
        &mut self,
        destination: Arc<TempoMap>,
        range: Range<u64>,
    ) -> Result<ProjectedSpanCursor, String> {
        if range.start >= range.end {
            return Err("scalar range must be nonempty".into());
        }
        self.projected_coverage(&destination)?;
        let mut pieces = [None; 3];
        let mut count = 0;
        let source_pieces = self
            .mapping
            .map(ClockMapping::pieces)
            .transpose()?
            .unwrap_or([None; 3]);
        for source_piece in source_pieces.into_iter().flatten() {
            let (start, end, first_source) = {
                let projection =
                    CapturedAutomationProjection::new(self.captured_tempo_map(), &destination);
                let placed = projection
                    .project_range(source_piece.start..source_piece.end)
                    .map_err(|e| e.to_string())?;
                let start = placed.start.max(range.start);
                let end = placed.end.min(range.end);
                if start >= end {
                    continue;
                }
                let first_source = projection
                    .source_frame_at(start, source_piece.start..source_piece.end)
                    .map_err(|e| e.to_string())?
                    .ok_or("projected piece has no initial source frame")?;
                (start, end, first_source)
            };
            let performance = source_piece
                .performance_start
                .checked_add(first_source - source_piece.start)
                .ok_or("automation piece clock overflow")?;
            let (ordinal, event) = self
                .source
                .event_with_ordinal_at_or_before(performance)
                .map_err(|e| e.to_string())?
                .ok_or("automation piece has no held event")?;
            let piece = Piece {
                source_start: source_piece.start,
                performance_start: source_piece.performance_start,
                performance_end: source_piece
                    .performance_start
                    .checked_add(source_piece.end - source_piece.start)
                    .ok_or("automation piece end overflow")?,
                next_ordinal: ordinal
                    .checked_add(1)
                    .ok_or("automation event ordinal overflow")?,
                held: event.value,
                destination_cursor: start,
                destination_end: end,
            };
            *pieces
                .get_mut(count)
                .ok_or("automation projection has too many pieces")? = Some(piece);
            count += 1;
        }
        Ok(ProjectedSpanCursor {
            owner: Arc::clone(&self.cursor_owner),
            destination,
            pieces,
            piece_count: count,
            piece_index: 0,
            batch: [None; MAX_APPEND_EVENTS],
            batch_count: 0,
            batch_at: 0,
            failed: false,
        })
    }

    /// Fetch AND visit at most input_budget (1..64) data records and emit at
    /// most output.len() (1..64) spans. Refills are charged in full even when a
    /// piece sentinel discards unread prefetch. Each refill also has at most
    /// one fixed integrity predecessor (at most three per step). Cached records
    /// count as visited work. At most three empty-piece transitions occur.
    /// Foreign source/map calls refuse without changing an otherwise valid
    /// cursor. A genuine read/conversion failure seals it and clears this batch.
    pub fn read_projected_spans_on_control(
        &mut self,
        destination: &Arc<TempoMap>,
        cursor: &mut ProjectedSpanCursor,
        input_budget: usize,
        output: &mut [Option<ProjectedAutomationSpan>],
    ) -> Result<SpanReadProgress, String> {
        output.fill(None);
        if !Arc::ptr_eq(&cursor.owner, &self.cursor_owner)
            || !Arc::ptr_eq(&cursor.destination, destination)
        {
            return Err("automation cursor belongs to another source or projection".into());
        }
        if !(1..=MAX_APPEND_EVENTS).contains(&input_budget)
            || !(1..=MAX_SPANS_PER_STEP).contains(&output.len())
        {
            return Err("automation span step needs input/output budgets within 1..64".into());
        }
        if cursor.failed {
            return Err(
                "automation span cursor failed; retain the source and start a fresh cursor".into(),
            );
        }
        let result = self.read_span_step(cursor, input_budget, output);
        if result.is_err() {
            output.fill(None);
            cursor.failed = true;
        }
        result
    }

    fn read_span_step(
        &mut self,
        cursor: &mut ProjectedSpanCursor,
        input_budget: usize,
        output: &mut [Option<ProjectedAutomationSpan>],
    ) -> Result<SpanReadProgress, String> {
        let mut written = 0;
        let mut work = 0;
        let mut records_read = 0;
        while written < output.len() && cursor.piece_index < cursor.piece_count {
            let index = cursor.piece_index;
            let mut piece = cursor
                .pieces
                .get(index)
                .copied()
                .flatten()
                .ok_or("automation cursor piece is unavailable")?;
            if piece.destination_cursor >= piece.destination_end {
                cursor.piece_index += 1;
                cursor.batch_at = 0;
                cursor.batch_count = 0;
                continue;
            }
            if work == input_budget {
                break;
            }
            if cursor.batch_at == cursor.batch_count {
                if records_read == input_budget {
                    break;
                }
                let capacity = (input_budget - work)
                    .min(input_budget - records_read)
                    .min(MAX_APPEND_EVENTS);
                cursor.batch_count = self
                    .source
                    .read_events(piece.next_ordinal, &mut cursor.batch[..capacity])
                    .map_err(|e| e.to_string())?;
                records_read += cursor.batch_count;
                cursor.batch_at = 0;
            }
            let event = if cursor.batch_at < cursor.batch_count {
                let event = cursor
                    .batch
                    .get(cursor.batch_at)
                    .copied()
                    .flatten()
                    .ok_or("automation cursor record is unavailable")?;
                cursor.batch_at += 1;
                piece.next_ordinal = piece
                    .next_ordinal
                    .checked_add(1)
                    .ok_or("automation event ordinal overflow")?;
                work += 1;
                Some(event)
            } else {
                None
            };
            let change = match event {
                Some(event) if event.performance_frame < piece.performance_end => {
                    let delta = event
                        .performance_frame
                        .checked_sub(piece.performance_start)
                        .ok_or("automation piece event precedes its origin")?;
                    let content = piece
                        .source_start
                        .checked_add(delta)
                        .ok_or("automation piece content overflow")?;
                    let at = CapturedAutomationProjection::new(
                        self.captured_tempo_map(),
                        &cursor.destination,
                    )
                    .project_frame(content)
                    .map_err(|e| e.to_string())?;
                    Some((at, event.value))
                }
                _ => None,
            };
            let end = change.map_or(piece.destination_end, |(at, _)| {
                at.min(piece.destination_end)
            });
            if end > piece.destination_cursor {
                *output
                    .get_mut(written)
                    .ok_or("automation span output is unavailable")? =
                    Some(ProjectedAutomationSpan {
                        start: piece.destination_cursor,
                        end,
                        value: piece.held,
                    });
                written += 1;
                piece.destination_cursor = end;
            }
            if let Some((at, value)) = change {
                if at < piece.destination_end {
                    piece.held = value;
                }
            } else {
                piece.destination_cursor = piece.destination_end;
            }
            *cursor
                .pieces
                .get_mut(index)
                .ok_or("automation cursor piece is unavailable")? = Some(piece);
        }
        // Retire a completed last piece without requiring another record read.
        while cursor.piece_index < cursor.piece_count {
            let complete = cursor
                .pieces
                .get(cursor.piece_index)
                .copied()
                .flatten()
                .is_some_and(|p| p.destination_cursor == p.destination_end);
            if !complete {
                break;
            }
            cursor.piece_index += 1;
            cursor.batch_at = 0;
            cursor.batch_count = 0;
        }
        Ok(SpanReadProgress {
            written,
            records_read,
            records_visited: work,
            state: if cursor.piece_index == cursor.piece_count {
                SpanReadState::Finished
            } else {
                SpanReadState::Pending
            },
        })
    }

    /// Convenience control/offline consumer. It may drive many bounded steps
    /// under dense collisions; a 4096-frame output limit is NOT a total raw-work
    /// bound. A future worker must retain the cursor and yield between steps.
    /// Output becomes usable only on Ok; every failure clears the entire range.
    pub fn read_projected_values_on_control(
        &mut self,
        destination: Arc<TempoMap>,
        start: u64,
        output: &mut [Option<AutomationValue>],
    ) -> Result<(), String> {
        output.fill(None);
        if output.len() > MAX_SCALAR_RANGE_FRAMES {
            return Err("scalar range exceeds 4096 output frames".into());
        }
        if output.is_empty() {
            return Ok(());
        }
        let end = start
            .checked_add(output.len() as u64)
            .ok_or("scalar range frame overflow")?;
        let result = (|| {
            let mut cursor =
                self.begin_projected_spans_on_control(Arc::clone(&destination), start..end)?;
            let mut spans = [None; MAX_SPANS_PER_STEP];
            loop {
                let progress = self.read_projected_spans_on_control(
                    &destination,
                    &mut cursor,
                    MAX_APPEND_EVENTS,
                    &mut spans,
                )?;
                for span in spans[..progress.written].iter().flatten() {
                    let first = usize::try_from(span.start - start)
                        .map_err(|_| "scalar range index overflow")?;
                    let last = usize::try_from(span.end - start)
                        .map_err(|_| "scalar range index overflow")?;
                    output
                        .get_mut(first..last)
                        .ok_or("scalar span lies outside its output range")?
                        .fill(Some(span.value));
                }
                if progress.state == SpanReadState::Finished {
                    return Ok(());
                }
            }
        })();
        if result.is_err() {
            output.fill(None);
        }
        result
    }
}
