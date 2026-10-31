use hex_engine::{MidiArrangementPreview, PreparedRevision, midi_clip_preview};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

fn b(quarters: u64) -> BeatTime {
    BeatTime::from_quarters(quarters).unwrap()
}

#[test]
fn display_only_clip_summaries_equal_compiled_previews_and_refuse_duplicates() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "preview".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let mut clips = Vec::new();
    for (start, active) in [(0, true), (40, false)] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: b(start),
                length: b(32),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        // A looped source sounds more occurrences than the sampled preview keeps.
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: b(2),
                source_start: b(0),
                loop_start: b(0),
                loop_end: b(2),
                looping: true,
                length: b(32),
            })
            .unwrap();
        for pitch in [48, 60, 67] {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::from_ratio(pitch as u64 % 3, 2).unwrap(),
                    duration: BeatTime::from_ratio(1, 2).unwrap(),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        editor
            .execute(Command::SetClipActive { clip, active })
            .unwrap();
        clips.push(clip);
    }
    let project = editor.project();
    let prepared = PreparedRevision::prepare(
        project,
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
    )
    .unwrap();
    let mut summaries = Vec::new();
    for clip in project.tracks()[0].midi_clips() {
        let summary = midi_clip_preview(project, clip).unwrap();
        assert!(summary.occurrence_count > summary.samples.len());
        assert_eq!(Some(&summary), prepared.midi_preview().get(clip.id()));
        summaries.push(summary);
    }
    assert_eq!(summaries.len(), clips.len());
    summaries.reverse();
    let index = MidiArrangementPreview::from_clips(summaries.clone()).unwrap();
    assert_eq!(index.clips(), prepared.midi_preview().clips());
    summaries.push(summaries[0].clone());
    assert!(MidiArrangementPreview::from_clips(summaries).is_err());
}
