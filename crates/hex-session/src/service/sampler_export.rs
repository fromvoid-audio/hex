//! Private Service-owned offline sampler snapshots and their admission union.
//! Neither this wrapper nor a resource-bearing graph is emitted to public APIs.
use super::audio_sink::native_sampler::NativeSamplerSources;
use super::*;
use hex_engine::native_device_resources::{
    self as resources, ControlSamplerSelection, ControlSamplerSources, DeviceResourceInventory,
    DeviceResourcePhase, StagedNativeAssetView,
};

enum SnapshotAction {
    Command(Box<Command>),
    MidiCommand(Box<Command>),
    History { redo: bool },
}

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
    pub(crate) registered: &'a [resources::DeviceResourceAdmission<'a>],
    pub(crate) seeks: Option<&'a super::resident_seek::Lane>,
    pub(crate) preview: Option<&'a super::resident_preview::Preview>,
    pub(crate) current: Option<ControlSamplerSelection<'a>>,
    pub(crate) native: NativeSamplerReservationHold,
    pub(crate) export: Option<ControlSamplerSelection<'a>>,
    pub(crate) export_resources: Option<resources::DeviceResourceAdmission<'a>>,
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
/// Borrowed actual replacement sources, plus a checked upcoming PCM allowance.
/// Recovery retains the old Session until the complete candidate is admitted.
pub(crate) struct RecoverySourceStage<'a> {
    pub(crate) originals: Option<&'a resources::ResidentDeviceAssets>,
    pub(crate) staged: Option<&'a StagedNativeAssetView<'a>>,
    pub(crate) prepared: &'a [(hex_project::AssetId, PreparedImport)],
    pub(crate) decoded: Option<&'a hex_engine::Audio>,
    pub(crate) additional_frames: u64,
}
impl Admission<'_> {
    pub(crate) fn registered_views(&self) -> Vec<resources::DeviceResourceAdmission<'_>> {
        self.registered
            .iter()
            .copied()
            .chain(self.export_resources)
            .chain(
                self.preview
                    .map(super::resident_preview::Preview::admission),
            )
            .chain(
                self.seeks
                    .into_iter()
                    .flat_map(super::resident_seek::Lane::views),
            )
            .collect()
    }
    pub(crate) fn check_recovery_sources(
        &self,
        current: &Session,
        source: RecoverySourceStage<'_>,
    ) -> Result<(), SessionError> {
        let limits = current.native_reservation.ok_or(SessionError::Invalid(
            "device-original reopening requires resident native limits",
        ))?;
        let stores = std::iter::once(&current.native_assets)
            .chain(source.originals)
            .collect::<Vec<_>>();
        let selections = self
            .current
            .into_iter()
            .chain(self.export)
            .collect::<Vec<_>>();
        let outbound = self.outbound.into_iter().collect::<Vec<_>>();
        let legacy = current
            .media
            .values()
            .flat_map(|m| [&m.source_audio, &m.audio])
            .chain(
                source
                    .prepared
                    .iter()
                    .flat_map(|(_, m)| [&m.source_audio, &m.audio]),
            )
            .chain(source.decoded)
            .collect::<Vec<_>>();
        let extra_bytes = usize::try_from(source.additional_frames)
            .ok()
            .and_then(|n| n.checked_mul(std::mem::size_of::<hex_engine::Frame>()))
            .ok_or(SessionError::Invalid("recovery source allowance overflow"))?;
        let phase = DeviceResourcePhase {
            registered: &self.registered_views(),
            stores: &stores,
            selections: &selections,
            outbound: &outbound,
            legacy: &legacy,
            checked_runtime_bytes: self
                .runtime_bytes
                .checked_add(self.native.construction_bytes)
                .ok_or(SessionError::Invalid("native reservation overflow"))?,
            construction_bytes: extra_bytes,
        };
        let inventory = match source.staged {
            Some(stage) => stage.inspect(&phase, &phase, limits.storage),
            None => resources::inspect_device_resource_union(&phase, &phase, limits.storage),
        }?;
        for frames in [inventory.retained_frames, inventory.construction_frames] {
            if frames
                .checked_add(self.native.frames)
                .and_then(|n| n.checked_add(source.additional_frames))
                .is_none_or(|n| n > current.max_resident_frames)
            {
                return Err(SessionError::Invalid(
                    "resident recovery source frame limit reached",
                ));
            }
        }
        Ok(())
    }
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
    ) -> Result<Option<DeviceResourceInventory>, SessionError> {
        let legacy = session
            .media
            .values()
            .flat_map(|m| [&m.source_audio, &m.audio])
            .chain(staged.iter().flat_map(|m| [&m.source_audio, &m.audio]))
            .collect::<Vec<_>>();
        self.check_native_parts(
            &session.native_assets,
            &legacy,
            session.native_reservation,
            session.max_resident_frames,
            None,
        )
    }
    pub(in crate::service) fn check_native_parts(
        &self,
        native: &resources::ResidentDeviceAssets,
        legacy: &[&hex_engine::Audio],
        limits: Option<crate::ResidentNativeReservation>,
        max_frames: u64,
        candidate: Option<ControlSamplerSelection<'_>>,
    ) -> Result<Option<DeviceResourceInventory>, SessionError> {
        self.check_native_originals(
            NativeSamplerSources::Committed(native),
            legacy,
            limits,
            max_frames,
            candidate,
        )
    }
    pub(in crate::service) fn check_native_originals(
        &self,
        originals: NativeSamplerSources<'_>,
        legacy: &[&hex_engine::Audio],
        limits: Option<crate::ResidentNativeReservation>,
        max_frames: u64,
        candidate: Option<ControlSamplerSelection<'_>>,
    ) -> Result<Option<DeviceResourceInventory>, SessionError> {
        self.check_native_originals_registered(
            originals,
            legacy,
            limits,
            max_frames,
            candidate,
            &[],
        )
    }
    pub(in crate::service) fn check_native_originals_registered(
        &self,
        originals: NativeSamplerSources<'_>,
        legacy: &[&hex_engine::Audio],
        limits: Option<crate::ResidentNativeReservation>,
        max_frames: u64,
        candidate: Option<ControlSamplerSelection<'_>>,
        registered: &[resources::DeviceResourceAdmission<'_>],
    ) -> Result<Option<DeviceResourceInventory>, SessionError> {
        let Some(limits) = limits else {
            return Ok(None);
        };
        let selections = self
            .current
            .into_iter()
            .chain(self.export)
            .chain(candidate)
            .collect::<Vec<_>>();
        let outbound = self.outbound.into_iter().collect::<Vec<_>>();
        let selected = self
            .registered_views()
            .into_iter()
            .chain(registered.iter().copied())
            .collect::<Vec<_>>();
        let phase = DeviceResourcePhase {
            registered: &selected,
            stores: &[],
            selections: &selections,
            outbound: &outbound,
            legacy,
            checked_runtime_bytes: self
                .runtime_bytes
                .checked_add(self.native.construction_bytes)
                .ok_or(SessionError::Invalid("native reservation overflow"))?,
            construction_bytes: 0,
        };
        let inventory = originals.inspect(&phase, limits.storage)?;
        for frames in [inventory.retained_frames, inventory.construction_frames] {
            if frames
                .checked_add(self.native.frames)
                .is_none_or(|n| n > max_frames)
            {
                return Err(SessionError::Invalid("resident audio frame limit reached"));
            }
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
    ) -> Result<DeviceResourceInventory, SessionError> {
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
        let kept = DeviceResourcePhase {
            registered: &self.registered_views(),
            stores: &kept_stores,
            selections: &kept_selections,
            outbound: &outbound,
            legacy: &kept_legacy,
            checked_runtime_bytes: self
                .runtime_bytes
                .checked_add(self.native.retained_bytes)
                .ok_or(SessionError::Invalid("native reservation overflow"))?,
            construction_bytes: 0,
        };
        let peak = DeviceResourcePhase {
            registered: &self.registered_views(),
            stores: &peak_stores,
            selections: &peak_selections,
            outbound: &outbound,
            legacy: &peak_legacy,
            checked_runtime_bytes: self
                .runtime_bytes
                .checked_add(self.native.construction_bytes)
                .ok_or(SessionError::Invalid("native reservation overflow"))?,
            construction_bytes: compiler_bytes,
        };
        let inventory = match stage {
            Some(stage) => stage.inspect(&kept, &peak, reservation.storage),
            None => resources::inspect_device_resource_union(&kept, &peak, reservation.storage),
        }
        .map_err(SessionError::from)?;
        if inventory
            .retained_frames
            .checked_add(self.native.frames)
            .is_none_or(|n| n > future.max_resident_frames)
        {
            return Err(SessionError::Invalid("resident audio frame limit reached"));
        }
        Ok(inventory)
    }
}
#[cfg(test)]
thread_local! {
    static SNAPSHOT_COMPILER_ENTRIES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
impl Session {
    pub(crate) fn prepare_service_snapshot(
        &self,
        project: &Project,
        admission: Admission<'_>,
    ) -> Result<ServiceExportSnapshot, SessionError> {
        self.prepare_service_snapshot_against(self, project, admission)
    }
    pub(crate) fn prepare_service_snapshot_against(
        &self,
        old: &Session,
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
                admission.check(old, self, result.selection(), 0, None)?;
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
        // The old Session remains alive until canonical replacement commits.
        // Its unassigned originals and unreferenced legacy PCM must be included
        // before any candidate factory/compiler allocation.
        admission.check(
            old,
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
        #[cfg(test)]
        SNAPSHOT_COMPILER_ENTRIES.with(|n| n.set(n.get() + 1));
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
        admission.check(old, self, result.selection(), allowance, None)?;
        Ok(result)
    }
    pub(crate) fn command_service_snapshot(
        &mut self,
        command: Command,
        admission: Admission<'_>,
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        self.command_service_snapshot_with_native(command, admission, None)
    }
    pub(in crate::service) fn command_service_snapshot_with_native(
        &mut self,
        command: Command,
        admission: Admission<'_>,
        native: Option<(
            hex_engine::AudioSourceContext,
            &mut dyn WorkerAudioSink,
            &AtomicBool,
        )>,
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        crate::validate_command_media(&command)?;
        self.prepare_service_snapshot_action(
            SnapshotAction::Command(Box::new(command)),
            admission,
            native,
        )
    }
    pub(in crate::service) fn midi_service_snapshot_with_native(
        &mut self,
        command: Box<Command>,
        admission: Admission<'_>,
        native: (
            hex_engine::AudioSourceContext,
            &mut dyn WorkerAudioSink,
            &AtomicBool,
        ),
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        self.prepare_service_snapshot_action(
            SnapshotAction::MidiCommand(command),
            admission,
            Some(native),
        )
    }
    pub(in crate::service) fn history_service_snapshot_with_native(
        &mut self,
        redo: bool,
        admission: Admission<'_>,
        native: (
            hex_engine::AudioSourceContext,
            &mut dyn WorkerAudioSink,
            &AtomicBool,
        ),
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        self.prepare_service_snapshot_action(
            SnapshotAction::History { redo },
            admission,
            Some(native),
        )
    }
    fn prepare_service_snapshot_action(
        &mut self,
        action: SnapshotAction,
        admission: Admission<'_>,
        native: Option<(
            hex_engine::AudioSourceContext,
            &mut dyn WorkerAudioSink,
            &AtomicBool,
        )>,
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        // Prepare against a separate immutable view of Session-owned media while
        // Editor controls the sole canonical candidate/history transaction.
        let media = self.media_bank();
        let media_assets = &self.media;
        let native_hook = native;
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
        let history = if let SnapshotAction::History { redo } = &action {
            Some((*redo, self.editor.next_midi_history_effect(*redo)))
        } else {
            None
        };
        let prepare = |source: &Project, candidate: &Project| {
            if let Some((redo, effect)) = history {
                super::sampler_parameter::validate_history_edge(effect, redo, source, candidate)
                    .map_err(SessionError::NativeAudio)?;
            }
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
                    let kept = DeviceResourcePhase {
                        registered: &admission.registered_views(),
                        stores: &stores,
                        selections: &retained,
                        outbound: &outbound,
                        legacy: &current_legacy,
                        checked_runtime_bytes: admission
                            .runtime_bytes
                            .checked_add(admission.native.construction_bytes)
                            .ok_or(SessionError::Invalid("native reservation overflow"))?,
                        construction_bytes: 0,
                    };
                    let peak = DeviceResourcePhase {
                        registered: &admission.registered_views(),
                        stores: &stores,
                        selections: &construction,
                        outbound: &outbound,
                        legacy: &current_legacy,
                        checked_runtime_bytes: admission
                            .runtime_bytes
                            .checked_add(admission.native.construction_bytes)
                            .ok_or(SessionError::Invalid("native reservation overflow"))?,
                        construction_bytes: extra,
                    };
                    let inventory =
                        resources::inspect_device_resource_union(&kept, &peak, limits.storage)?;
                    if inventory
                        .retained_frames
                        .checked_add(admission.native.frames)
                        .is_none_or(|n| n > max_frames)
                    {
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
            if let Some((context, sink, cancel)) = native_hook {
                sink.prepare_sampler_edit(NativeSamplerInput {
                    project: candidate,
                    media: media_assets,
                    originals: NativeSamplerSources::Committed(native),
                    reservation,
                    audio_mode,
                    max_frames,
                    shutdown,
                    context,
                    admission,
                    candidate: Some(ControlSamplerSelection {
                        sources: sources.as_ref(),
                        prepared: Some(&prepared),
                    }),
                })
                .map_err(SessionError::NativeAudio)?;
                if cancel.load(Ordering::Acquire) || shutdown.load(Ordering::Acquire) {
                    return Err(SessionError::Invalid("sampler gain cancelled"));
                }
            }
            Ok(ServiceExportSnapshot { prepared, sources })
        };
        match action {
            SnapshotAction::Command(command) => {
                self.editor.execute_prepared_transition(*command, prepare)
            }
            SnapshotAction::MidiCommand(command) => self
                .editor
                .execute_midi_prepared_transition(*command, prepare),
            SnapshotAction::History { redo } => {
                let snapshot = if redo {
                    self.editor.redo_prepared_transition(prepare)
                } else {
                    self.editor.undo_prepared_transition(prepare)
                }?;
                snapshot
                    .map(|snapshot| (Created::None, snapshot))
                    .ok_or(SessionError::Invalid(
                        "sampler history changed during preparation",
                    ))
            }
        }
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
        self.import_service_snapshot_to(decoded, cancel, admission, None)
    }
    pub(crate) fn import_service_snapshot_to(
        &mut self,
        decoded: hex_media::DecodedAudio,
        cancel: &AtomicBool,
        admission: Admission<'_>,
        track: Option<hex_project::TrackId>,
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        let destination = match track {
            Some(track) => {
                let (state, source_slot) =
                    resources::sampler_import_template_on_control(decoded.audio().frames().len())?;
                crate::native_assets::NativeAssetDestination::NewInstrument {
                    track,
                    placement: hex_project::InstrumentLoadPlacement::First,
                    state,
                    source_slot,
                }
            }
            None => crate::native_assets::NativeAssetDestination::Unassigned,
        };
        self.import_service_snapshot_destination(decoded, cancel, admission, destination, None)
    }
    pub(in crate::service) fn replace_sampler_service_snapshot(
        &mut self,
        decoded: hex_media::DecodedAudio,
        edit: &SamplerReplacementEdit,
        admission: Admission<'_>,
        context: hex_engine::AudioSourceContext,
        audio: &mut dyn WorkerAudioSink,
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        let (state, source_slot) = hex_engine::sampler_replacement_template_on_control(
            &edit.expected_state,
            decoded.audio().frames().len(),
        )?;
        let destination = crate::native_assets::NativeAssetDestination::ExistingInstrument {
            owner: edit.owner,
            device: edit.device,
            expected_state: edit.expected_state.clone(),
            state,
            source_slot,
        };
        self.import_service_snapshot_destination(
            decoded,
            &edit.cancel,
            admission,
            destination,
            Some((context, audio)),
        )
    }
    pub(in crate::service) fn import_service_snapshot_destination(
        &mut self,
        decoded: hex_media::DecodedAudio,
        cancel: &AtomicBool,
        admission: Admission<'_>,
        destination: crate::native_assets::NativeAssetDestination,
        native_hook: Option<(hex_engine::AudioSourceContext, &mut dyn WorkerAudioSink)>,
    ) -> Result<(Created, ServiceExportSnapshot), SessionError> {
        let limits = self.native_reservation.ok_or(SessionError::Invalid(
            "native source import requires a resident reservation",
        ))?;
        let audio_mode = self.audio_mode;
        let shutdown = Arc::clone(&self.shutdown);
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
            destination,
            |candidate, media, table, transforms, waveforms, stage, media_assets| {
                let sources = if resources::requires_private_sampler_sources(candidate) {
                    Some(ControlSamplerSources::prepare_staged_on_control(
                        stage,
                        candidate,
                        limits.storage.max_retained_bytes,
                        limits.max_scan_frames,
                        cancel,
                    )?)
                } else {
                    None
                };

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
                    let retained = DeviceResourcePhase {
                        registered: &admission.registered_views(),
                        stores: &[],
                        selections: &kept,
                        outbound: &outbound,
                        legacy: &legacy_refs,
                        checked_runtime_bytes: admission
                            .runtime_bytes
                            .checked_add(admission.native.construction_bytes)
                            .ok_or(SessionError::Invalid("native reservation overflow"))?,
                        construction_bytes: 0,
                    };
                    let construction = DeviceResourcePhase {
                        registered: &admission.registered_views(),
                        stores: &[],
                        selections: &peak,
                        outbound: &outbound,
                        legacy: &legacy_refs,
                        checked_runtime_bytes: admission
                            .runtime_bytes
                            .checked_add(admission.native.construction_bytes)
                            .ok_or(SessionError::Invalid("native reservation overflow"))?,
                        construction_bytes: extra,
                    };
                    let inventory = stage.inspect(&retained, &construction, limits.storage)?;
                    if inventory
                        .retained_frames
                        .checked_add(admission.native.frames)
                        .is_none_or(|n| n > maximum)
                    {
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
                if let Some((context, sink)) = native_hook {
                    sink.prepare_sampler_edit(NativeSamplerInput {
                        project: candidate,
                        media: media_assets,
                        originals: NativeSamplerSources::Staged(stage),
                        reservation: Some(limits),
                        audio_mode,
                        max_frames: maximum,
                        shutdown: &shutdown,
                        context,
                        admission,
                        candidate: Some(ControlSamplerSelection {
                            sources: sources.as_ref(),
                            prepared: Some(&prepared),
                        }),
                    })
                    .map_err(SessionError::NativeAudio)?;
                    if cancel.load(Ordering::Acquire) || shutdown.load(Ordering::Acquire) {
                        return Err(SessionError::Invalid(
                            "sampler source replacement cancelled",
                        ));
                    }
                }
                Ok(ServiceExportSnapshot { prepared, sources })
            },
            |_, _, _| Ok(()),
        )
    }
}

impl Admission<'_> {
    /// Same-Session append: every existing original, current/export capture,
    /// outbound job, legacy owner and native hold remains alive while decoding.
    pub(crate) fn append_original_frame_allowance(
        &self,
        session: &Session,
    ) -> Result<u64, SessionError> {
        let limits = session.native_reservation.ok_or(SessionError::Invalid(
            "native source import requires a resident reservation",
        ))?;
        let inventory = self.inspect_appended_original(session, None)?;
        let retained = limits
            .storage
            .max_retained_bytes
            .checked_sub(inventory.retained.retained_bytes)
            .ok_or(SessionError::Invalid(
                "resident retained reservation is full",
            ))?;
        let peak = limits
            .storage
            .max_construction_peak_bytes
            .checked_sub(inventory.construction.construction_peak_bytes)
            .ok_or(SessionError::Invalid(
                "resident construction reservation is full",
            ))?;
        let bytes = u64::try_from(retained.min(peak) / std::mem::size_of::<hex_engine::Frame>())
            .map_err(|_| SessionError::Invalid("resident byte allowance exceeds frame range"))?;
        session
            .max_resident_frames
            .checked_sub(
                inventory
                    .retained_frames
                    .max(inventory.construction_frames)
                    .checked_add(self.native.frames)
                    .ok_or(SessionError::Invalid("native frame reservation overflow"))?,
            )
            .map(|frames| frames.min(bytes))
            .ok_or(SessionError::Invalid(
                "resident original append reservation is full",
            ))
    }
    /// Borrow the actual decoded allocation before original ingress can consume
    /// it; no derivative or owning native projection has been created yet.
    pub(crate) fn inspect_appended_original(
        &self,
        session: &Session,
        decoded: Option<&hex_engine::Audio>,
    ) -> Result<DeviceResourceInventory, SessionError> {
        let legacy = session
            .media
            .values()
            .flat_map(|media| [&media.source_audio, &media.audio])
            .chain(decoded)
            .collect::<Vec<_>>();
        self.check_native_parts(
            &session.native_assets,
            &legacy,
            session.native_reservation,
            session.max_resident_frames,
            None,
        )?
        .ok_or(SessionError::Invalid(
            "native source import requires a resident reservation",
        ))
    }
    /// Before ordinary New/Open allocates admitted resident media, reserve room
    /// for the exact captured export plus the still-live predecessor document.
    pub(crate) fn replacement_frame_allowance(
        &self,
        current: &Session,
    ) -> Result<u64, SessionError> {
        let Some(limits) = current.native_reservation else {
            return Ok(current.max_resident_frames);
        };
        let candidate = Session::new_with_overview_budget(
            current.project().sample_rate(),
            1,
            current.max_resident_frames,
            current.overview_budget.clone(),
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
            .checked_sub(
                base.retained_frames
                    .checked_add(self.native.frames)
                    .ok_or(SessionError::Invalid("native frame reservation overflow"))?,
            )
            .ok_or(SessionError::Invalid(
                "captured export exceeds resident frame reservation",
            ))?
            .min(by_bytes))
    }
}
