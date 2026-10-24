//! A source-elapsed origin on the existing global performance clock. The anchor
//! is also an inclusive no-chase floor: historical onsets are never projected.
use super::MidiTemplateError;
use hex_project::BeatTime;

/// Source elapsed time anchored to the unchanged global performance clock.
/// Onsets before `elapsed_at_anchor` are excluded; equality is included. The
/// two coordinates are independent, so a cue need not fit before global zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionMidiOrigin {
    /// Global performance beat at which the cue takes effect.
    pub performance_anchor: BeatTime,
    /// Source elapsed beat at that anchor and the inclusive no-chase floor.
    pub elapsed_at_anchor: BeatTime,
}
impl SessionMidiOrigin {
    /// Decode bounded scalar telemetry without adding a project-model dependency
    /// to its transport owner. Every u64 is a representable Q32.32 beat.
    pub const fn from_raw(performance_anchor_raw: u64, elapsed_at_anchor_raw: u64) -> Self {
        Self::new(
            BeatTime::from_raw(performance_anchor_raw),
            BeatTime::from_raw(elapsed_at_anchor_raw),
        )
    }

    /// Both coordinates are already validated Q32.32 beats; neither must precede the other.
    pub const fn new(performance_anchor: BeatTime, elapsed_at_anchor: BeatTime) -> Self {
        Self {
            performance_anchor,
            elapsed_at_anchor,
        }
    }
    /// An ordinary launch starts at elapsed zero on the performance clock.
    pub const fn launch(launch: BeatTime) -> Self {
        Self::new(launch, BeatTime::ZERO)
    }
    /// Source elapsed time at or after the performance anchor. Refuse historical
    /// positions and exhausted Q32.32 coordinates instead of wrapping or clamping.
    pub fn elapsed_at(self, performance: BeatTime) -> Option<BeatTime> {
        performance
            .raw()
            .checked_sub(self.performance_anchor.raw())?
            .checked_add(self.elapsed_at_anchor.raw())
            .map(BeatTime::from_raw)
    }
    pub(crate) fn project(self, elapsed: u128) -> Result<Option<u64>, MidiTemplateError> {
        if elapsed > u128::from(u64::MAX) {
            return Err(MidiTemplateError::BeatDomainExhausted);
        }
        let Some(delta) = elapsed.checked_sub(u128::from(self.elapsed_at_anchor.raw())) else {
            return Ok(None);
        };
        let absolute = u128::from(self.performance_anchor.raw())
            .checked_add(delta)
            .ok_or(MidiTemplateError::BeatDomainExhausted)?;
        u64::try_from(absolute)
            .map(Some)
            .map_err(|_| MidiTemplateError::BeatDomainExhausted)
    }
    /// Invert GLOBAL timing bounds into elapsed coordinates for the existing
    /// indexes. Bounds before the anchor clip to the inclusive no-chase floor.
    pub(super) fn query_bounds(
        self,
        lower: u128,
        upper: u128,
    ) -> Result<Option<(u128, u128)>, MidiTemplateError> {
        let anchor = u128::from(self.performance_anchor.raw());
        if upper <= anchor {
            return Ok(None);
        }
        let floor = u128::from(self.elapsed_at_anchor.raw());
        let first = floor
            .checked_add(lower.saturating_sub(anchor))
            .ok_or(MidiTemplateError::BeatDomainExhausted)?;
        let last = floor
            .checked_add(upper.saturating_sub(anchor))
            .ok_or(MidiTemplateError::BeatDomainExhausted)?;
        if first >= last {
            return Ok(None);
        }
        if first > u128::from(u64::MAX) || last > u128::from(u64::MAX) + 1 {
            return Err(MidiTemplateError::BeatDomainExhausted);
        }
        Ok(Some((first, last)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MidiSpanBuffer, PreparedMidiInstrumentTemplate, PreparedMidiSourceTemplate,
        PreparedTempoMap,
    };
    use hex_project::{Command, Created, Editor, Project};

    fn beat(value: u64) -> BeatTime {
        BeatTime::from_quarters(value).unwrap()
    }

    #[test]
    fn checked_origin_mapping_keeps_inclusive_floor_and_refuses_domain_exhaustion() {
        let origin = SessionMidiOrigin::new(beat(1), beat(10));
        assert_eq!(origin.elapsed_at(BeatTime::ZERO), None);
        assert_eq!(origin.elapsed_at(beat(1)), Some(beat(10)));
        assert_eq!(origin.elapsed_at(beat(3)), Some(beat(12)));
        assert_eq!(origin.project(u128::from(beat(9).raw())), Ok(None));
        assert_eq!(
            origin.project(u128::from(beat(10).raw())),
            Ok(Some(beat(1).raw()))
        );
        assert_eq!(origin.query_bounds(0, u128::from(beat(1).raw())), Ok(None));
        assert_eq!(
            origin.query_bounds(0, u128::from(beat(2).raw())),
            Ok(Some((
                u128::from(beat(10).raw()),
                u128::from(beat(11).raw())
            )))
        );
        let last = SessionMidiOrigin::new(BeatTime::ZERO, BeatTime::from_raw(u64::MAX));
        assert_eq!(last.elapsed_at(BeatTime::from_raw(1)), None);
        assert_eq!(last.project(u128::from(u64::MAX)), Ok(Some(0)));
        assert_eq!(
            last.project(u128::from(u64::MAX) + 1),
            Err(MidiTemplateError::BeatDomainExhausted)
        );
        assert_eq!(
            last.query_bounds(0, 1),
            Ok(Some((u128::from(u64::MAX), u128::from(u64::MAX) + 1)))
        );
        assert_eq!(
            last.query_bounds(0, 2),
            Err(MidiTemplateError::BeatDomainExhausted)
        );
        assert_eq!(
            origin.query_bounds(u128::MAX - 1, u128::MAX),
            Err(MidiTemplateError::BeatDomainExhausted)
        );
        let exhausted_performance = SessionMidiOrigin::launch(BeatTime::from_raw(u64::MAX));
        assert_eq!(
            exhausted_performance.project(1),
            Err(MidiTemplateError::BeatDomainExhausted)
        );
    }

    #[test]
    fn captured_resource_free_gates_and_handoff_occurrence_share_global_cue_identity() {
        let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "cue".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: beat(16),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let mut notes = Vec::new();
        for (start, duration, pitch) in [(BeatTime::ZERO, beat(12), 60), (beat(10), beat(1), 64)] {
            let Created::Note(note) = editor
                .execute(Command::AddMidiNote {
                    clip,
                    start,
                    duration,
                    pitch,
                    channel: 0,
                    velocity: 93,
                })
                .unwrap()
            else {
                panic!("note")
            };
            notes.push(note);
        }
        let timing =
            PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8_000).unwrap();
        let source = PreparedMidiSourceTemplate::prepare(
            track,
            editor.project().midi_clip(clip).unwrap(),
            &timing,
        )
        .unwrap();
        let table = hex_devices::SineTable::new();
        let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
        let gates = instrument.capture_gate_template().unwrap();
        let origin = SessionMidiOrigin::new(beat(1), beat(10));
        let start = timing.beat_to_frame(beat(1)).unwrap();
        assert!(source.occurrence_with_origin(notes[0], 0, origin).is_none());
        let occurrence = source.occurrence_with_origin(notes[1], 0, origin).unwrap();
        assert_eq!(
            (occurrence.onset, occurrence.gate_end, occurrence.velocity),
            (beat(1), beat(2), 93)
        );
        let mut spans = MidiSpanBuffer::default();
        gates
            .query_with_origin(origin, &timing, start, start + 1, &mut spans)
            .unwrap();
        let gate = spans.spans().next().unwrap();
        assert_eq!(
            (
                gate.note,
                gate.onset_beat,
                gate.gate_end_beat,
                gate.velocity
            ),
            (notes[1], occurrence.onset, occurrence.gate_end, 93)
        );
        assert!(
            gate.prepared_voice().is_none(),
            "captured gate keeps no instrument bank resource"
        );
        assert_eq!(spans.spans().count(), 1);
        instrument
            .query_after_onset_with_origin(origin, &timing, start, start + 1, &mut spans, beat(1))
            .unwrap();
        assert_eq!(
            spans.spans().count(),
            1,
            "exact global onset floor is inclusive"
        );
        instrument
            .query_after_onset_with_origin(
                origin,
                &timing,
                start,
                start + 1,
                &mut spans,
                BeatTime::from_raw(beat(1).raw() + 1),
            )
            .unwrap();
        assert!(spans.is_empty());
        assert_eq!(
            gates.query_with_origin(origin, &timing, start, start + 257, &mut spans),
            Err(MidiTemplateError::QueryTooWide)
        );
        assert!(spans.is_empty());
    }
}
