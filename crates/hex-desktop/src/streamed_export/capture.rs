//! Bounded file capture, storage quoting, failure mapping, and worker startup
//! for offline `VerifiedPages`.
use super::{
    POLL, PREPARATION_TIMEOUT,
    derived::{Cancellation, DerivedCapture, check, media},
    pages::VerifiedPages,
};
use hex_engine::{StreamPageExtent, StreamSourceKey};
use hex_export::ExportError;
use hex_session::{AudioSourceBinding, VerifiedAudioFile, service::StreamedExportRequest};
use hex_stream::{
    PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind, StreamRuntime,
};
use std::{sync::atomic::AtomicBool, time::Instant};

impl VerifiedPages {
    pub(super) fn capture(
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
            DerivedCapture::Canonical(request.source()),
            Some(request.path()),
            capture_config,
            request.control(),
        )?;
        drop(requirements);
        pages.config = config;
        Ok(pages)
    }

    pub(super) fn capture_files<'a>(
        prepared: &hex_engine::PreparedRevision,
        mut file_for: impl FnMut(StreamSourceKey) -> Result<&'a VerifiedAudioFile, ExportError>,
        repitches: &[hex_engine::audio_transform::StreamedRepitchRequirement],
        derived: DerivedCapture<'_>,
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
            let is_repitch = metadata.key().repitch_identity().is_some();
            let is_granular = metadata.key().granular_identity().is_some();
            let expected = match metadata.key().pitch() {
                Some((cents, law)) => original
                    .with_plain_pitch(cents, law)
                    .map_err(ExportError::Render)?,
                None if metadata.key().is_raw() => original.clone(),
                None if is_repitch && metadata.is_repitch_of(&original) => metadata.clone(),
                None if is_repitch => {
                    return Err(ExportError::Invalid(
                        "verified export file facts differ from compiled source",
                    ));
                }
                None if is_granular => metadata.clone(),
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
                        let root = derived.granular_root(metadata, file, control)?;
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
                        let definition = if is_repitch {
                            let available = config
                                .max_kernel_table_bytes
                                .checked_sub(root_slots)
                                .and_then(|left| left.checked_sub(granular_map_bytes))
                                .and_then(|left| left.checked_sub(repitch_map_bytes))
                                .ok_or(ExportError::Invalid("repitch map reservation exceeded"))?;
                            let (definition, bytes) = derived.repitch_definition(
                                metadata, original, file, repitches, available, control,
                            )?;
                            repitch_map_bytes += bytes;
                            definition
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

    pub(super) fn storage_bytes(&self) -> Result<usize, ExportError> {
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

    pub(super) fn failure(
        &self,
        failure: StreamFailure,
        control: &impl Cancellation,
    ) -> ExportError {
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

    pub(super) fn ensure_runtime(
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
