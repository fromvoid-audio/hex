//! Frame-fixed placement uses the model's whole-clip preview; the UI owns only
//! pointer geometry. The current unsigned camera exposes rightward free moves.
//! A signed camera remains necessary before exposing negative-left placement.
use super::*;
use crate::editor::{waveform, waveform_geometry::WaveformGeometryCache};
use hex_project::{AudioEventPlacementPreview, Clip};
use hex_session::{CachedWaveform, service::ViewState};
use std::{cell::RefCell, rc::Rc, sync::Arc};

/// The accepted peak source is frozen with the model query. An unrelated actor
/// event can install new summaries while this edit still awaits its own receipt.
/// Never interpret those new summaries through the old candidate's source map.
pub(super) struct Preview {
    model: AudioEventPlacementPreview,
    source: Option<Source>,
}
enum Source {
    Processed(Arc<CachedWaveform>),
    Original {
        waveform: Arc<CachedWaveform>,
        clip: Clip,
        first: u64,
    },
}
impl Preview {
    fn capture(state: &ViewState, clip: ClipId, model: AudioEventPlacementPreview) -> Option<Self> {
        let original = state.project.clip(clip)?;
        let source = state
            .audio_clip_waveforms
            .get(&clip)
            .map(|p| Source::Processed(Arc::clone(&p.waveform)))
            .or_else(|| {
                if original.audio_edit().is_some() {
                    return None;
                }
                let waveform = state.waveforms.get(&original.asset())?;
                Some(Source::Original {
                    waveform: Arc::clone(waveform),
                    clip: original.clone(),
                    first: original.audio_event_root().view_start,
                })
            });
        Some(Self { model, source })
    }
    pub(super) fn clip(&self) -> &Clip {
        self.model.clip()
    }
    pub(super) fn waveform(
        &self,
        event: u64,
        first: u64,
        end: u64,
        cache: Rc<RefCell<WaveformGeometryCache>>,
    ) -> Option<gpui::Div> {
        use gpui::prelude::*;
        let a = self.model.original_waveform_frame(event, first)?;
        let b = self.model.original_waveform_frame(event, end)?;
        let element = match self.source.as_ref()? {
            Source::Processed(waveform) => waveform::waveform_source_segment_gain_view_cached(
                Arc::clone(waveform),
                0,
                a,
                b,
                self.clip().gain(),
                true,
                cache,
            ),
            Source::Original {
                waveform,
                clip,
                first,
            } => {
                let a = clip.start().checked_add(a.checked_sub(*first)?)?;
                let b = clip.start().checked_add(b.checked_sub(*first)?)?;
                waveform::waveform_view_cached(waveform, clip, a, b, cache)
            }
        };
        Some(element.debug_selector(move || format!("audio-event-waveform-{event}")))
    }
}

impl EditorView {
    pub(super) fn update_free_event_placement(
        &self,
        gesture: &mut Gesture,
        event: AudioEvent,
        x: f32,
        platform: bool,
    ) {
        let Some((clip_id, root, _)) = self.event_context() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(clip) = state.project.clip(clip_id) else {
            return;
        };
        let Some(bounds) = self.audio_event_editor.bounds else {
            return;
        };
        let width = f64::from(f32::from(bounds.size.width));
        if !x.is_finite() || !width.is_finite() || width <= 0.0 {
            return;
        }
        let Some(last) = clip
            .audio_event_placement_limit()
            .and_then(|limit| limit.checked_sub(event.length()))
        else {
            return;
        };
        let first = editor_bounds(&root).0;
        if first > last {
            return;
        }
        let viewport = self.event_viewport(&root);
        let delta = ((f64::from(x) - f64::from(gesture.origin_x)) * viewport.length as f64 / width)
            .round() as i128;
        let raw =
            (i128::from(event.start) + delta).clamp(i128::from(first), i128::from(last)) as u64;
        let start = self
            .free_event_snap(&root, raw, platform)
            .unwrap_or(raw)
            .clamp(first, last);
        let Ok(start) = i64::try_from(start) else {
            return;
        };
        let Ok(preview) = state
            .project
            .preview_audio_event_placement(clip_id, event.id, start)
        else {
            return;
        };
        gesture.operation = Some(AudioEventEdit::Place {
            event: event.id,
            start,
        });
        gesture.placement = Preview::capture(state, clip_id, preview).map(Arc::new);
    }
    // The admitted frame clock continues beyond its old extent. Do not call
    // root_position_at here: its ordinary read contract clamps to that extent.
    fn free_event_snap(&self, root: &AudioClipEdit, raw: u64, platform: bool) -> Option<u64> {
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.detail_clip_id()?)?;
        let first = root
            .root_frame_at(editor_bounds(root).0, state.project.tempo_map())
            .ok()?;
        let frame = clip.start().checked_add(raw.checked_sub(first)?)?;
        let snapped = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform)
            .snap_frame(
                &state.project,
                frame,
                self.event_project_viewport(root)?,
                f32::from(self.audio_event_editor.bounds?.size.width),
            )?;
        snapped.checked_sub(clip.start())?.checked_add(first)
    }
    pub(super) fn placement_preview(&self, operation: &AudioEventEdit) -> Option<Arc<Preview>> {
        let AudioEventEdit::Place { event, start } = *operation else {
            return None;
        };
        let state = self.state.as_ref()?;
        let clip = self.detail_clip_id()?;
        let model = state
            .project
            .preview_audio_event_placement(clip, event, start)
            .ok()?;
        Preview::capture(state, clip, model).map(Arc::new)
    }
}
