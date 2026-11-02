//! Offline bounce of file-backed (streamed) sources through a host page provider.
//!
//! Session owns the bounce plan, budgets, isolation and adoption. A host that can
//! verify original files and serve bounded pages offers a stateless
//! [`StreamedBouncer`]; it runs synchronously on the bounce's control worker and
//! must create, use and join its own page resources before returning. Nothing here
//! borrows native output, catalog or readiness custody, so a bounce never waits on
//! or blocks the playing transport. Without a bouncer a streamed source refuses
//! explicitly (the renderer has no pages) and adoption never starts.
use crate::{AssetId, VerifiedAudioFile};
use hex_engine::{PreparedRevision, native_device_resources::PreparedDeviceResources};
use std::{collections::BTreeMap, path::Path, sync::Arc};

/// Stateless host capability: render one isolated bounce item to `path` with
/// independently verified pages, via `hex_export::render_arrangement_bounce_with_pages`.
pub type StreamedBouncer =
    fn(&StreamedBounceRequest<'_>) -> Result<hex_export::ExportReport, String>;

/// Borrowed complete input for one bounce item. Only Session constructs it.
pub struct StreamedBounceRequest<'a> {
    pub(crate) prepared: &'a Arc<PreparedRevision>,
    pub(crate) resources: Option<&'a PreparedDeviceResources>,
    pub(crate) files: &'a BTreeMap<AssetId, VerifiedAudioFile>,
    pub(crate) path: &'a Path,
    pub(crate) start: u64,
    pub(crate) frames: u64,
    pub(crate) include_tails: bool,
    pub(crate) control: &'a hex_export::ExportControl,
}
impl StreamedBounceRequest<'_> {
    /// The isolated revision; its streamed source keys name the captured files.
    pub fn prepared(&self) -> &Arc<PreparedRevision> {
        self.prepared
    }
    /// The admitted compiler scope, when the bounce renders through one.
    pub fn resources(&self) -> Option<&PreparedDeviceResources> {
        self.resources
    }
    /// Verified file facts captured with the document's source bindings.
    pub fn file(&self, asset: AssetId) -> Option<&VerifiedAudioFile> {
        self.files.get(&asset)
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
    pub fn include_tails(&self) -> bool {
        self.include_tails
    }
    pub fn control(&self) -> &hex_export::ExportControl {
        self.control
    }
}

/// One isolated bounce input: the prepared revision plus the file facts its
/// streamed sources need (empty when every source is resident).
pub(crate) struct BounceRevision {
    pub(crate) prepared: Arc<PreparedRevision>,
    pub(crate) files: BTreeMap<AssetId, VerifiedAudioFile>,
}
impl BounceRevision {
    pub(crate) fn capture(
        prepared: Arc<PreparedRevision>,
        verified: &BTreeMap<AssetId, VerifiedAudioFile>,
    ) -> Self {
        let files = if prepared.arrangement_stream_sources().next().is_some() {
            verified.clone()
        } else {
            BTreeMap::new()
        };
        Self { prepared, files }
    }
}

/// Render one bounce item, through the host bouncer when it has streamed sources.
#[allow(clippy::too_many_arguments)] // The exact bounce render contract plus its optional host.
pub(crate) fn render(
    path: &Path,
    revision: BounceRevision,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    include_tails: bool,
    control: &hex_export::ExportControl,
    bouncer: Option<StreamedBouncer>,
) -> Result<u64, String> {
    if revision
        .prepared
        .arrangement_stream_sources()
        .next()
        .is_some()
        && let Some(bouncer) = bouncer
    {
        return bouncer(&StreamedBounceRequest {
            prepared: &revision.prepared,
            resources,
            files: &revision.files,
            path,
            start,
            frames,
            include_tails,
            control,
        })
        .map(|report| report.frames);
    }
    hex_export::render_arrangement_bounce_new(
        path,
        revision.prepared,
        resources,
        start,
        frames,
        include_tails,
        control,
    )
    .map(|report| report.frames)
    .map_err(|e| e.to_string())
}
