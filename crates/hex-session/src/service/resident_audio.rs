//! Ordinary resident preparation and final worker publication inside canonical transactions.
//! Original-bank owners, derived clip PCM and fixed factory/runtime allowances remain distinct.
//! A queue/refusal leaves authored state/history unchanged; accepted offline custody is explicit
//! and never reports a callback-applied revision. No source set or Playback is emitted to UI.
use super::audio_sink::native_sampler::NativeSamplerSources;
use super::*;
use crate::MediaBank;
use hex_engine::native_device_resources::{
    DeviceResourceAdmission, DeviceResourcePhase, PreparedDeviceResources,
};

pub(super) struct Snapshot {
    pub(super) prepared: Arc<PreparedRevision>,
    pub(super) resources: PreparedDeviceResources,
}
struct Inputs<'a> {
    native: NativeSamplerSources<'a>,
    limits: crate::ResidentNativeReservation,
    max_frames: u64,
    shutdown: &'a AtomicBool,
    cancel: Option<&'a AtomicBool>,
    admission: sampler_export::Admission<'a>,
    current: Option<&'a Snapshot>,
    legacy: Vec<&'a hex_engine::Audio>,
}
impl Inputs<'_> {
    fn admit(
        &self,
        resources: &PreparedDeviceResources,
        prepared: Option<&PreparedRevision>,
        native: &[DeviceResourceAdmission<'_>],
        runtime_bytes: usize,
        construction_bytes: usize,
    ) -> Result<(), SessionError> {
        let candidate_sets = [resources];
        let candidate_revisions = prepared.map(|prepared| [prepared]);
        let candidate = DeviceResourceAdmission::new(&candidate_sets, &[])
            .with_prepared_revisions(candidate_revisions.as_ref().map_or(&[], |r| r.as_slice()));
        let current_sets = self.current.map(|s| [&s.resources]);
        let current_revisions = self.current.map(|s| [s.prepared.as_ref()]);
        let current =
            DeviceResourceAdmission::new(current_sets.as_ref().map_or(&[], |s| s.as_slice()), &[])
                .with_prepared_revisions(current_revisions.as_ref().map_or(&[], |r| r.as_slice()));
        let mut registered = Vec::new();
        registered
            .try_reserve_exact(
                native
                    .len()
                    .checked_add(2)
                    .ok_or(SessionError::Invalid("resident admission count overflow"))?,
            )
            .map_err(|_| SessionError::Invalid("resident admission allocation failed"))?;
        registered.extend([candidate, current]);
        registered.extend_from_slice(native);
        let selections = self
            .admission
            .current
            .into_iter()
            .chain(self.admission.export)
            .chain(prepared.map(|prepared| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: None,
                    prepared: Some(prepared),
                }
            }))
            .collect::<Vec<_>>();
        let outbound = self.admission.outbound.into_iter().collect::<Vec<_>>();
        let phase = DeviceResourcePhase {
            stores: &[],
            registered: &registered,
            selections: &selections,
            outbound: &outbound,
            legacy: &self.legacy,
            checked_runtime_bytes: self
                .admission
                .runtime_bytes
                .checked_add(runtime_bytes)
                .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?,
            construction_bytes,
        };
        let inventory = self.native.inspect(&phase, self.limits.storage)?;
        if inventory.retained_frames > self.max_frames
            || inventory.construction_frames > self.max_frames
        {
            return Err(SessionError::Invalid("resident audio frame limit reached"));
        }
        Ok(())
    }
    fn prepare(
        &self,
        project: &Project,
        sink: &dyn WorkerAudioSink,
        media: &MediaBank,
        table: Arc<hex_engine::SineTable>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        waveforms: &std::cell::RefCell<BTreeMap<hex_project::ClipId, AudioClipWaveform>>,
    ) -> Result<Snapshot, SessionError> {
        if self.shutdown.load(Ordering::Acquire)
            || self
                .cancel
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
        {
            return Err(SessionError::Invalid("resident preparation cancelled"));
        }
        let resources = match self.native {
            NativeSamplerSources::Committed(store) => PreparedDeviceResources::prepare_on_control(
                store,
                project,
                self.limits.storage.max_retained_bytes,
                self.shutdown,
            )?,
            NativeSamplerSources::Staged(stage) => {
                PreparedDeviceResources::prepare_staged_on_control(
                    stage,
                    project,
                    self.limits.storage.max_retained_bytes,
                    self.shutdown,
                )?
            }
        };
        let held = sink.native_sampler_hold();
        self.admit(
            &resources,
            None,
            &sink.resident_admission(),
            held.construction_bytes,
            resources.compiler_construction_allowance_bytes()?,
        )?;
        let work = resources.compiler_scan_work_on_control(project)?;
        let prepared =
            crate::prepare_with_waveforms_using(project, media, transforms, waveforms, || {
                PreparedRevision::prepare_with_device_resources(
                    project,
                    &media.resident,
                    table,
                    self.current.map(|s| s.prepared.session().as_ref()),
                    transforms,
                    &resources,
                    work,
                    self.shutdown,
                )
            })?;
        let snapshot = Snapshot {
            resources,
            prepared: Arc::new(prepared),
        };
        let runtime =
            hex_engine::SessionPlayback::linear_export_runtime_allowance(&snapshot.prepared)
                .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
        self.admit(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            held.construction_bytes
                .checked_add(runtime)
                .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?,
            0,
        )?;
        Ok(snapshot)
    }
    fn publish(
        &self,
        snapshot: &Snapshot,
        sink: &mut dyn WorkerAudioSink,
        context: hex_engine::AudioSourceContext,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let playback = Playback::try_from_prepared_with_device_resources(
            Arc::clone(&snapshot.prepared),
            &snapshot.resources,
        )
        .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
        if self.shutdown.load(Ordering::Acquire)
            || self
                .cancel
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
        {
            return Err(SessionError::Invalid("resident publication cancelled"));
        }
        let quoted =
            hex_engine::SessionPlayback::linear_export_runtime_allowance(&snapshot.prepared)
                .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
        if playback
            .accounted_session_runtime_bytes_on_control()
            .is_none_or(|bytes| bytes > quoted)
        {
            return Err(SessionError::Invalid(
                "resident runtime exceeds its prepared allowance",
            ));
        }
        let runtime = sink
            .native_sampler_hold()
            .construction_bytes
            .checked_add(quoted)
            .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?;
        let metadata = ResidentPublicationMetadata {
            context,
            device_preview: None,
            tuning_preview: None,
            seek_request: None,
            requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
        };
        // The offline sink may capture configuration handles before final admission. Reserve
        // their bounded metadata overlap before that allocation; actual capacities are visited
        // again together with the old pending owner before acceptance.
        self.admit(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            runtime,
            snapshot.resources.metadata_bytes()?,
        )?;
        sink.publish_resident(
            playback,
            Some(&snapshot.resources),
            metadata,
            ResidentPublicationPolicy::InheritRuntime,
            self.limits.storage.max_retained_bytes,
            &mut |native| {
                self.admit(
                    &snapshot.resources,
                    Some(&snapshot.prepared),
                    native,
                    runtime,
                    0,
                )
                .map_err(|error| error.to_string())
            },
        )
        .map_err(|refused| SessionError::NativeAudio(refused.error))
    }
}

impl Session {
    #[allow(clippy::too_many_arguments)] // Captured target, source, control union and final publication remain explicit.
    pub(super) fn import_resident_sampler(
        &mut self,
        decoded: hex_media::DecodedAudio,
        cancel: &AtomicBool,
        destination: crate::native_assets::NativeAssetDestination,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        let limits = self.native_reservation.ok_or(SessionError::Invalid(
            "resident resource reservation unavailable",
        ))?;
        let max_frames = self.max_resident_frames;
        let shutdown = Arc::clone(&self.shutdown);
        let legacy = self
            .media
            .values()
            .flat_map(|media| [&media.source_audio, &media.audio])
            .cloned()
            .collect::<Vec<_>>();
        let legacy_refs = legacy.iter().collect::<Vec<_>>();
        // Both closures run synchronously and never overlap. Checked borrows preserve an explicit
        // failure if that contract changes, without storing a sink in Session or a UI event.
        let sink = std::cell::RefCell::new(sink);
        self.import_device_asset_prepared(
            decoded,
            cancel,
            destination,
            |project, media, table, transforms, waveforms, stage, _| {
                let inputs = Inputs {
                    native: NativeSamplerSources::Staged(stage),
                    limits,
                    max_frames,
                    shutdown: &shutdown,
                    cancel: Some(cancel),
                    admission,
                    current,
                    legacy: legacy_refs.clone(),
                };
                let sink = sink.try_borrow().map_err(|_| {
                    SessionError::Invalid("resident publication is already borrowed")
                })?;
                inputs.prepare(project, &**sink, media, table, transforms, waveforms)
            },
            |snapshot, stage| {
                let inputs = Inputs {
                    native: NativeSamplerSources::Staged(stage),
                    limits,
                    max_frames,
                    shutdown: &shutdown,
                    cancel: Some(cancel),
                    admission,
                    current,
                    legacy: legacy_refs.clone(),
                };
                let mut sink = sink.try_borrow_mut().map_err(|_| {
                    SessionError::Invalid("resident publication is already borrowed")
                })?;
                inputs.publish(snapshot, &mut **sink, context)?;
                Ok(())
            },
        )
    }
    pub(super) fn publish_resident_snapshot(
        &self,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
            native: NativeSamplerSources::Committed(&self.native_assets),
            limits: self.native_reservation.ok_or(SessionError::Invalid(
                "resident resource reservation unavailable",
            ))?,
            max_frames: self.max_resident_frames,
            shutdown: &self.shutdown,
            cancel: None,
            admission,
            current,
            legacy: self
                .media
                .values()
                .flat_map(|media| [&media.source_audio, &media.audio])
                .collect(),
        };
        let snapshot = inputs.prepare(
            self.project(),
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
        )?;
        inputs.publish(&snapshot, sink, context)?;
        Ok(snapshot)
    }
    pub(super) fn command_resident_snapshot(
        &mut self,
        command: Command,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        crate::validate_command_media(&command)?;
        if let Command::PlaceClip { asset, .. } = &command
            && self.native_assets.contains(*asset)
            && !self.media.contains_key(asset)
        {
            return Err(SessionError::DeviceAssetClipUnsupported(*asset));
        }
        let inputs = Inputs {
            native: NativeSamplerSources::Committed(&self.native_assets),
            limits: self.native_reservation.ok_or(SessionError::Invalid(
                "resident resource reservation unavailable",
            ))?,
            max_frames: self.max_resident_frames,
            shutdown: &self.shutdown,
            cancel: None,
            admission,
            current,
            legacy: self
                .media
                .values()
                .flat_map(|m| [&m.source_audio, &m.audio])
                .collect(),
        };
        let media = self.media_bank();
        let table = self.sine_table();
        let transforms = &self.audio_transforms;
        let waveforms = &self.audio_clip_waveforms;
        self.editor
            .execute_prepared_transition(command, |_, project| {
                let snapshot =
                    inputs.prepare(project, sink, &media, table, transforms, waveforms)?;
                inputs.publish(&snapshot, sink, context)?;
                Ok(snapshot)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, atomic::AtomicUsize};
    use std::time::Duration;

    #[derive(Default)]
    struct Observed {
        accepted: Mutex<Vec<(u64, thread::ThreadId)>>,
        refuse: AtomicBool,
        closed: AtomicUsize,
    }
    struct OfflineSink {
        observed: Arc<Observed>,
        pending: Option<(Playback, PreparedDeviceResources)>,
    }
    impl WorkerAudioSink for OfflineSink {
        fn resident_publication_enabled(&self) -> bool {
            true
        }
        fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
            [
                self.pending.as_ref().map_or(
                    DeviceResourceAdmission::default(),
                    |(playback, resources)| {
                        DeviceResourceAdmission::unpublished_on_control(playback, Some(resources))
                    },
                ),
                DeviceResourceAdmission::default(),
                DeviceResourceAdmission::default(),
            ]
        }
        #[allow(clippy::result_large_err)] // Return the exact unpublished owner.
        fn publish_resident(
            &mut self,
            playback: Playback,
            resources: Option<&PreparedDeviceResources>,
            metadata: ResidentPublicationMetadata,
            _: ResidentPublicationPolicy,
            metadata_limit: usize,
            admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
        ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
            let result = (|| {
                let resources = resources
                    .ok_or("missing resources")?
                    .capture_on_control(metadata_limit)
                    .map_err(|error| format!("{error:?}"))?;
                let candidate =
                    DeviceResourceAdmission::unpublished_on_control(&playback, Some(&resources));
                let old = self.resident_admission();
                admit(&[candidate, old[0]])?;
                if self.observed.refuse.load(Ordering::Acquire) {
                    return Err("fixture publication refused".into());
                }
                Ok(resources)
            })();
            match result {
                Ok(resources) => {
                    self.pending = Some((playback, resources));
                    self.observed
                        .accepted
                        .lock()
                        .unwrap()
                        .push((metadata.context.editor_revision, thread::current().id()));
                    Ok(ResidentPublicationAcceptance::OfflineRetained)
                }
                Err(error) => Err(ResidentPublicationRefusal { playback, error }),
            }
        }
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Err("fixture offline".into())
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {
            self.pending = None;
            self.observed.closed.fetch_add(1, Ordering::Release);
        }
    }

    // Exercise the real default-stack canonical worker: factories and Playback are constructed
    // there, never in an enlarged fixture thread or UI event. Final publication refusal must
    // preserve the prior authoring/history state while the old private pending owner survives.
    #[test]
    fn default_worker_prepares_private_resident_graph_and_refuses_before_commit() {
        let observed = Arc::new(Observed::default());
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            1024,
            None,
            Box::new(OfflineSink {
                observed: observed.clone(),
                pending: None,
            }),
            crate::ResidentNativeReservation {
                storage: hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 64,
                    root_rows: 64,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 30_000_000,
                },
                max_scan_frames: 1024,
            },
        )
        .unwrap();
        let receive = || service.events.recv_timeout(Duration::from_secs(5)).unwrap();
        let initial = receive();
        assert!(initial.error.as_ref().unwrap().contains("fixture offline"));
        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "ordinary".into(),
            }))
            .unwrap();
        let added = receive();
        assert!(added.error.is_none(), "{:?}", added.error);
        assert!(added.playback.is_none());
        let track = added.state.project.tracks()[0].id();
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        service
            .submit(Request::Edit(Command::InsertInstrument { track, state }))
            .unwrap();
        let installed = receive();
        assert!(installed.error.is_none(), "{:?}", installed.error);
        assert!(installed.playback.is_none());
        let files = sampler_export::tests::Files::new(false);
        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "sampler".into(),
            }))
            .unwrap();
        let sampler_track = receive();
        assert!(sampler_track.error.is_none(), "{:?}", sampler_track.error);
        let sampler_track_id = sampler_track.state.project.tracks().last().unwrap().id();
        service
            .submit(Request::LoadFirstSampler {
                request_id: 1,
                project_generation: 0,
                revision: sampler_track.state.revision,
                track: sampler_track_id,
                empty_sampler: None,
                path: files.source().to_owned(),
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        let loaded = receive();
        assert!(loaded.error.is_none(), "{:?}", loaded.error);
        assert!(loaded.playback.is_none());
        assert!(
            loaded
                .state
                .project
                .track(sampler_track_id)
                .unwrap()
                .instrument()
                .is_some()
        );
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "audio after sampler".into(),
            }))
            .unwrap();
        let installed = receive();
        assert!(installed.error.is_none(), "{:?}", installed.error);
        assert!(installed.playback.is_none());
        observed.refuse.store(true, Ordering::Release);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "refused".into(),
            }))
            .unwrap();
        let refused = receive();
        assert!(
            refused
                .error
                .as_ref()
                .unwrap()
                .contains("fixture publication refused")
        );
        assert_eq!(refused.state.project, installed.state.project);
        assert_eq!(refused.state.revision, installed.state.revision);
        assert_eq!(observed.accepted.lock().unwrap().len(), 6);
        assert!(
            observed
                .accepted
                .lock()
                .unwrap()
                .iter()
                .all(|(_, worker)| *worker != thread::current().id())
        );
        drop(service);
    }
}
