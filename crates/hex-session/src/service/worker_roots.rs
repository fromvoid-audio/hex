//! Fresh same-worker dispatch borrows. No owner moves, clone or stored permit.
use super::*;
use crate::snapshot_exports::SnapshotWorkerControl;

pub(super) type KeyboardActivation = Option<(
    u64,
    u64,
    hex_project::TrackId,
    Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
)>;

// Private dispatch INPUTS, not a public inspection/admission scope. No guard or
// ledger is stored here. A fresh WorkerRootView is built for EACH synchronous
// audio call, ending before try_send, receive/sleep or the next model mutation.
pub(super) struct WorkerDispatchState<'a> {
    pub session: &'a Session,
    pub current: &'a Arc<ViewState>,
    pub compiled: &'a Option<(u64, Arc<PreparedRevision>)>,
    pub midi_preview: &'a hex_engine::SharedMidiArrangementPreview,
    pub generation: u64,
    pub revision: u64,
    pub control: Option<&'a SnapshotWorkerControl>,
    pub clipboard: &'a Option<ArrangementClipboard>,
    pub media_base: &'a Option<PathBuf>,
    pub recovery: &'a Option<PathBuf>,
    pub document: &'a Option<DocumentHandle>,
    pub journal: &'a Option<hex_storage::Journal>,
    pub keyboard: &'a KeyboardActivation,
    pub keyboard_hint: &'a Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
    pub audition: &'a Option<Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    pub expression: &'a Option<Arc<hex_engine::live_expression::LiveExpressionInput>>,
    pub device_gesture: &'a GestureMachine,
    pub tuning_gesture: &'a TuningGestureMachine,
    pub export_job: Option<&'a ExportJob>,
    pub export_completion: Option<&'a ExportEvent>,
}

/// Which actual synchronous dispatch is being observed; never an admission flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerRootDispatch {
    ChangedEvent,
    PendingEvent,
    Idle,
}

/// Sealed borrow of ACTUAL same-worker roots, valid only for this audio call.
/// No Session, Project, source, Arc, waveform or PCM can be extracted. It is not
/// Clone and provides no saved permit or completeness flag. Opaque channels,
/// legacy parent allocations and unvisited children stay Unknown.
/// ```compile_fail
/// let _: hex_session::service::WorkerRootView<'static> =
///     hex_session::service::WorkerRootView {};
/// ```
/// ```compile_fail
/// fn save<'a>(root: &'a hex_session::service::WorkerRootView<'a>)
///     -> &'static hex_session::service::WorkerRootView<'static> { root }
/// ```
/// ```compile_fail
/// fn escape(root: &hex_session::service::WorkerRootView<'_>) {
///     let _ = root.with_original_child_inventory_on_control(|inventory| Ok(inventory));
/// }
/// ```
pub struct WorkerRootView<'a> {
    state: &'a WorkerDispatchState<'a>,
    cancellation: &'a SessionCancellation,
    event: Option<&'a Event>,
    dispatch: WorkerRootDispatch,
}
impl<'a> WorkerRootView<'a> {
    pub(super) fn new(
        state: &'a WorkerDispatchState<'a>,
        cancellation: &'a SessionCancellation,
        event: Option<&'a Event>,
        dispatch: WorkerRootDispatch,
    ) -> Self {
        Self {
            state,
            cancellation,
            event,
            dispatch,
        }
    }
    pub fn dispatch(&self) -> WorkerRootDispatch {
        self.dispatch
    }
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        hex_engine::AudioSourceContext {
            project_generation: self.state.generation,
            editor_revision: self.state.revision,
        }
    }
    pub fn pending_event_revision(&self) -> Option<u64> {
        self.event.map(|event| event.state.revision)
    }
    pub fn has_pending_export(&self) -> bool {
        self.state.export_job.is_some() || self.state.export_completion.is_some()
    }
    /// Actual source correlation only, NOT PCM provenance or an admission seal.
    /// Old async snapshots cannot stand in for this dispatch's current source.
    pub fn matches_current_source_on_control(&self, source: &SharedCanonicalAudioSource) -> bool {
        source.context() == self.context()
            && source.sample_rate() == self.state.session.project().sample_rate()
            && self
                .state
                .current
                .canonical_audio_source
                .as_ref()
                .is_some_and(|current| current.shares_owner_with(source))
    }
    /// Concrete references from these actual dispatch fields, not cloned views
    /// or substituted current/history snapshots. Unsupported parents remain in
    /// require_complete_root_storage_on_control's explicit refusal below.
    fn sampler_fields_on_control(
        &self,
    ) -> hex_engine::sampler_host_fields::SamplerWorkerFields<'_> {
        use hex_engine::sampler_host_fields::{SamplerWorkerFields, SnapshotViewPayload};
        let WorkerDispatchState {
            session: _,
            current,
            compiled: _,
            midi_preview,
            generation: _,
            revision: _,
            control: _,
            clipboard: _,
            media_base,
            recovery,
            document: _,
            journal: _,
            keyboard: _,
            keyboard_hint: _,
            audition: _,
            expression: _,
            device_gesture: _,
            tuning_gesture: _,
            export_job,
            export_completion,
        } = self.state;
        let (event, device_preview, tuning_preview) =
            self.event.map_or((None, None, None), |event| {
                let Event {
                    state,
                    snapshot_reads: _,
                    playback: _,
                    status: _,
                    error: _,
                    audio_batch_failure: _,
                    request_complete: _,
                    computer_keyboard_complete: _,
                    tuning_audition_complete: _,
                    live_expression_complete: _,
                    audio_batch_complete: _,
                    save_complete: _,
                    open_complete: _,
                    project_replaced: _,
                    stopped_playback_restore_complete: _,
                    created: _,
                    duplicate_complete: _,
                    restore_complete: _,
                    device_preview,
                    gesture_ack: _,
                    gesture_protocol_ack: _,
                    gesture_request: _,
                    active_device_gesture: _,
                    tuning_preview,
                    tuning_gesture_ack: _,
                    tuning_gesture_request: _,
                    active_tuning_gesture: _,
                    tuning_library: _,
                } = event;
                (
                    Some(state.as_ref() as &dyn SnapshotViewPayload),
                    device_preview
                        .as_ref()
                        .map(|DevicePreview { identity: _, state }| {
                            state.as_ref() as &dyn SnapshotViewPayload
                        }),
                    tuning_preview
                        .as_ref()
                        .map(|TuningPreview { identity: _, state }| {
                            state.as_ref() as &dyn SnapshotViewPayload
                        }),
                )
            });
        SamplerWorkerFields {
            current: current.as_ref(),
            event,
            device_preview,
            tuning_preview,
            midi_preview,
            media_base: media_base.as_ref(),
            recovery: recovery.as_ref(),
            export_path: export_job.map(
                |ExportJob {
                     revision: _,
                     path,
                     prepared: _,
                     frames: _,
                     controlled: _,
                 }| path,
            ),
            export_completion_path: export_completion.map(
                |ExportEvent {
                     revision: _,
                     path,
                     result: _,
                 }| path,
            ),
        }
    }
    /// Acquire actual Session/cache guards AFTER the caller's outer/inner family
    /// guards, BEFORE the ledger and native barrier. All guards/borrows must stay
    /// through final recheck+commit; no result of this callback is a saved permit.
    pub fn with_original_child_inventory_on_control<T>(
        &self,
        inspect: impl for<'scope> FnOnce(
            &WorkerRootChildInventory<'scope>,
        ) -> Result<T, hex_engine::AudioStorageError>,
    ) -> Result<T, hex_engine::AudioStorageError> {
        self.state
            .session
            .with_resident_pcm_inventory_on_control(|resident| {
                inspect(&WorkerRootChildInventory {
                    roots: self,
                    resident,
                })
            })
    }
}

/// Audited selected children of actual dispatch fields. Enclosing/opaque parents
/// and incomplete prepared/gesture/input/queue inventory still REFUSE closure.
pub struct WorkerRootChildInventory<'a> {
    roots: &'a WorkerRootView<'a>,
    resident: &'a crate::ResidentPcmInventory<'a>,
}
impl WorkerRootChildInventory<'_> {
    pub fn visit_selected_original_children_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.resident
            .visit_original_only_worker_children_on_control(ledger)?;
        self.roots.cancellation.visit(ledger)?;
        if let Some(control) = self.roots.state.control {
            control.visit_cancellation_storage_on_control(ledger)?;
        }
        ledger.borrow_sampler_worker_fields_on_control(&self.roots.sampler_fields_on_control())?;
        Ok(())
    }
    /// Explicitly refuse unqualified physical parents, prepared/gesture/input
    /// roots and transferred channel custody. Selected child totals cannot open
    /// processing. Each actual field is retained in the view, not reconstructed.
    pub fn require_complete_root_storage_on_control(
        &self,
    ) -> Result<(), hex_engine::AudioStorageError> {
        let WorkerDispatchState {
            session: _,
            current: _,
            compiled,
            midi_preview: _,
            generation: _,
            revision: _,
            control: _,
            clipboard,
            media_base: _,
            recovery: _,
            document,
            journal,
            keyboard,
            keyboard_hint,
            audition,
            expression,
            device_gesture,
            tuning_gesture,
            export_job,
            export_completion,
        } = self.roots.state;
        // Borrow-only marker of the ACTUAL currently unsupported fields. No
        // len/count/type/content heuristic manufactures a storage declaration.
        let _ = (
            compiled,
            clipboard,
            document,
            journal,
            keyboard,
            keyboard_hint,
            audition,
            expression,
            device_gesture,
            tuning_gesture,
            export_job,
            export_completion,
        );
        Err(hex_engine::AudioStorageError::UnknownStorage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};
    fn budget() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 512,
            root_rows: 16,
            max_retained_bytes: 4_000_000,
            max_construction_peak_bytes: 4_000_000,
        }
    }
    #[test]
    fn actual_worker_fields_include_all_event_previews_and_distinct_path_buffers() {
        if !hex_engine::audio_storage_constructor_layout_supported()
            || !hex_engine::canonical_midi_storage_layout_supported()
            || !hex_engine::canonical_native_state_storage_layout_supported()
        {
            eprintln!("unsupported worker child constructors; no positive proof");
            return;
        }
        let session = Session::new(48_000, 8, 0).unwrap();
        let midi = hex_engine::empty_midi_preview_on_control();
        let paths: Vec<_> = (0..6)
            .map(|n| {
                let mut path = PathBuf::with_capacity(128 + n * 17);
                path.push(format!("actual-worker-{n}"));
                path
            })
            .collect();
        let current = view_state(&session, midi.clone(), 1, 3, false, Some(&paths[0]));
        let event = Event {
            state: view_state(&session, midi.clone(), 2, 3, false, Some(&paths[1])),
            snapshot_reads: Default::default(),
            playback: None,
            status: None,
            error: None,
            audio_batch_failure: None,
            request_complete: false,
            computer_keyboard_complete: None,
            tuning_audition_complete: None,
            live_expression_complete: None,
            audio_batch_complete: None,
            save_complete: false,
            open_complete: false,
            project_replaced: false,
            stopped_playback_restore_complete: false,
            created: None,
            duplicate_complete: false,
            restore_complete: None,
            device_preview: Some(DevicePreview {
                identity: DevicePreviewIdentity {
                    project_generation: 3,
                    key: crate::device_gesture::GestureKey {
                        generation: 3,
                        token: 1,
                    },
                    sequence: 1,
                    canonical_revision: 2,
                },
                state: view_state(&session, midi.clone(), 3, 3, false, Some(&paths[2])),
            }),
            gesture_ack: None,
            gesture_protocol_ack: false,
            gesture_request: false,
            active_device_gesture: None,
            tuning_preview: Some(TuningPreview {
                identity: TuningPreviewIdentity {
                    project_generation: 3,
                    key: crate::tuning_gesture::TuningGestureKey {
                        generation: 3,
                        token: 2,
                    },
                    sequence: 1,
                    canonical_revision: 2,
                },
                state: view_state(&session, midi.clone(), 4, 3, false, Some(&paths[3])),
            }),
            tuning_gesture_ack: None,
            tuning_gesture_request: false,
            active_tuning_gesture: None,
            tuning_library: None,
        };
        let media_base = Some(paths[4].clone());
        let recovery = Some(paths[5].clone());
        let gesture = GestureMachine::default();
        let tuning = TuningGestureMachine::default();
        let cancellation = SessionCancellation::Unqualified(Arc::new(AtomicBool::new(false)));
        let state = WorkerDispatchState {
            session: &session,
            current: &current,
            compiled: &None,
            midi_preview: &midi,
            generation: 3,
            revision: 2,
            control: None,
            clipboard: &None,
            media_base: &media_base,
            recovery: &recovery,
            document: &None,
            journal: &None,
            keyboard: &None,
            keyboard_hint: &None,
            audition: &None,
            expression: &None,
            device_gesture: &gesture,
            tuning_gesture: &tuning,
            export_job: None,
            export_completion: None,
        };
        let roots = WorkerRootView::new(
            &state,
            &cancellation,
            Some(&event),
            WorkerRootDispatch::PendingEvent,
        );
        let explicit = |omit: Option<usize>| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
            for (index, view) in [
                &current,
                &event.state,
                &event.device_preview.as_ref().unwrap().state,
                &event.tuning_preview.as_ref().unwrap().state,
            ]
            .into_iter()
            .enumerate()
            {
                if omit != Some(index) {
                    view.visit_children_selected_storage_on_control(&mut ledger)
                        .unwrap();
                }
            }
            hex_engine::visit_shared_midi_preview_storage_on_control(&midi, &mut ledger).unwrap();
            for path in [&media_base, &recovery].into_iter().flatten() {
                ledger
                    .borrow_metadata_path_backing_on_control(path)
                    .unwrap();
            }
            ledger.summary().unwrap()
        };
        let expected = explicit(None);
        let fields = roots.sampler_fields_on_control();
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        ledger
            .borrow_sampler_worker_fields_on_control(&fields)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(ledger.summary().unwrap(), expected);
        ledger
            .borrow_sampler_worker_fields_on_control(&fields)
            .unwrap();
        assert_eq!(ledger.summary().unwrap(), expected);
        for index in 0..4 {
            assert!(
                explicit(Some(index)).data_bytes < expected.data_bytes,
                "actual distinct current/event/device/tuning view{index} cannot be omitted"
            );
        }
        assert_eq!(event.state.revision, 2);
        assert_eq!(event.device_preview.as_ref().unwrap().state.revision, 3);
        assert_eq!(event.tuning_preview.as_ref().unwrap().state.revision, 4);
        eprintln!(
            "SUPPORTED actual pending Event + both independent preview views + current, four omission negatives, distinct path capacity, same physical handle dedup; legacy parent still unqualified"
        );
    }

    #[test]
    fn actual_worker_root_children_detect_omitted_midi_field_and_pending_export_path_capacity() {
        if !hex_engine::audio_storage_constructor_layout_supported()
            || !hex_engine::canonical_midi_storage_layout_supported()
            || !hex_engine::canonical_native_state_storage_layout_supported()
        {
            eprintln!("unsupported root child constructors; no positive root storage proof");
            return;
        }
        eprintln!(
            "SUPPORTED actual worker root Session/view/MIDI/cancellation and pending export path child union"
        );
        let signal =
            hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
            &mut Some(0_u8),
            usize::MAX,
            |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
        )
        .unwrap();
        let session = Session::new_with_retained_cancellation_on_control(
            48_000,
            8,
            0,
            signal.bind_worker_on_control(job.route()),
        )
        .unwrap();
        drop(session.compile_prepared_revision(None).unwrap());
        let cancellation =
            SessionCancellation::Retained(signal.bind_worker_on_control(job.route()));
        let midi = hex_engine::empty_midi_preview_on_control();
        let current = view_state(&session, midi.clone(), 7, 11, false, None);
        let gesture = GestureMachine::default();
        let tuning = TuningGestureMachine::default();
        let mut path = PathBuf::with_capacity(256);
        path.push("pending-export.wav");
        assert!(path.capacity() > path.as_os_str().len());
        let completion = ExportEvent {
            revision: 7,
            path,
            result: Err("nothing to export".into()),
        };
        let state = WorkerDispatchState {
            session: &session,
            current: &current,
            compiled: &None,
            midi_preview: &midi,
            generation: 11,
            revision: 7,
            control: None,
            clipboard: &None,
            media_base: &None,
            recovery: &None,
            document: &None,
            journal: &None,
            keyboard: &None,
            keyboard_hint: &None,
            audition: &None,
            expression: &None,
            device_gesture: &gesture,
            tuning_gesture: &tuning,
            export_job: None,
            export_completion: Some(&completion),
        };
        let roots = WorkerRootView::new(&state, &cancellation, None, WorkerRootDispatch::Idle);
        assert!(roots.has_pending_export());
        assert_eq!(
            roots.context(),
            hex_engine::AudioSourceContext {
                project_generation: 11,
                editor_revision: 7,
            }
        );
        let collect = |reservation| {
            roots.with_original_child_inventory_on_control(|inventory| {
                let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
                inventory
                    .resident
                    .visit_original_only_worker_children_on_control(&mut ledger)?;
                current.visit_children_selected_storage_on_control(&mut ledger)?;
                cancellation.visit(&mut ledger)?;
                signal.visit_storage_on_control(&mut ledger)?;
                // Actual worker MIDI field is missing, even though the view has a
                // physically different handle to the very same immutable body.
                assert!(
                    ledger
                        .validate_selected_shared_handles_on_control()
                        .is_err()
                );
                hex_engine::visit_shared_midi_preview_storage_on_control(&midi, &mut ledger)?;
                ledger.validate_selected_shared_handles_on_control()?;
                let without_path = ledger.summary()?;
                inventory.visit_selected_original_children_on_control(&mut ledger)?;
                ledger.validate_selected_shared_handles_on_control()?;
                let measured = ledger.summary()?;
                assert_eq!(
                    measured.data_bytes - without_path.data_bytes,
                    completion.path.capacity()
                );
                assert_eq!(measured.data_allocations - without_path.data_allocations, 1);
                inventory.visit_selected_original_children_on_control(&mut ledger)?;
                assert_eq!(ledger.summary()?, measured);
                assert_eq!(
                    inventory.require_complete_root_storage_on_control(),
                    Err(AudioStorageError::UnknownStorage)
                );
                Ok(measured)
            })
        };
        let measured = collect(budget()).unwrap();
        let mut exact = budget();
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        assert_eq!(completion.path, PathBuf::from("pending-export.wav"));
        assert_eq!(current.revision, 7);
    }
}
