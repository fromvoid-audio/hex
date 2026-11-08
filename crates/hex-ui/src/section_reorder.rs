//! Canonical section reorder/copy preview shared by native section hosts.
//!
//! The host owns pointer capture, snapping, selection and receipt custody. This
//! boundary chooses stable edges within the song and snapped absolute positions
//! beyond it, pairing the exact project preview with its typed command.
use hex_project::{
    ArrangementSection, BeatTime, Command, ElementColor, Invalid, Project, SectionId,
    SectionReorderPreview, SectionSplicePreview, TempoMap,
};

enum Geometry {
    Move(SectionReorderPreview),
    Splice(SectionSplicePreview),
}

pub(crate) struct Preview {
    pub(crate) command: Command,
    geometry: Geometry,
}

impl Preview {
    pub(crate) fn sections(&self) -> &[ArrangementSection] {
        match &self.geometry {
            Geometry::Move(preview) => preview.sections(),
            Geometry::Splice(preview) => preview.sections(),
        }
    }

    pub(crate) fn tempo_map(&self) -> &TempoMap {
        match &self.geometry {
            Geometry::Move(preview) => preview.tempo_map(),
            Geometry::Splice(preview) => preview.tempo_map(),
        }
    }
}

/// Beyond the final edge, use the host's snapped pointer beat. The host retains
/// `gap_appearance` for the entire capture, including live modifier changes.
/// Interior/end-edge placements retain the original structural insertion law.
pub(crate) fn drag_preview(
    project: &Project,
    source: SectionId,
    raw: BeatTime,
    snapped: BeatTime,
    copy: bool,
    gap_appearance: &mut Option<(String, ElementColor)>,
) -> Result<Option<Preview>, Invalid> {
    let end = project
        .arrangement_sections()
        .last()
        .ok_or(Invalid("arrangement has no section to move"))?
        .end();
    if raw <= end || (!copy && snapped <= end) {
        return preview(project, source, raw, copy);
    }
    let start = snapped.max(end);
    let (name, color) =
        gap_appearance.get_or_insert_with(|| crate::section_appearance::new_marker(project));
    let command = if copy {
        Command::AppendArrangementSectionCopy {
            section: source,
            start,
            gap_name: name.clone(),
            gap_color: *color,
        }
    } else {
        Command::MoveArrangementSectionBeyondEnd {
            section: source,
            start,
            gap_name: name.clone(),
            gap_color: *color,
        }
    };
    let geometry = Geometry::Splice(project.preview_arrangement_section_edit(&command)?);
    Ok(Some(Preview { command, geometry }))
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
        let geometry = Geometry::Splice(project.preview_arrangement_section_edit(&command)?);
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
    #[test]
    fn beyond_end_drag_uses_snapped_absolute_commands_and_retains_appearance() {
        let (mut editor, ids) = fixture();
        let before = editor.project().snapshot();
        let mut appearance = None;
        let copy = drag_preview(
            editor.project(),
            ids[2],
            beat(25),
            beat(24),
            true,
            &mut appearance,
        )
        .unwrap()
        .unwrap();
        let chosen = appearance.clone().unwrap();
        assert!(
            matches!(&copy.command, Command::AppendArrangementSectionCopy { start, gap_name, gap_color, .. }
            if *start == beat(24) && (gap_name, gap_color) == (&chosen.0, &chosen.1))
        );
        let moved = drag_preview(
            editor.project(),
            ids[2],
            beat(26),
            beat(26),
            false,
            &mut appearance,
        )
        .unwrap()
        .unwrap();
        assert_eq!(appearance, Some(chosen.clone()));
        assert!(
            matches!(&moved.command, Command::MoveArrangementSectionBeyondEnd { start, gap_name, gap_color, .. }
            if *start == beat(26) && (gap_name, gap_color) == (&chosen.0, &chosen.1))
        );
        assert_eq!(editor.project().snapshot(), before);
        let expected = geometry(moved.sections());
        editor.execute(moved.command).unwrap();
        assert_eq!(geometry(editor.project().arrangement_sections()), expected);
        let gap = editor
            .project()
            .arrangement_sections()
            .iter()
            .find(|m| m.name() == chosen.0)
            .unwrap();
        assert_eq!((gap.start(), gap.end()), (beat(12), beat(26)));
    }

    #[test]
    fn interior_and_exact_end_drag_keep_edges_and_do_not_allocate_gap_appearance() {
        let (editor, ids) = fixture();
        for raw in [beat(0), beat(14), beat(20)] {
            for copy in [false, true] {
                let mut appearance = None;
                let old = preview(editor.project(), ids[0], raw, copy).unwrap();
                let new = drag_preview(
                    editor.project(),
                    ids[0],
                    raw,
                    beat(30),
                    copy,
                    &mut appearance,
                )
                .unwrap();
                assert_eq!(
                    old.as_ref().map(|p| geometry(p.sections())),
                    new.as_ref().map(|p| geometry(p.sections()))
                );
                assert!(appearance.is_none());
            }
        }
        let mut appearance = None;
        let edge = drag_preview(
            editor.project(),
            ids[0],
            beat(21),
            beat(20),
            false,
            &mut appearance,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(edge.command, Command::ReorderSectionAfter { .. }));
        assert!(
            appearance.is_none(),
            "snapping to the end keeps the edge law"
        );
        let edge_copy = drag_preview(
            editor.project(),
            ids[2],
            beat(21),
            beat(20),
            true,
            &mut appearance,
        )
        .unwrap()
        .unwrap();
        assert_eq!(edge_copy.sections().len(), 4, "equal-end copy has no gap");
    }
}
