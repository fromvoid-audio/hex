//! Canonical section reorder/copy preview shared by native section hosts.
//!
//! The host owns pointer capture, snapping, selection and receipt custody. This
//! boundary chooses the nearest stable project edge and pairs the exact project
//! preview with the command that can produce it.
use hex_project::{
    ArrangementSection, BeatTime, Command, Invalid, Project, SectionId, SectionReorderPreview,
    SectionSplicePreview, TempoMap,
};

enum Geometry {
    Move(SectionReorderPreview),
    Copy(SectionSplicePreview),
}

pub(crate) struct Preview {
    pub(crate) command: Command,
    geometry: Geometry,
}

impl Preview {
    pub(crate) fn sections(&self) -> &[ArrangementSection] {
        match &self.geometry {
            Geometry::Move(preview) => preview.sections(),
            Geometry::Copy(preview) => preview.sections(),
        }
    }

    pub(crate) fn tempo_map(&self) -> &TempoMap {
        match &self.geometry {
            Geometry::Move(preview) => preview.tempo_map(),
            Geometry::Copy(preview) => preview.tempo_map(),
        }
    }
}

/// Resolve a raw pointer beat to the same insertion edge and project preview
/// used to create the command. `None` represents an unchanged move.
pub(crate) fn preview(
    project: &Project,
    source: SectionId,
    beat: BeatTime,
    copy: bool,
) -> Result<Option<Preview>, Invalid> {
    let Some((target, after)) = insertion_target(project, source, beat, copy) else {
        return Ok(None);
    };
    if copy {
        let command = Command::DuplicateArrangementSectionAt {
            section: source,
            target,
            after,
        };
        let geometry = Geometry::Copy(project.preview_arrangement_section_edit(&command)?);
        return Ok(Some(Preview { command, geometry }));
    }
    let (command, geometry) = if after {
        (
            Command::ReorderSectionAfter {
                section: source,
                after: target,
            },
            project.preview_section_reorder_after(source, target)?,
        )
    } else {
        (
            Command::ReorderSectionBefore {
                section: source,
                before: target,
            },
            project.preview_section_reorder(source, target)?,
        )
    };
    if geometry.is_noop() {
        return Ok(None);
    }
    Ok(Some(Preview {
        command,
        geometry: Geometry::Move(geometry),
    }))
}

pub(crate) fn insertion_target(
    project: &Project,
    source: SectionId,
    beat: BeatTime,
    copy: bool,
) -> Option<(SectionId, bool)> {
    if !copy
        && project
            .arrangement_section(source)
            .is_some_and(|section| section.start() <= beat && beat < section.end())
    {
        return None;
    }
    project
        .arrangement_sections()
        .iter()
        .filter(|section| copy || section.id() != source)
        .flat_map(|section| {
            [
                (section.id(), false, section.start()),
                (section.id(), true, section.end()),
            ]
        })
        .min_by_key(|(_, _, at)| at.raw().abs_diff(beat.raw()))
        .map(|(id, after, _)| (id, after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Created, Editor, ElementColor};

    fn beat(quarters: u64) -> BeatTime {
        BeatTime::from_quarters(quarters).unwrap()
    }

    fn fixture() -> (Editor, [SectionId; 3]) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut ids = Vec::new();
        for (start, end) in [(0, 4), (8, 12), (16, 20)] {
            let Created::ArrangementSection(id) = editor
                .execute(Command::AddArrangementSection {
                    name: format!("section {}", ids.len() + 1),
                    color: ElementColor::Cyan,
                    start: beat(start),
                    end: beat(end),
                })
                .unwrap()
            else {
                panic!("fixture must create a section");
            };
            ids.push(id);
        }
        (editor, [ids[0], ids[1], ids[2]])
    }

    fn geometry(sections: &[ArrangementSection]) -> Vec<(String, BeatTime, BeatTime)> {
        sections
            .iter()
            .map(|section| (section.name().to_owned(), section.start(), section.end()))
            .collect()
    }

    #[test]
    fn move_preview_and_command_share_canonical_geometry() {
        let (mut editor, ids) = fixture();
        let original = editor.project().snapshot();
        let preview = preview(editor.project(), ids[0], beat(16), false)
            .unwrap()
            .unwrap();
        assert!(matches!(
            preview.command,
            Command::ReorderSectionBefore { section, before }
                if section == ids[0] && before == ids[2]
        ));
        let expected = geometry(preview.sections());
        let tempo = preview.tempo_map().clone();
        assert_eq!(editor.project().snapshot(), original);
        editor.execute(preview.command).unwrap();
        assert_eq!(geometry(editor.project().arrangement_sections()), expected);
        assert_eq!(editor.project().tempo_map(), &tempo);
    }

    #[test]
    fn copy_to_self_uses_real_preview_and_leaves_source_unchanged() {
        let (mut editor, ids) = fixture();
        let original = editor.project().snapshot();
        assert!(
            preview(editor.project(), ids[0], beat(1), false)
                .unwrap()
                .is_none()
        );
        let preview = preview(editor.project(), ids[0], beat(1), true)
            .unwrap()
            .unwrap();
        assert!(matches!(
            preview.command,
            Command::DuplicateArrangementSectionAt { section, target, after: false }
                if section == ids[0] && target == ids[0]
        ));
        let expected = geometry(preview.sections());
        assert_eq!(editor.project().snapshot(), original);
        editor.execute(preview.command).unwrap();
        assert_eq!(geometry(editor.project().arrangement_sections()), expected);
        assert!(editor.project().arrangement_section(ids[0]).is_some());
    }

    #[test]
    fn moving_before_an_adjacent_target_is_a_noop() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut first = None;
        for (start, end) in [(0, 4), (4, 8)] {
            let Created::ArrangementSection(id) = editor
                .execute(Command::AddArrangementSection {
                    name: format!("section {start}"),
                    color: ElementColor::Cyan,
                    start: beat(start),
                    end: beat(end),
                })
                .unwrap()
            else {
                panic!("fixture must create a section");
            };
            if first.is_none() {
                first = Some(id);
            }
        }
        let original = editor.project().snapshot();
        assert!(
            preview(editor.project(), first.unwrap(), beat(4), false)
                .unwrap()
                .is_none()
        );
        assert_eq!(editor.project().snapshot(), original);
    }

    #[test]
    fn closest_edge_ties_are_stable_and_missing_source_is_refused() {
        let (mut editor, ids) = fixture();
        assert_eq!(
            insertion_target(editor.project(), ids[0], beat(14), false),
            Some((ids[1], true))
        );
        editor
            .execute(Command::RemoveArrangementSection(ids[0]))
            .unwrap();
        assert!(preview(editor.project(), ids[0], beat(12), false).is_err());
    }
}
