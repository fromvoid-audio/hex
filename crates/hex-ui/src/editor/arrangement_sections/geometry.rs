//! Section lane geometry, hit-testing, loop bounds and preview-span projection.
use super::*;

impl EditorView {
    /// Read the active Main-section selection through the current tempo map.
    /// The outer interval includes gaps. Unknown IDs or an empty frame projection
    /// refuse, so L cannot silently fall back to stale clip/time selection.
    pub(in crate::editor) fn section_loop_bounds(
        &self,
    ) -> Result<Option<(u64, u64)>, &'static str> {
        if self.mode != EditorMode::Arrangement
            || self.selected_clip.is_some()
            || self.selected_track.is_some()
            || self.focused_return.is_some()
            || self.device_panel.main_selected != Some(self.project_generation)
        {
            return Ok(None);
        }
        let Some(primary) = self.sections.selected else {
            return Ok(None);
        };
        let state = self
            .state
            .as_ref()
            .ok_or("section selection has no current project")?;
        let ids = if self.sections.selection.contains(&primary) {
            self.sections.selection.as_slice()
        } else {
            std::slice::from_ref(&primary)
        };
        let mut start = BeatTime::from_raw(u64::MAX);
        let mut end = BeatTime::ZERO;
        for id in ids {
            let section = state
                .project
                .arrangement_section(*id)
                .ok_or("selected section no longer belongs to the arrangement")?;
            start = start.min(section.start());
            end = end.max(section.end());
        }
        let map = state.project.tempo_map();
        let start = map
            .beat_to_frame(start)
            .map_err(|_| "selected section exceeds the timeline")?;
        let end = map
            .beat_to_frame(end)
            .map_err(|_| "selected section exceeds the timeline")?;
        if start >= end {
            return Err("selected sections have an empty scope");
        }
        Ok(Some((start, end)))
    }

    pub(super) fn section_layout(&self) -> Option<[f32; 4]> {
        (self.main_track_lane_epoch == self.drag_layout_generation)
            .then(|| self.section_geometry())
            .flatten()
    }
    // Render advances the layout epoch before Main's paint measurement. Frozen
    // display uses the cached geometry; input separately requires a fresh epoch.
    pub(super) fn section_geometry(&self) -> Option<[f32; 4]> {
        let lane = self.main_track_lane_bounds?;
        Some([lane.left, lane.top, lane.width, lane.height])
    }
    pub(in crate::editor) fn section_pointer(
        &self,
        x: f32,
        y: f32,
        free: bool,
    ) -> Option<BeatTime> {
        let [left, top, width, height] = self.section_layout()?;
        if x < left - crate::editor::arrangement::LEADING_BUFFER_WIDTH
            || x > left + width
            || y < top
            || y > top + height
        {
            return None;
        }
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        let frame = crate::editor::seek::pointer_to_frame(x, left, width, viewport)?;
        self.snap_grid.with_platform_modifier(free).snap_beat(
            &state.project,
            state.project.tempo_map().frame_to_beat(frame).ok()?,
            viewport,
            width,
        )
    }
    pub(super) fn section_hit(&self, x: f32, y: f32) -> Option<SectionId> {
        let [left, top, width, height] = self.section_layout()?;
        if x < left - crate::editor::arrangement::LEADING_BUFFER_WIDTH
            || x > left + width
            || y < top
            || y > top + height
        {
            return None;
        }
        let state = self.state.as_ref()?;
        let frame = crate::editor::seek::pointer_to_frame(
            x,
            left,
            width,
            self.visible_viewport(state.timeline_frames),
        )?;
        let beat = state.project.tempo_map().frame_to_beat(frame).ok()?;
        state
            .project
            .arrangement_sections()
            .iter()
            .find(|s| s.start() <= beat && beat < s.end())
            .map(|s| s.id())
    }
    /// Sections from `anchor` through `clicked` in song order, inclusive.
    pub(super) fn section_range(&self, anchor: SectionId, clicked: SectionId) -> Vec<SectionId> {
        let Some(project) = self.state.as_ref().map(|state| &state.project) else {
            return vec![clicked];
        };
        crate::section_selection::range(
            project
                .arrangement_sections()
                .iter()
                .map(|section| (section.id(), section.start())),
            anchor,
            clicked,
        )
    }
    pub(super) fn section_edge_hit(&self, x: f32) -> Option<SectionEdge> {
        self.section_edge_hit_in(x, self.section_layout()?)
    }
    pub(super) fn section_edge_hit_in(&self, x: f32, geometry: [f32; 4]) -> Option<SectionEdge> {
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        let [left, _, width, _] = geometry;
        let position = |beat| {
            state
                .project
                .tempo_map()
                .beat_to_frame(beat)
                .ok()
                .map(|frame| {
                    left + (frame as f64 - viewport.start as f64) as f32 / viewport.length as f32
                        * width
                })
        };
        let sections = state
            .project
            .arrangement_sections()
            .iter()
            .map(|section| (section.id(), section.start(), section.end()));
        section_hit::hit(sections, x, position).map(|hit| match hit {
            section_hit::Hit::Leading(id) => SectionEdge::Single(id, hex_project::TrimEdge::Left),
            section_hit::Hit::Trailing(id) => SectionEdge::Single(id, hex_project::TrimEdge::Right),
            section_hit::Hit::Seam(left, right) => SectionEdge::Shared(left, right),
        })
    }

    pub(in crate::editor) fn pointer_hits_section_launch(&self, x: f32, y: f32) -> bool {
        let Some([left, top, width, _]) = self.section_layout() else {
            return false;
        };
        let Some(state) = &self.state else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        state.project.arrangement_sections().iter().any(|section| {
            let tempo = state.project.tempo_map();
            let (Ok(start), Ok(end)) = (
                tempo.beat_to_frame(section.start()),
                tempo.beat_to_frame(section.end()),
            ) else {
                return false;
            };
            let Some((l, r)) = crate::editor::arrangement_map::project_interval(
                crate::editor::arrangement_map::Interval { start, end },
                viewport,
                width,
            ) else {
                return false;
            };
            crate::editor::section_launch::action_bounds(left + l, top, r - l)
                .is_some_and(|bounds| bounds.contains(&gpui::point(px(x), px(y))))
        })
    }
}

pub(super) fn copied_section_source(command: &Command) -> Option<SectionId> {
    match command {
        Command::DuplicateArrangementSectionAt { section, .. }
        | Command::AppendArrangementSectionCopy { section, .. } => Some(*section),
        Command::ReplaceArrangementSection {
            source, copy: true, ..
        } => Some(*source),
        _ => None,
    }
}

/// Resolve the project frame span `(section_range, preview_viewport)` whose
/// clips belong inside `marker` while resting, pending or dragging.
pub(super) fn section_preview_span(
    state: &ViewState,
    display: &Display,
    gesture: Option<&Gesture>,
    marker: &Marker,
    new_copy: bool,
    full_span: (u64, u64),
    visible_span: (u64, u64),
) -> ((u64, u64), Viewport) {
    let default = (
        full_span,
        Viewport {
            start: visible_span.0,
            length: visible_span.1.saturating_sub(visible_span.0),
        },
    );
    let Some(g) = gesture else {
        return default;
    };
    let Some(command) = g
        .command
        .as_ref()
        .filter(|cmd| cmd.requires_section_publication())
    else {
        return default;
    };
    match g.mode {
        Mode::Reorder(_) => {
            let source_id = marker
                .id
                .or_else(|| new_copy.then(|| copied_section_source(command)).flatten());
            let Some(source) = source_id.and_then(|id| state.project.arrangement_section(id))
            else {
                return (
                    (0, 0),
                    Viewport {
                        start: 0,
                        length: 0,
                    },
                );
            };
            let (Ok(src_start), Ok(src_end)) = (
                state.project.tempo_map().beat_to_frame(source.start()),
                state.project.tempo_map().beat_to_frame(source.end()),
            ) else {
                return (
                    (0, 0),
                    Viewport {
                        start: 0,
                        length: 0,
                    },
                );
            };
            let full_len = full_span.1.saturating_sub(full_span.0);
            let src_len = src_end.saturating_sub(src_start);
            let map_offset = |offset: u64| -> u64 {
                if full_len == 0 || src_len == full_len {
                    offset.min(src_len)
                } else {
                    ((u128::from(offset) * u128::from(src_len)) / u128::from(full_len))
                        .min(u128::from(src_len)) as u64
                }
            };
            let mapped_start =
                src_start.saturating_add(map_offset(visible_span.0.saturating_sub(full_span.0)));
            let mapped_end =
                src_start.saturating_add(map_offset(visible_span.1.saturating_sub(full_span.0)));
            (
                (src_start, src_end),
                Viewport {
                    start: mapped_start,
                    length: mapped_end.saturating_sub(mapped_start),
                },
            )
        }
        Mode::ContentStart(_)
        | Mode::ContentEnd(_)
        | Mode::Shared {
            marker_only: false, ..
        } => {
            let Some(source) = marker
                .id
                .and_then(|id| state.project.arrangement_section(id))
            else {
                return (
                    (0, 0),
                    Viewport {
                        start: 0,
                        length: 0,
                    },
                );
            };
            let keep_start = source.start().max(marker.start);
            let keep_end = source.end().min(marker.end);
            if keep_start >= keep_end {
                return (
                    (0, 0),
                    Viewport {
                        start: 0,
                        length: 0,
                    },
                );
            }
            let (Ok(src_start), Ok(src_end), Ok(display_keep_start)) = (
                state.project.tempo_map().beat_to_frame(keep_start),
                state.project.tempo_map().beat_to_frame(keep_end),
                display.tempo.beat_to_frame(keep_start),
            ) else {
                return default;
            };
            let shift = i128::from(display_keep_start) - i128::from(src_start);
            let mapped_start =
                u64::try_from(i128::from(visible_span.0) - shift).unwrap_or(visible_span.0);
            (
                (src_start, src_end),
                Viewport {
                    start: mapped_start,
                    length: visible_span.1.saturating_sub(visible_span.0),
                },
            )
        }
        Mode::Paint
        | Mode::Start(_)
        | Mode::End(_)
        | Mode::Move(_)
        | Mode::Shared {
            marker_only: true, ..
        } => default,
    }
}

pub(super) fn same_command(a: &Command, b: &Command) -> bool {
    match (a, b) {
        (
            Command::ReplaceArrangementSection {
                source: a,
                target: at,
                copy: ac,
            },
            Command::ReplaceArrangementSection {
                source: b,
                target: bt,
                copy: bc,
            },
        ) => (a, at, ac) == (b, bt, bc),
        (
            Command::AppendArrangementSectionCopy {
                section: a,
                start: at,
                gap_name: an,
                gap_color: ac,
            },
            Command::AppendArrangementSectionCopy {
                section: b,
                start: bt,
                gap_name: bn,
                gap_color: bc,
            },
        )
        | (
            Command::MoveArrangementSectionBeyondEnd {
                section: a,
                start: at,
                gap_name: an,
                gap_color: ac,
            },
            Command::MoveArrangementSectionBeyondEnd {
                section: b,
                start: bt,
                gap_name: bn,
                gap_color: bc,
            },
        ) => (a, at, an, ac) == (b, bt, bn, bc),
        (
            Command::PasteArrangementSection {
                clipboard: a,
                at: aa,
                gap_name: an,
                gap_color: ac,
            },
            Command::PasteArrangementSection {
                clipboard: b,
                at: ba,
                gap_name: bn,
                gap_color: bc,
            },
        ) => Arc::ptr_eq(a, b) && (aa, an, ac) == (ba, bn, bc),
        (
            Command::DuplicateArrangementSectionAt {
                section: a,
                target: at,
                after: aa,
            },
            Command::DuplicateArrangementSectionAt {
                section: b,
                target: bt,
                after: ba,
            },
        ) => (a, at, aa) == (b, bt, ba),
        (
            Command::AddArrangementSection {
                name: a,
                color: ac,
                start: as_,
                end: ae,
            },
            Command::AddArrangementSection {
                name: b,
                color: bc,
                start: bs,
                end: be,
            },
        ) => (a, ac, as_, ae) == (b, bc, bs, be),
        (
            Command::SetArrangementSectionRange {
                section: a,
                start: as_,
                end: ae,
            },
            Command::SetArrangementSectionRange {
                section: b,
                start: bs,
                end: be,
            },
        ) => (a, as_, ae) == (b, bs, be),
        (
            Command::ResizeArrangementSectionMarker {
                section: a,
                edge: ae,
                boundary: ab,
            },
            Command::ResizeArrangementSectionMarker {
                section: b,
                edge: be,
                boundary: bb,
            },
        ) => (a, ae, ab) == (b, be, bb),
        (
            Command::MoveArrangementSectionMarker {
                section: a,
                start: as_,
            },
            Command::MoveArrangementSectionMarker {
                section: b,
                start: bs,
            },
        ) => (a, as_) == (b, bs),
        (
            Command::ReorderSectionBefore {
                section: a,
                before: at,
            },
            Command::ReorderSectionBefore {
                section: b,
                before: bt,
            },
        ) => (a, at) == (b, bt),
        (
            Command::RenameArrangementSection {
                section: a,
                name: an,
            },
            Command::RenameArrangementSection {
                section: b,
                name: bn,
            },
        ) => (a, an) == (b, bn),
        (
            Command::SetArrangementSectionColor {
                section: a,
                color: ac,
            },
            Command::SetArrangementSectionColor {
                section: b,
                color: bc,
            },
        ) => (a, ac) == (b, bc),
        (
            Command::ReorderSectionAfter {
                section: a,
                after: at,
            },
            Command::ReorderSectionAfter {
                section: b,
                after: bt,
            },
        ) => (a, at) == (b, bt),
        (
            Command::ResizeArrangementSection {
                section: a,
                edge: ae,
                length: al,
            },
            Command::ResizeArrangementSection {
                section: b,
                edge: be,
                length: bl,
            },
        ) => (a, ae, al) == (b, be, bl),
        (
            Command::ResizeArrangementSectionBoundary {
                left: a,
                right: b,
                boundary: c,
                marker_only: d,
            },
            Command::ResizeArrangementSectionBoundary {
                left: x,
                right: y,
                boundary: z,
                marker_only: w,
            },
        ) => (a, b, c, d) == (x, y, z, w),
        (
            Command::ResizeArrangementSectionEdge {
                section: a,
                edge: b,
                boundary: c,
            },
            Command::ResizeArrangementSectionEdge {
                section: x,
                edge: y,
                boundary: z,
            },
        ) => (a, b, c) == (x, y, z),
        (Command::DeleteArrangementSection(a), Command::DeleteArrangementSection(b))
        | (Command::DuplicateArrangementSection(a), Command::DuplicateArrangementSection(b)) => {
            a == b
        }
        (
            Command::SplitArrangementSection { section: a, at: aa },
            Command::SplitArrangementSection { section: b, at: ba },
        ) => (a, aa) == (b, ba),
        (Command::MergeArrangementSections(a), Command::MergeArrangementSections(b)) => a == b,
        (Command::RemoveArrangementSection(a), Command::RemoveArrangementSection(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_section_ids() -> (SectionId, SectionId) {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::ArrangementSection(a) = editor
            .execute(Command::AddArrangementSection {
                name: "a".into(),
                color: ElementColor::Cyan,
                start: BeatTime::from_quarters(0).unwrap(),
                end: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("section a");
        };
        let hex_project::Created::ArrangementSection(b) = editor
            .execute(Command::AddArrangementSection {
                name: "b".into(),
                color: ElementColor::Green,
                start: BeatTime::from_quarters(4).unwrap(),
                end: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        else {
            panic!("section b");
        };
        (a, b)
    }

    #[test]
    fn copied_section_source_extracts_only_copy_commands() {
        let (id, _) = two_section_ids();
        assert_eq!(
            copied_section_source(&Command::AppendArrangementSectionCopy {
                section: id,
                start: BeatTime::from_quarters(8).unwrap(),
                gap_name: "gap".into(),
                gap_color: ElementColor::Cyan,
            }),
            Some(id)
        );
        assert_eq!(
            copied_section_source(&Command::DeleteArrangementSection(id)),
            None
        );
    }

    #[test]
    fn same_command_distinguishes_section_edit_variants() {
        let (first, second) = two_section_ids();
        let a = Command::DeleteArrangementSection(first);
        let b = Command::DeleteArrangementSection(first);
        let c = Command::DeleteArrangementSection(second);
        assert!(same_command(&a, &b));
        assert!(!same_command(&a, &c));
    }
}
