//! Section lane element rendering (headers, chamfered fills, drop indicators and rename inputs).
use super::*;

impl EditorView {
    pub(in crate::editor) fn section_elements(
        &self,
        viewport: Viewport,
        scale: f32,
        window: &Window,
        cx: &mut Context<Self>,
        coverage: Option<&crate::section_surface::SectionCoverage>,
    ) -> Vec<AnyElement> {
        let Some(state) = self.state.as_ref() else {
            return Vec::new();
        };
        let coverage = coverage.filter(|_| {
            self.sections.ahead.is_none()
                && self.sections.pending.is_none()
                && self.sections.queued.is_none()
                && self.sections.gesture.is_none()
                && self.sections.name.is_none()
        });
        let canonical;
        let (display, preview_state, drag_gesture) = if let Some(g) = self
            .sections
            .gesture
            .as_ref()
            .filter(|g| self.section_current(g))
        {
            (&g.display, None, Some(g))
        } else if let Some(queued_display) = self
            .sections
            .queued
            .as_ref()
            .filter(|q| q.generation == self.project_generation)
            .and_then(|q| q.display.as_ref())
        {
            (
                queued_display,
                self.sections
                    .pending
                    .as_ref()
                    .and_then(|p| p.layout.as_deref()),
                None,
            )
        } else if let Some(p) = self
            .sections
            .ahead
            .as_ref()
            .filter(|p| p.layout.is_some())
            .or(self.sections.pending.as_ref())
        {
            (&p.display, p.layout.as_deref(), None)
        } else {
            canonical = Display {
                markers: state
                    .project
                    .arrangement_sections()
                    .iter()
                    .map(Marker::from)
                    .collect(),
                tempo: state.project.tempo_map().clone(),
                pointer_hover: None,
                held: [None; 2],
                marker_only: false,
                drop_feedback: None,
            };
            (&canonical, None, None)
        };
        let map_state = preview_state.unwrap_or(state.as_ref());
        let Some(limit) = viewport.end() else {
            return Vec::new();
        };
        if viewport.length == 0 {
            return Vec::new();
        }
        let hover_marker = (self.sections.gesture.is_none()
            && self.sections.pending.is_none()
            && self.sections.name.is_none()
            && section_input::EditModifiers::from(window.modifiers()).marker_only)
            .then(|| {
                let position = window.mouse_position();
                // Rendering advances the epoch before layout measurement. Hover
                // uses cached paint geometry; admitted input still requires freshness.
                let geometry = self.section_geometry()?;
                let beat = display.pointer_hover_at(
                    viewport,
                    geometry,
                    (position.x.into(), position.y.into()),
                )?;
                match self.section_edge_hit_in(position.x.into(), geometry) {
                    Some(SectionEdge::Single(id, _)) | Some(SectionEdge::Shared(id, _)) => Some(id),
                    None => display
                        .markers
                        .iter()
                        .find(|m| m.start <= beat && beat < m.end)
                        .and_then(|m| m.id),
                }
            })
            .flatten();
        let mut elements = Vec::new();
        let mut selected_edges = Vec::new();
        for (index, m) in display.markers.iter().enumerate() {
            let (Ok(full_start), Ok(full_end)) = (
                display.tempo.beat_to_frame(m.start),
                display.tempo.beat_to_frame(m.end),
            ) else {
                continue;
            };
            let chamfer = if full_end <= limit {
                crate::section_surface::CHAMFER
            } else {
                0.0
            };
            let (start, end) = (full_start.max(viewport.start), full_end.min(limit));
            if end <= start {
                continue;
            }
            let copy_command = self
                .sections
                .gesture
                .as_ref()
                .and_then(|g| g.command.as_ref())
                .or_else(|| {
                    self.sections
                        .pending
                        .as_ref()
                        .filter(|p| {
                            p.selection_intent_epoch
                                .is_some_and(|epoch| Some(epoch) == self.selection_intent_epoch)
                        })
                        .map(|p| &p.command)
                });
            let copy_preview = copy_command.is_some_and(|command| {
                copied_section_source(command).is_some()
                    || matches!(command, Command::PasteArrangementSection { .. })
            });
            let new_copy = m.id.is_none()
                && copy_command.is_some_and(|command| match command {
                    Command::AppendArrangementSectionCopy { start, .. } => m.start == *start,
                    Command::PasteArrangementSection { at, .. } => m.start == *at,
                    Command::DuplicateArrangementSectionAt { .. }
                    | Command::ReplaceArrangementSection { copy: true, .. } => true,
                    _ => false,
                });
            let canonical_selected = m.id.is_some_and(|id| {
                self.sections.selection.contains(&id) || self.sections.selected == Some(id)
            });
            let selected = (copy_preview && new_copy) || (!copy_preview && canonical_selected);
            let focused = self.arrangement_focus.projection.enabled && canonical_selected;
            let owned_hover = crate::section_surface::Hover {
                held: display.held,
                pointer: display.pointer_hover,
            }
            .contains(m.id, m.start, m.end);
            let marker_only = (display.marker_only && owned_hover)
                || m.id.is_some_and(|id| hover_marker == Some(id));
            let colors = theme::section_colors(m.color, selected, owned_hover);
            let hover = theme::section_colors(m.color, selected, true);
            let border = colors.2;
            let pixel_width = self.main_track_lane_bounds.map_or(0.0, |lane| {
                lane.width * ((end - start) as f32 / viewport.length as f32)
            });
            let stroke = 1.0 / scale.max(1.0);
            let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
            let mut element = crate::section_surface::surface(
                (
                    (start - viewport.start) as f32 / viewport.length as f32,
                    (end - start) as f32 / viewport.length as f32,
                ),
                pixel_width, chamfer, m.color, selected, owned_hover,
                coverage.cloned(),
            )
                .id(("arrangement-section", index))
                .debug_selector(move || format!("arrangement-section-{index}"))
                .top_0()
                .bottom_0()
                .left_0()
                .w_full()
                .tooltip(|_, cx| {
                    cx.new(|_| crate::editor::waveform::ClipTooltip { kind: crate::tooltips::Kind::Help,
                        name: "header: select/reorder · click lane: place cursor · body drag: time selection · inner edge: resize one · seam: resize both · option: copy · option-command: markers only · command: snap · shift-click: select range · cmd-shift-click: toggle · g: merge · cmd-x/c/v: cut/copy/paste · cmd-d: duplicate · e: split"
                            .into(),
                    })
                    .into()
                });
            if marker_only {
                element = element.child(
                    crate::section_surface::marker_only_overlay(m.color, chamfer, selected)
                        .debug_selector(move || {
                            format!("arrangement-section-markers-only-{index}")
                        }),
                );
            }
            let mut header = crate::section_surface::header(
                m.color,
                selected,
                owned_hover,
                crate::editor::clip_name::CLIP_NAME_HEIGHT,
            )
            .when(marker_only, |header| {
                header.text_color(rgb(theme::color(theme::PaletteRole::Text)))
            })
            .id(("arrangement-section-header", index))
            .debug_selector(move || format!("arrangement-section-header-{index}"));
            if let Some(id) = m.id {
                element = element.on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                        view.section_menu(id, event.position, window, cx);
                        cx.stop_propagation();
                    }),
                );
            }
            let name_edit =
                m.id.and_then(|id| self.sections.name.as_ref().filter(|name| name.id == id));
            if name_edit.is_none() {
                if let Some(id) = m.id.filter(|_| {
                    crate::editor::section_launch::action_bounds(0.0, 0.0, pixel_width).is_some()
                }) {
                    header = header
                        .pl(px(16.0))
                        .child(self.section_launch_action(id, cx));
                }
                header = header.child(theme::text(if marker_only {
                    format!("◇ {}", m.name)
                } else {
                    m.name.clone()
                }));
            }
            if focused {
                element =
                    element.children([crate::section_surface::focus::overlay(m.color, chamfer)
                        .debug_selector(move || {
                            format!("arrangement-section-focus-hatch-{index}")
                        })]);
            }
            let (section_range, preview_viewport) = section_preview_span(
                map_state,
                display,
                drag_gesture,
                m,
                new_copy,
                (full_start, full_end),
                (start, end),
            );
            element = element
                .child(super::arrangement_map::section(
                    self,
                    map_state,
                    section_range,
                    preview_viewport,
                    chamfer,
                ))
                .child(header);
            if let Some(name) = name_edit {
                // Name editing keeps its existing full-size native field; held
                // pointer/range captures are disabled for its entire lifetime.
                element = element.children([name.input.clone()]);
            }
            for right in [false, true] {
                let part = if right {
                    RectanglePart::Trailing
                } else {
                    RectanglePart::Leading
                };
                // Preserve the existing section cursor overlay within its border.
                element =
                    element.children([hex_widgets::rectangle_edge(part, INNER_EDGE_WIDTH, 0.0)]);
            }
            elements.push(
                crate::editor::clip_boundary::clip_boundary_shell(
                    element,
                    (start - viewport.start) as f32 / viewport.length as f32,
                    (end - start) as f32 / viewport.length as f32,
                    stroke,
                    border,
                    hover.2,
                    true,
                    has_border && (selected || owned_hover),
                    chamfer,
                    move || format!("arrangement-section-trailing-{index}"),
                )
                .into_any_element(),
            );
            if selected {
                selected_edges.push(
                    div()
                        .debug_selector(move || {
                            format!("arrangement-section-selected-trailing-{index}")
                        })
                        .absolute()
                        .top_0()
                        .bottom(px(chamfer))
                        .left(relative(
                            (end - viewport.start) as f32 / viewport.length as f32,
                        ))
                        .w(px(stroke))
                        .bg(rgb(border))
                        .into_any_element(),
                );
            }
        }
        elements.extend(selected_edges);
        for pair in display.markers.windows(2) {
            if pair[0].end != pair[1].start {
                continue;
            }
            let Ok(frame) = display.tempo.beat_to_frame(pair[0].end) else {
                continue;
            };
            if frame < viewport.start || frame > limit {
                continue;
            }
            elements.push(
                hex_widgets::rectangle_edge(RectanglePart::Seam, 2.0 * SHARED_EDGE_RADIUS, 0.0)
                    .debug_selector(|| "section-shared-edge".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(
                        (frame - viewport.start) as f32 / viewport.length as f32,
                    ))
                    .ml(px(-SHARED_EDGE_RADIUS))
                    .into_any_element(),
            );
        }
        if self.sections.gesture.is_none()
            && self.sections.pending.is_none()
            && self.sections.name.is_none()
            && self.normalized_time_selection().is_none()
            && let Some(cursor) = self
                .sections
                .cursor
                .filter(|c| c.generation == self.project_generation && c.revision == state.revision)
            && let Ok(frame) = state.project.tempo_map().beat_to_frame(cursor.beat)
            && let Some(fraction) = viewport.fraction(frame)
        {
            elements.push(
                div()
                    .debug_selector(|| "section-edit-cursor".into())
                    .absolute()
                    .top(px(1.0))
                    .bottom(px(1.0))
                    .left(relative(fraction))
                    .w(px(1.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Text)))
                    .into_any_element(),
            );
        }
        if let Some(drop) = &display.drop_feedback {
            let fraction = |beat| {
                drop.tempo.beat_to_frame(beat).ok().map(|frame| {
                    (frame as f64 - viewport.start as f64) as f32 / viewport.length as f32
                })
            };
            match drop.feedback {
                section_reorder::DropFeedback::Insert(at) => {
                    if let Some(left) = fraction(at) {
                        elements.push(
                            crate::section_surface::insertion_indicator(left)
                                .debug_selector(|| "section-drop-insertion".into())
                                .into_any_element(),
                        );
                    }
                }
                section_reorder::DropFeedback::Replace { start, end, .. } => {
                    if let (Some(left), Some(right)) = (fraction(start), fraction(end)) {
                        elements.push(
                            crate::section_surface::replacement_indicator(left, right - left)
                                .debug_selector(|| "section-drop-replace".into())
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        elements
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_from_marker_preserves_canonical_section_bounds() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::ArrangementSection(id) = editor
            .execute(Command::AddArrangementSection {
                name: "verse".into(),
                color: ElementColor::Cyan,
                start: BeatTime::from_quarters(0).unwrap(),
                end: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        else {
            panic!("expected section");
        };
        let section = editor.project().arrangement_sections().first().unwrap();
        let marker = Marker::from(section);
        assert_eq!(marker.id, Some(id));
        assert_eq!(marker.name, "verse");
        assert_eq!(marker.start, BeatTime::from_quarters(0).unwrap());
        assert_eq!(marker.end, BeatTime::from_quarters(8).unwrap());
    }
}
