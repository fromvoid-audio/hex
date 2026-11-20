use super::*;
use hex_project::{BeatTime, TrackDeviceSlot};

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
