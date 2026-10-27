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
    measured(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 127]);
    control.request_shutdown();
    measured(&mut callback, &mut output);
    control.reclaim();
}
