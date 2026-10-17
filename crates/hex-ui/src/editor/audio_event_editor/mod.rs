//! Clip-local waveform editing. Only selection, viewport and previews live here.
#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
mod view;
use super::{EditorView, musical_time::SnapGrid, viewport::Viewport};
use gpui::{Bounds, Pixels};
use hex_project::{AudioClipEdit, AudioEvent, AudioEventEdit, ClipId};
pub(super) use view::view;

#[derive(Default)]
pub(super) struct State {
    owner: Option<(ClipId, u64)>,
    revision: u64,
    pub(super) source_view: bool,
    viewport: Option<Viewport>,
    cursor: u64,
    selected: Option<u64>,
    range: Option<(u64, u64)>,
    grid: SnapGrid,
    bounds: Option<Bounds<Pixels>>,
    pub(super) gesture: Option<Gesture>,
    clipboard: Option<Clipboard>,
}
struct Clipboard {
    owner: (ClipId, u64),
    root: AudioClipEdit,
    length: u64,
    events: Vec<AudioEvent>,
}
#[derive(Clone)]
pub(super) struct Gesture {
    anchor: u64,
    pointer_x: f32,
    origin_x: f32,
    moved: bool,
    kind: Kind,
    operation: Option<AudioEventEdit>,
}
#[derive(Clone, Copy)]
enum Kind {
    Select,
    RangeMove { start: u64, end: u64 },
    Move(AudioEvent),
    Leading(AudioEvent),
    Trailing(AudioEvent),
}

impl EditorView {
    fn event_context(&self) -> Option<(ClipId, AudioClipEdit, u64)> {
        let state = self.state.as_ref()?;
        let clip = self.selected_clip?;
        let source = state.project.clip(clip)?;
        Some((clip, source.audio_event_root(), state.revision))
    }
    fn event_context_current(&self) -> bool {
        self.event_context().is_some_and(|(clip, _, revision)| {
            self.audio_event_editor.owner == Some((clip, self.project_generation))
                && self.audio_event_editor.revision == revision
        })
    }
    fn event_commit(&mut self, operation: AudioEventEdit) {
        if self.edit_pending() || !self.event_context_current() {
            return;
        }
        let Some((clip, _, _)) = self.event_context() else {
            return;
        };
        self.audio_event_editor.gesture = None;
        self.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::EditAudioEvents { clip, operation },
        ));
    }
    fn event_range(&self) -> Option<(u64, u64)> {
        if let Some(range) = self.audio_event_editor.range.filter(|(a, b)| a < b) {
            return Some(range);
        }
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.selected_clip?)?;
        let selected = self.audio_event_editor.selected?;
        clip.editable_audio_events()
            .ok()?
            .events
            .iter()
            .find(|e| e.id == selected)
            .map(|e| (e.start, e.end))
    }
    fn event_copy(&mut self, cut: bool) {
        let Some((clip, root, _)) = self.event_context() else {
            return;
        };
        let Some((start, end)) = self.event_range() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = state.project.clip(clip) else {
            return;
        };
        self.audio_event_editor.clipboard = Some(Clipboard {
            owner: (clip, self.project_generation),
            root,
            length: end - start,
            events: match source.editable_audio_events() {
                Ok(events) => events.copy_range(start, end),
                Err(error) => {
                    self.error = Some(error.0.into());
                    return;
                }
            },
        });
        self.audio_event_editor.cursor = start;
        if cut {
            self.event_commit(AudioEventEdit::Delete { start, end });
        }
    }
    fn event_paste(&mut self, insert: bool) {
        let Some((clip, mut root, _)) = self.event_context() else {
            return;
        };
        let Some(clipboard) = self.audio_event_editor.clipboard.as_ref() else {
            return;
        };
        let mut saved = clipboard.root.clone();
        std::sync::Arc::make_mut(&mut root.domain).events = None;
        std::sync::Arc::make_mut(&mut saved.domain).events = None;
        if clipboard.owner != (clip, self.project_generation) || root.domain != saved.domain {
            self.error = Some(
                "audio event clipboard belongs to a different or changed clip; copy the events again".into(),
            );
            return;
        }
        let operation = if insert {
            AudioEventEdit::Insert {
                at: self.audio_event_editor.cursor,
                length: clipboard.length,
                events: clipboard.events.clone(),
            }
        } else {
            AudioEventEdit::Paste {
                at: self.audio_event_editor.cursor,
                length: clipboard.length,
                events: clipboard.events.clone(),
            }
        };
        self.event_commit(operation);
    }
    fn event_delete(&mut self) {
        if let Some((start, end)) = self.event_range() {
            self.event_commit(AudioEventEdit::Delete { start, end });
            self.audio_event_editor.range = None;
            self.audio_event_editor.selected = None;
        }
    }
    fn event_split(&mut self) {
        let Some((_, root, _)) = self.event_context() else {
            return;
        };
        if let Some((start, end)) = self.audio_event_editor.range.filter(|(a, b)| a < b) {
            self.event_commit(AudioEventEdit::SplitRange { start, end });
            return;
        }
        let at = self.audio_event_editor.cursor;
        if at > root.view_start && at < root.view_end {
            self.event_commit(AudioEventEdit::Split { at });
        }
    }
    fn event_viewport(&self, root: &AudioClipEdit) -> Viewport {
        let Some(state) = self.state.as_ref() else {
            return Viewport::fit(0);
        };
        let first = root
            .root_frame_at(root.view_start, state.project.tempo_map())
            .unwrap_or(0);
        let end = root
            .root_frame_at(root.view_end, state.project.tempo_map())
            .unwrap_or(first);
        self.audio_event_editor
            .viewport
            .unwrap_or(Viewport {
                start: 0,
                length: end - first,
            })
            .clamp(end - first)
    }
    fn event_project_viewport(&self, root: &AudioClipEdit) -> Option<Viewport> {
        let clip = self.state.as_ref()?.project.clip(self.selected_clip?)?;
        let local = self.event_viewport(root);
        Some(Viewport {
            start: clip.start().checked_add(local.start)?,
            length: local.length,
        })
    }
    fn event_pointer_position(&self, x: f32, platform: bool) -> Option<u64> {
        let (_, root, _) = self.event_context()?;
        let state = self.state.as_ref()?;
        let bounds = self.audio_event_editor.bounds?;
        let width = f32::from(bounds.size.width);
        if !width.is_finite() || width <= 0.0 {
            return None;
        }
        let viewport = self.event_project_viewport(&root)?;
        let fraction = ((x - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
        let candidate = viewport
            .start
            .checked_add((fraction as f64 * viewport.length as f64).round() as u64)?;
        let snapped = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform)
            .snap_frame(&state.project, candidate, viewport, width)?;
        self.event_position_from_project_frame(&root, snapped)
    }
    fn event_position_from_project_frame(&self, root: &AudioClipEdit, frame: u64) -> Option<u64> {
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.selected_clip?)?;
        let first = root
            .root_frame_at(root.view_start, state.project.tempo_map())
            .ok()?;
        let local = frame.saturating_sub(clip.start()).min(clip.duration());
        root.root_position_at(first.checked_add(local)?, state.project.tempo_map())
            .ok()
    }
    fn event_snap_position(&self, position: u64, platform: bool) -> Option<u64> {
        let (_, root, _) = self.event_context()?;
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.selected_clip?)?;
        let first = root
            .root_frame_at(root.view_start, state.project.tempo_map())
            .ok()?;
        let frame = clip.start().checked_add(
            root.root_frame_at(position, state.project.tempo_map())
                .ok()?
                .saturating_sub(first),
        )?;
        let viewport = self.event_project_viewport(&root)?;
        let width = f32::from(self.audio_event_editor.bounds?.size.width);
        let frame = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform)
            .snap_frame(&state.project, frame, viewport, width)?;
        self.event_position_from_project_frame(&root, frame)
    }
    fn event_update_pointer(&mut self, x: f32, platform: bool) {
        if !self.event_context_current() || self.edit_pending() {
            self.audio_event_editor.gesture = None;
            return;
        }
        let Some(mut gesture) = self.audio_event_editor.gesture.clone() else {
            return;
        };
        gesture.pointer_x = x;
        if !matches!(gesture.kind, Kind::Select) && !gesture.moved {
            if (x - gesture.origin_x).abs() < 3.0 {
                self.audio_event_editor.gesture = Some(gesture);
                return;
            }
            gesture.moved = true;
        }

        let Some((_, root, _)) = self.event_context() else {
            return;
        };
        let Some(raw) =
            self.event_pointer_position(x, self.audio_event_editor.grid != SnapGrid::Off)
        else {
            return;
        };
        let position = self.event_pointer_position(x, platform).unwrap_or(raw);
        match gesture.kind {
            Kind::Select => {
                self.audio_event_editor.range =
                    Some((gesture.anchor.min(position), gesture.anchor.max(position)));
                self.audio_event_editor.cursor = position;
            }
            Kind::RangeMove { start, end } => {
                let candidate = (i128::from(start) + i128::from(raw) - i128::from(gesture.anchor))
                    .clamp(
                        i128::from(root.view_start),
                        i128::from(
                            root.view_end
                                .saturating_sub(end - start)
                                .max(root.view_start),
                        ),
                    ) as u64;
                let destination = self
                    .event_snap_position(candidate, platform)
                    .unwrap_or(candidate)
                    .clamp(
                        root.view_start,
                        root.view_end
                            .saturating_sub(end - start)
                            .max(root.view_start),
                    );
                gesture.operation = Some(AudioEventEdit::MoveRange {
                    start,
                    end,
                    destination,
                });
            }
            Kind::Move(event) => {
                let candidate =
                    (i128::from(event.start) + i128::from(raw) - i128::from(gesture.anchor)).clamp(
                        i128::from(root.view_start),
                        i128::from(
                            root.view_end
                                .saturating_sub(event.length())
                                .max(root.view_start),
                        ),
                    ) as u64;
                let start = self
                    .event_snap_position(candidate, platform)
                    .unwrap_or(candidate)
                    .clamp(
                        root.view_start,
                        root.view_end
                            .saturating_sub(event.length())
                            .max(root.view_start),
                    );
                gesture.operation = Some(AudioEventEdit::Move {
                    event: event.id,
                    start,
                });
            }
            Kind::Leading(event) => {
                let start = position.clamp(root.view_start.min(event.end - 1), event.end - 1);
                gesture.operation = Some(AudioEventEdit::Trim {
                    event: event.id,
                    start,
                    end: event.end,
                });
            }
            Kind::Trailing(event) => {
                let end = position.clamp(event.start + 1, root.view_end.max(event.start + 1));
                gesture.operation = Some(AudioEventEdit::Trim {
                    event: event.id,
                    start: event.start,
                    end,
                });
            }
        }
        self.audio_event_editor.gesture = Some(gesture);
    }
    fn event_nudge(&mut self, right: bool, trim: bool, platform: bool) {
        let Some((clip_id, root, _)) = self.event_context() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(clip) = state.project.clip(clip_id) else {
            return;
        };
        let Ok(events) = clip.editable_audio_events() else {
            return;
        };
        let Some(event) = events
            .events
            .iter()
            .find(|e| Some(e.id) == self.audio_event_editor.selected)
            .copied()
        else {
            return;
        };
        let position = if trim { event.end } else { event.start };
        let first = root
            .root_frame_at(root.view_start, state.project.tempo_map())
            .unwrap_or(0);
        let frame = clip.start().saturating_add(
            root.root_frame_at(position, state.project.tempo_map())
                .unwrap_or(first)
                .saturating_sub(first),
        );
        let Some(viewport) = self.event_project_viewport(&root) else {
            return;
        };
        let grid = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform);
        let width = self
            .audio_event_editor
            .bounds
            .map_or(640.0, |b| f32::from(b.size.width));
        let next = if grid == SnapGrid::Off {
            Some(if right {
                frame.saturating_add(1)
            } else {
                frame.saturating_sub(1)
            })
        } else {
            grid.adjacent_frame(&state.project, frame, viewport, width, right)
        };
        let Some(next) =
            next.and_then(|frame| self.event_position_from_project_frame(&root, frame))
        else {
            return;
        };
        if trim {
            if next > event.start {
                self.event_commit(AudioEventEdit::Trim {
                    event: event.id,
                    start: event.start,
                    end: next,
                });
            }
        } else {
            let start = next.clamp(
                root.view_start,
                root.view_end
                    .saturating_sub(event.length())
                    .max(root.view_start),
            );
            self.event_commit(AudioEventEdit::Move {
                event: event.id,
                start,
            });
        }
    }
    fn event_select_all(&mut self) {
        if let Some((_, root, _)) = self.event_context() {
            self.audio_event_editor.range = Some((root.view_start, root.view_end));
            self.audio_event_editor.cursor = root.view_start;
        }
    }
    fn event_duplicate(&mut self) {
        let Some((start, end)) = self.event_range() else {
            return;
        };
        let Some((clip, _, _)) = self.event_context() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(clip) = state.project.clip(clip) else {
            return;
        };
        let events = match clip.editable_audio_events() {
            Ok(events) => events.copy_range(start, end),
            Err(error) => {
                self.error = Some(error.0.into());
                return;
            }
        };
        self.event_commit(AudioEventEdit::Paste {
            at: end,
            length: end - start,
            events,
        });
    }
    pub(super) fn refresh_audio_event_modifiers(&mut self, platform: bool) -> bool {
        let Some(pointer) = self
            .audio_event_editor
            .gesture
            .as_ref()
            .map(|g| g.pointer_x)
        else {
            return false;
        };
        self.event_update_pointer(pointer, platform);
        true
    }
    fn event_zoom(&mut self, inward: bool) {
        let Some((_, root, _)) = self.event_context() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let first = root
            .root_frame_at(root.view_start, state.project.tempo_map())
            .unwrap_or(0);
        let horizon = root
            .root_frame_at(root.view_end, state.project.tempo_map())
            .unwrap_or(first)
            .saturating_sub(first);
        let cursor = root
            .root_frame_at(self.audio_event_editor.cursor, state.project.tempo_map())
            .unwrap_or(first)
            .saturating_sub(first);
        self.audio_event_editor.viewport =
            Some(self.event_viewport(&root).zoom(horizon, cursor, inward));
        self.audio_event_editor.gesture = None;
    }
    fn event_pan(&mut self, right: bool) {
        let Some((_, root, _)) = self.event_context() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let first = root
            .root_frame_at(root.view_start, state.project.tempo_map())
            .unwrap_or(0);
        let end = root
            .root_frame_at(root.view_end, state.project.tempo_map())
            .unwrap_or(first);
        self.audio_event_editor.viewport =
            Some(self.event_viewport(&root).pan_half(end - first, right));
        self.audio_event_editor.gesture = None;
    }
}
