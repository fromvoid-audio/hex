//! Pinned page chunk preparation, trait adapters (`MediaPages`,
//! `PreparedExportPages`, `PreparedPrerollPages`), and post-render verification
//! for `VerifiedPages`.
use super::{
    PAGE_POLL, PREPARATION_TIMEOUT,
    derived::{Cancellation, check, media},
};
use hex_engine::{
    BorrowedMediaPage, MediaPages, SessionPlayback, StreamPageExtent, StreamSourceKey,
};
use hex_export::{ExportControl, ExportError, PreparedExportPages};
use hex_session::VerifiedAudioFile;
use hex_stream::{
    PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind, StreamRuntime,
};
use std::{sync::atomic::AtomicBool, time::Instant};

pub(super) struct VerifiedPages {
    pub(super) config: StreamConfig,
    pub(super) definitions: Option<Vec<SourceDefinition>>,
    pub(super) granulars: Option<Vec<hex_engine::audio_transform::StreamedGranularRoot>>,
    pub(super) files: Vec<(StreamSourceKey, VerifiedAudioFile)>,
    pub(super) runtime: Option<StreamRuntime>,
    pub(super) extents: Vec<StreamPageExtent>,
    pub(super) keys: Vec<PageKey>,
    pub(super) missed: bool,
}

impl MediaPages for VerifiedPages {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        let page = self
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.page(key, index));
        if page.is_none() {
            self.missed = true;
        }
        page
    }
    fn request_page(&mut self, _key: StreamSourceKey, _index: u64) {
        // A render-side request means preflight was incomplete. Never retry DSP
        // after it has already advanced using source-local fallback silence.
        self.missed = true;
    }
}

impl VerifiedPages {
    pub(super) fn prepare_stream_chunk(
        &mut self,
        playback: &SessionPlayback,
        frames: usize,
        ordinary: bool,
        control: &impl Cancellation,
    ) -> Result<usize, ExportError> {
        check(control)?;
        self.extents.clear();
        self.keys.clear();
        self.missed = false;
        let maximum = self.config.page_buffers;
        let extents = &mut self.extents;
        let mut visit = |extent: hex_engine::StreamResourceExtent| {
            let hex_engine::StreamResourceKey::Audio(source) = extent.source else {
                return Err(hex_engine::Invalid(
                    "offline scalar source capture is not prepared",
                ));
            };
            let extent = StreamPageExtent {
                source,
                page_index: extent.page_index,
                start: extent.start,
                end: extent.end,
            };
            if control.is_cancelled() {
                return Err(hex_engine::Invalid("export cancelled"));
            }
            if extent.start >= extent.end || extent.end > hex_engine::STREAM_PAGE_FRAMES {
                return Err(hex_engine::Invalid("invalid offline stream extent"));
            }
            let key = (extent.source, extent.page_index);
            match extents.binary_search_by_key(&key, |old| (old.source, old.page_index)) {
                Ok(index) => {
                    extents[index].start = extents[index].start.min(extent.start);
                    extents[index].end = extents[index].end.max(extent.end);
                }
                Err(index) => {
                    if extents.len() == maximum {
                        return Err(hex_engine::Invalid(
                            "offline export page demand exceeds its fixed pool",
                        ));
                    }
                    extents.insert(index, extent);
                }
            }
            Ok(())
        };
        let window = (if ordinary {
            playback.visit_ordinary_stream_chunk(frames, None, &mut visit)
        } else {
            playback.visit_next_stream_chunk(frames, None, &mut visit)
        })
        .map_err(|error| {
            if control.is_cancelled() {
                ExportError::Cancelled
            } else {
                ExportError::Render(error)
            }
        })?;
        if self.extents.is_empty() {
            return Ok(window.frames);
        }
        for extent in &self.extents {
            if self
                .files
                .binary_search_by_key(&extent.source, |(key, _)| *key)
                .is_err()
            {
                return Err(ExportError::Invalid("export demand has no captured file"));
            }
            self.keys.push(PageKey {
                source: extent.source,
                page_index: extent.page_index,
            });
        }
        self.ensure_runtime(control, self.keys.len())?;
        let capacity = self
            .runtime
            .as_ref()
            .map_or(0, StreamRuntime::page_capacity);
        if self.keys.len() > capacity {
            return Err(ExportError::Invalid(
                "offline export page demand exceeds simultaneous cache capacity",
            ));
        }
        let first_key = self
            .keys
            .first()
            .copied()
            .ok_or(ExportError::Invalid("offline export page demand is empty"))?;
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.unpin_all();
        }
        let deadline = Instant::now() + PREPARATION_TIMEOUT;
        loop {
            check(control)?;
            let runtime = self
                .runtime
                .as_mut()
                .ok_or(ExportError::Invalid("offline stream worker is unavailable"))?;
            // Protect the required subset across pumps. A full cache could
            // otherwise evict a required page while loading another.
            for key in &self.keys {
                runtime.pin_pages(std::slice::from_ref(key));
            }
            runtime.pump_ready(self.config.max_ready_pages_per_pump);
            let mut failures = [StreamFailure {
                key: first_key.source.into(),
                page_index: 0,
                kind: StreamFailureKind::Cancelled,
            }];
            if runtime.take_failures(&mut failures) != 0 {
                return Err(self.failure(failures[0], control));
            }
            if runtime.pin_pages(&self.keys) {
                for extent in &self.extents {
                    let page = runtime
                        .page(extent.source, extent.page_index)
                        .ok_or(ExportError::Invalid("pinned export page is unavailable"))?;
                    if page.page_index != extent.page_index || page.frames.len() < extent.end {
                        return Err(ExportError::Invalid(
                            "verified export page is short or mismatched",
                        ));
                    }
                }
                return Ok(window.frames);
            }
            // Admit one missing page at a time so pumping can return an
            // evicted page before the worker needs the next turnover buffer.
            if let Some(key) = self
                .keys
                .iter()
                .find(|key| runtime.page(key.source, key.page_index).is_none())
            {
                runtime.request_page(key.source, key.page_index);
                runtime.wake_worker();
            }
            if Instant::now() >= deadline {
                return Err(ExportError::Media(
                    "media pages timed out; check the source disk and retry".into(),
                ));
            }
            std::thread::park_timeout(PAGE_POLL);
        }
    }

    pub(super) fn finish_pages(&mut self, control: &impl Cancellation) -> Result<(), ExportError> {
        check(control)?;
        if let Some(mut runtime) = self.runtime.take() {
            runtime.unpin_all();
            let report = runtime.shutdown().map_err(media)?;
            if !report.worker_joined
                || !report.accounting_complete
                || report.page_buffers != self.config.page_buffers
            {
                return Err(ExportError::Invalid(
                    "offline stream cleanup did not balance",
                ));
            }
        }
        // Reopen/hash after all streaming reads and before the publication seal.
        // Atomic path replacement and changes to already-cached pages cannot
        // silently refresh the captured snapshot or produce a successful file.
        for (_, file) in &self.files {
            check(control)?;
            let reader = hex_media::VerifiedWavReader::open_with_cancel(
                file.path(),
                file.reader_limits(),
                || control.is_cancelled(),
            )
            .map_err(|error| {
                if control.is_cancelled() {
                    ExportError::Cancelled
                } else {
                    media(error)
                }
            })?;
            if reader.fingerprint() != file.fingerprint() {
                return Err(ExportError::Media(format!(
                    "original media changed; reopen or relink {}",
                    file.path().display()
                )));
            }
        }
        check(control)
    }
}

impl PreparedExportPages for VerifiedPages {
    fn prepare_chunk(
        &mut self,
        playback: &SessionPlayback,
        frames: usize,
        control: &ExportControl,
    ) -> Result<(), ExportError> {
        let width = self.prepare_stream_chunk(playback, frames, false, control)?;
        if width != frames {
            return Err(ExportError::Invalid("linear export demand width differs"));
        }
        Ok(())
    }
    fn complete_chunk(&mut self) -> Result<(), ExportError> {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.unpin_all();
        }
        if self.missed {
            Err(ExportError::Invalid(
                "export encountered an unexpected media page miss",
            ))
        } else {
            Ok(())
        }
    }
    fn finish(&mut self, control: &ExportControl) -> Result<(), ExportError> {
        self.finish_pages(control)
    }
}

pub(super) fn seek_error(error: ExportError) -> hex_engine::ArrangementPrerollError {
    match error {
        ExportError::Cancelled => hex_engine::ArrangementPrerollError::Cancelled,
        ExportError::Invalid(message) => {
            hex_engine::ArrangementPrerollError::MediaPreparation(message)
        }
        _ => hex_engine::ArrangementPrerollError::MediaPreparation(
            "file-backed seek media unavailable; reopen or relink the source",
        ),
    }
}

impl hex_engine::PreparedPrerollPages for VerifiedPages {
    fn prepare_chunk(
        &mut self,
        playback: &SessionPlayback,
        maximum_frames: usize,
        cancelled: &AtomicBool,
    ) -> Result<usize, hex_engine::ArrangementPrerollError> {
        self.prepare_stream_chunk(playback, maximum_frames, true, cancelled)
            .map_err(seek_error)
    }
    fn complete_chunk(&mut self) -> Result<(), hex_engine::ArrangementPrerollError> {
        PreparedExportPages::complete_chunk(self).map_err(seek_error)
    }
}
