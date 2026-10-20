//! The actual default exporter is a fixed implementation, with no captured
//! closure allocation. This says nothing about its queued compiled inputs or
//! rendering/runtime/File storage; those remain separate mandatory owners.
use super::*;

pub(super) enum ExportRenderer {
    Wav,
    #[cfg(test)]
    Injected(Exporter),
}

impl ExportRenderer {
    pub(super) fn render(
        &self,
        path: &Path,
        prepared: &Arc<PreparedRevision>,
        start: u64,
        frames: u64,
    ) -> Result<ExportReport, String> {
        match self {
            Self::Wav => hex_export::render_prepared_new(path, Arc::clone(prepared), start, frames)
                .map_err(|error| error.to_string().to_lowercase()),
            #[cfg(test)]
            Self::Injected(render) => render(path, prepared, start, frames),
        }
    }
}
