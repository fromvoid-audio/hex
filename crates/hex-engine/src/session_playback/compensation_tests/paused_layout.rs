//! A latency-layout change published while paused away from the origin.
use super::*;
use hex_project::Command;

fn limiter() -> hex_lattice_contract::NativeDeviceState {
    crate::default_native_device_state(crate::DeviceTypeId::from_static("hex.limiter")).unwrap()
}

/// Built-in registries, so real latency devices (the limiter) are prepared.
fn prepare(f: &crate::signal_graph::tests::Fixture) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

/// Pause, change one track's latency, then make a further unrelated edit.
/// Both handoffs apply: the paused cursor is kept and the candidate starts
/// with empty device/compensation history there, exactly as a Seek to that
/// cursor would. Resuming then sounds like the new revision played from it.
#[test]
fn paused_latency_layout_change_applies_at_the_cursor_and_later_edits_follow() {
    let mut f = fixture(&[&[], &[]], 512);
    let first = f.tracks[0];
    f.editor
        .execute(Command::InsertDevice {
            track: first,
            before: None,
            state: limiter(),
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 40]).unwrap();
    old.apply(SessionPlaybackCommand::Pause).unwrap();
    old.render(&mut [[0.0; 2]; 16]).unwrap();
    assert_eq!(old.performance_frame, 40);

    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: limiter(),
        })
        .unwrap();
    let latency = prepare(&f);
    assert_ne!(
        latency.arrangement().main_latency_frames(),
        old.revision.arrangement().main_latency_frames()
    );
    let mut next = SessionPlayback::new(Arc::clone(&latency)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert!(!next.playing);
    assert_eq!((next.performance_frame, next.arrangement_frame), (40, 40));

    // The following edit (another track) compares against the applied
    // revision above, not the stale pre-latency one.
    f.editor
        .execute(Command::Gain {
            track: f.tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let edited = prepare(&f);
    let mut later = SessionPlayback::new(Arc::clone(&edited)).unwrap();
    later.try_inherit_state_from(&mut next).unwrap();
    assert_eq!(later.performance_frame, 40);

    let mut oracle = SessionPlayback::new(edited).unwrap();
    oracle.apply(SessionPlaybackCommand::Seek(40)).unwrap();
    for runtime in [&mut later, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = vec![[0.0; 2]; 2048];
    let mut expected = actual.clone();
    later.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "resume equals a seek to the paused cursor"
    );
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
}

/// Playing: the change applies at the next block with the transport still
/// running. Audio restarts there (tails/PDC empty, notes chased), exactly as a
/// Play-from at that cursor; a following edit applies too.
#[test]
fn playing_latency_layout_change_restarts_audio_without_stopping() {
    let mut f = fixture(&[&[], &[]], 512);
    let first = f.tracks[0];
    f.editor
        .execute(Command::InsertDevice {
            track: first,
            before: None,
            state: limiter(),
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 40]).unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: limiter(),
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert!(next.playing);
    assert_eq!((next.performance_frame, next.arrangement_frame), (40, 40));
    f.editor
        .execute(Command::Gain {
            track: f.tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let edited = prepare(&f);
    let mut later = SessionPlayback::new(Arc::clone(&edited)).unwrap();
    later.try_inherit_state_from(&mut next).unwrap();
    assert!(later.playing);
    assert_eq!(later.performance_frame, 40);

    let mut oracle = SessionPlayback::new(edited).unwrap();
    oracle.play_from(40).unwrap();
    let mut actual = vec![[0.0; 2]; 2048];
    let mut expected = actual.clone();
    later.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "running restart equals play-from the cursor"
    );
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
}

/// Resident linear PCM demonstrates why a primed graph cannot use the host
/// cursor as its processing clock when latency changes. This is not a live
/// publication contract: loops, commands, automation and source custody still
/// require independent proof.
#[test]
fn primed_pcm_origin_requires_processing_clock_offset() {
    let mut f = fixture(&[&[]], 4096);
    for audio in f.media.values_mut() {
        *audio = crate::Audio::new(
            48_000,
            (0..4096).map(|i| [i as f32 / 8192.0, 0.0]).collect(),
        )
        .unwrap();
    }
    let no_latency = prepare(&f);
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: limiter(),
        })
        .unwrap();
    let with_latency = prepare(&f);
    let delay = with_latency.arrangement().main_latency_frames() as u64;
    assert_eq!(delay, 72);
    for (label, old_revision, new_revision, old_delay, new_delay) in [
        ("increase", &no_latency, &with_latency, 0, delay),
        ("decrease", &with_latency, &no_latency, delay, 0),
        ("equal", &with_latency, &with_latency, delay, delay),
    ] {
        for partition in [1, 257] {
            let mut old = SessionPlayback::new(Arc::clone(old_revision)).unwrap();
            old.play_from(0).unwrap();
            old.render(&mut vec![[0.0; 2]; 1024]).unwrap();
            let mut primed = SessionPlayback::new(Arc::clone(new_revision)).unwrap();
            primed.play_from(1024 - new_delay).unwrap();
            primed
                .render(&mut vec![[0.0; 2]; new_delay as usize])
                .unwrap();
            let mut old_pcm = [[0.0; 2]; 257];
            let mut primed_pcm = old_pcm;
            for chunk in old_pcm.chunks_mut(partition) {
                old.render(chunk).unwrap();
            }
            for chunk in primed_pcm.chunks_mut(partition) {
                primed.render(chunk).unwrap();
            }
            for i in 0..257 {
                let old_expected = (1024 + i as u64 - old_delay) as f32 / 8192.0;
                let new_expected = (1024 + i as u64 - new_delay) as f32 / 8192.0;
                assert!(
                    (old_pcm[i][0] - old_expected).abs() < 1e-6,
                    "{label} old {i}"
                );
                assert!(
                    (primed_pcm[i][0] - new_expected).abs() < 1e-6,
                    "{label} candidate {i}"
                );
            }
            // Align the content origin instead of the scalar processing cursor.
            let mut aligned = SessionPlayback::new(Arc::clone(new_revision)).unwrap();
            aligned.play_from(1024 - old_delay).unwrap();
            aligned
                .render(&mut vec![[0.0; 2]; new_delay as usize])
                .unwrap();
            let expected_processing_frame = 1024 - old_delay + new_delay;
            assert_eq!(aligned.arrangement_frame, expected_processing_frame);
            let mut aligned_pcm = [[0.0; 2]; 257];
            for chunk in aligned_pcm.chunks_mut(partition) {
                aligned.render(chunk).unwrap();
            }
            assert_eq!(aligned_pcm, old_pcm, "{label} partition={partition}");
        }
    }
}

#[derive(Clone, Copy)]
enum SilentLatencyCase {
    Eligible,
    ActiveChangedTrack,
    BufferedUnmutedTrack,
    StaleMutedHistory,
    IncreasedMaximum,
    SendsToReturn,
    ChildOfGroup,
}

#[test]
fn silent_isolated_latency_addition_preserves_other_track_reverb() {
    for partition in [1, 257] {
        silent_latency_addition_case(SilentLatencyCase::Eligible, partition);
    }
}

#[test]
fn isolated_latency_insert_keeps_unchanged_operator_note_and_release() {
    isolated_latency_edit_keeps_operator(IsolatedLatencyEdit::Insert, false);
}

#[test]
fn isolated_latency_removal_keeps_unchanged_operator_note_and_release() {
    isolated_latency_edit_keeps_operator(IsolatedLatencyEdit::Remove, false);
}

#[test]
fn approved_isolated_latency_cut_keeps_operator_gate_and_release() {
    for edit in [IsolatedLatencyEdit::Insert, IsolatedLatencyEdit::Remove] {
        isolated_latency_edit_keeps_operator(edit, true);
    }
}

#[derive(Clone, Copy)]
enum IsolatedLatencyEdit {
    Insert,
    Remove,
}

fn isolated_latency_edit_keeps_operator(edit: IsolatedLatencyEdit, approved: bool) {
    use crate::{OutputCapture, OutputCaptureId};
    use hex_project::{BeatTime, Created};

    let mut f = fixture(&[&[], &[]], 8_192);
    let edited = f.tracks[0];
    let instrument = f.tracks[1];
    for (index, audio) in f.media.values_mut().enumerate() {
        let sample = if approved && index == 0 {
            [0.25, -0.125]
        } else {
            [0.0; 2]
        };
        *audio = crate::Audio::new(48_000, vec![sample; 8_192]).unwrap();
    }
    f.editor
        .execute(Command::InsertInstrument {
            track: instrument,
            state: crate::default_native_device_state_for_class(
                crate::NativeDeviceClass::Instrument,
                crate::DeviceTypeId::from_static("hex.instrument.operator"),
            )
            .unwrap(),
        })
        .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: instrument,
            before: None,
            state: limiter(),
        })
        .unwrap();
    let Created::Clip(clip) = f
        .editor
        .execute(Command::PlaceMidiClip {
            track: instrument,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument clip")
    };
    f.editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(17, 64).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let (old_revision, new_revision) = isolated_latency_revisions(&mut f, edited, edit);
    let latency = old_revision.arrangement().main_latency_frames();
    assert!(latency > 0);
    assert_eq!(new_revision.arrangement().main_latency_frames(), latency);
    let device = f
        .editor
        .project()
        .track(instrument)
        .unwrap()
        .instrument()
        .unwrap();
    let new_state = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        device.state(),
        hex_devices::operator_state::parameter::oscillator_level(1),
        hex_lattice_contract::ParameterValue::Float(0.43),
    )
    .unwrap();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device: device.id(),
            state: new_state,
        })
        .unwrap();
    let changed_instrument = prepare(&f);

    const CUT: u64 = 6_000;
    const FRAMES: usize = 1_024;
    for partition in [1, 257] {
        let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        let mut uninterrupted = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        assert!(old.mutable_instruments.iter().any(Option::is_some));
        for runtime in [&mut old, &mut uninterrupted] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut vec![[0.0; 2]; CUT as usize]).unwrap();
        }
        let mut mixed = SessionPlayback::new(Arc::clone(&changed_instrument)).unwrap();
        mixed.require_continuous_insert = true;
        assert!(matches!(
            mixed.try_inherit_state_live_from(&mut old),
            Err(SessionPlaybackError::DeviceInsertNeedsConfirmation)
        ));
        assert_eq!(old.performance_frame(), CUT);
        let mut candidate = SessionPlayback::new(Arc::clone(&new_revision)).unwrap();
        inherit_isolated_latency_edit(&mut candidate, &mut old, approved);
        let render = |runtime: &mut SessionPlayback| {
            let plan = runtime
                .output_capture_plan(&[OutputCaptureId::Track(instrument)], partition)
                .unwrap();
            let mut main = [[0.0; 2]; FRAMES];
            let mut notes = [[0.0; 2]; FRAMES];
            for (main, notes) in main.chunks_mut(partition).zip(notes.chunks_mut(partition)) {
                runtime
                    .render_with_capture(main, &mut OutputCapture::new(&plan, &mut [notes]))
                    .unwrap();
            }
            notes
        };
        let expected = render(&mut uninterrupted);
        let actual = render(&mut candidate);
        let bits = |frames: [[f32; 2]; FRAMES]| frames.map(|frame| frame.map(f32::to_bits));
        assert_eq!(bits(actual), bits(expected), "partition {partition}");
        let release = 375 + latency as usize;
        assert!(expected[..release].iter().flatten().any(|x| x.abs() > 1e-6));
        assert!(expected[release..].iter().flatten().any(|x| x.abs() > 1e-6));
        let mut restarted = SessionPlayback::new(Arc::clone(&new_revision)).unwrap();
        restarted.play_from(CUT).unwrap();
        assert_ne!(bits(render(&mut restarted)), bits(expected));
    }
}

#[test]
fn isolated_latency_insert_keeps_unchanged_group_and_return_tails() {
    isolated_latency_edit_keeps_group_and_return(IsolatedLatencyEdit::Insert, false);
}

#[test]
fn isolated_latency_removal_keeps_unchanged_group_and_return_tails() {
    isolated_latency_edit_keeps_group_and_return(IsolatedLatencyEdit::Remove, false);
}

#[test]
fn approved_isolated_latency_cut_keeps_group_and_return_tails() {
    for edit in [IsolatedLatencyEdit::Insert, IsolatedLatencyEdit::Remove] {
        isolated_latency_edit_keeps_group_and_return(edit, true);
    }
}

fn isolated_latency_edit_keeps_group_and_return(edit: IsolatedLatencyEdit, approved: bool) {
    use crate::{OutputCapture, OutputCaptureId};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Created, TrackSelectionIds};

    let mut f = fixture(&[&[], &[], &[]], 12_000);
    for (index, audio) in f.media.values_mut().enumerate() {
        let mut samples = vec![[0.0; 2]; 12_000];
        let end = if approved && index == 0 {
            samples.len()
        } else {
            512
        };
        samples[..end].fill([0.25, -0.125]);
        *audio = crate::Audio::new(48_000, samples).unwrap();
    }
    let changed = f.tracks[0];
    let child = f.tracks[1];
    let Created::Track(group) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&f.tracks[1..]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "room".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: child,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    // The delayed child and its unprocessed sibling converge at the group.
    // The same delayed child's send also reaches Main through the Return.
    f.editor
        .execute(Command::InsertDevice {
            track: child,
            before: None,
            state: limiter(),
        })
        .unwrap();
    let reverb = &hex_devices::reverb::REVERB_DEFINITION;
    let wet = reverb
        .patch_parameter(
            &reverb.default_state().unwrap(),
            hex_devices::reverb::parameter::DRY_WET,
            ParameterValue::Float(100.0),
        )
        .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: group,
            before: None,
            state: wet.clone(),
        })
        .unwrap();
    f.editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: wet,
        })
        .unwrap();
    let (old_revision, new_revision) = isolated_latency_revisions(&mut f, changed, edit);
    let latency = old_revision.arrangement().main_latency_frames();
    assert!(latency > 0);
    assert_eq!(new_revision.arrangement().main_latency_frames(), latency);
    let changed_arrival = |revision: &PreparedRevision| {
        revision
            .arrangement()
            .signal_graph
            .as_ref()
            .unwrap()
            .resident_main_alignment(0)
            .unwrap()
            .1
    };
    let (before, after) = match edit {
        IsolatedLatencyEdit::Insert => (0, latency),
        IsolatedLatencyEdit::Remove => (latency, 0),
    };
    assert_eq!(changed_arrival(&old_revision), before);
    assert_eq!(changed_arrival(&new_revision), after);

    for partition in [1, 257] {
        let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        let mut uninterrupted = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        let cut = 6_000;
        for runtime in [&mut old, &mut uninterrupted] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut vec![[0.0; 2]; cut]).unwrap();
        }
        let mut candidate = SessionPlayback::new(Arc::clone(&new_revision)).unwrap();
        let mut fresh = SessionPlayback::new(Arc::clone(&new_revision)).unwrap();
        fresh.play_from(cut as u64).unwrap();
        inherit_isolated_latency_edit(&mut candidate, &mut old, approved);
        let render = |runtime: &mut SessionPlayback| {
            let ids = [
                OutputCaptureId::Track(group),
                OutputCaptureId::Return(bus),
                OutputCaptureId::Track(changed),
            ];
            let plan = runtime.output_capture_plan(&ids, partition).unwrap();
            let mut pcm = [[0.0; 2]; 512];
            let mut group = pcm;
            let mut bus = pcm;
            let mut changed = pcm;
            for (((pcm, group), bus), changed) in pcm
                .chunks_mut(partition)
                .zip(group.chunks_mut(partition))
                .zip(bus.chunks_mut(partition))
                .zip(changed.chunks_mut(partition))
            {
                runtime
                    .render_with_capture(
                        pcm,
                        &mut OutputCapture::new(&plan, &mut [group, bus, changed]),
                    )
                    .unwrap();
            }
            (pcm, group, bus, changed)
        };
        let (main, group, bus, affected) = render(&mut candidate);
        let (old_main, old_group, old_bus, _) = render(&mut uninterrupted);
        let (_, _, _, fresh_affected) = render(&mut fresh);
        let bits = |pcm: [[f32; 2]; 512]| pcm.map(|frame| frame.map(f32::to_bits));
        if !approved {
            assert_eq!(bits(main), bits(old_main));
        }
        assert_eq!(bits(group), bits(old_group));
        assert_eq!(bits(bus), bits(old_bus));
        assert_eq!(bits(affected), bits(fresh_affected));
        for tail in [group, bus] {
            assert!(tail.iter().flatten().any(|sample| sample.abs() > 1e-7));
        }
    }
}

fn inherit_isolated_latency_edit(
    candidate: &mut SessionPlayback,
    predecessor: &mut SessionPlayback,
    approved: bool,
) {
    candidate.require_continuous_insert = true;
    if approved {
        let cut = predecessor.performance_frame();
        assert!(matches!(
            candidate.try_inherit_state_live_from(predecessor),
            Err(SessionPlaybackError::DeviceInsertNeedsConfirmation)
        ));
        assert!(predecessor.playing());
        assert_eq!(predecessor.performance_frame(), cut);
        predecessor.set_playing_at_device_insert_cut(false);
        candidate
            .try_inherit_state_at_device_insert_cut(predecessor)
            .unwrap();
        assert_eq!(candidate.performance_frame(), cut);
        candidate.set_playing_at_device_insert_cut(true);
    } else {
        candidate.try_inherit_state_live_from(predecessor).unwrap();
    }
}

fn isolated_latency_revisions(
    fixture: &mut crate::signal_graph::tests::Fixture,
    track: TrackId,
    edit: IsolatedLatencyEdit,
) -> (Arc<PreparedRevision>, Arc<PreparedRevision>) {
    let without = prepare(fixture);
    let hex_project::Created::Device(device) = fixture
        .editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: limiter(),
        })
        .unwrap()
    else {
        panic!("limiter");
    };
    let with = prepare(fixture);
    match edit {
        IsolatedLatencyEdit::Insert => (without, with),
        IsolatedLatencyEdit::Remove => {
            fixture
                .editor
                .execute(Command::RemoveDevice(device))
                .unwrap();
            (with, prepare(fixture))
        }
    }
}

#[test]
fn unproven_latency_additions_keep_existing_restart_behavior() {
    for case in [
        SilentLatencyCase::ActiveChangedTrack,
        SilentLatencyCase::BufferedUnmutedTrack,
        SilentLatencyCase::StaleMutedHistory,
        SilentLatencyCase::IncreasedMaximum,
        SilentLatencyCase::SendsToReturn,
        SilentLatencyCase::ChildOfGroup,
    ] {
        silent_latency_addition_case(case, 257);
    }
}

fn silent_latency_addition_case(case: SilentLatencyCase, partition: usize) {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::Created;
    let mut f = fixture(&[&[], &[]], 12000);
    for (index, audio) in f.media.values_mut().enumerate() {
        let mut samples = vec![[0.0; 2]; 12000];
        if index == 0
            && matches!(
                case,
                SilentLatencyCase::ActiveChangedTrack
                    | SilentLatencyCase::BufferedUnmutedTrack
                    | SilentLatencyCase::StaleMutedHistory
            )
        {
            samples[..7000].fill([0.125, -0.0625]);
        } else {
            samples[..512].fill([0.25, -0.125]);
        }
        *audio = crate::Audio::new(48_000, samples).unwrap();
    }
    if !matches!(
        case,
        SilentLatencyCase::ActiveChangedTrack
            | SilentLatencyCase::BufferedUnmutedTrack
            | SilentLatencyCase::StaleMutedHistory
    ) {
        f.editor
            .execute(Command::Mute {
                track: f.tracks[0],
                muted: true,
            })
            .unwrap();
    }
    if matches!(case, SilentLatencyCase::ChildOfGroup) {
        f.editor
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&f.tracks).unwrap(),
                name: "group".into(),
            })
            .unwrap();
    }
    if matches!(case, SilentLatencyCase::SendsToReturn) {
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "room".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        f.editor
            .execute(Command::SetReturnSend {
                track: f.tracks[0],
                return_track: bus,
                gain: 1.0,
            })
            .unwrap();
    }
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[1],
            before: None,
            state: limiter(),
        })
        .unwrap();
    let reverb = &hex_devices::reverb::REVERB_DEFINITION;
    let state = reverb
        .patch_parameter(
            &reverb.default_state().unwrap(),
            hex_devices::reverb::parameter::DRY_WET,
            ParameterValue::Float(100.0),
        )
        .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[1],
            before: None,
            state,
        })
        .unwrap();
    let old_revision = prepare(&f);
    let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(old_revision).unwrap();
    for runtime in [&mut old, &mut reference] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    if matches!(
        case,
        SilentLatencyCase::ActiveChangedTrack | SilentLatencyCase::StaleMutedHistory
    ) {
        f.editor
            .execute(Command::Mute {
                track: f.tracks[0],
                muted: true,
            })
            .unwrap();
        if matches!(case, SilentLatencyCase::StaleMutedHistory) {
            let mut muted = SessionPlayback::new(prepare(&f)).unwrap();
            muted.try_inherit_state_live_from(&mut old).unwrap();
            old = muted;
        }
    }
    let added = if matches!(case, SilentLatencyCase::IncreasedMaximum) {
        let definition = &hex_devices::limiter::DEFINITION;
        definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                hex_devices::limiter::parameter::LOOKAHEAD,
                ParameterValue::Choice(2),
            )
            .unwrap()
    } else {
        limiter()
    };
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: added,
        })
        .unwrap();
    let revision = prepare(&f);
    let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    if !matches!(case, SilentLatencyCase::Eligible) {
        candidate.require_continuous_insert = true;
        assert!(matches!(
            candidate.try_inherit_state_live_from(&mut old),
            Err(SessionPlaybackError::DeviceInsertNeedsConfirmation)
        ));
        assert_eq!(old.performance_frame(), 6_000);
        assert!(old.playing());
        candidate.require_continuous_insert = false;
    }
    candidate.try_inherit_state_live_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 512];
    for chunk in actual.chunks_mut(partition) {
        candidate.render(chunk).unwrap();
    }
    let mut expected = [[0.0; 2]; 512];
    if matches!(case, SilentLatencyCase::Eligible) {
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert!(
            expected.iter().flatten().any(|sample| *sample != 0.0),
            "reference tail must sound"
        );
    } else {
        let mut restart = SessionPlayback::new(revision).unwrap();
        restart.play_from(6000).unwrap();
        for chunk in expected.chunks_mut(partition) {
            restart.render(chunk).unwrap();
        }
    }
    assert_eq!(actual, expected);
}
