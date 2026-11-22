//! Clip labels are canonical metadata, independent of source and placement.
use crate::{ClipId, Created, Invalid, Project, SessionCellContent};

impl Project {
    pub fn clip_name(&self, id: ClipId) -> Option<&str> {
        self.clip(id)
            .map(|c| c.name())
            .or_else(|| self.midi_clip(id).map(|c| c.name()))
            .or_else(|| self.session_audio_clip(id).map(|c| c.name()))
            .or_else(|| self.session_midi_clip(id).map(|c| c.name()))
    }
}
pub(crate) fn rename(project: &mut Project, id: ClipId, name: String) -> Result<Created, Invalid> {
    crate::edit::name_valid(&name)?;
    for track in &mut project.tracks {
        if let Some(clip) = track.content.audio_clips.iter_mut().find(|c| c.id == id) {
            clip.name = name.into();
            return Ok(Created::None);
        }
        if let Some(clip) = track.content.midi_clips.iter_mut().find(|c| c.id == id) {
            clip.name = name;
            return Ok(Created::None);
        }
    }
    for cell in &mut project.session.cells {
        match &mut cell.content {
            SessionCellContent::Audio(c) if c.id == id => {
                c.name = name;
                return Ok(Created::None);
            }
            SessionCellContent::Midi(c) if c.id == id => {
                c.name = name;
                return Ok(Created::None);
            }
            _ => {}
        }
    }
    Err(Invalid("clip no longer exists"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BeatTime, Command, Editor};

    #[test]
    fn rename_updates_clip_name_and_rejects_blank_or_missing_clip() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "m".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::RenameClip {
                clip,
                name: "lead".into(),
            })
            .unwrap();
        assert_eq!(editor.project().clip_name(clip), Some("lead"));
        assert!(
            editor
                .execute(Command::RenameClip {
                    clip,
                    name: "   ".into(),
                })
                .is_err()
        );
        assert!(
            editor
                .execute(Command::RenameClip {
                    clip: ClipId(999),
                    name: "other".into(),
                })
                .is_err()
        );
    }
}
