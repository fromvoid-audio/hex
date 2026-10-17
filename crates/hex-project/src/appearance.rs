//! Shared semantic element identities. RGB values belong to the active UI theme.
use crate::{ClipId, Created, Invalid, Project, SessionCellContent};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ElementColor {
    #[default]
    Gray,
    Red,
    Orange,
    Amber,
    Olive,
    Green,
    Cyan,
    Blue,
    Violet,
    Rose,
}
impl ElementColor {
    pub const ALL: [Self; 10] = [
        Self::Gray,
        Self::Red,
        Self::Orange,
        Self::Amber,
        Self::Olive,
        Self::Green,
        Self::Cyan,
        Self::Blue,
        Self::Violet,
        Self::Rose,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Gray => "gray",
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Amber => "amber",
            Self::Olive => "olive",
            Self::Green => "green",
            Self::Cyan => "cyan",
            Self::Blue => "blue",
            Self::Violet => "violet",
            Self::Rose => "rose",
        }
    }
    pub fn from_key(key: &str) -> Result<Self, Invalid> {
        Self::ALL
            .into_iter()
            .find(|color| color.key() == key)
            .ok_or(Invalid("unknown element color"))
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClipFlavor {
    #[default]
    Muted,
    Accent,
}
impl ClipFlavor {
    pub fn toggled(self) -> Self {
        match self {
            Self::Muted => Self::Accent,
            Self::Accent => Self::Muted,
        }
    }
}

impl Project {
    pub fn clip_flavor(&self, id: ClipId) -> Option<ClipFlavor> {
        self.clip(id)
            .map(|c| c.flavor)
            .or_else(|| self.midi_clip(id).map(|c| c.flavor))
            .or_else(|| {
                self.session
                    .cells
                    .iter()
                    .find_map(|cell| match &cell.content {
                        SessionCellContent::Audio(c) if c.id == id => Some(c.flavor),
                        SessionCellContent::Midi(c) if c.id == id => Some(c.flavor),
                        _ => None,
                    })
            })
    }
    pub(crate) fn set_clip_flavor(
        &mut self,
        id: ClipId,
        flavor: ClipFlavor,
    ) -> Result<Created, Invalid> {
        for track in &mut self.tracks {
            if let Some(c) = track.content.audio_clips.iter_mut().find(|c| c.id == id) {
                c.flavor = flavor;
                return Ok(Created::None);
            }
            if let Some(c) = track.content.midi_clips.iter_mut().find(|c| c.id == id) {
                c.flavor = flavor;
                return Ok(Created::None);
            }
        }
        for cell in &mut self.session.cells {
            match &mut cell.content {
                SessionCellContent::Audio(c) if c.id == id => {
                    c.flavor = flavor;
                    return Ok(Created::None);
                }
                SessionCellContent::Midi(c) if c.id == id => {
                    c.flavor = flavor;
                    return Ok(Created::None);
                }
                _ => {}
            }
        }
        Err(Invalid("clip no longer exists"))
    }
}
