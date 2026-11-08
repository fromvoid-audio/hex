//! Absolute copied-section append composed from the established whole-song
//! duplicate/reorder laws. Only the Editor's private candidate is mutated;
//! metadata previews never clone tracks/PCM or prepare audio.
use super::*;

struct Plan {
    gap_start: BeatTime,
    insertion: BeatTime,
}

fn plan(
    project: &Project,
    section: SectionId,
    start: BeatTime,
    name: &str,
) -> Result<Plan, Invalid> {
    crate::edit::name_valid(name)?;
    let source = project
        .arrangement_section(section)
        .ok_or(Invalid("arrangement section no longer exists"))?;
    let last = project
        .arrangement_sections
        .last()
        .ok_or(Invalid("arrangement has no section to append"))?;
    if start < last.end {
        return Err(Invalid(
            "section append must start at or after the final marker",
        ));
    }
    let additional = 1 + usize::from(start > last.end);
    if project
        .arrangement_sections
        .len()
        .checked_add(additional)
        .is_none_or(|count| count > MAX_ARRANGEMENT_SECTIONS)
    {
        return Err(Invalid("arrangement section limit reached"));
    }
    let length = source
        .end
        .raw()
        .checked_sub(source.start.raw())
        .filter(|length| *length > 0)
        .ok_or(Invalid("section copy length is invalid"))?;
    // The adjacent duplicate temporarily adds this length before its later
    // insertion point. Reorder removes it again, leaving final start exact.
    let insertion = BeatTime::from_raw(
        start
            .raw()
            .checked_add(length)
            .ok_or(Invalid("appended section end overflow"))?,
    );
    Ok(Plan {
        gap_start: last.end,
        insertion,
    })
}

pub(super) fn apply(
    project: &mut Project,
    section: SectionId,
    start: BeatTime,
    gap_name: String,
    gap_color: ElementColor,
) -> Result<Created, Invalid> {
    let plan = plan(project, section, start, &gap_name)?;
    let Created::ArrangementSection(copy) = super::splice::duplicate(project, section)? else {
        return Err(Invalid("section duplication produced no identity"));
    };
    super::reorder::reorder_at(project, copy, plan.insertion)?;
    if plan.gap_start < start {
        // Constant-depth owner composition; the outer Editor owns rollback.
        super::apply(
            project,
            Command::AddArrangementSection {
                name: gap_name,
                color: gap_color,
                start: plan.gap_start,
                end: start,
            },
        )?;
    }
    Ok(Created::ArrangementSection(copy))
}

pub(super) fn preview(
    project: &Project,
    section: SectionId,
    start: BeatTime,
    gap_name: &str,
    gap_color: ElementColor,
) -> Result<SectionSplicePreview, Invalid> {
    let plan = plan(project, section, start, gap_name)?;
    let layout = super::splice::preview_duplicate(project, section)?;
    let copy = layout
        .sections
        .iter()
        .find(|marker| marker.id == SectionId(0))
        .ok_or(Invalid("section copy preview is missing"))?;
    let reordered =
        super::preview_reorder_at(copy, plan.insertion, &layout.sections, &layout.tempo_map)?;
    let mut sections = reordered.sections;
    if plan.gap_start < start {
        sections.push(ArrangementSection {
            id: SectionId(0),
            name: gap_name.into(),
            color: gap_color,
            start: plan.gap_start,
            end: start,
        });
        sections.sort_by_key(|marker| marker.start);
    }
    super::validate(&sections, &reordered.tempo_map)?;
    Ok(SectionSplicePreview {
        sections,
        tempo_map: reordered.tempo_map,
    })
}

#[cfg(test)]
mod tests;
