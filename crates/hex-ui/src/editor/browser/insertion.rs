//! Enter captures one selected Arrangement target before bounded worker preparation.
//! Rapid Enter taps while a browser sample insertion is already in flight queue
//! their captured Arrangement target and submit sequentially once the current
//! insertion completes, instead of failing as busy.
use super::{EditorMode, EditorView};
use gpui::Context;
use hex_engine::playback::BrowserAuditionTiming;
use hex_session::import_batch_decode::AudioBatchContext;
use hex_session::service::Request;
use std::{path::PathBuf, sync::atomic::Ordering::Acquire};

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
    track: hex_project::TrackId,
    start: u64,
    audition_timing: Option<BrowserAuditionTiming>,
}

enum BrowserInsertionAdmission {
    Immediate {
        track: hex_project::TrackId,
        start: u64,
        audition_timing: Option<BrowserAuditionTiming>,
    },
    Queue {
        track: hex_project::TrackId,
        start: u64,
        audition_timing: Option<BrowserAuditionTiming>,
    },
}

impl EditorView {
    pub(super) fn insert_browser_project_sample(
        &mut self,
        generation: u64,
        revision: u64,
        asset: hex_project::AssetId,
        cx: &mut Context<Self>,
    ) {
        let admission = (|| -> Result<BrowserInsertionAdmission, String> {
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
            self.browser_insertion_admission(cx)
        })();
        match admission {
            Ok(BrowserInsertionAdmission::Immediate {
                track,
                start,
                audition_timing,
            }) => {
                let request = Request::InsertProjectSample {
                    project_generation: generation,
                    revision,
                    asset,
                    destination: hex_session::import_batch::AudioBatchDestination::Track {
                        track,
                        start,
                    },
                    new_track_gain: None,
                    warp: hex_project::AudioImportWarp::Off,
                    audition_timing,
                };
                if self.enqueue(request) {
                    self.browser_sample_insert_in_flight = true;
                    self.stop_browser_preview_for_drop();
                    self.error = None;
                }
            }
            Ok(BrowserInsertionAdmission::Queue {
                track,
                start,
                audition_timing,
            }) => {
                self.pending_browser_insertions
                    .push_back(PendingBrowserInsertion {
                        project_generation: self.project_generation,
                        source: PendingBrowserInsertionSource::ProjectSample(asset),
                        track,
                        start,
                        audition_timing,
                    });
                self.error = None;
                self.wake_poll();
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    pub(super) fn insert_browser_audio(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let admission = (|| -> Result<BrowserInsertionAdmission, String> {
            if self.browser_place_unavailable() {
                return Err("sample insertion refused · source is unavailable".into());
            }
            self.browser_insertion_admission(cx)
        })();
        match admission {
            Ok(BrowserInsertionAdmission::Immediate {
                track,
                start,
                audition_timing,
            }) => {
                let revision = match self.state.as_ref() {
                    Some(state) => state.revision,
                    None => {
                        self.error = Some("sample insertion refused · no project".into());
                        cx.notify();
                        return;
                    }
                };
                self.submit_browser_audio_batch(path, track, start, revision, audition_timing);
            }
            Ok(BrowserInsertionAdmission::Queue {
                track,
                start,
                audition_timing,
            }) => {
                self.pending_browser_insertions
                    .push_back(PendingBrowserInsertion {
                        project_generation: self.project_generation,
                        source: PendingBrowserInsertionSource::AudioFile(path),
                        track,
                        start,
                        audition_timing,
                    });
                self.error = None;
                self.wake_poll();
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    pub(in crate::editor) fn drain_pending_browser_insertion(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_browser_insertions.is_empty() {
            return false;
        }
        if self.audio_batch.is_some()
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
        {
            return false;
        }
        if self.mode != EditorMode::Arrangement || self.browser_insertion_hard_busy(cx) {
            self.pending_browser_insertions.clear();
            return true;
        }
        let Some(next) = self.pending_browser_insertions.pop_front() else {
            return false;
        };
        if next.project_generation != self.project_generation {
            self.pending_browser_insertions.clear();
            return true;
        }
        let Some(state) = self.state.as_ref() else {
            self.pending_browser_insertions.clear();
            return true;
        };
        if state.project.track(next.track).is_none() {
            self.pending_browser_insertions.clear();
            self.error = Some("sample insertion refused · select an arrangement track".into());
            return true;
        }
        let revision = state.revision;
        let audition_timing = self.refresh_queued_audition_timing(next.audition_timing);
        match next.source {
            PendingBrowserInsertionSource::AudioFile(path) => {
                if self.browser_place_unavailable() {
                    self.pending_browser_insertions.clear();
                    self.error = Some("sample insertion refused · source is unavailable".into());
                    return true;
                }
                self.submit_browser_audio_batch(
                    path,
                    next.track,
                    next.start,
                    revision,
                    audition_timing,
                );
                true
            }
            PendingBrowserInsertionSource::ProjectSample(asset) => {
                if state.project.existing_import_frames(asset).is_err() {
                    self.pending_browser_insertions.clear();
                    self.error = Some("sample insertion refused · source is unavailable".into());
                    return true;
                }
                let request = Request::InsertProjectSample {
                    project_generation: self.project_generation,
                    revision,
                    asset,
                    destination: hex_session::import_batch::AudioBatchDestination::Track {
                        track: next.track,
                        start: next.start,
                    },
                    new_track_gain: None,
                    warp: hex_project::AudioImportWarp::Off,
                    audition_timing,
                };
                if self.enqueue(request) {
                    self.browser_sample_insert_in_flight = true;
                    self.stop_browser_preview_for_drop();
                    self.error = None;
                }
                true
            }
        }
    }

    fn submit_browser_audio_batch(
        &mut self,
        path: PathBuf,
        track: hex_project::TrackId,
        start: u64,
        revision: u64,
        audition_timing: Option<BrowserAuditionTiming>,
    ) {
        let result = Request::browser_audio_batch(
            path,
            track,
            start,
            AudioBatchContext {
                project_generation: self.project_generation,
                revision,
                active_gesture: false,
            },
            audition_timing,
        );
        match result {
            Ok((request, control)) => {
                // Capture metadata only. Decode and the canonical overwrite commit stay on
                // the service worker; later UI selection/cursor changes cannot retarget it.
                let routed_audition = matches!(
                    &request,
                    Request::ImportAudioBatch {
                        audition_timing: Some(_),
                        ..
                    }
                );
                if self.enqueue(request) {
                    self.browser_sample_insert_in_flight = true;
                    if routed_audition && self.browser.playback_output.is_some() {
                        self.cancel_browser_sample_playback();
                    }
                    self.audio_batch = Some(control);
                    self.audio_batch_progress = None;
                    self.error = None;
                    self.poll_audio_batch();
                }
            }
            Err(error) => {
                self.pending_browser_insertions.clear();
                self.error = Some(error.into());
            }
        }
    }

    fn browser_insertion_hard_busy(&self, cx: &gpui::App) -> bool {
        self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.device_gesture.snapshot().is_some()
            || self.device_gesture.has_pending_terminal()
            || !self.device_gesture_barriers.is_empty()
            || self.device_panel.has_pending_draft(cx)
            || !self.pending_requests.is_empty()
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.audio_recorder.pending_commit_path.is_some()
    }

    fn browser_insertion_admission(
        &self,
        cx: &gpui::App,
    ) -> Result<BrowserInsertionAdmission, String> {
        if self.mode != EditorMode::Arrangement {
            return Err("sample insertion refused · select an arrangement track".into());
        }
        if self.browser_insertion_hard_busy(cx) {
            return Err("sample insertion refused · arrangement is busy".into());
        }
        let in_flight = self.audio_batch.is_some() || self.worker_requests_pending != 0;
        let can_queue = in_flight
            && self.browser_sample_insert_in_flight
            && !self
                .audio_batch
                .as_ref()
                .is_some_and(|control| control.progress().cancellation_requested)
            && self.pending_browser_insertions.len() < MAX_PENDING_BROWSER_INSERTIONS;
        if in_flight && !can_queue {
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
        if in_flight {
            Ok(BrowserInsertionAdmission::Queue {
                track,
                start,
                audition_timing,
            })
        } else {
            Ok(BrowserInsertionAdmission::Immediate {
                track,
                start,
                audition_timing,
            })
        }
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
