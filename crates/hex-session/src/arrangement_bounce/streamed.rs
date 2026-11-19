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
use hex_export::ForeignBounceRenderer;
use hex_project::Project;
use std::{collections::BTreeMap, path::Path, sync::Arc};

/// Stateless host capability: render one isolated bounce item to `path` with
/// independently verified pages and the bounce's optional private processor set.
pub type StreamedBouncer = fn(
    &StreamedBounceRequest<'_>,
    Option<&mut dyn ForeignBounceRenderer>,
) -> Result<hex_export::ExportReport, String>;

/// Borrowed complete input for one bounce item. Only Session constructs it.
pub struct StreamedBounceRequest<'a> {
    pub(crate) prepared: &'a Arc<PreparedRevision>,
    pub(crate) resources: Option<&'a PreparedDeviceResources>,
    pub(crate) files: &'a BTreeMap<AssetId, VerifiedAudioFile>,
    pub(crate) repitches: &'a [hex_engine::audio_transform::StreamedRepitchRequirement],
    pub(crate) granulars: &'a [hex_engine::audio_transform::StreamedGranularRequirement],
    isolated_project: Option<&'a Project>,
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
    /// Exact file-backed Re-Pitch maps captured with the isolated revision.
    pub fn repitches(&self) -> &[hex_engine::audio_transform::StreamedRepitchRequirement] {
        self.repitches
    }
    /// Without retained requirements, project one asset's Re-Pitch candidates
    /// lazily from the same isolated Project used to compile the bounce. The
    /// caller reserves projection workspace and checks exact derived metadata.
    pub fn projected_repitch_requirements(
        &self,
        asset: AssetId,
        generation: u64,
    ) -> Option<
        impl Iterator<
            Item = Result<
                hex_engine::audio_transform::StreamedRepitchRequirement,
                hex_engine::Invalid,
            >,
        > + '_,
    > {
        let project = self.isolated_project?;
        let file = self.file(asset)?;
        Some(
            project
                .tracks()
                .iter()
                .flat_map(|track| track.clips())
                .filter(move |clip| clip.asset() == asset)
                .filter_map(move |clip| {
                    let edit = clip
                        .audio_edit()
                        .filter(|edit| edit.is_streamed_repitch_root())?;
                    Some(file.source_metadata(asset, generation).and_then(|raw| {
                        hex_engine::audio_transform::StreamedRepitchRequirement::new(
                            &raw,
                            edit,
                            project.tempo_map(),
                            &std::sync::atomic::AtomicBool::new(false),
                        )
                    }))
                }),
        )
    }
    /// Exact granular roots retained with the isolated ordinary revision.
    pub fn granulars(&self) -> &[hex_engine::audio_transform::StreamedGranularRequirement] {
        self.granulars
    }
    /// Without a retained catalog, the offline worker may project one exact
    /// asset's candidate roots from the same isolated Project used to compile
    /// this item. The caller bounds projection workspace and matches the
    /// complete constructor metadata before admitting any page owner.
    pub fn projected_granular_roots(
        &self,
        asset: AssetId,
        generation: u64,
    ) -> impl Iterator<
        Item = Result<hex_engine::audio_transform::StreamedGranularRoot, hex_engine::Invalid>,
    > + '_ {
        self.isolated_project
            .zip(self.file(asset))
            .into_iter()
            .flat_map(move |(project, file)| {
                project
                    .tracks()
                    .iter()
                    .flat_map(|track| track.clips())
                    .filter(move |clip| clip.asset() == asset)
                    .filter_map(move |clip| {
                        crate::audio_sources::granular_source_root(project, clip, file, generation)
                            .transpose()
                    })
            })
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
    pub(crate) file_sources: Option<crate::service::ResidentFileSet>,
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
        Self {
            prepared,
            files,
            file_sources: None,
        }
    }
}

/// Render one bounce item, through the host bouncer when it has streamed sources.
#[allow(clippy::too_many_arguments)] // The exact bounce render contract plus its optional host.
pub(crate) fn render(
    path: &Path,
    revision: BounceRevision,
    isolated_project: Option<&Project>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    include_tails: bool,
    control: &hex_export::ExportControl,
    bouncer: Option<StreamedBouncer>,
    foreign: Option<&mut dyn ForeignBounceRenderer>,
) -> Result<u64, String> {
    if revision
        .prepared
        .arrangement_stream_sources()
        .next()
        .is_some()
        && let Some(bouncer) = bouncer
    {
        return bouncer(
            &StreamedBounceRequest {
                prepared: &revision.prepared,
                resources,
                files: &revision.files,
                repitches: revision
                    .file_sources
                    .as_ref()
                    .map_or(&[], |files| files.repitches().requirements()),
                granulars: revision
                    .file_sources
                    .as_ref()
                    .map_or(&[], |files| files.granulars().requirements()),
                isolated_project,
                path,
                start,
                frames,
                include_tails,
                control,
            },
            foreign,
        )
        .map(|report| report.frames);
    }
    if let Some(foreign) = foreign {
        return hex_export::render_arrangement_bounce_with_foreign(
            path,
            revision.prepared,
            resources,
            start,
            frames,
            include_tails,
            control,
            None,
            foreign,
        )
        .map(|report| report.frames)
        .map_err(|error| error.to_string());
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
