mod bounce_tests;
mod export_tests;
mod fence_tests;
mod import_reuse_tests;
mod modulation_edit_tests;
mod native_automation_recording;
mod plugin_effect_tests;
mod source_binding_tests;
mod verified_replacement_tests;
use super::*;
use std::sync::{Mutex, atomic::AtomicUsize};
use std::time::Duration;

// The private-export fixture's 20M budget intentionally excludes full
// interactive release/handoff banks. Resident callers use the same finite
// runtime reservation as the other interactive service fixtures.
fn interactive_sampler_session(files: &sampler_export::tests::Files) -> Session {
    let (mut session, export) = sampler_export::tests::direct_sampler(files);
    drop(export);
    let storage = &mut session.native_reservation.as_mut().unwrap().storage;
    storage.max_retained_bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
        .checked_mul(2)
        .unwrap();
    storage.max_construction_peak_bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
        .checked_mul(3)
        .unwrap();
    session
}

#[derive(Default)]
struct Observed {
    accepted: Mutex<Vec<(u64, thread::ThreadId)>>,
    refuse: AtomicBool,
    refuse_native_source: AtomicBool,
    refuse_source_confirmation: AtomicBool,
    source_confirmations: AtomicUsize,
    closed: AtomicUsize,
    seek_attempts: AtomicUsize,
    publications: Mutex<Vec<(ResidentPublicationMetadata, ResidentPublicationPolicy)>>,
    native_parameter_input: Mutex<Option<hex_engine::NativeParameterInputTarget>>,
    native_capture_playing: AtomicBool,
    keyboard_peak: Mutex<f32>,
    render_for_switch_test: AtomicBool,
    switch_peaks: Mutex<Vec<(u64, f32)>>,
    keyboard_publications: AtomicUsize,
    effect_compilations: AtomicUsize,
    plugin_preparations: AtomicUsize,
    plugin_ready: AtomicBool,
    plugin_terminals: Mutex<Vec<(Arc<PluginEffectControl>, bool)>>,
    cancel_at_admission: Mutex<Option<Arc<PluginEffectControl>>>,
    cancel_after_admission: Mutex<Option<Arc<PluginEffectControl>>>,
}
struct OfflineSink {
    observed: Arc<Observed>,
    pending: Option<(
        Playback,
        PreparedDeviceResources,
        Arc<hex_engine::PreparedPlaybackObservation>,
    )>,
}
impl WorkerAudioSink for OfflineSink {
    fn prepare_plugin_effect(
        &mut self,
        _input: PluginEffectPreparation<'_>,
    ) -> Result<PluginEffectPreparationState, String> {
        self.observed
            .plugin_preparations
            .fetch_add(1, Ordering::Release);
        if self.observed.plugin_ready.load(Ordering::Acquire) {
            // Deliberately wrong provider: the worker must refuse rather than admit it.
            hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .map(PluginEffectPreparationState::Ready)
            .map_err(|e| e.to_string())
        } else {
            Ok(PluginEffectPreparationState::Preparing)
        }
    }
    fn plugin_effect_ready(&self) -> bool {
        self.observed.plugin_ready.load(Ordering::Acquire)
    }
    fn finish_plugin_effect(&mut self, control: &Arc<PluginEffectControl>, accepted: bool) {
        self.observed
            .plugin_terminals
            .lock()
            .unwrap()
            .push((Arc::clone(control), accepted));
    }
    fn prepare_resident_effects(
        &mut self,
        _input: ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        // Real registered compiler/resource tests now traverse the same
        // borrowed host-facts hook. This fixture supplies builtins only;
        // native instance/endpoint admission remains a desktop test.
        self.observed
            .effect_compilations
            .fetch_add(1, Ordering::Release);
        hex_engine::AudioCandidateEffects::with_host_definitions(&[], prepare)
            .map(Some)
            .map_err(|error| error.to_string())
    }
    fn resident_publication_enabled(&self) -> bool {
        true
    }
    fn confirm_source_effect_publication(
        &mut self,
        context: hex_engine::AudioSourceContext,
        acceptance: ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        self.observed
            .source_confirmations
            .fetch_add(1, Ordering::AcqRel);
        match acceptance {
            ResidentPublicationAcceptance::OfflineRetained => Ok(()),
            ResidentPublicationAcceptance::Submitted {
                output_identity: 7,
                audio_revision,
            } => {
                assert_eq!(audio_revision, context.editor_revision);
                Err("fixture callback refused source edit".into())
            }
            _ => panic!("unexpected fixture source receipt"),
        }
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        [
            self.pending.as_ref().map_or(
                DeviceResourceAdmission::default(),
                |(playback, resources, _)| {
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
        mut playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        assert_eq!(
            playback.native_parameter_input_target(),
            observation.native_parameter_input_target(),
        );
        if matches!(policy, ResidentPublicationPolicy::FreshIfUnchanged(_)) {
            self.observed.seek_attempts.fetch_add(1, Ordering::Release);
            return Err(ResidentPublicationRefusal {
                kind: crate::service::ResidentPublicationFailure::Other,
                playback,
                error: "fixture has no fresh output".into(),
            });
        }
        let result = (|| {
            let resources = resources
                .ok_or("missing resources")?
                .capture_on_control(metadata_limit)
                .map_err(|error| format!("{error:?}"))?;
            let candidate =
                DeviceResourceAdmission::unpublished_on_control(&playback, Some(&resources));
            let old = self.resident_admission();
            if let Some(control) = self.observed.cancel_at_admission.lock().unwrap().take() {
                assert!(control.cancel());
            }
            admit(&[candidate, old[0]])?;
            if let Some(control) = self.observed.cancel_after_admission.lock().unwrap().take() {
                assert!(
                    !control.cancel(),
                    "accepted admission must win over a late cancellation"
                );
            }
            if self.observed.refuse.load(Ordering::Acquire) {
                return Err("fixture publication refused".into());
            }
            Ok(resources)
        })();
        match result {
            Ok(resources) => {
                if self
                    .observed
                    .refuse_source_confirmation
                    .load(Ordering::Acquire)
                {
                    return Ok(ResidentPublicationAcceptance::Submitted {
                        output_identity: 7,
                        audio_revision: metadata.context.editor_revision,
                    });
                }
                if playback.computer_keyboard_tuning_hint().is_some() {
                    self.observed
                        .keyboard_publications
                        .fetch_add(1, Ordering::Release);
                    let mut pcm = [[0.0; 2]; 32];
                    playback.render(&mut pcm).unwrap();
                    *self.observed.keyboard_peak.lock().unwrap() = pcm
                        .iter()
                        .flatten()
                        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                }
                if self.observed.render_for_switch_test.load(Ordering::Acquire) {
                    playback.apply(hex_engine::playback::PlaybackCommand::Play);
                    let mut pcm = [[0.0; 2]; 256];
                    playback.render(&mut pcm).unwrap();
                    let peak = pcm
                        .iter()
                        .flatten()
                        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                    self.observed
                        .switch_peaks
                        .lock()
                        .unwrap()
                        .push((metadata.context.editor_revision, peak));
                }
                self.observed
                    .publications
                    .lock()
                    .unwrap()
                    .push((metadata, policy));
                *self.observed.native_parameter_input.lock().unwrap() =
                    observation.native_parameter_input_target();
                self.pending = Some((playback, resources, observation));
                self.observed
                    .accepted
                    .lock()
                    .unwrap()
                    .push((metadata.context.editor_revision, thread::current().id()));
                Ok(ResidentPublicationAcceptance::OfflineRetained)
            }
            Err(error) => Err(ResidentPublicationRefusal {
                kind: if self.observed.refuse_native_source.load(Ordering::Acquire) {
                    crate::service::ResidentPublicationFailure::NativeSourceUnavailable
                } else {
                    crate::service::ResidentPublicationFailure::Other
                },
                playback,
                error,
            }),
        }
    }
    fn device_automation_source(
        &mut self,
        context: hex_engine::AudioSourceContext,
        requested: DeviceCaptureTarget,
    ) -> Result<Option<DeviceAutomationSource>, String> {
        let publications = self.observed.publications.lock().unwrap();
        if !self.observed.native_capture_playing.load(Ordering::Acquire)
            || publications
                .last()
                .is_none_or(|(metadata, _)| metadata.context != context)
        {
            return Ok(None);
        }
        Ok(self
            .observed
            .native_parameter_input
            .lock()
            .unwrap()
            .filter(|target| Some(*target) == requested.native())
            .map(|target| DeviceAutomationSource {
                target: target.into(),
                sample_rate: 48_000,
                output_identity: 7,
                audio_revision: publications.len() as u64,
                transport_epoch: 3,
                section_loop: None,
            }))
    }
    fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
        let bytes = self
            .pending
            .as_ref()
            .map_or(0, |(_, _, view)| view.retained_bytes_on_control().unwrap());
        NativeSamplerReservationHold {
            retained_bytes: bytes,
            construction_bytes: bytes,
            ..NativeSamplerReservationHold::default()
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

pub(in crate::service) fn fixture_sink() -> Box<dyn WorkerAudioSink> {
    Box::new(OfflineSink {
        observed: Arc::new(Observed::default()),
        pending: None,
    })
}

#[test]
fn held_checkpoint_allows_ordered_resident_mute_audio() {
    use std::time::Instant;
    let files = sampler_export::tests::Files::new(false);
    let path = files.source();
    let samples = vec![12000_i16; 4096];
    let data = (samples.len() * 2) as u32;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
    let journal = path.parent().unwrap().join("untitled.hex-journal");
    let observed = Arc::new(Observed::default());
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
        .checked_mul(2)
        .unwrap();
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        Some(journal.clone()),
        Box::new(OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    .checked_add(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
                    .unwrap(),
            },
            max_scan_frames: 8_000_000,
        },
    )
    .unwrap();
    let next = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    next();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "track".into(),
        }))
        .unwrap();
    let track = next().state.project.tracks()[0].id();
    service
        .submit(Request::ImportToTrack {
            path: path.to_path_buf(),
            track,
            start: 0,
        })
        .unwrap();
    assert!(next().error.is_none());
    observed
        .render_for_switch_test
        .store(true, Ordering::Release);
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    super::super::recovery::install_checkpoint_gate(journal.clone(), entered_tx, release_rx);
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first = service.try_track_switch_view().unwrap();
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    let until = Instant::now() + Duration::from_millis(300);
    let second = loop {
        if let Ok(view) = service.try_track_switch_view() {
            break Some(view);
        }
        if Instant::now() >= until {
            break None;
        }
        std::thread::yield_now();
    };
    let until = Instant::now() + Duration::from_millis(300);
    let peaks = loop {
        let peaks = observed.switch_peaks.lock().unwrap().clone();
        if second
            .as_ref()
            .is_some_and(|view| peaks.iter().any(|(revision, _)| *revision == view.revision))
            || Instant::now() >= until
        {
            break peaks;
        }
        std::thread::yield_now();
    };
    release.send(()).unwrap();
    let second = second.expect("second mute must commit before checkpoint release");
    let muted = peaks
        .iter()
        .find(|(revision, _)| *revision == first.revision)
        .expect("first prepared audio")
        .1;
    let unmuted = peaks
        .iter()
        .find(|(revision, _)| *revision == second.revision)
        .expect("second prepared audio")
        .1;
    assert_eq!(muted, 0.0);
    assert!(
        unmuted > 0.01,
        "unmuted prepared graph must render audible signal, got {unmuted}"
    );
    assert!(next().error.is_none() && next().error.is_none());
}

#[test]
fn library_load_uses_admitted_source_transaction_and_preserves_owner_on_refusal() {
    let files = sampler_export::tests::Files::new(false);
    let directory = files.source().parent().unwrap().join("library");
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: Arc::clone(&observed),
        pending: None,
    };
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
        .checked_mul(2)
        .unwrap();
    let mut session = Session::new(8000, 16, 8192)
        .unwrap()
        .with_native_reservation(crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    .checked_add(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
                    .unwrap(),
            },
            max_scan_frames: 8192,
        })
        .unwrap();
    let Created::Track(track) = session
        .command(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let decoded = decode_wav_file(files.source(), Limits::new(1024, 1024)).unwrap();
    let (state, source_slot) =
        hex_engine::native_device_resources::sampler_import_template_on_control(
            decoded.audio().frames().len(),
        )
        .unwrap();
    let (_, mut snapshot) = session
        .import_resident_sampler(
            decoded,
            &AtomicBool::new(false),
            crate::native_assets::NativeAssetDestination::NewInstrument {
                track,
                placement: hex_project::InstrumentLoadPlacement::First,
                state,
                source_slot,
            },
            hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 1,
            },
            &mut sink,
            sampler_export::Admission::default(),
            None,
        )
        .unwrap();
    let mut tuned = session.project().tuning().scale();
    tuned.set_reference(69, 880.0, 0).unwrap();
    hex_storage::tuning_library::save(&directory, 1, "double", tuned).unwrap();
    let before = session.project().clone();
    let accepted = observed.accepted.lock().unwrap().len();
    for refused in [true, false] {
        observed.refuse.store(refused, Ordering::Release);
        let (result, prepared) = super::super::run_tuning_library_operation_in_using(
            &mut session,
            TuningLibraryContext {
                request_generation: 0,
                request_revision: 1,
                current_generation: 0,
                current_revision: 1,
                operation_id: 1,
            },
            TuningLibraryOperation::Load { slot: 1 },
            &directory,
            |session, command| {
                let (_, next) = session.command_resident_snapshot(
                    command,
                    hex_engine::AudioSourceContext {
                        project_generation: 0,
                        editor_revision: 2,
                    },
                    &mut sink,
                    sampler_export::Admission::default(),
                    Some(&snapshot),
                )?;
                let prepared = Arc::clone(&next.prepared);
                snapshot = next;
                Ok(prepared)
            },
        );
        assert_eq!(result.error.is_some(), refused);
        assert_eq!(prepared.is_none(), refused);
        if refused {
            assert_eq!(session.project(), &before);
            assert_eq!(observed.accepted.lock().unwrap().len(), accepted);
        } else {
            assert_eq!(session.project().tuning().scale(), tuned);
            assert_eq!(
                session.project().track(track).unwrap().instrument(),
                before.track(track).unwrap().instrument()
            );
            assert_eq!(observed.accepted.lock().unwrap().len(), accepted + 1);
            assert!(session.can_undo());
        }
    }
}

#[test]
fn native_automation_arm_and_disarm_publish_without_editing_canonical_history() {
    let observed = Arc::new(Observed::default());
    let runtime = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: 2 * runtime,
                max_construction_peak_bytes: 3 * runtime,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    receive();
    let state = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertMainDevice {
            before: None,
            state,
        }))
        .unwrap();
    let added = receive();
    assert!(added.error.is_none(), "{:?}", added.error);
    let Some(Created::Device(device)) = added.created else {
        panic!("main effect");
    };
    let target = hex_lattice_contract::DeviceAutomationTarget::HostOutputGain;
    let prepare = |revision, target| {
        service
            .submit(Request::ArmNativeParameterRecording {
                project_generation: 0,
                revision,
                target,
            })
            .unwrap();
        receive()
    };
    let rev = added.state.revision;
    let count = observed.publications.lock().unwrap().len();
    assert!(prepare(rev + 1, Some((device, target))).error.is_some());
    assert_eq!(observed.publications.lock().unwrap().len(), count);
    let armed = prepare(rev, Some((device, target)));
    assert!(armed.error.is_none(), "{:?}", armed.error);
    let metadata = observed.publications.lock().unwrap().last().unwrap().0;
    let exact =
        hex_engine::native_parameter_input_target(&armed.state.project, device, target).unwrap();
    assert_eq!(
        *observed.native_parameter_input.lock().unwrap(),
        Some(exact)
    );
    assert_eq!(metadata.context.editor_revision, rev);
    assert!(metadata.gain_recording_track.is_none());
    let disarmed = prepare(rev, None);
    assert!(disarmed.error.is_none(), "{:?}", disarmed.error);
    assert!(observed.native_parameter_input.lock().unwrap().is_none());
    for result in [armed, disarmed] {
        assert_eq!(result.state.project, added.state.project);
        assert_eq!(result.state.revision, rev);
        assert_eq!(result.state.can_undo, added.state.can_undo);
    }
}

#[test]
fn range_copy_reports_source_refusal_only_before_commit() {
    let observed = Arc::new(Observed::default());
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    + hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    receive();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "track".into(),
        }))
        .unwrap();
    let added = receive();
    let track = added.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_raw(4 << 32),
        }))
        .unwrap();
    let before = receive();
    assert!(before.error.is_none());
    let copy = || {
        Request::Edit(Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], 0, 96_000).unwrap(),
            track,
            frame: 96_000,
        })
    };
    observed.refuse.store(true, Ordering::Release);
    service.submit(copy()).unwrap();
    let unrelated = receive();
    assert!(unrelated.error.is_some());
    assert_eq!(unrelated.range_copy_source_unavailable, None);
    assert_eq!(unrelated.state.revision, before.state.revision);
    observed.refuse_native_source.store(true, Ordering::Release);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "other".into(),
        }))
        .unwrap();
    let other_edit = receive();
    assert!(other_edit.error.is_some());
    assert_eq!(other_edit.range_copy_source_unavailable, None);
    assert_eq!(other_edit.state.revision, before.state.revision);
    service.submit(copy()).unwrap();
    let refused = receive();
    assert!(refused.error.is_some());
    assert_eq!(
        refused.range_copy_source_unavailable,
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: before.state.revision,
        })
    );
    assert_eq!(refused.state.revision, before.state.revision);
    assert_eq!(refused.state.project, before.state.project);
    assert_eq!(refused.state.can_undo, before.state.can_undo);
    observed.refuse.store(false, Ordering::Release);
    observed
        .refuse_native_source
        .store(false, Ordering::Release);
    service.submit(copy()).unwrap();
    let committed = receive();
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert_eq!(committed.range_copy_source_unavailable, None);
    assert_eq!(committed.state.revision, before.state.revision + 1);
    service.submit(Request::Undo).unwrap();
    let undone = receive();
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.range_copy_source_unavailable, None);
    assert_eq!(
        undone
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        1
    );
}

#[test]
fn project_sample_placement_publishes_or_refuses_before_commit() {
    let observed = Arc::new(Observed::default());
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    + hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    receive();
    let files = sampler_export::tests::Files::new(false);
    service
        .submit(Request::Import(files.source().to_owned()))
        .unwrap();
    let imported = receive();
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let track = imported.state.project.tracks()[0].id();
    let asset = imported.state.project.assets().next().unwrap().id();
    let request = |revision| Request::InsertProjectSample {
        project_generation: 0,
        revision,
        asset,
        destination: crate::import_batch::AudioBatchDestination::Track { track, start: 32 },
        new_track_gain: None,
        warp: hex_project::AudioImportWarp::Off,
        audition_timing: Some(hex_engine::playback::BrowserAuditionTiming {
            grid: hex_project::ResolvedInputGrid::off(),
            transport_epoch: 0,
        }),
    };
    let accepted_before = observed.accepted.lock().unwrap().len();
    service.submit(request(imported.state.revision)).unwrap();
    let placed = receive();
    assert!(placed.error.is_none(), "{:?}", placed.error);
    assert_eq!(placed.state.project.assets().count(), 1);
    assert_eq!(observed.accepted.lock().unwrap().len(), accepted_before + 1);
    observed.refuse.store(true, Ordering::Release);
    service.submit(request(placed.state.revision)).unwrap();
    let refused = receive();
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|reason| reason.contains("fixture publication refused"))
    );
    assert_eq!(refused.state.project, placed.state.project);
    assert_eq!(refused.state.revision, placed.state.revision);
    assert_eq!(observed.accepted.lock().unwrap().len(), accepted_before + 1);
}

// Exercise the real default-stack canonical worker: factories and Playback are constructed
// there, never in an enlarged fixture thread or UI event. Final publication refusal must
// preserve the prior authoring/history state while the old private pending owner survives.
#[test]
fn default_worker_prepares_private_resident_graph_and_refuses_before_commit() {
    let observed = Arc::new(Observed::default());
    // One old pending owner and one candidate use the existing graph ceiling, as the
    // production policy does per runtime role. This fixture's former20MB reservation could
    // not reserve the existing3MiB keyboard allowance; no production limit is changed.
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
        .checked_mul(2)
        .unwrap();
    let peak = retained
        .checked_add(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
        .unwrap();
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
                max_retained_bytes: retained,
                max_construction_peak_bytes: peak,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    let initial = receive();
    assert!(initial.error.as_ref().unwrap().contains("fixture offline"));
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "ordinary".into(),
        }))
        .unwrap();
    let added = receive();
    assert!(added.error.is_none(), "{:?}", added.error);
    assert!(observed.effect_compilations.load(Ordering::Acquire) > 0);
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
            slot: SamplerLoadSlot::New(hex_project::InstrumentLoadPlacement::First),
            path: files.source().to_owned(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let loaded = receive();
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    assert!(loaded.playback.is_none());
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 127, 1))).unwrap();
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: loaded.state.revision,
            request_id: 51,
            track: sampler_track_id,
            input: Arc::clone(&input),
        })
        .unwrap();
    let keyboard = receive();
    assert!(keyboard.error.is_none(), "{:?}", keyboard.error);
    assert_eq!(keyboard.computer_keyboard_complete, Some(51));
    assert!(keyboard.playback.is_none());
    assert!(
        *observed.keyboard_peak.lock().unwrap() > 0.0,
        "ordinary shared keyboard input renders real original sampler PCM while paused"
    );
    assert_eq!(
        observed
            .publications
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .0
            .computer_keyboard_request,
        Some(51)
    );
    // The same physical client survives transient graph preparation and ordinary edits.
    let audition = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
    audition.set_key(0, Some((67, 100, 2))).unwrap();
    service
        .submit(Request::PrepareTuningAudition {
            revision: loaded.state.revision,
            input: Arc::clone(&audition),
        })
        .unwrap();
    let audition_ready = receive();
    assert!(audition_ready.error.is_none(), "{:?}", audition_ready.error);
    assert!(audition_ready.tuning_audition_complete.is_some());
    let sampler_device = loaded
        .state
        .project
        .track(sampler_track_id)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let expression = Arc::new(hex_engine::live_expression::LiveExpressionInput::new(
        0,
        sampler_track_id,
        sampler_device,
    ));
    service
        .submit(Request::PrepareLiveExpression {
            revision: loaded.state.revision,
            input: Arc::clone(&expression),
        })
        .unwrap();
    let expression_ready = receive();
    assert!(
        expression_ready.error.is_none(),
        "{:?}",
        expression_ready.error
    );
    assert!(expression_ready.live_expression_complete.is_some());
    service
        .submit(Request::ArmGainRecording {
            project_generation: 0,
            revision: loaded.state.revision,
            track: sampler_track_id,
        })
        .unwrap();
    let armed = receive();
    assert!(armed.error.is_none(), "{:?}", armed.error);
    assert_eq!(
        observed
            .publications
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .0
            .gain_recording_track,
        Some(sampler_track_id)
    );
    service
        .submit(Request::RestorePlayback { generation: 9 })
        .unwrap();
    let restore = receive();
    assert!(restore.error.is_none(), "{:?}", restore.error);
    assert_eq!(restore.restore_complete, Some(9));
    assert!(restore.playback.is_none());
    assert_eq!(observed.keyboard_publications.load(Ordering::Acquire), 5);
    assert!(!input.is_cancelled());
    assert!(!audition.is_cancelled());
    assert!(!expression.is_cancelled());
    input.cancel();
    audition.cancel();
    expression.cancel();
    // A new keyboard client must not be refused because unrelated old monitor clients
    // were terminally canceled. Only this request's required input is strict.
    let next_input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: loaded.state.revision,
            request_id: 53,
            track: sampler_track_id,
            input: Arc::clone(&next_input),
        })
        .unwrap();
    let reselected = receive();
    assert!(reselected.error.is_none(), "{:?}", reselected.error);
    assert!(!next_input.is_cancelled());
    next_input.cancel();
    service.submit(Request::RestoreStoppedPlayback).unwrap();
    let restored = receive();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert!(restored.stopped_playback_restore_complete);
    let publications = observed.publications.lock().unwrap();
    let (metadata, policy) = publications.last().unwrap();
    assert!(metadata.stopped_restore);
    assert!(matches!(
        policy,
        ResidentPublicationPolicy::InstallFreshRuntime
    ));
    drop(publications);
    let accepted_before_seek = observed.accepted.lock().unwrap().len();
    let seek = ResidentArrangementSeekIntent {
        history: crate::service::ResidentSeekHistory::Fresh,
        request_id: 1,
        project_generation: 0,
        project_revision: loaded.state.revision,
        destination_frame: 129,
        start_playing: true,
        section: None,
        locator_quantization: None,
        fence: ResidentFreshRuntimeFence {
            output_identity: 1,
            active_audio_revision: 1,
            route_generation: 0,
            admitted_command_ticket: 0,
            output_fault_epoch: 0,
        },
        device_preview: None,
        tuning_preview: None,
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    service
        .submit_resident_arrangement_seek(seek.clone())
        .unwrap();
    let submitted = receive();
    assert!(submitted.error.is_none(), "{:?}", submitted.error);
    assert!(
        service.try_arrangement_seek_result().is_none(),
        "native owners cannot escape through legacy result getter"
    );
    let result = wait_for_seek(&service);
    assert_eq!(result.request_id, 1);
    assert_eq!(result.project_revision, loaded.state.revision);
    assert!(
        result
            .result
            .unwrap_err()
            .contains("fixture has no fresh output")
    );
    assert_eq!(
        observed.seek_attempts.load(Ordering::Acquire),
        1,
        "same-source preroll completed on existing seek worker before final fresh-output refusal"
    );
    assert_eq!(
        observed.accepted.lock().unwrap().len(),
        accepted_before_seek
    );
    service
        .submit_resident_arrangement_seek(ResidentArrangementSeekIntent {
            request_id: 2,
            project_revision: loaded.state.revision + 1,
            ..seek
        })
        .unwrap();
    let stale = receive();
    assert!(stale.error.is_some());
    assert!(
        wait_for_seek(&service)
            .result
            .unwrap_err()
            .contains("target changed")
    );
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
    for (request, expected) in [
        (Request::Undo, &loaded.state.project),
        (Request::Redo, &installed.state.project),
    ] {
        service.submit(request).unwrap();
        let history = receive();
        assert!(history.error.is_none(), "{:?}", history.error);
        // Editor preserves the allocator high-water mark across Undo; compare restored
        // authored objects and stable identities rather than that monotonic allocator.
        assert_eq!(history.state.project.tracks(), expected.tracks());
        assert_eq!(
            history.state.project.assets().collect::<Vec<_>>(),
            expected.assets().collect::<Vec<_>>()
        );
        assert!(history.playback.is_none());
    }
    let device = loaded
        .state
        .project
        .track(sampler_track_id)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    for request in [
        Request::Edit(Command::SetDeviceBypass {
            device,
            bypassed: true,
        }),
        Request::ReplaceNativeDevice {
            device,
            class: hex_engine::NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.sine",
            )),
        },
        Request::Edit(Command::RemoveInstrument {
            track: sampler_track_id,
        }),
    ] {
        service.submit(request).unwrap();
        let edited = receive();
        assert!(edited.error.is_none(), "{:?}", edited.error);
        service.submit(Request::Undo).unwrap();
        let undone = receive();
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert_eq!(undone.state.project, installed.state.project);
    }
    let current_revision = || observed.accepted.lock().unwrap().last().unwrap().0;
    service
        .submit(Request::SamplerEdit(SamplerEdit::Parameter(
            SamplerParameterEdit {
                request_id: 2,
                project_generation: 0,
                revision: current_revision(),
                track: sampler_track_id,
                device,
                parameter: hex_engine::SamplerParameter::Gain,
                value: 0.5,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )))
        .unwrap();
    let gain = receive();
    assert!(gain.error.is_none(), "{:?}", gain.error);
    assert_eq!(gain.sampler_edit_complete, Some(2));
    assert_eq!(
        hex_engine::sampler_parameter_value_on_control(
            hex_engine::SamplerParameter::Gain,
            gain.state.project.device(device).unwrap().state()
        )
        .unwrap(),
        0.5
    );
    service.submit(Request::Undo).unwrap();
    let reset = receive();
    assert!(reset.error.is_none(), "{:?}", reset.error);
    service
        .submit(Request::SamplerEdit(SamplerEdit::Replacement(
            SamplerReplacementEdit {
                request_id: 3,
                project_generation: 0,
                revision: reset.state.revision,
                track: sampler_track_id,
                device,
                expected_state: reset.state.project.device(device).unwrap().state().clone(),
                path: files.source().to_owned(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )))
        .unwrap();
    let replaced = receive();
    assert!(replaced.error.is_none(), "{:?}", replaced.error);
    assert_eq!(replaced.sampler_edit_complete, Some(3));
    assert_eq!(
        replaced
            .state
            .project
            .track(sampler_track_id)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );
    service
        .submit(Request::SamplerEdit(SamplerEdit::History(
            SamplerHistoryEdit {
                request_id: 4,
                project_generation: 0,
                revision: replaced.state.revision,
                redo: false,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )))
        .unwrap();
    let restored = receive();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(
        restored.state.project.tracks(),
        installed.state.project.tracks()
    );
    service
        .submit(Request::Import(files.source().to_owned()))
        .unwrap();
    let imported = receive();
    assert!(imported.error.is_none(), "{:?}", imported.error);
    assert_eq!(
        imported.state.project.tracks().len(),
        installed.state.project.tracks().len() + 1
    );
    assert_eq!(
        imported
            .state
            .project
            .track(sampler_track_id)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );
    assert!(imported.playback.is_none());
    service.submit(Request::Undo).unwrap();
    let undo_import = receive();
    assert!(undo_import.error.is_none(), "{:?}", undo_import.error);
    assert_eq!(
        undo_import.state.project.tracks(),
        installed.state.project.tracks()
    );
    service.submit(Request::Redo).unwrap();
    let installed = receive();
    assert!(installed.error.is_none(), "{:?}", installed.error);
    assert_eq!(installed.state.project, imported.state.project);
    let retained_input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    retained_input.set_key(0, Some((60, 127, 3))).unwrap();
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: installed.state.revision,
            request_id: 52,
            track: sampler_track_id,
            input: Arc::clone(&retained_input),
        })
        .unwrap();
    assert!(receive().error.is_none());
    observed.refuse.store(true, Ordering::Release);
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: installed.state.revision,
            request_id: 54,
            track: sampler_track_id,
            input: Arc::clone(&retained_input),
        })
        .unwrap();
    let refused_keyboard = receive();
    assert!(refused_keyboard.error.is_some());
    assert!(
        !retained_input.is_cancelled(),
        "failed reprepare cannot cancel an accepted physical client"
    );
    observed.refuse.store(false, Ordering::Release);
    let before = observed.keyboard_publications.load(Ordering::Acquire);
    for muted in [true, false] {
        service
            .submit(Request::Edit(Command::Mute {
                track: sampler_track_id,
                muted,
            }))
            .unwrap();
        let changed = receive();
        assert!(changed.error.is_none(), "{:?}", changed.error);
        assert_eq!(
            changed
                .state
                .project
                .track(sampler_track_id)
                .unwrap()
                .muted(),
            muted
        );
        assert!(!retained_input.is_cancelled());
    }
    assert_eq!(
        observed.keyboard_publications.load(Ordering::Acquire),
        before + 2
    );
    service
        .submit(Request::Edit(Command::Mute {
            track: sampler_track_id,
            muted: false,
        }))
        .unwrap();
    let no_change = receive();
    assert!(no_change.error.is_none(), "{:?}", no_change.error);
    assert_eq!(no_change.state.project, installed.state.project);
    assert_eq!(no_change.state.revision, installed.state.revision + 2);
    observed.refuse.store(true, Ordering::Release);
    service
        .submit(Request::Edit(Command::RemoveInstrument {
            track: sampler_track_id,
        }))
        .unwrap();
    let refused_remove = receive();
    assert!(refused_remove.error.is_some());
    assert_eq!(refused_remove.state.project, no_change.state.project);
    assert!(
        !retained_input.is_cancelled(),
        "refused candidate cannot retire an installed input"
    );
    observed.refuse.store(false, Ordering::Release);
    service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device,
            bypassed: true,
        }))
        .unwrap();
    let bypassed = receive();
    assert!(bypassed.error.is_none(), "{:?}", bypassed.error);
    assert!(
        retained_input.is_cancelled(),
        "accepted invalid source retires its input without blocking the edit"
    );
    service.submit(Request::Undo).unwrap();
    let restored_source = receive();
    assert!(restored_source.error.is_none());
    assert_eq!(restored_source.state.project, no_change.state.project);
    service
        .submit(Request::Export(files.output().to_owned()))
        .unwrap();
    let queued_export = receive();
    assert!(queued_export.error.is_none(), "{:?}", queued_export.error);
    let exported = service
        .export_events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(exported.result.is_ok(), "{:?}", exported.result);
    assert!(files.output().exists());
    let accepted = observed.accepted.lock().unwrap().len();
    let before_refusal_revision = observed.accepted.lock().unwrap().last().unwrap().0;
    observed.refuse.store(true, Ordering::Release);
    service
        .submit(Request::Import(files.source().to_owned()))
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
    assert_eq!(refused.state.revision, before_refusal_revision);
    assert_eq!(observed.accepted.lock().unwrap().len(), accepted);
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

#[test]
fn section_seek_publication_uses_actual_loop_metadata_allowance() {
    thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            use hex_project::{BeatTime, Created, Editor, Tempo};
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            editor
                .execute(Command::SetTempoPoint {
                    beat: BeatTime::ZERO,
                    tempo: Tempo::new(123_000_000).unwrap(),
                })
                .unwrap();
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack {
                    name: "keys".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            let Created::Scene(scene) = editor
                .execute(Command::AddScene {
                    name: "source".into(),
                })
                .unwrap()
            else {
                panic!("scene")
            };
            let Created::SessionClip(clip) = editor
                .execute(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "notes".into(),
                    source_length: BeatTime::from_quarters(1024).unwrap(),
                })
                .unwrap()
            else {
                panic!("clip")
            };
            // Enough independently authored metadata to exceed incidental slack
            // in the old canonical-loop runtime quote, while density stays tiny.
            for note in 0..512 {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_quarters(note * 2).unwrap(),
                        duration: BeatTime::ONE,
                        pitch: 60,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap();
            }
            let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 3;
            let session = Session::from_prepared(editor.project().clone(), 8, 1024, Vec::new())
                .unwrap()
                .with_native_reservation(crate::ResidentNativeReservation {
                    storage: hex_engine::AudioStorageLedgerReservation {
                        data_rows: 512,
                        handle_rows: 64,
                        root_rows: 64,
                        max_retained_bytes: bytes,
                        max_construction_peak_bytes: bytes,
                    },
                    max_scan_frames: 1024,
                })
                .unwrap();
            let observed = Arc::new(Observed::default());
            let mut sink = OfflineSink {
                observed: Arc::clone(&observed),
                pending: None,
            };
            let context = hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 1,
            };
            let mut current = session
                .publish_resident_snapshot(
                    context,
                    &mut sink,
                    sampler_export::Admission::default(),
                    None,
                )
                .unwrap();
            let captured = Snapshot {
                prepared_context: current.prepared_context,
                file_sources: current.file_sources.clone(),
                prepared: Arc::clone(&current.prepared),
                resources: current.resources.capture_on_control(bytes).unwrap(),
                inputs: current.inputs.clone(),
            };
            let region = hex_project::LoopRegion::new(0, 23_415, true).unwrap();
            let runtime = hex_engine::SessionPlayback::prepare_section_arrangement_preroll(
                Arc::clone(&captured.prepared),
                Some(&captured.resources),
                region,
                0,
                &AtomicBool::new(false),
            )
            .unwrap();
            let playback = Playback::from_interactive_arrangement_preroll(runtime).unwrap();
            assert!(
                playback
                    .accounted_session_runtime_bytes_on_control()
                    .unwrap()
                    > runtime_allowance(&captured.prepared, 0).unwrap(),
                "fixture must catch canonical quote reuse"
            );
            let accepted = session
                .publish_resident_seek(
                    &captured,
                    session.project(),
                    &mut current,
                    playback,
                    &mut sink,
                    sampler_export::Admission::default(),
                    ResidentPublicationMetadata {
                        clip_preview: None,
                        context,
                        device_preview: None,
                        tuning_preview: None,
                        seek_request: Some(1),
                        stopped_restore: false,
                        computer_keyboard_request: None,
                        gain_recording_track: None,
                        requires_preroll: false,
                    },
                    ResidentPublicationPolicy::QuantizedSeekWhenReady {
                        fence: ResidentFreshRuntimeFence {
                            output_identity: 1,
                            active_audio_revision: 1,
                            route_generation: 0,
                            admitted_command_ticket: 0,
                            output_fault_epoch: 0,
                        },
                        quantization: hex_engine::LaunchQuantization::Quarter,
                    },
                    &AtomicBool::new(false),
                )
                .unwrap();
            assert!(matches!(
                accepted,
                ResidentPublicationAcceptance::OfflineRetained
            ));
            assert_eq!(
                sink.pending.as_ref().unwrap().0.section_loop(),
                Some(region)
            );
            assert!(session.project().loop_region().is_none());
            assert_eq!(observed.accepted.lock().unwrap().len(), 2);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn captured_seek_refuses_a_later_accepted_input_selection_at_the_same_project_revision() {
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: Arc::clone(&observed),
        pending: None,
    };
    let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
        .checked_mul(3)
        .unwrap();
    let mut session = Session::new(48_000, 8, 1024)
        .unwrap()
        .with_native_reservation(crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: bytes,
                max_construction_peak_bytes: bytes,
            },
            max_scan_frames: 1024,
        })
        .unwrap();
    let Created::Track(track) = session
        .command(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let context = hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 1,
    };
    let mut current = session
        .publish_resident_snapshot(
            context,
            &mut sink,
            sampler_export::Admission::default(),
            None,
        )
        .unwrap();
    let (results, receiver) = mpsc::sync_channel(4);
    let mut lane = resident_seek::Lane::new(ArrangementSeekWorker::new().unwrap(), results);
    let intent = ResidentArrangementSeekIntent {
        history: crate::service::ResidentSeekHistory::Fresh,
        request_id: 1,
        project_generation: 0,
        project_revision: 1,
        destination_frame: 129,
        start_playing: true,
        section: None,
        locator_quantization: None,
        fence: ResidentFreshRuntimeFence {
            output_identity: 1,
            active_audio_revision: 1,
            route_generation: 0,
            admitted_command_ticket: 0,
            output_fault_epoch: 0,
        },
        device_preview: None,
        tuning_preview: None,
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    lane.submit(
        intent,
        &session,
        &current,
        &sink,
        sampler_export::Admission::default(),
        context,
    )
    .unwrap();
    // Canonical control deliberately does not drive the result until after accepting the
    // new physical selection. No scheduler timing or worker-idle inference establishes this cut.
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 100, 1))).unwrap();
    let mut selected = current.inputs.clone();
    selected.keyboard = Some((track, Arc::clone(&input)));
    let metadata = ResidentPublicationMetadata {
        clip_preview: None,
        context,
        device_preview: None,
        tuning_preview: None,
        seek_request: None,
        stopped_restore: false,
        computer_keyboard_request: Some(2),
        gain_recording_track: None,
        requires_preroll: false,
    };
    session
        .publish_resident_runtime_inputs(
            &mut current,
            &mut sink,
            sampler_export::Admission {
                seeks: Some(&lane),
                ..Default::default()
            },
            metadata,
            ResidentPublicationPolicy::InheritRuntime,
            selected,
            Some(RequiredInput::Keyboard),
        )
        .unwrap();
    assert_eq!(current.inputs.version, 1);
    let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
    let result = loop {
        lane.drive(
            &session,
            Some(&mut current),
            Some(&mut sink),
            sampler_export::Admission::default(),
            context,
        );
        if let Ok(result) = receiver.try_recv() {
            break result;
        }
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    };
    assert!(result.result.unwrap_err().contains("seek target changed"));
    assert_eq!(
        observed.seek_attempts.load(Ordering::Acquire),
        0,
        "stale result never reaches output admission"
    );
    assert!(!input.is_cancelled());
    assert_eq!(current.inputs.version, 1);
    // Likewise a worker result from a previously accepted preview cannot replace a newer
    // accepted preview, even while canonical project revision and physical input are equal.
    let mut current = Some(current);
    let mut preview = None;
    let draft = crate::device_gesture::GestureSnapshot {
        track_gains: None,
        touched_parameters: crate::device_gesture::TouchedParameters::default(),
        manual_edit: false,
        key: crate::device_gesture::GestureKey {
            generation: 0,
            token: 1,
        },
        target: crate::device_gesture::GestureTarget::TrackGain { track },
        sequence: 1,
        value: crate::device_gesture::ParameterValue::Float(0.5),
        secondary_value: None,
        parameters: None,
        creation: None,
        plugin_value: None,
    };
    resident_preview::device_operation(
        &mut session,
        crate::device_gesture::GesturePreparation::Preview(draft),
        context,
        &mut current,
        &mut preview,
        &mut sink,
        sampler_export::Admission::default(),
    )
    .unwrap();
    let identity = preview.as_ref().unwrap().device_identity().unwrap();
    let intent = ResidentArrangementSeekIntent {
        history: crate::service::ResidentSeekHistory::Fresh,
        request_id: 2,
        project_generation: 0,
        project_revision: 1,
        destination_frame: 129,
        start_playing: true,
        section: None,
        locator_quantization: None,
        fence: ResidentFreshRuntimeFence {
            output_identity: 1,
            active_audio_revision: 1,
            route_generation: 0,
            admitted_command_ticket: 0,
            output_fault_epoch: 0,
        },
        device_preview: Some(identity),
        tuning_preview: None,
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    lane.submit(
        intent,
        &session,
        current.as_ref().unwrap(),
        &sink,
        sampler_export::Admission {
            preview: preview.as_ref(),
            ..Default::default()
        },
        context,
    )
    .unwrap();
    resident_preview::device_operation(
        &mut session,
        crate::device_gesture::GesturePreparation::Preview(
            crate::device_gesture::GestureSnapshot {
                sequence: 2,
                value: crate::device_gesture::ParameterValue::Float(0.25),
                ..draft
            },
        ),
        context,
        &mut current,
        &mut preview,
        &mut sink,
        sampler_export::Admission {
            seeks: Some(&lane),
            ..Default::default()
        },
    )
    .unwrap();
    let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
    let result = loop {
        lane.drive(
            &session,
            current.as_mut(),
            Some(&mut sink),
            sampler_export::Admission {
                preview: preview.as_ref(),
                ..Default::default()
            },
            context,
        );
        if let Ok(result) = receiver.try_recv() {
            break result;
        }
        assert!(std::time::Instant::now() < deadline);
        crate::test_wait::pause();
    };
    assert!(result.result.unwrap_err().contains("seek target changed"));
    assert_eq!(observed.seek_attempts.load(Ordering::Acquire), 0);
    assert_eq!(
        preview
            .as_ref()
            .unwrap()
            .device_identity()
            .unwrap()
            .sequence,
        2
    );
    assert_eq!(current.as_ref().unwrap().inputs.version, 1);
    assert!(!input.is_cancelled());
    drop(lane); // The existing worker joins before its exact captured anchors retire.
}

fn wait_for_seek(service: &Service) -> ResidentArrangementSeekResult {
    let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
    loop {
        if let Some(result) = service.try_resident_arrangement_seek_result() {
            return result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native scalar seek result did not arrive"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
