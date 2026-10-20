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
impl WorkerDispatchState<'_> {
    pub(super) fn with_export_completion<'a>(
        &'a self,
        event: &'a ExportEvent,
    ) -> WorkerDispatchState<'a> {
        // All actual surrounding operands remain borrowed. No owner clone or
        // fabricated State/Session enters this fresh backpressure observation.
        let Self {
            session,
            current,
            compiled,
            midi_preview,
            generation,
            revision,
            control,
            clipboard,
            media_base,
            recovery,
            document,
            journal,
            keyboard,
            keyboard_hint,
            audition,
            expression,
            device_gesture,
            tuning_gesture,
            export_job,
            export_completion: _,
        } = self;
        WorkerDispatchState {
            session,
            current,
            compiled,
            midi_preview,
            generation: *generation,
            revision: *revision,
            control: *control,
            clipboard,
            media_base,
            recovery,
            document,
            journal,
            keyboard,
            keyboard_hint,
            audition,
            expression,
            device_gesture,
            tuning_gesture,
            export_job: *export_job,
            export_completion: Some(event),
        }
    }
}

impl DocumentHandle {
    fn document_storage_fields_on_control(
        &self,
    ) -> hex_engine::sampler_host_fields::DocumentFields<'_> {
        use hex_engine::sampler_host_fields::{BodyRef, DocumentFields, DocumentTokenFields};
        let Self { path, token } = self;
        let observation = token.file_token_storage_on_control();
        DocumentFields {
            body: BodyRef::of(self),
            path,
            token: DocumentTokenFields {
                body: BodyRef::of(token),
                contents: observation.borrowed_file_token_arc_field_on_control(),
            },
        }
    }
}

/// Which actual synchronous dispatch is being observed; never an admission flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerRootDispatch {
    ChangedEvent,
    PendingEvent,
    PendingExportResult,
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
    main_files: Option<&'a NativeMainFiles>,
}
impl<'a> WorkerRootView<'a> {
    /// Begin the actual worker Session's pending transition. The next source
    /// context comes from these dispatch fields, never a supplied snapshot or
    /// copied caller counter. This acquires guards only: the closed Audio
    /// operation still must inspect the whole union before any staging birth.
    pub fn original_sampler_pending_session_loan_on_control(
        &self,
        request: &mut Option<hex_project::OriginalSamplerTransitionRequest>,
    ) -> Result<crate::OriginalSamplerSessionLoan<'_>, hex_engine::AudioStorageError> {
        if self.state.current.revision != self.state.revision {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        let mut context = self.context();
        context.editor_revision = context
            .editor_revision
            .checked_add(1)
            .ok_or(hex_engine::AudioStorageError::Overflow)?;
        self.state
            .session
            .original_sampler_pending_session_loan_on_control(request, context)
    }

    /// Hold the actual Session's existing waveform/cache writers before Audio.
    /// This loan supplies no whole-worker closure or native admission proof.
    pub fn original_sampler_session_loan_on_control(
        &self,
    ) -> Result<crate::OriginalSamplerSessionLoan<'_>, hex_engine::AudioStorageError> {
        self.state
            .session
            .original_sampler_session_loan_on_control()
    }

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
            main_files: None,
        }
    }
    pub(super) fn with_actual_main_files(mut self, files: Option<&'a NativeMainFiles>) -> Self {
        self.main_files = files;
        self
    }
    /// The actual persistent worker destination, including real Vec capacity
    /// and occupied File children. This is neither a saved quote nor an empty
    /// substitute for an unqualified channel. It cannot outlive this dispatch.
    pub fn main_file_slots_on_control(
        &self,
    ) -> Result<
        impl hex_engine::sampler_host_fields::FileSlotsFields<'_>,
        hex_engine::AudioStorageError,
    > {
        self.main_files
            .map(NativeMainFiles::fields)
            .ok_or(hex_engine::AudioStorageError::UnknownStorage)
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
    pub fn sampler_fields_on_control(
        &self,
    ) -> hex_engine::sampler_host_fields::SamplerWorkerFields<'_> {
        use hex_engine::sampler_host_fields::{
            BodyRef, JournalFields, SamplerWorkerFields, SnapshotViewPayload,
        };
        let WorkerDispatchState {
            session: _,
            current,
            compiled,
            midi_preview,
            generation: _,
            revision: _,
            control: _,
            clipboard,
            media_base,
            recovery,
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
            legacy_inputs: hex_engine::sampler_host_fields::WorkerLegacyInputFields {
                clipboard: clipboard.as_ref().map(BodyRef::of),
                keyboard: keyboard.as_ref().map(BodyRef::of),
                keyboard_hint: keyboard_hint.as_ref().map(BodyRef::of),
                audition: audition.as_ref().map(BodyRef::of),
                expression: expression.as_ref().map(BodyRef::of),
            },
            gestures: hex_engine::sampler_host_fields::WorkerInlineGestureFields {
                device: device_gesture.inline_worker_gesture_frame_on_control(),
                tuning: tuning_gesture.inline_worker_gesture_frame_on_control(),
            },
            current_owner: hex_engine::sampler_host_fields::WorkerCurrentOwnerFields::Unqualified(
                BodyRef::of(*current),
            ),
            unqualified_compiled: compiled.as_ref().map(|(_, owner)| BodyRef::of(owner)),
            unqualified_playback: self
                .event
                .and_then(|event| event.playback.as_ref())
                .map(BodyRef::of),
            event_metadata: self.event.map(Event::event_metadata_fields_on_control),
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
            export_completion: export_completion.map(ExportEvent::sampler_fields_on_control),
            journal: journal.as_ref().map(|journal| {
                let (path, bytes) = journal.journal_storage_fields_on_control();
                JournalFields {
                    body: BodyRef::of(journal),
                    path,
                    bytes,
                }
            }),
            document: document
                .as_ref()
                .map(DocumentHandle::document_storage_fields_on_control),
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
    #[test]
    fn actual_worker_legacy_inputs_remain_present_and_refuse_without_owner_clone() {
        use hex_engine::{
            computer_keyboard::ComputerKeyboardInput, live_expression::LiveExpressionInput,
            tuning_audition::TuningAuditionInput,
        };
        let mut session = Session::new(48_000, 8, 0).unwrap();
        let hex_project::Created::Track(track) = session
            .editor
            .execute(Command::InsertTrackWithDevice {
                name: "actual input target".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_engine::empty_sampler_state_on_control().unwrap(),
            })
            .unwrap()
        else {
            panic!("actual test instrument track")
        };
        let instrument = session
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let midi = hex_engine::empty_midi_preview_on_control();
        let current = view_state(&session, midi.clone(), 7, 3, false, None);
        let cancellation = SessionCancellation::Unqualified(Arc::new(AtomicBool::new(false)));
        let gesture = GestureMachine::default();
        let tuning = TuningGestureMachine::default();
        for kind in 0..3 {
            let keyboard =
                (kind == 0).then(|| (3, 1, track, Arc::new(ComputerKeyboardInput::default())));
            let audition = (kind == 1).then(|| Arc::new(TuningAuditionInput::new(3)));
            let expression =
                (kind == 2).then(|| Arc::new(LiveExpressionInput::new(3, track, instrument)));
            let counts = || {
                (
                    keyboard
                        .as_ref()
                        .map(|(_, _, _, owner)| Arc::strong_count(owner)),
                    audition.as_ref().map(Arc::strong_count),
                    expression.as_ref().map(Arc::strong_count),
                )
            };
            let before = counts();
            let state = WorkerDispatchState {
                session: &session,
                current: &current,
                compiled: &None,
                midi_preview: &midi,
                generation: 3,
                revision: 7,
                control: None,
                clipboard: &None,
                media_base: &None,
                recovery: &None,
                document: &None,
                journal: &None,
                keyboard: &keyboard,
                keyboard_hint: &None,
                audition: &audition,
                expression: &expression,
                device_gesture: &gesture,
                tuning_gesture: &tuning,
                export_job: None,
                export_completion: None,
            };
            let roots = WorkerRootView::new(&state, &cancellation, None, WorkerRootDispatch::Idle);
            let fields = roots.sampler_fields_on_control();
            assert_eq!(fields.legacy_inputs.keyboard.is_some(), kind == 0);
            assert_eq!(fields.legacy_inputs.audition.is_some(), kind == 1);
            assert_eq!(fields.legacy_inputs.expression.is_some(), kind == 2);
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
            assert_eq!(
                ledger.borrow_sampler_worker_fields_on_control(&fields),
                Err(AudioStorageError::UnknownStorage)
            );
            assert!(ledger.summary().is_err());
            assert_eq!(counts(), before);
            assert!(
                roots
                    .with_original_child_inventory_on_control(
                        |inventory| inventory.require_complete_root_storage_on_control(),
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn actual_worker_pending_loan_derives_context_and_rejects_stale_or_exhausted_dispatch() {
        use hex_engine::sampler_host_fields::OriginalSamplerPendingSession;
        use hex_project::OriginalSamplerTransitionRequest;
        let signal =
            hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
            &mut Some(0_u8),
            usize::MAX,
            |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
        )
        .unwrap();
        let mut session = Session::new_with_retained_cancellation_on_control(
            48_000,
            8,
            0,
            signal.bind_worker_on_control(job.route()),
        )
        .unwrap();
        let cancellation =
            SessionCancellation::Retained(signal.bind_worker_on_control(job.route()));
        let midi = hex_engine::empty_midi_preview_on_control();
        let current = view_state(&session, midi.clone(), 7, 3, false, None);
        let gesture = GestureMachine::default();
        let tuning = TuningGestureMachine::default();
        {
            let state = WorkerDispatchState {
                session: &session,
                current: &current,
                compiled: &None,
                midi_preview: &midi,
                generation: 3,
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
                export_completion: None,
            };
            let roots = WorkerRootView::new(&state, &cancellation, None, WorkerRootDispatch::Idle);
            let mut request = Some(OriginalSamplerTransitionRequest::command(
                Command::MasterGain { gain: 0.5 },
            ));
            {
                let loan = roots
                    .original_sampler_pending_session_loan_on_control(&mut request)
                    .unwrap();
                assert!(request.is_none());
                assert_eq!(
                    loan.pending_context_on_control().unwrap(),
                    hex_engine::AudioSourceContext {
                        project_generation: 3,
                        editor_revision: 8,
                    }
                );
                assert!(std::ptr::eq(
                    loan.clone_source_on_control().unwrap().unwrap(),
                    session.project()
                ));
                assert!(
                    session
                        .editor
                        .require_no_original_sampler_pending_on_control()
                        .is_err()
                );
                // The real legacy outer owner is still present. A guard loan
                // does not qualify it or construct any candidate/source pair.
                assert!(matches!(
                    roots.sampler_fields_on_control().current_owner,
                    hex_engine::sampler_host_fields::WorkerCurrentOwnerFields::Unqualified(_)
                ));
            }
            // A second attempt cannot replace the first pending command.
            let mut another = Some(OriginalSamplerTransitionRequest::undo());
            assert!(matches!(
                roots.original_sampler_pending_session_loan_on_control(&mut another),
                Err(AudioStorageError::ExternalReadersPending)
            ));
            assert!(another.is_some());
            let stale_state = WorkerDispatchState {
                revision: 8,
                ..state
            };
            let stale =
                WorkerRootView::new(&stale_state, &cancellation, None, WorkerRootDispatch::Idle);
            assert!(matches!(
                stale.original_sampler_pending_session_loan_on_control(&mut another),
                Err(AudioStorageError::SourceMismatch)
            ));
            assert!(another.is_some());
        }
        session.clear_original_sampler_pending_on_control().unwrap();
        let exhausted_current = view_state(&session, midi.clone(), u64::MAX, 3, false, None);
        let state = WorkerDispatchState {
            session: &session,
            current: &exhausted_current,
            compiled: &None,
            midi_preview: &midi,
            generation: 3,
            revision: u64::MAX,
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
            export_completion: None,
        };
        let exhausted = WorkerRootView::new(&state, &cancellation, None, WorkerRootDispatch::Idle);
        let mut request = Some(OriginalSamplerTransitionRequest::redo());
        assert!(matches!(
            exhausted.original_sampler_pending_session_loan_on_control(&mut request),
            Err(AudioStorageError::Overflow)
        ));
        assert!(request.is_some());
        session
            .editor
            .require_no_original_sampler_pending_on_control()
            .unwrap();
    }

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
        let mut event = Event {
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
        let mut status = String::with_capacity(173);
        status.push_str("actual pending startup status");
        let mut error = String::with_capacity(239);
        error.push_str("actual retained error");
        event.status = Some(status);
        event.error = Some(error);
        let text = |capacity| {
            let mut value = String::with_capacity(capacity);
            value.push_str("actual metadata");
            value
        };
        let mut failed_path = PathBuf::with_capacity(311);
        failed_path.push("Case/failed-member.wav");
        event.audio_batch_failure = Some(AudioBatchMemberFailure {
            member: 3,
            path: failed_path,
            explanation: text(193),
        });
        event.gesture_ack = Some(GestureAck {
            key: crate::device_gesture::GestureKey {
                generation: 3,
                token: 8,
            },
            phase: crate::device_gesture::GesturePhase::Commit,
            sequence: 4,
            accepted: false,
            reason: Some(text(127)),
        });
        event.tuning_gesture_ack = Some(TuningGestureAck {
            key: crate::tuning_gesture::TuningGestureKey {
                generation: 3,
                token: 9,
            },
            phase: crate::tuning_gesture::TuningGesturePhase::Commit,
            sequence: 5,
            accepted: false,
            reason: Some(text(151)),
        });
        let mut slots = Vec::with_capacity(8);
        slots.push(hex_storage::tuning_library::Slot {
            number: 1,
            name: Some(text(211)),
            error: Some(text(223)),
        });
        slots.push(hex_storage::tuning_library::Slot {
            number: 2,
            name: Some(text(227)),
            error: Some(text(229)),
        });
        event.tuning_library = Some(TuningLibraryResult {
            operation_id: 27,
            slots,
            selected_slot: Some(1),
            loaded_name: Some(text(251)),
            error: Some(text(257)),
        });
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
        let mut expected = explicit(None);
        let batch = event.audio_batch_failure.as_ref().unwrap();
        let library = event.tuning_library.as_ref().unwrap();
        let metadata_bytes = event.status.as_ref().unwrap().capacity()
            + event.error.as_ref().unwrap().capacity()
            + batch.path.capacity()
            + batch.explanation.capacity()
            + event
                .gesture_ack
                .as_ref()
                .unwrap()
                .reason
                .as_ref()
                .unwrap()
                .capacity()
            + event
                .tuning_gesture_ack
                .as_ref()
                .unwrap()
                .reason
                .as_ref()
                .unwrap()
                .capacity()
            + library.slots.capacity() * std::mem::size_of::<hex_storage::tuning_library::Slot>()
            + library.slots[0].name.as_ref().unwrap().capacity()
            + library.slots[0].error.as_ref().unwrap().capacity()
            + library.slots[1].name.as_ref().unwrap().capacity()
            + library.slots[1].error.as_ref().unwrap().capacity()
            + library.loaded_name.as_ref().unwrap().capacity()
            + library.error.as_ref().unwrap().capacity();
        expected.data_allocations += 13;
        // Two actual stack bodies now join this selected walk. These are
        // physical DATA rows, not two newly allocated heap owners.
        let gesture_bytes = std::mem::size_of_val(&gesture) + std::mem::size_of_val(&tuning);
        expected.data_allocations += 2;
        expected.data_bytes += metadata_bytes + gesture_bytes;
        expected.retained_bytes += metadata_bytes + gesture_bytes;
        expected.construction_peak_bytes += metadata_bytes + gesture_bytes;
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
        let mut omitted = roots.sampler_fields_on_control();
        omitted.event_metadata = None;
        let mut missing = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        missing
            .borrow_sampler_worker_fields_on_control(&omitted)
            .unwrap();
        assert_eq!(
            missing.summary().unwrap().data_bytes + metadata_bytes,
            expected.data_bytes
        );
        let foreign =
            String::from("foreign field with equal content is not the pending Event field");
        let mut wrong = roots.sampler_fields_on_control();
        wrong.event_metadata.as_mut().unwrap().status = Some(&foreign);
        let mut invalid = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        assert_eq!(
            invalid.borrow_sampler_worker_fields_on_control(&wrong),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(invalid.summary().is_err());
        let mut exact = budget();
        exact.max_retained_bytes = expected.retained_bytes;
        exact.max_construction_peak_bytes = expected.construction_peak_bytes;
        let collect = |reservation| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            ledger.borrow_sampler_worker_fields_on_control(&fields)?;
            ledger.validate_selected_shared_handles_on_control()?;
            ledger.summary()
        };
        assert_eq!(collect(exact).unwrap(), expected);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        exact.max_retained_bytes += 1;
        exact.max_construction_peak_bytes -= 1;
        assert!(collect(exact).is_err());
        eprintln!(
            "SUPPORTED actual pending Event metadata:13 allocations/{metadata_bytes} bytes, Slot Vec length2/capacity{}, omitted metadata/foreign field/dedup/both selected caps; outer Event/runtime admission OPEN",
            library.slots.capacity()
        );
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

        // Even an empty compiled revision owns actual constructor products.
        // Absence of canonical MIDI aliases must NEVER stand in for that root.
        let compiled = Some((2, session.compile_prepared_revision(None).unwrap()));
        let compiled_state = WorkerDispatchState {
            compiled: &compiled,
            ..state
        };
        let compiled_roots = WorkerRootView::new(
            &compiled_state,
            &cancellation,
            None,
            WorkerRootDispatch::Idle,
        );
        let compiled_fields = compiled_roots.sampler_fields_on_control();
        assert!(compiled_fields.unqualified_compiled.is_some());
        assert!(compiled_fields.unqualified_playback.is_none());
        let mut legacy = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        assert_eq!(
            legacy.borrow_sampler_worker_fields_on_control(&compiled_fields),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(legacy.summary().is_err());

        // The actual pending Event owns Playback independently of the worker's
        // current compiled slot. Neither path may substitute an empty marker.
        event.playback = Some(native_playback(Arc::clone(&compiled.as_ref().unwrap().1)));
        let playback_state = WorkerDispatchState {
            compiled: &None,
            ..compiled_state
        };
        let playback_roots = WorkerRootView::new(
            &playback_state,
            &cancellation,
            Some(&event),
            WorkerRootDispatch::PendingEvent,
        );
        let playback_fields = playback_roots.sampler_fields_on_control();
        assert!(playback_fields.unqualified_compiled.is_none());
        assert!(playback_fields.unqualified_playback.is_some());
        let mut legacy = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        assert_eq!(
            legacy.borrow_sampler_worker_fields_on_control(&playback_fields),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(legacy.summary().is_err());
    }

    #[test]
    fn actual_worker_root_children_detect_omitted_midi_export_journal_and_document() {
        if !hex_engine::audio_storage_constructor_layout_supported()
            || !hex_engine::canonical_midi_storage_layout_supported()
            || !hex_engine::canonical_native_state_storage_layout_supported()
        {
            eprintln!("unsupported root child constructors; no positive root storage proof");
            return;
        }
        eprintln!(
            "SUPPORTED actual worker root Session/view/MIDI/cancellation, export, nonempty Journal and saved Document union"
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
        let gesture = GestureMachine::default();
        let tuning = TuningGestureMachine::default();
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = std::env::temp_dir().join(format!(
            "hex395-worker-journal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let scratch = Scratch(directory);
        let mut journal_path = scratch.0.join("Case.hex-journal");
        journal_path.reserve(317);
        let mut journal = hex_storage::Journal::open(journal_path).unwrap();
        journal.checkpoint(session.project()).unwrap();
        let journal = Some(journal);
        let (actual_journal_path, actual_checkpoint) = journal
            .as_ref()
            .unwrap()
            .journal_storage_fields_on_control();
        let actual_checkpoint = actual_checkpoint.unwrap();
        assert!(!actual_checkpoint.is_empty());
        let document_path = scratch.0.join("OriginalCase.hex");
        let token = hex_storage::save_new_with_token(&document_path, session.project()).unwrap();
        let document = Some(DocumentHandle {
            path: document_path,
            token,
        });
        let current = view_state(
            &session,
            midi.clone(),
            7,
            11,
            false,
            Some(&document.as_ref().unwrap().path),
        );
        let token_len = document
            .as_ref()
            .unwrap()
            .token
            .file_token_storage_on_control()
            .borrowed_file_token_arc_field_on_control()
            .len();
        let token_layout = std::alloc::Layout::array::<std::sync::atomic::AtomicUsize>(2)
            .unwrap()
            .extend(std::alloc::Layout::array::<u8>(token_len).unwrap())
            .unwrap()
            .0
            .pad_to_align();
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
            document: &document,
            journal: &journal,
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
                let without_metadata = ledger.summary()?;
                // Deliberately omit the ACTUAL Journal to distinguish that
                // owning storage from the independent completion allocations.
                let mut omitted_journal = roots.sampler_fields_on_control();
                omitted_journal.journal = None;
                omitted_journal.document = None;
                ledger.borrow_sampler_worker_fields_on_control(&omitted_journal)?;
                ledger.validate_selected_shared_handles_on_control()?;
                let without_document = ledger.summary()?;
                // BOTH completion allocations and BOTH actual inline gesture
                // bodies join this walk; none was in the manual baseline.
                let message = completion.result.as_ref().unwrap_err();
                assert_eq!(
                    without_document.data_bytes - without_metadata.data_bytes,
                    completion.path.capacity()
                        + message.capacity()
                        + std::mem::size_of_val(&gesture)
                        + std::mem::size_of_val(&tuning)
                );
                assert_eq!(
                    without_document.data_allocations - without_metadata.data_allocations,
                    4
                );
                omitted_journal.document = roots.sampler_fields_on_control().document;
                ledger.borrow_sampler_worker_fields_on_control(&omitted_journal)?;
                ledger.validate_selected_shared_handles_on_control()?;
                let without_journal = ledger.summary()?;
                assert_eq!(
                    without_journal.data_bytes - without_document.data_bytes,
                    document.as_ref().unwrap().path.capacity() + token_layout.size()
                );
                assert_eq!(
                    without_journal.data_allocations - without_document.data_allocations,
                    2
                );
                inventory.visit_selected_original_children_on_control(&mut ledger)?;
                ledger.validate_selected_shared_handles_on_control()?;
                let measured = ledger.summary()?;
                assert_eq!(
                    measured.data_bytes - without_journal.data_bytes,
                    actual_journal_path.capacity() + actual_checkpoint.capacity()
                );
                assert_eq!(
                    measured.data_allocations - without_journal.data_allocations,
                    2
                );
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
        exact.max_retained_bytes += 1;
        exact.max_construction_peak_bytes -= 1;
        assert!(collect(exact).is_err());
        assert_eq!(completion.path, PathBuf::from("pending-export.wav"));
        assert_eq!(current.revision, 7);
    }

    #[test]
    fn actual_document_token_four_storage_births_and_clone_alias_union() {
        assert!(
            hex_engine::document_file_token_storage_layout_supported(),
            "unsupported FileToken layout: no positive document component claim"
        );
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = std::env::temp_dir().join(format!(
            "hex395-document-token-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let scratch = Scratch(directory);
        let path = scratch.0.join("MixedCase.hex");
        let project = hex_project::Project::new(48000).unwrap();
        let saved = hex_storage::save_new_with_token(&path, &project).unwrap();
        let (_, loaded) = hex_storage::load_with_token(&path).unwrap();
        let replaced = hex_storage::replace_existing(&path, &loaded, &project).unwrap();
        let recovered = hex_storage::load_recovery(&path).unwrap();
        let mut documents = Vec::with_capacity(8);
        for token in [saved, loaded, replaced.token, recovered.token] {
            let mut owned_path = path.clone();
            owned_path.reserve(117 + documents.len() * 31);
            documents.push(DocumentHandle {
                path: owned_path,
                token,
            });
        }
        // Equal CONTENTS do not merge the four actual constructor allocations.
        let first_bytes = documents[0]
            .token
            .file_token_storage_on_control()
            .borrowed_file_token_arc_field_on_control();
        for document in &documents[1..] {
            let bytes = document
                .token
                .file_token_storage_on_control()
                .borrowed_file_token_arc_field_on_control();
            assert_eq!(first_bytes.as_ref(), bytes.as_ref());
            assert!(!Arc::ptr_eq(first_bytes, bytes)); // Test observation, not authority.
        }
        documents.push(DocumentHandle {
            path: path.clone(),
            token: documents[0].token.clone(),
        });
        let mut foreign_field = documents[0].document_storage_fields_on_control();
        foreign_field.token.contents = documents[1]
            .token
            .file_token_storage_on_control()
            .borrowed_file_token_arc_field_on_control();
        let mut invalid = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        assert_eq!(
            invalid.borrow_sampler_document_fields_on_control(&foreign_field),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(invalid.summary().is_err());
        let collect = |reservation| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            // Actual Vec capacity includes all spare inline Document bodies.
            ledger.borrow_metadata_vec_backing_on_control(&documents)?;
            ledger.borrow_sampler_document_fields_on_control(
                &documents[0].document_storage_fields_on_control(),
            )?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err(),
                "actual cloned token alias must not disappear"
            );
            for document in &documents[1..] {
                ledger.borrow_sampler_document_fields_on_control(
                    &document.document_storage_fields_on_control(),
                )?;
            }
            ledger.validate_selected_shared_handles_on_control()?;
            let complete_selected = ledger.summary()?;
            for document in &documents {
                ledger.borrow_sampler_document_fields_on_control(
                    &document.document_storage_fields_on_control(),
                )?;
            }
            assert_eq!(ledger.summary()?, complete_selected);
            assert!(
                complete_selected.require_complete().is_err(),
                "no fabricated whole-root summary"
            );
            Ok::<_, AudioStorageError>(complete_selected)
        };
        let measured = collect(budget()).unwrap();
        let token_len = documents[0]
            .token
            .file_token_storage_on_control()
            .borrowed_file_token_arc_field_on_control()
            .len();
        let token_layout = std::alloc::Layout::array::<std::sync::atomic::AtomicUsize>(2)
            .unwrap()
            .extend(std::alloc::Layout::array::<u8>(token_len).unwrap())
            .unwrap()
            .0
            .pad_to_align();
        assert_eq!(measured.data_allocations, 10); // Vec + five paths + FOUR token bodies.
        assert_eq!(
            measured.data_bytes,
            documents.capacity() * std::mem::size_of::<DocumentHandle>()
                + documents
                    .iter()
                    .map(|document| document.path.capacity())
                    .sum::<usize>()
                + token_layout.size() * 4
        );
        let mut exact = budget();
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        exact.max_retained_bytes += 1;
        exact.max_construction_peak_bytes -= 1;
        assert!(collect(exact).is_err());
        // Existing content-check and target/recovery semantics are untouched.
        for document in &documents {
            document.token.check_unchanged(&path).unwrap();
        }
        eprintln!(
            "SUPPORTED actual Save/Load/Replace/Recovery FileToken bodies4, physical fields5; selected union only, constructor overlap/native admission OPEN"
        );
    }
}
