//! Derived onset hints from the exact resident waveform asset. Scheduling and
//! cancellation live here. The saved warp map and event timing stay canonical.
use super::EditorView;
use gpui::Context;
use hex_project::{AudioClipEdit, AudioDomainClock, AudioEvent, TempoMap};
use hex_session::CachedWaveform;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Analysis {
    source: Weak<CachedWaveform>,
    cancelled: Arc<AtomicBool>,
    frames: Option<Vec<u64>>,
}

impl Analysis {
    pub(super) fn frames(&self) -> &[u64] {
        self.frames.as_deref().unwrap_or_default()
    }
}

impl Drop for Analysis {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl EditorView {
    pub(super) fn sync_audio_event_transients(
        &mut self,
        source: Option<Arc<CachedWaveform>>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = source else {
            self.audio_detail.event.transients = None;
            return;
        };
        let identity = Arc::downgrade(&source);
        if self
            .audio_detail
            .event
            .transients
            .as_ref()
            .is_some_and(|entry| entry.source.ptr_eq(&identity))
        {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        self.audio_detail.event.transients = Some(Analysis {
            source: identity,
            cancelled: Arc::clone(&cancelled),
            frames: None,
        });
        let worker_cancel = Arc::clone(&cancelled);
        let task = cx.background_executor().spawn(async move {
            source.detect_source_transients(&mut || worker_cancel.load(Ordering::Acquire))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                let Some(entry) = view
                    .audio_detail
                    .event
                    .transients
                    .as_mut()
                    .filter(|entry| Arc::ptr_eq(&entry.cancelled, &cancelled))
                else {
                    return;
                };
                match result {
                    Ok(frames) => entry.frames = frames,
                    Err(error) => view.status = format!("could not detect transients: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

pub(super) fn supports_root(root: &AudioClipEdit) -> bool {
    root.content_view.is_none()
        && !root.domain.looping
        && root.domain.source_phase == 0
        && root.domain.pitch_cents == 0
        && root
            .domain
            .events
            .as_ref()
            .is_none_or(|events| events.sources.is_empty())
}

/// Absolute asset frame to an occurrence in the event's destination clock.
pub(super) fn event_position(
    root: &AudioClipEdit,
    event: AudioEvent,
    asset_frame: u64,
    tempo: &TempoMap,
) -> Option<u64> {
    let offset = asset_frame.checked_sub(root.domain.source_offset)?;
    if offset >= root.domain.source_length || event.source != 0 {
        return None;
    }
    let source = match root.domain.clock {
        AudioDomainClock::Frames(_) => offset,
        AudioDomainClock::Beats(_) if root.domain.warp_enabled => root
            .domain
            .warp
            .as_ref()?
            .beat_for_source_frame(asset_frame)
            .ok()?
            .raw(),
        AudioDomainClock::Beats(_) => root.event_source_position_at(0, offset, tempo).ok()?,
    };
    event
        .start
        .checked_add(source.checked_sub(event.source_start)?)
        .filter(|position| *position < event.end)
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::super::rendered_test_fixture::{audio_project, editor_window};
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, px, size};
    use hex_session::service::ViewState;

    #[gpui::test]
    fn resident_attack_is_projected_onto_the_event_without_authoring_a_marker(
        cx: &mut TestAppContext,
    ) {
        let (project, track, _, clip) = audio_project();
        let asset = project.clip(clip).unwrap().asset();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1280.0), px(800.0)));
        let mut frames = vec![[0.0; 2]; 96_000];
        frames[24_000] = [0.7, -0.7];
        let waveform =
            hex_media::waveform::summarize(&hex_engine::Audio::new(48_000, frames).unwrap(), 256)
                .unwrap();
        let before = editor.update(&mut visual, |view, cx| {
            let state = view.state.as_ref().unwrap();
            let before = state.project.snapshot();
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: Arc::clone(&state.project),
                waveforms: [(asset, Arc::new(waveform))].into(),
                source_overviews: Default::default(),
                audio_clip_waveforms: Default::default(),
                midi_preview: Arc::clone(&state.midi_preview),
                gain_preview: state.gain_preview.clone(),
                timeline_frames: state.timeline_frames,
                can_undo: false,
                can_redo: false,
                revision: state.revision,
                unsaved: false,
                document_path: None,
            }));
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.audio_detail.clip_view_open = true;
            view.device_panel.active = false;
            cx.notify();
            before
        });
        visual.run_until_parked();
        let event = visual.debug_bounds("audio-event-1").expect("event");
        let marker = visual
            .debug_bounds("audio-event-transient")
            .expect("attack");
        assert!((f32::from(marker.left() - event.left() - event.size.width * 0.25)).abs() < 2.0);
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(state.project.snapshot(), before);
            assert!(!state.can_undo);
        });
    }
}
