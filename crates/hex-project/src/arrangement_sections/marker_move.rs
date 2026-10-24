//! Bounded marker-only destination carving shared by preview and canonical edit.
//! No track, clip, tempo, meter, loop or source data is visited or changed.
use super::{MAX_ARRANGEMENT_SECTIONS, SectionSplicePreview, validate};
use crate::{BeatTime, Created, Invalid, Project, SectionId};

pub(super) fn preview(
    project: &Project,
    section: SectionId,
    start: BeatTime,
) -> Result<SectionSplicePreview, Invalid> {
    let source = project
        .arrangement_section(section)
        .ok_or(Invalid("arrangement section no longer exists"))?;
    let length = source
        .end
        .raw()
        .checked_sub(source.start.raw())
        .filter(|length| *length > 0)
        .ok_or(Invalid("arrangement section range must be positive"))?;
    let end = start
        .raw()
        .checked_add(length)
        .map(BeatTime::from_raw)
        .ok_or(Invalid("section end overflow"))?;
    preview_range(project, section, start, end)
}

pub(super) fn preview_resize(
    project: &Project,
    section: SectionId,
    edge: crate::TrimEdge,
    boundary: BeatTime,
) -> Result<SectionSplicePreview, Invalid> {
    let source = project
        .arrangement_section(section)
        .ok_or(Invalid("arrangement section no longer exists"))?;
    let (start, end) = match edge {
        crate::TrimEdge::Left => (boundary, source.end),
        crate::TrimEdge::Right => (source.start, boundary),
    };
    preview_range(project, section, start, end)
}

fn preview_range(
    project: &Project,
    section: SectionId,
    start: BeatTime,
    end: BeatTime,
) -> Result<SectionSplicePreview, Invalid> {
    if start >= end {
        return Err(Invalid("arrangement section range must be positive"));
    }
    let source = project
        .arrangement_section(section)
        .ok_or(Invalid("arrangement section no longer exists"))?;
    project.tempo_map().beat_to_frame(start)?;
    project.tempo_map().beat_to_frame(end)?;
    let mut sections = Vec::new();
    // A single continuous destination can split at most one disjoint marker.
    let capacity = project
        .arrangement_sections
        .len()
        .checked_add(1)
        .ok_or(Invalid("arrangement section count overflow"))?;
    sections
        .try_reserve_exact(capacity)
        .map_err(|_| Invalid("could not reserve section marker move"))?;
    for old in &project.arrangement_sections {
        if old.id == section {
            continue;
        }
        if old.end <= start || old.start >= end {
            sections.push(old.clone());
            continue;
        }
        let left_survives = old.start < start;
        if left_survives {
            let mut left = old.clone();
            left.end = start;
            sections.push(left);
        }
        if old.end > end {
            let mut right = old.clone();
            right.start = end;
            if left_survives {
                // Same ghost convention as section split: only commit mints IDs.
                right.id = SectionId(0);
            }
            sections.push(right);
        }
    }
    let mut moved = source.clone();
    moved.start = start;
    moved.end = end;
    sections.push(moved);
    if sections.len() > MAX_ARRANGEMENT_SECTIONS {
        return Err(Invalid("arrangement section limit reached"));
    }
    sections.sort_by_key(|marker| marker.start);
    validate(&sections, project.tempo_map())?;
    Ok(SectionSplicePreview {
        sections,
        tempo_map: project.tempo_map.clone(),
    })
}

pub(super) fn apply(
    project: &mut Project,
    section: SectionId,
    start: BeatTime,
) -> Result<Created, Invalid> {
    // Editor calls this only on its private candidate; allocation refusal cannot
    // consume live IDs, discard redo or publish a partly carved marker set.
    let result = preview(project, section, start)?;
    install(project, result)
}

pub(super) fn resize(
    project: &mut Project,
    section: SectionId,
    edge: crate::TrimEdge,
    boundary: BeatTime,
) -> Result<Created, Invalid> {
    let result = preview_resize(project, section, edge, boundary)?;
    install(project, result)
}

fn install(project: &mut Project, mut result: SectionSplicePreview) -> Result<Created, Invalid> {
    for marker in &mut result.sections {
        if marker.id.value() == 0 {
            marker.id = SectionId(project.allocate_id()?);
        }
    }
    project.arrangement_sections = result.sections;
    Ok(Created::None)
}
