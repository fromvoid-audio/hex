//! Read-only control-side MIDI gate observations from an applied prepared graph.
use crate::{Invalid, MidiSpanBuffer, PreparedRevision, SessionPlaybackOwner};
use hex_project::{BeatTime, TrackId};

impl PreparedRevision {
    /// Stable track whose applied transport owner drives this instrument's MIDI.
    pub fn midi_activity_route_track(&self, track: TrackId) -> Result<TrackId, Invalid> {
        let tracks = self.arrangement().tracks();
        let compiled = tracks
            .iter()
            .find(|candidate| candidate.canonical_track_id() == Some(track))
            .ok_or(Invalid("midi activity track is unavailable"))?;
        match compiled.midi_source_index {
            Some(index) => tracks
                .get(index)
                .and_then(|source| source.canonical_track_id())
                .ok_or(Invalid("midi activity source track is unavailable")),
            None => Ok(track),
        }
    }

    /// Query every admitted gate at one transport cursor, rather than the
    /// drawing preview's deliberately sampled subset. This does not measure
    /// acoustic loudness or synth release tails. Caller supplies reusable fixed
    /// storage and must correlate routes/cursors with this exact applied graph.
    pub fn midi_gate_keys(
        &self,
        track: TrackId,
        owner: SessionPlaybackOwner,
        launch: Option<BeatTime>,
        cursors: (u64, u64),
        spans: &mut MidiSpanBuffer,
    ) -> Result<[u64; 2], Invalid> {
        let (arrangement_frame, performance_frame) = cursors;
        let mut keys = [0; 2];
        let mut mark = |pitch: u8| {
            keys[usize::from(pitch) / 64] |= 1_u64 << (pitch % 64);
        };
        match owner {
            SessionPlaybackOwner::Arrangement => {
                let compiled = self
                    .arrangement()
                    .tracks()
                    .iter()
                    .find(|compiled| compiled.canonical_track_id() == Some(track))
                    .ok_or(Invalid("midi activity track is unavailable"))?;
                if let Some(instrument) = &compiled.instrument {
                    let end = arrangement_frame
                        .checked_add(1)
                        .ok_or(Invalid("midi activity frame exhausted"))?;
                    instrument.for_each_intersecting(arrangement_frame, end, None, |span| {
                        if span.start <= arrangement_frame && arrangement_frame < span.gate_end {
                            mark(span.pitch);
                        }
                    });
                }
            }
            SessionPlaybackOwner::Session { clip } => {
                let inventory = self.session();
                let source = inventory
                    .midi_clip_for_track(clip, track)
                    .ok_or(Invalid("midi activity source is unavailable"))?;
                if source.instrument_enabled() {
                    let launch = launch.ok_or(Invalid("midi activity launch is unavailable"))?;
                    if inventory.periodic_tempo_error().is_some() {
                        return Err(Invalid("midi activity tempo is unavailable"));
                    }
                    let timing = inventory.periodic_tempo().map_or(
                        crate::prepared_tempo::SessionTimingView::Canonical(inventory.timing()),
                        |periodic| {
                            crate::prepared_tempo::SessionTimingView::Periodic(
                                periodic.view(inventory.timing()),
                            )
                        },
                    );
                    source
                        .instrument_template()
                        .query(
                            launch,
                            &timing,
                            performance_frame,
                            performance_frame
                                .checked_add(1)
                                .ok_or(Invalid("midi activity frame exhausted"))?,
                            spans,
                        )
                        .map_err(|_| Invalid("midi activity query was refused"))?;
                    for span in spans.spans() {
                        if span.start_frame <= performance_frame
                            && performance_frame < span.gate_end_frame
                        {
                            mark(span.pitch);
                        }
                    }
                }
            }
            SessionPlaybackOwner::Silence => {}
        }
        Ok(keys)
    }
}

// The projection reports the exact same admitted scalar interval/index data as playback.
pub(crate) fn visit_arrangement_spans(
    spans: &[crate::midi_compile::CompiledNoteSpan],
    nodes: &[crate::midi_compile::SpanNode],
    root: Option<usize>,
    start: u64,
    end: u64,
    minimum_start_exclusive: Option<u64>,
    mut visit: impl FnMut(&crate::midi_compile::CompiledNoteSpan),
) {
    let Some(root) = root else { return };
    let mut stack = [(0_usize, false); 64];
    let mut size = 1;
    stack[0] = (root, false);
    while size > 0 {
        size -= 1;
        let (index, emit) = stack[size];
        let node = &nodes[index];
        if emit {
            let span = &spans[node.span_index];
            if span.start < end
                && span.end > start
                && minimum_start_exclusive.is_none_or(|floor| span.start > floor)
            {
                visit(span);
            }
            continue;
        }
        // A median onset beyond the query end only rules out this node and
        // its right subtree. Earlier left-subtree spans may still be active.
        if node.max_end > start {
            if let Some(right) = node.right {
                if node.span_start < end && nodes[right].max_end > start {
                    stack[size] = (right, false);
                    size += 1;
                }
            }
            if node.span_start < end && spans[node.span_index].end > start {
                stack[size] = (index, true);
                size += 1;
            }
            if let Some(left) = node.left {
                if nodes[left].max_end > start {
                    stack[size] = (left, false);
                    size += 1;
                }
            }
        }
    }
}
