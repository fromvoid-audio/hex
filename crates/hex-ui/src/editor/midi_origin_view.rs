//! Coordinate origins for observed MIDI history states, never musical history.
//! The Editor has at most 128 undo edges. Two weak identities per observed edge
//! plus current states fit this fixed 512-entry FIFO; no note backing is retained.
//! Exact canonical terminals record translations. Ordinary states inherit their
//! predecessor's origin, and real Undo/Redo restore already-observed identities.
//! Replacement clears view metadata. This is no playback/source-freshness proof.
use hex_project::{ClipId, MidiSourceIdentity, MidiSourceWeakIdentity};

const CAPACITY: usize = 512;
struct Entry {
    clip: ClipId,
    source: MidiSourceWeakIdentity,
    origin: i128,
}
pub(super) struct State {
    generation: Option<u64>,
    next: usize,
    entries: [Option<Entry>; CAPACITY],
}
impl Default for State {
    fn default() -> Self {
        Self {
            generation: None,
            next: 0,
            entries: [const { None }; CAPACITY],
        }
    }
}
impl State {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    fn ensure_generation(&mut self, generation: u64) {
        if self.generation != Some(generation) {
            self.clear();
            self.generation = Some(generation);
        }
    }
    fn origin(&self, clip: ClipId, source: &MidiSourceIdentity) -> Option<i128> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.clip == clip && entry.source.matches(source))
            .map(|entry| entry.origin)
    }
    fn remember(&mut self, clip: ClipId, source: &MidiSourceIdentity, origin: i128) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.clip == clip && entry.source.matches(source))
        {
            entry.origin = origin;
            return;
        }
        self.entries[self.next] = Some(Entry {
            clip,
            source: source.downgrade(),
            origin,
        });
        self.next = (self.next + 1) % CAPACITY;
    }
    pub(super) fn record_prefix(
        &mut self,
        generation: u64,
        clip: ClipId,
        old: &MidiSourceIdentity,
        new: &MidiSourceIdentity,
        prefix: u64,
    ) {
        self.ensure_generation(generation);
        let origin = self.origin(clip, old).unwrap_or(0);
        self.remember(clip, old, origin);
        self.remember(clip, new, origin.saturating_add(i128::from(prefix)));
    }
    pub(super) fn advance(
        &mut self,
        generation: u64,
        clip: ClipId,
        old: &MidiSourceIdentity,
        new: &MidiSourceIdentity,
    ) -> i128 {
        self.ensure_generation(generation);
        if old == new {
            return 0;
        }
        let prior = self.origin(clip, old).unwrap_or(0);
        let next = self.origin(clip, new).unwrap_or(prior);
        if prior != 0 || next != 0 {
            self.remember(clip, old, prior);
            self.remember(clip, new, next);
        }
        next.saturating_sub(prior)
    }
}

impl super::EditorView {
    /// Same synchronous receive as canonical installation, including history.
    pub(super) fn advance_midi_origin_view(&mut self, event: &hex_session::service::Event) {
        if event.project_replaced {
            self.midi_origin_history.clear();
            return;
        }
        let Some(clip) = self.detail_clip_id() else {
            return;
        };
        let Some(old) = self
            .state
            .as_ref()
            .and_then(|s| super::midi_pending_drag::source_identity(&s.project, clip))
        else {
            return;
        };
        let Some(new) = super::midi_pending_drag::source_identity(&event.state.project, clip)
        else {
            return;
        };
        let delta = self
            .midi_origin_history
            .advance(self.project_generation, clip, &old, &new);
        if delta == 0 {
            return;
        }
        self.piano_beat_start = super::midi_scene::clamp_start(
            self.piano_beat_start.saturating_add(delta),
            self.piano_beat_span.raw(),
        );
        self.midi_marker = self
            .midi_marker
            .and_then(|raw| u64::try_from(i128::from(raw).saturating_add(delta)).ok());
        self.shift_midi_time_selection_origin(delta);
        self.zoom_fit
            .shift_midi_origin(self.project_generation, clip, delta);
    }
}

#[cfg(test)]
#[path = "midi_origin_view_tests.rs"]
mod tests;
