//! Song-form markers and one atomic beat-domain cut/insert permutation.
//! Sections are disjoint half-open ranges. Gaps are real song time and travel
//! with intervening content; explicit marker-only range edits never change music.
//! Fixed-span inner/shared edge edits and song-wide ripple resize/delete/duplicate
//! use the sibling splice planner. Inner edges keep other markers fixed; shared
//! edges keep the pair's outer span fixed. Crossed time becomes empty. Marker
//! merging preserves the earliest identity/name/color and all musical material.
//! Marker moves retain their identity/length and carve destination overlap from
//! other markers. A lone surviving fragment retains its identity; on an interior
//! cut the left retains it and the right receives a fresh identity/name/color copy.
//! Marker edge resizing uses the same carving law; isolated range/content edge
//! resizing still refuses overlap instead of carving neighbors.
//! Marker splitting retains the left identity and gives the adjoining right
//! marker a fresh ID with matching name/color, without editing any musical data.
//! Reordering cuts `[start,end)` and inserts it before/after the target's original
//! range. Later destinations subtract the removed length. The entire intervening
//! interval (including gaps) shifts by that length. All tracks, gain base/touch
//! lanes, tempo and meter follow one permutation; the loop keeps its musical
//! beat interval. Authored tempo/meter points remain present even when their
//! values match neighbors. Synthetic meter anchors are added only where the
//! effective signature changes; equal-meter section seams do not restart bars.
//! Copies draw a semantic color with reuse allowed. Preview and publication agree
//! for the same candidate identity; history and storage retain the chosen color.
//! Destination copies compose the adjacent-copy and stable-ID reorder laws in
//! one private candidate; display previews compose only markers and tempo.
//! The session/transport owner fences structural publication. Reorders/copies
//! may preserve active playback; resizing/deleting retain their stopped rules.
//! Recording and pending commits remain excluded before canonical preparation.
//! Audio sample intervals use the exact integer-frame cut/insert permutation.
//! Musical audio retains its full original processing context, with at most one
//! sample of explicit placement residual for nearest-frame tempo-prefix phase.
//! Unrepresentable residuals refuse the candidate. Later tempo/timing edits that
//! require rebasing these roots refuse explicitly. Uncut MIDI retains source
//! and loop metadata; boundary crossings reuse the finite canonical split law,
//! including retriggered carried gates and materialized source-beat volume.
//! Original source loops and finite outer Once/Repeat views retain their full
//! processing clock, crop, phase and silent tails. Musical outer views translate
//! the old integer-frame projection to their new onset, including internal repeat
//! and directed-read seams; splits trim that same saved clock before relocation.
//! Later global tempo replacement returns warped roots to live processing.
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
    /// True when `other` differs from `self` only in section markers and the
    /// ID allocator they advance. No playback input reads either, so a prepared
    /// audio graph for `self` also renders `other`. Every other field is compared
    /// by an exhaustive destructure: adding a project field must decide here.
    pub fn differs_only_in_arrangement_sections(&self, other: &Project) -> bool {
        let Project {
            aliases,
            native_sources,
            arrangement_sections: _,
            tuning,
            routing,
            audio_feeds,
            midi_routes,
            sidechains,
            import_provenance,
            sample_rate,
            sample_rate_mode,
            next_id: _,
            assets,
            offline_assets,
            tracks,
            returns,
            session,
            loop_region,
            master_gain,
            main_devices,
            tempo_map,
            meter_map,
        } = self;
        *sample_rate == other.sample_rate
            && *sample_rate_mode == other.sample_rate_mode
            && master_gain.to_bits() == other.master_gain.to_bits()
            && *loop_region == other.loop_region
            && *tempo_map == other.tempo_map
            && *meter_map == other.meter_map
            && *tuning == other.tuning
            && *aliases == other.aliases
            && *native_sources == other.native_sources
            && *routing == other.routing
            && *audio_feeds == other.audio_feeds
            && *midi_routes == other.midi_routes
            && *sidechains == other.sidechains
            && *import_provenance == other.import_provenance
            && *assets == other.assets
            && *offline_assets == other.offline_assets
            && *returns == other.returns
            && *session == other.session
            && *main_devices == other.main_devices
            && *tracks == other.tracks
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
        self.preview_section_reorder_at(source, insertion)
    }
    /// Return canonical geometry for moving one section after another section.
    /// Insertion is at the target's original end, so it supports appending after
    /// the final marker and retains all intervening gaps under the cut/insert law.
    pub fn preview_section_reorder_after(
        &self,
        section: SectionId,
        after: SectionId,
    ) -> Result<SectionReorderPreview, Invalid> {
        let source = self
            .arrangement_section(section)
            .ok_or(Invalid("arrangement section no longer exists"))?;
        let target = self
            .arrangement_section(after)
            .ok_or(Invalid("arrangement destination no longer exists"))?;
        self.preview_section_reorder_at(source, target.end)
    }
    fn preview_section_reorder_at(
        &self,
        source: &ArrangementSection,
        insertion: BeatTime,
    ) -> Result<SectionReorderPreview, Invalid> {
        preview_reorder_at(
            source,
            insertion,
            &self.arrangement_sections,
            &self.tempo_map,
        )
    }
}
// Metadata-only composition also serves copy insertion; no project/PCM clone
// or audio preparation belongs in a pointer preview.
fn preview_reorder_at(
    source: &ArrangementSection,
    insertion: BeatTime,
    sections: &[ArrangementSection],
    tempo: &crate::TempoMap,
) -> Result<SectionReorderPreview, Invalid> {
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
        tempo_map: tempo.clone(),
    };
    result
        .sections
        .try_reserve_exact(sections.len())
        .map_err(|_| Invalid("could not reserve arrangement section preview"))?;
    for old in sections {
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
    result.tempo_map = reorder::tempo_map(tempo, &result)?;
    validate(&result.sections, &result.tempo_map)?;
    Ok(result)
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
        Command::MoveArrangementSectionMarker { section, start } => {
            marker_move::apply(project, section, start)
        }
        Command::ResizeArrangementSectionMarker {
            section,
            edge,
            boundary,
        } => marker_move::resize(project, section, edge, boundary),
        Command::ReorderSectionAfter { section, after } => reorder_after(project, section, after),
        Command::ResizeArrangementSection {
            section,
            edge,
            length,
        } => splice::resize(project, section, edge, length),
        Command::ResizeArrangementSectionBoundary {
            left,
            right,
            boundary,
            marker_only,
        } => shared_boundary::apply(project, left, right, boundary, marker_only),
        Command::ResizeArrangementSectionEdge {
            section,
            edge,
            boundary,
        } => splice::resize_isolated(project, section, edge, boundary),
        Command::DeleteArrangementSection(section) => splice::delete(project, section),
        Command::DuplicateArrangementSection(section) => splice::duplicate(project, section),
        Command::DuplicateArrangementSectionAt {
            section,
            target,
            after,
        } => splice::duplicate_at(project, section, target, after),
        Command::SplitArrangementSection { section, at } => {
            let (right, mut sections) = project.preview_section_split(section, at)?;
            let id = SectionId(project.allocate_id()?);
            sections[right].id = id;
            project.arrangement_sections = sections;
            Ok(Created::ArrangementSection(id))
        }
        Command::MergeArrangementSections(selected) => {
            project.arrangement_sections = project.preview_section_merge(&selected)?.sections;
            Ok(Created::None)
        }
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
mod color;
mod meter;
mod reorder;
use reorder::{reorder, reorder_after};

mod marker_move;
mod shared_boundary;
mod splice;
pub use splice::SectionSplicePreview;
impl Project {
    /// Lightweight section/tempo display projection. It does not compile audio,
    /// transfer source custody, or promise that later preparation will succeed.
    pub fn preview_arrangement_section_edit(
        &self,
        command: &Command,
    ) -> Result<SectionSplicePreview, Invalid> {
        match command {
            Command::MoveArrangementSectionMarker { section, start } => {
                marker_move::preview(self, *section, *start)
            }
            Command::ResizeArrangementSectionMarker {
                section,
                edge,
                boundary,
            } => marker_move::preview_resize(self, *section, *edge, *boundary),
            Command::ResizeArrangementSection {
                section,
                edge,
                length,
            } => splice::preview_resize(self, *section, *edge, *length),
            Command::ResizeArrangementSectionBoundary {
                left,
                right,
                boundary,
                marker_only,
            } => shared_boundary::preview(self, *left, *right, *boundary, *marker_only),
            Command::ResizeArrangementSectionEdge {
                section,
                edge,
                boundary,
            } => splice::preview_isolated(self, *section, *edge, *boundary),
            Command::DeleteArrangementSection(section) => splice::preview_delete(self, *section),
            Command::DuplicateArrangementSection(section) => {
                splice::preview_duplicate(self, *section)
            }
            Command::DuplicateArrangementSectionAt {
                section,
                target,
                after,
            } => splice::preview_duplicate_at(self, *section, *target, *after),
            Command::SplitArrangementSection { section, at } => {
                let (_, sections) = self.preview_section_split(*section, *at)?;
                Ok(SectionSplicePreview {
                    sections,
                    tempo_map: self.tempo_map.clone(),
                })
            }
            Command::MergeArrangementSections(selected) => self.preview_section_merge(selected),
            _ => Err(Invalid("command has no section splice preview")),
        }
    }
    /// Display-only clip layout for a structural section command, built once
    /// per submitted command. It applies the same candidate edit canonical
    /// history applies, without commit validation, history, audio preparation
    /// or source custody; the canonical result may still refuse or differ.
    pub fn preview_section_layout(&self, command: &Command) -> Result<Project, Invalid> {
        if !command.requires_section_publication() {
            return Err(Invalid("command has no section clip layout preview"));
        }
        let mut command = command.clone();
        let alias = crate::clip_alias_edit::Plan::capture(self, &mut command)?;
        let mut candidate = self.clone();
        candidate.apply(command)?;
        alias.finish(&mut candidate)?;
        Ok(candidate)
    }
    // Preview uses the same zero-ID convention as copied marker previews. The
    // fresh identity is allocated only by the canonical candidate, never input.
    fn preview_section_split(
        &self,
        section: SectionId,
        at: BeatTime,
    ) -> Result<(usize, Vec<ArrangementSection>), Invalid> {
        if self.arrangement_sections.len() >= MAX_ARRANGEMENT_SECTIONS {
            return Err(Invalid("arrangement section limit reached"));
        }
        let index = self
            .arrangement_sections
            .iter()
            .position(|s| s.id == section)
            .ok_or(Invalid("arrangement section no longer exists"))?;
        let source = &self.arrangement_sections[index];
        if at <= source.start || at >= source.end {
            return Err(Invalid(
                "edit cursor must be strictly inside a section to split",
            ));
        }
        self.tempo_map.beat_to_frame(at)?;
        let mut sections = Vec::new();
        sections
            .try_reserve_exact(self.arrangement_sections.len() + 1)
            .map_err(|_| Invalid("could not reserve section split"))?;
        sections.extend(self.arrangement_sections.iter().cloned());
        let mut right = source.clone();
        right.id = SectionId(0);
        right.start = at;
        sections[index].end = at;
        sections.insert(index + 1, right);
        validate(&sections, &self.tempo_map)?;
        Ok((index + 1, sections))
    }
    fn preview_section_merge(
        &self,
        selected: &[SectionId],
    ) -> Result<SectionSplicePreview, Invalid> {
        if selected.is_empty() || selected.len() > MAX_ARRANGEMENT_SECTIONS {
            return Err(Invalid("select at least two sections to merge"));
        }
        let mut indices = Vec::new();
        indices
            .try_reserve_exact(selected.len())
            .map_err(|_| Invalid("could not reserve section merge selection"))?;
        for id in selected {
            indices.push(
                self.arrangement_sections
                    .iter()
                    .position(|s| s.id == *id)
                    .ok_or(Invalid("selected section no longer exists"))?,
            );
        }
        indices.sort_unstable();
        indices.dedup();
        if indices.len() < 2 {
            return Err(Invalid("select at least two distinct sections to merge"));
        }
        let first = indices[0];
        let last = indices[indices.len() - 1];
        let mut sections = self.arrangement_sections.clone();
        sections[first].end = sections[last].end;
        sections.drain(first + 1..=last);
        validate(&sections, &self.tempo_map)?;
        Ok(SectionSplicePreview {
            sections,
            tempo_map: self.tempo_map.clone(),
        })
    }
}
