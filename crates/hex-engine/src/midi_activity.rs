//! Read-only control-side MIDI gate observations from an applied prepared graph.
use crate::{
    Invalid, MidiSpanBuffer, PreparedRevision, SessionClockAnchor, SessionMidiOrigin,
    SessionPlaybackOwner,
};
use hex_project::TrackId;

impl PreparedRevision {
    /// Convert an accepted performance cursor through the existing applied
    /// Session clock, including its prepared-revision translation.
    pub fn midi_performance_beat_at(
        &self,
        frame: u64,
        clock_anchor: Option<SessionClockAnchor>,
    ) -> Result<hex_project::BeatTime, Invalid> {
        use crate::MidiTiming;
        let inventory = self.session();
        if inventory.periodic_tempo_error().is_some() {
            return Err(Invalid("midi activity tempo is unavailable"));
        }
        crate::session_playback::timing_view_for(
            inventory,
            inventory.periodic_tempo(),
            clock_anchor,
        )
        .frame_to_beat(frame)
        .map_err(|_| Invalid("midi activity performance beat is unavailable"))
    }

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
        origin: Option<SessionMidiOrigin>,
        clock_anchor: Option<SessionClockAnchor>,
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
                    let origin = origin.ok_or(Invalid("midi activity origin is unavailable"))?;
                    if inventory.periodic_tempo_error().is_some() {
                        return Err(Invalid("midi activity tempo is unavailable"));
                    }
                    let timing = crate::session_playback::timing_view_for(
                        inventory,
                        inventory.periodic_tempo(),
                        clock_anchor,
                    );
                    source
                        .instrument_template()
                        .query_with_origin(
                            origin,
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
    // The prepared tree is median-split, so one pending ancestor per level
    // fits in the number of bits needed to index its spans.
    let mut stack = [&nodes[root]; usize::BITS as usize];
    let mut pending = 0;
    let mut current = Some(root);
    loop {
        while let Some(index) = current {
            let node = &nodes[index];
            current = if node.max_end <= start {
                None
            } else if minimum_start_exclusive.is_some_and(|floor| node.span_start <= floor) {
                // The left subtree cannot have a later onset than this node.
                node.right
            } else if node.span_start >= end {
                node.left
            } else {
                stack[pending] = node;
                pending += 1;
                node.left
            };
        }
        if pending == 0 {
            return;
        }
        pending -= 1;
        let node = stack[pending];
        let span = &spans[node.span_index];
        if span.end > start {
            visit(span);
        }
        current = node.right;
    }
}
