//! Stopped foreign-program history pruning across resident previews, commits,
//! restores, refusals, and no-op commands (including in-flight predecessor readiness).
use super::*;
use crate::device_gesture::{
    GestureKey, GestureRequest, GestureTarget, ParameterValue as GestureValue,
};
use hex_lattice_contract::ForeignProcessorBinding;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, DeviceId, Editor, Project, TrackId,
};
use std::{
    num::NonZeroU64,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Default)]
struct SinkState {
    report_pending: AtomicBool,
    refuse_confirm: AtomicBool,
    program_revision: AtomicU64,
    next_audio_revision: AtomicU64,
    applied_revision: AtomicU64,
    latest_submission: Mutex<Option<ResidentRuntimeLoop>>,
    refused_submission: Mutex<Option<(ResidentRuntimeLoop, u64)>>,
    fenced: Mutex<Option<hex_engine::AudioSourceContext>>,
    begun: AtomicU64,
    confirmed: AtomicU64,
    finished: AtomicU64,
}

struct StoppedForeignHistorySink {
    owner: hex_engine::ProcessorChainOwner,
    device_id: u64,
    binding: ForeignProcessorBinding,
    state: Arc<SinkState>,
}

impl WorkerAudioSink for StoppedForeignHistorySink {
    fn initialize(&mut self, _shutdown: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, _snapshot: Arc<CanonicalAudioSourceSnapshot>) {}
    fn poll(&mut self) {}
    fn close(&mut self) {}
    fn resident_publication_enabled(&self) -> bool {
        true
    }
    fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
        if let Some((submission, applied_revision)) = *self.state.refused_submission.lock().unwrap()
        {
            return ResidentPublicationReadiness::Refused {
                submission,
                applied_revision,
                publication_in_flight: false,
            };
        }
        let Some(submission) = *self.state.latest_submission.lock().unwrap() else {
            return ResidentPublicationReadiness::Idle {
                output_identity: 1,
                applied_revision: 0,
                publication_in_flight: false,
            };
        };
        if self.state.report_pending.swap(false, Ordering::AcqRel) {
            ResidentPublicationReadiness::Pending {
                submission,
                applied_revision: self.state.applied_revision.load(Ordering::Acquire),
                publication_in_flight: true,
            }
        } else {
            self.state
                .applied_revision
                .store(submission.audio_revision, Ordering::Release);
            ResidentPublicationReadiness::Applied {
                submission,
                publication_in_flight: false,
            }
        }
    }
    fn prepare_resident_effects(
        &mut self,
        _input: ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        let next = self.state.program_revision.fetch_add(1, Ordering::AcqRel) + 1;
        hex_engine::AudioCandidateEffects::with_test_foreign_effect_program(
            self.owner,
            self.device_id,
            self.binding,
            NonZeroU64::new(next).unwrap(),
            7,
            prepare,
        )
        .map(Some)
        .map_err(|error| error.to_string())
    }
    fn begin_stopped_foreign_history(
        &mut self,
        predecessor: ResidentGraphSubmission,
        successor: hex_engine::AudioSourceContext,
    ) -> Result<bool, String> {
        let mut fenced = self.state.fenced.lock().unwrap();
        if fenced.is_some() {
            return Err("stopped vst3 history fence is already held".into());
        }
        assert_eq!(predecessor.output_identity, 1);
        *fenced = Some(successor);
        self.state.begun.fetch_add(1, Ordering::AcqRel);
        Ok(true)
    }
    fn confirm_stopped_foreign_history(
        &mut self,
        context: hex_engine::AudioSourceContext,
        accepted: ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        let fenced = self.state.fenced.lock().unwrap();
        if let Some(held) = *fenced {
            assert_eq!(held, context, "confirm context must match fenced context");
            assert!(matches!(
                accepted,
                ResidentPublicationAcceptance::Submitted {
                    output_identity: 1,
                    ..
                }
            ));
            if self.state.refuse_confirm.swap(false, Ordering::AcqRel) {
                let latest = self.state.latest_submission.lock().unwrap().unwrap();
                let prev_applied = self.state.applied_revision.load(Ordering::Acquire);
                *self.state.refused_submission.lock().unwrap() = Some((latest, prev_applied));
                return Err("injected stopped vst3 history refusal".into());
            }
            let latest = self.state.latest_submission.lock().unwrap().unwrap();
            self.state
                .applied_revision
                .store(latest.audio_revision, Ordering::Release);
            self.state.confirmed.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }
    fn finish_resident_effect_edit(
        &mut self,
        context: hex_engine::AudioSourceContext,
        _accepted: bool,
    ) {
        let mut fenced = self.state.fenced.lock().unwrap();
        if *fenced == Some(context) {
            *fenced = None;
            self.state.finished.fetch_add(1, Ordering::AcqRel);
        }
    }
    fn received_foreign_program_on_control(
        &mut self,
        binding: ForeignProcessorBinding,
    ) -> hex_engine::ForeignProgramFrontier {
        assert_eq!(binding, self.binding);
        if self.state.fenced.lock().unwrap().is_some() {
            hex_engine::ForeignProgramFrontier::Stopped {
                open_quantum: None,
                worker_empty: true,
            }
        } else {
            hex_engine::ForeignProgramFrontier::Empty
        }
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn publish_resident_transition(
        &mut self,
        _playback: Playback,
        _resources: Option<&PreparedDeviceResources>,
        _observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        _policy: ResidentPublicationPolicy,
        _limit: usize,
        _admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
        _inputs: ResidentPublicationInputs<'_>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        let audio_revision = self
            .state
            .next_audio_revision
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        *self.state.refused_submission.lock().unwrap() = None;
        *self.state.latest_submission.lock().unwrap() = Some(ResidentRuntimeLoop {
            source_origin: ResidentSourceOrigin::Unknown,
            output_identity: 1,
            audio_revision,
            source_context: Some(metadata.context),
            section_loop: None,
        });
        // Each newly submitted revision starts in flight (`Pending`) until the
        // next readiness/fence observation settles it to `Applied`.
        self.state.report_pending.store(true, Ordering::Release);
        Ok(ResidentPublicationAcceptance::Submitted {
            output_identity: 1,
            audio_revision,
        })
    }
}

fn build_group_track_with_automated_vst3() -> (Project, TrackId, TrackId, DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(child) = editor
        .execute(Command::AddTrack {
            name: "synth".into(),
        })
        .unwrap()
    else {
        panic!("expected track");
    };
    let Created::Track(group) = editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[child]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("expected group track");
    };
    let state = hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([0x42; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [1; 32],
                info_sha256: [2; 32],
            },
        },
        component: Some(&[4, 5]),
        controller: Some(&[]),
    })
    .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track: child,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("expected device");
    };
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: 7 },
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Linear,
            }],
        })
        .unwrap();
    (editor.project().clone(), child, group, device)
}

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

fn next_event(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
}

#[test]
fn stopped_group_track_volume_drag_prunes_foreign_program_history_across_preview_commit_and_cancel()
{
    let (project, child, group, device) = build_group_track_with_automated_vst3();
    let dir = std::env::temp_dir().join(format!(
        "hex-stopped-foreign-history-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.hex");
    hex_storage::save_new(&path, &project).unwrap();

    let binding = ForeignProcessorBinding {
        root: 1,
        instance: device.value(),
        graph: 1,
        epoch: 1,
    };
    let sink_state = Arc::new(SinkState::default());
    let service = Service::with_audio_sink_and_native_reservation(
        32,
        1024,
        None,
        Box::new(StoppedForeignHistorySink {
            owner: hex_engine::ProcessorChainOwner::Track(child.value()),
            device_id: device.value(),
            binding,
            state: Arc::clone(&sink_state),
        }),
        reservation(),
    )
    .unwrap();
    let initial = next_event(&service);
    assert!(initial.error.is_none(), "{:?}", initial.error);

    service
        .submit(Request::OpenWithAudioMode {
            source: ProjectSource::Saved(path),
            relinks: BTreeMap::new(),
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    let opened = next_event(&service);
    assert!(opened.error.is_none(), "{:?}", opened.error);
    let generation = opened
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context()
        .project_generation;

    // 1) Drag the group track volume across > MAX_RETAINED_FOREIGN_PROGRAMS preview
    // updates while stopped and while the previous preview submission is still Pending
    // (reproducing the exact rapid-drag race in the session log).
    sink_state.report_pending.store(true, Ordering::Release);
    let key1 = GestureKey {
        generation,
        token: 1,
    };
    service
        .submit(Request::device_gesture(GestureRequest::Begin {
            key: key1,
            target: GestureTarget::TrackGain { track: group },
        }))
        .unwrap();
    let begun = next_event(&service);
    assert!(begun.gesture_ack.as_ref().unwrap().accepted);

    for step in 1..=15_u64 {
        let value = 0.9 - (step as f32) * 0.02;
        service
            .submit(Request::device_gesture(GestureRequest::Change {
                key: key1,
                sequence: step,
                value: GestureValue::Float(value),
            }))
            .unwrap();
        let ack = next_event(&service);
        assert!(
            ack.error.is_none() && ack.gesture_ack.as_ref().unwrap().accepted,
            "preview step {step} failed: error={:?} ack={:?}",
            ack.error,
            ack.gesture_ack
        );
        assert!(
            sink_state.fenced.lock().unwrap().is_none(),
            "preview step {step} leaked stopped foreign history fence"
        );
    }
    assert!(
        sink_state.begun.load(Ordering::Acquire) >= 1,
        "saturated stopped preview must prune via stopped foreign history"
    );
    assert_eq!(
        sink_state.begun.load(Ordering::Acquire),
        sink_state.confirmed.load(Ordering::Acquire)
    );
    assert_eq!(
        sink_state.begun.load(Ordering::Acquire),
        sink_state.finished.load(Ordering::Acquire)
    );

    // 2) Commit (`Command::Gain`) at saturated history.
    let final_gain = 0.45_f32;
    service
        .submit(Request::device_gesture(GestureRequest::Commit {
            key: key1,
            sequence: 16,
            value: GestureValue::Float(final_gain),
            manual_edit: true,
        }))
        .unwrap();
    let committed = next_event(&service);
    assert!(
        committed.error.is_none() && committed.gesture_ack.as_ref().unwrap().accepted,
        "commit failed: error={:?} ack={:?}",
        committed.error,
        committed.gesture_ack
    );
    assert!((committed.state.project.track(group).unwrap().gain() - final_gain).abs() < 1e-6);
    assert!(sink_state.fenced.lock().unwrap().is_none());

    // 3) Saturate a second gesture to the exact 9-program boundary and Cancel
    // (exercising `restore_resident_preview_with_foreign_programs` pruning).
    let key2 = GestureKey {
        generation,
        token: 2,
    };
    service
        .submit(Request::device_gesture(GestureRequest::Begin {
            key: key2,
            target: GestureTarget::TrackGain { track: group },
        }))
        .unwrap();
    assert!(next_event(&service).gesture_ack.as_ref().unwrap().accepted);
    for step in 1..=8_u64 {
        service
            .submit(Request::device_gesture(GestureRequest::Change {
                key: key2,
                sequence: step,
                value: GestureValue::Float(0.4 + step as f32 * 0.01),
            }))
            .unwrap();
        let ack = next_event(&service);
        assert!(
            ack.error.is_none() && ack.gesture_ack.as_ref().unwrap().accepted,
            "second gesture step {step} failed: {:?}",
            ack.error
        );
    }
    service
        .submit(Request::device_gesture(GestureRequest::Cancel {
            key: key2,
        }))
        .unwrap();
    let cancelled = next_event(&service);
    assert!(
        cancelled.error.is_none() && cancelled.gesture_ack.as_ref().unwrap().accepted,
        "cancel at saturated history failed: error={:?} ack={:?}",
        cancelled.error,
        cancelled.gesture_ack
    );
    assert!(sink_state.fenced.lock().unwrap().is_none());

    // 4) Inject a confirmation refusal on a saturated preview step and verify
    // the fence is still released cleanly (no "already held" deadlock afterwards).
    let key3 = GestureKey {
        generation,
        token: 3,
    };
    service
        .submit(Request::device_gesture(GestureRequest::Begin {
            key: key3,
            target: GestureTarget::TrackGain { track: group },
        }))
        .unwrap();
    assert!(next_event(&service).gesture_ack.as_ref().unwrap().accepted);
    sink_state.refuse_confirm.store(true, Ordering::Release);
    let mut saw_refusal = false;
    for step in 1..=10_u64 {
        service
            .submit(Request::device_gesture(GestureRequest::Change {
                key: key3,
                sequence: step,
                value: GestureValue::Float(0.2 + step as f32 * 0.01),
            }))
            .unwrap();
        let ack = next_event(&service);
        assert!(
            sink_state.fenced.lock().unwrap().is_none(),
            "refused preview step {step} must release its stopped foreign history fence"
        );
        if !ack.gesture_ack.as_ref().unwrap().accepted {
            saw_refusal = true;
            break;
        }
    }
    assert!(
        saw_refusal,
        "expected injected confirmation refusal on pruning step"
    );
    // Cancel succeeds immediately after the refusal (no leaked fence).
    service
        .submit(Request::device_gesture(GestureRequest::Cancel {
            key: key3,
        }))
        .unwrap();
    let recovered = next_event(&service);
    assert!(
        recovered.error.is_none() && recovered.gesture_ack.as_ref().unwrap().accepted,
        "recovery cancel failed: error={:?} ack={:?}",
        recovered.error,
        recovered.gesture_ack
    );
    assert!(sink_state.fenced.lock().unwrap().is_none());

    // 5) No-op canonical command (`Command::Gain` with unchanged value -> shadowed
    // `editor_revision - 1` context) at saturated history must also release its fence.
    let begun_before_noop = sink_state.begun.load(Ordering::Acquire);
    for _ in 0..10 {
        service
            .submit(Request::Edit(Command::Gain {
                track: group,
                gain: final_gain,
            }))
            .unwrap();
        let ev = next_event(&service);
        assert!(
            ev.error.is_none(),
            "no-op gain command failed: {:?}",
            ev.error
        );
        assert!(
            sink_state.fenced.lock().unwrap().is_none(),
            "no-op gain command leaked stopped foreign history fence"
        );
    }
    assert!(
        sink_state.begun.load(Ordering::Acquire) > begun_before_noop,
        "no-op commands should have crossed a pruning boundary"
    );
    assert_eq!(
        sink_state.begun.load(Ordering::Acquire),
        sink_state.finished.load(Ordering::Acquire),
        "every acquired stopped foreign history fence must be released"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
