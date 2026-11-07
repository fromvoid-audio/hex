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
    /// Eight musical colors for new tracks and element pickers. Blue is reserved
    /// for UI highlights; blue and gray remain valid persisted identities.
    pub const TRACK_CHOICES: [Self; 8] = [
        Self::Red,
        Self::Orange,
        Self::Amber,
        Self::Olive,
        Self::Green,
        Self::Cyan,
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
// Control-side only: count at most MAX_TRACKS existing identities, then choose
// among equally least-used hues. No new failure mode when the palette fills.
fn least_used_track_color(project: &Project, ticket: u64) -> ElementColor {
    let mut counts = ElementColor::TRACK_CHOICES.map(|_| 0_u16);
    for track in &project.tracks {
        if let Some(index) = ElementColor::TRACK_CHOICES
            .iter()
            .position(|c| *c == track.color)
        {
            counts[index] += 1;
        }
    }
    let minimum = counts.iter().copied().min().unwrap_or(0);
    let ties = counts.iter().filter(|n| **n == minimum).count();
    let mut remaining = (ticket % ties.max(1) as u64) as usize;
    // The fixed palette is nonempty at compile time. Select its ordered tie
    // rank with bounded work, without a fallible iterator lookup or Gray fallback.
    let [mut selected, ..] = ElementColor::TRACK_CHOICES;
    for (color, count) in ElementColor::TRACK_CHOICES.into_iter().zip(counts) {
        if count == minimum {
            selected = color;
            if remaining == 0 {
                break;
            }
            remaining -= 1;
        }
    }
    selected
}

pub(crate) fn new_track_color(project: &Project) -> ElementColor {
    use std::hash::BuildHasher;
    // Standard-library randomly seeded hashing supplies a non-security tie draw
    // without adding a dependency, clock/global setting or saved random state.
    // The chosen identity is written into the track once; snapshot redo/load
    // restore it and never rerun this function.
    let ticket = std::collections::hash_map::RandomState::new().hash_one(project.next_id);
    least_used_track_color(project, ticket)
}

#[cfg(test)]
mod automatic_color_tests {
    use super::*;
    #[test]
    fn every_unused_tie_is_reachable_without_changing_the_palette() {
        let project = Project::new(48_000).unwrap();
        let choices: Vec<_> = (0..8)
            .map(|ticket| least_used_track_color(&project, ticket))
            .collect();
        assert_eq!(choices, ElementColor::TRACK_CHOICES);
        assert_eq!(
            least_used_track_color(&project, u64::MAX),
            ElementColor::TRACK_CHOICES[(u64::MAX % 8) as usize]
        );
        assert_eq!(ElementColor::ALL[0], ElementColor::Gray);
        assert!(!ElementColor::TRACK_CHOICES.contains(&ElementColor::Gray));
        assert!(!ElementColor::TRACK_CHOICES.contains(&ElementColor::Blue));
        for identity in ElementColor::ALL {
            assert_eq!(ElementColor::from_key(identity.key()), Ok(identity));
        }
        assert_eq!(ElementColor::ALL.len(), 10);
    }
    #[test]
    fn authored_count_profiles_preserve_exact_ticket_rank_for_every_tie_subset() {
        use crate::{Command, Editor};
        let palette = ElementColor::TRACK_CHOICES;
        let mut completed = 0;
        // Legal projects cover every unused-hue subset and every least-used
        // subset after exhaustion, without inventing malformed runtime state.
        for base in [0_u16, 1] {
            for mask in 0..(1_usize << palette.len()) {
                let counts =
                    std::array::from_fn::<_, 8, _>(|i| base + u16::from(mask & (1 << i) != 0));
                let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
                for (color, count) in palette.into_iter().zip(counts) {
                    for _ in 0..count {
                        let Created::Track(track) = editor
                            .execute(Command::AddTrack {
                                name: "authored".into(),
                            })
                            .unwrap()
                        else {
                            panic!("track");
                        };
                        editor
                            .execute(Command::SetTrackColor { track, color })
                            .unwrap();
                    }
                }
                let minimum = counts.into_iter().min().unwrap();
                let candidates = palette
                    .into_iter()
                    .zip(counts)
                    .filter_map(|(color, count)| (count == minimum).then_some(color))
                    .collect::<Vec<_>>();
                assert!(!candidates.is_empty());
                for ticket in (0..(2 * candidates.len()) as u64).chain([u64::MAX - 1, u64::MAX]) {
                    assert_eq!(
                        least_used_track_color(editor.project(), ticket),
                        candidates[(ticket % candidates.len() as u64) as usize],
                        "base{base} mask{mask} ticket{ticket}"
                    );
                }
                completed += 1;
            }
        }
        assert_eq!(completed, 512);
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
