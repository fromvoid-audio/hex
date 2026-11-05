//! Display windows have signed origins; only their visible intersections reach
//! unsigned source reads. Root and project frame clocks remain explicit here.
use super::*;

impl EditorView {
    pub(in crate::editor) fn event_frame_window(
        &self,
        root: &AudioClipEdit,
    ) -> SignedFrameViewport {
        let Some(state) = &self.state else {
            return Viewport::fit(0).into();
        };
        let tempo = state.project.tempo_map();
        let first = root
            .root_frame_at(editor_bounds(root).0, tempo)
            .unwrap_or(0);
        let end = root
            .root_frame_at(editor_bounds(root).1, tempo)
            .unwrap_or(first);
        let view = self.audio_event_editor.viewport.unwrap_or(Viewport {
            start: 0,
            length: end.saturating_sub(first),
        });
        SignedFrameViewport {
            start: i128::from(view.start)
                - i128::from(if self.audio_event_editor.viewport.is_some() {
                    self.audio_event_editor.negative_pan
                } else {
                    0
                }),
            length: view.length,
        }
        .bounded(
            -i128::from(u64::MAX),
            i128::from(self.event_navigation_limit(root)),
        )
        .unwrap_or(SignedFrameViewport {
            start: 0,
            length: 0,
        })
    }
    // Compatibility query for existing positive-camera callers. Geometry uses
    // the complete signed window, never this positive half on its own.
    pub(in crate::editor) fn event_viewport(&self, root: &AudioClipEdit) -> Viewport {
        let window = self.event_frame_window(root);
        Viewport {
            start: u64::try_from(window.start.max(0)).unwrap_or(u64::MAX),
            length: window.length,
        }
    }
    pub(in crate::editor) fn event_root_window(
        &self,
        root: &AudioClipEdit,
    ) -> Option<SignedFrameViewport> {
        let first = root
            .root_frame_at(
                editor_bounds(root).0,
                self.state.as_ref()?.project.tempo_map(),
            )
            .ok()?;
        self.event_frame_window(root).shifted(i128::from(first))
    }
    pub(super) fn event_project_window(&self, root: &AudioClipEdit) -> Option<SignedFrameViewport> {
        let clip = self.state.as_ref()?.project.clip(self.detail_clip_id()?)?;
        self.event_frame_window(root)
            .shifted(i128::from(clip.start()))
    }
    pub(super) fn event_pointer_position(&self, x: f32, platform: bool) -> Option<u64> {
        let (_, root, _) = self.event_context()?;
        let state = self.state.as_ref()?;
        let window = self.event_project_window(&root)?;
        let (frame, width) = self.event_pointer_frame(window, x)?;
        let snapped = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform)
            .snap_signed_frame(&state.project, frame, window, width)?;
        self.event_position_from_signed_project_frame(&root, snapped)
    }
    /// Selection can clamp to the content edge. Hit testing must leave blank
    /// space blank so an invisible event at the first frame cannot be grabbed.
    pub(super) fn event_hit_position(&self, root: &AudioClipEdit, x: f32) -> Option<u64> {
        let tempo = self.state.as_ref()?.project.tempo_map();
        let (first, end) = content_bounds(root);
        let first = i128::from(root.root_frame_at(first, tempo).ok()?);
        let end = i128::from(root.root_frame_at(end, tempo).ok()?);
        let (frame, _) = self.event_pointer_frame(self.event_root_window(root)?, x)?;
        if !(first..end).contains(&frame) {
            return None;
        }
        root.root_position_at(u64::try_from(frame).ok()?, tempo)
            .ok()
    }

    fn event_pointer_frame(&self, window: SignedFrameViewport, x: f32) -> Option<(i128, f32)> {
        let bounds = self.audio_event_editor.bounds?;
        let width = f32::from(bounds.size.width);
        if !x.is_finite() || !width.is_finite() || width <= 0.0 {
            return None;
        }
        let fraction = ((x - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
        let frame = window
            .start
            .checked_add((f64::from(fraction) * window.length as f64).round() as i128)?;
        Some((frame, width))
    }

    pub(super) fn event_position_from_signed_project_frame(
        &self,
        root: &AudioClipEdit,
        frame: i128,
    ) -> Option<u64> {
        let state = self.state.as_ref()?;
        let tempo = state.project.tempo_map();
        let clip = state.project.clip(self.detail_clip_id()?)?;
        let origin = root.root_frame_at(editor_bounds(root).0, tempo).ok()?;
        let (a, b) = content_bounds(root);
        let first = root.root_frame_at(a, tempo).ok()?;
        let end = root.root_frame_at(b, tempo).ok()?;
        let root_frame = frame
            .checked_sub(i128::from(clip.start()))?
            .checked_add(i128::from(origin))?
            .clamp(i128::from(first), i128::from(end));
        root.root_position_at(u64::try_from(root_frame).ok()?, tempo)
            .ok()
    }
    pub(super) fn event_snap_position(&self, position: u64, platform: bool) -> Option<u64> {
        let (_, root, _) = self.event_context()?;
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.detail_clip_id()?)?;
        let tempo = state.project.tempo_map();
        let first = root.root_frame_at(editor_bounds(&root).0, tempo).ok()?;
        let root_frame = root.root_frame_at(position, tempo).ok()?;
        let project_frame = i128::from(clip.start()) + i128::from(root_frame) - i128::from(first);
        let snapped = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform)
            .snap_signed_frame(
                &state.project,
                project_frame,
                self.event_project_window(&root)?,
                f32::from(self.audio_event_editor.bounds?.size.width),
            )?;
        self.event_position_from_signed_project_frame(&root, snapped)
    }
    pub(super) fn store_event_camera(&mut self, window: SignedFrameViewport) -> bool {
        let Some((_, root, _)) = self.event_context() else {
            return false;
        };
        let Some(window) = window.bounded(
            -i128::from(u64::MAX),
            i128::from(self.event_navigation_limit(&root)),
        ) else {
            return false;
        };
        let (Ok(start), Ok(negative)) = (
            u64::try_from(window.start.max(0)),
            u64::try_from((-window.start).max(0)),
        ) else {
            return false;
        };
        self.audio_event_editor.viewport = Some(Viewport {
            start,
            length: window.length,
        });
        self.audio_event_editor.negative_pan = negative;
        true
    }
}
