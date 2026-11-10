//! Atomic replacement composes the existing whole-song deletion, copy and
//! reorder laws inside the Editor's private candidate. No intermediate state
//! acquires history/publication or independent media custody.
use super::*;

fn copy_insertion(
    copy_start: BeatTime,
    destination: BeatTime,
    length: u64,
) -> Result<BeatTime, Invalid> {
    if copy_start <= destination {
        Ok(BeatTime::from_raw(
            destination
                .raw()
                .checked_add(length)
                .ok_or(Invalid("section replacement insertion overflow"))?,
        ))
    } else {
        Ok(destination)
    }
}

pub(super) fn apply(
    project: &mut Project,
    source: SectionId,
    target: SectionId,
    copy: bool,
) -> Result<Created, Invalid> {
    let color = project
        .arrangement_section(source)
        .ok_or(Invalid("arrangement source no longer exists"))?
        .color;
    let destination = project
        .arrangement_section(target)
        .ok_or(Invalid("arrangement target no longer exists"))?
        .start;
    if source == target {
        return Ok(Created::None);
    }
    super::splice::delete(project, target)?;
    if copy {
        let Created::ArrangementSection(id) =
            super::splice::duplicate_colored(project, source, color)?
        else {
            return Err(Invalid("section replacement copy produced no identity"));
        };
        let marker = project
            .arrangement_section(id)
            .ok_or(Invalid("section replacement copy is missing"))?;
        let insertion = copy_insertion(
            marker.start,
            destination,
            marker.end.raw() - marker.start.raw(),
        )?;
        super::reorder::reorder_at(project, id, insertion)?;
        Ok(Created::ArrangementSection(id))
    } else {
        // The old target slot is expressed before the source cut. Existing
        // reorder subtracts source length for a later destination.
        super::reorder::reorder_at(project, source, destination)?;
        Ok(Created::None)
    }
}

pub(super) fn preview(
    project: &Project,
    source: SectionId,
    target: SectionId,
    copy: bool,
) -> Result<SectionSplicePreview, Invalid> {
    let color = project
        .arrangement_section(source)
        .ok_or(Invalid("arrangement source no longer exists"))?
        .color;
    let destination = project
        .arrangement_section(target)
        .ok_or(Invalid("arrangement target no longer exists"))?
        .start;
    if source == target {
        return Ok(SectionSplicePreview {
            sections: project.arrangement_sections.clone(),
            tempo_map: project.tempo_map.clone(),
        });
    }
    let deleted = super::splice::preview_delete(project, target)?;
    let (layout, id) = if copy {
        (
            super::splice::preview_duplicate_in(
                &deleted.sections,
                &deleted.tempo_map,
                source,
                color,
            )?,
            SectionId(0),
        )
    } else {
        (deleted, source)
    };
    let marker = layout
        .sections
        .iter()
        .find(|m| m.id == id)
        .ok_or(Invalid("section replacement preview is missing"))?;
    let insertion = if copy {
        copy_insertion(
            marker.start,
            destination,
            marker.end.raw() - marker.start.raw(),
        )?
    } else {
        destination
    };
    let reordered =
        super::preview_reorder_at(marker, insertion, &layout.sections, &layout.tempo_map)?;
    Ok(SectionSplicePreview {
        sections: reordered.sections,
        tempo_map: reordered.tempo_map,
    })
}

#[cfg(test)]
mod tests;
