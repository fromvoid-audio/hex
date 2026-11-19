//! Clip-clipboard paste and insert events carry the clips they created.
use super::service_tests::next_event;
use super::*;
use hex_project::BeatTime;

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

#[test]
fn unread_paste_observation_retains_the_newest_contiguous_state() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "track".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let placed = run(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }),
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("placed clip")
    };
    run(&service, Request::CopyClips(vec![clip]));
    let first = run(
        &service,
        Request::PasteTimeRange {
            track,
            frame: 48_000,
        },
    );
    let second = run(
        &service,
        Request::PasteTimeRange {
            track,
            frame: 96_000,
        },
    );
    let observation = service.try_arrangement_edit_view().unwrap();
    let observed = observation.state.upgrade().unwrap();
    assert_eq!(
        observation.first_previous_revision + 1,
        first.state.revision
    );
    assert_eq!(observation.previous_revision, first.state.revision);
    assert_eq!(observed.revision, second.state.revision);
    assert_eq!(observed.project.track(track).unwrap().midi_clips().len(), 3);
    assert_eq!(first.state.revision + 1, second.state.revision);
}

#[test]
fn clip_paste_and_insert_events_report_created_clips() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "track".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let mut clips = Vec::new();
    for start in [0, 1] {
        let placed = run(
            &service,
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(start).unwrap(),
                length: BeatTime::ONE,
            }),
        );
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("placed clip")
        };
        clips.push(clip);
    }
    run(&service, Request::CopyClips(clips.clone()));
    let pasted = run(
        &service,
        Request::PasteTimeRange {
            track,
            frame: 480_000,
        },
    );
    let Some(Created::Clips { first, last }) = pasted.created else {
        panic!("paste reports clips: {:?}", pasted.created)
    };
    assert!(first > clips[1] && last > first);
    let midi = pasted.state.project.track(track).unwrap().midi_clips();
    assert!(midi.iter().any(|c| c.id() == first));
    assert!(midi.iter().any(|c| c.id() == last));

    run(&service, Request::CopyClips(vec![clips[0]]));
    let inserted = run(
        &service,
        Request::InsertTimeRange {
            track,
            frame: 960_000,
        },
    );
    let Some(Created::Clips { first: clip, last }) = inserted.created else {
        panic!("insert reports one clip: {:?}", inserted.created)
    };
    assert_eq!(clip, last);
    assert!(
        inserted
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .iter()
            .any(|c| c.id() == clip)
    );
}
