use hex_project::{
    AssetSnapshot, ClipSnapshot, Command, Created, Editor, MAX_ASSETS, MAX_CLIPS, MAX_TRACKS,
    Project, Snapshot, TrackContentSnapshot, TrackSnapshot,
};
use std::path::PathBuf;

fn valid_snapshot() -> Snapshot {
    Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: 100,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: 10,
            path: PathBuf::from("Media/Source.WAV"),
            source_sample_rate: 48_000,
            source_frames: 512,
            playback_frames: 512,
            conversion_version: 0,
        }],
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![hex_project::TempoPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Tempo::DEFAULT,
        )],
        meter_points: vec![hex_project::MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: 20,
            name: "Track Case".into(),
            gain: 0.75,
            balance: 0.0,
            muted: true,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Hybrid {
                audio_clips: vec![
                    ClipSnapshot {
                        active: true,
                        name: "clip".into(),
                        flavor: hex_project::ClipFlavor::default(),
                        edit: None,
                        gain: hex_project::ClipGain::UNITY,
                        id: 40,
                        asset: 10,
                        start: 1_024,
                        duration: 80,
                        source_offset: 64,
                        source_length: 32,
                        looping: true,
                        source_phase: 0,
                        fade: hex_project::ClipFade::neutral(80).unwrap(),
                    },
                    ClipSnapshot {
                        active: true,
                        name: "clip".into(),
                        flavor: hex_project::ClipFlavor::default(),
                        edit: None,
                        gain: hex_project::ClipGain::UNITY,
                        id: 30,
                        asset: 10,
                        start: 0,
                        duration: 512,
                        source_offset: 0,
                        source_length: 512,
                        looping: false,
                        source_phase: 0,
                        fade: hex_project::ClipFade::neutral(512).unwrap(),
                    },
                ],
                midi_clips: Vec::new(),
            },
            sends: Vec::new(),
        }],
        return_tracks: Vec::new(),
        session: Default::default(),
    }
}

fn rejects(change: impl FnOnce(&mut Snapshot)) {
    let mut snapshot = valid_snapshot();
    change(&mut snapshot);
    assert!(Project::from_snapshot(snapshot).is_err());
}

#[test]
fn snapshot_round_trip_preserves_order_case_ids_and_next_id() {
    let snapshot = valid_snapshot();
    let project = Project::from_snapshot(snapshot.clone()).expect("valid snapshot");
    assert_eq!(project.snapshot(), snapshot);
    assert_eq!(project.tracks()[0].name(), "Track Case");
    assert_eq!(project.tracks()[0].clips()[0].id().value(), 40);
    assert_eq!(project.tracks()[0].clips()[1].id().value(), 30);
    assert_eq!(
        project.assets().next().expect("asset").path(),
        PathBuf::from("Media/Source.WAV")
    );

    let mut editor = Editor::new(project, 4).expect("editor");
    assert!(matches!(
        editor.execute(Command::AddTrack {
            name: "after load".into(),
        }),
        Ok(Created::Track(id)) if id.value() == 100
    ));
}

#[test]
fn rejects_invalid_rates_ids_and_high_water_marks() {
    rejects(|snapshot| snapshot.sample_rate = 0);
    rejects(|snapshot| snapshot.next_id = 0);
    rejects(|snapshot| snapshot.next_id = 40);
    rejects(|snapshot| snapshot.assets[0].id = 0);
    rejects(|snapshot| snapshot.tracks[0].id = 10);
    rejects(|snapshot| snapshot.tracks[0].audio_clips_mut().unwrap()[1].id = 40);
}

#[test]
fn instrument_position_requires_a_present_slot_and_valid_effect_gap() {
    rejects(|snapshot| snapshot.tracks[0].instrument_position = Some(0));

    let mut missing_position = valid_snapshot();
    missing_position.tracks[0].instrument = Some(hex_project::DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: 50,
        bypassed: false,
        state: hex_project::default_instrument_state().unwrap(),
    });
    assert!(Project::from_snapshot(missing_position).is_err());

    let mut out_of_range = valid_snapshot();
    out_of_range.tracks[0].instrument = Some(hex_project::DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: 50,
        bypassed: false,
        state: hex_project::default_instrument_state().unwrap(),
    });
    out_of_range.tracks[0].instrument_position = Some(1);
    assert!(Project::from_snapshot(out_of_range).is_err());

    let mut valid = valid_snapshot();
    valid.tracks[0].instrument = Some(hex_project::DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: 50,
        bypassed: false,
        state: hex_project::default_instrument_state().unwrap(),
    });
    valid.tracks[0].instrument_position = Some(0);
    valid
        .native_sources
        .push(hex_project::NativeSourceSnapshot {
            id: 51,
            name: "source".into(),
            devices: vec![valid.tracks[0].instrument.clone().unwrap()],
            instrument_index: 0,
            receiver: Some(valid.tracks[0].id),
        });
    assert_eq!(
        Project::from_snapshot(valid).unwrap().tracks()[0].instrument_position(),
        Some(0)
    );
}

#[test]
fn rejects_invalid_assets_tracks_and_clip_ranges() {
    rejects(|snapshot| snapshot.assets[0].path = PathBuf::new());
    rejects(|snapshot| snapshot.assets[0].playback_frames = 0);
    rejects(|snapshot| snapshot.tracks[0].name = " \t".into());
    rejects(|snapshot| snapshot.tracks[0].name = "bad\nname".into());
    rejects(|snapshot| snapshot.tracks[0].gain = f32::NAN);
    rejects(|snapshot| snapshot.tracks[0].gain = 2.01);
    rejects(|snapshot| snapshot.tracks[0].audio_clips_mut().unwrap()[0].asset = 999);
    rejects(|snapshot| snapshot.tracks[0].audio_clips_mut().unwrap()[0].duration = 0);
    rejects(|snapshot| snapshot.tracks[0].audio_clips_mut().unwrap()[0].start = u64::MAX);
    rejects(|snapshot| snapshot.tracks[0].audio_clips_mut().unwrap()[0].source_offset = u64::MAX);
    rejects(|snapshot| snapshot.tracks[0].audio_clips_mut().unwrap()[0].source_length = 0);
    rejects(|snapshot| snapshot.tracks[0].audio_clips_mut().unwrap()[0].source_length = 513);
}

#[test]
fn rejects_track_asset_and_clip_limit_overruns() {
    let mut tracks = valid_snapshot();
    tracks.tracks = (0..=MAX_TRACKS)
        .map(|index| TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: 1_000 + index as u64,
            name: format!("track {index}"),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(Vec::new()),
            sends: Vec::new(),
        })
        .collect();
    tracks.next_id = 1_000 + tracks.tracks.len() as u64;
    assert!(Project::from_snapshot(tracks).is_err());

    let mut assets = valid_snapshot();
    assets.assets = (0..=MAX_ASSETS)
        .map(|index| AssetSnapshot {
            original_source: None,
            id: 1_000 + index as u64,
            path: PathBuf::from(format!("Samples/{index}.wav")),
            source_sample_rate: 48_000,
            source_frames: 1,
            playback_frames: 1,
            conversion_version: 0,
        })
        .collect();
    assets.tracks.clear();
    assets.next_id = 1_000 + assets.assets.len() as u64;
    assert!(Project::from_snapshot(assets).is_err());

    let mut clips = valid_snapshot();
    let track = &mut clips.tracks[0];
    *track.audio_clips_mut().unwrap() = (0..=MAX_CLIPS)
        .map(|index| ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id: 1_000 + index as u64,
            asset: 10,
            start: index as u64,
            duration: 1,
            source_offset: 0,
            source_length: 1,
            looping: false,
            source_phase: 0,
            fade: hex_project::ClipFade::neutral(1).unwrap(),
        })
        .collect();
    clips.next_id = 1_000 + MAX_CLIPS as u64 + 2;
    assert!(Project::from_snapshot(clips).is_err());
}
