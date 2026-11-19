//! Frame-fixed placement uses the model's whole-clip preview; the UI owns only
//! pointer geometry. Signed display space reaches either side of the clip;
//! the captured canonical bounds still limit actual retained-root growth.
use super::*;
use crate::editor::{
    waveform,
    waveform_geometry::{WaveformMapping, WaveformViewCache},
};
use hex_project::{AudioDomainClock, AudioEventPlacementPreview, AudioEvents, Clip, TempoMap};
use hex_session::{CachedWaveform, service::ViewState};
use std::{rc::Rc, sync::Arc};

/// The accepted peak source is frozen with the model query. An unrelated actor
/// event can install new summaries while this edit still awaits its own receipt.
/// Never interpret those new summaries through the old candidate's source map.
pub(super) struct Preview {
    model: AudioEventPlacementPreview,
    source: Option<Source>,
    origin_shift: i64,
    frame_shift: i128,
    musical: Option<Arc<MusicalWaveformClock>>,
}
struct MusicalWaveformClock {
    tempo: TempoMap,
    original: AudioClipEdit,
    destination: AudioClipEdit,
}

/// A frozen source map for one event. Cache identity includes the exact held
/// preview and event ID, so an unrelated accepted revision cannot retarget it.
#[derive(Clone)]
pub(in crate::editor) struct MusicalEventWaveform {
    clock: Arc<MusicalWaveformClock>,
    event: u64,
    start: u64,
    end: u64,
    original_start: u64,
    gain: hex_project::ClipGain,
}

impl std::fmt::Debug for MusicalEventWaveform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("MusicalEventWaveform")
            .field(&self.event)
            .finish()
    }
}
impl PartialEq for MusicalEventWaveform {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.clock, &other.clock) && self.event == other.event
    }
}
impl Eq for MusicalEventWaveform {}

impl MusicalEventWaveform {
    pub(in crate::editor) fn gain(&self) -> hex_project::ClipGain {
        self.gain
    }

    pub(in crate::editor) fn ranges(
        &self,
        range: std::ops::Range<u64>,
        available: usize,
    ) -> waveform::SourceRanges {
        let mut ranges = waveform::SourceRanges::default();
        let tempo = &self.clock.tempo;
        let position = |frame| {
            let raw = self.clock.destination.root_position_at(frame, tempo).ok()?;
            let offset = raw.clamp(self.start, self.end) - self.start;
            self.clock
                .original
                .root_frame_at(self.original_start.checked_add(offset)?, tempo)
                .ok()
        };
        if let (Some(first), Some(last)) = (position(range.start), position(range.end)) {
            waveform::push_source_range(
                &mut ranges,
                first,
                last.max(first.saturating_add(1)),
                available,
            );
        }
        ranges
    }
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
        let before = original.audio_event_root();
        let after = model.clip().audio_event_root();
        let tempo = state.project.tempo_map();
        let frame_shift = i128::from(after.root_frame_at(after.view_start, tempo).ok()?)
            - i128::from(before.root_frame_at(before.view_start, tempo).ok()?);
        let musical = matches!(before.domain.clock, AudioDomainClock::Beats(_)).then(|| {
            Arc::new(MusicalWaveformClock {
                tempo: tempo.clone(),
                original: before,
                destination: after,
            })
        });
        Some(Self {
            model,
            source,
            origin_shift,
            frame_shift,
            musical,
        })
    }
    pub(super) fn origin_shift(&self) -> i64 {
        self.origin_shift
    }
    pub(super) fn frame_shift(&self) -> i128 {
        self.frame_shift
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
        cache: Rc<WaveformViewCache>,
        ink: u32,
    ) -> Option<gpui::Div> {
        use gpui::prelude::*;
        if let Some(clock) = &self.musical {
            let (start, end_raw) = self.model.waveform_destination(event)?;
            let original_start = self.model.original_waveform_position(event, start)?;
            let Source::Processed(waveform) = self.source.as_ref()? else {
                return None;
            };
            let projection = MusicalEventWaveform {
                clock: Arc::clone(clock),
                event,
                start,
                end: end_raw,
                original_start,
                gain: self.clip().gain(),
            };
            return Some(
                waveform::waveform_canvas(
                    Arc::clone(waveform),
                    WaveformMapping::MusicalEvent(projection),
                    first,
                    end,
                    Some(cache),
                    None,
                    ink,
                )
                .debug_selector(move || format!("audio-event-waveform-{event}")),
            );
        }
        let a = self.model.original_waveform_position(event, first)?;
        let b = self.model.original_waveform_position(event, end)?;
        let element = match self.source.as_ref()? {
            Source::Processed(waveform) => waveform::waveform_source_segment_gain_view_cached(
                Arc::clone(waveform),
                0,
                a..b,
                self.clip().gain().linear(),
                true,
                cache,
                ink,
            ),
            Source::Original {
                waveform,
                clip,
                first,
            } => {
                let a = clip.start().checked_add(a.checked_sub(*first)?)?;
                let b = clip.start().checked_add(b.checked_sub(*first)?)?;
                waveform::waveform_view_cached(waveform, clip, a..b, None, cache, ink)
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
        if matches!(gesture.kind, Kind::Duplicate { .. }) {
            // An invalid final drop cannot admit an earlier valid preview.
            gesture.operation = None;
            gesture.placement = None;
        }
        let (origin, limits) = match gesture.kind {
            Kind::FreeMove { event, limits }
            | Kind::Duplicate {
                event,
                limits: Some(limits),
            } => (event.start, limits),
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
        let Some(tempo) = self.state.as_ref().map(|state| state.project.tempo_map()) else {
            return;
        };
        let Ok(origin_frame) = root.signed_root_frame_at(i128::from(origin), tempo) else {
            return;
        };
        let (Ok(minimum), Ok(maximum)) = (
            root.signed_root_frame_at(i128::from(first), tempo),
            root.signed_root_frame_at(i128::from(last), tempo),
        ) else {
            return;
        };
        let raw_frame = origin_frame.saturating_add(delta).clamp(minimum, maximum);
        let frame = self
            .free_event_snap(&root, raw_frame, platform)
            .unwrap_or(raw_frame);
        let Ok(start) = root.signed_root_position_at(frame.clamp(minimum, maximum), tempo) else {
            return;
        };
        let start = start.clamp(i128::from(first), i128::from(last)) as i64;
        let operation = match gesture.kind {
            Kind::Duplicate { event, .. } => AudioEventEdit::Duplicate {
                event: event.id,
                start,
            },
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
    fn free_event_snap(&self, root: &AudioClipEdit, raw: i128, platform: bool) -> Option<i128> {
        let state = self.state.as_ref()?;
        let clip = state.project.clip(self.detail_clip_id()?)?;
        let first = root
            .root_frame_at(event_grid_origin(root), state.project.tempo_map())
            .ok()?;
        let frame = i128::from(clip.start()) + raw - i128::from(first);
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
        Some(snapped - i128::from(clip.start()) + i128::from(first))
    }
    pub(super) fn placement_preview(&self, operation: &AudioEventEdit) -> Option<Arc<Preview>> {
        if !matches!(
            operation,
            AudioEventEdit::Place { .. }
                | AudioEventEdit::PlaceRange { .. }
                | AudioEventEdit::Duplicate { .. }
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
