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
            let rebase = |v: Viewport| Viewport {
                start: first.saturating_add(v.start).saturating_sub(bounds.0),
                length: v.length,
            };
            self.viewport = self.viewport.map(rebase);
            self.fit_back = self.fit_back.map(|prior| prior.map(rebase));
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
                let state = &mut self.audio_event_editor;
                // History can install a different live tempo projection. Keep
                // its changed start visible without recording a second edit history.
                if current != state.canonical_start
                    && let (Some((_, old_frame)), Some((position, frame)), Some(view)) =
                        (state.canonical_start, current, state.viewport.as_mut())
                {
                    let old_visible = old_frame >= first.saturating_add(view.start)
                        && old_frame
                            <= first.saturating_add(view.start).saturating_add(view.length);
                    if old_visible {
                        let local = frame.saturating_sub(first);
                        view.start = if let Some((_, offset, length)) =
                            state.start_grip.filter(|(point, _, _)| *point == position)
                        {
                            let offset = (u128::from(offset) * u128::from(view.length)
                                / u128::from(length.max(1)))
                            .min(u128::from(u64::MAX))
                                as u64;
                            local.saturating_sub(offset)
                        } else if local < view.start {
                            local
                        } else if local > view.start.saturating_add(view.length) {
                            local.saturating_sub(view.length)
                        } else {
                            view.start
                        };
                    }
                }
                state.canonical_start = current;
            }
        }
        if self.audio_event_editor.revision != revision {
            let (first, last) = editor_bounds(&root);
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
    }
    fn audio_navigation_minimum(&self) -> u64 {
        self.state
            .as_ref()
            .and_then(|s| {
                hex_project::BeatTime::from_quarters(16)
                    .ok()
                    .and_then(|beat| s.project.tempo_map().beat_to_frame(beat).ok())
            })
            .unwrap_or(1)
            .max(1)
    }
    pub(super) fn event_navigation_limit(&self, root: &AudioClipEdit) -> u64 {
        let Some(state) = self.state.as_ref() else {
            return 0;
        };
        let frame = |p| root.root_frame_at(p, state.project.tempo_map()).ok();
        let Some(first) = frame(editor_bounds(root).0) else {
            return 0;
        };
        let Some(end) = frame(editor_bounds(root).1) else {
            return 0;
        };
        let placement = self
            .detail_clip_id()
            .and_then(|id| state.project.clip(id))
            .map_or(0, |c| c.start());
        let source = self.detail_clip_id().and_then(|id| state.project.clip(id));
        source
            .and_then(|clip| clip.audio_event_placement_limit())
            .map_or_else(
                || navigation_limit(end.saturating_sub(first), self.audio_navigation_minimum()),
                |limit| limit.saturating_sub(first),
            )
            .min(u64::MAX - first)
            .min(u64::MAX - placement)
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
    fn source_navigation_limit(&self) -> u64 {
        navigation_limit(self.source_extent(), self.audio_navigation_minimum())
    }
    pub(in crate::editor) fn audio_source_viewport(&self) -> Viewport {
        self.audio_event_editor
            .source_viewport
            .unwrap_or(Viewport::fit(self.source_extent()))
            .clamp(self.source_navigation_limit())
    }
    fn audio_navigation(&self) -> Option<(Viewport, u64, u64)> {
        if !self.event_context_current() {
            return None;
        }
        if self.audio_event_editor.source_view {
            let clip = self.state.as_ref()?.project.clip(self.detail_clip_id()?)?;
            Some((
                self.audio_source_viewport(),
                self.source_navigation_limit(),
                clip.source_offset().saturating_add(clip.source_phase()),
            ))
        } else {
            let (_, root, _) = self.event_context()?;
            let tempo = self.state.as_ref()?.project.tempo_map();
            let first = root.root_frame_at(editor_bounds(&root).0, tempo).ok()?;
            let cursor = root
                .root_frame_at(self.audio_event_editor.cursor, tempo)
                .ok()?
                .saturating_sub(first);
            Some((
                self.event_viewport(&root),
                self.event_navigation_limit(&root),
                cursor,
            ))
        }
    }
    fn set_audio_navigation(&mut self, viewport: Viewport) {
        if self.audio_event_editor.source_view {
            self.audio_event_editor.source_viewport = Some(viewport);
        } else {
            self.audio_event_editor.viewport = Some(viewport);
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
        if let Some((viewport, limit, cursor)) = self.audio_navigation() {
            self.set_audio_navigation(viewport.zoom(limit, cursor, inward));
        }
    }
    pub(in crate::editor) fn event_pan(&mut self, right: bool) {
        if !self.audio_navigation_idle() {
            return;
        }
        if let Some((viewport, limit, _)) = self.audio_navigation() {
            self.set_audio_navigation(viewport.pan_half(limit, right));
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
            self.audio_event_editor.source_viewport =
                prior.map(|v| v.clamp(self.source_navigation_limit()));
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
        let Some((mut viewport, limit, _)) = self.audio_navigation() else {
            return false;
        };
        let fraction =
            ((f32::from(event.position.x) - f32::from(bounds.left())) / width).clamp(0.0, 1.0);
        let anchor = viewport
            .start
            .saturating_add((viewport.length as f64 * f64::from(fraction)) as u64);
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
            viewport = viewport.zoom_at(limit, anchor, inward);
            *wheel += if inward { 80.0 } else { -80.0 };
        }
        viewport = viewport.pan_by_frames(
            limit,
            (-f64::from(dx).clamp(-f64::from(width), f64::from(width)) * viewport.length as f64
                / f64::from(width))
            .round() as i128,
        );
        self.set_audio_navigation(viewport);
        true
    }
}

// Same finite envelope as the piano roll, with an explicitly converted minimum.
fn navigation_limit(content: u64, minimum: u64) -> u64 {
    content.saturating_mul(4).max(minimum)
}

#[cfg(test)]
mod tests {
    #[test]
    fn navigation_envelope_handles_short_and_extreme_content() {
        assert_eq!(super::navigation_limit(10, 100), 100);
        assert_eq!(super::navigation_limit(100, 10), 400);
        assert_eq!(super::navigation_limit(u64::MAX, 1), u64::MAX);
    }
}
