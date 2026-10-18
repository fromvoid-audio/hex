use super::*;
use crate::audio_preparation::{
    AudioArrangementInput, AudioPreparationCustody, AudioPreparationPhase,
};
use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, Tempo, TempoPoint,
};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};
fn fixture(
    streamed: bool,
    automation: bool,
    solo: bool,
    clips: usize,
) -> (
    hex_project::Project,
    OwnedAudioInputs,
    crate::AudioSourceToken,
) {
    let (p, b) = crate::audio_preparation::tests::fixture(clips);
    let asset = p.assets().next().unwrap().id();
    let first = p.tracks()[0].id();
    let mut e = Editor::new(p, 32).unwrap();
    let Created::Track(second) = e
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    e.execute(Command::RemoveInstrument { track: second })
        .unwrap();
    e.execute(Command::PlaceClip {
        track: second,
        asset,
        start: 0,
        duration: 8,
        source_offset: 0,
        source_length: 8,
        looping: false,
    })
    .unwrap();
    let Created::ReturnTrack(bus) = e
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let mut s = e.project().snapshot();
    s.tracks[0].gain = 0.6;
    s.tracks[0].balance = 0.25;
    s.tracks[0].soloed = solo;
    s.tracks[1].gain = 0.8;
    s.tracks[1].balance = -0.5;
    s.tracks[1].muted = !solo;
    s.tracks[0].sends = vec![hex_project::SendLevelSnapshot {
        return_track: bus.value(),
        gain: 0.3,
    }];
    if automation {
        s.tracks[0].gain_envelope = vec![
            GainPoint {
                beat: BeatTime::ZERO,
                gain: 0.5,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                beat: BeatTime::from_raw(1),
                gain: 0.75,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                beat: BeatTime::from_quarters(2).unwrap(),
                gain: 1.25,
                outgoing: GainInterpolation::Step,
            },
        ];
        s.tempo_points = vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(BeatTime::ONE, Tempo::new(90_000_000).unwrap()),
        ];
    }
    let p = hex_project::Project::from_snapshot(s).unwrap();
    assert_eq!(p.tracks()[0].id(), first);
    let b = if streamed {
        let reservation = b.reservation;
        drop(b);
        OwnedAudioInputs::try_new(
            vec![(
                asset,
                OwnedAudioSource::try_streamed(
                    crate::StreamedSourceMetadata::new(
                        crate::StreamSourceKey::new(11, asset),
                        48000,
                        48000,
                        8,
                        8,
                        hex_project::IDENTITY_CONVERSION,
                        [9; 32],
                    )
                    .unwrap(),
                )
                .ok()
                .unwrap(),
            )],
            reservation,
        )
        .ok()
        .unwrap()
    } else {
        b
    };
    let token = b.source_token(asset).unwrap();
    (p, b, token)
}
fn supported(name: &str) -> bool {
    crate::audio_preparation::session_source_tests::supported(name)
}
fn begin(
    c: &mut AudioPreparationCustody,
    p: &hex_project::Project,
    b: OwnedAudioInputs,
) -> Result<crate::AudioPreparationToken, Box<crate::AudioArrangementRefusal>> {
    c.begin_input_on_control(AudioArrangementInput::new(p, 11, b).with_audio_track_inventory())
}
fn legacy(p: &hex_project::Project, streamed: bool) -> crate::Project {
    let id = p.assets().next().unwrap().id();
    let source = if streamed {
        crate::compile::PreparedAudioSource::Streamed(Arc::new(
            crate::StreamedSourceMetadata::new(
                crate::StreamSourceKey::new(11, id),
                48000,
                48000,
                8,
                8,
                hex_project::IDENTITY_CONVERSION,
                [9; 32],
            )
            .unwrap(),
        ))
    } else {
        crate::compile::PreparedAudioSource::Resident(
            crate::Audio::new(48000, vec![[0.25; 2]; 8]).unwrap(),
        )
    };
    crate::compile::project_with_prepared_audio_sources(p, &BTreeMap::from([(id, source)]), 11)
        .unwrap()
}
struct Pages {
    key: crate::StreamSourceKey,
    frames: [crate::Frame; 8],
}
impl crate::MediaPages for Pages {
    fn page(
        &mut self,
        key: crate::StreamSourceKey,
        index: u64,
    ) -> Option<crate::BorrowedMediaPage<'_>> {
        if key != self.key || index != 0 {
            return None;
        }
        Some(crate::BorrowedMediaPage {
            page_index: 0,
            frames: &self.frames,
        })
    }
    fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {}
}
#[test]
fn actual_tracks_match_legacy_source_pcm_mix_solo_mute_sends_and_absolute_automation() {
    if !supported("arrangement_tracks_oracle") {
        return;
    }
    for streamed in [false, true] {
        for automation in [false, true] {
            for solo in [false, true] {
                let (p, b, token) = fixture(streamed, automation, solo, 2);
                let pointer = b.sources.as_ptr();
                let cap = b.sources.capacity();
                let reservation = b.reservation;
                let expected = legacy(&p, streamed);
                let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
                let t = begin(&mut c, &p, b).ok().unwrap();
                assert_eq!(
                    c.phase(),
                    AudioPreparationPhase::ArrangementAndSessionSources
                );
                assert_eq!(c.installed_source_bindings(), 3);
                let a = &c.arrangement_clips.tracks;
                assert!(a.rows[0].track.is_some());
                assert_eq!(a.rows[0].track.as_ref().unwrap().clips.len(), 2);
                assert!(!Arc::ptr_eq(
                    &a.rows[0].track.as_ref().unwrap().device_chain,
                    &a.rows[1].track.as_ref().unwrap().device_chain
                ));
                assert!(!Arc::ptr_eq(
                    &a.rows[0].track.as_ref().unwrap().instrument_meter,
                    &a.rows[1].track.as_ref().unwrap().instrument_meter
                ));
                for (index, (row, want)) in a.rows.iter().zip(expected.tracks()).enumerate() {
                    let actual = row.track.as_ref().unwrap();
                    assert_eq!(actual.track_id, want.track_id);
                    assert_eq!(actual.mix, want.mix);
                    assert_eq!(actual.source_mix, want.source_mix);
                    assert_eq!(Arc::strong_count(&actual.instrument_meter), 1);
                    assert_eq!(Arc::strong_count(&actual.device_chain), 1);
                    assert!(
                        row.pairs.iter().all(|x| x
                            .owner
                            .as_ref()
                            .is_some_and(|b| b.handle == Some(x.origin.handle)
                                && b.slot == Some(x.origin.slot)))
                    );
                    let view = c.arrangement_track_metadata(index).unwrap();
                    assert_eq!(view.id(), want.track_id);
                    assert_eq!(
                        view.sends().collect::<Vec<_>>(),
                        want.sends
                            .iter()
                            .map(|s| (s.return_index, s.gain))
                            .collect::<Vec<_>>()
                    );
                    let knots = |t: &Track| {
                        t.gain_envelope_knots()
                            .iter()
                            .map(|k| (k.frame(), k.gain(), k.outgoing()))
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(knots(actual), knots(want));
                    if automation && index == 0 {
                        assert_eq!(actual.prepared_gain_at(0), Some(0.75));
                        assert!(view.knots().iter().any(|k| k.frame() == 24000));
                    }
                    for frame in [0, 1, 4, 8, 15, 24000, 40000] {
                        assert_eq!(actual.mix_at(frame), want.mix_at(frame));
                        assert_eq!(view.gain_at(frame), want.prepared_gain_at(frame));
                        let mut x = [[0.; 2]; 16];
                        let mut y = x;
                        if streamed {
                            let key =
                                crate::StreamSourceKey::new(11, p.assets().next().unwrap().id());
                            let mut pages = Pages {
                                key,
                                frames: [[0.25; 2]; 8],
                            };
                            actual.render_audio_range_with_pages(
                                frame,
                                frame + 16,
                                &mut x,
                                &mut pages,
                            );
                            want.render_audio_range_with_pages(
                                frame,
                                frame + 16,
                                &mut y,
                                &mut pages,
                            );
                        } else {
                            actual.render_audio_range(frame, frame + 16, &mut x);
                            want.render_audio_range(frame, frame + 16, &mut y);
                        }
                        assert_eq!(x, y);
                        for (offset, (x, y)) in x.into_iter().zip(y).enumerate() {
                            assert_eq!(
                                actual.mix_at(frame + offset as u64).apply_frame(x),
                                want.mix_at(frame + offset as u64).apply_frame(y)
                            );
                        }
                    }
                }
                let report = c.capacity_report();
                let tracks = report.arrangement_clips.tracks;
                assert_eq!(tracks.independent_handle_obligations, 4);
                assert_eq!(tracks.reserved_handle_destinations, 4);
                assert_eq!(
                    tracks.meter_payload_bytes,
                    2 * mem::size_of::<hex_lattice::DeviceMeter>()
                );
                assert_eq!(tracks.chain_payload_bytes, 0);
                assert!(tracks.chain_declared_overhead_bytes > 0);
                assert_eq!(
                    report.arrangement_clips,
                    c.arrangement_clips.capacity(true).unwrap()
                );
                assert!(c.arrangement_clips.rows.iter().all(|r| r.transferred
                    && r.clip.is_none()
                    && r.installed_handle.is_none()
                    && r.declaration.is_none()));
                assert!(c.cancel(&t));
                c.cleanup_on_control();
                assert!(c.arrangement_track_metadata(0).is_none());
                let b = c.recover_on_control(&t).unwrap();
                assert_eq!(b.sources.as_ptr(), pointer);
                assert_eq!(b.sources.capacity(), cap);
                assert_eq!(b.reservation, reservation);
                assert!(b.source_view(&token).is_ok());
            }
        }
    }
    eprintln!(
        "hex preparation evidence: arrangement_tracks_oracle; supported-controls=completed; resident-streamed-automation-solo=8"
    );
}
#[test]
fn actual_partial_track_failure_and_panic_keep_source_pairs_and_recover_only_after_cleanup() {
    if !supported("arrangement_tracks_partial") {
        return;
    }
    for phase in 0..2 {
        let (p, b, token) = fixture(true, true, false, 2);
        let pointer = b.sources.as_ptr();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.arrangement_clips.tracks.fail_after = Some(phase);
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::Cleaning);
        assert_eq!(
            c.arrangement_clips
                .tracks
                .rows
                .iter()
                .filter(|r| r.track.is_some())
                .count(),
            phase + 1
        );
        assert!(c.arrangement_track_metadata(0).is_none());
        c.cleanup_on_control();
        let b = c.recover_on_control(&t).unwrap();
        assert_eq!(b.sources.as_ptr(), pointer);
        assert!(b.source_view(&token).is_ok());
        c.arrangement_clips.tracks.fail_after = None;
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert!(c.arrangement_track_metadata(0).is_some());
        assert!(c.cancel(&t));
        c.cleanup_on_control();
        assert!(c.recover_on_control(&t).is_some());
        let (p, b, _) = fixture(true, true, false, 2);
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.arrangement_clips.tracks.panic_after = Some(phase);
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(c.first_panic.is_some());
        let before = c.arrangement_clips.tracks.installed_clips(&[]);
        c.cleanup_on_control();
        assert_eq!(c.arrangement_clips.tracks.installed_clips(&[]), before);
        assert!(c.recover_on_control(&t).is_none());
    }
    eprintln!(
        "hex preparation evidence: arrangement_tracks_partial; supported-controls=completed; failures2-panics2"
    );
}
#[test]
fn borrowed_gain_fill_error_and_panic_keep_actual_knot_storage_and_original_inputs() {
    if !supported("arrangement_tracks_gain_failure") {
        return;
    }
    for panic in [false, true] {
        let (p, b, token) = fixture(false, true, false, 2);
        let pointer = b.sources.as_ptr();
        let capacity = b.sources.capacity();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.arrangement_clips.tracks.rows.push(Row::default());
        if panic {
            c.arrangement_clips.tracks.rows[0].gain.panic_after = Some(0);
        } else {
            c.arrangement_clips.tracks.rows[0].gain.fail_after = Some(0);
        }
        let failure = begin(&mut c, &p, b).err().unwrap();
        assert_eq!(failure.inputs.sources.as_ptr(), pointer);
        assert_eq!(failure.inputs.sources.capacity(), capacity);
        assert!(failure.inputs.source_view(&token).is_ok());
        if panic {
            assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
            assert!(c.first_panic.is_some());
            assert!(c.arrangement_clips.tracks.rows[0].gain.bytes().unwrap() > 0);
            c.cleanup_on_control();
            assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        } else {
            assert_eq!(c.phase(), AudioPreparationPhase::Vacant);
            c.arrangement_clips.tracks.rows[0].gain.fail_after = None;
            let t = begin(&mut c, &p, failure.inputs).ok().unwrap();
            assert!(c.cancel(&t));
            c.cleanup_on_control();
            assert!(c.recover_on_control(&t).is_some());
        }
    }
    eprintln!(
        "hex preparation evidence: arrangement_tracks_gain_failure; supported-controls=completed; borrowed-error-panic=2"
    );
}
#[test]
fn actual_meter_disposal_panic_keeps_chain_returned_pairs_bank_and_payload_no_retry() {
    if !supported("arrangement_tracks_disposal_panic") {
        return;
    }
    for chain in [false, true] {
        let (p, b, _) = fixture(true, true, false, 2);
        let pointer = b.sources.as_ptr();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let t = begin(&mut c, &p, b).ok().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let observe = Arc::clone(&count);
        let thread = std::thread::current().id();
        let probe = Some(DropProbe(Some(Box::new(move || {
            assert_eq!(std::thread::current().id(), thread);
            observe.fetch_add(1, Ordering::Relaxed);
            panic!("actual persistent meter disposal probe");
        }))));
        if chain {
            c.arrangement_clips.tracks.chain_probe = probe;
        } else {
            c.arrangement_clips.tracks.probe = probe;
        }
        assert!(c.cancel(&t));
        c.cleanup_on_control();
        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(c.first_panic.is_some());
        assert_eq!(c.bank.as_ref().unwrap().sources.as_ptr(), pointer);
        assert_eq!(
            c.arrangement_clips.tracks.rows[0].bare_chain.is_some(),
            !chain
        );
        assert!(c.arrangement_clips.tracks.rows[0].prepared_gain.is_some());
        assert!(c.arrangement_clips.rows.iter().any(|p| p.clip.is_some()));
        assert_eq!(
            if chain {
                c.arrangement_clips.tracks.rows[0].chain_disposal
            } else {
                c.arrangement_clips.tracks.rows[0].meter_disposal
            },
            Disposal::Pending
        );
        c.cleanup_on_control();
        assert!(c.recover_on_control(&t).is_none());
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
    eprintln!(
        "hex preparation evidence: arrangement_tracks_disposal_panic; supported-controls=completed; meter-chain=2; each-destructor-count=1"
    );
}
#[test]
fn actual_track_spares_growth_caller_limits_handle_pressure_and_clone_conservation() {
    if !supported("arrangement_tracks_capacity") {
        return;
    }
    let (p, b, token) = fixture(true, true, false, 2);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    crate::owned_shared::reset_clone_count();
    let t = begin(&mut c, &p, b).ok().unwrap();
    let clones = crate::owned_shared::clone_count();
    assert!(clones > 0);
    let first = c.capacity_report();
    assert!(first.arrangement_clips.tracks.bytes().unwrap() > 0);
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    assert!(b.source_view(&token).is_ok());
    let t = begin(&mut c, &p, b).ok().unwrap();
    assert_eq!(
        c.capacity_report()
            .arrangement_clips
            .previous_growth_backing_bytes,
        0
    );
    assert_eq!(
        c.capacity_report().arrangement_clips.tracks.bytes(),
        first.arrangement_clips.tracks.bytes()
    );
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    let old = c.begin_arrangement_on_control(&p, 11, b).ok().unwrap();
    let spare = c.capacity_report().arrangement_clips.tracks;
    assert!(spare.row_bytes > 0 && spare.clip_bytes > 0 && spare.gain_bytes > 0);
    assert_eq!(spare.independent_handle_obligations, 0);
    assert_eq!(spare.meter_payload_bytes, 0);
    assert!(c.cancel(&old));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&old).is_some());
    let (p, b, _) = fixture(true, true, false, 4);
    let t = begin(&mut c, &p, b).ok().unwrap();
    let report = c.capacity_report();
    assert!(
        report
            .arrangement_clips
            .tracks
            .previous_growth_backing_bytes
            > 0
    );
    assert_eq!(
        report.construction_peak_bytes,
        report.retained_stage_bytes
            + report.previous_slot_backing_bytes
            + report.session.previous_backing_bytes
            + report.structure.previous_growth_backing_bytes
            + report.tempo.previous_growth_backing_bytes
            + report.assembly.previous_growth_backing_bytes
            + report.arrangement_clips.previous_growth_backing_bytes
    );
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_some());
    for handles in [true, false] {
        let (p, mut b, token) = fixture(true, true, false, 2);
        let pointer = b.sources.as_ptr();
        let capacity = b.sources.capacity();
        if handles {
            b.reservation.handle_obligations = 10;
        } else {
            b.reservation.max_prepared_data_bytes = first.retained_stage_bytes - 1;
        }
        let reservation = b.reservation;
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let failure = begin(&mut c, &p, b).err().unwrap();
        assert_eq!(
            failure.reason,
            if handles {
                AudioStorageError::Capacity
            } else {
                AudioStorageError::ReservationExceeded
            }
        );
        assert_eq!(failure.inputs.sources.as_ptr(), pointer);
        assert_eq!(failure.inputs.sources.capacity(), capacity);
        assert_eq!(failure.inputs.reservation, reservation);
        assert!(failure.inputs.source_view(&token).is_ok());
    }
    let (p, b, _) = fixture(true, true, false, 2);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    crate::owned_shared::reset_clone_count();
    let t = c
        .begin_input_on_control(AudioArrangementInput::new(&p, 11, b).with_audio_clip_inventory())
        .ok()
        .unwrap();
    assert_eq!(crate::owned_shared::clone_count(), clones);
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_some());
    assert_eq!(
        ArrangementTrackCapacity {
            row_bytes: usize::MAX,
            clip_bytes: 1,
            ..Default::default()
        }
        .bytes(),
        Err(AudioStorageError::Overflow)
    );
    eprintln!(
        "hex preparation evidence: arrangement_tracks_capacity; supported-controls=completed; staging-clones={clones}"
    );
}

#[test]
fn mismatched_track_pair_preflight_keeps_all_actual_clips_in_original_persistent_destinations() {
    if !supported("arrangement_tracks_preflight") {
        return;
    }
    let (p, b, token) = fixture(true, true, false, 2);
    let pointer = b.sources.as_ptr();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    c.arrangement_clips.tracks.mismatch = true;
    let t = begin(&mut c, &p, b).ok().unwrap();
    assert_eq!(c.phase(), AudioPreparationPhase::Cleaning);
    assert_eq!(c.reason(), Some(AudioStorageError::SourceMismatch));
    assert!(
        c.arrangement_clips
            .tracks
            .rows
            .iter()
            .all(|r| r.track.is_none() && r.pairs.iter().all(|p| p.owner.is_none()))
    );
    assert!(c.arrangement_clips.rows.iter().all(|p| p.clip.is_some()
        && !p.transferred
        && p.installed_handle == Some(p.binding.handle)));
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    assert_eq!(b.sources.as_ptr(), pointer);
    assert!(b.source_view(&token).is_ok());
    eprintln!(
        "hex preparation evidence: arrangement_tracks_preflight; supported-controls=completed; originals=retained"
    );
}

#[test]
fn compiler_source_rows_correlate_actual_originals_and_refuse_wrong_or_repeated_sources() {
    use crate::compile::PreparedAudioSourceRef;
    use compiler_bindings::CompilerClipOutcome as Outcome;
    if !supported("compiler_source_rows_originals") {
        return;
    }
    for streamed in [false, true] {
        for fault in 0..4 {
            eprintln!("SUPPORTED compiler_source_rows streamed={streamed} fault={fault}");
            let (p, bank, _) = fixture(streamed, false, false, 2);
            let mut editor = Editor::new(p, 8).unwrap();
            let original_model = editor.project().snapshot();
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            let _token = custody
                .begin_input_on_control(
                    AudioArrangementInput::new(editor.project(), 11, bank)
                        .with_audio_return_inventory(),
                )
                .ok()
                .unwrap();
            let source_capacity = custody.bank.as_ref().unwrap().sources.capacity();
            let source_pointer = custody.bank.as_ref().unwrap().sources.as_ptr();
            let asset = editor.project().assets().next().unwrap().id();
            let wrong_audio = crate::Audio::new(48_000, vec![[0.25; 2]; 8]).unwrap();
            let wrong_stream = Arc::new(
                crate::StreamedSourceMetadata::new(
                    crate::StreamSourceKey::new(11, asset),
                    48_000,
                    48_000,
                    8,
                    8,
                    hex_project::IDENTITY_CONVERSION,
                    [9; 32],
                )
                .unwrap(),
            );
            let effects = hex_devices::native_registry().unwrap();
            let instruments = hex_devices::native_instrument_registry().unwrap();
            let transforms = crate::audio_transform::AudioTransformPreparation::default();
            let mut contexts = None;
            let mut destination = None;
            let mut calls = 0;
            let bank = custody.bank.as_ref().unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                transforms.compile_transaction(|| {
                    crate::compile::compile_project_into(
                        editor.project(),
                        |asset, _| {
                            calls += 1;
                            if calls == 3 {
                                if fault == 1 {
                                    return Err(crate::Invalid("injected source refusal"));
                                }
                                assert_ne!(fault, 3, "injected source resolver panic");
                                if fault == 2 {
                                    return Ok(if streamed {
                                        PreparedAudioSourceRef::Streamed(&wrong_stream)
                                    } else {
                                        PreparedAudioSourceRef::Resident(&wrong_audio)
                                    });
                                }
                            }
                            let source = bank
                                .sources
                                .iter()
                                .find(|(id, _)| *id == asset.id())
                                .unwrap();
                            Ok(match &source.1.kind {
                                SourceKind::Resident(audio) => {
                                    PreparedAudioSourceRef::Resident(audio)
                                }
                                SourceKind::Streamed(stream) => {
                                    PreparedAudioSourceRef::Streamed(stream.arc())
                                }
                                _ => panic!("no mono frame-copy route"),
                            })
                        },
                        None,
                        &effects,
                        &instruments,
                        &mut contexts,
                        &transforms,
                        &mut destination,
                        Some((&mut custody.arrangement_clips, bank)),
                    )
                })
            }));
            match fault {
                0 => assert!(result.unwrap().is_ok()),
                1 => assert!(matches!(
                    result.unwrap(),
                    Err(crate::Invalid("injected source refusal"))
                )),
                2 => assert!(matches!(
                    result.unwrap(),
                    Err(crate::Invalid(
                        "compiler resolved source differs from retained bank"
                    ))
                )),
                3 => assert!(result.is_err()),
                _ => unreachable!(),
            }
            assert_eq!(calls, 3);
            let retained = destination.as_ref().unwrap();
            assert_eq!(retained.tracks.len(), if fault == 0 { 2 } else { 1 });
            let first = &custody.arrangement_clips.tracks.rows[0];
            assert!(first.compiler_complete);
            for (index, pair) in first.pairs.iter().enumerate() {
                assert_eq!(
                    pair.compiled,
                    if streamed {
                        Outcome::StreamedOriginal(index)
                    } else {
                        Outcome::ResidentOriginal(index)
                    }
                );
            }
            let second = &custody.arrangement_clips.tracks.rows[1];
            assert_eq!(second.compiler_complete, fault == 0);
            assert_eq!(
                second.pairs[0].compiled,
                if fault == 0 {
                    if streamed {
                        Outcome::StreamedOriginal(0)
                    } else {
                        Outcome::ResidentOriginal(0)
                    }
                } else {
                    Outcome::Unseen
                }
            );
            assert_eq!(
                custody.bank.as_ref().unwrap().sources.as_ptr(),
                source_pointer
            );
            assert_eq!(
                custody.bank.as_ref().unwrap().sources.capacity(),
                source_capacity
            );
            let actual_clip = &retained.tracks[0].clips[0];
            match &custody.bank.as_ref().unwrap().sources[0].1.kind {
                SourceKind::Resident(original) => {
                    assert!(original.shares_pcm_with(actual_clip.audio.as_ref().unwrap()))
                }
                SourceKind::Streamed(original) => assert!(Arc::ptr_eq(
                    original.arc(),
                    actual_clip
                        .metadata
                        .as_ref()
                        .unwrap()
                        .streamed
                        .as_ref()
                        .unwrap()
                )),
                _ => unreachable!(),
            }
            // Correlation cannot be reused as an adoption permit or silently
            // reset after failure. Refusal occurs before resolver or input take.
            let mut another = None;
            let mut input = Some(crate::owned_shared::OwnedShared::new_owned(
                crate::shared_analytic::SharedAnalyticContextTable::default(),
            ));
            let input_pointer = Arc::as_ptr(input.as_ref().unwrap().arc());
            let refusal = crate::compile::compile_project_into(
                editor.project(),
                |_, _| panic!("repeated bindings must refuse before lookup"),
                None,
                &effects,
                &instruments,
                &mut input,
                &transforms,
                &mut another,
                Some((
                    &mut custody.arrangement_clips,
                    custody.bank.as_ref().unwrap(),
                )),
            );
            assert!(matches!(
                refusal,
                Err(crate::Invalid(
                    "compiler source bindings are stale or mismatched"
                ))
            ));
            assert!(another.is_none());
            assert_eq!(Arc::as_ptr(input.as_ref().unwrap().arc()), input_pointer);
            assert_eq!(editor.project().snapshot(), original_model);
            let track = editor.project().tracks()[0].id();
            editor.execute(Command::Gain { track, gain: 0.25 }).unwrap();
            assert!(editor.undo());
            assert_eq!(editor.project().snapshot(), original_model);
            assert!(editor.redo());
            assert!(editor.undo());
            assert_eq!(editor.project().snapshot(), original_model);
            drop(editor);
            drop(custody);
            // Already completed actual source aliases survive model/history and
            // original preparation retirement; partial projects are not played.
            if !streamed {
                assert_eq!(
                    destination.as_ref().unwrap().tracks[0].clips[0]
                        .audio
                        .as_ref()
                        .unwrap()
                        .frames(),
                    &[[0.25; 2]; 8]
                );
            } else {
                assert_eq!(
                    destination.as_ref().unwrap().tracks[0].clips[0]
                        .metadata
                        .as_ref()
                        .unwrap()
                        .streamed
                        .as_ref()
                        .unwrap()
                        .key(),
                    crate::StreamSourceKey::new(11, asset)
                );
            }
        }
    }
}

#[test]
fn compiler_source_rows_record_transformed_and_omitted_outputs_without_original_adoption() {
    use crate::compile::PreparedAudioSourceRef;
    use compiler_bindings::CompilerClipOutcome as Outcome;
    if !supported("compiler_source_rows_transform_omit") {
        return;
    }
    for streamed in [false, true] {
        eprintln!("SUPPORTED compiler_source_rows_transform_omit streamed={streamed}");
        let (p, bank, _) = fixture(streamed, false, false, 3);
        let mut editor = Editor::new(p, 16).unwrap();
        let track = editor.project().tracks()[0].id();
        let clips = editor.project().tracks()[0]
            .clips()
            .iter()
            .map(|c| c.id())
            .collect::<Vec<_>>();
        for (clip, start) in [(clips[1], 64), (clips[2], 128)] {
            editor
                .execute(Command::MoveClip { clip, track, start })
                .unwrap();
        }
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let _token = custody
            .begin_input_on_control(
                AudioArrangementInput::new(editor.project(), 11, bank)
                    .with_audio_return_inventory(),
            )
            .ok()
            .unwrap();
        let original = editor.project().snapshot();
        if streamed {
            // An actual direct streamed crop wholly beyond the nonlooping
            // source is omitted by the existing compiler. A following original
            // clip must keep its own ID while occupying the compressed index1.
            editor
                .execute(Command::TrimClip {
                    clip: clips[1],
                    start: 64,
                    duration: 32,
                    source_offset: 0,
                    source_length: 8,
                })
                .unwrap();
            // Pointer-edge trimming clamps to source extent; explicit TrimClip
            // above authors a valid longer view with a silent nonlooping tail.
            // Retain that real processing domain before cropping its view;
            // returning pitch to zero keeps this domain without processing it.
            editor
                .execute(Command::SetAudioClipPitch {
                    clip: clips[1],
                    cents: 137,
                })
                .unwrap();
            editor
                .execute(Command::SetAudioClipPitch {
                    clip: clips[1],
                    cents: 0,
                })
                .unwrap();
            editor
                .execute(Command::TrimClipEdge {
                    clip: clips[1],
                    edge: hex_project::TrimEdge::Left,
                    frame: 80,
                })
                .unwrap();
        } else {
            editor
                .execute(Command::SetAudioClipPitch {
                    clip: clips[1],
                    cents: 137,
                })
                .unwrap();
        }
        if streamed {
            let edit = editor
                .project()
                .clip(clips[1])
                .unwrap()
                .audio_edit()
                .unwrap();
            let (_, _, start, _) = edit.projected_domain(editor.project().tempo_map()).unwrap();
            assert!(!edit.domain.looping);
            assert_eq!(edit.domain.pitch_cents, 0);
            assert!(!edit.domain.warp_enabled);
            assert!(start >= edit.domain.source_length);
        }
        let effects = hex_devices::native_registry().unwrap();
        let instruments = hex_devices::native_instrument_registry().unwrap();
        let transforms = crate::audio_transform::AudioTransformPreparation::default();
        let mut contexts = None;
        let mut destination = None;
        let bank = custody.bank.as_ref().unwrap();
        transforms
            .compile_transaction(|| {
                crate::compile::compile_project_into(
                    editor.project(),
                    |asset, _| {
                        let source = bank
                            .sources
                            .iter()
                            .find(|(id, _)| *id == asset.id())
                            .unwrap();
                        Ok(match &source.1.kind {
                            SourceKind::Resident(audio) => PreparedAudioSourceRef::Resident(audio),
                            SourceKind::Streamed(stream) => {
                                PreparedAudioSourceRef::Streamed(stream.arc())
                            }
                            _ => panic!("no mono frame-copy route"),
                        })
                    },
                    None,
                    &effects,
                    &instruments,
                    &mut contexts,
                    &transforms,
                    &mut destination,
                    Some((&mut custody.arrangement_clips, bank)),
                )
            })
            .unwrap();
        let row = &custody.arrangement_clips.tracks.rows[0];
        assert!(row.compiler_complete);
        assert_eq!(
            row.pairs.iter().map(|p| p.origin.clip).collect::<Vec<_>>(),
            clips
        );
        if streamed {
            assert_eq!(
                row.pairs.iter().map(|p| p.compiled).collect::<Vec<_>>(),
                vec![
                    Outcome::StreamedOriginal(0),
                    Outcome::Omitted,
                    Outcome::StreamedOriginal(1)
                ]
            );
            assert_eq!(destination.as_ref().unwrap().tracks[0].clips.len(), 2);
            assert_eq!(destination.as_ref().unwrap().tracks[0].clips[1].start, 128);
        } else {
            assert_eq!(
                row.pairs.iter().map(|p| p.compiled).collect::<Vec<_>>(),
                vec![
                    Outcome::ResidentOriginal(0),
                    Outcome::Transformed(1),
                    Outcome::ResidentOriginal(2)
                ]
            );
            let output = &destination.as_ref().unwrap().tracks[0].clips[1];
            let SourceKind::Resident(original) = &custody.bank.as_ref().unwrap().sources[0].1.kind
            else {
                unreachable!()
            };
            assert!(!original.shares_pcm_with(output.audio.as_ref().unwrap()));
            assert!(output.audio.as_ref().unwrap().frames._derived.is_some());
            assert_eq!(destination.as_ref().unwrap().audio_clip_views.len(), 1);
        }
        // Actual edits/history can return to the same originals; these scalar
        // correlations never transplant their old declarations to new outputs.
        assert!(editor.undo());
        if streamed {
            for _ in 0..3 {
                assert!(editor.undo());
            }
        }
        assert_eq!(editor.project().snapshot(), original);
        assert!(row.pairs.iter().all(|p| p.owner.is_some()));
        assert!(row.track.is_some());
    }
}

#[test]
fn compiler_source_rows_latch_first_and_mid_track_failure_or_panic() {
    if !supported("compiler_source_rows_first_lookup") {
        return;
    }
    for (panic, fail_at) in [(false, 1), (true, 1), (false, 2), (true, 2)] {
        eprintln!("SUPPORTED compiler_source_rows_first_lookup panic={panic} fail_at={fail_at}");
        let (project, bank, _) = fixture(false, false, false, 2);
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let _token = custody
            .begin_input_on_control(
                AudioArrangementInput::new(&project, 11, bank).with_audio_return_inventory(),
            )
            .ok()
            .unwrap();
        let effects = hex_devices::native_registry().unwrap();
        let instruments = hex_devices::native_instrument_registry().unwrap();
        let transforms = crate::audio_transform::AudioTransformPreparation::default();
        let mut contexts = None;
        let mut destination = None;
        let mut calls = 0;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transforms.compile_transaction(|| {
                crate::compile::compile_project_into(
                    &project,
                    |_, _| {
                        calls += 1;
                        if calls == fail_at {
                            assert!(!panic, "injected selected lookup panic");
                            return Err(crate::Invalid("first lookup refusal"));
                        }
                        let SourceKind::Resident(audio) =
                            &custody.bank.as_ref().unwrap().sources[0].1.kind
                        else {
                            unreachable!()
                        };
                        Ok(crate::compile::PreparedAudioSourceRef::Resident(audio))
                    },
                    None,
                    &effects,
                    &instruments,
                    &mut contexts,
                    &transforms,
                    &mut destination,
                    Some((
                        &mut custody.arrangement_clips,
                        custody.bank.as_ref().unwrap(),
                    )),
                )
            })
        }));
        if panic {
            assert!(result.is_err());
        } else {
            assert!(matches!(
                result.unwrap(),
                Err(crate::Invalid("first lookup refusal"))
            ));
        }
        assert_eq!(calls, fail_at);
        assert!(destination.as_ref().unwrap().tracks.is_empty());
        assert!(custody.arrangement_clips.tracks.compiler_started);
        for (index, row) in custody.arrangement_clips.tracks.rows.iter().enumerate() {
            assert!(!row.compiler_complete);
            assert_eq!(
                row.compiler_cursor,
                if index == 0 { fail_at - 1 } else { 0 }
            );
            for (clip, pair) in row.pairs.iter().enumerate() {
                assert_eq!(
                    pair.compiled,
                    if index == 0 && clip < fail_at - 1 {
                        compiler_bindings::CompilerClipOutcome::ResidentOriginal(clip)
                    } else {
                        compiler_bindings::CompilerClipOutcome::Unseen
                    }
                );
            }
        }
        let mut second = None;
        let mut contexts = Some(crate::owned_shared::OwnedShared::new_owned(
            crate::shared_analytic::SharedAnalyticContextTable::default(),
        ));
        let pointer = Arc::as_ptr(contexts.as_ref().unwrap().arc());
        assert!(matches!(
            crate::compile::compile_project_into(
                &project,
                |_, _| panic!("attempt must refuse before repeated lookup"),
                None,
                &effects,
                &instruments,
                &mut contexts,
                &transforms,
                &mut second,
                Some((
                    &mut custody.arrangement_clips,
                    custody.bank.as_ref().unwrap()
                )),
            ),
            Err(crate::Invalid(
                "compiler source bindings are stale or mismatched"
            ))
        ));
        assert!(second.is_none());
        assert_eq!(Arc::as_ptr(contexts.as_ref().unwrap().arc()), pointer);
        let SourceKind::Resident(original) = &custody.bank.as_ref().unwrap().sources[0].1.kind
        else {
            unreachable!()
        };
        assert_eq!(original.frames(), &[[0.25; 2]; 8]);
        assert!(!custody.arrangement_clips.can_transfer_revision());
    }
}

#[test]
fn compiler_source_rows_refuse_equal_independent_bank_before_attempt() {
    if !supported("compiler_source_rows_independent_bank") {
        return;
    }
    for streamed in [false, true] {
        eprintln!("SUPPORTED compiler_source_rows_independent_bank streamed={streamed}");
        let (project, original, _) = fixture(streamed, false, false, 2);
        let (equal_project, independent, _) = fixture(streamed, false, false, 2);
        assert_eq!(project.snapshot(), equal_project.snapshot());
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let _token = custody
            .begin_input_on_control(
                AudioArrangementInput::new(&project, 11, original).with_audio_return_inventory(),
            )
            .ok()
            .unwrap();
        let source_pointer = custody.bank.as_ref().unwrap().sources.as_ptr();
        let effects = hex_devices::native_registry().unwrap();
        let instruments = hex_devices::native_instrument_registry().unwrap();
        let transforms = crate::audio_transform::AudioTransformPreparation::default();
        let mut contexts = Some(crate::owned_shared::OwnedShared::new_owned(
            crate::shared_analytic::SharedAnalyticContextTable::default(),
        ));
        let context_pointer = Arc::as_ptr(contexts.as_ref().unwrap().arc());
        let mut destination = None;
        assert!(matches!(
            crate::compile::compile_project_into(
                &project,
                |_, _| panic!("independent bank must refuse before lookup"),
                None,
                &effects,
                &instruments,
                &mut contexts,
                &transforms,
                &mut destination,
                Some((&mut custody.arrangement_clips, &independent)),
            ),
            Err(crate::Invalid(
                "compiler source bindings are stale or mismatched"
            ))
        ));
        assert!(destination.is_none());
        assert_eq!(
            Arc::as_ptr(contexts.as_ref().unwrap().arc()),
            context_pointer
        );
        assert!(!custody.arrangement_clips.tracks.compiler_started);
        assert_eq!(
            custody.bank.as_ref().unwrap().sources.as_ptr(),
            source_pointer
        );
        custody
            .arrangement_clips
            .tracks
            .validate_root_sources(&[], custody.bank.as_ref().unwrap())
            .unwrap();
        assert!(custody.arrangement_clips.tracks.rows.iter().all(|row| {
            row.compiler_cursor == 0
                && !row.compiler_complete
                && row
                    .pairs
                    .iter()
                    .all(|pair| pair.compiled == compiler_bindings::CompilerClipOutcome::Unseen)
        }));
    }
}

fn compiler_track_child_fixture(
    filtered: bool,
    bypassed: bool,
    automation: bool,
) -> hex_project::Project {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
    for name in ["first", "second"] {
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track");
        };
        editor.execute(Command::RemoveInstrument { track }).unwrap();
    }
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("return");
    };
    let mut snapshot = editor.project().snapshot();
    let definition = &hex_devices::utility::UTILITY_DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::utility::parameter::DC_REMOVE,
            ParameterValue::Bool(filtered),
        )
        .unwrap()
        .with_input_gain_millidb(-6000)
        .unwrap()
        .with_output_gain_millidb(3000)
        .unwrap();
    snapshot.tracks[0].gain = 0.75;
    snapshot.tracks[0].balance = -0.3;
    snapshot.tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            id: snapshot.next_id,
            bypassed,
            state,
        });
    snapshot.next_id += 1;
    snapshot.tracks[0]
        .sends
        .push(hex_project::SendLevelSnapshot {
            return_track: bus.value(),
            gain: 0.25,
        });
    if automation {
        snapshot.tracks[0].gain_envelope = vec![
            GainPoint {
                beat: BeatTime::ZERO,
                gain: 0.5,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                beat: BeatTime::ONE,
                gain: 1.2,
                outgoing: GainInterpolation::Step,
            },
        ];
    }
    hex_project::Project::from_snapshot(snapshot).unwrap()
}

#[test]
fn selected_track_children_use_original_births_and_same_mix_gain_sends_and_retirement() {
    if !supported("selected_track_children_common_compiler") {
        return;
    }
    for filtered in [false, true] {
        for bypassed in [false, true] {
            for automation in [false, true] {
                eprintln!(
                    "SUPPORTED selected_track_children filtered={filtered} bypassed={bypassed} automation={automation}"
                );
                let project = compiler_track_child_fixture(filtered, bypassed, automation);
                let snapshot = project.snapshot();
                let reference = crate::compile::project(&project, &BTreeMap::new()).unwrap();
                let mut rows = ArrangementClips::default();
                let effects = hex_devices::native_registry().unwrap();
                let instruments = hex_devices::native_instrument_registry().unwrap();
                let transforms = crate::audio_transform::AudioTransformPreparation::default();
                let mut contexts = None;
                let mut destination = None;
                transforms
                    .compile_transaction(|| {
                        crate::compile::compile_project_into_with_children(
                            &project,
                            |_, _| panic!("no authored audio source"),
                            None,
                            &effects,
                            &instruments,
                            &mut contexts,
                            &transforms,
                            &mut destination,
                            None,
                            None,
                            Some(&mut rows),
                        )
                    })
                    .unwrap();
                let actual = destination.as_mut().unwrap();
                assert_eq!(actual.tracks.len(), 2);
                for (index, (track, expected)) in
                    actual.tracks.iter().zip(&reference.tracks).enumerate()
                {
                    let row = &rows.tracks.rows[index];
                    assert_eq!(row.root_index, Some(index));
                    assert!(row.meter.is_none() && row.chain.is_none() && row.track.is_none());
                    assert_eq!(Arc::strong_count(&track.instrument_meter), 1);
                    assert_eq!(Arc::strong_count(&track.device_chain), 1);
                    assert!(
                        row.meter_declaration
                            .as_ref()
                            .and_then(|d| d.parts(&track.instrument_meter))
                            .is_some()
                    );
                    assert_eq!(track.mix, expected.mix);
                    assert_eq!(track.source_mix, expected.source_mix);
                    assert_eq!(track.sends.len(), expected.sends.len());
                    for (send, other) in track.sends.iter().zip(&expected.sends) {
                        assert_eq!(
                            (send.return_index, send.gain),
                            (other.return_index, other.gain)
                        );
                    }
                    let knots = |track: &Track| {
                        track
                            .gain_envelope_knots()
                            .iter()
                            .map(|k| (k.frame(), k.gain(), k.outgoing()))
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(knots(track), knots(expected));
                    if index == 0 {
                        assert_eq!(track.device_chain.len(), 1);
                        assert_eq!(track.device_chain[0].definition.is_some(), filtered);
                        assert_eq!(track.device_chain[0].bypassed, bypassed);
                    } else {
                        assert!(track.device_chain.is_empty());
                    }
                }
                let pointers = actual
                    .tracks
                    .iter()
                    .map(|t| {
                        (
                            Arc::as_ptr(&t.instrument_meter),
                            Arc::as_ptr(&t.device_chain),
                        )
                    })
                    .collect::<Vec<_>>();
                rows.tracks.capacity_at(false, &actual.tracks).unwrap();
                assert_eq!(
                    rows.tracks.dispose_next(&mut rows.rows),
                    Err(AudioStorageError::UnknownStorage)
                );
                rows.tracks.preflight_root_restore(&actual.tracks).unwrap();
                rows.tracks.restore_root_rows(&mut actual.tracks);
                for (row, (meter, chain)) in rows.tracks.rows.iter().zip(pointers) {
                    let track = row.track.as_ref().unwrap();
                    assert_eq!(Arc::as_ptr(&track.instrument_meter), meter);
                    assert_eq!(Arc::as_ptr(&track.device_chain), chain);
                }
                while rows.tracks.has_next() {
                    rows.tracks.dispose_next(&mut rows.rows).unwrap();
                }
                rows.tracks.clear_disposed().unwrap();
                assert_eq!(project.snapshot(), snapshot);
            }
        }
    }
}

#[test]
fn selected_track_children_partial_failure_keeps_actual_rows_and_refuses_unsupported_ingress() {
    if !supported("selected_track_children_failure_inventory") {
        return;
    }
    let collect = |rows: &TrackAssembly, roots: &Vec<Track>, budget| {
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(budget)?;
        ledger.root(rows, crate::AudioStorageRole::Current)?;
        ledger.vec(roots)?;
        ledger.mutate(|ledger| rows.visit_audio_storage(ledger, roots))?;
        ledger.validate_selected_shared_handles_on_control()?;
        let report = ledger.summary()?;
        report.require_known_storage().unwrap();
        assert_eq!(
            report.require_complete(),
            Err(crate::AudioStorageIncomplete::RetirementReceiptUnknown)
        );
        Ok::<_, AudioStorageError>(report)
    };
    let budget = crate::AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 512,
        root_rows: 32,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    for panic in [false, true] {
        eprintln!("SUPPORTED selected_track_children_failure_inventory panic={panic}");
        let project = compiler_track_child_fixture(true, false, true);
        let mut rows = ArrangementClips::default();
        rows.tracks.fail_after = (!panic).then_some(0);
        rows.tracks.panic_after = panic.then_some(0);
        let effects = hex_devices::native_registry().unwrap();
        let instruments = hex_devices::native_instrument_registry().unwrap();
        let transforms = crate::audio_transform::AudioTransformPreparation::default();
        let mut contexts = None;
        let mut destination = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transforms.compile_transaction(|| {
                crate::compile::compile_project_into_with_children(
                    &project,
                    |_, _| panic!("no audio sources"),
                    None,
                    &effects,
                    &instruments,
                    &mut contexts,
                    &transforms,
                    &mut destination,
                    None,
                    None,
                    Some(&mut rows),
                )
            })
        }));
        if panic {
            assert!(result.is_err());
        } else {
            assert!(matches!(
                result.unwrap(),
                Err(crate::Invalid(
                    "compiler paired Track destination is unavailable"
                ))
            ));
        }
        let actual = destination.as_mut().unwrap();
        assert_eq!(actual.tracks.len(), 1);
        assert_eq!(rows.tracks.rows[0].root_index, Some(0));
        assert!(rows.tracks.rows[1].meter.is_some() && rows.tracks.rows[1].chain.is_some());
        let meter = Arc::as_ptr(&actual.tracks[0].instrument_meter);
        let chain = Arc::as_ptr(&actual.tracks[0].device_chain);
        let state = Arc::as_ptr(
            actual.tracks[0].device_chain[0]
                .configuration
                .as_ref()
                .unwrap(),
        );
        let report = collect(&rows.tracks, &actual.tracks, budget).unwrap();
        let mut exact = budget;
        exact.max_retained_bytes = report.retained_bytes;
        exact.max_construction_peak_bytes = report.construction_peak_bytes;
        assert_eq!(
            collect(&rows.tracks, &actual.tracks, exact).unwrap(),
            report
        );
        exact.max_retained_bytes -= 1;
        assert_eq!(
            collect(&rows.tracks, &actual.tracks, exact),
            Err(AudioStorageError::ReservationExceeded)
        );
        // A genuine extra physical meter field must join closure independently.
        let exported = Arc::clone(&actual.tracks[0].instrument_meter);
        assert!(collect(&rows.tracks, &actual.tracks, budget).is_err());
        drop(exported);
        rows.tracks.preflight_root_restore(&actual.tracks).unwrap();
        rows.tracks.restore_root_rows(&mut actual.tracks);
        let first = rows.tracks.rows[0].track.as_ref().unwrap();
        assert_eq!(Arc::as_ptr(&first.instrument_meter), meter);
        assert_eq!(Arc::as_ptr(&first.device_chain), chain);
        assert_eq!(
            Arc::as_ptr(first.device_chain[0].configuration.as_ref().unwrap()),
            state
        );
        while rows.tracks.has_next() {
            rows.tracks.dispose_next(&mut rows.rows).unwrap();
            collect(&rows.tracks, &actual.tracks, budget).unwrap();
        }
        rows.tracks.clear_disposed().unwrap();
    }
    // Existing original-source/loaded instruments require their actual ingress,
    // not adoption by this first meter/chain-only selection.
    let (project, bank, _) = fixture(false, false, false, 1);
    let pointer = bank.sources.as_ptr();
    let mut rows = ArrangementClips::default();
    let effects = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let transforms = crate::audio_transform::AudioTransformPreparation::default();
    let mut contexts = Some(crate::owned_shared::OwnedShared::new_owned(
        crate::shared_analytic::SharedAnalyticContextTable::default(),
    ));
    let context = Arc::as_ptr(contexts.as_ref().unwrap().arc());
    let mut destination = None;
    assert!(matches!(
        crate::compile::compile_project_into_with_children(
            &project,
            |_, _| panic!("unsupported selection before source lookup"),
            None,
            &effects,
            &instruments,
            &mut contexts,
            &transforms,
            &mut destination,
            None,
            None,
            Some(&mut rows),
        ),
        Err(crate::Invalid(
            "compiler paired Track children are unavailable"
        ))
    ));
    assert!(destination.is_none());
    assert_eq!(Arc::as_ptr(contexts.as_ref().unwrap().arc()), context);
    assert!(rows.tracks.phase == Phase::Vacant);
    assert!(rows.tracks.rows.is_empty());
    assert_eq!(bank.sources.as_ptr(), pointer);
}

#[test]
fn selected_track_children_loaded_sampler_uses_actual_context_and_survives_restore_and_retirement()
{
    use crate::shared_analytic::SharedAnalyticContextTable;
    if !supported("selected_track_children_loaded_sampler") {
        return;
    }
    for routed in [false, true] {
        eprintln!("SUPPORTED selected_track_children_loaded_sampler routed={routed}");
        let (bank, identity, mut editor) =
            crate::shared_analytic::sampler_tests::canonical_fixture().unwrap();
        let source = editor.project().tracks()[0].id();
        drop(identity);
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: source,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        if routed {
            let state = editor
                .project()
                .track(source)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .clone();
            let Created::Track(destination) = editor
                .execute(Command::AddMidiTrack {
                    name: "destination".into(),
                })
                .unwrap()
            else {
                panic!("track");
            };
            let device = editor
                .project()
                .track(destination)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            editor
                .execute(Command::SetMidiSource {
                    destination,
                    source: Some(source),
                })
                .unwrap();
        }
        let instruments = hex_devices::native_instrument_registry().unwrap();
        let effects = hex_devices::native_registry().unwrap();
        let mut contexts = Some(
            SharedAnalyticContextTable::prepare(
                editor.project(),
                &instruments,
                None,
                Some(crate::native_device_resources::DeviceResourcePreparation {
                    bank: &bank,
                    max_scope_metadata_bytes: 65_536,
                }),
            )
            .unwrap(),
        );
        let table = Arc::as_ptr(contexts.as_ref().unwrap().arc());
        let mut rows = ArrangementClips::default();
        let transforms = crate::audio_transform::AudioTransformPreparation::default();
        let mut destination = None;
        transforms
            .compile_transaction(|| {
                crate::compile::compile_project_into_with_children(
                    editor.project(),
                    |_, _| panic!("device-only original is not an Arrangement audio clip"),
                    None,
                    &effects,
                    &instruments,
                    &mut contexts,
                    &transforms,
                    &mut destination,
                    None,
                    None,
                    Some(&mut rows),
                )
            })
            .unwrap();
        assert!(contexts.is_none());
        let actual = destination.as_mut().unwrap();
        assert_eq!(Arc::as_ptr(actual.shared_contexts.arc()), table);
        assert_eq!(actual.tracks.len(), if routed { 2 } else { 1 });
        for (index, track) in actual.tracks.iter().enumerate() {
            assert!(track.mutable_instrument);
            assert_eq!(track.midi_source_index, (index == 1).then_some(0));
            let instrument = track.instrument.as_ref().unwrap();
            assert_eq!(Arc::as_ptr(instrument.shared_contexts.arc()), table);
            assert_eq!(instrument.spans.len(), 1);
            assert_eq!(
                (instrument.spans[0].pitch, instrument.spans[0].velocity),
                (60, 127)
            );
            assert_eq!(Arc::strong_count(&track.instrument_meter), 1);
            assert_eq!(Arc::strong_count(&track.device_chain), 1);
        }
        let collect = |rows: &TrackAssembly, actual: &crate::Project| {
            let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
                crate::AudioStorageLedgerReservation {
                    data_rows: 4096,
                    handle_rows: 4096,
                    root_rows: 512,
                    max_retained_bytes: 16 * 1024 * 1024,
                    max_construction_peak_bytes: 16 * 1024 * 1024,
                },
            )
            .unwrap();
            ledger
                .borrow_device_resource_bank_on_control(&bank, crate::AudioStorageRole::Caller)
                .unwrap();
            ledger
                .borrow_editor_native_states_on_control(&editor)
                .unwrap();
            crate::shared_analytic::resource_storage::visit_table(
                &actual.shared_contexts,
                &mut ledger,
                crate::AudioStorageRole::Caller,
            )
            .unwrap();
            ledger.root(rows, crate::AudioStorageRole::Current).unwrap();
            ledger.vec(&actual.tracks).unwrap();
            ledger
                .mutate(|ledger| rows.visit_audio_storage(ledger, &actual.tracks))
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            let report = ledger.summary().unwrap();
            report.require_known_storage().unwrap();
            assert!(report.require_complete().is_err());
            report
        };
        collect(&rows.tracks, actual);
        rows.tracks.preflight_root_restore(&actual.tracks).unwrap();
        rows.tracks.restore_root_rows(&mut actual.tracks);
        collect(&rows.tracks, actual);
        // The actual retained factory and canonical compiled note drive PCM;
        // this is not a renderer publication or callback allocation test.
        drop(editor);
        drop(bank);
        for row in &rows.tracks.rows {
            let track = row.track.as_ref().unwrap();
            let instrument = track.instrument.as_ref().unwrap();
            let owner = instrument
                .shared_contexts
                .owner_for_device(track.instrument_id.unwrap())
                .unwrap();
            let state = track.instrument_state.as_ref().unwrap();
            let factory = owner
                .sampler_prepared_on_control(
                    track.track_id.unwrap(),
                    track.instrument_id.unwrap(),
                    state,
                    44_100,
                )
                .unwrap()
                .unwrap();
            let mut pair = hex_lattice::NativeInstrumentRuntimePair::new(
                factory,
                hex_lattice::NativeInstrumentRuntimeLimits {
                    max_pair_bytes: 2 * 1024 * 1024,
                    max_events_per_block: 259,
                    max_active_occurrences: 32,
                    max_tail_frames: 44_100 * 600,
                },
            )
            .unwrap();
            let note = &instrument.spans[0];
            let mut pcm = [[0.0; 2]; 6];
            pair.begin_block();
            pair.process_block(
                &[hex_lattice::NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: note.pitch,
                    velocity: note.velocity,
                }],
                &mut pcm,
                owner.resources(),
            )
            .unwrap();
            pair.commit_block();
            assert_eq!(&pcm[..4], &crate::native_device_resources::tests::PCM);
            assert_eq!(&pcm[4..], &[[0.0; 2]; 2]);
        }
        while rows.tracks.has_next() {
            rows.tracks.dispose_next(&mut rows.rows).unwrap();
        }
        rows.tracks.clear_disposed().unwrap();
    }
}

#[test]
fn selected_track_children_missing_or_stale_sampler_context_refuses_before_child_births() {
    if !supported("selected_track_children_sampler_preflight") {
        return;
    }
    for stale in [false, true] {
        eprintln!("SUPPORTED selected_track_children_sampler_preflight stale={stale}");
        let (bank, identity, mut editor) =
            crate::shared_analytic::sampler_tests::canonical_fixture().unwrap();
        let device = editor.project().tracks()[0].instrument().unwrap().id();
        drop(identity);
        let instruments = hex_devices::native_instrument_registry().unwrap();
        let effects = hex_devices::native_registry().unwrap();
        let mut contexts = stale.then(|| {
            crate::shared_analytic::SharedAnalyticContextTable::prepare(
                editor.project(),
                &instruments,
                None,
                Some(crate::native_device_resources::DeviceResourcePreparation {
                    bank: &bank,
                    max_scope_metadata_bytes: 65_536,
                }),
            )
            .unwrap()
        });
        if stale {
            let old = editor.project().tracks()[0].instrument().unwrap().state();
            let mut state = hex_devices::sampler::state::SamplerState::from_native(old).unwrap();
            state.transpose_semitones = 12.0;
            let state = state
                .to_native()
                .unwrap()
                .with_asset_references(old.asset_references().to_vec())
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
        }
        let pointer = contexts.as_ref().map(|value| Arc::as_ptr(value.arc()));
        let mut rows = ArrangementClips::default();
        let mut destination = None;
        let transforms = crate::audio_transform::AudioTransformPreparation::default();
        assert!(matches!(
            crate::compile::compile_project_into_with_children(
                editor.project(),
                |_, _| panic!("preflight before source lookup"),
                None,
                &effects,
                &instruments,
                &mut contexts,
                &transforms,
                &mut destination,
                None,
                None,
                Some(&mut rows),
            ),
            Err(crate::Invalid(
                "compiler paired Track children are unavailable"
            ))
        ));
        assert!(destination.is_none());
        assert_eq!(
            contexts.as_ref().map(|value| Arc::as_ptr(value.arc())),
            pointer
        );
        assert!(rows.tracks.phase == Phase::Vacant);
        assert!(rows.tracks.rows.is_empty());
    }
}
