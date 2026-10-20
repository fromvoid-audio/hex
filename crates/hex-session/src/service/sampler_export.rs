//! Private Service-owned offline sampler snapshots and their admission union.
//! Neither this wrapper nor a resource-bearing graph is emitted to public APIs.
use super::*;
use hex_engine::native_device_resources::{
    self as resources, ControlSamplerInventory, ControlSamplerPhase, ControlSamplerSelection,
    ControlSamplerSources, StagedNativeAssetView,
};

pub(crate) struct ServiceExportSnapshot {
    pub(crate) prepared: Arc<PreparedRevision>,
    pub(crate) sources: Option<ControlSamplerSources>,
}
impl ServiceExportSnapshot {
    pub(crate) fn selection(&self) -> ControlSamplerSelection<'_> {
        ControlSamplerSelection {
            sources: self.sources.as_ref(),
            prepared: Some(&self.prepared),
        }
    }
}
#[derive(Clone, Copy, Default)]
pub(crate) struct Admission<'a> {
    pub(crate) current: Option<ControlSamplerSelection<'a>>,
    pub(crate) export: Option<ControlSamplerSelection<'a>>,
    pub(crate) runtime_bytes: usize,
    pub(crate) outbound: Option<resources::ControlSamplerOutbound<'a>>,
}
fn legacy(session: &Session) -> Vec<&hex_engine::Audio> {
    session
        .media
        .values()
        .flat_map(|m| [&m.source_audio, &m.audio])
        .collect()
}
impl Admission<'_> {
    /// The real decoded original and derivative exist, but no canonical/media
    /// transaction has begun. Ordinary import adds only legacy PCM to this
    /// source domain; native store/sidecar/factory ownership stays unchanged.
    pub(crate) fn check_legacy_stage(
        &self,
        session: &Session,
        staged: &[PreparedImport],
    ) -> Result<(), SessionError> {
        self.legacy_stage_inventory(session, staged).map(|_| ())
    }
    fn legacy_stage_inventory(
        &self,
        session: &Session,
        staged: &[PreparedImport],
    ) -> Result<Option<ControlSamplerInventory>, SessionError> {
        let Some(limits) = session.native_reservation else {
            return Ok(None);
        };
        let stores = [&session.native_assets];
        let selections = self
            .current
            .into_iter()
            .chain(self.export)
            .collect::<Vec<_>>();
        let outbound = self.outbound.into_iter().collect::<Vec<_>>();
        let legacy = session
            .media
            .values()
            .flat_map(|m| [&m.source_audio, &m.audio])
            .chain(staged.iter().flat_map(|m| [&m.source_audio, &m.audio]))
            .collect::<Vec<_>>();
        let phase = ControlSamplerPhase {
            stores: &stores,
            selections: &selections,
            outbound: &outbound,
            legacy: &legacy,
            checked_runtime_bytes: self.runtime_bytes,
            construction_bytes: 0,
        };
        let inventory = resources::inspect_control_sampler_union(&phase, &phase, limits.storage)?;
        if inventory.retained_frames > session.max_resident_frames {
            return Err(SessionError::Invalid("resident audio frame limit reached"));
        }
        Ok(Some(inventory))
    }
    pub(crate) fn check(
        &self,
        old: &Session,
        future: &Session,
        candidate: ControlSamplerSelection<'_>,
        compiler_bytes: usize,
        stage: Option<&StagedNativeAssetView<'_>>,
    ) -> Result<ControlSamplerInventory, SessionError> {
        let reservation =
            future
                .native_reservation
                .or(old.native_reservation)
                .ok_or(SessionError::Invalid(
                    "native source import requires an explicit resident reservation",
                ))?;
        let kept_selections = std::iter::once(candidate)
            .chain(self.export)
            .collect::<Vec<_>>();
        let peak_selections = kept_selections
            .iter()
            .copied()
            .chain(self.current)
            .collect::<Vec<_>>();
        let kept_stores = [&future.native_assets];
        let peak_stores = if std::ptr::eq(old, future) {
            vec![&future.native_assets]
        } else {
            vec![&future.native_assets, &old.native_assets]
        };
        let kept_legacy = legacy(future);
        let peak_legacy = kept_legacy
            .iter()
            .copied()
            .chain(legacy(old))
            .collect::<Vec<_>>();
        let outbound = self.outbound.into_iter().collect::<Vec<_>>();
        let kept = ControlSamplerPhase {
            stores: &kept_stores,
            selections: &kept_selections,
            outbound: &outbound,
            legacy: &kept_legacy,
            checked_runtime_bytes: self.runtime_bytes,
            construction_bytes: 0,
        };
        let peak = ControlSamplerPhase {
            stores: &peak_stores,
            selections: &peak_selections,
            outbound: &outbound,
            legacy: &peak_legacy,
            checked_runtime_bytes: self.runtime_bytes,
            construction_bytes: compiler_bytes,
        };
        let inventory = match stage {
            Some(stage) => stage.inspect(&kept, &peak, reservation.storage),
            None => resources::inspect_control_sampler_union(&kept, &peak, reservation.storage),
        }
        .map_err(SessionError::from)?;
        if inventory.retained_frames > future.max_resident_frames {
            return Err(SessionError::Invalid("resident audio frame limit reached"));
        }
        Ok(inventory)
    }
}
impl Session {
    pub(crate) fn prepare_service_snapshot(
        &self,
        project: &Project,
        admission: Admission<'_>,
    ) -> Result<ServiceExportSnapshot, SessionError> {
        if !resources::requires_private_sampler_sources(project) {
            let prepared = Arc::new(crate::prepare_with_waveforms(
                project,
                &self.media_bank(),
                self.sine_table(),
                None,
                &self.audio_transforms,
                &self.audio_clip_waveforms,
            )?);
            let result = ServiceExportSnapshot {
                prepared,
                sources: None,
            };
            if self.native_reservation.is_some() {
                admission.check(self, self, result.selection(), 0, None)?;
            }
            return Ok(result);
        }
        if self.audio_mode != DocumentAudioMode::Resident {
            return Err(SessionError::Invalid(
                "sampler offline export requires resident document mode",
            ));
        }
        let limits = self.native_reservation.ok_or(SessionError::Invalid(
            "sampler requires an explicit resident reservation",
        ))?;
        let sources = ControlSamplerSources::prepare_on_control(
            &self.native_assets,
            project,
            limits.storage.max_retained_bytes,
            limits.max_scan_frames,
            &self.shutdown,
        )?;
        let allowance = sources.compiler_factory_allowance()?;
        admission.check(
            self,
            self,
            ControlSamplerSelection {
                sources: Some(&sources),
                prepared: None,
            },
            allowance
                .checked_mul(2)
                .ok_or(SessionError::Invalid("sampler compiler allowance overflow"))?,
            None,
        )?;
        let prepared = Arc::new(PreparedRevision::prepare_control_sampler_export(
            project,
            &self.media_bank().resident,
            self.sine_table(),
            &self.audio_transforms,
            &sources,
        )?);
        let result = ServiceExportSnapshot {
            prepared,
            sources: Some(sources),
        };
        admission.check(self, self, result.selection(), allowance, None)?;
        Ok(result)
    }
    pub(crate) fn command_service_snapshot(
        &mut self,
        command: Command,
        admission: Admission<'_>,
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        // Prepare against a separate immutable view of Session-owned media while
        // Editor controls the sole canonical candidate/history transaction.
        let media = self.media_bank();
        let native = &self.native_assets;
        let reservation = self.native_reservation;
        let sine = self.sine_table();
        let transforms = &self.audio_transforms;
        let waveforms = &self.audio_clip_waveforms;
        let shutdown = &self.shutdown;
        let current_legacy = self
            .media
            .values()
            .flat_map(|m| [&m.source_audio, &m.audio])
            .collect::<Vec<_>>();
        let audio_mode = self.audio_mode;
        let max_frames = self.max_resident_frames;
        let current = admission.current;
        self.editor.execute_prepared(command, |candidate| {
            media.validate_clip_sources(candidate)?;
            let sources = if resources::requires_private_sampler_sources(candidate) {
                if audio_mode != DocumentAudioMode::Resident {
                    return Err(SessionError::Invalid(
                        "sampler offline export requires resident document mode",
                    ));
                }
                let limits = reservation.ok_or(SessionError::Invalid(
                    "sampler requires an explicit resident reservation",
                ))?;
                Some(ControlSamplerSources::prepare_on_control(
                    native,
                    candidate,
                    limits.storage.max_retained_bytes,
                    limits.max_scan_frames,
                    shutdown,
                )?)
            } else {
                None
            };
            let inspect = |selection: ControlSamplerSelection<'_>,
                           extra: usize|
             -> Result<(), SessionError> {
                if let Some(limits) = reservation {
                    let retained = std::iter::once(selection)
                        .chain(admission.export)
                        .collect::<Vec<_>>();
                    let construction = retained.iter().copied().chain(current).collect::<Vec<_>>();
                    let stores = [native];
                    let outbound = admission.outbound.into_iter().collect::<Vec<_>>();
                    let kept = ControlSamplerPhase {
                        stores: &stores,
                        selections: &retained,
                        outbound: &outbound,
                        legacy: &current_legacy,
                        checked_runtime_bytes: admission.runtime_bytes,
                        construction_bytes: 0,
                    };
                    let peak = ControlSamplerPhase {
                        stores: &stores,
                        selections: &construction,
                        outbound: &outbound,
                        legacy: &current_legacy,
                        checked_runtime_bytes: admission.runtime_bytes,
                        construction_bytes: extra,
                    };
                    let inventory =
                        resources::inspect_control_sampler_union(&kept, &peak, limits.storage)?;
                    if inventory.retained_frames > max_frames {
                        return Err(SessionError::Invalid("resident audio frame limit reached"));
                    }
                }
                Ok(())
            };
            let factory = sources
                .as_ref()
                .map(ControlSamplerSources::compiler_factory_allowance)
                .transpose()?
                .unwrap_or(0);
            inspect(
                ControlSamplerSelection {
                    sources: sources.as_ref(),
                    prepared: None,
                },
                factory
                    .checked_mul(2)
                    .ok_or(SessionError::Invalid("sampler compiler allowance overflow"))?,
            )?;
            let prepared = match sources.as_ref() {
                Some(sources) => PreparedRevision::prepare_control_sampler_export(
                    candidate,
                    &media.resident,
                    Arc::clone(&sine),
                    transforms,
                    sources,
                )?,
                None => crate::prepare_with_waveforms(
                    candidate,
                    &media,
                    Arc::clone(&sine),
                    None,
                    transforms,
                    waveforms,
                )?,
            };
            let prepared = Arc::new(prepared);
            inspect(
                ControlSamplerSelection {
                    sources: sources.as_ref(),
                    prepared: Some(&prepared),
                },
                factory,
            )?;
            if shutdown.load(Ordering::Acquire) {
                return Err(SessionError::Invalid("sampler preparation canceled"));
            }
            Ok(ServiceExportSnapshot { prepared, sources })
        })
    }
}

#[cfg(test)]
pub(super) mod tests;

impl Session {
    pub(crate) fn import_service_snapshot(
        &mut self,
        decoded: hex_media::DecodedAudio,
        cancel: &AtomicBool,
        admission: Admission<'_>,
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        let limits = self.native_reservation.ok_or(SessionError::Invalid(
            "native source import requires an explicit resident reservation",
        ))?;
        let sources = if resources::requires_private_sampler_sources(self.project()) {
            Some(ControlSamplerSources::prepare_on_control(
                &self.native_assets,
                self.project(),
                limits.storage.max_retained_bytes,
                limits.max_scan_frames,
                cancel,
            )?)
        } else {
            None
        };
        let legacy = self
            .media
            .values()
            .flat_map(|m| [&m.source_audio, &m.audio])
            .cloned()
            .collect::<Vec<_>>();
        // These legacy-only aliases live in this synchronous control transaction;
        // they never mint a native handle or duplicate PCM DATA.
        let legacy_refs = legacy.iter().collect::<Vec<_>>();
        let maximum = self.max_resident_frames;
        self.import_device_asset_prepared(
            decoded,
            cancel,
            |candidate, media, table, transforms, waveforms, stage| {
                let inspect = |selection: ControlSamplerSelection<'_>,
                               extra: usize|
                 -> Result<(), SessionError> {
                    let kept = std::iter::once(selection)
                        .chain(admission.export)
                        .collect::<Vec<_>>();
                    let peak = kept
                        .iter()
                        .copied()
                        .chain(admission.current)
                        .collect::<Vec<_>>();
                    let outbound = admission.outbound.into_iter().collect::<Vec<_>>();
                    let retained = ControlSamplerPhase {
                        stores: &[],
                        selections: &kept,
                        outbound: &outbound,
                        legacy: &legacy_refs,
                        checked_runtime_bytes: admission.runtime_bytes,
                        construction_bytes: 0,
                    };
                    let construction = ControlSamplerPhase {
                        stores: &[],
                        selections: &peak,
                        outbound: &outbound,
                        legacy: &legacy_refs,
                        checked_runtime_bytes: admission.runtime_bytes,
                        construction_bytes: extra,
                    };
                    let inventory = stage.inspect(&retained, &construction, limits.storage)?;
                    if inventory.retained_frames > maximum {
                        return Err(SessionError::Invalid("resident audio frame limit reached"));
                    }
                    Ok(())
                };
                let factory = sources
                    .as_ref()
                    .map(ControlSamplerSources::compiler_factory_allowance)
                    .transpose()?
                    .unwrap_or(0);
                inspect(
                    ControlSamplerSelection {
                        sources: sources.as_ref(),
                        prepared: None,
                    },
                    factory
                        .checked_mul(2)
                        .ok_or(SessionError::Invalid("sampler compiler allowance overflow"))?,
                )?;
                let prepared = Arc::new(match sources.as_ref() {
                    Some(sources) => PreparedRevision::prepare_control_sampler_export(
                        candidate,
                        &media.resident,
                        table,
                        transforms,
                        sources,
                    )?,
                    None => crate::prepare_with_waveforms(
                        candidate, media, table, None, transforms, waveforms,
                    )?,
                });
                inspect(
                    ControlSamplerSelection {
                        sources: sources.as_ref(),
                        prepared: Some(&prepared),
                    },
                    factory,
                )?;
                Ok(ServiceExportSnapshot { prepared, sources })
            },
        )
    }
}

impl Admission<'_> {
    /// Before ordinary New/Open allocates admitted resident media, reserve room
    /// for the exact captured export plus the still-live predecessor document.
    pub(crate) fn replacement_frame_allowance(
        &self,
        current: &Session,
    ) -> Result<u64, SessionError> {
        let Some(limits) = current.native_reservation else {
            return Ok(current.max_resident_frames);
        };
        let candidate = Session::new(
            current.project().sample_rate(),
            1,
            current.max_resident_frames,
        )?
        .with_native_reservation(limits)?;
        let empty = ControlSamplerSelection {
            sources: None,
            prepared: None,
        };
        let base = self.check(current, &candidate, empty, 0, None)?;
        let retained = limits
            .storage
            .max_retained_bytes
            .checked_sub(base.retained.retained_bytes)
            .ok_or(SessionError::Invalid(
                "resident retained reservation is full",
            ))?;
        let peak = limits
            .storage
            .max_construction_peak_bytes
            .checked_sub(base.construction.construction_peak_bytes)
            .ok_or(SessionError::Invalid(
                "resident construction reservation is full",
            ))?;
        let by_bytes = u64::try_from(retained.min(peak) / std::mem::size_of::<hex_engine::Frame>())
            .map_err(|_| SessionError::Invalid("resident byte allowance exceeds frame range"))?;
        Ok(current
            .max_resident_frames
            .checked_sub(base.retained_frames)
            .ok_or(SessionError::Invalid(
                "captured export exceeds resident frame reservation",
            ))?
            .min(by_bytes))
    }
}
