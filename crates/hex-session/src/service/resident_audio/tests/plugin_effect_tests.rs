//! Canonical request/admission tests only. This sink deliberately cannot admit
//! VST3 state; actual helper/endpoint/PCM qualification belongs to the native consumer.
use super::*;

fn reservation() -> crate::ResidentNativeReservation {
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
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
    }
}
fn receive(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
}
fn insertion(state: &ViewState) -> PluginEffectInsertion {
    PluginEffectInsertion {
        context: hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: state.revision,
        },
        selection: PluginEffectSelection {
            module: PathBuf::from("/original-fixture.vst3"),
            executable: "Contents/MacOS/fixture".into(),
            cid: [1; 16],
            executable_sha256: "1".repeat(64),
            info_sha256: "2".repeat(64),
        },
        destination: PluginEffectDestination::Main,
        before: None,
        control: Arc::new(PluginEffectControl::default()),
    }
}
fn service() -> (Service, Arc<Observed>, Event) {
    let observed = Arc::new(Observed::default());
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        }),
        reservation(),
    )
    .unwrap();
    let initial = receive(&service);
    assert!(initial.error.as_ref().unwrap().contains("fixture offline"));
    (service, observed, initial)
}

#[test]
fn pending_plugin_request_is_not_a_terminal_and_cancel_wakes_without_native_readiness() {
    let (service, observed, initial) = service();
    let request = insertion(&initial.state);
    service
        .submit(Request::InsertPluginEffect(request.clone()))
        .unwrap();
    let pending = receive(&service);
    assert!(pending.request_complete);
    assert!(pending.plugin_effect_complete.is_empty());
    assert_eq!(pending.state.project, initial.state.project);
    assert_eq!(observed.plugin_preparations.load(Ordering::Acquire), 1);

    // Reusing the same request cannot retire the original preparation.
    service
        .submit(Request::InsertPluginEffect(request.clone()))
        .unwrap();
    let duplicate = receive(&service);
    assert!(duplicate.error.unwrap().contains("already submitted"));
    assert!(duplicate.plugin_effect_complete.is_empty());
    assert!(observed.plugin_terminals.lock().unwrap().is_empty());

    assert!(request.control.cancel());
    let cancelled = receive(&service);
    assert!(!cancelled.request_complete);
    assert_eq!(cancelled.plugin_effect_complete.len(), 1);
    assert!(!cancelled.plugin_effect_complete[0].accepted);
    assert!(Arc::ptr_eq(
        &cancelled.plugin_effect_complete[0].control,
        &request.control
    ));
    assert_eq!(cancelled.state.project, initial.state.project);
    assert!(!cancelled.state.can_undo);
    assert_eq!(observed.plugin_preparations.load(Ordering::Acquire), 1);
    let terminals = observed.plugin_terminals.lock().unwrap();
    assert_eq!(terminals.len(), 1);
    assert!(Arc::ptr_eq(&terminals[0].0, &request.control));
    assert!(!terminals[0].1);
    assert!(!request.control.cancel());
    assert!(request.control.is_cancelled());
}

#[test]
fn canonical_edit_invalidates_pending_plugin_in_the_same_event_without_retargeting() {
    let (service, observed, initial) = service();
    let request = insertion(&initial.state);
    service
        .submit(Request::InsertPluginEffect(request.clone()))
        .unwrap();
    receive(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "next".into(),
        }))
        .unwrap();
    let edited = receive(&service);
    assert!(edited.error.is_none(), "{:?}", edited.error);
    assert!(edited.request_complete);
    assert_eq!(edited.state.revision, initial.state.revision + 1);
    assert_eq!(edited.state.project.tracks().len(), 1);
    assert!(edited.state.project.main_devices().is_empty());
    assert_eq!(edited.plugin_effect_complete.len(), 1);
    assert!(!edited.plugin_effect_complete[0].accepted);
    assert!(Arc::ptr_eq(
        &edited.plugin_effect_complete[0].control,
        &request.control
    ));
    assert_eq!(observed.plugin_preparations.load(Ordering::Acquire), 1);
    service.submit(Request::Undo).unwrap();
    let undone = receive(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert!(undone.state.project.tracks().is_empty());
    assert!(!undone.state.can_undo);
}

#[test]
fn native_ready_continuation_refuses_wrong_provider_without_history_or_fifo_completion() {
    let (service, observed, initial) = service();
    let request = insertion(&initial.state);
    service
        .submit(Request::InsertPluginEffect(request.clone()))
        .unwrap();
    receive(&service);
    observed.plugin_ready.store(true, Ordering::Release);
    let refused = receive(&service);
    assert!(!refused.request_complete);
    assert!(refused.error.unwrap().contains("different device provider"));
    assert_eq!(refused.plugin_effect_complete.len(), 1);
    assert!(!refused.plugin_effect_complete[0].accepted);
    assert!(Arc::ptr_eq(
        &refused.plugin_effect_complete[0].control,
        &request.control
    ));
    assert_eq!(refused.state.project, initial.state.project);
    assert!(!refused.state.can_undo);
    assert_eq!(observed.plugin_preparations.load(Ordering::Acquire), 2);
    assert_eq!(observed.plugin_terminals.lock().unwrap().len(), 1);
}

#[test]
fn invalid_plugin_metadata_and_stale_context_never_reach_native_preparation() {
    let (service, observed, initial) = service();
    for case in 0..6 {
        let mut request = insertion(&initial.state);
        match case {
            0 => request.context.project_generation += 1,
            1 => request.selection.module = "/original\0fixture.vst3".into(),
            2 => request.selection.executable = "../other".into(),
            3 => request.selection.executable_sha256 = "A".repeat(64),
            4 => request.selection.cid = [0; 16],
            _ => {
                assert!(request.control.cancel());
            }
        }
        service
            .submit(Request::InsertPluginEffect(request.clone()))
            .unwrap();
        let refused = receive(&service);
        assert!(refused.request_complete);
        assert!(refused.error.is_some());
        assert_eq!(refused.plugin_effect_complete.len(), 1);
        assert!(!refused.plugin_effect_complete[0].accepted);
        assert!(Arc::ptr_eq(
            &refused.plugin_effect_complete[0].control,
            &request.control
        ));
        assert_eq!(refused.state.project, initial.state.project);
        assert!(!refused.state.can_undo);
    }
    assert_eq!(observed.plugin_preparations.load(Ordering::Acquire), 0);
    assert!(observed.plugin_terminals.lock().unwrap().is_empty());
}

#[test]
fn plugin_transaction_cancel_race_is_resolved_at_final_publication_admission() {
    // Exercise the owning transaction gate with the existing real builtin compiler.
    // This is deliberately not proof of VST3 compilation or native source admission.
    for cancel_before in [true, false] {
        let observed = Arc::new(Observed::default());
        let mut sink = OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        };
        let mut session = Session::new(48_000, 16, 1024)
            .unwrap()
            .with_native_reservation(reservation())
            .unwrap();
        let before = session.project().snapshot();
        let control = Arc::new(PluginEffectControl::default());
        if cancel_before {
            *observed.cancel_at_admission.lock().unwrap() = Some(Arc::clone(&control));
        } else {
            *observed.cancel_after_admission.lock().unwrap() = Some(Arc::clone(&control));
        }
        let result = session.plugin_effect_resident_snapshot(
            Command::AddTrack {
                name: "transaction".into(),
            },
            Arc::clone(&control),
            hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 1,
            },
            &mut sink,
            sampler_export::Admission::default(),
            None,
        );
        if cancel_before {
            assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("cancelled before admission")
            );
            assert_eq!(session.project().snapshot(), before);
            assert!(!session.editor.can_undo());
            assert!(observed.accepted.lock().unwrap().is_empty());
        } else {
            assert!(result.is_ok());
            assert_eq!(session.project().tracks().len(), 1);
            assert!(session.editor.can_undo());
            assert_eq!(observed.accepted.lock().unwrap().len(), 1);
            assert!(!control.cancel());
        }
    }
}
