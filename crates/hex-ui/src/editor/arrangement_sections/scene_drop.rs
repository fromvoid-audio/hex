//! Drag-and-drop placement of a Console scene onto the Arrangement Main track (#3444).
//!
//! Only `main-track-lane` accepts drops (explicitly never the Main mix bus header,
//! return tracks, or ordinary audio/MIDI tracks). Release transfers the frozen
//! candidate marker into `sections.pending` with a `pending_edit::Receipt` so
//! repaints hold the exact final preview until canonical acknowledgement or refusal.
use super::*;
use hex_project::SceneId;

/// Stable drag payload captured at pointer drag start on a Console scene row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::editor) struct ConsoleSceneDrag {
    pub(in crate::editor) project_generation: u64,
    pub(in crate::editor) revision: u64,
    pub(in crate::editor) scene: SceneId,
    pub(in crate::editor) name: String,
    pub(in crate::editor) duration: BeatTime,
    pub(in crate::editor) color: ElementColor,
}

/// Active live preview state while dragging a Console scene toward the Main track.
#[derive(Clone)]
pub(super) struct ActiveSceneDrag {
    pub(super) payload: ConsoleSceneDrag,
    pub(super) preview_span: Option<(BeatTime, BeatTime)>,
}

/// Minimal floating ghost painted while dragging a Console scene.
pub(in crate::editor) struct ConsoleSceneDragGhost {
    pub(in crate::editor) name: String,
    pub(in crate::editor) color: ElementColor,
}

impl gpui::Render for ConsoleSceneDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::element_colors(self.color);
        div()
            .px_2()
            .py_1()
            .rounded(px(3.0))
            .bg(rgb(palette.accent))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .text_xs()
            .text_color(rgb(theme::readable_text(palette.accent, 0x000000)))
            .child(theme::text(hex_session::service::text::verbatim(
                &self.name,
            )))
    }
}

impl EditorView {
    pub(in crate::editor) fn begin_console_scene_drag(&mut self, payload: ConsoleSceneDrag) {
        self.sections.scene_drag = Some(ActiveSceneDrag {
            payload,
            preview_span: None,
        });
    }

    pub(in crate::editor) fn clear_console_scene_drag_preview(&mut self) -> bool {
        if let Some(drag) = self.sections.scene_drag.as_mut()
            && drag.preview_span.is_some()
        {
            drag.preview_span = None;
            return true;
        }
        false
    }

    pub(in crate::editor) fn clear_console_scene_drag(&mut self, generation: u64, scene: SceneId) {
        if self.sections.scene_drag.as_ref().is_some_and(|active| {
            active.payload.project_generation == generation && active.payload.scene == scene
        }) {
            self.sections.scene_drag = None;
        }
    }

    fn resolve_console_scene_drop_span(
        &self,
        drag: &ConsoleSceneDrag,
        x: f32,
        y: f32,
        modifiers: gpui::Modifiers,
    ) -> Result<(BeatTime, BeatTime), &'static str> {
        if drag.project_generation != self.project_generation {
            return Err("project changed while placing scene");
        }
        let Some(state) = self.state.as_ref() else {
            return Err("project is not ready");
        };
        if state.revision != drag.revision {
            return Err("project changed while placing scene");
        }
        if !state
            .project
            .scenes()
            .iter()
            .any(|scene| scene.id() == drag.scene)
        {
            return Err("console scene no longer exists");
        }
        if !self.sections_available() {
            return Err(SECTION_BUSY);
        }
        let free = section_input::EditModifiers::from(modifiers).invert_snap;
        let Some(start) = self.section_pointer(x, y, free) else {
            return Err("drop onto the arrangement main track to place scene");
        };
        let end = BeatTime::from_raw(
            start
                .raw()
                .checked_add(drag.duration.raw())
                .ok_or("scene placement exceeds timeline range")?,
        );
        if start >= end {
            return Err("scene duration must be positive");
        }
        state
            .project
            .tempo_map()
            .beat_to_frame(start)
            .map_err(|_| "scene start exceeds timeline range")?;
        state
            .project
            .tempo_map()
            .beat_to_frame(end)
            .map_err(|_| "scene end exceeds timeline range")?;
        if state.project.arrangement_sections().len() >= hex_project::MAX_ARRANGEMENT_SECTIONS {
            return Err("arrangement section limit reached");
        }
        let overlaps = state
            .project
            .arrangement_sections()
            .iter()
            .any(|existing| start < existing.end() && end > existing.start());
        if overlaps {
            return Err("scene placement overlaps an existing arrangement scene");
        }
        Ok((start, end))
    }

    fn build_scene_drop_display(
        &self,
        drag: &ConsoleSceneDrag,
        start: BeatTime,
        end: BeatTime,
    ) -> Option<Display> {
        let state = self.state.as_ref()?;
        let mut markers: Vec<Marker> = state
            .project
            .arrangement_sections()
            .iter()
            .map(Marker::from)
            .collect();
        markers.push(Marker {
            id: None,
            name: drag.name.clone(),
            color: drag.color,
            start,
            end,
        });
        markers.sort_by_key(|m| m.start);
        Some(Display {
            markers,
            tempo: state.project.tempo_map().clone(),
            pointer_hover: None,
            held: [None; 2],
            marker_only: false,
            drop_feedback: None,
        })
    }

    pub(super) fn console_scene_drag_preview_display(&self) -> Option<Display> {
        let active = self.sections.scene_drag.as_ref()?;
        let (start, end) = active.preview_span?;
        let state = self.state.as_ref()?;
        if active.payload.project_generation != self.project_generation
            || state.revision != active.payload.revision
        {
            return None;
        }
        self.build_scene_drop_display(&active.payload, start, end)
    }

    pub(in crate::editor) fn hover_console_scene_on_main_track(
        &mut self,
        drag: &ConsoleSceneDrag,
        x: f32,
        y: f32,
        modifiers: gpui::Modifiers,
    ) -> bool {
        let span = self
            .resolve_console_scene_drop_span(drag, x, y, modifiers)
            .ok();
        match self.sections.scene_drag.as_mut() {
            Some(active)
                if active.payload.project_generation == drag.project_generation
                    && active.payload.scene == drag.scene =>
            {
                if active.preview_span != span {
                    active.preview_span = span;
                    return true;
                }
                false
            }
            _ => {
                self.sections.scene_drag = Some(ActiveSceneDrag {
                    payload: drag.clone(),
                    preview_span: span,
                });
                true
            }
        }
    }

    pub(in crate::editor) fn drop_console_scene_on_main_track(
        &mut self,
        drag: &ConsoleSceneDrag,
        x: f32,
        y: f32,
        modifiers: gpui::Modifiers,
    ) -> bool {
        self.sections.scene_drag = None;
        let (start, end) = match self.resolve_console_scene_drop_span(drag, x, y, modifiers) {
            Ok(span) => span,
            Err(reason) => {
                self.error = Some(reason.into());
                return false;
            }
        };
        let Some(display) = self.build_scene_drop_display(drag, start, end) else {
            return false;
        };
        self.error = None;
        let command = Command::PlaceLinkedScene {
            scene: drag.scene,
            start,
            end,
            color: drag.color,
        };
        self.submit_section(command, display, drag.project_generation, drag.revision);
        true
    }
}
