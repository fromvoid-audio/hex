//! Alias source ownership through real prepared publication and callback retirement.
use super::*;
use hex_project::{BeatTime, Command, Created, Editor, Project, TrackDeviceSlot};

fn measured(callback: &mut Callback, output: &mut [Frame]) {
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
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
}

#[test]
fn alias_edit_unique_removal_and_shutdown_do_not_reclaim_sources_in_callback() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "aliases".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
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
    let Created::Clips { first: alias, .. } = editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap()
    else {
        panic!("alias")
    };
    let (mut control, mut callback) = channel(8_000).unwrap();
    let mut output = [[0.0; 2]; 127];
    for edit in [
        None,
        Some(Command::AddMidiNote {
            clip: alias,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 90,
        }),
        Some(Command::MakeClipsUnique(vec![alias])),
        Some(Command::RemoveMidiClip(clip)),
    ] {
        if let Some(edit) = edit {
            editor.execute(edit).unwrap();
        }
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        control
            .publish(Playback::try_from_prepared_with_session(revision).unwrap())
            .unwrap();
        measured(&mut callback, &mut output);
        // Exercise the surviving alias itself, including held-note chase.
        control.command(PlaybackCommand::PlayFrom(4_000)).unwrap();
        for _ in 0..3 {
            measured(&mut callback, &mut output);
        }
        assert!(output.iter().flatten().any(|sample| sample.abs() > 0.01));
        control.reclaim();
    }
    control.command(PlaybackCommand::Stop).unwrap();
    // The queued Stop fades over 128 frames: one block and the first frame
    // of the next, which is the fade's final zero (#4151).
    measured(&mut callback, &mut output);
    measured(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 127]);
    control.request_shutdown();
    measured(&mut callback, &mut output);
    control.reclaim();
}

#[test]
fn alias_source_edit_undo_redo_reaches_active_callback_without_allocations() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "aliases".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
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
    let Created::Clips { first: alias, .. } = editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap()
    else {
        panic!("alias")
    };
    let before = editor.project().snapshot();
    let (mut control, mut callback) = channel(8_000).unwrap();
    let mut output = [[0.0; 2]; 127];
    let publish = |editor: &Editor,
                   control: &mut Control,
                   callback: &mut Callback,
                   output: &mut [Frame; 127]| {
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let playing = control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire);
        let position = control
            .status
            .position
            .load(std::sync::atomic::Ordering::Acquire);
        let revision = control
            .publish(Playback::try_from_prepared_with_session(revision).unwrap())
            .unwrap();
        measured(callback, output);
        assert_eq!(
            control
                .status
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            revision
        );
        if playing {
            assert_eq!(
                control
                    .status
                    .position
                    .load(std::sync::atomic::Ordering::Acquire),
                position + 127
            );
            assert!(output.iter().flatten().any(|sample| sample.abs() > 0.01));
        }
        control.command(PlaybackCommand::Stop).unwrap();
        measured(callback, output);
        control.command(PlaybackCommand::PlayFrom(4_000)).unwrap();
        measured(callback, output);
        control.reclaim();
        *output
    };
    // An independently authored, non-alias clip is the reference for each
    // source state; it does not use the alias or history transformation.
    let reference = |with_added_note: bool| {
        let mut reference = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
        let Created::Track(track) = reference
            .execute(Command::InsertTrackWithDevice {
                name: "reference".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("reference track")
        };
        for start in [BeatTime::ZERO, BeatTime::ONE] {
            let Created::Clip(clip) = reference
                .execute(Command::PlaceMidiClip {
                    track,
                    start,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("reference clip")
            };
            for (pitch, velocity) in
                [(60, 100), (64, 90)]
                    .into_iter()
                    .take(if with_added_note { 2 } else { 1 })
            {
                reference
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity,
                    })
                    .unwrap();
            }
        }
        let (mut control, mut callback) = channel(8_000).unwrap();
        let mut output = [[0.0; 2]; 127];
        let result = publish(&reference, &mut control, &mut callback, &mut output);
        control.request_shutdown();
        measured(&mut callback, &mut output);
        control.reclaim();
        result
    };
    let expected_original = reference(false);
    let expected_edited = reference(true);
    let original = publish(&editor, &mut control, &mut callback, &mut output);
    assert_eq!(original, expected_original);
    editor
        .execute(Command::AddMidiNote {
            clip: alias,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    let changed = editor.project().snapshot();
    let edited = publish(&editor, &mut control, &mut callback, &mut output);
    assert_eq!(edited, expected_edited);
    assert_ne!(
        edited, original,
        "shared source edit must change audible alias PCM"
    );
    assert!(editor.undo());
    let undone = editor.project().snapshot();
    assert_eq!(undone.aliases, before.aliases);
    assert_eq!(undone.tracks, before.tracks);
    assert_eq!(
        publish(&editor, &mut control, &mut callback, &mut output),
        original
    );
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), changed);
    assert_eq!(
        publish(&editor, &mut control, &mut callback, &mut output),
        edited
    );
    control.command(PlaybackCommand::Stop).unwrap();
    // The 128-frame queued Stop fade spans one 127-frame block (#4151).
    measured(&mut callback, &mut output);
    measured(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 127]);
    control.request_shutdown();
    measured(&mut callback, &mut output);
    control.reclaim();
}
