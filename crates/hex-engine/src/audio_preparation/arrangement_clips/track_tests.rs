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
        .execute(Command::InsertTrackWithDevice {
            name: "second".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
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
    s.tracks[0].muted = solo;
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
                incoming_gain: None,
                beat: BeatTime::ZERO,
                gain: 0.5,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                incoming_gain: None,
                beat: BeatTime::from_raw(1),
                gain: 0.75,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                incoming_gain: None,
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
                    if index == 0 {
                        assert!(!actual.mix.muted(), "solo must override authored mute");
                        assert!(
                            !actual.mix_at(0).muted(),
                            "automation must preserve solo precedence"
                        );
                    }
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
