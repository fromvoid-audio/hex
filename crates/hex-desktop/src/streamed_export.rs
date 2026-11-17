//! Independent offline source verification/pages, never native admission.
//!
//! One existing Session export job owns this control-thread facade, fixed pool
//! and joining worker. Native output/domain/catalog/readiness tokens are not
//! borrowed. Optional job-owned ordinary resources are forwarded as the exact
//! borrowed compiler scope. Core export owns DSP, cancellation/progress and
//! file publication; page preparation never transfers resource custody.
//! Pool/catalog/reader/kernel bounds are the existing StreamConfig limits; no
//! full-file resident PCM fallback or process-RSS/deadline claim is made.
use hex_engine::{
    BorrowedMediaPage, MediaPages, SessionPlayback, StreamPageExtent, StreamSourceKey,
};
use hex_export::{
    ExportControl, ExportError, ExportReport, ForeignBounceRenderer, PreparedExportPages,
};
use hex_session::{AudioSourceBinding, VerifiedAudioFile, service::StreamedExportRequest};
use hex_stream::{
    PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind, StreamRuntime,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const PREPARATION_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(5);
// Page replies wake the offline consumer. The timeout bounds cancellation
// and fault checks even if no reply arrives.
const PAGE_POLL: Duration = Duration::from_millis(1);

pub(super) fn render(
    request: &StreamedExportRequest<'_>,
) -> Result<ExportReport, hex_session::service::ExportFailure> {
    render_with_config(request, StreamConfig::initial()).map_err(Into::into)
}

pub(crate) fn render_with_config(
    request: &StreamedExportRequest<'_>,
    config: StreamConfig,
) -> Result<ExportReport, ExportError> {
    render_with_config_and_foreign(request, config, request.take_foreign())
}

/// The job retains exclusive custody of the supplied offline processors; the
/// independent verified page worker and WAV publisher retain their own owners.
pub(crate) fn render_with_config_and_foreign(
    request: &StreamedExportRequest<'_>,
    config: StreamConfig,
    foreign: Option<&mut dyn ForeignBounceRenderer>,
) -> Result<ExportReport, ExportError> {
    check(request.control())?;
    if foreign.is_some() && !matches!(request.signal(), hex_export::ExportSignal::Main) {
        return Err(ExportError::Invalid(
            "offline vst3 export supports main only",
        ));
    }
    let mut pages = VerifiedPages::capture(request, config)?;
    if let Some(foreign) = foreign {
        return hex_export::render_linear_main_with_foreign(
            request.path(),
            Arc::clone(request.prepared()),
            request.resources(),
            request.start(),
            request.frames(),
            request.format(),
            request.control(),
            request.publication(),
            Some(&mut pages),
            foreign,
        );
    }
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
}
/// [`hex_session::StreamedBouncer`]: one isolated bounce item through the same
/// independent verification, bounded pool and joining worker as export. File
/// facts come from the document's captured bindings; the worker is joined and
/// every file re-hashed before the bounce file is published. Nothing native is
/// borrowed, so a bounce never waits on or disturbs the playing transport.
pub(super) fn bounce(
    request: &hex_session::StreamedBounceRequest<'_>,
    foreign: Option<&mut dyn ForeignBounceRenderer>,
) -> Result<ExportReport, String> {
    bounce_with_config(request, StreamConfig::initial(), foreign)
        .map_err(|error| hex_session::service::export_error_text(&error))
}

pub(crate) fn bounce_with_config(
    request: &hex_session::StreamedBounceRequest<'_>,
    config: StreamConfig,
    foreign: Option<&mut dyn ForeignBounceRenderer>,
) -> Result<ExportReport, ExportError> {
    check(request.control())?;
    let mut pages = VerifiedPages::capture_files(
        request.prepared(),
        |key| {
            request.file(key.asset()).ok_or(ExportError::Invalid(
                "verified bounce source binding is missing",
            ))
        },
        request.repitches(),
        GranularCapture::Captured(request.granulars()),
        Some(request.path()),
        config,
        request.control(),
    )?;
    if let Some(foreign) = foreign {
        return hex_export::render_arrangement_bounce_with_foreign(
            request.path(),
            Arc::clone(request.prepared()),
            request.resources(),
            request.start(),
            request.frames(),
            request.include_tails(),
            request.control(),
            Some(&mut pages),
            foreign,
        );
    }
    hex_export::render_arrangement_bounce_with_pages(
        request.path(),
        Arc::clone(request.prepared()),
        request.resources(),
        request.start(),
        request.frames(),
        request.include_tails(),
        request.control(),
        Some(&mut pages),
    )
}
trait Cancellation {
    fn is_cancelled(&self) -> bool;
}
impl Cancellation for ExportControl {
    fn is_cancelled(&self) -> bool {
        ExportControl::is_cancelled(self)
    }
}
impl Cancellation for AtomicBool {
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Acquire)
    }
}
fn check(control: &impl Cancellation) -> Result<(), ExportError> {
    if control.is_cancelled() {
        Err(ExportError::Cancelled)
    } else {
        Ok(())
    }
}
fn media(error: impl std::fmt::Display) -> ExportError {
    ExportError::Media(error.to_string())
}

/// Canonical export and retained ordinary inputs share exact root projection;
/// neither form authorizes readers, workers, native domains or page loans.
#[derive(Clone, Copy)]
enum GranularCapture<'a> {
    Canonical(&'a hex_session::service::CanonicalAudioSourceSnapshot),
    Captured(&'a [hex_engine::audio_transform::StreamedGranularRequirement]),
}
impl GranularCapture<'_> {
    fn root_for(
        &self,
        expected: &hex_engine::StreamedSourceMetadata,
        file: &VerifiedAudioFile,
        control: &impl Cancellation,
    ) -> Result<hex_engine::audio_transform::StreamedGranularRoot, ExportError> {
        let raw = file
            .source_metadata(expected.key().asset(), expected.key().generation())
            .map_err(ExportError::Render)?;
        if !expected.is_granular_of(&raw) {
            return Err(ExportError::Invalid("granular export source facts differ"));
        }
        match self {
            Self::Canonical(source) => {
                for result in source.required_granular_roots(expected.key().generation()) {
                    check(control)?;
                    let (candidate, root) = result.map_err(ExportError::Render)?;
                    if candidate == file && root.metadata() == expected {
                        return Ok(root);
                    }
                }
            }
            Self::Captured(requirements) => {
                for requirement in *requirements {
                    check(control)?;
                    if requirement
                        .metadata_for(&raw)
                        .is_ok_and(|metadata| &metadata == expected)
                    {
                        return requirement
                            .try_owned_root(&raw, &AtomicBool::new(false))
                            .map_err(ExportError::Render);
                    }
                }
            }
        }
        Err(ExportError::Invalid(
            "verified granular requirement missing or differs",
        ))
    }
}

struct VerifiedPages {
    config: StreamConfig,
    definitions: Option<Vec<SourceDefinition>>,
    granulars: Option<Vec<hex_engine::audio_transform::StreamedGranularRoot>>,
    files: Vec<(StreamSourceKey, VerifiedAudioFile)>,
    runtime: Option<StreamRuntime>,
    extents: Vec<StreamPageExtent>,
    keys: Vec<PageKey>,
    missed: bool,
}
impl VerifiedPages {
    fn capture(
        request: &StreamedExportRequest<'_>,
        config: StreamConfig,
    ) -> Result<Self, ExportError> {
        if request.source().sample_rate() != request.prepared().arrangement().sample_rate() {
            return Err(ExportError::Invalid(
                "verified export source context differs",
            ));
        }
        // The snapshot is the canonical owner for offline export; retain only
        // bounded exact maps while definitions are constructed. They are
        // dropped before the independent worker starts.
        let needed = |asset| {
            request
                .prepared()
                .arrangement_stream_sources()
                .any(|metadata| {
                    metadata.key().asset() == asset && metadata.key().repitch_identity().is_some()
                })
        };
        let count = request
            .source()
            .required_repitch_sources()
            .filter(|(asset, _, _, _)| needed(*asset))
            .count();
        let mut requirements = Vec::new();
        let mut keys = Vec::new();
        let mut retained = 0usize;
        let mut capture_config = config;
        if count != 0 {
            if config.max_sources == 0 || config.max_sources > StreamConfig::initial().max_sources {
                return Err(ExportError::Invalid("invalid offline streaming bounds"));
            }
            let capacity = count.min(config.max_sources);
            let mut handles = capacity
                .checked_mul(
                    std::mem::size_of::<hex_engine::audio_transform::StreamedRepitchRequirement>()
                        + std::mem::size_of::<StreamSourceKey>(),
                )
                .and_then(|bytes| {
                    bytes.checked_add(
                        std::mem::size_of_val(&requirements) + std::mem::size_of_val(&keys),
                    )
                })
                .ok_or(ExportError::Invalid(
                    "repitch requirement reservation overflow",
                ))?;
            let workspace = hex_engine::audio_transform::native_metadata_workspace_bytes()
                .ok_or(ExportError::Invalid("repitch workspace overflow"))?;
            if handles
                .checked_add(workspace)
                .is_none_or(|bytes| bytes > config.max_kernel_table_bytes)
            {
                return Err(ExportError::Invalid("repitch map reservation exceeded"));
            }
            requirements.try_reserve_exact(capacity).map_err(media)?;
            keys.try_reserve_exact(capacity).map_err(media)?;
            handles = requirements
                .capacity()
                .checked_mul(std::mem::size_of::<
                    hex_engine::audio_transform::StreamedRepitchRequirement,
                >())
                .and_then(|bytes| {
                    keys.capacity()
                        .checked_mul(std::mem::size_of::<StreamSourceKey>())
                        .and_then(|keys| bytes.checked_add(keys))
                })
                .and_then(|bytes| {
                    bytes.checked_add(
                        std::mem::size_of_val(&requirements) + std::mem::size_of_val(&keys),
                    )
                })
                .ok_or(ExportError::Invalid(
                    "repitch requirement reservation overflow",
                ))?;
            for (asset, file, edit, tempo) in request
                .source()
                .required_repitch_sources()
                .filter(|(asset, _, _, _)| needed(*asset))
            {
                check(request.control())?;
                if handles
                    .checked_add(retained)
                    .and_then(|bytes| bytes.checked_add(workspace))
                    .is_none_or(|bytes| bytes > config.max_kernel_table_bytes)
                {
                    return Err(ExportError::Invalid("repitch map reservation exceeded"));
                }
                let raw = file
                    .source_metadata(asset, 1)
                    .map_err(ExportError::Render)?;
                let requirement = hex_engine::audio_transform::StreamedRepitchRequirement::new(
                    &raw,
                    edit,
                    tempo,
                    &AtomicBool::new(false),
                )
                .map_err(ExportError::Render)?;
                check(request.control())?;
                let key = requirement
                    .metadata_for(&raw)
                    .map_err(ExportError::Render)?
                    .key();
                let mut used = false;
                for metadata in request.prepared().arrangement_stream_sources() {
                    if metadata.key().asset() != asset
                        || metadata.key().repitch_identity().is_none()
                    {
                        continue;
                    }
                    let source = file
                        .source_metadata(asset, metadata.key().generation())
                        .map_err(ExportError::Render)?;
                    if requirement
                        .metadata_for(&source)
                        .is_ok_and(|expected| expected == *metadata)
                    {
                        used = true;
                        break;
                    }
                }
                if !used {
                    continue;
                }
                if let Err(index) = keys.binary_search(&key) {
                    if requirements.len() == capacity {
                        return Err(ExportError::Invalid(
                            "offline export source count exceeds its bound",
                        ));
                    }
                    retained = retained
                        .checked_add(
                            requirement
                                .retained_bytes()
                                .ok_or(ExportError::Invalid("repitch map size overflow"))?,
                        )
                        .ok_or(ExportError::Invalid("repitch map size overflow"))?;
                    if handles
                        .checked_add(retained)
                        .and_then(|bytes| bytes.checked_add(workspace))
                        .is_none_or(|bytes| bytes > config.max_kernel_table_bytes)
                    {
                        return Err(ExportError::Invalid("repitch map reservation exceeded"));
                    }
                    keys.insert(index, key);
                    requirements.push(requirement);
                }
            }
            drop(keys);
            let retained_handles = requirements
                .capacity()
                .checked_mul(std::mem::size_of::<
                    hex_engine::audio_transform::StreamedRepitchRequirement,
                >())
                .and_then(|bytes| bytes.checked_add(std::mem::size_of_val(&requirements)))
                .ok_or(ExportError::Invalid(
                    "repitch requirement reservation overflow",
                ))?;
            capture_config.max_kernel_table_bytes = config
                .max_kernel_table_bytes
                .checked_sub(
                    retained
                        .checked_add(retained_handles)
                        .ok_or(ExportError::Invalid("repitch map size overflow"))?,
                )
                .ok_or(ExportError::Invalid("repitch map reservation exceeded"))?;
        }
        let mut pages = Self::capture_files(
            request.prepared(),
            |key| match request.source().sources().get(&key.asset()) {
                Some(AudioSourceBinding::VerifiedFile(file)) => Ok(file),
                Some(_) => Err(ExportError::Invalid(
                    "verified export requires exact file facts",
                )),
                None => Err(ExportError::Invalid(
                    "verified export source binding is missing",
                )),
            },
            &requirements,
            GranularCapture::Canonical(request.source()),
            Some(request.path()),
            capture_config,
            request.control(),
        )?;
        drop(requirements);
        pages.config = config;
        Ok(pages)
    }
    fn capture_files<'a>(
        prepared: &hex_engine::PreparedRevision,
        mut file_for: impl FnMut(StreamSourceKey) -> Result<&'a VerifiedAudioFile, ExportError>,
        repitches: &[hex_engine::audio_transform::StreamedRepitchRequirement],
        granular_capture: GranularCapture<'_>,
        destination: Option<&std::path::Path>,
        config: StreamConfig,
        control: &impl Cancellation,
    ) -> Result<Self, ExportError> {
        if prepared.scalar_automation_binding().is_some() {
            return Err(ExportError::Invalid(
                "offline scalar source capture is not prepared",
            ));
        }
        // These private composition inputs must be bounded before reserving Vecs.
        // The original stream constructor still validates its complete configuration.
        let maximum = StreamConfig::initial();
        if config.page_buffers < 2
            || config.page_buffers > maximum.page_buffers
            || config.max_sources == 0
            || config.max_sources > maximum.max_sources
        {
            return Err(ExportError::Invalid("invalid offline streaming bounds"));
        }
        // Overwrite consent concerns an export, never an imported source. Resolve
        // existing paths so an alias through a symlink cannot replace the media.
        let destination = match destination.map(std::fs::canonicalize) {
            Some(Ok(path)) => Some(path),
            Some(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
            Some(Err(error)) => return Err(media(error)),
            None => None,
        };
        let mut definitions: Vec<SourceDefinition> = Vec::new();
        let mut files: Vec<(StreamSourceKey, VerifiedAudioFile)> = Vec::new();
        let mut granulars: Vec<hex_engine::audio_transform::StreamedGranularRoot> = Vec::new();
        let has_granular = prepared
            .arrangement_stream_sources()
            .any(|m| m.key().granular_identity().is_some());
        let root_slots = if has_granular {
            if config.page_buffers < 3 {
                return Err(ExportError::Invalid(
                    "granular export requires three pool pages",
                ));
            }
            let bytes = config
                .max_sources
                .checked_mul(std::mem::size_of::<
                    hex_engine::audio_transform::StreamedGranularRoot,
                >())
                .ok_or(ExportError::Invalid("granular root slots overflow"))?;
            if bytes > config.max_kernel_table_bytes {
                return Err(ExportError::Invalid(
                    "granular metadata reservation exceeded",
                ));
            }
            granulars
                .try_reserve_exact(config.max_sources)
                .map_err(media)?;
            granulars
                .capacity()
                .checked_mul(std::mem::size_of::<
                    hex_engine::audio_transform::StreamedGranularRoot,
                >())
                .ok_or(ExportError::Invalid("granular root slots overflow"))?
        } else {
            0
        };
        let mut granular_map_bytes = 0usize;
        definitions
            .try_reserve_exact(config.max_sources)
            .map_err(media)?;
        files.try_reserve_exact(config.max_sources).map_err(media)?;
        let mut repitch_map_bytes = 0usize;
        for metadata in prepared.arrangement_stream_sources() {
            check(control)?;
            let file = file_for(metadata.key())?;
            if let Some(destination) = &destination {
                let source = std::fs::canonicalize(file.path()).map_err(media)?;
                if &source == destination {
                    return Err(ExportError::Invalid(
                        "export destination is original media; choose another path",
                    ));
                }
            }
            let original = file
                .source_metadata(metadata.key().asset(), metadata.key().generation())
                .map_err(ExportError::Render)?;
            let mut repitch = None;
            let mut is_granular = false;
            let expected = match metadata.key().pitch() {
                Some((cents, law)) => original
                    .with_plain_pitch(cents, law)
                    .map_err(ExportError::Render)?,
                None if metadata.key().is_raw() => original.clone(),
                None if metadata.key().repitch_identity().is_some() => {
                    let requirement = repitches
                        .iter()
                        .find(|requirement| {
                            requirement.asset() == metadata.key().asset()
                                && requirement
                                    .metadata_for(&original)
                                    .is_ok_and(|expected| expected == *metadata)
                        })
                        .ok_or(ExportError::Invalid(
                            "verified repitch requirement missing or differs",
                        ))?;
                    repitch = Some(requirement);
                    requirement
                        .metadata_for(&original)
                        .map_err(ExportError::Render)?
                }
                None if metadata.key().granular_identity().is_some() => {
                    is_granular = true;
                    metadata.clone()
                }
                None => return Err(ExportError::Invalid("unsupported derived export source")),
            };
            if expected != *metadata {
                return Err(ExportError::Invalid(
                    "verified export file facts differ from compiled source",
                ));
            }
            match files.binary_search_by_key(&metadata.key(), |(key, _)| *key) {
                Ok(index) => {
                    let known = definitions
                        .iter()
                        .any(|definition| definition.metadata == *metadata)
                        || granulars.iter().any(|root| root.metadata() == metadata);
                    if !known || files[index].1 != *file {
                        return Err(ExportError::Invalid("conflicting compiled export source"));
                    }
                }
                Err(index) => {
                    if files.len() == config.max_sources {
                        return Err(ExportError::Invalid(
                            "offline export source count exceeds its bound",
                        ));
                    }
                    if is_granular {
                        let workspace =
                            hex_engine::audio_transform::native_metadata_workspace_bytes()
                                .ok_or(ExportError::Invalid("granular workspace overflow"))?;
                        if root_slots
                            .checked_add(granular_map_bytes)
                            .and_then(|bytes| bytes.checked_add(repitch_map_bytes))
                            .and_then(|bytes| bytes.checked_add(workspace))
                            .is_none_or(|bytes| bytes > config.max_kernel_table_bytes)
                        {
                            return Err(ExportError::Invalid(
                                "granular metadata reservation exceeded",
                            ));
                        }
                        let root = granular_capture.root_for(metadata, file, control)?;
                        granular_map_bytes = granular_map_bytes
                            .checked_add(
                                root.backing_bytes()
                                    .ok_or(ExportError::Invalid("granular map size overflow"))?,
                            )
                            .ok_or(ExportError::Invalid("granular map size overflow"))?;
                        if root_slots
                            .checked_add(granular_map_bytes)
                            .and_then(|bytes| bytes.checked_add(repitch_map_bytes))
                            .is_none_or(|bytes| bytes > config.max_kernel_table_bytes)
                        {
                            return Err(ExportError::Invalid(
                                "granular metadata reservation exceeded",
                            ));
                        }
                        granulars.push(root);
                    } else {
                        let definition = if let Some(requirement) = repitch {
                            repitch_map_bytes = repitch_map_bytes
                                .checked_add(
                                    requirement
                                        .map()
                                        .backing_bytes()
                                        .ok_or(ExportError::Invalid("repitch map size overflow"))?,
                                )
                                .ok_or(ExportError::Invalid("repitch map size overflow"))?;
                            if repitch_map_bytes
                                .checked_add(granular_map_bytes)
                                .and_then(|bytes| bytes.checked_add(root_slots))
                                .is_none_or(|bytes| bytes > config.max_kernel_table_bytes)
                            {
                                return Err(ExportError::Invalid(
                                    "repitch map reservation exceeded",
                                ));
                            }
                            let map = requirement.try_owned_map().map_err(ExportError::Render)?;
                            check(control)?;
                            SourceDefinition::new(
                                file.path().to_path_buf(),
                                original,
                                file.reader_limits(),
                            )
                            .with_repitch(map, requirement.law())
                            .map_err(ExportError::Render)?
                        } else {
                            SourceDefinition::new(
                                file.path().to_path_buf(),
                                metadata.clone(),
                                file.reader_limits(),
                            )
                        }
                        .with_verified_manifest(file.verified_manifest().clone());
                        let at = definitions
                            .partition_point(|definition| definition.key < metadata.key());
                        definitions.insert(at, definition);
                    }
                    files.insert(index, (metadata.key(), file.clone()));
                }
            }
        }
        let mut extents = Vec::new();
        let mut keys = Vec::new();
        extents
            .try_reserve_exact(config.page_buffers)
            .map_err(media)?;
        keys.try_reserve_exact(config.page_buffers).map_err(media)?;
        Ok(Self {
            config,
            definitions: Some(definitions),
            granulars: Some(granulars),
            files,
            runtime: None,
            extents,
            keys,
            missed: false,
        })
    }
    fn storage_bytes(&self) -> Result<usize, ExportError> {
        let definitions = self
            .definitions
            .as_ref()
            .ok_or(ExportError::Invalid("offline definitions missing"))?;
        let bound = hex_stream::PendingStreamDomain::preparation_bound(
            self.config,
            definitions,
            definitions.capacity(),
            None,
        )
        .map_err(media)?;
        let roots = self
            .granulars
            .as_ref()
            .ok_or(ExportError::Invalid("offline roots missing"))?;
        let adapter = std::mem::size_of::<Self>()
            .checked_add(
                self.extents
                    .capacity()
                    .checked_mul(std::mem::size_of::<StreamPageExtent>())
                    .ok_or(ExportError::Invalid("seek extent quote overflow"))?,
            )
            .and_then(|bytes| {
                bytes.checked_add(
                    self.keys
                        .capacity()
                        .checked_mul(std::mem::size_of::<PageKey>())?,
                )
            })
            .and_then(|bytes| {
                bytes.checked_add(
                    self.files
                        .capacity()
                        .checked_mul(std::mem::size_of::<(StreamSourceKey, VerifiedAudioFile)>())?,
                )
            })
            .and_then(|bytes| {
                bytes.checked_add(roots.capacity().checked_mul(std::mem::size_of::<
                    hex_engine::audio_transform::StreamedGranularRoot,
                >())?)
            })
            .and_then(|bytes| {
                roots.iter().try_fold(bytes, |bytes, root| {
                    bytes.checked_add(root.backing_bytes()?)
                })
            })
            .ok_or(ExportError::Invalid("seek adapter quote overflow"))?;
        let extra = if roots.is_empty() {
            0
        } else {
            // Bound the shared constructor's listed per-source obligations from
            // captured facts, without opening readers. Its processing cap is
            // 32MiB; the public pure quote covers temporary construction too.
            let mut bank = roots
                .len()
                .checked_mul(std::mem::size_of::<hex_stream::VerifiedGranularSource>())
                .ok_or(ExportError::Invalid("granular source quote overflow"))?;
            let mut overlap = 0usize;
            for root in roots {
                let index = self
                    .files
                    .binary_search_by_key(&root.metadata().key(), |(key, _)| *key)
                    .map_err(|_| ExportError::Invalid("granular quote source missing"))?;
                let file = &self.files[index].1;
                let reader = file
                    .path()
                    .as_os_str()
                    .len()
                    .checked_add(
                        file.reader_limits()
                            .max_manifest_bytes
                            .min(self.config.max_manifest_bytes),
                    )
                    .and_then(|bytes| {
                        bytes.checked_add(hex_media::VerifiedWavManifest::owner_payload_bytes())
                    })
                    .and_then(|bytes| {
                        bytes.checked_add(hex_media::ResampleKernel::owner_payload_bytes())
                    })
                    .and_then(|bytes| {
                        bytes.checked_add(hex_media::ResampleKernel::max_table_payload_bytes())
                    })
                    .and_then(|bytes| bytes.checked_add(root.backing_bytes()?))
                    .ok_or(ExportError::Invalid("granular reader quote overflow"))?;
                let processing = 32 * 1024 * 1024;
                let limits = hex_stream::GranularSourcePreparationLimits {
                    source_scratch_frames: self.config.source_scratch_frames,
                    checkpoint_entries: 8,
                    checkpoint_budget: 16 * 1024 * 1024,
                    max_processing_bytes: processing,
                    max_construction_peak_bytes: 0,
                };
                let peak = limits
                    .construction_processing_bound(root.metadata().project_rate())
                    .map_err(|error| media(format!("{error:?}")))?;
                bank = bank
                    .checked_add(reader)
                    .and_then(|bytes| bytes.checked_add(processing))
                    .ok_or(ExportError::Invalid("granular bank quote overflow"))?;
                overlap = overlap.max(peak.saturating_sub(processing));
            }
            bank.checked_add(overlap)
                .map(|bytes| {
                    bytes.min(crate::audio_policy::GranularPolicy::initial().max_construction_bytes)
                })
                .and_then(|bytes| bytes.checked_add(16 * 1024 * 1024))
                .ok_or(ExportError::Invalid("granular construction quote overflow"))?
        };
        bound
            .construction_peak_data_bytes
            .checked_add(adapter)
            .and_then(|bytes| bytes.checked_add(extra))
            .ok_or(ExportError::Invalid("seek page quote overflow"))
    }
    fn failure(&self, failure: StreamFailure, control: &impl Cancellation) -> ExportError {
        if control.is_cancelled() {
            return ExportError::Cancelled;
        }
        let reason = match failure.kind {
            StreamFailureKind::SourceChanged => "original media changed; reopen or relink it",
            StreamFailureKind::Cancelled => "verified media worker cancelled its read",
            StreamFailureKind::DecodeFailed => {
                "original media could not be read; check its format and access"
            }
            StreamFailureKind::UnknownSource => {
                "compiled media source is not in the captured catalog"
            }
            StreamFailureKind::CapacityExhausted => {
                "offline media preparation exceeds its fixed resources"
            }
        };
        match self.files.binary_search_by_key(&failure.key, |(key, _)| {
            hex_engine::StreamResourceKey::Audio(*key)
        }) {
            Ok(index) => ExportError::Media(format!(
                "{reason}: {}",
                self.files[index].1.path().display()
            )),
            Err(_) => ExportError::Media(reason.into()),
        }
    }
    fn ensure_runtime(
        &mut self,
        control: &impl Cancellation,
        required: usize,
    ) -> Result<(), ExportError> {
        if self.runtime.is_some() {
            return Ok(());
        }
        let definitions = self.definitions.take().ok_or(ExportError::Invalid(
            "offline stream constructor was already consumed",
        ))?;
        let granulars = self
            .granulars
            .take()
            .ok_or(ExportError::Invalid("offline roots already consumed"))?;
        let mut config = self.config;
        config.wake_offline_consumer = true;
        self.runtime = Some(if granulars.is_empty() {
            StreamRuntime::new(config, definitions).map_err(media)?
        } else {
            check(control)?;
            let metadata_bytes = granulars
                .capacity()
                .checked_mul(std::mem::size_of::<
                    hex_engine::audio_transform::StreamedGranularRoot,
                >())
                .and_then(|bytes| {
                    granulars.iter().try_fold(bytes, |bytes, root| {
                        bytes.checked_add(root.backing_bytes()?)
                    })
                })
                .ok_or(ExportError::Invalid("granular metadata size overflow"))?;
            let mut policy = crate::audio_policy::GranularPolicy::initial();
            policy.max_construction_bytes = policy
                .max_construction_bytes
                .checked_sub(metadata_bytes)
                .ok_or(ExportError::Invalid(
                    "granular construction reservation exceeded",
                ))?;
            let count = granulars.len();
            let roots = granulars.into_iter().map(|root| {
                let index = self
                    .files
                    .binary_search_by_key(&root.metadata().key(), |(key, _)| *key)
                    .map_err(|_| "granular source file missing".to_owned())?;
                Ok((&self.files[index].1, root))
            });
            let bank = crate::granular_sources::prepare_granular_bank(
                count,
                roots,
                self.config,
                policy,
                || control.is_cancelled(),
            )
            .map_err(media)?
            .ok_or(ExportError::Invalid("offline granular bank missing"))?;
            let ordinary = hex_stream::PendingStreamDomain::preparation_bound(
                self.config,
                &definitions,
                definitions.capacity(),
                None,
            )
            .map_err(media)?;
            let combined =
                hex_stream::PendingStreamDomain::preparation_bound_with_granular_sources(
                    self.config,
                    &definitions,
                    definitions.capacity(),
                    &[],
                    0,
                    &bank,
                    None,
                )
                .map_err(media)?;
            if combined.retained_data_bytes
                > ordinary
                    .retained_data_bytes
                    .checked_add(policy.runtime_allowance())
                    .ok_or(ExportError::Invalid("granular runtime quote overflow"))?
                || combined.construction_peak_data_bytes
                    > ordinary
                        .construction_peak_data_bytes
                        .checked_add(policy.runtime_allowance())
                        .ok_or(ExportError::Invalid("granular runtime quote overflow"))?
            {
                return Err(ExportError::Invalid(
                    "granular runtime reservation exceeded",
                ));
            }
            let worker =
                hex_stream::GranularWorkerControl::data_bound(&bank, self.config.page_buffers)
                    .map_err(media)?;
            let allowance = hex_stream::GranularWorkerAllowance {
                max_data_bytes: worker
                    .bytes()
                    .ok_or(ExportError::Invalid("granular worker quote overflow"))?,
                max_shared_handles: worker
                    .shared_handle_bound()
                    .ok_or(ExportError::Invalid("granular handle quote overflow"))?,
            };
            StreamRuntime::spawn_with_granular_sources(
                config,
                definitions,
                Vec::new(),
                bank,
                allowance,
            )
            .map_err(media)?
        });

        if self
            .runtime
            .as_ref()
            .map_or(0, StreamRuntime::page_capacity)
            < required
        {
            return Err(ExportError::Invalid(
                "offline export page demand exceeds simultaneous cache capacity",
            ));
        }
        let deadline = Instant::now() + PREPARATION_TIMEOUT;
        loop {
            check(control)?;
            let result = self
                .runtime
                .as_mut()
                .ok_or(ExportError::Invalid("offline stream worker is unavailable"))?
                .wait_until_ready(POLL);
            match result {
                Ok(()) => return Ok(()),
                Err(hex_stream::StreamRuntimeError::CatalogNotReady)
                    if Instant::now() < deadline => {}
                Err(hex_stream::StreamRuntimeError::CatalogFailed(kind)) => {
                    let key = self.keys.first().map(|key| key.source);
                    if let Some(key) = key {
                        let mut failures = [StreamFailure {
                            key: key.into(),
                            page_index: 0,
                            kind,
                        }];
                        if self
                            .runtime
                            .as_mut()
                            .is_some_and(|runtime| runtime.take_failures(&mut failures) != 0)
                        {
                            return Err(self.failure(failures[0], control));
                        }
                    }
                    return Err(ExportError::Media(
                        match kind {
                            StreamFailureKind::SourceChanged => {
                                "original media changed; reopen or relink it"
                            }
                            StreamFailureKind::DecodeFailed => {
                                "original media could not be verified; check format and access"
                            }
                            StreamFailureKind::Cancelled => {
                                "original media verification was cancelled"
                            }
                            StreamFailureKind::UnknownSource => "original media source is unknown",
                            StreamFailureKind::CapacityExhausted => {
                                "original media exceeds bounded preparation resources"
                            }
                        }
                        .into(),
                    ));
                }
                Err(hex_stream::StreamRuntimeError::CatalogNotReady) => {
                    return Err(ExportError::Media(
                        "media verification timed out; check the source disk and retry".into(),
                    ));
                }
                Err(error) => return Err(media(error)),
            }
        }
    }
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
    fn prepare_stream_chunk(
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
}
impl VerifiedPages {
    fn finish_pages(&mut self, control: &impl Cancellation) -> Result<(), ExportError> {
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

fn seek_error(error: ExportError) -> hex_engine::ArrangementPrerollError {
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
/// Quote the original stream constructor's listed data before any pool/thread
/// exists, including this adapter's bounded metadata. No source I/O occurs here.
pub(super) fn resident_seek_storage_bytes(
    prepared: &hex_engine::PreparedRevision,
    sources: &hex_session::service::ResidentFileSet,
) -> Result<usize, String> {
    let pages = VerifiedPages::capture_files(
        prepared,
        |key| {
            sources
                .files()
                .iter()
                .find(|(asset, _)| *asset == key.asset())
                .map(|(_, file)| file)
                .ok_or(ExportError::Invalid(
                    "verified seek source binding is missing",
                ))
        },
        sources.repitches().requirements(),
        GranularCapture::Captured(sources.granulars().requirements()),
        None,
        StreamConfig::initial(),
        &AtomicBool::new(false),
    )
    .map_err(|error| error.to_string())?;
    pages.storage_bytes().map_err(|error| error.to_string())
}

/// Run only on the original seek worker. This composes its exact admitted
/// ordinary resource scope with this adapter's independent bounded page owner.
/// All readers/pages are joined before returning to ordinary native publication.
pub(super) fn prepare_resident_seek(
    request: hex_session::service::ResidentFileSeekRequest<'_>,
) -> Result<hex_engine::playback::Playback, hex_engine::ArrangementPrerollError> {
    let runtime = prepare_file_seek_runtime(
        request.spec(),
        request.resources(),
        request.sources().context(),
        |key| {
            request
                .sources()
                .files()
                .iter()
                .find(|(asset, _)| *asset == key.asset())
                .map(|(_, file)| file)
                .ok_or(ExportError::Invalid(
                    "verified seek source binding is missing",
                ))
        },
        request.sources().repitches().requirements(),
        request.sources().granulars().requirements(),
        request.cancelled(),
    )?;
    hex_engine::playback::Playback::from_interactive_arrangement_preroll(runtime)
}

// Borrowed factual inputs only: the request's canonical custody and native
// publication remain with their owners. This helper joins all preparation pages.
fn prepare_file_seek_runtime<'a>(
    spec: &hex_session::arrangement_seek::ArrangementSeekSpec,
    resources: &hex_engine::native_device_resources::PreparedDeviceResources,
    context: hex_engine::AudioSourceContext,
    resolve: impl FnMut(StreamSourceKey) -> Result<&'a VerifiedAudioFile, ExportError>,
    repitches: &[hex_engine::audio_transform::StreamedRepitchRequirement],
    granulars: &[hex_engine::audio_transform::StreamedGranularRequirement],
    cancelled: &AtomicBool,
) -> Result<SessionPlayback, hex_engine::ArrangementPrerollError> {
    if context.project_generation != spec.generation {
        return Err(hex_engine::ArrangementPrerollError::MediaPreparation(
            "file seek source context differs",
        ));
    }
    check(cancelled).map_err(seek_error)?;
    let section = match spec.mode {
        hex_session::arrangement_seek::ArrangementSeekMode::SectionLoop(region) => Some(region),
        hex_session::arrangement_seek::ArrangementSeekMode::Interactive
        | hex_session::arrangement_seek::ArrangementSeekMode::HistoryPreservingInteractive => None,
        _ => {
            return Err(hex_engine::ArrangementPrerollError::MediaPreparation(
                "ordinary seek requires interactive policy",
            ));
        }
    };
    let mut pages = VerifiedPages::capture_files(
        &spec.prepared_revision,
        resolve,
        repitches,
        GranularCapture::Captured(granulars),
        None,
        StreamConfig::initial(),
        cancelled,
    )
    .map_err(seek_error)?;
    let runtime = if spec.mode
        == hex_session::arrangement_seek::ArrangementSeekMode::HistoryPreservingInteractive
    {
        SessionPlayback::prepare_history_preserving_arrangement_preroll_with_pages(
            Arc::clone(&spec.prepared_revision),
            Some(resources),
            spec.destination_frame,
            spec.work_budget_frames,
            spec.start_playing,
            cancelled,
            &mut pages,
        )?
    } else {
        SessionPlayback::prepare_ordinary_arrangement_preroll_with_pages(
            Arc::clone(&spec.prepared_revision),
            resources,
            spec.destination_frame,
            spec.work_budget_frames,
            spec.start_playing,
            section,
            cancelled,
            &mut pages,
        )?
    };
    pages.finish_pages(cancelled).map_err(seek_error)?;
    Ok(runtime)
}

#[cfg(all(test, feature = "allocation-probe"))]
#[path = "streamed_export/test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "streamed_export/tests.rs"]
mod tests;
