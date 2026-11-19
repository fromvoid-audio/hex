//! Deterministic fault injection around the real offline page provider.
use super::*;

pub(crate) fn finish_action(
    request: &StreamedExportRequest<'_>,
    action: impl FnOnce(),
) -> Result<ExportReport, hex_session::service::ExportFailure> {
    render_action(request, true, false, action)
}

pub(crate) fn chunk_action(
    request: &StreamedExportRequest<'_>,
    action: impl FnOnce(),
) -> Result<ExportReport, hex_session::service::ExportFailure> {
    render_action(request, false, false, action)
}

pub(crate) fn missing_page(
    request: &StreamedExportRequest<'_>,
) -> Result<ExportReport, hex_session::service::ExportFailure> {
    render_action(request, true, true, || {})
}

fn render_action(
    request: &StreamedExportRequest<'_>,
    at_finish: bool,
    hide_pages: bool,
    action: impl FnOnce(),
) -> Result<ExportReport, hex_session::service::ExportFailure> {
    struct FinalAction<F> {
        pages: VerifiedPages,
        action: Option<F>,
        at_finish: bool,
        hide_pages: bool,
    }
    impl<F: FnOnce()> MediaPages for FinalAction<F> {
        fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
            if self.hide_pages {
                self.pages.request_page(key, index);
                None
            } else {
                self.pages.page(key, index)
            }
        }
        fn request_page(&mut self, key: StreamSourceKey, index: u64) {
            self.pages.request_page(key, index);
        }
    }
    impl<F: FnOnce()> PreparedExportPages for FinalAction<F> {
        fn prepare_chunk(
            &mut self,
            playback: &SessionPlayback,
            frames: usize,
            control: &ExportControl,
        ) -> Result<(), ExportError> {
            self.pages.prepare_chunk(playback, frames, control)
        }
        fn complete_chunk(&mut self) -> Result<(), ExportError> {
            self.pages.complete_chunk()?;
            if !self.at_finish
                && let Some(action) = self.action.take()
            {
                action();
            }
            Ok(())
        }
        fn finish(&mut self, control: &ExportControl) -> Result<(), ExportError> {
            if let Some(action) = self.action.take() {
                action();
            }
            self.pages.finish(control)
        }
    }
    let pages = VerifiedPages::capture(request, StreamConfig::with_pool_pages(3))
        .map_err(hex_session::service::ExportFailure::from)?;
    let mut pages = FinalAction {
        pages,
        action: Some(action),
        at_finish,
        hide_pages,
    };
    hex_export::render_prepared_with_pages_and_policy(
        request.path(),
        Arc::clone(request.prepared()),
        request.resources(),
        request.start(),
        request.frames(),
        request.signal().clone(),
        request.format(),
        request.control(),
        request.publication(),
        &mut pages,
    )
    .map_err(Into::into)
}
