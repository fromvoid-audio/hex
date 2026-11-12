//! Trim preview metadata and redraw decisions based on visible candidate changes.
use hex_project::{
    BeatTime, Clip, MidiClip, MidiClipLoopResize, MidiClipStretch, MidiClipTrim, MidiSourcePlayback,
};
use std::cell::RefCell;

#[derive(Clone, Debug)]
pub(in crate::editor) struct TrimPreviewState {
    pub(in crate::editor) clip: Clip,
    pub(super) command_frame: u64,
    pub(super) stretch: bool,
}

#[cfg(all(test, feature = "ui-test-support"))]
impl TrimPreviewState {
    pub(in crate::editor) fn command_frame_for_test(&self) -> u64 {
        self.command_frame
    }
}

pub(super) fn replace_audio_trim_preview(
    preview: &RefCell<Option<TrimPreviewState>>,
    next: Option<TrimPreviewState>,
) -> bool {
    let Ok(mut current) = preview.try_borrow_mut() else {
        return false;
    };
    if current.as_ref().map(|preview| &preview.clip) == next.as_ref().map(|preview| &preview.clip) {
        return false;
    }
    *current = next;
    true
}

/// One immutable source plus metadata projection survives capture and the exact
/// worker terminal. Pointer updates never materialize a stretched note array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum MidiEdgePreview {
    Trim(MidiClipTrim),
    Stretch(MidiClipStretch),
    Loop(MidiClipLoopResize),
}
impl From<MidiClipTrim> for MidiEdgePreview {
    fn from(value: MidiClipTrim) -> Self {
        Self::Trim(value)
    }
}
impl MidiEdgePreview {
    pub(in crate::editor) fn start(self) -> BeatTime {
        match self {
            Self::Trim(p) => p.start(),
            Self::Stretch(p) => p.start(),
            Self::Loop(p) => p.start(),
        }
    }
    pub(in crate::editor) fn length(self) -> BeatTime {
        match self {
            Self::Trim(p) => p.length(),
            Self::Stretch(p) => p.length(),
            Self::Loop(p) => p.length(),
        }
    }
    pub(in crate::editor) fn source_start(self) -> BeatTime {
        match self {
            Self::Trim(p) => p.source_start(),
            Self::Stretch(p) => p.source_start(),
            Self::Loop(p) => p.source_start(),
        }
    }
    pub(in crate::editor) fn playback_offset(self) -> BeatTime {
        match self {
            Self::Trim(p) => p.playback_offset(),
            Self::Stretch(p) => p.playback_offset(),
            Self::Loop(p) => p.playback_offset(),
        }
    }
    pub(in crate::editor) fn note_interval(
        self,
        note: &hex_project::MidiNote,
    ) -> Option<(BeatTime, BeatTime)> {
        let start = self.source_beat(note.start())?;
        let end = self.source_beat(BeatTime::from_raw(
            note.start().raw().checked_add(note.duration().raw())?,
        ))?;
        let duration = end.raw().checked_sub(start.raw()).filter(|n| *n > 0)?;
        Some((start, BeatTime::from_raw(duration)))
    }
    /// The same endpoint projection used by the canonical stretch command.
    pub(in crate::editor) fn source_beat(self, beat: BeatTime) -> Option<BeatTime> {
        match self {
            Self::Trim(p) => beat
                .raw()
                .checked_add(p.source_prefix().raw())
                .map(BeatTime::from_raw),
            Self::Stretch(p) => p.scale(beat).ok(),
            Self::Loop(_) => Some(beat),
        }
    }
    pub(in crate::editor) fn looping(self, clip: &MidiClip) -> bool {
        matches!(self, Self::Loop(_)) || clip.looping()
    }
    pub(in crate::editor) fn source_playback(
        self,
        clip: &MidiClip,
    ) -> Result<MidiSourcePlayback, hex_project::Invalid> {
        if let Self::Loop(p) = self {
            return p.source_playback();
        }
        let shifted = |beat| {
            self.source_beat(beat).ok_or(hex_project::Invalid(
                "midi preview source position overflow",
            ))
        };
        let loop_range = if clip.looping() {
            Some((shifted(clip.loop_start())?, shifted(clip.loop_end())?))
        } else {
            None
        };
        MidiSourcePlayback::new_with_initial_end(
            shifted(clip.source_length())?,
            self.source_start(),
            loop_range,
            self.length(),
            shifted(clip.source().effective_end())?,
        )?
        .with_playback_offset(self.playback_offset())
    }
    pub(super) fn boundary(self, clip: &MidiClip) -> Option<BeatTime> {
        self.start()
            .raw()
            .checked_add(
                self.source_playback(clip)
                    .ok()?
                    .first_repeat_boundary()?
                    .raw(),
            )
            .map(BeatTime::from_raw)
    }
}

pub(super) fn replace_midi_trim_preview(
    preview: &RefCell<Option<MidiEdgePreview>>,
    next: Option<MidiEdgePreview>,
) -> bool {
    let Ok(mut current) = preview.try_borrow_mut() else {
        return false;
    };
    if *current == next {
        return false;
    }
    *current = next;
    true
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{TrimPreviewState, replace_audio_trim_preview, replace_midi_trim_preview};
    use hex_project::{Command, Created, Editor, Project, TrimEdge};
    use std::cell::RefCell;

    #[test]
    fn identical_audio_and_midi_trim_candidates_do_not_request_redraws() {
        let (editor, _, id) = super::super::geometry::clip_fixture(300);
        let clip = editor.project().clip(id).expect("clip");
        let audio = RefCell::new(None);
        let candidate = TrimPreviewState {
            clip: clip.clone(),
            command_frame: clip.start(),
            stretch: false,
        };
        assert!(replace_audio_trim_preview(&audio, Some(candidate.clone())));
        assert!(!replace_audio_trim_preview(&audio, Some(candidate)));
        assert!(replace_audio_trim_preview(&audio, None));
        assert!(!replace_audio_trim_preview(&audio, None));

        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "midi".into(),
            })
            .expect("track")
        else {
            panic!("track result")
        };
        let Created::Clip(clip_id) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::ONE,
            })
            .expect("midi clip")
        else {
            panic!("clip result")
        };
        let midi_clip = editor.project().midi_clip(clip_id).expect("midi clip");
        let midi_candidate = midi_clip
            .preview_edge_trim(TrimEdge::Right, hex_project::BeatTime::ONE)
            .expect("midi trim candidate");
        let midi = RefCell::new(None);
        assert!(replace_midi_trim_preview(
            &midi,
            Some(midi_candidate.into())
        ));
        assert!(!replace_midi_trim_preview(
            &midi,
            Some(midi_candidate.into())
        ));
    }
}
