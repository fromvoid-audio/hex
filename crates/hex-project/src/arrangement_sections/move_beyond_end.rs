//! Absolute whole-song moves reuse the established cut/insert law. The Editor
//! owns the private candidate and rolls back both music and marker on refusal.
use super::*;

struct Plan {
    insertion: BeatTime,
    gap_start: BeatTime,
    layout: SectionReorderPreview,
}

fn plan(
    project: &Project,
    section: SectionId,
    start: BeatTime,
    gap_name: &str,
) -> Result<Plan, Invalid> {
    crate::edit::name_valid(gap_name)?;
    let source = project
        .arrangement_section(section)
        .ok_or(Invalid("arrangement section no longer exists"))?;
    let last = project
        .arrangement_sections
        .last()
        .ok_or(Invalid("arrangement has no section to move"))?;
    if start <= last.end {
        return Err(Invalid("section move must start beyond the final marker"));
    }
    if project.arrangement_sections.len() >= MAX_ARRANGEMENT_SECTIONS {
        return Err(Invalid("arrangement section limit reached"));
    }
    let length = source
        .end
        .raw()
        .checked_sub(source.start.raw())
        .filter(|length| *length > 0)
        .ok_or(Invalid("section move length is invalid"))?;
    // Reorder resolves an insertion in the old timeline. Its cut subtracts the
    // source length, leaving the requested final start exactly.
    let insertion = BeatTime::from_raw(
        start
            .raw()
            .checked_add(length)
            .ok_or(Invalid("moved section end overflow"))?,
    );
    let layout = project.preview_section_reorder_at(source, insertion)?;
    let gap_start = layout
        .sections
        .iter()
        .filter(|marker| marker.id != section)
        .map(|marker| marker.end)
        .max()
        .unwrap_or(BeatTime::ZERO);
    Ok(Plan {
        insertion,
        gap_start,
        layout,
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
    super::reorder::reorder_at(project, section, plan.insertion)?;
    // Constant-depth owner composition; no second history or publication edge.
    super::apply(
        project,
        Command::AddArrangementSection {
            name: gap_name,
            color: gap_color,
            start: plan.gap_start,
            end: start,
        },
    )?;
    Ok(Created::None)
}

pub(super) fn preview(
    project: &Project,
    section: SectionId,
    start: BeatTime,
    gap_name: &str,
    gap_color: ElementColor,
) -> Result<SectionSplicePreview, Invalid> {
    let plan = plan(project, section, start, gap_name)?;
    let mut sections = plan.layout.sections;
    sections.push(ArrangementSection {
        id: SectionId(0),
        name: gap_name.into(),
        color: gap_color,
        start: plan.gap_start,
        end: start,
    });
    sections.sort_by_key(|marker| marker.start);
    super::validate(&sections, &plan.layout.tempo_map)?;
    Ok(SectionSplicePreview {
        sections,
        tempo_map: plan.layout.tempo_map,
    })
}

#[cfg(test)]
mod tests;
