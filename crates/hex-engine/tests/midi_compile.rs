use hex_devices::{
    SineTable,
    utility::{UTILITY_DEFINITION, parameter as utility_parameter},
};
use hex_engine::{
    Audio, Invalid, PreparedSessionInventory, Renderer, compile,
    midi_compile::{MAX_MIDI_EVENT_EDGES, MidiEventKind, midi_events},
    playback::{Playback, PlaybackCommand},
};
use hex_lattice::NativeDeviceDefinition;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    BeatTime, Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint,
    MidiNoteSnapshot, Project, Tempo, TrackContentSnapshot,
};
use std::{collections::BTreeMap, sync::Arc};

fn beat(num: u64, den: u64) -> BeatTime {
    BeatTime::from_ratio(num, den).unwrap()
}
fn setup() -> (Editor, hex_project::TrackId) {
    setup_at_rate(48_000)
}

#[test]
fn grouping_mixed_clip_activation_keeps_scheduled_notes_and_inactive_content() {
    let (mut editor, track) = setup();
    let active = add_clip(&mut editor, track, BeatTime::ZERO, beat(2, 1));
    let inactive = add_clip(&mut editor, track, beat(4, 1), beat(2, 1));
    for (clip, pitch) in [(active, 60), (inactive, 67)] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: beat(1, 1),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetClipActive {
            clip: inactive,
            active: false,
        })
        .unwrap();
    let notes = |project: &Project| {
        midi_events(project)
            .unwrap()
            .into_iter()
            .map(|e| (e.frame, e.kind, e.pitch, e.channel, e.velocity))
            .collect::<Vec<_>>()
    };
    let before = notes(editor.project());
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(vec![active, inactive]))
        .unwrap()
    else {
        panic!("group")
    };
    assert_eq!(notes(editor.project()), before);
    let grouped = editor.project().midi_clip(group).unwrap();
    assert!(grouped.active());
    assert_eq!(
        grouped
            .notes()
            .iter()
            .map(|note| note.active())
            .collect::<Vec<_>>(),
        vec![true, false]
    );
    assert!(editor.undo());
    assert_eq!(notes(editor.project()), before);
    assert!(editor.redo());
    assert_eq!(notes(editor.project()), before);
}

#[test]
fn inactive_subframe_note_does_not_prevent_midi_preparation() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(1, 1));
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    else {
        panic!("note")
    };
    editor
        .execute(Command::SetMidiNotesActive {
            clip,
            notes: vec![note],
            active: false,
        })
        .unwrap();
    assert!(midi_events(editor.project()).unwrap().is_empty());
    compile::project(editor.project(), &BTreeMap::new()).unwrap();
}
fn setup_at_rate(sample_rate: u32) -> (Editor, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(sample_rate).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    (editor, track)
}
fn add_clip(
    editor: &mut Editor,
    track: hex_project::TrackId,
    start: BeatTime,
    len: BeatTime,
) -> hex_project::ClipId {
    match editor
        .execute(Command::PlaceMidiClip {
            track,
            start,
            length: len,
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip"),
    }
}

fn rendered_excerpt(project: &Project, frame: u64) -> [hex_engine::Frame; 64] {
    let compiled = compile::project(project, &BTreeMap::new()).unwrap();
    let mut playback = Playback::new(compiled);
    playback.apply(PlaybackCommand::Seek(frame));
    playback.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 64];
    playback.render(&mut output).unwrap();
    output
}

#[test]
fn alias_notes_keep_distinct_occurrences_and_make_unique_preserves_sound() {
    let (mut editor, track) = setup_at_rate(8_000);
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(2, 1));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(2, 1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let Created::Clips { first: alias, .. } = editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap()
    else {
        panic!("alias")
    };
    let events = midi_events(editor.project()).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].clip, clip);
    assert_eq!(events[2].clip, alias);
    assert_eq!(events[0].note, events[2].note);
    assert_eq!(events[1].frame, events[2].frame);
    let render = |project: &Project, block: usize| {
        let compiled = compile::project(project, &BTreeMap::new()).unwrap();
        let mut renderer = Renderer::new(&compiled);
        renderer.play();
        let mut output = vec![[0.0; 2]; 20_000];
        for chunk in output.chunks_mut(block) {
            renderer.render(chunk).unwrap();
        }
        output
    };
    let expected = render(editor.project(), 127);
    for start in [0, 8_000] {
        assert!(
            expected[start..start + 8_000]
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.02)
        );
    }
    editor
        .execute(Command::MakeClipsUnique(vec![alias]))
        .unwrap();
    let unique_events = midi_events(editor.project()).unwrap();
    assert_ne!(unique_events[0].note, unique_events[2].note);
    for block in [1, 17, 257] {
        assert_eq!(render(editor.project(), block), expected);
    }
    assert!(editor.undo());
    assert_eq!(render(editor.project(), 257), expected);
}

#[test]
fn note_edges_convert_across_tempo_changes_and_sort_off_before_on() {
    let (mut editor, track) = setup();
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(1, 1),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(3, 1));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1, 2),
            duration: beat(1, 2),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1, 1),
            duration: beat(1, 2),
            pitch: 62,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    let events = midi_events(editor.project()).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].frame, 12_000);
    assert_eq!(events[1].frame, 24_000);
    assert_eq!(events[1].kind, MidiEventKind::NoteOff);
    assert_eq!(events[2].frame, 24_000);
    assert_eq!(events[2].kind, MidiEventKind::NoteOn);
    assert_eq!(events[3].frame, 48_000); // tempo change affects note-off conversion.
    let (_, preview) =
        compile::project_with_midi_preview(editor.project(), &BTreeMap::new()).unwrap();
    let summary = preview
        .get(editor.project().tracks()[0].midi_clips()[0].id())
        .expect("one-shot summary");
    assert_eq!((summary.frame_start, summary.frame_end), (0, 120_000));
    assert_eq!(summary.occurrence_count, 2);
    assert_eq!(
        summary
            .samples
            .iter()
            .map(|sample| (sample.frame_start, sample.frame_gate_end, sample.pitch))
            .collect::<Vec<_>>(),
        [(12_000, 24_000, 60), (24_000, 48_000, 62)]
    );
}

#[test]
fn source_offset_local_loop_projects_tempo_edges_gates_and_finite_tail() {
    let (mut editor, track) = setup();
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(4, 1),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(4, 1));
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(8, 1),
            source_start: beat(1, 1),
            loop_start: beat(2, 1),
            loop_end: beat(4, 1),
            looping: true,
            length: beat(13, 2),
        })
        .unwrap();
    let hidden = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1, 2),
            duration: beat(1, 4),
            pitch: 48,
            channel: 0,
            velocity: 90,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note expected"),
    };
    let intro = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(3, 2),
            duration: beat(1, 1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note expected"),
    };
    let looping = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(5, 2),
            duration: beat(5, 2),
            pitch: 62,
            channel: 0,
            velocity: 110,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note expected"),
    };
    let outside_loop = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(6, 1),
            duration: beat(1, 4),
            pitch: 64,
            channel: 0,
            velocity: 90,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note expected"),
    };

    let events = midi_events(editor.project()).unwrap();
    assert_eq!(events.len(), 8);
    assert!(!events.iter().any(|event| event.note == hidden));
    assert!(!events.iter().any(|event| event.note == outside_loop));
    let actual = events
        .iter()
        .map(|event| (event.frame, event.kind, event.note, event.iteration))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            (12_000, MidiEventKind::NoteOn, intro, 0),
            (36_000, MidiEventKind::NoteOff, intro, 0),
            (36_000, MidiEventKind::NoteOn, looping, 0),
            (72_000, MidiEventKind::NoteOff, looping, 0),
            (84_000, MidiEventKind::NoteOn, looping, 1),
            (144_000, MidiEventKind::NoteOff, looping, 1),
            (168_000, MidiEventKind::NoteOn, looping, 2),
            (216_000, MidiEventKind::NoteOff, looping, 2),
        ]
    );

    let linear = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let (_, preview) =
        compile::project_with_midi_preview(editor.project(), &BTreeMap::new()).unwrap();
    let summary = preview.get(clip).expect("loop summary");
    assert_eq!((summary.frame_start, summary.frame_end), (0, 216_000));
    assert_eq!(
        (summary.authored_note_count, summary.occurrence_count),
        (4, 4)
    );
    assert_eq!(
        summary
            .samples
            .iter()
            .map(|sample| (sample.frame_start, sample.frame_gate_end, sample.pitch))
            .collect::<Vec<_>>(),
        [
            (12_000, 36_000, 60),
            (36_000, 72_000, 62),
            (84_000, 144_000, 62),
            (168_000, 216_000, 62),
        ]
    );
    assert_eq!(linear.audible_end_frame(), Some(218_400));
    let mut renderer = Renderer::new(&linear);
    renderer.play();
    let mut tail = vec![[0.0; 2]; 218_410];
    renderer.render(&mut tail).unwrap();
    assert!(
        tail[216_000..218_400]
            .iter()
            .any(|frame| frame[0].abs() + frame[1].abs() > 0.0)
    );
    assert!(tail[218_400..].iter().all(|frame| *frame == [0.0; 2]));

    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 216_000,
            enabled: true,
        })
        .unwrap();
    let project = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let mut whole_playback = Playback::new(project.clone());
    whole_playback.apply(PlaybackCommand::Play);
    let mut whole = vec![[0.0; 2]; 250_000];
    whole_playback.render(&mut whole).unwrap();

    let mut split_playback = Playback::new(project.clone());
    split_playback.apply(PlaybackCommand::Play);
    let mut split = vec![[0.0; 2]; 250_000];
    for chunk in split.chunks_mut(127) {
        split_playback.render(chunk).unwrap();
    }
    assert_eq!(whole, split);

    let mut seek = Playback::new(project);
    seek.apply(PlaybackCommand::Seek(83_900));
    seek.apply(PlaybackCommand::Play);
    let mut sought = [[0.0; 2]; 256];
    seek.render(&mut sought).unwrap();
    assert_eq!(sought, whole[83_900..84_156]);
}

#[test]
fn looping_preview_is_evenly_sampled_and_includes_empty_clips() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(1, 1));
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(1, 1),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: beat(1, 100),
            looping: true,
            length: beat(1, 1),
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1, 200),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let empty = add_clip(&mut editor, track, beat(2, 1), beat(1, 1));

    let (_, preview) =
        compile::project_with_midi_preview(editor.project(), &BTreeMap::new()).unwrap();
    let loop_summary = preview.get(clip).expect("loop summary");
    assert_eq!(loop_summary.occurrence_count, 100);
    assert_eq!(loop_summary.samples.len(), 32);
    assert_eq!(
        loop_summary
            .samples
            .first()
            .map(|sample| sample.frame_start),
        Some(0)
    );
    assert_eq!(
        loop_summary.samples.last().map(|sample| sample.frame_start),
        Some(23_760)
    );
    let sample_gaps = loop_summary
        .samples
        .windows(2)
        .map(|pair| pair[1].frame_start - pair[0].frame_start)
        .collect::<Vec<_>>();
    assert!(sample_gaps.iter().all(|gap| matches!(gap, 720 | 960)));
    assert!(sample_gaps.contains(&720));
    assert!(sample_gaps.contains(&960));
    let empty_summary = preview.get(empty).expect("empty clip summary");
    assert_eq!(empty_summary.authored_note_count, 0);
    assert_eq!(empty_summary.occurrence_count, 0);
    assert!(empty_summary.samples.is_empty());
    assert!(
        preview
            .clips()
            .windows(2)
            .all(|pair| pair[0].clip_id < pair[1].clip_id)
    );
}

#[test]
fn project_wide_occurrence_budget_rejects_before_edges_or_spans_are_emitted() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(4, 1));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: BeatTime::from_raw(2),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::from_raw(1),
            looping: true,
            length: BeatTime::from_raw(65_538),
        })
        .unwrap();

    assert_eq!(
        midi_events(editor.project()),
        Err(Invalid("compiled midi occurrence limit reached"))
    );
    assert_eq!(
        compile::project(editor.project(), &BTreeMap::new()).unwrap_err(),
        Invalid("compiled midi occurrence limit reached")
    );
}

#[test]
fn zero_frame_note_is_explicitly_refused() {
    let (mut editor, track) = setup();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(1_000_000_000).unwrap(),
        })
        .unwrap();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, BeatTime::ONE);
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    assert_eq!(
        midi_events(editor.project()),
        Err(Invalid("midi note duration quantizes to zero frames"))
    );
    assert_eq!(
        compile::project(editor.project(), &BTreeMap::new()).unwrap_err(),
        Invalid("midi note duration quantizes to zero frames")
    );
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1, 2),
            duration: BeatTime::from_raw(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    // Force right-remainder materialization without clipping this second
    // authored gate. Frame editing must not turn invalid source data into
    // silently accepted playback by filtering every zero-frame note.
    editor
        .execute(Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], 10_000, 10_720).unwrap(),
            track,
            frame: 0,
        })
        .unwrap();
    let notes = editor
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .iter()
        .flat_map(|clip| clip.notes())
        .collect::<Vec<_>>();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].duration(), BeatTime::from_raw(1));
    assert_eq!(
        midi_events(editor.project()),
        Err(Invalid("midi note duration quantizes to zero frames"))
    );
}

#[test]
fn loop_clamped_subframe_gate_is_skipped_without_rejecting_project() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, BeatTime::ONE);
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(2, 1),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::ONE,
            looping: true,
            length: BeatTime::ONE,
        })
        .unwrap();
    for (start, duration, pitch) in [
        (BeatTime::ZERO, beat(1, 2), 60),
        (
            BeatTime::from_raw(BeatTime::ONE.raw() - 1),
            BeatTime::from_raw(100),
            72,
        ),
    ] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration,
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let events = midi_events(editor.project()).unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.pitch == 60));
    compile::project(editor.project(), &BTreeMap::new()).unwrap();
}

fn arrangement_source_window_editor(high_note_start: BeatTime) -> Editor {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(3, 1));
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(4, 1),
            source_start: beat(1, 1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4, 1),
            looping: false,
            length: beat(3, 1),
        })
        .unwrap();
    for (start, pitch) in [(beat(3, 2), 60), (high_note_start, 127)] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration: beat(1, 4),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    editor
}

#[test]
fn arrangement_bank_ignores_notes_outside_the_emitted_source_window() {
    let hidden = arrangement_source_window_editor(beat(1, 2));
    assert!(compile::project(hidden.project(), &BTreeMap::new()).is_ok());

    let emitted = arrangement_source_window_editor(beat(5, 2));
    assert!(compile::project(emitted.project(), &BTreeMap::new()).is_err());
}

fn session_source_window_editor(high_note_start: BeatTime) -> Editor {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "source".into(),
            source_length: beat(4, 1),
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("session clip"),
    };
    editor
        .execute(Command::SetSessionMidiClipSource {
            clip,
            source_length: beat(4, 1),
            source_start: beat(1, 1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4, 1),
            looping: false,
        })
        .unwrap();
    for (start, pitch) in [(beat(3, 2), 60), (high_note_start, 127)] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration: beat(1, 4),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    editor
}

#[test]
fn session_bank_ignores_notes_outside_the_emitted_source_window() {
    let hidden = session_source_window_editor(beat(1, 2));
    assert!(
        PreparedSessionInventory::prepare(hidden.project(), Arc::new(SineTable::new()), None,)
            .is_ok()
    );

    let emitted = session_source_window_editor(beat(5, 2));
    assert!(
        PreparedSessionInventory::prepare(emitted.project(), Arc::new(SineTable::new()), None,)
            .is_err()
    );
}

#[test]
fn ordered_instrument_segment_oracle_covers_audio_midi_and_hybrid_tracks() {
    let (mut editor, track) = setup();
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/ordered.wav".into(),
            sample_rate: 48_000,
            frames: 24_000,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset"),
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 24_000,
            source_offset: 0,
            source_length: 24_000,
            looping: false,
        })
        .unwrap();
    let midi = add_clip(&mut editor, track, BeatTime::ZERO, beat(1, 1));
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: beat(1, 2),
            pitch: 69,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let utility = UTILITY_DEFINITION.default_state().unwrap();
    let invert = UTILITY_DEFINITION
        .patch_parameter(
            &utility,
            utility_parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let swap = UTILITY_DEFINITION
        .patch_parameter(
            &utility,
            utility_parameter::CHANNEL,
            ParameterValue::Choice(3),
        )
        .unwrap();
    let prefix_id = match editor
        .execute(Command::InsertDevice {
            track,
            before: Some(instrument),
            state: invert.clone(),
        })
        .unwrap()
    {
        Created::Device(id) => id,
        _ => panic!("prefix effect"),
    };
    let suffix_id = match editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: swap.clone(),
        })
        .unwrap()
    {
        Created::Device(id) => id,
        _ => panic!("suffix effect"),
    };
    let prefix_matrix = UTILITY_DEFINITION.prepare_effect_matrix(&invert).unwrap();
    let suffix_matrix = UTILITY_DEFINITION.prepare_effect_matrix(&swap).unwrap();
    let media = BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![[0.25, -0.5]; 24_000]).unwrap(),
    )]);
    let render = |project: &hex_project::Project| {
        let compiled = compile::project(project, &media).unwrap();
        let mut renderer = Renderer::new(&compiled);
        renderer.play();
        let mut output = vec![[0.0; 2]; 24_000];
        renderer.render(&mut output).unwrap();
        output
    };
    let mut audio_only = editor.project().snapshot();
    let mut midi_only = editor.project().snapshot();
    for snapshot in [&mut audio_only, &mut midi_only] {
        let target = &mut snapshot.tracks[0];
        target.devices.clear();
        target.instrument_position = Some(0);
    }
    if let hex_project::TrackContentSnapshot::Hybrid {
        audio_clips: _,
        midi_clips,
    } = &mut audio_only.tracks[0].content
    {
        midi_clips.clear();
    }
    if let hex_project::TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips: _,
    } = &mut midi_only.tracks[0].content
    {
        audio_clips.clear();
    }
    let audio = render(&hex_project::Project::from_snapshot(audio_only).unwrap());
    let midi = render(&hex_project::Project::from_snapshot(midi_only).unwrap());

    let base_project = editor.project().clone();
    let renders_at_position = |position: usize| {
        let mut candidate = Editor::new(base_project.clone(), 32).unwrap();
        let before = match position {
            0 => Some(prefix_id),
            1 => Some(suffix_id),
            _ => None,
        };
        candidate
            .execute(Command::MoveTrackDevice {
                device: instrument,
                track,
                before,
            })
            .unwrap();
        assert_eq!(
            candidate
                .project()
                .track(track)
                .unwrap()
                .instrument_position(),
            Some(position)
        );
        render(candidate.project())
    };
    for (position, prefix, suffix) in [
        (0, None, Some(prefix_matrix.then(suffix_matrix).unwrap())),
        (1, Some(prefix_matrix), Some(suffix_matrix)),
        (2, Some(prefix_matrix.then(suffix_matrix).unwrap()), None),
    ] {
        let actual = renders_at_position(position);
        for frame in 0..actual.len() {
            let mut expected = prefix.map_or(audio[frame], |matrix| matrix.apply(audio[frame]));
            expected[0] += midi[frame][0];
            expected[1] += midi[frame][1];
            if let Some(matrix) = suffix {
                expected = matrix.apply(expected);
            }
            for channel in 0..2 {
                assert!(
                    (actual[frame][channel] - expected[channel]).abs() < 0.000_001,
                    "position {position}, frame {frame}, channel {channel}: actual {:?}, expected {:?}",
                    actual[frame],
                    expected
                );
            }
        }
    }

    // Audio-only and MIDI-only sources enter the same ordered chain at their
    // respective source positions.
    let mut ordered_audio_only = editor.project().snapshot();
    let mut ordered_midi_only = editor.project().snapshot();
    if let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut ordered_audio_only.tracks[0].content
    {
        midi_clips.clear();
    }
    if let hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } =
        &mut ordered_midi_only.tracks[0].content
    {
        audio_clips.clear();
    }
    let actual_audio_only =
        render(&hex_project::Project::from_snapshot(ordered_audio_only).unwrap());
    let actual_midi_only = render(&hex_project::Project::from_snapshot(ordered_midi_only).unwrap());
    for frame in 0..actual_audio_only.len() {
        let expected = suffix_matrix.apply(prefix_matrix.apply(audio[frame]));
        for channel in 0..2 {
            assert!((actual_audio_only[frame][channel] - expected[channel]).abs() < 0.000_001);
        }
        let expected_midi = suffix_matrix.apply(midi[frame]);
        for channel in 0..2 {
            assert!((actual_midi_only[frame][channel] - expected_midi[channel]).abs() < 0.000_001);
        }
    }
    assert!(editor.project().device(suffix_id).is_some());
}

#[test]
fn cancelling_prefix_suffix_matrices_still_process_midi_through_the_suffix() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(4, 1));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1, 1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let utility = UTILITY_DEFINITION.default_state().unwrap();
    let inverted = UTILITY_DEFINITION
        .patch_parameter(
            &utility,
            utility_parameter::INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::InsertDevice {
            track,
            before: Some(instrument),
            state: inverted.clone(),
        })
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: inverted,
        })
        .unwrap();
    let compiled = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut actual = [[0.0; 2]; 256];
    renderer.render(&mut actual).unwrap();

    let table = SineTable::new();
    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let expected: Vec<_> = (0..actual.len())
        .map(|frame| {
            let sample = -table.sample(voice, frame as u64, 12_000);
            [sample, -sample]
        })
        .collect();
    // Prefix and suffix cancel for audio-only input, but MIDI enters between
    // them and must still see the non-identity suffix exactly once.
    assert_eq!(actual.as_slice(), expected.as_slice());
}

#[test]
fn session_inventory_admits_non_front_instrument_and_keeps_slot_validation() {
    let (mut editor, track) = setup();
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::MoveTrackDevice {
            device: instrument,
            track,
            before: None,
        })
        .unwrap();
    assert_eq!(
        editor.project().track(track).unwrap().instrument_position(),
        Some(0)
    );
    let effect_state = UTILITY_DEFINITION.default_state().unwrap();
    let effect = match editor
        .execute(Command::InsertDevice {
            track,
            before: Some(instrument),
            state: effect_state,
        })
        .unwrap()
    {
        Created::Device(id) => id,
        _ => panic!("effect"),
    };
    editor
        .execute(Command::MoveTrackDevice {
            device: instrument,
            track,
            before: None,
        })
        .unwrap();
    assert_eq!(
        editor.project().track(track).unwrap().instrument_position(),
        Some(1)
    );
    assert!(compile::project(editor.project(), &BTreeMap::new()).is_ok());
    assert!(
        PreparedSessionInventory::prepare(editor.project(), Arc::new(SineTable::new()), None)
            .is_ok()
    );
    assert!(editor.project().device(effect).is_some());
}

#[test]
fn stateful_front_instrument_enters_the_effect_chain_once() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(4, 1));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1, 1),
            pitch: 69,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let unfiltered = editor.project().clone();
    let utility = UTILITY_DEFINITION.default_state().unwrap();
    let dc_remove = UTILITY_DEFINITION
        .patch_parameter(
            &utility,
            utility_parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: dc_remove.clone(),
        })
        .unwrap();
    assert_eq!(
        editor.project().track(track).unwrap().instrument_position(),
        Some(0)
    );

    let render = |source: &hex_project::Project, media: &BTreeMap<hex_project::AssetId, Audio>| {
        let compiled = compile::project(source, media).unwrap();
        let mut renderer = Renderer::new(&compiled);
        renderer.play();
        let mut frames = vec![[0.0; 2]; 64];
        renderer.render(&mut frames).unwrap();
        frames
    };
    let empty_media = BTreeMap::new();
    let raw_instrument = render(&unfiltered, &empty_media);
    let filtered_instrument = render(editor.project(), &empty_media);

    let mut audio_editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let audio_track = match audio_editor
        .execute(Command::AddTrack {
            name: "oracle".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("audio track"),
    };
    let asset = match audio_editor
        .execute(Command::AddAsset {
            path: "memory/front-stateful-oracle.wav".into(),
            sample_rate: 48_000,
            frames: raw_instrument.len() as u64,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("audio asset"),
    };
    audio_editor
        .execute(Command::PlaceClip {
            track: audio_track,
            asset,
            start: 0,
            duration: raw_instrument.len() as u64,
            source_offset: 0,
            source_length: raw_instrument.len() as u64,
            looping: false,
        })
        .unwrap();
    audio_editor
        .execute(Command::InsertDevice {
            track: audio_track,
            before: None,
            state: dc_remove,
        })
        .unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, raw_instrument).unwrap())]);
    let audio_oracle = render(audio_editor.project(), &media);
    for (actual, expected) in filtered_instrument.iter().zip(audio_oracle) {
        for channel in 0..2 {
            assert!((actual[channel] - expected[channel]).abs() < 1.0e-6);
        }
    }
}

#[test]
fn instrument_render_is_partition_seek_pause_and_stop_invariant() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(1, 2));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1, 2),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let project = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    assert_eq!(project.audible_end_frame(), Some(14_400));

    let mut whole_playback = Playback::new(project.clone());
    whole_playback.apply(PlaybackCommand::Play);
    let mut whole = [[0.0; 2]; 14_400];
    whole_playback.render(&mut whole).unwrap();
    assert!(whole.iter().any(|frame| frame[0].abs() > 0.05));

    let original_table = SineTable::new();
    let original_voice = original_table.prepare_voice(48_000, 69, 127).unwrap();
    let exact_original: Vec<[f32; 2]> = (0..whole.len())
        .map(|age| {
            let sample = original_table.sample(original_voice, age as u64, 12_000);
            [sample, sample]
        })
        .collect();
    assert_eq!(whole.as_slice(), exact_original.as_slice());

    let mut split_playback = Playback::new(project.clone());
    split_playback.apply(PlaybackCommand::Play);
    let mut split = [[0.0; 2]; 14_400];
    for chunk in split.chunks_mut(127) {
        split_playback.render(chunk).unwrap();
    }
    assert_eq!(whole, split);
    assert_eq!(whole_playback.position(), split_playback.position());

    let mut seek_playback = Playback::new(project);
    seek_playback.apply(PlaybackCommand::Seek(5_000));
    seek_playback.apply(PlaybackCommand::Play);
    let mut sought = [[0.0; 2]; 128];
    seek_playback.render(&mut sought).unwrap();
    assert_eq!(sought, whole[5_000..5_128]);
    seek_playback.apply(PlaybackCommand::Pause);
    let paused_at = seek_playback.position();
    sought.fill([9.0; 2]);
    seek_playback.render(&mut sought).unwrap();
    assert_eq!(sought, [[0.0; 2]; 128]);
    assert_eq!(seek_playback.position(), paused_at);
    seek_playback.apply(PlaybackCommand::Stop);
    assert_eq!(seek_playback.position(), 0);

    let mut playing_graph =
        Playback::new(compile::project(editor.project(), &BTreeMap::new()).unwrap());
    playing_graph.apply(PlaybackCommand::Seek(5_000));
    playing_graph.apply(PlaybackCommand::Play);
    let replacement_project = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let mut replacement = Playback::new(replacement_project);
    replacement.inherit_transport(&playing_graph).unwrap();
    assert_eq!(replacement.position(), 5_000);
    assert!(replacement.playing());
    let mut replacement_output = [[0.0; 2]; 128];
    replacement.render(&mut replacement_output).unwrap();
    assert_eq!(replacement_output, whole[5_000..5_128]);
}

#[test]
fn sounding_note_does_not_survive_seek_or_stop_across_partitions() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(1, 2));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1, 2),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let project = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let mut whole_playback = Playback::new(project.clone());
    whole_playback.apply(PlaybackCommand::Play);
    let mut whole = vec![[0.0; 2]; 14_400];
    whole_playback.render(&mut whole).unwrap();

    for block in [1_usize, 17, 64, 128, 512] {
        // (destination, expected PCM after the transport moves away from a held note)
        for (destination, expected) in [
            (20_000_u64, vec![[0.0; 2]; 1_024]),
            (5_000, whole[5_000..6_024].to_vec()),
        ] {
            let mut playback = Playback::new(project.clone());
            playback.apply(PlaybackCommand::Play);
            let mut held = vec![[0.0; 2]; 3_000];
            for chunk in held.chunks_mut(block) {
                playback.render(chunk).unwrap();
            }
            assert_eq!(held, whole[..3_000], "block {block}");
            playback.apply(PlaybackCommand::Seek(destination));
            let mut after = vec![[9.0; 2]; 1_024];
            for chunk in after.chunks_mut(block) {
                playback.render(chunk).unwrap();
            }
            assert_eq!(after, expected, "block {block} seek {destination}");
        }
        let mut playback = Playback::new(project.clone());
        playback.apply(PlaybackCommand::Play);
        let mut held = vec![[0.0; 2]; 3_000];
        for chunk in held.chunks_mut(block) {
            playback.render(chunk).unwrap();
        }
        playback.apply(PlaybackCommand::Stop);
        playback.apply(PlaybackCommand::Seek(20_000));
        playback.apply(PlaybackCommand::Play);
        let mut after = vec![[9.0; 2]; 1_024];
        for chunk in after.chunks_mut(block) {
            playback.render(chunk).unwrap();
        }
        assert_eq!(after, vec![[0.0; 2]; 1_024], "block {block} stop");
    }
}

#[test]
fn pause_and_resume_of_a_sounding_note_is_partition_invariant() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(1, 2));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1, 2),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let project = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let run = |block: usize| {
        let mut playback = Playback::new(project.clone());
        let render = |playback: &mut Playback, frames: usize| {
            let mut output = vec![[9.0; 2]; frames];
            for chunk in output.chunks_mut(block) {
                playback.render(chunk).unwrap();
            }
            output
        };
        playback.apply(PlaybackCommand::Play);
        let before = render(&mut playback, 3_000);
        playback.apply(PlaybackCommand::Pause);
        let paused_at = playback.position();
        let paused = render(&mut playback, 500);
        assert_eq!(playback.position(), paused_at, "block {block}");
        playback.apply(PlaybackCommand::Play);
        let resumed = render(&mut playback, 2_000);
        (before, paused, resumed, paused_at)
    };
    let expected = run(128);
    assert_eq!(expected.1, vec![[0.0; 2]; 500]);
    let mut whole_playback = Playback::new(project.clone());
    whole_playback.apply(PlaybackCommand::Play);
    let mut whole = vec![[0.0; 2]; 5_000];
    whole_playback.render(&mut whole).unwrap();
    assert_eq!(expected.0, whole[..3_000]);
    // Resume continues the same timeline position as an uninterrupted render.
    assert_eq!(expected.2, whole[3_000..]);
    for block in [1_usize, 17, 64, 512] {
        assert_eq!(run(block), expected, "block {block}");
    }
}

#[test]
fn interval_tree_keeps_earlier_notes_when_median_onset_is_in_the_future() {
    const TOTAL_FRAMES: usize = 320_000;
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(16, 1));
    let positions = [0_u64, 4, 8, 12];
    for (index, start) in positions.into_iter().enumerate() {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: beat(start, 1),
                duration: beat(1, 1),
                pitch: 48 + index as u8 * 7,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }

    let source_snapshot = editor.project().snapshot();
    let combined = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let combined_output = render_chunks(&combined, TOTAL_FRAMES, &[128, 512, 43]);
    let mut isolated_sum = vec![[0.0; 2]; TOTAL_FRAMES];
    let hex_project::TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &source_snapshot.tracks[0].content
    else {
        panic!("midi track")
    };
    for note in &clips[0].notes {
        let mut isolated_snapshot = source_snapshot.clone();
        let hex_project::TrackContentSnapshot::Hybrid {
            midi_clips: clips, ..
        } = &mut isolated_snapshot.tracks[0].content
        else {
            panic!("midi track")
        };
        clips[0].notes = vec![note.clone()];
        let isolated_edit = Project::from_snapshot(isolated_snapshot).unwrap();
        let isolated = compile::project(&isolated_edit, &BTreeMap::new()).unwrap();
        let isolated_output = render_chunks(&isolated, TOTAL_FRAMES, &[257]);
        for (sum, sample) in isolated_sum.iter_mut().zip(isolated_output) {
            sum[0] += sample[0];
            sum[1] += sample[1];
        }
    }
    assert_eq!(combined_output, isolated_sum);

    let mut seeking = Playback::new(combined);
    seeking.apply(PlaybackCommand::Seek(50_000));
    seeking.apply(PlaybackCommand::Play);
    let mut gap = [[9.0; 2]; 128];
    seeking.render(&mut gap).unwrap();
    assert_eq!(gap, [[0.0; 2]; 128]);

    seeking.apply(PlaybackCommand::Seek(96_000));
    let mut later_note = [[0.0; 2]; 128];
    seeking.render(&mut later_note).unwrap();
    assert_eq!(later_note, isolated_sum[96_000..96_128]);
    assert!(
        combined_output[0..128]
            .iter()
            .any(|frame| frame[0].abs() > 0.01)
    );
    assert!(
        combined_output[96_000..96_128]
            .iter()
            .any(|frame| frame[0].abs() > 0.01)
    );
}

fn render_chunks(
    project: &hex_engine::Project,
    frames: usize,
    block_sizes: &[usize],
) -> Vec<[f32; 2]> {
    let mut playback = Playback::new(project.clone());
    playback.apply(PlaybackCommand::Play);
    let mut output = vec![[0.0; 2]; frames];
    let mut offset = 0;
    let mut block_index = 0;
    while offset < frames {
        let block_size = block_sizes[block_index % block_sizes.len()];
        let count = block_size.min(frames - offset);
        playback
            .render(&mut output[offset..offset + count])
            .unwrap();
        offset += count;
        block_index += 1;
    }
    output
}

#[test]
fn adjacent_same_pitch_gates_keep_distinct_release_tails_and_count_toward_voice_cap() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(4, 1));
    let frame_beat = beat(1, 24_000).raw();
    for index in 0..64 {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::from_raw(index * 32 * frame_beat),
                duration: BeatTime::from_raw(32 * frame_beat),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let project = compile::project(editor.project(), &BTreeMap::new()).expect("64 voices accepted");
    let mut playback = Playback::new(project);
    playback.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 4096];
    playback.render(&mut output).unwrap();
    assert!(output.iter().any(|frame| frame[0].abs() > 0.001));
    // Authored gates touch but never overlap. Their 50 ms releases overlap as
    // 64 independent occurrences; retriggering must not steal an older tail.
    let table = SineTable::new();
    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    for (frame, actual) in output.iter().enumerate() {
        let expected: f32 = (0..64)
            .filter_map(|index| frame.checked_sub(index * 32))
            .map(|age| table.sample(voice, age as u64, 32))
            .sum();
        assert!(actual.iter().all(|sample| (sample - expected).abs() < 1e-6));
    }

    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::from_raw(64 * 32 * frame_beat),
            duration: BeatTime::from_raw(32 * frame_beat),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    assert_eq!(
        compile::project(editor.project(), &BTreeMap::new()).unwrap_err(),
        Invalid("midi voice density exceeds 64 simultaneous voices per track")
    );
}

#[test]
fn empty_midi_clip_has_no_edges_and_event_capacity_is_two_per_note() {
    assert_eq!(MAX_MIDI_EVENT_EDGES, 131_072);
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(4, 1));
    assert!(midi_events(editor.project()).unwrap().is_empty());
    assert!(editor.project().midi_clip(clip).unwrap().notes().is_empty());
    let compiled = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    assert_eq!(compiled.audible_end_frame(), Some(96_000));
}

#[test]
fn compile_refuses_pitch_at_or_above_project_nyquist() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(1, 1));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1, 2),
            pitch: 127,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    assert_eq!(
        compile::project(editor.project(), &BTreeMap::new()).unwrap_err(),
        Invalid("midi note cannot be prepared by its instrument")
    );
}

#[test]
fn removed_instrument_is_explicit_silence_and_unknown_bypassed_instrument_refuses_compile() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(4, 1));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let compiled = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = [[1.0, -1.0]; 8];
    renderer.render(&mut output).unwrap();
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));

    let device = match editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_lattice_contract::NativeDeviceState::new(
                hex_lattice_contract::DeviceTypeId::new("future.instrument").unwrap(),
                19,
                vec![1, 2, 3],
            )
            .unwrap(),
        })
        .unwrap()
    {
        Created::Device(device) => device,
        _ => panic!("instrument device"),
    };
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    assert_eq!(
        compile::project(editor.project(), &BTreeMap::new()).unwrap_err(),
        Invalid("track instrument state is unsupported")
    );
}

#[test]
fn absent_and_bypassed_instruments_keep_midi_extent_and_preview() {
    for hybrid_track in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let track = match editor
            .execute(if hybrid_track {
                Command::AddTrack {
                    name: "hybrid".into(),
                }
            } else {
                Command::AddMidiTrack {
                    name: "midi".into(),
                }
            })
            .unwrap()
        {
            Created::Track(track) => track,
            _ => panic!("track"),
        };
        editor
            .execute(Command::InsertInstrument {
                track,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let clip = add_clip(&mut editor, track, beat(1, 4), beat(3, 4));
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: beat(1, 8),
                duration: beat(1, 4),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();

        let (audible, audible_preview) =
            compile::project_with_midi_preview(editor.project(), &BTreeMap::new()).unwrap();
        let expected_end = audible.audible_end_frame();
        let expected_preview = audible_preview.get(clip).unwrap().clone();
        if hybrid_track {
            let device = editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            editor
                .execute(Command::SetDeviceBypass {
                    device,
                    bypassed: true,
                })
                .unwrap();
        } else {
            editor.execute(Command::RemoveInstrument { track }).unwrap();
        }

        let (silent, silent_preview) =
            compile::project_with_midi_preview(editor.project(), &BTreeMap::new()).unwrap();
        assert_eq!(silent.audible_end_frame(), expected_end);
        assert_eq!(silent_preview.get(clip), Some(&expected_preview));
        let mut renderer = Renderer::new(&silent);
        renderer.play();
        let mut output = [[1.0, -1.0]; 64];
        renderer.render(&mut output).unwrap();
        assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    }
}

#[test]
fn maximum_valid_project_compiles_exactly_the_bounded_edge_count() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(4, 1));
    let mut snapshot = editor.project().snapshot();
    let first_note = snapshot.next_id;
    let hex_project::TrackSnapshot { content, .. } = &mut snapshot.tracks[0];
    let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = content
    else {
        panic!("instrument snapshot")
    };
    let frame_beat = beat(1, 24_000).raw();
    clips[0].notes = (0..hex_project::MAX_NOTES)
        .map(|index| MidiNoteSnapshot {
            active: true,
            id: first_note + index as u64,
            start_raw: index as u64 * frame_beat,
            duration_raw: frame_beat,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .collect();
    snapshot.next_id = first_note + hex_project::MAX_NOTES as u64;
    let project = Project::from_snapshot(snapshot).expect("maximum valid note count");
    let events = midi_events(&project).expect("exact maximum event count");
    assert_eq!(events.len(), MAX_MIDI_EVENT_EDGES);
    for (index, pair) in events.chunks_exact(2).enumerate() {
        assert_eq!(pair[0].kind, MidiEventKind::NoteOn);
        assert_eq!(pair[0].frame, index as u64);
        assert_eq!(pair[1].kind, MidiEventKind::NoteOff);
        assert_eq!(pair[1].frame, index as u64 + 1);
        assert_eq!(pair[0].note.value(), first_note + index as u64);
        assert_eq!(pair[1].note, pair[0].note);
    }
    assert_eq!(clip, project.tracks()[0].midi_clips()[0].id());
}

#[test]
fn frame_range_copy_never_creates_zero_frame_midi_remainders() {
    for (rate, bpm, tail, leading_silence, reversed) in [
        (48_000, 120_000_000, false, true, false),
        (48_000, 120_000_000, true, true, false),
        (48_000, 143_000_000, false, true, false),
        (48_000, 143_000_000, false, true, true),
        (48_000, 143_000_000, false, false, false),
        (48_000, 143_000_000, true, true, false),
        (44_100, 143_000_000, false, true, false),
        (44_100, 143_000_000, true, true, false),
    ] {
        let (mut editor, track) = setup_at_rate(rate);
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(bpm).unwrap(),
            })
            .unwrap();
        let one = editor
            .project()
            .tempo_map()
            .beat_to_frame(BeatTime::ONE)
            .unwrap();
        let two = editor
            .project()
            .tempo_map()
            .beat_to_frame(beat(2, 1))
            .unwrap();
        let (note_start, selection_end, destination, remaining) = if reversed {
            let onset = editor
                .project()
                .tempo_map()
                .beat_to_frame(beat(3, 2))
                .unwrap();
            (
                beat(5, 2),
                two,
                two,
                vec![onset, two, two + onset, two + two],
            )
        } else if tail {
            (
                BeatTime::ZERO,
                one / 2,
                two - one / 2,
                vec![one, two - one / 2],
            )
        } else {
            (BeatTime::ONE, two, two, vec![])
        };
        let selection_start = if leading_silence { 0 } else { one };
        let clip = add_clip(&mut editor, track, BeatTime::ONE, beat(4, 1));
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: note_start,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        if reversed || (tail && rate == 44_100) {
            editor
                .execute(Command::ReplaceMidiClipVolume {
                    clip,
                    volume: MidiClipVolume::new(
                        vec![
                            MidiClipVolumePoint::linear(
                                BeatTime::ZERO,
                                MidiClipGain::new(0.25).unwrap(),
                            ),
                            MidiClipVolumePoint::linear(
                                beat(4, 1),
                                MidiClipGain::new(0.75).unwrap(),
                            ),
                        ],
                        beat(4, 1),
                    )
                    .unwrap(),
                })
                .unwrap();
        }
        if reversed {
            editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        }
        let mut before = editor.project().snapshot();
        let source_notes = editor.project().midi_clip(clip).unwrap().notes().to_vec();
        let source_volume = editor.project().midi_clip(clip).unwrap().volume().clone();
        let excerpt = reversed.then(|| {
            let frame = remaining[0] + 128;
            let output = rendered_excerpt(editor.project(), frame);
            assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-5));
            (frame, output)
        });
        assert_eq!(midi_events(editor.project()).unwrap().len(), 2);
        editor
            .execute(Command::CopyTimeRangeTo {
                range: hex_project::TimeRange::new(vec![track], selection_start, selection_end)
                    .unwrap(),
                track,
                frame: destination,
            })
            .unwrap();
        let events = midi_events(editor.project())
            .expect("frame-aligned overwrite must not invent an unplayable note gate");
        assert_eq!(
            events.iter().map(|event| event.frame).collect::<Vec<_>>(),
            remaining
        );
        compile::project(editor.project(), &BTreeMap::new()).unwrap();
        assert_eq!(
            editor.project().midi_clip(clip).unwrap().notes(),
            source_notes
        );
        assert_eq!(
            editor.project().midi_clip(clip).unwrap().volume(),
            &source_volume
        );
        if let Some((frame, expected)) = excerpt {
            let actual = rendered_excerpt(editor.project(), frame);
            for (actual, expected) in actual.iter().flatten().zip(expected.iter().flatten()) {
                assert!(
                    (actual - expected).abs() <= 1e-6,
                    "a prefix crop must preserve audible phase and authored volume before its cut"
                );
            }
        }
        let after = editor.project().snapshot();
        // Undo restores complete musical state while the identity allocator
        // retains its documented high-water mark to prevent ID reuse.
        assert!(after.next_id >= before.next_id);
        before.next_id = after.next_id;
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        assert_eq!(midi_events(editor.project()).unwrap(), events);
    }
}

#[test]
fn frame_range_copy_keeps_a_touching_midi_source_exact() {
    let (mut editor, track) = setup();
    let clip = add_clip(
        &mut editor,
        track,
        BeatTime::ZERO,
        BeatTime::from_raw(BeatTime::ONE.raw() + 1),
    );
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let original = editor.project().midi_clip(clip).unwrap().clone();
    let events = midi_events(editor.project()).unwrap();
    editor
        .execute(Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], 48_000, 72_000).unwrap(),
            track,
            frame: 24_000,
        })
        .unwrap();
    assert_eq!(
        editor.project().midi_clip(clip).unwrap(),
        &original,
        "a clip ending in the cut's first frame has no frame overlap"
    );
    assert_eq!(midi_events(editor.project()).unwrap(), events);
}

#[test]
fn arrangement_split_preserves_playability_of_a_subframe_carry() {
    for (rate, bpm) in [
        (44_100, 120_000_000),
        (48_000, 120_000_000),
        (44_100, 137_000_000),
        (48_000, 137_000_000),
    ] {
        let (mut editor, track) = setup_at_rate(rate);
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(bpm).unwrap(),
            })
            .unwrap();
        let frame = editor
            .project()
            .tempo_map()
            .beat_to_frame(BeatTime::ONE)
            .unwrap();
        let cut = editor.project().tempo_map().frame_to_beat(frame).unwrap();
        let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(2, 1));
        for (start, duration, pitch) in [
            (BeatTime::ZERO, BeatTime::from_raw(cut.raw() + 1), 60),
            (BeatTime::from_raw(cut.raw() + 100), beat(1, 64), 64),
        ] {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start,
                    duration,
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        let edges = |project: &Project| {
            midi_events(project)
                .unwrap()
                .into_iter()
                .map(|event| {
                    (
                        event.frame,
                        event.kind,
                        event.pitch,
                        event.channel,
                        event.velocity,
                    )
                })
                .collect::<Vec<_>>()
        };
        let before = edges(editor.project());
        editor
            .execute(Command::SplitTracksAtFrame {
                tracks: vec![track],
                frame,
            })
            .unwrap();
        assert_eq!(
            edges(editor.project()),
            before,
            "the subframe carry adds no edges at {rate} Hz and {bpm} microbpm"
        );
        let after = editor.project().snapshot();
        assert!(editor.undo());
        assert_eq!(edges(editor.project()), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        assert_eq!(edges(editor.project()), before);
    }
}

#[test]
fn subframe_carry_fallback_keeps_other_crossing_gates_balanced() {
    let (mut editor, track) = setup();
    let clip = add_clip(&mut editor, track, BeatTime::ZERO, beat(2, 1));
    for (start, duration, pitch) in [
        (
            BeatTime::ZERO,
            BeatTime::from_raw(BeatTime::ONE.raw() + 1),
            60,
        ),
        (beat(3, 4), beat(1, 2), 70),
    ] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration,
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let frame = editor
        .project()
        .tempo_map()
        .beat_to_frame(BeatTime::ONE)
        .unwrap();
    editor
        .execute(Command::SplitTracksAtFrame {
            tracks: vec![track],
            frame,
        })
        .unwrap();
    let edges: Vec<_> = midi_events(editor.project())
        .unwrap()
        .into_iter()
        .filter(|event| event.pitch == 70)
        .map(|event| (event.frame, event.kind))
        .collect();
    assert_eq!(edges.len(), 4);
    assert_eq!(edges[0].1, MidiEventKind::NoteOn);
    assert_eq!(edges[1], (frame, MidiEventKind::NoteOff));
    assert_eq!(edges[2], (frame, MidiEventKind::NoteOn));
    assert_eq!(edges[3].1, MidiEventKind::NoteOff);
    assert!(edges[0].0 < frame && edges[3].0 > frame);
}
