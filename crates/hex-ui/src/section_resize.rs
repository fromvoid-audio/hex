//! Canonical section resize previews shared by native section hosts.
//!
//! Hosts own input capture, snapping, modifier latching and receipt custody.
//! This boundary pairs a typed resize intent with the project's exact preview
//! and the command that can produce it.
use hex_project::{
    ArrangementSection, BeatTime, Command, Invalid, Project, SectionId, SectionSplicePreview,
    TempoMap, TrimEdge,
};

pub(crate) enum Edit {
    Marker {
        section: SectionId,
        edge: TrimEdge,
        boundary: BeatTime,
    },
    Content {
        section: SectionId,
        edge: TrimEdge,
        boundary: BeatTime,
    },
    Shared {
        left: SectionId,
        right: SectionId,
        boundary: BeatTime,
        marker_only: bool,
    },
}

pub(crate) struct Preview {
    pub(crate) command: Command,
    geometry: SectionSplicePreview,
}

impl Preview {
    pub(crate) fn sections(&self) -> &[ArrangementSection] {
        self.geometry.sections()
    }

    pub(crate) fn tempo_map(&self) -> &TempoMap {
        self.geometry.tempo_map()
    }
}

pub(crate) fn preview(project: &Project, edit: Edit) -> Result<Preview, Invalid> {
    let command = match edit {
        Edit::Marker {
            section,
            edge,
            boundary,
        } => Command::ResizeArrangementSectionMarker {
            section,
            edge,
            boundary,
        },
        Edit::Content {
            section,
            edge,
            boundary,
        } => Command::ResizeArrangementSectionEdge {
            section,
            edge,
            boundary,
        },
        Edit::Shared {
            left,
            right,
            boundary,
            marker_only,
        } => Command::ResizeArrangementSectionBoundary {
            left,
            right,
            boundary,
            marker_only,
        },
    };
    let geometry = project.preview_arrangement_section_edit(&command)?;
    Ok(Preview { command, geometry })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Created, Editor, ElementColor};

    fn beat(quarters: u64) -> BeatTime {
        BeatTime::from_quarters(quarters).unwrap()
    }

    fn fixture() -> (Editor, [SectionId; 2]) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut ids = Vec::new();
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
            ids.push(id);
        }
        (editor, [ids[0], ids[1]])
    }

    fn commit_and_compare(editor: &mut Editor, preview: Preview) {
        let expected_sections = preview.sections().to_vec();
        let expected_tempo = preview.tempo_map().clone();
        editor.execute(preview.command).unwrap();
        assert_eq!(editor.project().arrangement_sections(), expected_sections);
        assert_eq!(editor.project().tempo_map(), &expected_tempo);
    }

    #[test]
    fn marker_and_content_edges_use_their_distinct_canonical_commands() {
        for marker_only in [true, false] {
            let (mut editor, ids) = fixture();
            let before = editor.project().snapshot();
            let edit = if marker_only {
                Edit::Marker {
                    section: ids[1],
                    edge: TrimEdge::Left,
                    boundary: beat(5),
                }
            } else {
                Edit::Content {
                    section: ids[1],
                    edge: TrimEdge::Left,
                    boundary: beat(5),
                }
            };
            let result = preview(editor.project(), edit).unwrap();
            assert!(if marker_only {
                matches!(result.command, Command::ResizeArrangementSectionMarker { section, edge: TrimEdge::Left, boundary } if section == ids[1] && boundary == beat(5))
            } else {
                matches!(result.command, Command::ResizeArrangementSectionEdge { section, edge: TrimEdge::Left, boundary } if section == ids[1] && boundary == beat(5))
            });
            assert_eq!(editor.project().snapshot(), before);
            commit_and_compare(&mut editor, result);
        }
    }

    #[test]
    fn shared_boundary_preserves_marker_only_intent_and_matches_commit() {
        for marker_only in [false, true] {
            let (mut editor, ids) = fixture();
            let before = editor.project().snapshot();
            let result = preview(
                editor.project(),
                Edit::Shared {
                    left: ids[0],
                    right: ids[1],
                    boundary: beat(5),
                    marker_only,
                },
            )
            .unwrap();
            assert!(
                matches!(result.command, Command::ResizeArrangementSectionBoundary {
                left, right, boundary, marker_only: mode,
            } if left == ids[0] && right == ids[1] && boundary == beat(5) && mode == marker_only)
            );
            assert_eq!(editor.project().snapshot(), before);
            commit_and_compare(&mut editor, result);
        }
    }

    #[test]
    fn invalid_shared_boundary_is_refused_without_mutation() {
        let (editor, ids) = fixture();
        let before = editor.project().snapshot();
        assert!(
            preview(
                editor.project(),
                Edit::Shared {
                    left: ids[0],
                    right: ids[1],
                    boundary: beat(9),
                    marker_only: false,
                }
            )
            .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
    }
}
