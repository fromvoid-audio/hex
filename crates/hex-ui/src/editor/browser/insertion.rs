//! Enter captures one selected Arrangement target before bounded worker preparation.
//! Rapid Enter taps while a browser sample insertion is already in flight queue
//! their captured Arrangement target and submit sequentially once the current
//! insertion completes, instead of failing as busy.
use super::{EditorMode, EditorView};
use crate::editor::pending_edit::{Disposition, Receipt};
use gpui::Context;
use hex_engine::playback::BrowserAuditionTiming;
use hex_session::import_batch::AudioBatchDestination;
use hex_session::import_batch_decode::{AudioBatchContext, AudioBatchControl};
use hex_session::service::Request;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering::Acquire},
};

pub(in crate::editor) const MAX_PENDING_BROWSER_INSERTIONS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::editor) enum PendingBrowserInsertionSource {
    AudioFile(PathBuf),
    ProjectSample(hex_project::AssetId),
}

#[derive(Clone, Debug)]
pub(in crate::editor) struct PendingBrowserInsertion {
    project_generation: u64,
    source: PendingBrowserInsertionSource,
    warp: hex_project::AudioImportWarp,
    track: hex_project::TrackId,
    start: u64,
    audition_timing: Option<BrowserAuditionTiming>,
}

pub(in crate::editor) enum BrowserInsertionFlight {
    AudioFile(Arc<AudioBatchControl>),
    ProjectSample {
        asset: hex_project::AssetId,
        track: hex_project::TrackId,
        start: u64,
        receipt: Receipt,
    },
}

impl BrowserInsertionFlight {
    pub(in crate::editor) fn matches(&self, request: &Request) -> bool {
        match (self, request) {
            (Self::AudioFile(active), Request::ImportAudioBatch { control, .. }) => {
                Arc::ptr_eq(active, control)
            }
            (
                Self::ProjectSample {
                    asset,
                    track,
                    start,
                    receipt,
                },
                Request::InsertProjectSample {
                    asset: requested_asset,
                    destination:
                        AudioBatchDestination::Track {
                            track: requested_track,
                            start: requested_start,
                        },
                    ..
                },
            ) => {
                receipt.is_queued()
                    && asset == requested_asset
                    && track == requested_track
                    && start == requested_start
            }
            _ => false,
        }
    }

    pub(in crate::editor) fn submitted(&mut self, pending: usize) {
        if let Self::ProjectSample { receipt, .. } = self {
            receipt.submitted(pending);
        }
    }

    pub(in crate::editor) fn finished(
        &mut self,
        generation: u64,
        event: &hex_session::service::Event,
    ) -> bool {
        match self {
            Self::AudioFile(active) => {
                event.project_replaced
                    || event
                        .audio_batch_complete
                        .as_ref()
                        .is_some_and(|control| Arc::ptr_eq(active, control))
            }
            Self::ProjectSample { receipt, .. } => {
                receipt.observe(generation, event.project_replaced, event.request_complete)
                    != Disposition::Waiting
            }
        }
    }
}

impl EditorView {
    pub(super) fn insert_browser_project_sample(
        &mut self,
        generation: u64,
        revision: u64,
        asset: hex_project::AssetId,
        cx: &mut Context<Self>,
    ) {
        let result = (|| {
            let state = self
                .state
                .as_ref()
                .ok_or("sample insertion refused · no project")?;
            if generation != self.project_generation || revision != state.revision {
                return Err("sample insertion canceled · project changed".into());
            }
            state
                .project
                .existing_import_frames(asset)
                .map_err(|_| "sample insertion refused · source is unavailable")?;
            self.admit_browser_insertion(PendingBrowserInsertionSource::ProjectSample(asset), cx)
        })();
        if let Err(error) = result {
            self.error = Some(error.into());
        }
        cx.notify();
    }

    pub(super) fn insert_browser_audio(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.browser_place_unavailable() {
            self.error = Some("sample insertion refused · source is unavailable".into());
        } else if let Err(error) =
            self.admit_browser_insertion(PendingBrowserInsertionSource::AudioFile(path), cx)
        {
            self.error = Some(error.into());
        }
        cx.notify();
    }

    pub(in crate::editor) fn drain_pending_browser_insertion(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.browser.pending_insertions.is_empty()
            || self.browser.insertion_flight.is_some()
            || self.persistence.audio_batch.is_some()
            || self.worker_requests_pending != 0
            || self.mode != EditorMode::Arrangement
            || self.browser_insertion_hard_busy(cx)
        {
            return false;
        }
        let Some(next) = self.browser.pending_insertions.pop_front() else {
            return false;
        };
        self.submit_browser_insertion(next);
        true
    }

    fn submit_browser_insertion(&mut self, insertion: PendingBrowserInsertion) {
        let Some(state) = &self.state else {
            self.error = Some("sample insertion refused · no project".into());
            return;
        };
        if insertion.project_generation != self.project_generation {
            self.error = Some("sample insertion canceled · project changed".into());
            return;
        }
        if state.project.track(insertion.track).is_none() {
            self.error = Some("sample insertion refused · select an arrangement track".into());
            return;
        }
        let revision = state.revision;
        let audition_timing = self.refresh_queued_audition_timing(insertion.audition_timing);
        let routed_audition = audition_timing.is_some();
        let (request, flight) = match insertion.source {
            PendingBrowserInsertionSource::AudioFile(path) => {
                let context = AudioBatchContext {
                    project_generation: self.project_generation,
                    revision,
                    active_gesture: false,
                };
                match Request::browser_audio_batch(
                    path,
                    insertion.track,
                    insertion.start,
                    context,
                    audition_timing,
                ) {
                    Ok((mut request, control)) => {
                        if let Request::ImportAudioBatch { options, .. } = &mut request {
                            options.warp = insertion.warp;
                        }
                        (request, BrowserInsertionFlight::AudioFile(control))
                    }
                    Err(error) => {
                        self.error = Some(error.into());
                        return;
                    }
                }
            }
            PendingBrowserInsertionSource::ProjectSample(asset) => {
                if state.project.existing_import_frames(asset).is_err() {
                    self.error = Some("sample insertion refused · source is unavailable".into());
                    return;
                }
                let destination = AudioBatchDestination::Track {
                    track: insertion.track,
                    start: insertion.start,
                };
                let request = Request::InsertProjectSample {
                    project_generation: self.project_generation,
                    revision,
                    asset,
                    destination,
                    new_track_gain: None,
                    warp: insertion.warp,
                    audition_timing,
                };
                (
                    request,
                    BrowserInsertionFlight::ProjectSample {
                        asset,
                        track: insertion.track,
                        start: insertion.start,
                        receipt: Receipt::new(self.project_generation),
                    },
                )
            }
        };
        // Install custody before enqueue so immediate admission and UI FIFO retries
        // both stamp the same request. A refused entry leaves later taps intact.
        self.browser.insertion_flight = Some(flight);
        if !self.enqueue_captured_audio_import(request) {
            self.browser.insertion_flight = None;
            return;
        }
        match &self.browser.insertion_flight {
            Some(BrowserInsertionFlight::AudioFile(control)) => {
                self.persistence.audio_batch = Some(Arc::clone(control));
                if routed_audition && self.browser.playback_output.is_some() {
                    self.cancel_browser_sample_playback();
                }
                self.persistence.audio_batch_progress = None;
                self.poll_audio_batch();
            }
            Some(BrowserInsertionFlight::ProjectSample { .. }) => {
                self.stop_browser_preview_for_drop()
            }
            None => {}
        }
    }

    fn browser_insertion_hard_busy(&self, cx: &gpui::App) -> bool {
        self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.persistence.save_dialog_open
            || self.persistence.export_dialog_open
            || self.device_panel.gesture.snapshot().is_some()
            || self.device_panel.gesture.has_pending_terminal()
            || !self.device_panel.gesture_barriers.is_empty()
            || self.device_panel.has_pending_draft(cx)
            || !self.pending_requests.is_empty()
            || self.active_clip_drag.is_some()
            || self.browser.active_audio_drag.is_some()
            || self.device_panel.active_native_drag.is_some()
            || self.audio_recorder.pending_commit_path.is_some()
    }

    fn admit_browser_insertion(
        &mut self,
        source: PendingBrowserInsertionSource,
        cx: &gpui::App,
    ) -> Result<(), String> {
        if self.mode != EditorMode::Arrangement {
            return Err("sample insertion refused · select an arrangement track".into());
        }
        if self.browser_insertion_hard_busy(cx) {
            return Err("sample insertion refused · arrangement is busy".into());
        }
        let queued =
            self.browser.insertion_flight.is_some() || !self.browser.pending_insertions.is_empty();
        if queued {
            if self.browser.pending_insertions.len() >= MAX_PENDING_BROWSER_INSERTIONS
                || self
                    .persistence
                    .audio_batch
                    .as_ref()
                    .is_some_and(|control| control.progress().cancellation_requested)
            {
                return Err("sample insertion refused · arrangement is busy".into());
            }
        } else if self.persistence.audio_batch.is_some() || self.worker_requests_pending != 0 {
            return Err("sample insertion refused · arrangement is busy".into());
        }
        let state = self
            .state
            .as_ref()
            .ok_or("sample insertion refused · no project")?;
        let track = self
            .selected_track
            .filter(|id| state.project.track(*id).is_some())
            .ok_or("sample insertion refused · select an arrangement track")?;
        let grid = self.resolve_input_grid()?;
        let status = self.audio_status();
        let transport_epoch = status.map(|status| status.transport_epoch.load(Acquire));
        let start = grid
            .snap_frame(state.project.tempo_map(), self.cursor_position())
            .map_err(|_| "sample insertion refused · cannot resolve input grid")?;
        let audition_timing = status.zip(transport_epoch).and_then(|(status, epoch)| {
            (self.document_audio_mode == hex_session::DocumentAudioMode::Resident
                && status.playing.load(Acquire)
                && status.transport_epoch.load(Acquire) == epoch)
                .then_some(BrowserAuditionTiming {
                    grid,
                    transport_epoch: epoch,
                })
        });
        let insertion = PendingBrowserInsertion {
            project_generation: self.project_generation,
            source,
            warp: self.settings.audio_import.arrangement_policy()?,
            track,
            start,
            audition_timing,
        };
        self.error = None;
        if queued {
            self.browser.pending_insertions.push_back(insertion);
            self.wake_poll();
        } else {
            self.submit_browser_insertion(insertion);
        }
        Ok(())
    }

    fn refresh_queued_audition_timing(
        &self,
        captured: Option<BrowserAuditionTiming>,
    ) -> Option<BrowserAuditionTiming> {
        let timing = captured?;
        let status = self.audio_status()?;
        (self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && status.playing.load(Acquire)
            && status.transport_epoch.load(Acquire) == timing.transport_epoch)
            .then_some(timing)
    }
}
