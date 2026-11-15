//! Exact canonical export input; no stream/cache/native resource ownership.
use super::*;

/// Stateless host capability, invoked only on the existing offline export worker.
/// It owns independent verification/page preparation through ordinary export and
/// must drop/join every resource before returning. Publisher failures retain
/// their structured recovery paths. This grants no native admission.
pub type StreamedExporter = fn(&StreamedExportRequest<'_>) -> Result<ExportReport, ExportFailure>;

pub(super) struct StreamedExportJob {
    pub(super) source: Arc<CanonicalAudioSourceSnapshot>,
    pub(super) render: StreamedExporter,
}
impl StreamedExportJob {
    pub(super) fn capture(
        session: &Session,
        project: &Arc<Project>,
        generation: u64,
        revision: u64,
        renderer: Option<StreamedExporter>,
    ) -> Result<Self, String> {
        let render = renderer.ok_or(
            if session.audio_mode() == DocumentAudioMode::VerifiedStreaming {
                "offline export is unavailable for verified streaming"
            } else {
                "offline file-backed export is unavailable"
            },
        )?;
        Ok(Self {
            // Current canonical worker state, never an audition/preview event.
            source: Arc::new(CanonicalAudioSourceSnapshot::capture(
                project, generation, revision, session,
            )),
            render,
        })
    }
}

/// Borrowed complete input from one admitted canonical worker job. Only Session
/// constructs it; source context and project/file facts are captured together.
/// A compiled authoring alias may retain older media keys: use its actual source
/// metadata rather than relabeling it with document/editor counters.
pub struct StreamedExportRequest<'a> {
    pub(super) source: &'a CanonicalAudioSourceSnapshot,
    pub(super) prepared: &'a Arc<PreparedRevision>,
    pub(super) resources: Option<&'a hex_engine::native_device_resources::PreparedDeviceResources>,
    pub(super) path: &'a Path,
    pub(super) start: u64,
    pub(super) frames: u64,
    pub(super) signal: &'a hex_export::ExportSignal,
    pub(super) format: hex_export::ExportFormat,
    pub(super) control: &'a hex_export::ExportControl,
    pub(super) publication: &'a hex_export::Publication,
}
impl StreamedExportRequest<'_> {
    pub fn source(&self) -> &CanonicalAudioSourceSnapshot {
        self.source
    }
    /// Exact compiler scope captured and admitted by the ordinary export lane.
    /// This borrow grants no native source/readiness or output authority.
    pub fn resources(
        &self,
    ) -> Option<&hex_engine::native_device_resources::PreparedDeviceResources> {
        self.resources
    }
    pub fn prepared(&self) -> &Arc<PreparedRevision> {
        self.prepared
    }
    pub fn path(&self) -> &Path {
        self.path
    }
    pub fn start(&self) -> u64 {
        self.start
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn signal(&self) -> &hex_export::ExportSignal {
        self.signal
    }
    /// Sample format of the file or of every stem file in the batch.
    pub fn format(&self) -> hex_export::ExportFormat {
        self.format
    }
    pub fn control(&self) -> &hex_export::ExportControl {
        self.control
    }
    pub fn publication(&self) -> &hex_export::Publication {
        self.publication
    }
}
