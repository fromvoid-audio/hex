//! A released sample drop keeps its paint until the matching import receipt.
//! Input cancellation and browser scan changes only affect the held payload.
use super::{
    EditorView,
    drag::{BrowserAudioDrag, BrowserDropDestination, DropPreview},
    pending_edit::{Disposition, Receipt},
};
use hex_session::service::{AudioPreview, Event, Request};
use std::{path::PathBuf, rc::Rc};

pub(super) struct PendingSampleDrop {
    receipt: Receipt,
    path: PathBuf,
    destination: BrowserDropDestination,
    pub(super) preview: Option<DropPreview>,
    pub(super) insertion_line_y: Option<f32>,
    pub(super) metadata: Option<Rc<AudioPreview>>,
}

impl EditorView {
    pub(super) fn enqueue_sample_drop(
        &mut self,
        request: Request,
        drag: &BrowserAudioDrag,
        destination: BrowserDropDestination,
    ) -> bool {
        if self.pending_sample_drop.is_some() {
            return false;
        }
        self.pending_sample_drop = Some(PendingSampleDrop {
            receipt: Receipt::new(self.project_generation),
            path: drag.path.clone(),
            destination,
            preview: drag.preview.get(),
            insertion_line_y: drag.insertion_line_y.get(),
            metadata: drag.metadata.borrow().clone(),
        });
        if self.enqueue(request) {
            true
        } else {
            self.pending_sample_drop = None;
            false
        }
    }

    pub(super) fn pending_sample_drop_matches(&self, request: &Request) -> bool {
        use hex_session::import_batch::AudioBatchDestination;
        let Some(drop) = &self.pending_sample_drop else {
            return false;
        };
        drop.receipt.is_queued()
            && match (drop.destination, request) {
                (
                    BrowserDropDestination::ExistingTrack { track, start },
                    Request::ImportToTrackWithOverwrite {
                        path,
                        track: t,
                        start: s,
                    },
                ) => drop.path == *path && (track, start) == (*t, *s),
                (
                    BrowserDropDestination::NewTrack {
                        before_track,
                        start,
                    },
                    Request::ImportToNewTrack {
                        path,
                        before_track: t,
                        start: s,
                    },
                ) => drop.path == *path && (before_track, start) == (*t, *s),
                (
                    BrowserDropDestination::Console(target),
                    Request::ImportAudioBatch {
                        paths,
                        destination: AudioBatchDestination::Console(t),
                        ..
                    },
                ) => target == *t && paths.as_slice() == std::slice::from_ref(&drop.path),
                _ => false,
            }
    }

    pub(super) fn sample_drop_submitted(&mut self) {
        if let Some(drop) = &mut self.pending_sample_drop {
            drop.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(super) fn sample_drop_complete(&mut self, event: &Event) {
        if let Some(drop) = &mut self.pending_sample_drop
            && drop.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        {
            self.pending_sample_drop = None;
        }
    }

    pub(super) fn sample_drop_target(&self) -> Option<BrowserDropDestination> {
        self.pending_sample_drop
            .as_ref()
            .map(|drop| drop.destination)
            .or_else(|| self.displayed_browser_audio_drag()?.destination.get())
    }

    pub(super) fn displayed_browser_audio_drag(&self) -> Option<&BrowserAudioDrag> {
        self.active_browser_audio_drag
            .as_ref()
            .filter(|drag| self.browser_audio_drag_is_current(drag))
    }
}
