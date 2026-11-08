//! Shared project fixture for section-focus tests.
use hex_project::{BeatTime, ClipId, Command, Created, Editor, Project, SectionId, TrackId};

pub(super) struct Fixture {
    pub(super) model: Editor,
    pub(super) tracks: Vec<TrackId>,
    pub(super) clips: Vec<ClipId>,
    #[allow(dead_code)]
    pub(super) asset: hex_project::AssetId,
    pub(super) sections: Vec<SectionId>,
}
pub(super) fn model() -> Fixture {
    let mut model = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let add = |model: &mut Editor, name: &str| {
        let Created::Track(id) = model
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        id
    };
    let empty = add(&mut model, "empty");
    let Created::Imported {
        track: audio,
        clip,
        asset,
    } = model
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "focus.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    let tracks = vec![
        empty,
        audio,
        add(&mut model, "outside"),
        add(&mut model, "inside"),
        add(&mut model, "late"),
    ];
    let Created::Clip(outside_audio) = model.execute(Command::DuplicateClip(clip)).unwrap() else {
        panic!("duplicate audio")
    };
    model
        .execute(Command::MoveClip {
            clip: outside_audio,
            track: tracks[1],
            start: 192_000,
        })
        .unwrap();
    let mut clips = vec![clip];
    for (track, start, length) in [
        (tracks[2], 0, 2),
        (tracks[3], 2, 1),
        (tracks[4], 3, 1),
        (tracks[1], 6, 1),
    ] {
        let Created::Clip(id) = model
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(start).unwrap(),
                length: BeatTime::from_quarters(length).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi")
        };
        clips.push(id);
    }
    clips.push(outside_audio);
    let mut sections = Vec::new();
    for (start, end) in [(2, 3), (4, 5), (6, 7)] {
        let Created::ArrangementSection(id) = model
            .execute(Command::AddArrangementSection {
                name: format!("section {}", sections.len()),
                color: hex_project::ElementColor::Cyan,
                start: BeatTime::from_quarters(start).unwrap(),
                end: BeatTime::from_quarters(end).unwrap(),
            })
            .unwrap()
        else {
            panic!("section")
        };
        sections.push(id);
    }
    model
        .execute(Command::SetLoopRegion {
            start: 48_000,
            end: 72_000,
            enabled: false,
        })
        .unwrap();
    Fixture {
        model,
        tracks,
        clips,
        asset,
        sections,
    }
}
