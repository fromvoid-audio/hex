//! Read-only control-side MIDI gate observations from an applied prepared graph.
use crate::{Invalid, MidiSpanBuffer, PreparedRevision, SessionPlaybackOwner};
use hex_project::{BeatTime, TrackId};

impl PreparedRevision {
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
