//! Frame-fixed placement uses the model's whole-clip preview; the UI owns only
//! pointer geometry. Signed display space reaches either side of the clip;
//! the captured canonical bounds still limit actual retained-root growth.
use super::*;
use crate::editor::{waveform, waveform_geometry::WaveformGeometryCache};
use hex_project::{AudioEventPlacementPreview, AudioEvents, Clip};
use hex_session::{CachedWaveform, service::ViewState};
use std::{cell::RefCell, rc::Rc, sync::Arc};

/// The accepted peak source is frozen with the model query. An unrelated actor
/// event can install new summaries while this edit still awaits its own receipt.
/// Never interpret those new summaries through the old candidate's source map.
pub(super) struct Preview {
    model: AudioEventPlacementPreview,
    source: Option<Source>,
    origin_shift: i64,
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
            .map(|p| {
                let waveform = p
                    .preview_root
                    .as_ref()
                    .map_or(&p.waveform, |root| &root.waveform);
                Source::Processed(Arc::clone(waveform))
            })
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
        let origin_shift = model
            .clip()
            .audio_event_coordinate_shift_from(original)
            .unwrap_or(0);
        Some(Self {
            model,
            source,
            origin_shift,
        })
    }
    pub(super) fn origin_shift(&self) -> i64 {
        self.origin_shift
    }
    pub(super) fn position(&self, position: u64) -> Option<u64> {
        u64::try_from(i128::from(position) + i128::from(self.origin_shift)).ok()
    }
    pub(super) fn clip(&self) -> &Clip {
        self.model.clip()
    }
    pub(super) fn replacement_spans(
        &self,
        events: &AudioEvents,
        operation: &AudioEventEdit,
    ) -> Vec<(u64, u64)> {
        let Some(operation) = self.rebased_operation(operation) else {
            return Vec::new();
        };
        let Ok(lead) = u64::try_from(self.origin_shift) else {
            return Vec::new();
        };
        let mut events = events.clone();
        for item in &mut events.events {
            let (Some(a), Some(b)) = (item.start.checked_add(lead), item.end.checked_add(lead))
            else {
                return Vec::new();
            };
            item.start = a;
            item.end = b;
        }
        paint::replacement_spans(&events, &operation)
    }
    fn rebased_operation(&self, operation: &AudioEventEdit) -> Option<AudioEventEdit> {
        let placed = |start| u64::try_from(i128::from(start) + i128::from(self.origin_shift)).ok();
        match *operation {
            AudioEventEdit::Place { event, start } => Some(AudioEventEdit::Move {
                event,
                start: placed(start)?,
            }),
            AudioEventEdit::PlaceRange {
                start,
                end,
                destination,
            } => Some(AudioEventEdit::MoveRange {
                start: self.position(start)?,
                end: self.position(end)?,
                destination: placed(destination)?,
            }),
            _ => None,
        }
    }

    pub(super) fn range(&self, operation: &AudioEventEdit) -> Option<(u64, u64)> {
        let AudioEventEdit::MoveRange {
            start,
            end,
            destination,
        } = self.rebased_operation(operation)?
        else {
            return None;
        };
        Some((
            destination,
            destination.checked_add(end.checked_sub(start)?)?,
        ))
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
        x: f32,
        platform: bool,
    ) {
        let (origin, limits) = match gesture.kind {
            Kind::FreeMove { event, limits } => (event.start, limits),
            Kind::FreeRangeMove { start, limits, .. } => (start, limits),
            _ => return,
        };
        let Some((_, root, _)) = self.event_context() else {
            return;
        };
        let Some(bounds) = self.audio_event_editor.bounds else {
            return;
        };
        let width = f64::from(f32::from(bounds.size.width));
        if !x.is_finite() || !width.is_finite() || width <= 0.0 {
            return;
        }
        let (first, last) = limits;
        let viewport = self.event_frame_window(&root);
        let delta = ((f64::from(x) - f64::from(gesture.origin_x)) * viewport.length as f64 / width)
            .round() as i128;
        let raw = i128::from(origin)
            .saturating_add(delta)
            .clamp(i128::from(first), i128::from(last)) as i64;
        let start = self
            .free_event_snap(&root, raw, platform)
            .unwrap_or(raw)
            .clamp(first, last);
        let operation = match gesture.kind {
            Kind::FreeMove { event, .. } => AudioEventEdit::Place {
                event: event.id,
                start,
            },
            Kind::FreeRangeMove {
                start: from, end, ..
            } => AudioEventEdit::PlaceRange {
                start: from,
                end,
                destination: start,
            },
            _ => return,
        };
        if let Some(preview) = self.placement_preview(&operation) {
            gesture.operation = Some(operation);
            gesture.placement = Some(preview);
        }
    }
    // The admitted frame clock continues beyond its old extent. Do not call
    // root_position_at here: its ordinary read contract clamps to that extent.
    fn free_event_snap(&self, root: &AudioClipEdit, raw: i64, platform: bool) -> Option<i64> {
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.detail_clip_id()?)?;
        let first = root
            .root_frame_at(event_grid_origin(root), state.project.tempo_map())
            .ok()?;
        let frame = i128::from(clip.start()) + i128::from(raw) - i128::from(first);
        let snapped = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform)
            .snap_signed_frame(
                &state.project,
                frame,
                self.event_project_window(root)?,
                f32::from(self.audio_event_editor.bounds?.size.width),
            )?;
        i64::try_from(snapped - i128::from(clip.start()) + i128::from(first)).ok()
    }
    pub(super) fn placement_preview(&self, operation: &AudioEventEdit) -> Option<Arc<Preview>> {
        if !matches!(
            operation,
            AudioEventEdit::Place { .. } | AudioEventEdit::PlaceRange { .. }
        ) {
            return None;
        };
        let state = self.state.as_ref()?;
        let clip = self.detail_clip_id()?;
        let model = state
            .project
            .preview_audio_event_edit(clip, operation)
            .ok()?;
        Preview::capture(state, clip, model).map(Arc::new)
    }
}
