//! Explicit recovery offer for a truncated WAV import (Pulse #4136).
//!
//! `hex-session` refuses a single-file import whose WAV data is truncated and
//! reports it as `Event::import_truncation`. The editor keeps an owned copy of
//! each submitted single-file import with an ordinary FIFO
//! [`pending_edit::Receipt`], so the refusal is matched to its exact request,
//! never to a path seen earlier. A matching refusal arms one offer; the next
//! render shows it as a native prompt. Confirming re-enqueues that same import
//! as `Request::RecoverTruncatedImport`; cancelling submits nothing. Project
//! replacement withdraws the offer and every outstanding record.
//!
//! Imports captured as `ImportAudioBatch` (auto-warp enabled) carry no
//! truncation report and therefore never offer recovery here.
use super::{EditorView, pending_edit, theme};
use gpui::{Context, PromptButton, PromptLevel, Window};
use hex_project::{SceneId, TrackId};
use hex_session::service::{Event, ImportTruncation, Request, TruncatedImportRecovery, text};
use std::{collections::VecDeque, path::PathBuf};

/// Matches the bounded Service control FIFO; older records are dropped first.
const OUTSTANDING_CAPACITY: usize = 32;

/// Owned copy of one submitted single-file import; `Request` itself is not `Clone`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ImportIntent {
    Import(PathBuf),
    ToTrack {
        path: PathBuf,
        track: TrackId,
        start: u64,
        overwrite: bool,
    },
    ToNewTrack {
        path: PathBuf,
        before_track: Option<TrackId>,
        start: u64,
    },
    ToNewTrackWithGain {
        path: PathBuf,
        before_track: Option<TrackId>,
        start: u64,
        gain_bits: u32,
    },
    ToSession {
        path: PathBuf,
        track: TrackId,
        scene: SceneId,
    },
}

impl ImportIntent {
    pub(super) fn capture(request: &Request) -> Option<Self> {
        Some(match request {
            Request::Import(path) => Self::Import(path.clone()),
            Request::ImportToTrack { path, track, start } => Self::ToTrack {
                path: path.clone(),
                track: *track,
                start: *start,
                overwrite: false,
            },
            Request::ImportToTrackWithOverwrite { path, track, start } => Self::ToTrack {
                path: path.clone(),
                track: *track,
                start: *start,
                overwrite: true,
            },
            Request::ImportToNewTrack {
                path,
                before_track,
                start,
            } => Self::ToNewTrack {
                path: path.clone(),
                before_track: *before_track,
                start: *start,
            },
            Request::ImportToNewTrackWithGain {
                path,
                before_track,
                start,
                gain,
            } => Self::ToNewTrackWithGain {
                path: path.clone(),
                before_track: *before_track,
                start: *start,
                gain_bits: gain.to_bits(),
            },
            Request::ImportToSession { path, track, scene } => Self::ToSession {
                path: path.clone(),
                track: *track,
                scene: *scene,
            },
            _ => return None,
        })
    }

    fn path(&self) -> &std::path::Path {
        match self {
            Self::Import(path)
            | Self::ToTrack { path, .. }
            | Self::ToNewTrack { path, .. }
            | Self::ToNewTrackWithGain { path, .. }
            | Self::ToSession { path, .. } => path,
        }
    }

    fn into_request(self) -> Request {
        match self {
            Self::Import(path) => Request::Import(path),
            Self::ToTrack {
                path,
                track,
                start,
                overwrite: false,
            } => Request::ImportToTrack { path, track, start },
            Self::ToTrack {
                path,
                track,
                start,
                overwrite: true,
            } => Request::ImportToTrackWithOverwrite { path, track, start },
            Self::ToNewTrack {
                path,
                before_track,
                start,
            } => Request::ImportToNewTrack {
                path,
                before_track,
                start,
            },
            Self::ToNewTrackWithGain {
                path,
                before_track,
                start,
                gain_bits,
            } => Request::ImportToNewTrackWithGain {
                path,
                before_track,
                start,
                gain: f32::from_bits(gain_bits),
            },
            Self::ToSession { path, track, scene } => {
                Request::ImportToSession { path, track, scene }
            }
        }
    }
}

#[derive(Debug)]
struct Outstanding {
    intent: ImportIntent,
    receipt: pending_edit::Receipt,
}

/// One refused import awaiting the user's explicit choice.
#[derive(Debug)]
pub(super) struct Offer {
    pub(super) intent: ImportIntent,
    pub(super) truncation: ImportTruncation,
    generation: u64,
}

#[derive(Debug, Default)]
pub(super) struct State {
    outstanding: VecDeque<Outstanding>,
    pub(super) offer: Option<Offer>,
    prompting: bool,
}

impl State {
    /// Record a single-file import immediately after its successful submission.
    pub(super) fn submitted(&mut self, intent: ImportIntent, generation: u64, pending: usize) {
        let mut receipt = pending_edit::Receipt::new(generation);
        if !receipt.submitted(pending) {
            return;
        }
        if self.outstanding.len() == OUTSTANDING_CAPACITY {
            self.outstanding.pop_front();
        }
        self.outstanding.push_back(Outstanding { intent, receipt });
    }

    /// Observe one worker event before its canonical state is installed.
    pub(super) fn observe(&mut self, generation: u64, event: &Event) {
        if event.project_replaced
            || self
                .offer
                .as_ref()
                .is_some_and(|offer| offer.generation != generation)
        {
            self.offer = None;
        }
        let mut answered = None;
        self.outstanding.retain_mut(|outstanding| {
            match outstanding.receipt.observe(
                generation,
                event.project_replaced,
                event.request_complete,
            ) {
                pending_edit::Disposition::Waiting => true,
                pending_edit::Disposition::Invalidated => false,
                pending_edit::Disposition::Terminal => {
                    answered = Some(outstanding.intent.clone());
                    false
                }
            }
        });
        if let Some(intent) = answered
            && let Some(truncation) = &event.import_truncation
            && truncation.path == intent.path()
        {
            self.offer = Some(Offer {
                intent,
                truncation: truncation.clone(),
                generation,
            });
        }
    }
}

impl EditorView {
    pub(super) fn truncated_import_submitted(&mut self, intent: Option<ImportIntent>) {
        if let Some(intent) = intent {
            self.truncated_import.submitted(
                intent,
                self.project_generation,
                self.worker_requests_pending,
            );
        }
    }

    pub(super) fn reconcile_truncated_import_offer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.truncated_import.prompting
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
        {
            return;
        }
        let Some(offer) = self.truncated_import.offer.take() else {
            return;
        };
        if offer.generation != self.project_generation {
            return;
        }
        self.truncated_import.prompting = true;
        cx.defer_in(window, move |view, window, cx| {
            view.offer_truncated_import_recovery(offer, window, cx);
        });
    }

    fn offer_truncated_import_recovery(
        &mut self,
        offer: Offer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = theme::text("recover readable frames?");
        let detail = theme::text(recovery_detail(&offer.truncation));
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &[
                PromptButton::new(theme::text("recover readable frames")),
                PromptButton::cancel(theme::text("cancel")),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let recover = answer.await.ok() == Some(0);
            let _ = this.update(cx, |view, cx| {
                view.truncated_import.prompting = false;
                view.answer_truncated_import(offer, recover);
                cx.notify();
            });
        })
        .detach();
    }

    fn answer_truncated_import(&mut self, offer: Offer, recover: bool) {
        if !recover {
            self.status = "truncated recording not imported".into();
            return;
        }
        if offer.generation != self.project_generation {
            self.error = Some("project changed · truncated recording not recovered".into());
            return;
        }
        match TruncatedImportRecovery::new(offer.intent.into_request()) {
            Ok(recovery) => {
                self.enqueue(Request::RecoverTruncatedImport(recovery));
            }
            Err(_) => {
                self.error = Some("this import cannot be recovered".into());
            }
        }
    }
}

fn recovery_detail(truncation: &ImportTruncation) -> String {
    let name = truncation
        .path
        .file_name()
        .unwrap_or(truncation.path.as_os_str())
        .to_string_lossy();
    let mut discarded = format!("{} missing bytes", truncation.missing_bytes);
    if truncation.partial_frame_bytes > 0 {
        discarded.push_str(&format!(
            " and {} partial-frame bytes",
            truncation.partial_frame_bytes
        ));
    }
    format!(
        "{} ends before its declared audio data. recovery imports its {} readable frames; {discarded} are discarded. the original file is not changed.",
        text::verbatim(name),
        truncation.readable_frames,
    )
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "truncated_import_tests.rs"]
mod tests;
