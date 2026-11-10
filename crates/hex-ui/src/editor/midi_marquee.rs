//! Live marquee coordination for piano and velocity surfaces. Selection remains
//! view state; canonical musical data, history and callback work keep their owners.
//! Hit-entry batches use the existing finite audition path. A new batch replaces
//! the previous finite chord; leaving/canceling revokes it. Re-entry may sound once
//! again. Unchanged movement, paints and release never create another intent.
use super::{
    EditorView,
    midi_selection::{Marquee, MarqueeContext},
    midi_stroke::CanvasBounds,
};
use hex_project::NoteId;

#[path = "midi_marquee_pointer.rs"]
mod pointer;

/// Immutable hit projections; selection, rollback and audition retain their owners.
pub(super) enum Surface {
    Piano(std::sync::Arc<super::midi_scene::MidiScene>),
    Velocity(std::sync::Arc<super::midi_velocity::Scene>),
}

impl EditorView {
    pub(super) fn midi_marquee_context(&self, bounds: CanvasBounds) -> Option<MarqueeContext> {
        Some(MarqueeContext {
            scene: super::midi_scene::SceneKey {
                generation: self.project_generation,
                revision: self.state.as_ref()?.revision,
                clip: self.detail_clip_id()?,
                beat_start: self.piano_beat_start,
                beat_span: self.piano_beat_span.raw(),
                pitch_low: self.piano_pitch_low,
                pitch_rows: self.piano_pitch_rows,
            },
            bounds,
            selection_epoch: self.midi_selection.epoch(),
        })
    }

    pub(super) fn apply_live_midi_marquee(
        &mut self,
        marquee: &mut Marquee,
        previous: &[NoteId],
    ) -> Result<(), &'static str> {
        let hits = marquee.hits();
        if previous == hits.as_ref() {
            marquee.apply_live(&mut self.midi_selection)?;
            return Ok(());
        }
        let removed = previous.iter().any(|id| hits.binary_search(id).is_err());
        // One extra identity retains whole-batch overflow refusal, without
        // allocating a bank proportional to a potentially very large selection.
        let mut entered = Vec::new();
        entered
            .try_reserve_exact(hex_engine::computer_keyboard::MAX_NOTE_AUDITION_NOTES + 1)
            .map_err(|_| "could not reserve marquee audition")?;
        entered.extend(
            hits.iter()
                .copied()
                .filter(|id| previous.binary_search(id).is_err())
                .take(hex_engine::computer_keyboard::MAX_NOTE_AUDITION_NOTES + 1),
        );
        marquee.apply_live(&mut self.midi_selection)?;
        if !entered.is_empty() {
            self.audition_midi_notes(&entered);
        } else if removed {
            self.cancel_midi_audition();
        }
        Ok(())
    }

    pub(super) fn cancel_live_midi_marquee(&mut self, marquee: Marquee) {
        let context = marquee.context();
        self.cancel_marquee_audition(context);
        if self.project_generation == context.scene.generation
            && self.detail_clip_id() == Some(context.scene.clip)
            && self.midi_selection.epoch() == context.selection_epoch
        {
            marquee.restore_baseline(&mut self.midi_selection);
            if let Some(source) = self
                .state
                .as_ref()
                .and_then(|s| s.project.midi_source(context.scene.clip))
                && let Err(error) = self
                    .midi_selection
                    .retain_valid(source.notes().iter().map(|note| note.id()))
            {
                self.error = Some(error.into());
            }
        }
    }

    /// An explicit musical/selection action adopts the already visible live
    /// membership. Retire its pointer owner before canceling other edit drafts.
    pub(super) fn finish_live_midi_marquees(&mut self) {
        let grid = self.midi_marquee.take();
        let velocity = self.midi_velocity.take_marquee();
        for marquee in grid.into_iter().chain(velocity) {
            self.cancel_marquee_audition(marquee.context());
        }
    }

    pub(super) fn cancel_all_midi_marquees(&mut self) -> bool {
        let grid = self.midi_marquee.take();
        let velocity = self.midi_velocity.take_marquee();
        let changed = grid.is_some() || velocity.is_some();
        for marquee in grid.into_iter().chain(velocity) {
            self.cancel_live_midi_marquee(marquee);
        }
        changed
    }
}
