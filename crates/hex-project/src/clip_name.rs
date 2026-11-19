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
