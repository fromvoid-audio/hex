//! Clip-local waveform editing. Only selection, viewport and previews live here.
//! A shared Window lease owns held event/selection/fade input; pending receipts
//! retain frozen release geometry independently until the canonical terminal.
mod fades;
#[cfg(all(test, feature = "ui-test-support"))]
mod free_placement_tests;
mod navigation;
mod paint;
mod pending;
mod placement;
mod play_start;
mod playhead;
#[cfg(all(test, feature = "ui-test-support"))]
mod playhead_tests;
mod pointer;
mod projection;
#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
mod view;
mod waveform_preview;
#[cfg(all(test, feature = "ui-test-support"))]
mod workflow_tests;
use super::{
    EditorView, musical_time::SnapGrid, signed_frame_viewport::SignedFrameViewport,
    viewport::Viewport,
};
use gpui::{Bounds, Pixels};
use hex_project::{AudioClipEdit, AudioEvent, AudioEventEdit, ClipId};
use hex_session::service::Request;
pub(super) use placement::MusicalEventWaveform;
pub(in crate::editor) use play_start::Camera as PlayStartCamera;
pub(super) use view::view;

/// Placed cycles expose the complete retained root while loop markers move.
/// Frame containers also expose material after the Arrangement window ends;
/// their camera origin stays at the Arrangement window start. These
/// display bounds never extend the Arrangement window or alter playback.
/// Units remain the root's declared AudioDomainClock.
pub(in crate::editor) fn editor_bounds(root: &AudioClipEdit) -> (u64, u64) {
    if root.placement.is_some() {
        (0, root.domain.clock.length())
    } else if root
        .domain
        .events
        .as_ref()
        .is_some_and(|e| !e.sources.is_empty())
    {
        (root.view_start, root.domain.clock.length())
    } else {
        (root.view_start, root.view_end)
    }
}
/// The event grid uses the frame cycle start as its project-time anchor.
pub(in crate::editor) fn event_grid_origin(root: &AudioClipEdit) -> u64 {
    if matches!(root.timing, hex_project::AudioClipTiming::Frames) {
        root.view_start
    } else {
        editor_bounds(root).0
    }
}

/// Grouped events and processed frame roots expose their retained material
/// independently of the Arrangement window.
fn content_bounds(root: &AudioClipEdit) -> (u64, u64) {
    if root.domain.musical_event_source.is_some() {
        return (0, root.domain.clock.length());
    }
    if root.domain.events.as_ref().is_some_and(|events| {
        !events.sources.is_empty()
            || (matches!(root.timing, hex_project::AudioClipTiming::Frames)
                && (root.domain.looping
                    || root.domain.pitch_cents != 0
                    || !root.domain.source_gain.is_empty()
                    || root.content_view.is_some()))
    }) {
        (0, root.domain.clock.length())
    } else {
        editor_bounds(root)
    }
}
#[derive(Default)]
pub(super) struct State {
    owner: Option<(ClipId, u64)>,
    revision: u64,
    pub(super) source_view: bool,
    viewport: Option<Viewport>,
    negative_pan: u64,
    fit_back_negative: u64,
    frame_clip: Option<hex_project::Clip>,
    frame_bounds: Option<(u64, u64)>,
    // Last canonical (root position, projected frame), plus the latest admitted
    // grip (root position, visible frame offset, viewport length). Display only.
    canonical_start: Option<(u64, u64)>,
    start_grip: Option<(u64, u64, u64)>,
    frame_domain: Option<std::sync::Arc<hex_project::AudioClipDomain>>,
    source_viewport: Option<SignedFrameViewport>,
    source_fit_back: Option<Option<SignedFrameViewport>>,
    wheel: f32,
    source_wheel: f32,
    pub(super) source_bounds: Option<Bounds<Pixels>>,
    // Outer option is a retained fit; inner None preserves automatic full-view mode.
    fit_back: Option<Option<Viewport>>,
    cursor: u64,
    selected: Option<u64>,
    retained_selection: Option<(Vec<u64>, u64)>,
    retained_ancestors: Vec<u64>,
    range: Option<(u64, u64)>,
    grid: SnapGrid,
    pub(super) fade_key_held: bool,
    fade_hover: Option<u64>,
    fade_handle_hover: Option<(u64, super::fade_handles::Handle)>,
    bounds: Option<Bounds<Pixels>>,
    pub(super) gesture: Option<Gesture>,
    clipboard: Option<Clipboard>,
    pending: Option<pending::Pending>,
}
impl State {
    fn set_fade_hover(
        &mut self,
        owner: (ClipId, u64),
        revision: u64,
        event: u64,
        hovered: bool,
    ) -> bool {
        if self.owner != Some(owner) || self.revision != revision {
            return false;
        }
        let next = if hovered {
            Some(event)
        } else {
            self.fade_hover.filter(|id| *id != event)
        };
        if self.fade_hover == next {
            return false;
        }
        self.fade_hover = next;
        true
    }

    pub(super) fn clear_fit_back(&mut self) {
        self.fit_back = None;
        self.fit_back_negative = 0;
        self.source_fit_back = None;
    }
}
struct Clipboard {
    owner: (ClipId, u64),
    root: AudioClipEdit,
    length: u64,
    events: Vec<AudioEvent>,
    waveform: Option<waveform_preview::Source>,
}

fn retained_events(clip: &hex_project::Clip, path: &[u64]) -> Option<hex_project::AudioEvents> {
    let mut clip = clip.clone();
    for source in path {
        let events = clip.editable_audio_events().ok()?;
        clip = events
            .sources
            .iter()
            .find(|root| root.id == *source)?
            .clip
            .clone();
    }
    clip.editable_audio_events().ok()
}
#[derive(Clone)]
pub(super) struct Gesture {
    anchor: u64,
    pointer_x: f32,
    origin_x: f32,
    moved: bool,
    kind: Kind,
    operation: Option<AudioEventEdit>,
    placement: Option<std::sync::Arc<placement::Preview>>,
}
#[derive(Clone, Copy)]
enum Kind {
    Select,
    PlayStart {
        position: u64,
    },
    RangeMove {
        start: u64,
        end: u64,
    },
    FreeRangeMove {
        start: u64,
        end: u64,
        limits: (i64, i64),
    },
    Move(AudioEvent),
    Duplicate {
        event: AudioEvent,
        limits: Option<(i64, i64)>,
    },
    FreeMove {
        event: AudioEvent,
        limits: (i64, i64),
    },
    Leading(AudioEvent),
    Trailing(AudioEvent),
    Fade(fades::FadeGesture),
    Gain {
        event: AudioEvent,
        origin_y: f32,
        last_y: f32,
        value: f64,
    },
}

impl EditorView {
    fn event_context(&self) -> Option<(ClipId, AudioClipEdit, u64)> {
        let state = self.state.as_ref()?;
        let clip = self.detail_clip_id()?;
        let source = state.project.clip(clip)?;
        Some((clip, source.audio_event_root(), state.revision))
    }
    pub(super) fn held_audio_play_start_audition(
        &self,
    ) -> Option<hex_session::service::ArrangementPlayStartAudition> {
        use hex_session::service::{ArrangementPlayStartAudition, PlayStartEdit};
        let gesture = self.audio_event_editor.gesture.as_ref()?;
        let Kind::PlayStart { position } = gesture.kind else {
            return None;
        };
        if position == gesture.anchor {
            return None;
        }
        let clip = self.detail_clip_id()?;
        let editor_revision = self.state.as_ref()?.revision;
        if self.audio_event_editor.owner != Some((clip, self.project_generation))
            || self.audio_event_editor.revision != editor_revision
        {
            return None;
        }
        Some(ArrangementPlayStartAudition {
            context: hex_engine::AudioSourceContext {
                project_generation: self.project_generation,
                editor_revision,
            },
            edit: PlayStartEdit::Audio { clip, position },
        })
    }

    fn event_context_current(&self) -> bool {
        self.event_context().is_some_and(|(clip, _, revision)| {
            self.audio_event_editor.owner == Some((clip, self.project_generation))
                && self.audio_event_editor.revision == revision
        })
    }
    pub(in crate::editor) fn audio_loop_current(&self) -> bool {
        self.event_context_current()
    }
    pub(in crate::editor) fn audio_loop_grid(&self) -> SnapGrid {
        self.audio_event_editor.grid
    }
    pub(in crate::editor) fn audio_loop_time_selection(&self) -> Option<(u64, u64)> {
        self.event_context_current()
            .then_some(self.audio_event_editor.range)
            .flatten()
    }
    fn event_range(&self) -> Option<(u64, u64)> {
        if let Some(range) = self.audio_event_editor.range.filter(|(a, b)| a < b) {
            return Some(range);
        }
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.detail_clip_id()?)?;
        let selected = self.audio_event_editor.selected?;
        clip.editable_audio_events()
            .ok()?
            .events
            .iter()
            .find(|e| e.id == selected)
            .map(|e| (e.start, e.end))
    }
    /// Toggle the explicitly selected event; retained geometry and source stay intact.
    pub(in crate::editor) fn event_toggle_active(&mut self) -> bool {
        if self.audio_event_editor.source_view || !self.event_context_current() {
            return false;
        }
        let Some(selected) = self.audio_event_editor.selected else {
            return false;
        };
        if self.activation_gesture_blocked()
            || self.audio_event_editor.pending.is_some()
            || self.audio_event_editor.gesture.is_some()
        {
            self.error = Some("finish the current edit before changing activation".into());
            return true;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(clip) = self.detail_clip_id().and_then(|id| state.project.clip(id)) else {
            return false;
        };
        let Ok(events) = clip.editable_audio_events() else {
            return false;
        };
        let Some(event) = events.events.iter().find(|event| event.id == selected) else {
            return false;
        };
        if let Some((path, event_id)) = self.audio_event_editor.retained_selection.clone() {
            if path.first() != Some(&event.source) {
                self.error = Some("retained audio selection changed".into());
                return true;
            }
            let Some(inner) = retained_events(clip, &path) else {
                return false;
            };
            let Some(event) = inner.events.iter().find(|event| event.id == event_id) else {
                self.error = Some("retained audio selection changed".into());
                return true;
            };
            let active = !event.active;
            self.event_commit(AudioEventEdit::SetRetainedPathActive {
                sources: path,
                events: vec![event_id],
                active,
            });
            return true;
        }
        let active = !event.active;
        self.event_commit(AudioEventEdit::SetActive {
            events: vec![selected],
            active,
        });
        true
    }
    fn event_retained_step(&mut self, forward: bool) {
        if self.activation_gesture_blocked()
            || self.audio_event_editor.pending.is_some()
            || self.audio_event_editor.gesture.is_some()
        {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(clip) = self.detail_clip_id().and_then(|id| state.project.clip(id)) else {
            return;
        };
        let Ok(events) = clip.editable_audio_events() else {
            return;
        };
        let Some(outer) = events
            .events
            .iter()
            .find(|event| Some(event.id) == self.audio_event_editor.selected)
        else {
            return;
        };
        let continuing = self
            .audio_event_editor
            .retained_selection
            .as_ref()
            .is_some_and(|(path, _)| path.first() == Some(&outer.source));
        if !continuing {
            self.audio_event_editor.retained_ancestors.clear();
        }
        let path = self
            .audio_event_editor
            .retained_selection
            .as_ref()
            .filter(|(path, _)| path.first() == Some(&outer.source))
            .map_or_else(|| vec![outer.source], |(path, _)| path.clone());
        let Some(inner) = retained_events(clip, &path) else {
            return;
        };
        if inner.events.is_empty() {
            return;
        }
        let current = self
            .audio_event_editor
            .retained_selection
            .as_ref()
            .filter(|(selected_path, _)| *selected_path == path)
            .and_then(|(_, id)| inner.events.iter().position(|event| event.id == *id));
        let index = match current {
            Some(index) if forward => (index + 1) % inner.events.len(),
            Some(index) => (index + inner.events.len() - 1) % inner.events.len(),
            None => 0,
        };
        self.audio_event_editor.retained_selection = Some((path, inner.events[index].id));
    }

    fn event_retained_depth(&mut self, deeper: bool) {
        if self.activation_gesture_blocked()
            || self.audio_event_editor.pending.is_some()
            || self.audio_event_editor.gesture.is_some()
            || !self.event_context_current()
        {
            return;
        }
        let Some((mut path, selected)) = self.audio_event_editor.retained_selection.clone() else {
            return;
        };
        let Some(clip) = self
            .state
            .as_ref()
            .and_then(|state| self.detail_clip_id().and_then(|id| state.project.clip(id)))
        else {
            return;
        };
        let Some(current) = retained_events(clip, &path) else {
            return;
        };
        if deeper {
            let Some(event) = current.events.iter().find(|event| event.id == selected) else {
                return;
            };
            if event.source == 0 || path.len() >= hex_project::MAX_RETAINED_AUDIO_DEPTH {
                return;
            }
            path.push(event.source);
            let Some(next) = retained_events(clip, &path) else {
                return;
            };
            let Some(event) = next.events.first() else {
                return;
            };
            self.audio_event_editor.retained_ancestors.push(selected);
            self.audio_event_editor.retained_selection = Some((path, event.id));
        } else if path.len() > 1 {
            let Some(source) = path.pop() else {
                return;
            };
            let Some(parent) = retained_events(clip, &path) else {
                return;
            };
            let Some(parent_id) = self.audio_event_editor.retained_ancestors.last().copied() else {
                return;
            };
            let Some(event) = parent
                .events
                .iter()
                .find(|event| event.id == parent_id && event.source == source)
            else {
                return;
            };
            self.audio_event_editor.retained_ancestors.pop();
            self.audio_event_editor.retained_selection = Some((path, event.id));
        }
    }
    pub(super) fn event_group_selected(&mut self) {
        if self.audio_event_editor.source_view
            || !self.event_context_current()
            || self.edit_pending()
            || self.audio_event_editor.pending.is_some()
        {
            return;
        }
        let Some((clip, _, revision)) = self.event_context() else {
            return;
        };
        let Some((start, end)) = self.event_range().filter(|(start, end)| start < end) else {
            return;
        };
        self.enqueue(Request::GroupAudioEvents {
            project_generation: self.project_generation,
            revision,
            clip,
            start,
            end,
        });
    }
    fn event_copy(&mut self, cut: bool) {
        if self.edit_pending()
            || self.audio_event_editor.pending.is_some()
            || !self.event_context_current()
        {
            return;
        }
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
        let clipboard = Clipboard {
            owner: (clip, self.project_generation),
            waveform: waveform_preview::Source::capture(clip, &root, state, start, end),
            root,
            length: end - start,
            events: match source.editable_audio_events() {
                Ok(events) => events.copy_range(start, end),
                Err(error) => {
                    self.error = Some(error.0.into());
                    return;
                }
            },
        };
        if cut {
            self.event_commit(AudioEventEdit::Delete { start, end });
            if let Some(pending) = self.audio_event_editor.pending.as_mut() {
                pending.clipboard = Some(clipboard);
            }
        } else {
            self.audio_event_editor.clipboard = Some(clipboard);
            self.audio_event_editor.cursor = start;
        }
    }
    fn event_paste(&mut self, insert: bool) {
        if self.edit_pending()
            || self.audio_event_editor.pending.is_some()
            || !self.event_context_current()
        {
            return;
        }
        let Some((clip, mut root, _)) = self.event_context() else {
            return;
        };
        let Some(clipboard) = self.audio_event_editor.clipboard.as_ref() else {
            return;
        };
        let last_event_end = root
            .domain
            .events
            .as_ref()
            .map_or(root.domain.clock.length(), |events| {
                events.events.last().map_or(0, |event| event.end)
            });
        let mut saved = clipboard.root.clone();
        std::sync::Arc::make_mut(&mut root.domain).events = None;
        std::sync::Arc::make_mut(&mut saved.domain).events = None;
        if clipboard.owner != (clip, self.project_generation) || root.domain != saved.domain {
            self.error = Some(
                "audio event clipboard belongs to a different or changed clip; copy the events again".into(),
            );
            return;
        }
        if !event_paste_fits_view(&root, self.audio_event_editor.cursor, clipboard.length) {
            self.error = Some("audio event paste exceeds the visible clip".into());
            return;
        }
        if insert
            && last_event_end
                .checked_add(clipboard.length)
                .is_none_or(|end| end > editor_bounds(&root).1)
        {
            self.error = Some("audio event insert needs free time inside the visible clip".into());
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
        let waveform = self.state.as_ref().and_then(|state| {
            clipboard.waveform.as_ref().and_then(|source| {
                source.preview(
                    &root,
                    state.project.tempo_map(),
                    self.audio_event_editor.cursor,
                    clipboard.length,
                )
            })
        });
        self.event_commit(operation);
        if let Some(pending) = &mut self.audio_event_editor.pending {
            pending.waveform = waveform;
        }
    }
    fn event_delete(&mut self) {
        if let Some((start, end)) = self.event_range() {
            self.event_commit(AudioEventEdit::Delete { start, end });
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
        if at > editor_bounds(&root).0 && at < editor_bounds(&root).1 {
            self.event_commit(AudioEventEdit::Split { at });
        }
    }
    // Releasing outside the waveform ends the same edit as releasing
    // inside it. Only explicit cancellation discards held input.
    fn event_release_pointer(&mut self, x: f32, platform: bool) {
        self.event_update_pointer(x, platform);
        if let Some(gesture) = self.audio_event_editor.gesture.take() {
            if let Kind::PlayStart { position } = gesture.kind {
                if position != gesture.anchor
                    && let Some((clip, _, _)) = self.event_context()
                {
                    self.finish_held_play_start_audition();
                    self.commit_audio_clip_loop(
                        clip,
                        hex_project::AudioViewResizeOperation::PlayStart { position },
                    );
                    self.settle_released_play_start_audition(clip);
                } else {
                    self.sync_held_play_start_audition();
                }
            } else if let Some(operation) = gesture.operation {
                self.event_commit(operation);
            }
        }
    }
    fn event_update_gain(&mut self, y: f32, fine: bool) {
        if !y.is_finite() || !self.event_context_current() || self.edit_pending() {
            self.audio_event_editor.gesture = None;
            return;
        }
        let Some(gesture) = self.audio_event_editor.gesture.as_mut() else {
            return;
        };
        let Kind::Gain {
            event,
            origin_y,
            last_y,
            value,
        } = &mut gesture.kind
        else {
            return;
        };
        *value = (*value + (f64::from(*last_y) - f64::from(y)) * if fine { 10.0 } else { 100.0 })
            .clamp(
                f64::from(hex_project::ClipGain::MIN_MILLIDB),
                f64::from(hex_project::ClipGain::MAX_MILLIDB),
            );
        *last_y = y;
        gesture.moved |= (y - *origin_y).abs() >= 1.0;
        let millidb = if gesture.moved {
            value.round() as i32
        } else {
            event.gain.millidb()
        };
        gesture.operation = hex_project::ClipGain::new(millidb)
            .ok()
            .filter(|gain| *gain != event.gain)
            .map(|gain| match &self.audio_event_editor.retained_selection {
                Some((sources, id)) if *id == event.id => AudioEventEdit::SetRetainedGain {
                    sources: sources.clone(),
                    event: event.id,
                    gain,
                },
                _ => AudioEventEdit::SetGain {
                    event: event.id,
                    gain,
                },
            });
    }
    fn event_update_pointer(&mut self, x: f32, platform: bool) {
        if let Some(Kind::Fade(fade)) = self.audio_event_editor.gesture.as_ref().map(|g| g.kind) {
            self.event_update_fade_pointer(x, fade.last_y, platform);
            return;
        }
        if !self.event_context_current() || self.edit_pending() {
            self.audio_event_editor.gesture = None;
            self.sync_held_play_start_audition();
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
            Kind::PlayStart { .. } => {
                let Some(position) = self.audio_play_start_at_pointer(&root, &gesture, x, platform)
                else {
                    return;
                };
                gesture.kind = Kind::PlayStart { position };
            }
            Kind::Select => {
                self.audio_event_editor.range =
                    Some((gesture.anchor.min(position), gesture.anchor.max(position)));
                self.audio_event_editor.cursor = position;
            }
            Kind::RangeMove { start, end } => {
                let candidate = (i128::from(start) + i128::from(raw) - i128::from(gesture.anchor))
                    .clamp(
                        i128::from(content_bounds(&root).0),
                        i128::from(
                            content_bounds(&root)
                                .1
                                .saturating_sub(end - start)
                                .max(content_bounds(&root).0),
                        ),
                    ) as u64;
                let destination = self
                    .event_snap_position(candidate, platform)
                    .unwrap_or(candidate)
                    .clamp(
                        content_bounds(&root).0,
                        content_bounds(&root)
                            .1
                            .saturating_sub(end - start)
                            .max(content_bounds(&root).0),
                    );
                gesture.operation = Some(AudioEventEdit::MoveRange {
                    start,
                    end,
                    destination,
                });
            }
            Kind::FreeMove { .. }
            | Kind::FreeRangeMove { .. }
            | Kind::Duplicate {
                limits: Some(_), ..
            } => self.update_free_event_placement(&mut gesture, x, platform),
            Kind::Move(event)
            | Kind::Duplicate {
                event,
                limits: None,
            } => {
                let candidate =
                    (i128::from(event.start) + i128::from(raw) - i128::from(gesture.anchor)).clamp(
                        i128::from(content_bounds(&root).0),
                        i128::from(
                            content_bounds(&root)
                                .1
                                .saturating_sub(event.length())
                                .max(content_bounds(&root).0),
                        ),
                    ) as u64;
                let start = self
                    .event_snap_position(candidate, platform)
                    .unwrap_or(candidate)
                    .clamp(
                        content_bounds(&root).0,
                        content_bounds(&root)
                            .1
                            .saturating_sub(event.length())
                            .max(content_bounds(&root).0),
                    );
                gesture.operation = if matches!(gesture.kind, Kind::Duplicate { .. }) {
                    i64::try_from(start)
                        .ok()
                        .map(|start| AudioEventEdit::Duplicate {
                            event: event.id,
                            start,
                        })
                } else {
                    Some(AudioEventEdit::Move {
                        event: event.id,
                        start,
                    })
                };
            }
            Kind::Leading(event) => {
                let start =
                    position.clamp(content_bounds(&root).0.min(event.end - 1), event.end - 1);
                gesture.operation = Some(AudioEventEdit::Trim {
                    event: event.id,
                    start,
                    end: event.end,
                });
            }
            Kind::Fade(_) | Kind::Gain { .. } => return,
            Kind::Trailing(event) => {
                let end = position.clamp(
                    event.start + 1,
                    content_bounds(&root).1.max(event.start + 1),
                );
                gesture.operation = Some(AudioEventEdit::Trim {
                    event: event.id,
                    start: event.start,
                    end,
                });
            }
        }
        if gesture.placement.is_some() && self.audio_event_editor.viewport.is_none() {
            self.store_event_camera(self.event_frame_window(&root));
        }
        let play_start = matches!(gesture.kind, Kind::PlayStart { .. });
        self.audio_event_editor.gesture = Some(gesture);
        if play_start {
            self.sync_held_play_start_audition();
        }
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
        let tempo = state.project.tempo_map();
        let Ok(origin) = root.root_frame_at(event_grid_origin(&root), tempo) else {
            return;
        };
        let Ok(frame) = root.root_frame_at(position, tempo) else {
            return;
        };
        let frame = i128::from(clip.start()) + i128::from(frame) - i128::from(origin);
        let Some(window) = self.event_project_window(&root) else {
            return;
        };
        let width = self
            .audio_event_editor
            .bounds
            .map_or(640.0, |b| f32::from(b.size.width));
        let Some(next) = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform)
            .adjacent_signed_frame(&state.project, frame, window, width, right)
        else {
            return;
        };
        if !trim
            && let Ok((first, last)) =
                state
                    .project
                    .audio_event_drag_bounds(clip_id, event.start, event.end)
        {
            let frame = next - i128::from(clip.start()) + i128::from(origin);
            let Ok(raw) = root.signed_root_position_at(frame, state.project.tempo_map()) else {
                return;
            };
            let start = raw.clamp(i128::from(first), i128::from(last)) as i64;
            self.event_commit(AudioEventEdit::Place {
                event: event.id,
                start,
            });
            return;
        }
        let Some(next) = self.event_position_from_signed_project_frame(&root, next) else {
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
            let bounds = content_bounds(&root);
            let start = next.clamp(
                bounds.0,
                bounds.1.saturating_sub(event.length()).max(bounds.0),
            );
            self.event_commit(AudioEventEdit::Move {
                event: event.id,
                start,
            });
        }
    }
    fn event_select_all(&mut self) {
        if let Some((_, root, _)) = self.event_context() {
            self.advance_selection_intent();
            self.audio_event_editor.range = Some((editor_bounds(&root).0, editor_bounds(&root).1));
            self.audio_event_editor.cursor = editor_bounds(&root).0;
        }
    }
    fn duplicate_selected_event(&mut self) {
        let Some(selected) = self.audio_event_editor.selected else {
            return;
        };
        let Some((clip, _, _)) = self.event_context() else {
            return;
        };
        let Some(source) = self
            .state
            .as_ref()
            .and_then(|state| state.project.clip(clip))
        else {
            return;
        };
        let Some(event) = source
            .editable_audio_events()
            .ok()
            .and_then(|events| events.events.into_iter().find(|event| event.id == selected))
        else {
            return;
        };
        let Ok(start) = i64::try_from(event.end) else {
            self.error = Some("audio event duplicate exceeds its timeline".into());
            return;
        };
        self.event_commit(AudioEventEdit::Duplicate {
            event: event.id,
            start,
        });
    }
    fn event_duplicate(&mut self) {
        let Some((start, end)) = self.event_range() else {
            return;
        };
        let Some((clip, root, _)) = self.event_context() else {
            return;
        };
        if !event_paste_fits_view(&root, end, end - start) {
            self.error = Some("audio event duplicate exceeds the visible clip".into());
            return;
        }
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
    fn remember_audio_fit(&mut self) {
        if self.event_context_current() {
            self.audio_event_editor.fit_back = Some(self.audio_event_editor.viewport);
            self.audio_event_editor.fit_back_negative = self.audio_event_editor.negative_pan;
        }
    }
    /// Manual fit calculation in the source root's output clock.
    pub(super) fn fit_audio_interval(&mut self, range: (u64, u64), remember: bool) -> bool {
        if !self.event_context_current() {
            return false;
        }
        let Some((_, root, _)) = self.event_context() else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let tempo = state.project.tempo_map();
        let bounds = content_bounds(&root);
        let Ok(first) = root.root_frame_at(editor_bounds(&root).0, tempo) else {
            return false;
        };
        let (Ok(a), Ok(b)) = (
            root.root_frame_at(range.0.clamp(bounds.0, bounds.1), tempo),
            root.root_frame_at(range.1.clamp(bounds.0, bounds.1), tempo),
        ) else {
            return false;
        };
        if a >= b {
            return false;
        }
        let window = SignedFrameViewport {
            start: i128::from(a) - i128::from(first),
            length: b - a,
        };
        if window == self.event_frame_window(&root) {
            return false;
        }
        if remember {
            self.remember_audio_fit();
        }
        self.audio_event_editor.gesture = None;
        self.store_event_camera(window)
    }
    pub(super) fn event_zoom_back_available(&self) -> bool {
        self.event_context_current()
            && if self.audio_event_editor.source_view {
                self.audio_event_editor.source_fit_back.is_some()
            } else {
                self.audio_event_editor.fit_back.is_some()
            }
    }
    pub(super) fn event_fit_selection(&mut self, back: bool) -> bool {
        if !self.audio_navigation_idle() {
            return false;
        }
        if self.audio_event_editor.source_view {
            return self.source_fit(back);
        }
        if !self.event_context_current() {
            return false;
        }
        let Some((_, root, _)) = self.event_context() else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Ok(first) = root.root_frame_at(editor_bounds(&root).0, state.project.tempo_map())
        else {
            return false;
        };
        let Ok(end) = root.root_frame_at(editor_bounds(&root).1, state.project.tempo_map()) else {
            return false;
        };
        let horizon = end.saturating_sub(first);
        if horizon == 0 {
            return false;
        }
        let next = if back {
            let Some(prior) = self.audio_event_editor.fit_back.take() else {
                return false;
            };
            prior
        } else {
            let range = self.audio_event_editor.range.and_then(|(a, b)| {
                let first = a
                    .min(b)
                    .clamp(content_bounds(&root).0, content_bounds(&root).1);
                let last = a
                    .max(b)
                    .clamp(content_bounds(&root).0, content_bounds(&root).1);
                (first < last).then_some((first, last))
            });
            return self.fit_audio_interval(
                range.unwrap_or((content_bounds(&root).0, content_bounds(&root).1)),
                true,
            );
        };
        self.audio_event_editor.gesture = None;
        self.audio_event_editor.viewport = next;
        self.audio_event_editor.negative_pan = if next.is_some() {
            self.audio_event_editor.fit_back_negative
        } else {
            0
        };
        true
    }
}

fn event_paste_fits_view(root: &AudioClipEdit, at: u64, length: u64) -> bool {
    length > 0
        && at >= editor_bounds(root).0
        && at
            .checked_add(length)
            .is_some_and(|end| end <= editor_bounds(root).1)
}
