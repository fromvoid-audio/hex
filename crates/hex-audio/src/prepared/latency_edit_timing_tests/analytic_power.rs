use super::*;
use hex_project::{BeatTime, TrackDeviceSlot};

#[cfg(feature = "allocation-probe")]
#[test]
fn deleting_a_releasing_analytic_instrument_cuts_its_voice_without_cutting_another_track() {
    const CUT: usize = 256;
    const FADE: usize = 240;
    const DRY: Frame = [0.125, -0.0625];
    let (mut editor, _, prepare) = resident_input(vec![DRY; 1024]);
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "sine".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    let duration = editor.project().tempo_map().frame_to_beat(120).unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let old = prepare(editor.project());
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let next = prepare(editor.project());
    let mut reference = Playback::try_from_prepared_with_session(Arc::clone(&old)).unwrap();
    reference.apply(PlaybackCommand::Play);
    let mut old_pcm = [[0.0; 2]; 1024];
    reference.render(&mut old_pcm).unwrap();
    assert!(old_pcm[CUT + FADE..].iter().any(|&frame| frame != DRY));
    for partition in [1, BLOCK] {
        let (control, mut callback, original, receipt) =
            begin_insert_with_pending_track(Arc::clone(&old), Arc::clone(&next), None, CUT);
        let mut pcm = [[0.0; 2]; FADE + 100];
        for chunk in pcm[..FADE].chunks_mut(partition) {
            let counts = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(chunk).unwrap();
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
            assert_eq!(control.status.recording_source(), Some(original));
        }
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut pcm[FADE..]).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        assert!(pcm[FADE..].iter().all(|&frame| frame == DRY));
        for (offset, frame) in pcm[..FADE].iter().enumerate() {
            let gain = 1.0 - (offset + 1) as f32 / FADE as f32;
            for lane in 0..2 {
                let expected = DRY[lane] + (old_pcm[CUT + offset][lane] - DRY[lane]) * gain;
                assert!((frame[lane] - expected).abs() < 1e-6);
            }
        }
    }
    for action in 0..3 {
        let (mut control, mut callback, original, receipt) =
            begin_insert_with_pending_track(Arc::clone(&old), Arc::clone(&next), None, CUT);
        let mut prefix = [[0.0; 2]; 64];
        callback.begin_block();
        callback.render(&mut prefix).unwrap();
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        match action {
            0 => assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled),
            1 => control.command(PlaybackCommand::Play).unwrap(),
            _ => control.stop(),
        }
        let mut pcm = [[0.0; 2]; FADE + 100];
        let counts = allocation_counter::measure(|| {
            for block in pcm.chunks_mut(BLOCK) {
                callback.begin_block();
                callback.render(block).unwrap();
            }
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.recording_source(), Some(original));
        assert_eq!(
            receipt.state(),
            if action == 0 {
                PublicationReceiptState::Cancelled
            } else {
                PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
            }
        );
        if action == 2 {
            assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
        } else {
            let start_gain = 1.0 - prefix.len() as f32 / FADE as f32;
            for (offset, frame) in pcm.iter().enumerate() {
                let gain =
                    start_gain + (1.0 - start_gain) * (offset + 1).min(FADE) as f32 / FADE as f32;
                for lane in 0..2 {
                    let expected =
                        DRY[lane] + (old_pcm[CUT + prefix.len() + offset][lane] - DRY[lane]) * gain;
                    assert!(
                        (frame[lane] - expected).abs() < 1e-6,
                        "cancel/stale action {action}, frame {offset}, lane {lane}"
                    );
                }
            }
        }
    }
}

#[test]
fn analytic_instrument_power_off_fades_an_already_releasing_note_without_allocations() {
    const FADE: usize = 240;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "sine".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument track");
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    let duration = editor.project().tempo_map().frame_to_beat(120).unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |editor: &Editor| {
        let source = hex_engine::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
        )
        .unwrap();
        Playback::try_from_prepared_with_session(Arc::new(source)).unwrap()
    };
    let (mut control, mut callback) = channel(RATE).unwrap();
    control.publish(prepare(&editor)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();

    let mut reference = prepare(&editor);
    reference.apply(PlaybackCommand::Play);
    let mut warm = [[0.0; 2]; BLOCK];
    let mut expected_warm = warm;
    callback.begin_block();
    callback.render(&mut warm).unwrap();
    reference.render(&mut expected_warm).unwrap();
    assert_eq!(warm, expected_warm);
    assert!(warm[120..].iter().any(|&frame| frame != [0.0; 2]));

    let mut expected = [[0.0; 2]; FADE + 64];
    for block in expected.chunks_mut(BLOCK) {
        reference.render(block).unwrap();
    }
    assert!(expected[FADE..].iter().any(|&frame| frame != [0.0; 2]));
    assert_ne!(expected[0], [0.0; 2]);

    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let applied_revision = control.publish(prepare(&editor)).unwrap();
    let mut actual = [[1.0; 2]; FADE + 64];
    let mut render = || {
        for block in actual.chunks_mut(BLOCK) {
            callback.begin_block();
            callback.render(block).unwrap();
        }
    };
    #[cfg(feature = "allocation-probe")]
    {
        let counts = allocation_counter::measure(&mut render);
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    render();

    assert_eq!(
        control.status.applied_revision.load(Acquire),
        applied_revision
    );
    assert_eq!(
        control.status.position.load(Acquire),
        (BLOCK + actual.len()) as u64
    );
    for (index, (actual, expected)) in actual[..FADE].iter().zip(expected).enumerate() {
        let gain = (FADE - index) as f32 / FADE as f32;
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                (actual - expected * gain).abs() < 0.000_001,
                "frame {index}"
            );
        }
    }
    assert_eq!(actual[FADE..], [[0.0; 2]; 64]);
    assert_eq!(control.reclaim(), 1);
}
