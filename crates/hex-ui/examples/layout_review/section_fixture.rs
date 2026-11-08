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

    #[cfg(test)]
    pub(super) fn plan(
        &self,
        section: SectionId,
        part: RectanglePart,
        position: f32,
        copy: bool,
        marker_only: bool,
    ) -> Result<Option<Plan>, Invalid> {
        let beat = position_to_beat(position, view_span(self.project().arrangement_sections()))?;
        self.plan_at_beat(section, part, beat, copy, marker_only)
    }

    pub(super) fn plan_at_beat(
        &self,
        section: SectionId,
        part: RectanglePart,
        beat: BeatTime,
        copy: bool,
        marker_only: bool,
    ) -> Result<Option<Plan>, Invalid> {
        let project = self.project();
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
                let edit = if marker_only {
                    crate::section_resize::Edit::Marker {
                        section,
                        edge,
                        boundary: beat,
                    }
                } else {
                    crate::section_resize::Edit::Content {
                        section,
                        edge,
                        boundary: beat,
                    }
                };
                let preview = crate::section_resize::preview(project, edit)?;
                Ok(Some(Plan {
                    display: display(preview.sections()),
                    sections: preview.sections().to_vec(),
                    command: preview.command,
                }))
            }
            RectanglePart::Seam => {
                if marker_only {
                    let preview = crate::section_resize::preview(
                        project,
                        crate::section_resize::Edit::Marker {
                            section,
                            edge: TrimEdge::Right,
                            boundary: beat,
                        },
                    )?;
                    return Ok(Some(Plan {
                        display: display(preview.sections()),
                        sections: preview.sections().to_vec(),
                        command: preview.command,
                    }));
                }
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

    pub(super) fn plan_drag(
        &self,
        section: SectionId,
        raw: BeatTime,
        snapped: BeatTime,
        copy: bool,
        gap_appearance: &mut Option<(String, ElementColor)>,
    ) -> Result<Option<Plan>, Invalid> {
        Ok(crate::section_reorder::drag_preview(
            self.project(),
            section,
            raw,
            snapped,
            copy,
            gap_appearance,
        )?
        .map(|preview| Plan {
            display: display(preview.sections()),
            sections: preview.sections().to_vec(),
            command: preview.command,
        }))
    }

    /// Resolve this audio-free, fixed-tempo fixture's lane pointer to the
    /// same frame viewport and adaptive grid used by arrangement input.
    pub(super) fn snapped_beat(
        &self,
        pointer: f32,
        left: f32,
        width: f32,
        free: bool,
    ) -> Result<BeatTime, Invalid> {
        if !pointer.is_finite() || !left.is_finite() || !width.is_finite() || width <= 0.0 {
            return Err(Invalid("invalid section grid position"));
        }
        let project = self.project();
        let viewport = self.grid_viewport()?;
        let frame = crate::seek::pointer_to_frame(pointer, left, width, viewport)
            .ok_or(Invalid("section grid position unavailable"))?;
        let beat = project
            .tempo_map()
            .frame_to_beat(frame)
            .map_err(|_| Invalid("section grid position unavailable"))?;
        crate::musical_time::SnapGrid::Adaptive
            .with_platform_modifier(free)
            .snap_beat(project, beat, viewport, width)
            .ok_or(Invalid("section grid position unavailable"))
    }

    fn grid_viewport(&self) -> Result<crate::viewport::Viewport, Invalid> {
        let project = self.project();
        let span = BeatTime::from_raw(view_span(project.arrangement_sections()));
        let length = project
            .tempo_map()
            .beat_to_frame(span)
            .map_err(|_| Invalid("section grid viewport unavailable"))?;
        if length == 0 {
            return Err(Invalid("section grid viewport unavailable"));
        }
        Ok(crate::viewport::Viewport::fit(length))
    }

    pub(super) fn paint_point_end(
        &self,
        anchor: BeatTime,
        width: f32,
        free: bool,
    ) -> Result<BeatTime, Invalid> {
        if !width.is_finite() || width <= 0.0 {
            return Err(Invalid("invalid section grid position"));
        }
        let viewport = self.grid_viewport()?;
        crate::musical_time::SnapGrid::Adaptive
            .with_platform_modifier(free)
            .adjacent_beat(self.project(), anchor, viewport, width, true)
            .or_else(|| {
                anchor
                    .raw()
                    .checked_add(BeatTime::ONE.raw())
                    .map(BeatTime::from_raw)
            })
            .ok_or(Invalid("section paint end overflow"))
    }

    pub(super) fn commit(&mut self, command: Command) -> Result<Created, Invalid> {
        self.editor.execute(command)
    }

    pub(super) fn plan_paint(
        &self,
        anchor: BeatTime,
        position: BeatTime,
        point_end: BeatTime,
        name: &str,
        color: ElementColor,
    ) -> Result<Plan, Invalid> {
        let start = anchor.min(position);
        let end = if anchor == position {
            point_end
        } else {
            anchor.max(position)
        };
        let command = Command::AddArrangementSection {
            name: name.into(),
            color,
            start,
            end,
        };
        // This fixture contains no audio resources. A disposable editor validates
        // the exact project command without changing the live fixture or history.
        let mut candidate = Editor::new(self.project().clone(), 1)?;
        candidate.execute(command.clone())?;
        let sections = candidate.project().arrangement_sections().to_vec();
        Ok(Plan {
            display: display(&sections),
            sections,
            command,
        })
    }

    pub(super) fn plan_marker_move(
        &self,
        section: SectionId,
        anchor: BeatTime,
        position: BeatTime,
    ) -> Result<Option<Plan>, Invalid> {
        let project = self.project();
        let marker = project
            .arrangement_section(section)
            .ok_or(Invalid("arrangement section no longer exists"))?;
        let start = i128::from(marker.start().raw()) + i128::from(position.raw())
            - i128::from(anchor.raw());
        let start =
            u64::try_from(start).map_err(|_| Invalid("section marker movement out of range"))?;
        let start = BeatTime::from_raw(start);
        if start == marker.start() {
            return Ok(None);
        }
        let command = Command::MoveArrangementSectionMarker { section, start };
        let preview = project.preview_arrangement_section_edit(&command)?;
        Ok(Some(Plan {
            display: display(preview.sections()),
            sections: preview.sections().to_vec(),
            command,
        }))
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

#[cfg(test)]
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
            .plan(id, RectanglePart::Seam, 0.55, false, false)
            .unwrap()
            .unwrap();
        let expected = plan.display.clone();
        assert_eq!(fixture.project().snapshot(), before);
        fixture.commit(plan.command).unwrap();
        assert_eq!(fixture.display(), expected);
        let committed = fixture.project().snapshot();
        assert!(
            fixture
                .plan(id, RectanglePart::Seam, 0.99, false, false)
                .is_err()
        );
        assert_eq!(fixture.project().snapshot(), committed);
        assert!(fixture.editor.try_undo().unwrap());
        assert_eq!(fixture.project().snapshot(), before);
    }

    #[test]
    fn marker_edge_and_content_edge_use_distinct_project_commands() {
        for marker_only in [false, true] {
            let mut fixture = Fixture::new().unwrap();
            let before = fixture.project().snapshot();
            let id = fixture.project().arrangement_sections()[0].id();
            let plan = fixture
                .plan(id, RectanglePart::Leading, 0.2, false, marker_only)
                .unwrap()
                .unwrap();
            if marker_only {
                assert!(
                    matches!(plan.command, Command::ResizeArrangementSectionMarker { section, edge: TrimEdge::Left, .. } if section == id)
                );
            } else {
                assert!(
                    matches!(plan.command, Command::ResizeArrangementSectionEdge { section, edge: TrimEdge::Left, .. } if section == id)
                );
            }
            assert_eq!(fixture.project().snapshot(), before);
            let expected = plan.sections;
            fixture.commit(plan.command).unwrap();
            assert_eq!(fixture.project().arrangement_sections(), expected);
        }
    }

    #[test]
    fn edge_grid_uses_production_resolver_and_control_inversion() {
        let fixture = Fixture::new().unwrap();
        let project = fixture.project();
        let id = project.arrangement_sections()[0].id();
        let span = BeatTime::from_raw(view_span(project.arrangement_sections()));
        let frames = project.tempo_map().beat_to_frame(span).unwrap();
        let viewport = crate::viewport::Viewport::fit(frames);
        let raw_frame = (0.4375_f64 * frames as f64).round() as u64;
        let raw = project.tempo_map().frame_to_beat(raw_frame).unwrap();
        for width in [160.0, 640.0, 1200.0] {
            let expected = crate::musical_time::SnapGrid::Adaptive
                .snap_beat(project, raw, viewport, width)
                .unwrap();
            let beat = fixture
                .snapped_beat(0.4375 * width, 0.0, width, false)
                .unwrap();
            assert_eq!(beat, expected);
            assert_eq!(
                fixture
                    .snapped_beat(0.4375 * width, 0.0, width, true)
                    .unwrap(),
                raw
            );
            let plan = fixture
                .plan_at_beat(id, RectanglePart::Trailing, beat, false, true)
                .unwrap()
                .unwrap();
            assert!(
                matches!(plan.command, Command::ResizeArrangementSectionMarker { boundary, .. } if boundary == expected)
            );
        }
        assert!(fixture.snapped_beat(f32::NAN, 0.0, 640.0, false).is_err());
        assert!(fixture.snapped_beat(0.5, 0.0, 0.0, false).is_err());
    }

    #[test]
    fn marker_move_uses_pointer_offset_and_rejects_underflow() {
        let mut fixture = Fixture::new().unwrap();
        let before = fixture.project().snapshot();
        let id = fixture.project().arrangement_sections()[0].id();
        let anchor = BeatTime::from_quarters(5).unwrap();
        assert!(
            fixture
                .plan_marker_move(id, anchor, BeatTime::ZERO)
                .is_err()
        );
        assert_eq!(fixture.project().snapshot(), before);
        let position = BeatTime::from_ratio(9, 2).unwrap();
        let plan = fixture
            .plan_marker_move(id, anchor, position)
            .unwrap()
            .unwrap();
        assert!(
            matches!(plan.command, Command::MoveArrangementSectionMarker { section, .. } if section == id)
        );
        let expected = plan.sections;
        fixture.commit(plan.command).unwrap();
        assert_eq!(fixture.project().arrangement_sections(), expected);
        assert!(fixture.project().arrangement_sections()[0].start() < BeatTime::ONE);
    }

    #[test]
    fn paint_preview_validates_without_mutating_and_matches_commit() {
        let fixture = Fixture::new().unwrap();
        let before = fixture.project().snapshot();
        assert!(
            fixture
                .plan_paint(
                    BeatTime::from_quarters(2).unwrap(),
                    BeatTime::from_quarters(8).unwrap(),
                    BeatTime::from_quarters(3).unwrap(),
                    "new section",
                    ElementColor::Cyan
                )
                .is_err()
        );
        assert_eq!(fixture.project().snapshot(), before);
        for (anchor, position) in [(0.98, 0.92), (0.92, 0.92)] {
            let mut fixture = Fixture::new().unwrap();
            let sections_before = fixture.project().arrangement_sections().to_vec();
            let span = view_span(fixture.project().arrangement_sections());
            let anchor = position_to_beat(anchor, span).unwrap();
            let position = position_to_beat(position, span).unwrap();
            let point_end = fixture.paint_point_end(anchor, 640.0, false).unwrap();
            let plan = fixture
                .plan_paint(
                    anchor,
                    position,
                    point_end,
                    "new section",
                    ElementColor::Cyan,
                )
                .unwrap();
            let expected = plan.sections;
            assert_eq!(fixture.project().snapshot(), before);
            fixture.commit(plan.command).unwrap();
            assert_eq!(fixture.project().arrangement_sections(), expected);
            assert!(fixture.editor.try_undo().unwrap());
            assert_eq!(fixture.project().arrangement_sections(), sections_before);
        }
    }

    #[test]
    fn point_paint_uses_the_shared_next_grid_landmark() {
        let fixture = Fixture::new().unwrap();
        let project = fixture.project();
        let viewport = fixture.grid_viewport().unwrap();
        for width in [160.0, 640.0, 1200.0] {
            for free in [false, true] {
                let anchor = fixture
                    .snapped_beat(0.92 * width, 0.0, width, free)
                    .unwrap();
                let expected = crate::musical_time::SnapGrid::Adaptive
                    .with_platform_modifier(free)
                    .adjacent_beat(project, anchor, viewport, width, true)
                    .unwrap_or_else(|| BeatTime::from_raw(anchor.raw() + BeatTime::ONE.raw()));
                let point_end = fixture.paint_point_end(anchor, width, free).unwrap();
                assert_eq!(point_end, expected);
                let plan = fixture
                    .plan_paint(anchor, anchor, point_end, "new section", ElementColor::Cyan)
                    .unwrap();
                assert!(
                    matches!(plan.command, Command::AddArrangementSection { start, end, .. } if start == anchor && end == expected)
                );
            }
        }
        assert!(
            fixture
                .paint_point_end(BeatTime::ZERO, f32::NAN, false)
                .is_err()
        );
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
