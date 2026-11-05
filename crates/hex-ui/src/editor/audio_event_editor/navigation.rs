//! Independent, transient cameras in output-root and original-media frame clocks.
//! Navigation reveals blank space without changing edit bounds or project history.
use super::*;
use gpui::{Context, ScrollWheelEvent, Window, px};

impl State {
    /// Manual cameras use offsets from editor bounds. Preserve their absolute
    /// output-root frames when the same retained root gains or loses placement
    /// metadata, including Undo/Redo. A replacement root has its own clock.
    pub(super) fn install_frame_space(&mut self, root: &AudioClipEdit, bounds: (u64, u64)) {
        let retained = self.frame_domain.as_ref().is_some_and(|prior| {
            std::sync::Arc::ptr_eq(prior, &root.domain) || prior.as_ref() == root.domain.as_ref()
        });
        if !retained {
            self.canonical_start = None;
            self.start_grip = None;
        }
        if retained && let Some((first, _)) = self.frame_bounds {
            let rebase = |view: Viewport, negative: u64| {
                let start = i128::from(first) + i128::from(view.start)
                    - i128::from(negative)
                    - i128::from(bounds.0);
                (
                    Viewport {
                        start: u64::try_from(start.max(0)).unwrap_or(u64::MAX),
                        length: view.length,
                    },
                    u64::try_from((-start).max(0)).unwrap_or(u64::MAX),
                )
            };
            if let Some(view) = self.viewport {
                let (view, negative) = rebase(view, self.negative_pan);
                self.viewport = Some(view);
                self.negative_pan = negative;
            }
            if let Some(Some(view)) = self.fit_back {
                let (view, negative) = rebase(view, self.fit_back_negative);
                self.fit_back = Some(Some(view));
                self.fit_back_negative = negative;
            }
        }
        self.frame_bounds = Some(bounds);
        self.frame_domain = Some(root.domain.clone());
    }
}

impl EditorView {
    pub(in crate::editor) fn sync_audio_navigation(&mut self) {
        let Some((clip_id, root, revision)) = self.event_context() else {
            return;
        };
        let owner = (clip_id, self.project_generation);
        if self.audio_event_editor.owner != Some(owner) {
            let clipboard = self.audio_event_editor.clipboard.take();
            let pending = self.audio_event_editor.pending.take();
            let source_view = self.audio_event_editor.source_view;
            self.audio_event_editor = State {
                owner: Some(owner),
                revision,
                cursor: editor_bounds(&root).0,
                clipboard,
                pending,
                source_view,
                ..State::default()
            };
        }
        let remember_coordinates = self.audio_event_editor.revision != revision
            || self.audio_event_editor.frame_clip.is_none();
        if self.audio_event_editor.revision != revision
            && let Some(current) = self
                .state
                .as_ref()
                .and_then(|state| state.project.clip(clip_id))
            && let Some(previous) = self.audio_event_editor.frame_clip.as_ref()
            && let Some(shift) = current.audio_event_coordinate_shift_from(previous)
            && shift != 0
        {
            let length = root.domain.clock.length();
            let shifted = |position: u64| {
                (i128::from(position) + i128::from(shift)).clamp(0, i128::from(length)) as u64
            };
            let state = &mut self.audio_event_editor;
            state.cursor = shifted(state.cursor);
            state.range = state.range.and_then(|(a, b)| {
                let (a, b) = (shifted(a), shifted(b));
                (a < b).then_some((a, b))
            });
            state.canonical_start = state
                .canonical_start
                .map(|(raw, frame)| (shifted(raw), shifted(frame)));
            state.start_grip = state
                .start_grip
                .map(|(raw, offset, length)| (shifted(raw), offset, length));
        }
        if (self.audio_event_editor.revision != revision
            || self.audio_event_editor.frame_bounds.is_none())
            && let Some(tempo) = self.state.as_ref().map(|s| s.project.tempo_map())
        {
            let (first, end) = editor_bounds(&root);
            if let (Ok(first), Ok(end)) = (
                root.root_frame_at(first, tempo),
                root.root_frame_at(end, tempo),
            ) {
                self.audio_event_editor
                    .install_frame_space(&root, (first, end));
                let current = self.state.as_ref().and_then(|s| {
                    let clip = s.project.clip(clip_id)?;
                    let position = hex_project::AudioViewProjection::new(clip, tempo)
                        .ok()?
                        .play_start_position()
                        .ok()??;
                    Some((position, root.root_frame_at(position, tempo).ok()?))
                });
                let window = self.event_frame_window(&root);
                let old_start = self.audio_event_editor.canonical_start;
                if current != old_start
                    && let (Some((_, old_frame)), Some((position, frame))) = (old_start, current)
                    && window
                        .shifted(i128::from(first))
                        .is_some_and(|view| view.contains(i128::from(old_frame)))
                {
                    let local = i128::from(frame) - i128::from(first);
                    let start = if let Some((_, offset, length)) = self
                        .audio_event_editor
                        .start_grip
                        .filter(|(point, _, _)| *point == position)
                    {
                        let offset = (u128::from(offset) * u128::from(window.length)
                            / u128::from(length.max(1)))
                        .min(u128::from(u64::MAX)) as u64;
                        local - i128::from(offset)
                    } else if local < window.start {
                        local
                    } else if window.end().is_some_and(|end| local > end) {
                        local - i128::from(window.length)
                    } else {
                        window.start
                    };
                    self.store_event_camera(SignedFrameViewport {
                        start,
                        length: window.length,
                    });
                }
                self.audio_event_editor.canonical_start = current;
            }
        }
        if self.audio_event_editor.revision != revision {
            let (first, last) = content_bounds(&root);
            let state = &mut self.audio_event_editor;
            state.gesture = None;
            state.cursor = state.cursor.clamp(first, last);
            // Retained-root selections survive moving loop bounds.
            state.range = state.range.and_then(|(a, b)| {
                let a = a.max(first);
                let b = b.min(last);
                (a < b).then_some((a, b))
            });
            state.revision = revision;
        }
        if remember_coordinates {
            self.audio_event_editor.frame_clip = self
                .state
                .as_ref()
                .and_then(|state| state.project.clip(clip_id))
                .cloned();
        }
    }

    pub(super) fn event_navigation_limit(&self, root: &AudioClipEdit) -> u64 {
        let Some(state) = self.state.as_ref() else {
            return 0;
        };
        let frame = |p| root.root_frame_at(p, state.project.tempo_map()).ok();
        let Some(first) = frame(editor_bounds(root).0) else {
            return 0;
        };
        let placement = self
            .detail_clip_id()
            .and_then(|id| state.project.clip(id))
            .map_or(0, |c| c.start());
        (u64::MAX - first).min(u64::MAX - placement)
    }

    fn source_extent(&self) -> u64 {
        self.state
            .as_ref()
            .and_then(|s| {
                s.project
                    .clip(self.detail_clip_id()?)
                    .and_then(|c| s.project.asset(c.asset()))
            })
            .map_or(0, |a| a.playback_frames())
    }
    pub(in crate::editor) fn audio_source_viewport(&self) -> SignedFrameViewport {
        self.audio_event_editor
            .source_viewport
            .unwrap_or_else(|| Viewport::fit(self.source_extent()).into())
    }
    fn audio_navigation(&self) -> Option<(SignedFrameViewport, i128, i128, i128)> {
        if !self.event_context_current() {
            return None;
        }
        if self.audio_event_editor.source_view {
            let clip = self.state.as_ref()?.project.clip(self.detail_clip_id()?)?;
            Some((
                self.audio_source_viewport(),
                -i128::from(u64::MAX),
                i128::from(u64::MAX),
                i128::from(clip.source_offset().saturating_add(clip.source_phase())),
            ))
        } else {
            let (_, root, _) = self.event_context()?;
            let tempo = self.state.as_ref()?.project.tempo_map();
            let first = root.root_frame_at(editor_bounds(&root).0, tempo).ok()?;
            let cursor = i128::from(
                root.root_frame_at(self.audio_event_editor.cursor, tempo)
                    .ok()?,
            ) - i128::from(first);
            Some((
                self.event_frame_window(&root),
                -i128::from(u64::MAX),
                i128::from(self.event_navigation_limit(&root)),
                cursor,
            ))
        }
    }
    fn set_audio_navigation(&mut self, viewport: SignedFrameViewport) {
        if self.audio_event_editor.source_view {
            self.audio_event_editor.source_viewport = Some(viewport);
        } else {
            self.store_event_camera(viewport);
        }
    }
    pub(super) fn audio_navigation_idle(&self) -> bool {
        !self.clip_loop_held()
            && !self.edit_pending()
            && self.audio_event_editor.pending.is_none()
            && self.audio_event_editor.gesture.is_none()
            && self.context_menu.is_none()
    }
    pub(in crate::editor) fn event_zoom(&mut self, inward: bool) {
        if !self.audio_navigation_idle() {
            return;
        }
        if let Some((viewport, minimum, maximum, cursor)) = self.audio_navigation()
            && let Some(viewport) = viewport.zoom(minimum, maximum, cursor, inward, false)
        {
            self.set_audio_navigation(viewport);
        }
    }
    pub(in crate::editor) fn event_pan(&mut self, right: bool) {
        if !self.audio_navigation_idle() {
            return;
        }
        if let Some((viewport, minimum, maximum, _)) = self.audio_navigation() {
            let delta = i128::from((viewport.length / 2).max(1)) * if right { 1 } else { -1 };
            if let Some(viewport) = viewport.pan(minimum, maximum, delta) {
                self.set_audio_navigation(viewport);
            }
        }
    }
    pub(in crate::editor) fn audio_fit_all(&mut self) {
        if !self.audio_navigation_idle() {
            return;
        }
        if self.audio_event_editor.source_view {
            self.audio_event_editor.source_viewport = None;
        } else {
            self.audio_event_editor.viewport = None;
            self.audio_event_editor.negative_pan = 0;
            if let Some((_, root, _)) = self.event_context()
                && content_bounds(&root).0 < editor_bounds(&root).0
            {
                self.fit_audio_interval(content_bounds(&root), false);
            }
        }
    }
    pub(super) fn source_fit(&mut self, back: bool) -> bool {
        if !self.event_context_current() || !self.audio_navigation_idle() {
            return false;
        }
        if back {
            let Some(prior) = self.audio_event_editor.source_fit_back.take() else {
                return false;
            };
            self.audio_event_editor.source_viewport = prior;
        } else {
            let Some(clip) = self
                .state
                .as_ref()
                .and_then(|s| s.project.clip(self.detail_clip_id()?))
            else {
                return false;
            };
            let Some(end) = clip.source_offset().checked_add(clip.source_length()) else {
                return false;
            };
            let Some(next) = super::super::zoom_fit::interval(
                clip.source_offset(),
                end,
                self.source_extent(),
                1,
            ) else {
                return false;
            };
            let next = SignedFrameViewport::from(next);
            if next == self.audio_source_viewport() {
                return false;
            }
            self.audio_event_editor.source_fit_back = Some(self.audio_event_editor.source_viewport);
            self.audio_event_editor.source_viewport = Some(next);
        }
        true
    }
    pub(in crate::editor) fn audio_navigation_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        source: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> bool {
        if source != self.audio_event_editor.source_view
            || !self.audio_navigation_idle()
            || cx.has_active_drag()
            || window.is_pointer_dragging()
        {
            return false;
        }
        let Some(bounds) = (if source {
            self.audio_event_editor.source_bounds
        } else {
            self.audio_event_editor.bounds
        }) else {
            return false;
        };
        let width = f32::from(bounds.size.width);
        if !bounds.contains(&event.position) || !width.is_finite() || width <= 0.0 {
            return false;
        }
        let delta = event.physical_delta().pixel_delta(px(80.0));
        let (dx, dy) = (f32::from(delta.x), -f32::from(delta.y));
        if !dx.is_finite() || !dy.is_finite() || (dx == 0.0 && dy == 0.0) {
            return false;
        }
        let Some((mut viewport, minimum, maximum, _)) = self.audio_navigation() else {
            return false;
        };
        let fraction =
            ((f32::from(event.position.x) - f32::from(bounds.left())) / width).clamp(0.0, 1.0);
        let Some(anchor) = viewport
            .start
            .checked_add((viewport.length as f64 * f64::from(fraction)) as i128)
        else {
            return false;
        };
        let wheel = if source {
            &mut self.audio_event_editor.source_wheel
        } else {
            &mut self.audio_event_editor.wheel
        };
        *wheel = (*wheel + dy.clamp(-320.0, 320.0)).clamp(-320.0, 320.0);
        for _ in 0..4 {
            if wheel.abs() < 80.0 {
                break;
            }
            let inward = *wheel < 0.0;
            if let Some(next) = viewport.zoom(minimum, maximum, anchor, inward, true) {
                viewport = next;
            }
            *wheel += if inward { 80.0 } else { -80.0 };
        }
        let Some(viewport) = viewport.pan(
            minimum,
            maximum,
            (-f64::from(dx).clamp(-f64::from(width), f64::from(width)) * viewport.length as f64
                / f64::from(width))
            .round() as i128,
        ) else {
            return false;
        };
        self.set_audio_navigation(viewport);
        true
    }
}
