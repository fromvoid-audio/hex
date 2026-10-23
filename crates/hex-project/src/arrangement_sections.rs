//! Song-form markers and one atomic beat-domain cut/insert permutation.
//! Sections are disjoint half-open ranges. Gaps are real song time and travel
//! with intervening content; marker resize/move never changes musical material.
//! Reordering cuts `[start,end)` and inserts it before the target's original
//! start. Later destinations subtract the removed length. The entire intervening
//! interval (including gaps) shifts by that length. All tracks, gain base/touch
//! lanes, tempo and meter follow one permutation; the loop keeps its musical
//! beat interval. Authored tempo/meter points remain present even when their
//! values match neighbors. Synthetic meter anchors start a bar fragment at each
//! splice seam; equal signatures still carry meaningful bar-origin semantics.
//! The session/transport owner excludes active playback/recording
//! and pending commits before canonical preparation.
//! Audio sample intervals use the exact integer-frame cut/insert permutation.
//! Musical audio retains its full original processing context, with at most one
//! sample of explicit placement residual for nearest-frame tempo-prefix phase.
//! Unrepresentable residuals refuse the candidate. Later tempo/timing edits that
//! require rebasing these roots refuse explicitly. Uncut MIDI retains source
//! and loop metadata; boundary crossings reuse the finite canonical split law,
//! including retriggered carried gates and materialized source-beat volume.
//! Original source loops are supported. The separate outer Once/Repeat view
//! placement capability refuses until its engine occurrence path can preserve
//! the complete song edit; it is never silently flattened or partly moved.
//! Every fragment is planned from the original state; Editor owns one atomic
//! preparation/history transition and preserves the original on any failure.
use crate::{BeatTime, Command, Created, ElementColor, Invalid, Project, SectionId};

pub const MAX_ARRANGEMENT_SECTIONS: usize = 1_024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementSection {
    pub(crate) id: SectionId,
    pub(crate) name: String,
    pub(crate) color: ElementColor,
    pub(crate) start: BeatTime,
    pub(crate) end: BeatTime,
}
impl ArrangementSection {
    pub fn id(&self) -> SectionId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn color(&self) -> ElementColor {
        self.color
    }
    pub fn start(&self) -> BeatTime {
        self.start
    }
    pub fn end(&self) -> BeatTime {
        self.end
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementSectionSnapshot {
    pub id: u64,
    pub name: String,
    pub color: ElementColor,
    pub start: BeatTime,
    pub end: BeatTime,
}
impl From<&ArrangementSection> for ArrangementSectionSnapshot {
    fn from(section: &ArrangementSection) -> Self {
        Self {
            id: section.id.value(),
            name: section.name.clone(),
            color: section.color,
            start: section.start,
            end: section.end,
        }
    }
}
pub(crate) fn from_snapshot(
    section: ArrangementSectionSnapshot,
) -> Result<ArrangementSection, Invalid> {
    crate::edit::name_valid(&section.name)?;
    if section.id == 0 || section.start >= section.end {
        return Err(Invalid("arrangement section identity/range is invalid"));
    }
    Ok(ArrangementSection {
        id: SectionId(section.id),
        name: section.name,
        color: section.color,
        start: section.start,
        end: section.end,
    })
}
pub(crate) fn validate(
    sections: &[ArrangementSection],
    tempo: &crate::TempoMap,
) -> Result<(), Invalid> {
    if sections.len() > MAX_ARRANGEMENT_SECTIONS {
        return Err(Invalid("arrangement section limit reached"));
    }
    for (index, section) in sections.iter().enumerate() {
        crate::edit::name_valid(&section.name)?;
        tempo.beat_to_frame(section.start)?;
        tempo.beat_to_frame(section.end)?;
        if section.start >= section.end {
            return Err(Invalid("arrangement section range must be positive"));
        }
        if index > 0 && sections[index - 1].end > section.start {
            return Err(Invalid("arrangement sections must be ordered and disjoint"));
        }
    }
    Ok(())
}
/// Canonical read-only insertion geometry. This is metadata preview only;
/// preparation and publication still own the complete atomic project transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionReorderPreview {
    source_start: BeatTime,
    source_end: BeatTime,
    insertion_beat: BeatTime,
    destination_start: BeatTime,
    sections: Vec<ArrangementSection>,
    tempo_map: crate::TempoMap,
}
impl SectionReorderPreview {
    pub fn source_start(&self) -> BeatTime {
        self.source_start
    }
    pub fn source_end(&self) -> BeatTime {
        self.source_end
    }
    pub fn insertion_beat(&self) -> BeatTime {
        self.insertion_beat
    }
    pub fn destination_start(&self) -> BeatTime {
        self.destination_start
    }
    pub fn sections(&self) -> &[ArrangementSection] {
        &self.sections
    }
    pub fn tempo_map(&self) -> &crate::TempoMap {
        &self.tempo_map
    }
    pub fn is_noop(&self) -> bool {
        self.source_start == self.destination_start
    }
    /// Map an original song position through the half-open cut/insert law.
    pub fn map_beat(&self, beat: BeatTime) -> Result<BeatTime, Invalid> {
        let s = self.source_start.raw();
        let e = self.source_end.raw();
        let t = self.insertion_beat.raw();
        let b = beat.raw();
        let length = e - s;
        let mapped = if self.is_noop() {
            b
        } else if s <= b && b < e {
            self.destination_start
                .raw()
                .checked_add(b - s)
                .ok_or(Invalid("section placement overflow"))?
        } else if t < s && t <= b && b < s {
            b.checked_add(length)
                .ok_or(Invalid("section placement overflow"))?
        } else if t > e && e <= b && b < t {
            b - length
        } else {
            b
        };
        Ok(BeatTime::from_raw(mapped))
    }
    pub(crate) fn cuts(&self) -> [BeatTime; 3] {
        let mut cuts = [self.source_start, self.source_end, self.insertion_beat];
        cuts.sort();
        cuts
    }
}
impl Project {
    pub fn arrangement_sections(&self) -> &[ArrangementSection] {
        &self.arrangement_sections
    }
    pub fn arrangement_section(&self, id: SectionId) -> Option<&ArrangementSection> {
        self.arrangement_sections
            .iter()
            .find(|section| section.id == id)
    }
    pub fn preview_section_reorder(
        &self,
        section: SectionId,
        before: SectionId,
    ) -> Result<SectionReorderPreview, Invalid> {
        let source = self
            .arrangement_section(section)
            .ok_or(Invalid("arrangement section no longer exists"))?;
        let target = self
            .arrangement_section(before)
            .ok_or(Invalid("arrangement destination no longer exists"))?;
        let insertion = if section == before {
            source.start
        } else {
            target.start
        };
        let destination = if insertion >= source.end {
            BeatTime::from_raw(insertion.raw() - (source.end.raw() - source.start.raw()))
        } else {
            insertion
        };
        let mut result = SectionReorderPreview {
            source_start: source.start,
            source_end: source.end,
            insertion_beat: insertion,
            destination_start: destination,
            sections: Vec::new(),
            tempo_map: self.tempo_map.clone(),
        };
        result
            .sections
            .try_reserve_exact(self.arrangement_sections.len())
            .map_err(|_| Invalid("could not reserve arrangement section preview"))?;
        for old in &self.arrangement_sections {
            let mut new = old.clone();
            new.start = result.map_beat(old.start)?;
            new.end = BeatTime::from_raw(
                new.start
                    .raw()
                    .checked_add(old.end.raw() - old.start.raw())
                    .ok_or(Invalid("section end overflow"))?,
            );
            result.sections.push(new);
        }
        result.sections.sort_by_key(|s| s.start);
        result.tempo_map = reorder::tempo_map(self.tempo_map(), &result)?;
        validate(&result.sections, &result.tempo_map)?;
        Ok(result)
    }
}
pub(crate) fn apply(project: &mut Project, command: Command) -> Result<Created, Invalid> {
    match command {
        Command::AddArrangementSection {
            name,
            color,
            start,
            end,
        } => {
            crate::edit::name_valid(&name)?;
            if project.arrangement_sections.len() == MAX_ARRANGEMENT_SECTIONS {
                return Err(Invalid("arrangement section limit reached"));
            }
            let id = SectionId(project.allocate_id()?);
            project.arrangement_sections.push(ArrangementSection {
                id,
                name,
                color,
                start,
                end,
            });
            project.arrangement_sections.sort_by_key(|s| s.start);
            validate(&project.arrangement_sections, project.tempo_map())?;
            Ok(Created::ArrangementSection(id))
        }
        Command::ReorderSectionBefore { section, before } => reorder(project, section, before),
        Command::RemoveArrangementSection(id) => {
            let index = project
                .arrangement_sections
                .iter()
                .position(|s| s.id == id)
                .ok_or(Invalid("arrangement section no longer exists"))?;
            project.arrangement_sections.remove(index);
            Ok(Created::None)
        }
        command => {
            let id = match &command {
                Command::SetArrangementSectionRange { section, .. }
                | Command::RenameArrangementSection { section, .. }
                | Command::SetArrangementSectionColor { section, .. } => *section,
                _ => return Err(Invalid("unsupported arrangement section command")),
            };
            let section = project
                .arrangement_sections
                .iter_mut()
                .find(|s| s.id == id)
                .ok_or(Invalid("arrangement section no longer exists"))?;
            match command {
                Command::SetArrangementSectionRange { start, end, .. } => {
                    section.start = start;
                    section.end = end;
                }
                Command::RenameArrangementSection { name, .. } => {
                    crate::edit::name_valid(&name)?;
                    section.name = name;
                }
                Command::SetArrangementSectionColor { color, .. } => section.color = color,
                _ => return Err(Invalid("unsupported arrangement section command")),
            }
            project.arrangement_sections.sort_by_key(|s| s.start);
            validate(&project.arrangement_sections, project.tempo_map())?;
            Ok(Created::None)
        }
    }
}
mod reorder;
use reorder::reorder;
