//! Owned source-template buffers only; shared Arc overhead remains separate.
use super::{DensityEnvelope, PreparedMidiSourceTemplate, SourceEdge, SourceSpan};
use crate::storage_report::{PlaybackStorageVisitor, Report, StorageOwner, StorageReportError};
impl PreparedMidiSourceTemplate {
    pub(crate) fn visit_storage<'a, V: PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut Report<'a, '_, V>,
        owner: StorageOwner<'a>,
    ) -> Result<(), StorageReportError> {
        let Self {
            track: _,
            clip: _,
            sample_rate: _,
            admitted_max_micro_bpm: _,
            first_pass: _,
            period: _,
            intro,
            repeats,
            intro_spans,
            repeat_spans,
            density,
        } = self;
        let DensityEnvelope {
            max_edges: _,
            max_onsets: _,
        } = density;
        report.vec(owner, "midi_source.intro_edges", [0; 2], intro)?;
        report.vec(owner, "midi_source.repeat_edges", [0; 2], repeats)?;
        report.vec(owner, "midi_source.intro_spans", [0; 2], intro_spans)?;
        report.vec(owner, "midi_source.repeat_spans", [0; 2], repeat_spans)?;
        // The parent pointee already includes headers and scalar proof/state.
        for edge in intro.iter().chain(repeats) {
            let SourceEdge {
                phase: _,
                kind: _,
                note: _,
                pitch: _,
                channel: _,
                velocity: _,
            } = edge;
        }
        for span in intro_spans.iter().chain(repeat_spans) {
            let SourceSpan {
                start: _,
                gate_end: _,
                note: _,
                pitch: _,
                channel: _,
                velocity: _,
            } = span;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests;
