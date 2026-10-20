use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation};
use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, GainTouchContext,
    GainTouchSpan, GainTouchStep,
};

fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 32,
        handle_rows: 16,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
fn fixture(touch: bool) -> crate::Project {
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "gain".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track,
            points: vec![GainPoint {
                beat: BeatTime::ZERO,
                gain: 0.5,
                outgoing: GainInterpolation::Step,
            }],
        })
        .unwrap();
    if touch {
        editor
            .execute(Command::SpliceGainTouch {
                track,
                expected_epoch: 0,
                span: GainTouchSpan {
                    start: BeatTime::ZERO,
                    end: BeatTime::ONE,
                    context: GainTouchContext {
                        recording_epoch: 1,
                        project_generation: 0,
                        canonical_revision: 1,
                        audio_revision: 1,
                        transport_epoch: 1,
                        gesture_epoch: 1,
                        gesture_serial: 1,
                        sample_rate: 48_000,
                        loop_pass: 0,
                    },
                    steps: vec![GainTouchStep {
                        beat: BeatTime::ZERO,
                        gain: 0.75,
                        origin_beat: BeatTime::ZERO,
                        source_sequence: 1,
                    }],
                },
            })
            .unwrap();
    }
    // Actual SAME complete compiler gain output. Other legacy Project children
    // stay real in this fixture; their storage/teardown is not certified here.
    crate::compile::project(editor.project(), &std::collections::BTreeMap::new()).unwrap()
}

#[test]
fn fresh_gain_retirement_destroys_actual_compiled_curve_and_keeps_independent_copy_backing() {
    for touch in [false, true] {
        let mut project = fixture(touch);
        let source = &mut project.tracks[0].gain_envelope;
        source.as_mut().unwrap().knots.try_reserve_exact(9).unwrap();
        let mut custody = GainCopyCustody::default();
        custody.knots.try_reserve_exact(7).unwrap();
        let kept = custody.knots.as_ptr();
        let kept_bytes = custody.knots.capacity() * std::mem::size_of::<GainEnvelopeKnot>();
        let gain_bytes = source.as_ref().unwrap().checked_backing_bytes().unwrap();
        assert!(gain_bytes > 0);
        assert_eq!(
            source.as_ref().unwrap().value_at(0),
            if touch { 0.75 } else { 0.5 }
        );
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        custody.visit_audio_storage(&mut ledger).unwrap();
        source
            .as_ref()
            .unwrap()
            .visit_audio_storage(&mut ledger)
            .unwrap();
        assert_eq!(
            ledger.summary().unwrap().data_bytes,
            kept_bytes + gain_bytes
        );
        assert_eq!(
            ledger.summary().unwrap().data_allocations,
            if touch { 4 } else { 2 }
        );
        drop(ledger);
        custody.retire_prepared_on_control(source).unwrap();
        assert!(source.is_none());
        assert_eq!(custody.knots.as_ptr(), kept);
        assert_eq!(custody.bytes().unwrap(), kept_bytes);
        custody.clear_on_control().unwrap();
        custody.retire_prepared_on_control(source).unwrap();
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        custody.visit_audio_storage(&mut ledger).unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, kept_bytes);
        assert_eq!(ledger.summary().unwrap().data_allocations, 1);
        eprintln!(
            "SUPPORTED fresh gain retirement actual compiler touch={touch} exact capacity/control drop/retained spare copy backing"
        );
    }
}

#[test]
fn fresh_gain_retirement_preserves_copied_destination_and_refuses_unexpected_touch() {
    let project = hex_project::Project::new(48_000).unwrap();
    let points = [GainPoint {
        beat: BeatTime::ZERO,
        gain: 0.5,
        outgoing: GainInterpolation::Step,
    }];
    let mut custody = GainCopyCustody::default();
    custody
        .reserve(
            &points,
            project.tempo_map(),
            PreparedTrackMix::identity(),
            1.0,
        )
        .unwrap();
    custody.fill(&points, project.tempo_map()).unwrap();
    let pointer = custody.knots.as_ptr();
    let capacity = custody.knots.capacity();
    let mut original = None;
    assert!(custody.install(&mut original));
    let mut wrong = fixture(true);
    let wrong_source = &mut wrong.tracks[0].gain_envelope;
    let wrong_pointer = wrong_source.as_ref().unwrap().knots.as_ptr();
    assert!(matches!(
        custody.retire_prepared_on_control(wrong_source),
        Err(GainCopyError::Capacity)
    ));
    assert_eq!(wrong_source.as_ref().unwrap().knots.as_ptr(), wrong_pointer);
    assert_eq!(original.as_ref().unwrap().knots.as_ptr(), pointer);
    custody.retire_prepared_on_control(&mut original).unwrap();
    assert!(original.is_none());
    assert_eq!(custody.knots.as_ptr(), pointer);
    assert_eq!(custody.knots.capacity(), capacity);
    custody.clear_on_control().unwrap();
    assert_eq!(custody.knots.as_ptr(), pointer);
    eprintln!(
        "SUPPORTED fresh gain retirement actual copy restoration unchanged/wrong touch retained"
    );
}

#[test]
fn fresh_gain_retirement_abnormal_exit_latches_before_take_and_refuses_retry_or_inspection() {
    let mut project = fixture(true);
    let source = &mut project.tracks[0].gain_envelope;
    let mut custody = GainCopyCustody {
        panic_after_fresh_take: true,
        ..Default::default()
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        custody.retire_prepared_on_control(source)
    }));
    assert!(result.is_err());
    assert!(source.is_none());
    assert!(custody.phase == Phase::Retiring);
    assert!(matches!(custody.bytes(), Err(GainCopyError::Incomplete)));
    assert!(matches!(
        custody.clear_on_control(),
        Err(GainCopyError::Incomplete)
    ));
    assert!(matches!(
        custody.retire_prepared_on_control(source),
        Err(GainCopyError::Incomplete)
    ));
    assert!(matches!(
        custody.restore(source),
        Err(GainCopyError::Incomplete)
    ));
    assert!(!custody.can_install(source));
    let model = hex_project::Project::new(48_000).unwrap();
    assert!(
        custody
            .reserve(&[], model.tempo_map(), PreparedTrackMix::identity(), 1.0)
            .is_err()
    );
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    assert_eq!(
        custody.visit_audio_storage(&mut ledger),
        Err(crate::AudioStorageError::UnknownStorage)
    );
    assert_eq!(
        ledger.summary(),
        Err(crate::AudioStorageError::UnknownStorage)
    );
    // This injects a fault AFTER the concrete scalar Vec drop, not an opaque
    // destructor panic or a complete parent failure-charge survival proof.
    eprintln!("SUPPORTED fresh gain retirement post-take fault sticky/unknown/no retry or clear");
}
