//! Clip editor subject retained from an Arrangement cursor or time selection.
//!
//! As in Live, placing the point cursor or pressing a time selection inside a
//! clip body shows that clip in the clip editor without making it a whole-clip
//! (object) selection. Whole-clip selection stays `selected_clip` plus
//! `clip_selection`; it alone highlights the clip, hides the point cursor and
//! supplies Play's start, and it alone is the target of Delete, nudge and trim.
//! A retained context only answers `detail_clip_id`, so the cursor stays
//! visible and Play starts from the cursor or time selection.
//!
//! Whole-clip selection outranks and replaces the context. The context belongs
//! to the selection intent that placed it: any later selection intent (clip,
//! track, section or range choices), Main focus, pressing empty lane space,
//! focusing track details, leaving Arrangement, document replacement and clip
//! removal retire it. Canonical project state is never changed here.
use super::EditorView;
use gpui::{Context, Window};
use hex_project::{ClipId, TrackId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DetailContext {
    pub(super) clip: ClipId,
    generation: u64,
    revision: u64,
    intent: Option<u64>,
}

impl EditorView {
    /// The clip editor subject: the whole-clip selection, else a current context.
    pub(super) fn detail_clip_id(&self) -> Option<ClipId> {
        self.selected_clip.or_else(|| {
            self.detail_context
                .filter(|context| {
                    self.mode == super::EditorMode::Arrangement
                        && context.generation == self.project_generation
                        && context.intent == self.selection_intent_epoch
                })
                .map(|context| context.clip)
        })
    }

    /// The Arrangement clip containing `frame` on `track`, preferring `prefer`
    /// where audio and MIDI material overlap. MIDI starts and lengths are beats
    /// and are compared through the tempo map; clip ends are exclusive.
    pub(super) fn arrangement_clip_at(
        &self,
        track: TrackId,
        frame: u64,
        prefer: Option<ClipId>,
    ) -> Option<ClipId> {
        let project = &self.state.as_ref()?.project;
        let track = project.track(track)?;
        let audio = track.clips().iter().filter_map(|clip| {
            let end = clip.start().checked_add(clip.duration())?;
            (frame >= clip.start() && frame < end).then_some(clip.id())
        });
        let beat = project.tempo_map().frame_to_beat(frame).ok();
        let midi = track.midi_clips().iter().filter_map(move |clip| {
            let beat = beat?.raw();
            let end = clip.start().raw().checked_add(clip.length().raw())?;
            (beat >= clip.start().raw() && beat < end).then_some(clip.id())
        });
        let mut hits = audio.chain(midi);
        let first = hits.next()?;
        if prefer.is_some_and(|preferred| preferred != first) {
            return hits.find(|id| Some(*id) == prefer).or(Some(first));
        }
        Some(first)
    }

    /// Retain `clip` as the clip editor subject for the current selection
    /// intent, without whole-clip selection; `None` retires the context.
    /// Clip-local editor state resets on a change.
    pub(super) fn set_detail_context(&mut self, clip: Option<ClipId>) {
        let Some(clip) = clip else {
            self.detail_context = None;
            return;
        };
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            return;
        };
        // Compare against the current subject first, so the same clip keeps
        // its editor viewport when it changes from selection to context.
        self.retire_clip_detail_state_for(clip);
        self.selected_clip = None;
        self.detail_context = Some(DetailContext {
            clip,
            generation: self.project_generation,
            revision,
            intent: self.selection_intent_epoch,
        });
    }

    /// Show the retained context in the clip editor after a pointer gesture.
    pub(super) fn focus_detail_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_clip.is_none() && self.detail_clip_id().is_some() {
            self.focus_clip_details(window, cx);
        }
    }

    /// Retire a context whose document or clip no longer exists.
    pub(super) fn reconcile_detail_context(&mut self) {
        let Some(context) = self.detail_context else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            self.detail_context = None;
            return;
        };
        if context.generation != self.project_generation
            || context.intent != self.selection_intent_epoch
        {
            self.detail_context = None;
        } else if context.revision != state.revision {
            let exists = state.project.clip(context.clip).is_some()
                || state.project.midi_clip(context.clip).is_some();
            self.detail_context = exists.then_some(DetailContext {
                revision: state.revision,
                ..context
            });
        }
    }
}
