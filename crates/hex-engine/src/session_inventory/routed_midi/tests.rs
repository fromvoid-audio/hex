use super::*;
use crate::MidiSpanBuffer;
use hex_project::{BeatTime, Command, Created, Editor, SessionCellContentSnapshot};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}
fn fixture() -> (Editor, [TrackId; 3], ClipId, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut tracks = Vec::new();
    for _ in 0..3 {
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "track".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track");
        };
        tracks.push(track);
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    let mut clips = Vec::new();
    for (track, pitch) in [(tracks[0], 60), (tracks[1], 84)] {
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "clip".into(),
                source_length: beat(4),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: beat(1),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        clips.push(clip);
    }
    (
        editor,
        [tracks[0], tracks[1], tracks[2]],
        clips[0],
        clips[1],
    )
}
fn pcm(inventory: &PreparedSessionInventory, clip: &PreparedSessionMidiClip) -> Vec<[f32; 2]> {
    let mut output = vec![[0.0; 2]; 64];
    clip.instrument_template()
        .render(
            BeatTime::ZERO,
            inventory.timing(),
            inventory.sine_table(),
            0,
            &mut output,
            &mut MidiSpanBuffer::default(),
        )
        .unwrap();
    output
}

#[test]
fn routed_session_templates_use_destination_instruments_and_keep_source_occurrences_isolated() {
    let (mut editor, [source, a, b], source_clip, hidden_local) = fixture();
    for pitch in [85, 86] {
        editor
            .execute(Command::AddMidiNote {
                clip: hidden_local,
                start: BeatTime::ZERO,
                duration: beat(1),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let table = Arc::new(SineTable::new());
    let reference =
        PreparedSessionInventory::prepare(editor.project(), Arc::clone(&table), None).unwrap();
    let expected = pcm(&reference, reference.midi_clip(source_clip).unwrap());
    assert!(expected.iter().any(|frame| frame[0] != 0.0));
    editor
        .execute(Command::RemoveInstrument { track: source })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: a,
            source: Some(source),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: b,
            source: Some(a),
        })
        .unwrap();
    let prepared =
        PreparedSessionInventory::prepare(editor.project(), Arc::clone(&table), None).unwrap();
    // Canonical inventory retains the hidden local clip for the Session UI.
    assert_eq!(prepared.midi_clips().len(), 2);
    assert_eq!(prepared.midi_clip(hidden_local).unwrap().track, a);
    assert!(prepared.midi_clip_for_track(hidden_local, a).is_none());
    let destination_work = prepared
        .tracks()
        .iter()
        .find(|track| track.id == a)
        .unwrap()
        .work;
    assert_eq!(
        destination_work,
        prepared.midi_clip_for_track(source_clip, a).unwrap().work
    );
    assert!(
        destination_work.active_voices
            < prepared.midi_clip(hidden_local).unwrap().work.active_voices
    );
    assert!(
        pcm(&prepared, prepared.midi_clip(source_clip).unwrap())
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    for destination in [a, b] {
        let routed = prepared
            .midi_clip_for_track(source_clip, destination)
            .unwrap();
        assert!(routed.instrument_enabled());
        assert_eq!(routed.track, destination);
        assert_eq!(routed.id, source_clip);
        assert_eq!(
            routed.instrument_id,
            editor
                .project()
                .track(destination)
                .unwrap()
                .instrument()
                .map(|device| device.id())
        );
        assert_eq!(pcm(&prepared, routed), expected);
        let mut spans = MidiSpanBuffer::default();
        routed
            .instrument_template()
            .query(BeatTime::ZERO, prepared.timing(), 0, 64, &mut spans)
            .unwrap();
        assert_eq!(spans.len(), 1);
        let span = spans.spans().next().unwrap();
        assert_eq!(
            (span.track, span.clip, span.pitch),
            (destination, source_clip, 60)
        );
    }
    let reused =
        PreparedSessionInventory::prepare(editor.project(), Arc::clone(&table), Some(&prepared))
            .unwrap();
    assert!(std::ptr::eq(
        prepared
            .midi_clip_for_track(source_clip, a)
            .unwrap()
            .source_template(),
        reused
            .midi_clip_for_track(source_clip, a)
            .unwrap()
            .source_template()
    ));
    assert!(std::ptr::eq(
        prepared
            .midi_clip_for_track(source_clip, a)
            .unwrap()
            .instrument_template(),
        reused
            .midi_clip_for_track(source_clip, a)
            .unwrap()
            .instrument_template()
    ));
    editor
        .execute(Command::SetMidiSource {
            destination: a,
            source: None,
        })
        .unwrap();
    let restored =
        PreparedSessionInventory::prepare(editor.project(), table, Some(&reused)).unwrap();
    assert!(restored.midi_clip_for_track(hidden_local, a).is_some());
    assert!(restored.midi_clip_for_track(source_clip, a).is_none());
    // B now resolves to A's local notes when its upstream route is cleared.
    let mut spans = MidiSpanBuffer::default();
    restored
        .midi_clip_for_track(hidden_local, b)
        .unwrap()
        .instrument_template()
        .query(BeatTime::ZERO, restored.timing(), 0, 64, &mut spans)
        .unwrap();
    assert_eq!(spans.spans().next().unwrap().pitch, 84);
}

#[test]
fn routed_session_fanout_admits_exact_weighted_note_limit_before_reserving_templates() {
    let (mut editor, [source, a, _b], _, _) = fixture();
    editor
        .execute(Command::SetMidiSource {
            destination: a,
            source: Some(source),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    // Remove destination-local notes: the weighted cost is two copies of root notes.
    let SessionCellContentSnapshot::Midi(local) = &mut snapshot.session.cells[1].content else {
        panic!("local");
    };
    local.source.notes.clear();
    let next_id = snapshot.next_id;
    let count = hex_project::MAX_NOTES / 2;
    let SessionCellContentSnapshot::Midi(root) = &mut snapshot.session.cells[0].content else {
        panic!("root");
    };
    let template = root.source.notes[0].clone();
    root.source.notes = (0..count)
        .map(|index| {
            let mut note = template.clone();
            note.id = next_id + index as u64;
            note.start_raw = beat(index as u64).raw();
            note
        })
        .collect();
    root.source.source_length_raw = beat(count as u64 + 2).raw();
    root.source.loop_end_raw = root.source.source_length_raw;
    snapshot.next_id = next_id + count as u64;
    let at_limit = Project::from_snapshot(snapshot.clone()).unwrap();
    assert_eq!(
        RoutedMidiPlan::prepare(&at_limit).unwrap().template_count,
        1
    );
    let id = snapshot.next_id;
    snapshot.next_id += 1;
    let SessionCellContentSnapshot::Midi(root) = &mut snapshot.session.cells[0].content else {
        panic!("root");
    };
    let mut extra = template;
    extra.id = id;
    extra.start_raw = beat(count as u64).raw();
    root.source.notes.push(extra);
    let over_limit = Project::from_snapshot(snapshot).unwrap();
    assert!(matches!(
        PreparedSessionInventory::prepare(&over_limit, Arc::new(SineTable::new()), None),
        Err(crate::Invalid("routed Session MIDI note key limit reached"))
    ));
}

mod volume;
