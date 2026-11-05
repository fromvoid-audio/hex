use super::*;
use hex_lattice_contract::DeviceTypeId;
use hex_project::{BounceAdoption, TimeRange, TrackSelectionIds};

fn group(session: &mut Session, tracks: &[TrackId], name: &str) -> TrackId {
    let Created::Track(id) = session
        .command(Command::GroupTracks {
            tracks: TrackSelectionIds::new(tracks).unwrap(),
            name: name.into(),
        })
        .unwrap()
    else {
        panic!("group");
    };
    id
}

fn audio_track(session: &mut Session, folder: &Folder, name: &str, frame: [f32; 2]) -> TrackId {
    let Created::Track(track) = session
        .command(Command::InsertTrack {
            name: name.into(),
            before_track: None,
        })
        .unwrap()
    else {
        panic!("audio track");
    };
    let path = folder.0.join(format!("{name}.wav"));
    grouping::write_fixture(&path, 8_000, &[frame; 192]);
    let decoded = hex_media::decode_wav_file(&path, hex_media::Limits::new(16_000, 1_000)).unwrap();
    session
        .import_prepared_audio_clip_revision(
            PreparedImport::new(decoded).unwrap(),
            track,
            4_000,
            None,
        )
        .unwrap();
    track
}

fn overdrive() -> hex_lattice_contract::NativeDeviceState {
    hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.overdrive"),
    )
    .unwrap()
}

fn aligned_tap(session: &Session, frames: usize) -> Vec<hex_engine::Frame> {
    let revision = session.compile_prepared_revision(None).unwrap();
    let latency = revision.arrangement().main_latency_frames() as usize;
    pcm(revision, frames + latency).split_off(latency)
}

#[test]
fn group_range_prints_the_nested_post_device_submix_and_adopts_a_sibling() {
    let folder = Folder::new();
    let (mut session, midi, _) = fixture_at_rate(8_000);
    let a = audio_track(&mut session, &folder, "first", [0.35, -0.27]);
    let b = audio_track(&mut session, &folder, "second", [0.26, 0.5]);
    let muted = audio_track(&mut session, &folder, "muted", [0.85, 0.9]);
    session
        .command(Command::Gain {
            track: a,
            gain: 0.7,
        })
        .unwrap();
    session
        .command(Command::Balance {
            track: b,
            balance: -0.35,
        })
        .unwrap();
    session
        .command(Command::Mute {
            track: muted,
            muted: true,
        })
        .unwrap();
    let inner = group(&mut session, &[midi, a], "inner");
    session
        .command(Command::Gain {
            track: inner,
            gain: 0.6,
        })
        .unwrap();
    session
        .command(Command::InsertDevice {
            track: inner,
            before: None,
            state: overdrive(),
        })
        .unwrap();
    let selected = group(&mut session, &[inner, b, muted], "selected");
    let Created::Device(effect) = session
        .command(Command::InsertDevice {
            track: selected,
            before: None,
            state: overdrive(),
        })
        .unwrap()
    else {
        panic!("effect");
    };
    let start = 4_022;
    let end = 4_320;
    // With the selected group as the only root, a unity mixer and an empty
    // Main expose its normal post-device tap without using bounce isolation.
    let reference = aligned_tap(&session, end);
    assert!(
        reference[start..end]
            .iter()
            .flatten()
            .any(|s| s.abs() > 0.01)
    );
    session
        .command(Command::SetDeviceBypass {
            device: effect,
            bypassed: true,
        })
        .unwrap();
    let bypassed = aligned_tap(&session, end);
    assert!(
        reference[start..end]
            .iter()
            .zip(&bypassed[start..end])
            .any(|(a, b)| (a[0] - b[0]).abs() > 0.01)
    );
    assert!(session.undo());

    // These paths change the normal Main mix but lie outside the printed tap.
    let unrelated = audio_track(&mut session, &folder, "unrelated", [0.75, -0.65]);
    let ancestor = group(&mut session, &[selected, unrelated], "ancestor");
    session
        .command(Command::InsertDevice {
            track: ancestor,
            before: None,
            state: overdrive(),
        })
        .unwrap();
    session
        .command(Command::InsertMainDevice {
            before: None,
            state: overdrive(),
        })
        .unwrap();
    session.command(Command::MasterGain { gain: 0.4 }).unwrap();
    session
        .command(Command::Gain {
            track: selected,
            gain: 0.55,
        })
        .unwrap();
    let Created::ReturnTrack(bus) = session
        .command(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return");
    };
    session
        .command(Command::SetReturnSend {
            track: a,
            return_track: bus,
            gain: 0.8,
        })
        .unwrap();
    session
        .command(Command::SetReturnSend {
            track: selected,
            return_track: bus,
            gain: 0.7,
        })
        .unwrap();

    let range = TimeRange::new(vec![selected], start as u64, end as u64).unwrap();
    let mut original = session.project().snapshot();
    let before_publish = folder.0.join("unsupported");
    assert!(
        session
            .bounce_arrangement_adopting(
                BounceTarget::Range(range.clone()),
                false,
                BounceAdoption::NewTracksMutingSources,
                &before_publish,
                &hex_export::ExportControl::default(),
                None
            )
            .unwrap_err()
            .contains("group output")
    );
    assert!(!before_publish.exists());
    assert_eq!(session.project().snapshot(), original);

    let cancelled = hex_export::ExportControl::default();
    assert!(cancelled.cancel());
    assert!(
        session
            .bounce_arrangement(
                BounceTarget::Range(range.clone()),
                false,
                &folder.0,
                &cancelled,
                None
            )
            .is_err()
    );
    assert_eq!(session.project().snapshot(), original);

    let (Created::Clips { first, last }, _) = session
        .bounce_arrangement(
            BounceTarget::Range(range),
            false,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap()
    else {
        panic!("bounce");
    };
    assert_eq!(first, last);
    let project = session.project();
    let owner_index = project
        .tracks()
        .iter()
        .position(|track| track.clips().iter().any(|clip| clip.id() == first))
        .unwrap();
    let owner = &project.tracks()[owner_index];
    assert_eq!(owner.parent_group(), Some(ancestor));
    assert_eq!(project.tracks()[owner_index + 1].id(), unrelated);
    assert_eq!(owner.gain(), 0.55);
    let clip = project.clip(first).unwrap();
    assert_eq!(
        (clip.start(), clip.duration()),
        (start as u64, (end - start) as u64)
    );
    let samples = session.audio(clip.asset()).unwrap().frames();
    assert_eq!(samples.len(), end - start);
    assert!(
        samples
            .iter()
            .zip(&reference[start..end])
            .all(|(a, b)| (a[0] - b[0]).abs() <= 1e-7 && (a[1] - b[1]).abs() <= 1e-7)
    );
    let adopted = project.snapshot();
    let mut unchanged = adopted.tracks.clone();
    unchanged.remove(owner_index);
    assert_eq!(unchanged, original.tracks);
    assert!(session.undo());
    let undone = session.project().snapshot();
    assert!(undone.next_id > original.next_id);
    original.next_id = undone.next_id; // Undo keeps the allocator high-water mark.
    assert_eq!(undone, original);
    assert!(session.redo());
    assert_eq!(session.project().snapshot(), adopted);
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(session.project()).unwrap())
            .unwrap()
            .snapshot(),
        adopted
    );
}

#[test]
fn group_range_refuses_sources_and_sidechain_keys_crossing_its_boundary() {
    let (mut session, midi, _) = fixture_at_rate(8_000);
    let selected = group(&mut session, &[midi], "selected");
    let Created::Track(outside) = session
        .command(Command::InsertTrack {
            name: "outside".into(),
            before_track: None,
        })
        .unwrap()
    else {
        panic!("outside");
    };
    let target = BounceTarget::Range(TimeRange::new(vec![selected], 4_000, 4_128).unwrap());
    assert!(
        session
            .project()
            .plan_arrangement_bounce(&target, false)
            .is_ok()
    );
    let source = session.project().track(midi).unwrap().source_id().unwrap();
    session
        .command(Command::BindTrackSource {
            track: outside,
            source: Some(source),
        })
        .unwrap();
    assert_eq!(
        session
            .project()
            .plan_arrangement_bounce(&target, false)
            .unwrap_err()
            .0,
        "cannot bounce a group with a shared source crossing its boundary"
    );
    assert!(session.undo());
    let compressor = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.compressor"),
    )
    .unwrap();
    let Created::Device(device) = session
        .command(Command::InsertDevice {
            track: selected,
            before: None,
            state: compressor,
        })
        .unwrap()
    else {
        panic!("compressor");
    };
    session
        .command(Command::SetSidechain {
            device,
            source: Some(outside),
        })
        .unwrap();
    assert_eq!(
        session
            .project()
            .plan_arrangement_bounce(&target, false)
            .unwrap_err()
            .0,
        "cannot bounce a group with an external sidechain key"
    );
    session
        .command(Command::SetSidechain {
            device,
            source: Some(midi),
        })
        .unwrap();
    let plan = session
        .project()
        .plan_arrangement_bounce(&target, false)
        .unwrap();
    let isolated = isolated(session.project(), &target, &plan[0], false).unwrap();
    assert_eq!(isolated.sidechains(), session.project().sidechains());
}
