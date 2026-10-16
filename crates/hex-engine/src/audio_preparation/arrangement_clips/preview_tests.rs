use super::*;
use crate::audio_preparation::{
    AudioArrangementInput, AudioPreparationCustody, AudioPreparationPhase,
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn supported(name: &str) -> bool {
    crate::audio_preparation::session_source_tests::supported(name)
}
fn begin(
    c: &mut AudioPreparationCustody,
    p: &hex_project::Project,
    b: OwnedAudioInputs,
) -> Result<crate::AudioPreparationToken, Box<crate::AudioArrangementRefusal>> {
    c.begin_input_on_control(AudioArrangementInput::new(p, 11, b).with_audio_preview_inventory())
}
#[test]
fn actual_empty_preview_conserves_constructor_allocation_handle_and_no_clone() {
    if !supported("audio_preview_owner") {
        return;
    }
    let (p, b) = crate::audio_preparation::tests::fixture(1);
    let old = b.source_token(p.assets().next().unwrap().id()).unwrap();
    let pointer = b.sources.as_ptr();
    let capacity = b.sources.capacity();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    crate::owned_shared::reset_clone_count();
    let t = begin(&mut c, &p, b).ok().unwrap();
    let clones = crate::owned_shared::clone_count();
    let observation = c.arrangement_clips.preview.observation().unwrap();
    assert_eq!(observation.1, 1);
    assert_ne!(observation.2, 0);
    assert!(observation.3);
    assert_eq!(c.audio_preview_metadata().unwrap().clip_count, 0);
    let report = c.capacity_report().arrangement_clips.preview;
    assert_eq!(report, c.arrangement_clips.preview.capacity().unwrap());
    assert_eq!(report.clips_payload_bytes, 0);
    assert!(report.clips_declared_overhead_bytes > 0);
    assert_eq!(report.independent_handle_obligations, 1);
    assert_eq!(report.reserved_handle_destinations, 1);
    assert_eq!(report.unresolved_allocations, 1); // layout is NOT retirement
    assert!(c.cancel(&t));
    // The actual value unpacks into vacant persistent slice custody, unchanged.
    c.arrangement_clips.preview.dispose_next().unwrap();
    let unpacked = c.arrangement_clips.preview.observation().unwrap();
    assert_eq!(
        (unpacked.0, unpacked.1, unpacked.2),
        (observation.0, observation.1, observation.2)
    );
    assert!(!unpacked.3);
    assert!(c.audio_preview_metadata().is_none());
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    assert_eq!(b.sources.as_ptr(), pointer);
    assert_eq!(b.sources.capacity(), capacity);
    assert!(b.source_view(&old).is_ok());
    assert!(c.arrangement_clips.preview.observation().is_none());
    assert_eq!(
        c.arrangement_clips.preview.capacity().unwrap(),
        crate::AudioPreviewCapacity::default()
    );
    crate::owned_shared::reset_clone_count();
    let t = c
        .begin_input_on_control(AudioArrangementInput::new(&p, 11, b).with_audio_return_inventory())
        .ok()
        .unwrap();
    assert_eq!(crate::owned_shared::clone_count(), clones);
    assert!(c.audio_preview_metadata().is_none()); // prior opt-in unchanged
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_some());
    eprintln!(
        "hex preparation evidence: audio_preview_owner; supported-controls=completed; exact-empty-handle1; staging-clones={clones}"
    );
}
#[test]
fn persistent_preview_reserve_and_installed_error_or_panic_retains_original_custody() {
    if !supported("audio_preview_failures") {
        return;
    }
    for phase in 0..=1 {
        for panic in [false, true] {
            let (p, b) = crate::audio_preparation::tests::fixture(1);
            let pointer = b.sources.as_ptr();
            let capacity = b.sources.capacity();
            let old = b.source_token(p.assets().next().unwrap().id()).unwrap();
            let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
            if panic {
                c.arrangement_clips.preview.panic_after = Some(phase);
            } else {
                c.arrangement_clips.preview.fail_after = Some(phase);
            }
            let result = begin(&mut c, &p, b);
            if phase == 0 {
                let refusal = result.err().unwrap();
                assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
                assert_eq!(refusal.inputs.sources.capacity(), capacity);
                assert!(refusal.inputs.source_view(&old).is_ok());
                if panic {
                    assert!(c.arrangement_clips.preview.observation().is_some());
                    assert!(c.first_panic.is_some());
                    assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
                    c.cleanup_on_control();
                    assert!(c.arrangement_clips.preview.observation().is_some());
                } else {
                    assert_eq!(c.phase(), AudioPreparationPhase::Vacant);
                    c.arrangement_clips.preview.fail_after = None;
                    let t = begin(&mut c, &p, refusal.inputs).ok().unwrap();
                    assert!(c.cancel(&t));
                    c.cleanup_on_control();
                    assert!(c.recover_on_control(&t).is_some());
                }
            } else {
                let t = result.ok().unwrap();
                assert!(c.arrangement_clips.preview.observation().unwrap().3);
                assert!(c.audio_preview_metadata().is_none());
                assert_eq!(c.bank.as_ref().unwrap().sources.as_ptr(), pointer);
                if panic {
                    assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
                    assert!(c.first_panic.is_some());
                    c.cleanup_on_control();
                    assert!(c.recover_on_control(&t).is_none());
                    assert!(c.arrangement_clips.preview.observation().is_some());
                } else {
                    assert_eq!(c.phase(), AudioPreparationPhase::Cleaning);
                    c.cleanup_on_control();
                    let b = c.recover_on_control(&t).unwrap();
                    assert_eq!(b.sources.as_ptr(), pointer);
                    assert_eq!(b.sources.capacity(), capacity);
                    assert!(b.source_view(&old).is_ok());
                }
            }
        }
    }
    eprintln!(
        "hex preparation evidence: audio_preview_failures; supported-controls=completed; reserve-installed-errors2-panics2"
    );
}
#[test]
fn actual_preview_destructor_probe_pins_remaining_graph_bank_and_first_payload() {
    if !supported("audio_preview_disposal_panic") {
        return;
    }
    let (p, b) = crate::audio_preparation::tests::fixture(1);
    let pointer = b.sources.as_ptr();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let t = begin(&mut c, &p, b).ok().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let observe = Arc::clone(&count);
    let thread = std::thread::current().id();
    c.arrangement_clips.preview.probe = Some(DropProbe(Some(Box::new(move || {
        assert_eq!(std::thread::current().id(), thread);
        observe.fetch_add(1, Ordering::Relaxed);
        panic!("actual empty preview disposal probe");
    }))));
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(c.first_panic.is_some());
    assert!(c.arrangement_clips.preview.pending());
    assert!(c.arrangement_clips.return_metadata().is_some());
    assert!(c.arrangement_clips.track_metadata(0).is_some());
    assert_eq!(c.bank.as_ref().unwrap().sources.as_ptr(), pointer);
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_none());
    assert_eq!(count.load(Ordering::Relaxed), 1);
    eprintln!(
        "hex preparation evidence: audio_preview_disposal_panic; supported-controls=completed; destructor-count1"
    );
}
#[test]
fn preview_handle_byte_pressure_and_occupied_custody_preserve_exact_inputs() {
    if !supported("audio_preview_reservations") {
        return;
    }
    let (p, b) = crate::audio_preparation::tests::fixture(1);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let t = begin(&mut c, &p, b).ok().unwrap();
    let report = c.capacity_report();
    assert_eq!(
        report.custodian_inline_bytes,
        mem::size_of::<AudioPreparationCustody>()
    );
    let (p2, b2) = crate::audio_preparation::tests::fixture(1);
    let pointer = b2.sources.as_ptr();
    let occupied = begin(&mut c, &p2, b2).err().unwrap();
    assert_eq!(occupied.reason, AudioStorageError::Capacity);
    assert_eq!(occupied.inputs.sources.as_ptr(), pointer);
    assert_eq!(c.audio_preview_metadata().unwrap().clip_count, 0);
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_some());
    for handles in [true, false] {
        let (p, mut b) = crate::audio_preparation::tests::fixture(1);
        let pointer = b.sources.as_ptr();
        let capacity = b.sources.capacity();
        let old = b.source_token(p.assets().next().unwrap().id()).unwrap();
        if handles {
            b.reservation.handle_obligations = 7;
        } else {
            b.reservation.max_prepared_data_bytes = report.retained_stage_bytes - 1;
        }
        let reservation = b.reservation;
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let refusal = begin(&mut c, &p, b).err().unwrap();
        assert_eq!(
            refusal.reason,
            if handles {
                AudioStorageError::Capacity
            } else {
                AudioStorageError::ReservationExceeded
            }
        );
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert_eq!(refusal.inputs.sources.capacity(), capacity);
        assert_eq!(refusal.inputs.reservation, reservation);
        assert!(refusal.inputs.source_view(&old).is_ok());
    }
    assert_eq!(
        crate::AudioPreviewCapacity {
            clips_payload_bytes: usize::MAX,
            clips_declared_overhead_bytes: 1,
            ..Default::default()
        }
        .bytes(),
        Err(AudioStorageError::Overflow)
    );
    eprintln!(
        "hex preparation evidence: audio_preview_reservations; supported-controls=completed; original-bank-and-reservation-preserved"
    );
}
