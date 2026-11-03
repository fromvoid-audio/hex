//! In-memory project custody for the sandbox's section demonstration.
use hex_project::{
    ArrangementSection, BeatTime, Command, Created, Editor, ElementColor, Invalid, Project,
    SectionId, TrimEdge,
};
use hex_widgets::RectanglePart;

const VIEW_QUARTERS: u64 = 10;

pub(super) struct Fixture {
    editor: Editor,
}

pub(super) struct Plan {
    pub(super) command: Command,
    pub(super) display: Vec<[f32; 2]>,
    pub(super) sections: Vec<ArrangementSection>,
}

impl Fixture {
    pub(super) fn new() -> Result<Self, Invalid> {
        let mut editor = Editor::new(Project::new(48_000)?, 16)?;
        for (name, color, start, end) in [
            ("section 1", ElementColor::Cyan, 1, 5),
            ("section 2", ElementColor::Blue, 5, 9),
        ] {
            let created = editor.execute(Command::AddArrangementSection {
                name: name.into(),
                color,
                start: BeatTime::from_quarters(start)?,
                end: BeatTime::from_quarters(end)?,
            })?;
            if !matches!(created, Created::ArrangementSection(_)) {
                return Err(Invalid("section fixture creation failed"));
            }
        }
        Ok(Self { editor })
    }

    pub(super) fn project(&self) -> &Project {
        self.editor.project()
    }

    pub(super) fn display(&self) -> Vec<[f32; 2]> {
        display(self.project().arrangement_sections())
    }

    pub(super) fn position(&self, beat: BeatTime) -> f32 {
        (beat.raw() as f64 / view_span(self.project().arrangement_sections()) as f64) as f32
    }

    pub(super) fn plan(
        &self,
        section: SectionId,
        part: RectanglePart,
        position: f32,
        copy: bool,
    ) -> Result<Option<Plan>, Invalid> {
        let project = self.project();
        let beat = position_to_beat(position, view_span(project.arrangement_sections()))?;
        let Some(index) = project
            .arrangement_sections()
            .iter()
            .position(|s| s.id() == section)
        else {
            return Err(Invalid("arrangement section no longer exists"));
        };
        match part {
            RectanglePart::Body => Ok(crate::section_reorder::preview(
                project, section, beat, copy,
            )?
            .map(|preview| Plan {
                display: display(preview.sections()),
                sections: preview.sections().to_vec(),
                command: preview.command,
            })),
            RectanglePart::Leading | RectanglePart::Trailing => {
                let edge = if part == RectanglePart::Leading {
                    TrimEdge::Left
                } else {
                    TrimEdge::Right
                };
                let preview = crate::section_resize::preview(
                    project,
                    crate::section_resize::Edit::Content {
                        section,
                        edge,
                        boundary: beat,
                    },
                )?;
                Ok(Some(Plan {
                    display: display(preview.sections()),
                    sections: preview.sections().to_vec(),
                    command: preview.command,
                }))
            }
            RectanglePart::Seam => {
                let Some(right) = project.arrangement_sections().get(index + 1) else {
                    return Err(Invalid("section boundary no longer exists"));
                };
                let preview = crate::section_resize::preview(
                    project,
                    crate::section_resize::Edit::Shared {
                        left: section,
                        right: right.id(),
                        boundary: beat,
                        marker_only: false,
                    },
                )?;
                Ok(Some(Plan {
                    display: display(preview.sections()),
                    sections: preview.sections().to_vec(),
                    command: preview.command,
                }))
            }
        }
    }

    pub(super) fn commit(&mut self, command: Command) -> Result<Created, Invalid> {
        self.editor.execute(command)
    }
}

fn view_span(sections: &[ArrangementSection]) -> u64 {
    sections
        .iter()
        .map(|section| section.end().raw())
        .max()
        .unwrap_or(0)
        .max(VIEW_QUARTERS * BeatTime::ONE.raw())
}

fn display(sections: &[ArrangementSection]) -> Vec<[f32; 2]> {
    let span = view_span(sections) as f64;
    sections
        .iter()
        .map(|section| {
            [
                (section.start().raw() as f64 / span) as f32,
                (section.end().raw() as f64 / span) as f32,
            ]
        })
        .collect()
}

fn position_to_beat(position: f32, span: u64) -> Result<BeatTime, Invalid> {
    if !position.is_finite() {
        return Err(Invalid("invalid section position"));
    }
    let span = span as f64;
    Ok(BeatTime::from_raw(
        (f64::from(position.clamp(0.0, 1.0)) * span).round() as u64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_seam_preview_commit_refusal_and_undo() {
        let mut fixture = Fixture::new().unwrap();
        let before = fixture.project().snapshot();
        let id = fixture.project().arrangement_sections()[0].id();
        let plan = fixture
            .plan(id, RectanglePart::Seam, 0.55, false)
            .unwrap()
            .unwrap();
        let expected = plan.display.clone();
        assert_eq!(fixture.project().snapshot(), before);
        fixture.commit(plan.command).unwrap();
        assert_eq!(fixture.display(), expected);
        let committed = fixture.project().snapshot();
        assert!(fixture.plan(id, RectanglePart::Seam, 0.99, false).is_err());
        assert_eq!(fixture.project().snapshot(), committed);
        assert!(fixture.editor.try_undo().unwrap());
        assert_eq!(fixture.project().snapshot(), before);
    }

    #[test]
    fn tiny_canonical_gap_does_not_become_a_shared_seam() {
        let mut fixture = Fixture::new().unwrap();
        let second = fixture.project().arrangement_sections()[1].id();
        let start = BeatTime::from_raw(5 * BeatTime::ONE.raw() + 1);
        fixture
            .commit(Command::SetArrangementSectionRange {
                section: second,
                start,
                end: BeatTime::from_quarters(9).unwrap(),
            })
            .unwrap();
        let sections = fixture.project().arrangement_sections();
        assert_eq!(
            fixture.position(sections[0].end()),
            fixture.position(sections[1].start())
        );
        let hit = crate::section_hit::hit(
            sections
                .iter()
                .enumerate()
                .map(|(index, section)| (index, section.start(), section.end())),
            fixture.position(sections[0].end()),
            |beat| Some(fixture.position(beat)),
        );
        assert!(!matches!(hit, Some(crate::section_hit::Hit::Seam(_, _))));
    }

    #[test]
    fn fixture_keeps_all_real_sections_after_copy() {
        let mut fixture = Fixture::new().unwrap();
        let id = fixture.project().arrangement_sections()[0].id();
        fixture
            .editor
            .execute(Command::DuplicateArrangementSectionAt {
                section: id,
                target: id,
                after: true,
            })
            .unwrap();
        let sections = fixture.project().arrangement_sections();
        assert_eq!(sections.len(), 3);
        assert_eq!(fixture.display().len(), sections.len());
        assert!(sections.iter().any(|section| section.id() == id));
    }
}
