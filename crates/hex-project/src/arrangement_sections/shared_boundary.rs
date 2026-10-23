//! One shared seam edit, using the established edge splice law. Shrink first so
//! the intermediate candidate cannot overflow song time merely because the pair
//! temporarily grew. Editor owns candidate rollback, one preparation and history.
//! Preview composes only bounded marker/tempo projections, never audio or tracks.
use super::{ArrangementSection, BeatTime, Created, Invalid, Project, SectionId, splice};
use crate::TrimEdge;

fn pair(
    sections: &[ArrangementSection],
    left: SectionId,
    right: SectionId,
    boundary: BeatTime,
) -> Result<(usize, usize), Invalid> {
    let index = sections
        .iter()
        .position(|s| s.id == left)
        .ok_or(Invalid("left arrangement section no longer exists"))?;
    let next = index
        .checked_add(1)
        .ok_or(Invalid("section boundary index overflow"))?;
    let a = sections
        .get(index)
        .ok_or(Invalid("left section is missing"))?;
    let b = sections.get(next).filter(|s| s.id == right).ok_or(Invalid(
        "shared resize requires adjacent section identities",
    ))?;
    if a.end != b.start {
        return Err(Invalid("shared resize requires touching sections"));
    }
    if boundary <= a.start || boundary >= b.end {
        return Err(Invalid(
            "shared resize must leave both sections a positive length",
        ));
    }
    Ok((index, next))
}

pub(super) fn apply(
    project: &mut Project,
    left: SectionId,
    right: SectionId,
    boundary: BeatTime,
    marker_only: bool,
) -> Result<Created, Invalid> {
    let (a, b) = pair(&project.arrangement_sections, left, right, boundary)?;
    let old = project.arrangement_sections[a].end;
    if old == boundary {
        return Ok(Created::None);
    }
    if marker_only {
        project.arrangement_sections[a].end = boundary;
        project.arrangement_sections[b].start = boundary;
        super::validate(&project.arrangement_sections, &project.tempo_map)?;
    } else {
        let left_length =
            BeatTime::from_raw(boundary.raw() - project.arrangement_sections[a].start.raw());
        let right_length =
            BeatTime::from_raw(project.arrangement_sections[b].end.raw() - boundary.raw());
        if boundary > old {
            splice::resize(project, right, TrimEdge::Left, right_length)?;
            splice::resize(project, left, TrimEdge::Right, left_length)?;
        } else {
            splice::resize(project, left, TrimEdge::Right, left_length)?;
            splice::resize(project, right, TrimEdge::Left, right_length)?;
        }
    }
    Ok(Created::None)
}

pub(super) fn preview(
    project: &Project,
    left: SectionId,
    right: SectionId,
    boundary: BeatTime,
    marker_only: bool,
) -> Result<splice::SectionSplicePreview, Invalid> {
    let (a, b) = pair(&project.arrangement_sections, left, right, boundary)?;
    let sections = &project.arrangement_sections;
    if marker_only || sections[a].end == boundary {
        let mut result = Vec::new();
        result
            .try_reserve_exact(sections.len())
            .map_err(|_| Invalid("could not reserve shared section preview"))?;
        result.extend(sections.iter().cloned());
        result[a].end = boundary;
        result[b].start = boundary;
        super::validate(&result, &project.tempo_map)?;
        return Ok(splice::SectionSplicePreview {
            sections: result,
            tempo_map: project.tempo_map.clone(),
        });
    }
    let left_length = BeatTime::from_raw(boundary.raw() - sections[a].start.raw());
    let right_length = BeatTime::from_raw(sections[b].end.raw() - boundary.raw());
    let (first, edge, length, second, next_edge, next_length) = if boundary > sections[a].end {
        (
            right,
            TrimEdge::Left,
            right_length,
            left,
            TrimEdge::Right,
            left_length,
        )
    } else {
        (
            left,
            TrimEdge::Right,
            left_length,
            right,
            TrimEdge::Left,
            right_length,
        )
    };
    let preview = splice::preview_resize_parts(sections, &project.tempo_map, first, edge, length)?;
    splice::preview_resize_parts(
        &preview.sections,
        &preview.tempo_map,
        second,
        next_edge,
        next_length,
    )
}
