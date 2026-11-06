//! Prepare the selected take excerpt off the UI thread. Only a completed
//! excerpt can be offered to Hex's existing audio-file drag receivers.

use super::{EditorView, button, selection::Range, waveform::take_source};
use crate::editor::{
    browser::BrowserAudioDragGhost,
    drag::{BrowserAudioDrag, copy_cursor},
};
use gpui::{BackgroundExecutor, Context, Div, Stateful, prelude::*};
use hex_session::service::take_audio::{self, PreparedTakeExcerpt};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
pub(super) struct ExcerptState {
    ready: Option<Rc<Ready>>,
    pending: Option<Pending>,
}
struct Pending {
    range: Range,
    rate: u32,
    cancel: Arc<AtomicBool>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Ready {
    excerpt: PreparedTakeExcerpt,
    range: Range,
    executor: BackgroundExecutor,
    offered: Cell<bool>,
}
impl Drop for Ready {
    fn drop(&mut self) {
        // Once offered, ordinary media import may retain the path in the
        // document, history or recovery state. Only unoffered files are ours.
        if !self.offered.get() {
            let path = self.excerpt.path.clone();
            self.executor
                .spawn(async move {
                    let _ = std::fs::remove_file(path);
                })
                .detach();
        }
    }
}

impl EditorView {
    pub(super) fn prepare_take_excerpt(&mut self, cx: &mut Context<Self>) {
        let Some(rate) = self.state.as_ref().map(|state| state.project.sample_rate()) else {
            return;
        };
        let Some(wave) = self.sample_recorder.waveform.as_mut() else {
            return;
        };
        let range = wave.range;
        if wave
            .excerpt
            .ready
            .as_ref()
            .is_some_and(|ready| ready.range == range && ready.excerpt.preview.sample_rate == rate)
            || wave
                .excerpt
                .pending
                .as_ref()
                .is_some_and(|pending| pending.range == range && pending.rate == rate)
        {
            return;
        }
        wave.excerpt.ready = None;
        let cancel = Arc::new(AtomicBool::new(false));
        wave.excerpt.pending = Some(Pending {
            range,
            rate,
            cancel: cancel.clone(),
        });
        self.sample_recorder.error = None;
        let take = wave.take.clone();
        let entity = cx.entity().downgrade();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |_, cx| {
            let worker_cancel = cancel.clone();
            let result = executor
                .spawn(async move {
                    take_audio::prepare_excerpt(
                        take_source(&take),
                        range.start..range.end,
                        rate,
                        &worker_cancel,
                    )
                })
                .await
                .map(|excerpt| {
                    Rc::new(Ready {
                        excerpt,
                        range,
                        executor,
                        offered: Cell::new(false),
                    })
                });
            let _ = entity.update(cx, |view, cx| {
                let Some(wave) = view.sample_recorder.waveform.as_mut() else {
                    return;
                };
                if !wave
                    .excerpt
                    .pending
                    .as_ref()
                    .is_some_and(|p| Arc::ptr_eq(&p.cancel, &cancel))
                {
                    return;
                }
                wave.excerpt.pending = None;
                if wave.range != range
                    || view
                        .state
                        .as_ref()
                        .is_none_or(|state| state.project.sample_rate() != rate)
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(ready) => wave.excerpt.ready = Some(ready),
                    Err(error) => view.sample_recorder.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn take_excerpt_grip(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let wave = self.sample_recorder.waveform.as_ref()?;
        if self.selecting() {
            return None;
        }
        let ready = wave
            .excerpt
            .ready
            .as_ref()
            .filter(|ready| ready.range == wave.range)
            .cloned();
        let payload = ready.as_ref().and_then(|ready| {
            let payload = self.audio_file_drag_payload(
                ready.excerpt.path.clone(),
                wave.name.clone(),
                Rc::new(RefCell::new(Some(Rc::new(ready.excerpt.preview.clone())))),
                Default::default(),
            )?;
            (payload.sample_rate == ready.excerpt.preview.sample_rate).then_some(payload)
        });
        let (Some(ready), Some(payload)) = (ready, payload) else {
            if wave.excerpt.pending.is_some() {
                return None;
            }
            return Some(
                button(
                    "prepare drag",
                    "prepare the selected range to drag",
                    ("prepare-take-drag", 0),
                )
                .on_click(cx.listener(|view, _, _, cx| view.prepare_take_excerpt(cx))),
            );
        };
        let entity = cx.entity();
        Some(
            button(
                "drag range",
                "drag this exact range to arrangement or session",
                ("take-excerpt-drag", 0),
            )
            .debug_selector(|| "take-excerpt-drag".into())
            .on_drag(payload, move |payload, _, window, cx| {
                let pointer = window.mouse_position();
                payload.snap_modifier.set(window.modifiers().platform);
                payload.copy_modifier.set(window.modifiers().alt);
                let active = entity.update(cx, |view, cx| {
                    let current = view.sample_recorder.waveform.as_ref().is_some_and(|wave| {
                        wave.range == ready.range
                            && wave
                                .excerpt
                                .ready
                                .as_ref()
                                .is_some_and(|value| Rc::ptr_eq(value, &ready))
                    });
                    if !current || view.selecting() || view.opening_project {
                        return false;
                    }
                    ready.offered.set(true);
                    view.stop_take_preview();
                    view.begin_browser_audio_drag(payload);
                    move_payload(view, payload, f32::from(pointer.x), f32::from(pointer.y));
                    cx.notify();
                    true
                });
                if active {
                    cx.set_active_drag_cursor_style(
                        copy_cursor(payload.copy_modifier.get()),
                        window,
                    );
                }
                cx.new(|_| BrowserAudioDragGhost)
            }),
        )
    }
}

fn move_payload(view: &EditorView, payload: &BrowserAudioDrag, x: f32, y: f32) {
    payload.last_pointer.set(Some((x, y)));
    let (preview, hovered) = view.browser_audio_drop_position(payload, x, y);
    payload.preview.set(preview);
    payload.lane_hovered.set(hovered);
}
