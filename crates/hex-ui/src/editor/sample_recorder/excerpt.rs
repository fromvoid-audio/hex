//! Prepare the selected take excerpt off the UI thread. Only a completed
//! excerpt can be offered to Hex's existing audio-file drag receivers.
//!
//! # Bounded excerpt custody contract (`#8122` / `#8121`)
//! - **Selection changes are read-only**: changing the take selection range
//!   never creates or writes excerpt files on disk (`take_audio::preview_excerpt`
//!   reads the retained take in memory), so repeated selection changes remain
//!   strictly bounded with zero file accumulation.
//! - **Explicit on-demand materialization**: an immutable excerpt WAV is written
//!   only when the user explicitly clicks `"prepare drag"`, reusing any matching
//!   `(range, rate)` already prepared.
//! - **No blind pathname unlink on drop/cancel/supersede**: `Ready::drop` NEVER
//!   calls `std::fs::remove_file` on an unoffered excerpt pathname, because a
//!   non-cooperating same-user process may have replaced that pathname (and
//!   POSIX/macOS provides no kernel-atomic check-then-unlink). Unoffered or
//!   superseded excerpt paths (and any `retained_stage` returned by non-renaming
//!   WAV publication) remain untouched on disk for deliberate user inspection
//!   and are tracked in a bounded retention ring (`MAX_RETAINED_EXCERPT_PATHS`).
//! - **Post-publication waveform failure**: handled in `take_audio::prepare_excerpt`,
//!   which preserves both the published excerpt path and any `retained_stage`
//!   and reports them verbatim.

use super::{EditorView, button, selection::Range, waveform::take_source};
use crate::editor::{
    browser::BrowserAudioDragGhost,
    drag::{BrowserAudioDrag, copy_cursor},
};
use gpui::{Context, Div, Stateful, prelude::*};
use hex_session::service::take_audio::{self, PreparedTakeExcerpt};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_RETAINED_EXCERPT_PATHS: usize = 16;

/// Bounded in-memory log of unoffered excerpt / retained-stage paths preserved
/// on disk rather than blindly unlinked during drop, supersede, or shutdown.
#[derive(Default)]
pub(super) struct RetainedExcerptPaths {
    paths: RefCell<Vec<PathBuf>>,
}

impl RetainedExcerptPaths {
    fn record(&self, path: PathBuf) {
        let mut paths = self.paths.borrow_mut();
        if paths.contains(&path) {
            return;
        }
        paths.push(path);
        if paths.len() > MAX_RETAINED_EXCERPT_PATHS {
            let excess = paths.len() - MAX_RETAINED_EXCERPT_PATHS;
            paths.drain(..excess);
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> Vec<PathBuf> {
        self.paths.borrow().clone()
    }
}

#[derive(Default)]
pub(super) struct ExcerptState {
    ready: Option<Rc<Ready>>,
    pending: Option<Pending>,
    retained: Rc<RetainedExcerptPaths>,
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
    retained: Rc<RetainedExcerptPaths>,
    offered: Cell<bool>,
}
impl Drop for Ready {
    fn drop(&mut self) {
        // Never blindly unlink `self.excerpt.path` by pathname: a competing
        // process may have replaced the pathname with unrelated user data, and
        // check-then-unlink is not atomic. Preserve unoffered excerpts on disk
        // and record their paths in the bounded retention log for inspection.
        if !self.offered.get() {
            self.retained.record(self.excerpt.path.clone());
        }
        if let Some(stage) = &self.excerpt.retained_stage {
            self.retained.record(stage.clone());
        }
    }
}

impl EditorView {
    pub(super) fn prepare_take_excerpt(&mut self, cx: &mut Context<Self>) {
        let Some(rate) = self.state.as_ref().map(|state| state.project.sample_rate()) else {
            return;
        };
        let Some(wave) = self.device_panel.sample_recorder.waveform.as_mut() else {
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
        self.device_panel.sample_recorder.error = None;
        let take = wave.take.clone();
        let retained = Rc::clone(&wave.excerpt.retained);
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
                        retained: Rc::clone(&retained),
                        offered: Cell::new(false),
                    })
                });
            let _ = entity.update(cx, |view, cx| {
                let Some(wave) = view.device_panel.sample_recorder.waveform.as_mut() else {
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
                    Ok(ready) => {
                        if let Some(stage) = ready.excerpt.retained_stage.as_ref() {
                            view.device_panel.sample_recorder.error = Some(format!(
                                "staging path {} was not removed · inspect it before removing anything",
                                hex_session::service::text::verbatim(stage.display())
                            ));
                        }
                        wave.excerpt.ready = Some(ready);
                    }
                    Err(error) => view.device_panel.sample_recorder.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn take_excerpt_grip(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let wave = self.device_panel.sample_recorder.waveform.as_ref()?;
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
                None,
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
            .on_drag(payload, move |payload, _, window, cx| {
                let pointer = window.mouse_position();
                payload.snap_modifier.set(window.modifiers().platform);
                payload.copy_modifier.set(window.modifiers().alt);
                let active = entity.update(cx, |view, cx| {
                    let current = view
                        .device_panel
                        .sample_recorder
                        .waveform
                        .as_ref()
                        .is_some_and(|wave| {
                            wave.range == ready.range
                                && wave
                                    .excerpt
                                    .ready
                                    .as_ref()
                                    .is_some_and(|value| Rc::ptr_eq(value, &ready))
                        });
                    if !current || view.selecting() || view.persistence.opening_project {
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

fn move_payload(view: &mut EditorView, payload: &BrowserAudioDrag, x: f32, y: f32) {
    payload.last_pointer.set(Some((x, y)));
    let (preview, hovered) = view.browser_audio_drop_position(payload, x, y);
    payload.preview.set(preview);
    payload.lane_hovered.set(hovered);
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_media::waveform::Peak;
    use hex_session::CachedWaveform;
    use std::fs;
    use std::sync::atomic::AtomicU64;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn dummy_preview() -> hex_session::service::AudioPreview {
        hex_session::service::AudioPreview {
            playback_frames: 4,
            sample_rate: 48_000,
            waveform: Arc::new(
                CachedWaveform::from_peaks(4, vec![[Peak { min: 0.0, max: 0.0 }; 2]]).unwrap(),
            ),
        }
    }

    #[test]
    fn dropping_unoffered_excerpt_preserves_substituted_foreign_bytes_and_records_path() {
        let dir = std::env::temp_dir().join(format!(
            "hex-excerpt-custody-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let excerpt_path = dir.join("take-excerpt.wav");
        let stage_path = dir.join(".hex-export-1-0.tmp");
        fs::write(&excerpt_path, b"original-excerpt-wav").unwrap();
        fs::write(&stage_path, b"retained-stage-wav").unwrap();

        let retained = Rc::new(RetainedExcerptPaths::default());
        let ready = Ready {
            excerpt: PreparedTakeExcerpt {
                path: excerpt_path.clone(),
                preview: dummy_preview(),
                retained_stage: Some(stage_path.clone()),
            },
            range: Range { start: 0, end: 4 },
            retained: Rc::clone(&retained),
            offered: Cell::new(false),
        };

        // Adversarial same-user substitution before unoffered `Ready::drop`.
        let foreign = dir.join("foreign.wav");
        fs::write(&foreign, b"unrelated-foreign-user-data").unwrap();
        fs::rename(&foreign, &excerpt_path).unwrap();

        drop(ready);

        assert_eq!(
            fs::read(&excerpt_path).expect("substituted foreign bytes must survive Ready::drop"),
            b"unrelated-foreign-user-data"
        );
        assert_eq!(
            fs::read(&stage_path).expect("retained stage must survive Ready::drop"),
            b"retained-stage-wav"
        );
        let recorded = retained.snapshot();
        assert_eq!(recorded, vec![excerpt_path, stage_path]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn offered_excerpt_is_not_recorded_as_unoffered_and_retention_list_is_bounded() {
        let retained = Rc::new(RetainedExcerptPaths::default());
        let offered_ready = Ready {
            excerpt: PreparedTakeExcerpt {
                path: PathBuf::from("/tmp/offered.wav"),
                preview: dummy_preview(),
                retained_stage: None,
            },
            range: Range { start: 0, end: 4 },
            retained: Rc::clone(&retained),
            offered: Cell::new(true),
        };
        drop(offered_ready);
        assert!(retained.snapshot().is_empty());

        for idx in 0..(MAX_RETAINED_EXCERPT_PATHS + 8) {
            let ready = Ready {
                excerpt: PreparedTakeExcerpt {
                    path: PathBuf::from(format!("/tmp/unoffered-{idx}.wav")),
                    preview: dummy_preview(),
                    retained_stage: None,
                },
                range: Range { start: 0, end: 4 },
                retained: Rc::clone(&retained),
                offered: Cell::new(false),
            };
            drop(ready);
        }
        let snapshot = retained.snapshot();
        assert_eq!(snapshot.len(), MAX_RETAINED_EXCERPT_PATHS);
        assert_eq!(
            snapshot.last().unwrap(),
            &PathBuf::from(format!(
                "/tmp/unoffered-{}.wav",
                MAX_RETAINED_EXCERPT_PATHS + 7
            ))
        );
    }
}
