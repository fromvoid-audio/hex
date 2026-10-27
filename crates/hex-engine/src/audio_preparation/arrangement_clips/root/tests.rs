use super::*;
use crate::audio_preparation::{
    AudioArrangementInput, AudioPreparationCustody, AudioPreparationPhase,
};
use hex_project::{Command, Created, Editor};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn supported(name: &str) -> bool {
    crate::audio_preparation::session_source_tests::supported(name)
}
pub(super) fn fixture(streamed: bool) -> (hex_project::Project, OwnedAudioInputs) {
    let (project, mut bank, _) =
        crate::audio_preparation::arrangement_clip_tests::fixture(streamed, true, 2);
    bank.reservation.handle_obligations = 64;
    let mut editor = Editor::new(project, 8).unwrap();
    let mut returns = Vec::new();
    for _ in 0..2 {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(id);
    }
    let mut snapshot = editor.project().snapshot();
    snapshot.master_gain = 0.75;
    snapshot.loop_region = Some(hex_project::LoopRegion::new(1, 11, true).unwrap());
    snapshot.tracks[0].gain = 0.375;
    snapshot.tracks[0].balance = -0.25;
    snapshot.tracks[0].soloed = true;
    snapshot.tracks[0].gain_envelope = vec![hex_project::GainPoint {
        incoming_gain: None,
        beat: hex_project::BeatTime::ZERO,
        gain: 0.5,
        outgoing: hex_project::GainInterpolation::Step,
    }];
    snapshot.tracks[0]
        .sends
        .push(hex_project::SendLevelSnapshot {
            return_track: returns[1].value(),
            gain: 0.5,
        });
    snapshot = snapshot
        .with_return_feed(returns[1], returns[0], 0.25)
        .unwrap();
    (hex_project::Project::from_snapshot(snapshot).unwrap(), bank)
}
fn begin(
    c: &mut AudioPreparationCustody,
    p: &hex_project::Project,
    b: OwnedAudioInputs,
) -> Result<crate::AudioPreparationToken, Box<crate::AudioArrangementRefusal>> {
    c.begin_input_on_control(AudioArrangementInput::new(p, 11, b).with_audio_project_inventory())
}
pub(super) struct Pages;
impl crate::MediaPages for Pages {
    fn page(
        &mut self,
        _: crate::StreamSourceKey,
        page_index: u64,
    ) -> Option<crate::BorrowedMediaPage<'_>> {
        static PCM: [crate::Frame; 8] = [[0.25; 2]; 8];
        (page_index == 0).then_some(crate::BorrowedMediaPage {
            page_index,
            frames: &PCM,
        })
    }
    fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {
        panic!("all fixture pages are present")
    }
}
#[test]
fn actual_resident_and_streamed_root_matches_legacy_pcm_and_preserves_context_handles() {
    if !supported("arrangement_project_root_pcm") {
        return;
    }
    for streamed in [false, true] {
        let (project, bank) = fixture(streamed);
        let source = bank
            .source_token(project.assets().next().unwrap().id())
            .unwrap();
        let pointer = bank.sources.as_ptr();
        let capacity = bank.sources.capacity();
        let reservation = bank.reservation;
        let sources: BTreeMap<_, _> = bank
            .sources
            .iter()
            .map(|(id, source)| {
                (
                    *id,
                    match &source.kind {
                        SourceKind::Resident(audio) => {
                            crate::compile::PreparedAudioSource::Resident(audio.clone())
                        }
                        SourceKind::Streamed(metadata) => {
                            crate::compile::PreparedAudioSource::Streamed(Arc::clone(
                                metadata.arc(),
                            ))
                        }
                    },
                )
            })
            .collect();
        let expected =
            crate::compile::project_with_prepared_audio_sources(&project, &sources, 11).unwrap();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = begin(&mut custody, &project, bank).ok().unwrap();
        assert_eq!(
            custody.phase(),
            AudioPreparationPhase::ArrangementAndSessionSources
        );
        let root = custody.arrangement_clips.root.project.as_ref().unwrap();
        assert_eq!(root.sample_rate(), project.sample_rate());
        assert_eq!(root.loop_region(), project.loop_region());
        assert_eq!(root.master_gain.value(), project.master_gain());
        assert_eq!(root.contains_streamed_sources(), streamed);
        assert_eq!(root.audible_end_frame, expected.audible_end_frame);
        let session = custody.assembly.shared_storage().unwrap();
        assert!(Arc::ptr_eq(
            root.shared_contexts.arc(),
            session.contexts.unwrap().arc()
        ));
        assert_eq!(Arc::strong_count(root.shared_contexts.arc()), 2);
        let mut actual = crate::Renderer::new(root);
        actual.play();
        let mut oracle = crate::Renderer::new(&expected);
        oracle.play();
        for count in [3, 17, 1, 31] {
            let mut a = vec![[0.0; 2]; count];
            let mut b = a.clone();
            actual.render_with_pages(&mut a, &mut Pages).unwrap();
            oracle.render_with_pages(&mut b, &mut Pages).unwrap();
            assert_eq!(a, b);
        }
        let report = custody.arrangement_clips.capacity(false).unwrap();
        assert!(report.root.constructed);
        assert_eq!(report.root.installed_tracks, 1);
        assert_eq!(report.root.installed_returns, 2);
        assert!(report.root.track_container_bytes >= mem::size_of::<Track>());
        assert_eq!(report.tracks.independent_handle_obligations, 2);
        assert_eq!(report.returns.independent_handle_obligations, 4);
        assert_eq!(
            custody
                .capacity_report()
                .shared
                .installed_handle_obligations,
            3
        );
        assert_eq!(custody.capacity_report().shared.unresolved_allocations, 2);
        assert_eq!(
            custody
                .arrangement_return_metadata()
                .unwrap()
                .buses()
                .count(),
            2
        );
        assert_eq!(
            custody.arrangement_track_metadata(0).unwrap().id(),
            Some(project.tracks()[0].id())
        );
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        let recovered = custody.recover_on_control(&token).unwrap();
        assert_eq!(recovered.sources.as_ptr(), pointer);
        assert_eq!(recovered.sources.capacity(), capacity);
        assert_eq!(recovered.reservation, reservation);
        assert!(recovered.source_view(&source).is_ok());
    }
    eprintln!(
        "hex preparation evidence: arrangement_project_root_pcm; supported-controls=completed; resident-streamed-parity; context-handles2"
    );
}
#[test]
fn every_root_reserve_move_and_final_install_error_or_panic_retains_custody() {
    if !supported("arrangement_project_root_faults") {
        return;
    }
    // One Track, two Returns: two reserve frontiers, then Track/Return moves,
    // feed transfer, context transfer and actual Project installation.
    for frontier in 0..=7 {
        for panic in [false, true] {
            let (project, bank) = fixture(false);
            let pointer = bank.sources.as_ptr();
            let capacity = bank.sources.capacity();
            let old = bank
                .source_token(project.assets().next().unwrap().id())
                .unwrap();
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            if panic {
                custody.arrangement_clips.root.panic_after = Some(frontier);
            } else {
                custody.arrangement_clips.root.fail_after = Some(frontier);
            }
            let result = begin(&mut custody, &project, bank);
            if frontier < 2 {
                let refusal = result.err().unwrap();
                assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
                assert_eq!(refusal.inputs.sources.capacity(), capacity);
                assert!(refusal.inputs.source_view(&old).is_ok());
                if panic {
                    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
                    assert!(custody.first_panic.is_some());
                } else {
                    assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
                }
            } else {
                let token = result.ok().unwrap();
                assert_eq!(custody.bank.as_ref().unwrap().sources.as_ptr(), pointer);
                if panic {
                    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
                    let first =
                        custody.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any;
                    let before = custody.arrangement_clips.root.capacity(false).unwrap();
                    custody.cleanup_on_control();
                    assert!(custody.recover_on_control(&token).is_none());
                    assert_eq!(
                        custody.arrangement_clips.root.capacity(false).unwrap(),
                        before
                    );
                    assert!(std::ptr::eq(
                        first,
                        custody.first_panic.as_ref().unwrap().as_ref()
                    ));
                } else {
                    assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
                    custody.cleanup_on_control();
                    let recovered = custody.recover_on_control(&token).unwrap();
                    assert_eq!(recovered.sources.as_ptr(), pointer);
                    assert_eq!(recovered.sources.capacity(), capacity);
                    assert!(recovered.source_view(&old).is_ok());
                }
            }
        }
    }
    eprintln!(
        "hex preparation evidence: arrangement_project_root_faults; supported-controls=completed; reserve-transfer-install-frontiers8-errors-and-panics"
    );
}
#[test]
fn mismatched_root_association_refuses_reporting_and_restore_without_losing_owners() {
    if !supported("arrangement_project_root_association") {
        return;
    }
    let (project, bank) = fixture(false);
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut custody, &project, bank).ok().unwrap();
    custody
        .arrangement_clips
        .root
        .project
        .as_mut()
        .unwrap()
        .tracks[0]
        .track_id = None;
    assert_eq!(
        custody.arrangement_clips.capacity(false),
        Err(AudioStorageError::SourceMismatch)
    );
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(custody.arrangement_clips.root.project.is_some());
    assert!(custody.bank.is_some());
    assert!(custody.recover_on_control(&token).is_none());
}
#[test]
fn restored_root_child_destructor_panic_retains_first_payload_bank_and_remaining_owners() {
    if !supported("arrangement_project_root_disposal") {
        return;
    }
    let (project, bank) = fixture(false);
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let probe_count = Arc::clone(&count);
    custody.arrangement_clips.tracks.probe = Some(crate::audio_preparation::DropProbe(Some(
        Box::new(move || {
            probe_count.fetch_add(1, Ordering::SeqCst);
            panic!("root child destructor probe");
        }),
    )));
    let token = begin(&mut custody, &project, bank).ok().unwrap();
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(custody.arrangement_clips.root.project.is_none());
    assert!(custody.bank.is_some());
    assert!(
        custody
            .arrangement_clips
            .tracks
            .capacity(false)
            .unwrap()
            .independent_handle_obligations
            > 0
    );
    let first = custody.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any;
    custody.cleanup_on_control();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(std::ptr::eq(
        first,
        custody.first_panic.as_ref().unwrap().as_ref()
    ));
    assert!(custody.recover_on_control(&token).is_none());
}

#[test]
fn root_moves_add_no_aliases_and_equal_content_roots_retain_distinct_allocations() {
    if !supported("arrangement_project_root_aliases") {
        return;
    }
    let mut counts = Vec::new();
    let mut custodians = Vec::new();
    for rooted in [false, true, true] {
        let (project, bank) = fixture(true);
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        crate::owned_shared::reset_clone_count();
        let input = AudioArrangementInput::new(&project, 11, bank);
        let token = custody
            .begin_input_on_control(if rooted {
                input.with_audio_project_inventory()
            } else {
                input.with_audio_preview_inventory()
            })
            .ok()
            .unwrap();
        counts.push(crate::owned_shared::clone_count());
        custodians.push((custody, token));
    }
    assert_eq!(counts[0], counts[1]);
    assert_eq!(counts[1], counts[2]);
    let left = custodians[1]
        .0
        .arrangement_clips
        .root
        .project
        .as_ref()
        .unwrap();
    let right = custodians[2]
        .0
        .arrangement_clips
        .root
        .project
        .as_ref()
        .unwrap();
    assert!(!Arc::ptr_eq(
        left.shared_contexts.arc(),
        right.shared_contexts.arc()
    ));
    assert!(!Arc::ptr_eq(
        &left.tracks[0].instrument_meter,
        &right.tracks[0].instrument_meter
    ));
    for (custody, token) in &mut custodians {
        assert!(custody.cancel(token));
        custody.cleanup_on_control();
        assert!(custody.recover_on_control(token).is_some());
    }
}

#[test]
fn occupied_restore_target_and_caller_byte_pressure_keep_original_owners() {
    if !supported("arrangement_project_root_restore_pressure") {
        return;
    }
    let (project, bank) = fixture(false);
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut custody, &project, bank).ok().unwrap();
    let required = custody.capacity_report().retained_stage_bytes;
    // Actual occupied persistent destination, rather than a boolean stand-in.
    custody
        .arrangement_clips
        .root
        .tracks
        .push(Track::new(Vec::new(), 1.0, false).unwrap());
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(custody.arrangement_clips.root.project.is_some());
    assert!(custody.bank.is_some());
    assert_eq!(custody.arrangement_clips.root.tracks.len(), 1);
    let (project, mut bank) = fixture(false);
    bank.reservation.max_prepared_data_bytes = required - 1;
    let pointer = bank.sources.as_ptr();
    let capacity = bank.sources.capacity();
    let reservation = bank.reservation;
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let refusal = begin(&mut custody, &project, bank).err().unwrap();
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert_eq!(refusal.inputs.sources.capacity(), capacity);
    assert_eq!(refusal.inputs.reservation, reservation);
    assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
}

#[test]
fn empty_main_carrier_survives_construct_transfer_restore_and_reuse() {
    if !supported("main_carrier_identity") {
        return;
    }
    let (project, bank) = fixture(false);
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut custody, &project, bank).ok().unwrap();
    let root = &mut custody.arrangement_clips.root;
    let pointer = Arc::as_ptr(&root.project.as_ref().unwrap().main_device_chain);
    assert_eq!(
        root.capacity(false).unwrap().main_prepared_bytes,
        ArrangementRootAssembly::main_carrier_bytes().unwrap()
    );
    assert!(root.main_chain.is_none());
    let mut target = None;
    root.transfer_project(&mut target);
    assert!(root.can_restore_project());
    assert!(std::ptr::eq(
        Arc::as_ptr(&target.as_ref().unwrap().main_device_chain),
        pointer
    ));
    assert_eq!(
        root.capacity_at(false, target.as_ref())
            .unwrap()
            .main_prepared_bytes,
        ArrangementRootAssembly::main_carrier_bytes().unwrap()
    );
    root.restore_project(&mut target);
    assert!(target.is_none());
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert!(std::ptr::eq(
        Arc::as_ptr(custody.arrangement_clips.root.main_chain.as_ref().unwrap()),
        pointer
    ));
    let bank = custody.recover_on_control(&token).unwrap();
    let next = begin(&mut custody, &project, bank).ok().unwrap();
    assert!(std::ptr::eq(
        Arc::as_ptr(
            &custody
                .arrangement_clips
                .root
                .project
                .as_ref()
                .unwrap()
                .main_device_chain
        ),
        pointer
    ));
    assert!(custody.cancel(&next));
    custody.cleanup_on_control();
    assert!(custody.recover_on_control(&next).is_some());
}

#[test]
fn main_carrier_is_not_born_before_either_byte_reservation_admits_it() {
    if !supported("main_carrier_before_birth") {
        return;
    }
    let (project, bank) = fixture(false);
    let mut successful = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut successful, &project, bank).ok().unwrap();
    let report = successful.capacity_report();
    assert!(successful.cancel(&token));
    successful.cleanup_on_control();
    for prepared_limit in [true, false] {
        let (project, mut bank) = fixture(false);
        if prepared_limit {
            bank.reservation.max_prepared_data_bytes = report.retained_stage_bytes - 1;
        } else {
            bank.reservation.max_construction_data_bytes = report.construction_peak_bytes - 1;
        }
        let pointer = bank.sources.as_ptr();
        let mut refused = AudioPreparationCustody::try_new_on_control().unwrap();
        let failure = begin(&mut refused, &project, bank).err().unwrap();
        assert_eq!(failure.reason, AudioStorageError::ReservationExceeded);
        assert_eq!(failure.inputs.sources.as_ptr(), pointer);
        assert!(refused.arrangement_clips.root.main_chain.is_none());
        assert!(refused.arrangement_clips.root.project.is_none());
    }
}

#[test]
fn narrow_root_refuses_authored_main_before_taking_any_source() {
    if !supported("main_carrier_authored_refusal") {
        return;
    }
    use hex_lattice_contract::NativeDeviceMetadata;
    let (project, bank) = fixture(false);
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    let pointer = bank.sources.as_ptr();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let refused = begin(&mut custody, editor.project(), bank).err().unwrap();
    assert_eq!(refused.reason, AudioStorageError::UnsupportedGraph);
    assert_eq!(refused.inputs.sources.as_ptr(), pointer);
    assert!(custody.arrangement_clips.root.project.is_none());
    assert!(custody.arrangement_clips.root.main_chain.is_none());
}
