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
fn merged_range_prints_one_mix_and_mutes_only_the_source_interval() {
    let folder = Folder::new();
    let mut session = Session::new(8_000, 32, 40_000).unwrap();
    let a = audio_track(&mut session, &folder, "first", [0.2, 0.4]);
    let b = audio_track(&mut session, &folder, "second", [0.3, -0.2]);
    let bus = group(&mut session, &[a, b], "group");
    let c = audio_track(&mut session, &folder, "third", [0.125, -0.25]);
    let outside = audio_track(&mut session, &folder, "outside", [0.9, 0.9]);
    for (track, gain) in [(a, 0.5), (b, 0.75), (bus, 0.5), (c, 0.25), (outside, 1.0)] {
        session.command(Command::Gain { track, gain }).unwrap();
    }
    session
        .command(Command::Balance {
            track: b,
            balance: -1.0,
        })
        .unwrap();
    let (start, end) = (4_032, 4_160);
    let group_row = TimeRange::new(vec![bus], start, end).unwrap();
    assert_eq!(
        session
            .project()
            .merged_bounce_range(&group_row)
            .unwrap()
            .tracks(),
        &[bus, a, b]
    );
    let range = TimeRange::new(vec![bus, a, b, c], start, end).unwrap();
    let before = session.project().snapshot();
    session
        .bounce_arrangement_adopting(
            BounceTarget::MergedRange(range.clone()),
            false,
            BounceAdoption::NewTracksMutingSources,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap();
    let project = session.project();
    assert_eq!(project.tracks().len(), before.tracks.len() + 1);
    let bounce = &project.tracks()[4];
    assert_eq!(bounce.parent_group(), None);
    assert_eq!(bounce.gain(), 1.0);
    assert_eq!(project.tracks()[5].id(), outside);
    let [clip] = bounce.clips() else {
        panic!("one bounced clip")
    };
    assert_eq!((clip.start(), clip.duration()), (start, end - start));
    let asset = project.asset(clip.asset()).unwrap();
    let pcm =
        hex_media::decode_wav_file(asset.path(), hex_media::Limits::new(16_000, 1_000)).unwrap();
    let expected = [
        0.5 * (0.2 * 0.5 + 0.3 * 0.75) + 0.125 * 0.25,
        0.5 * 0.4 * 0.5 - 0.25 * 0.25,
    ];
    assert_eq!(pcm.audio().frames().len(), (end - start) as usize);
    for frame in pcm.audio().frames() {
        assert!((frame[0] - expected[0]).abs() < 0.000_001, "{frame:?}");
        assert!((frame[1] - expected[1]).abs() < 0.000_001, "{frame:?}");
    }
    assert_eq!(
        project.time_range_has_active_clips(&range).unwrap(),
        Some(false)
    );
    for id in [a, b, c] {
        let clips = project.track(id).unwrap().clips();
        assert!(clips.first().unwrap().active() && clips.last().unwrap().active());
    }
    assert!(project.track(outside).unwrap().clips()[0].active());
    let after = project.snapshot();
    assert!(session.undo());
    assert_eq!(session.project().snapshot().tracks, before.tracks);
    assert!(session.redo());
    assert_eq!(session.project().snapshot().tracks, after.tracks);
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
    let multi = TimeRange::new(vec![selected, inner], start as u64, end as u64).unwrap();
    let before_publish = folder.0.join("unsupported");
    assert!(
        session
            .bounce_arrangement_adopting(
                BounceTarget::Range(multi),
                false,
                BounceAdoption::NewTracksMutingSources,
                &before_publish,
                &hex_export::ExportControl::default(),
                None
            )
            .unwrap_err()
            .contains("select one group row")
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
            state: compressor.clone(),
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

    let Created::Device(external) = session
        .command(Command::InsertDevice {
            track: outside,
            before: None,
            state: compressor,
        })
        .unwrap()
    else {
        panic!("external compressor");
    };
    session
        .command(Command::SetSidechain {
            device: external,
            source: Some(midi),
        })
        .unwrap();
    assert!(
        session
            .project()
            .plan_arrangement_bounce(&target, false)
            .is_ok()
    );
    assert_eq!(
        session
            .project()
            .plan_arrangement_bounce(&BounceTarget::Track(selected), true)
            .unwrap_err()
            .0,
        "cannot mute group children used as external sidechain keys"
    );
    session
        .command(Command::SetSidechain {
            device: external,
            source: Some(selected),
        })
        .unwrap();
    assert!(
        session
            .project()
            .plan_arrangement_bounce(&target, false)
            .is_ok()
    );
    assert_eq!(
        session
            .project()
            .validate_bounce_adoption(&target, false, BounceAdoption::NewTracksMutingSources)
            .unwrap_err()
            .0,
        "cannot mute a group used as a sidechain key"
    );
}

#[test]
fn whole_group_prints_one_child_and_preserves_existing_descendants_and_console() {
    let folder = Folder::new();
    let (mut session, midi, midi_clip) = fixture_at_rate(8_000);
    let audio = audio_track(&mut session, &folder, "audio", [0.3, -0.2]);
    let muted = audio_track(&mut session, &folder, "muted", [0.8, 0.75]);
    session
        .command(Command::Mute {
            track: muted,
            muted: true,
        })
        .unwrap();
    let inner = group(&mut session, &[midi, audio], "inner");
    let selected = group(&mut session, &[inner, muted], "selected");
    session
        .command(Command::InsertDevice {
            track: inner,
            before: None,
            state: overdrive(),
        })
        .unwrap();
    session
        .command(Command::InsertDevice {
            track: selected,
            before: None,
            state: overdrive(),
        })
        .unwrap();
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "independent".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    session
        .command(Command::CopyArrangementMidiToSession {
            source: midi_clip,
            scene,
        })
        .unwrap();
    let audio_clip = session.project().track(audio).unwrap().clips()[0].id();
    session
        .command(Command::CopyArrangementAudioClipToSession {
            source: audio_clip,
            scene,
        })
        .unwrap();

    let target = BounceTarget::Track(selected);
    session
        .command(Command::ToggleSolo {
            track: audio,
            exclusive: false,
        })
        .unwrap();
    assert_eq!(
        session
            .project()
            .plan_arrangement_bounce(&target, true)
            .unwrap_err()
            .0,
        "unsolo the group children before bouncing in place"
    );
    assert!(session.undo());
    let plan = session
        .project()
        .plan_arrangement_bounce(&target, true)
        .unwrap();
    assert_eq!(plan.len(), 1, "child console cells are not printed");
    assert_eq!(
        (plan[0].track, plan[0].start, plan[0].end),
        (selected, 4_000, 8_000)
    );

    let tap = aligned_tap(&session, 8_032);
    assert!(
        tap[8_000..].iter().flatten().any(|s| s.abs() > 1e-4),
        "group effects have a real tail"
    );
    session
        .command(Command::Gain {
            track: selected,
            gain: 0.6,
        })
        .unwrap();
    session
        .command(Command::Balance {
            track: selected,
            balance: 0.1,
        })
        .unwrap();
    let live_before = aligned_tap(&session, 8_032);
    let mut original = session.project().snapshot();
    let control = hex_export::ExportControl::default();
    assert!(control.cancel());
    assert!(
        session
            .bounce_arrangement(target.clone(), true, &folder.0, &control, None)
            .is_err()
    );
    assert_eq!(session.project().snapshot(), original);
    let (Created::Clips { first, last }, _) = session
        .bounce_arrangement(
            target,
            true,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap()
    else {
        panic!("whole group bounce");
    };
    assert_eq!(first, last);
    let project = session.project();
    let result = &project.tracks()[1];
    assert_eq!(result.parent_group(), Some(selected));
    assert_eq!(result.clips()[0].id(), first);
    assert!(!result.muted() && result.devices().is_empty());
    let group = project.track(selected).unwrap();
    assert!(group.clips().is_empty() && group.devices().is_empty());
    assert_eq!((group.gain(), group.balance()), (0.6, 0.1));
    let clip = project.clip(first).unwrap();
    assert_eq!(clip.start(), 4_000);
    let samples = session.audio(clip.asset()).unwrap().frames();
    assert!(
        samples.len() > 4_032,
        "bounded whole-track tail extends the material"
    );
    assert!(
        samples[..4_032]
            .iter()
            .zip(&tap[4_000..])
            .all(|(a, b)| (a[0] - b[0]).abs() <= 1e-7 && (a[1] - b[1]).abs() <= 1e-7)
    );
    let live_after = aligned_tap(&session, 8_032);
    // The whole-track file starts at the earliest authored clip. Normal
    // latency-compensated playback can have filter pre-ringing before it.
    assert!(live_after[..4_000].iter().all(|frame| *frame == [0.0; 2]));
    for (offset, (a, b)) in live_after[4_000..]
        .iter()
        .zip(&live_before[4_000..])
        .enumerate()
    {
        let frame = 4_000 + offset;
        assert!(
            (a[0] - b[0]).abs() <= 1e-7 && (a[1] - b[1]).abs() <= 1e-7,
            "original children are silent and printed devices run once, frame {frame}: {a:?} != {b:?}"
        );
    }

    let adopted = project.snapshot();
    assert_eq!(adopted.session, original.session);
    assert_eq!(adopted.assets.len(), original.assets.len() + 1);
    let mut originals_with_mutes = original.tracks[1..].to_vec();
    for track in &mut originals_with_mutes {
        track.muted = true;
    }
    assert_eq!(&adopted.tracks[2..], &originals_with_mutes);
    assert!(session.undo());
    let undone = session.project().snapshot();
    assert!(undone.next_id > original.next_id);
    original.next_id = undone.next_id;
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

fn aligned_group_capture(
    session: &Session,
    group: TrackId,
    unrelated: TrackId,
    frames: usize,
    block: usize,
) -> [Vec<hex_engine::Frame>; 3] {
    use hex_engine::{OutputCapture, OutputCaptureId, SessionPlayback, SessionPlaybackCommand};
    let revision = session.compile_prepared_revision(None).unwrap();
    let latency = revision.arrangement().main_latency_frames() as usize;
    let mut runtime = SessionPlayback::new_linear_arrangement_export(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let plan = runtime
        .output_capture_plan(
            &[
                OutputCaptureId::Track(group),
                OutputCaptureId::Track(unrelated),
            ],
            block,
        )
        .unwrap();
    let [mut main, mut group, mut unrelated] =
        std::array::from_fn(|_| vec![[0.0; 2]; frames + latency]);
    for ((main, group), unrelated) in main
        .chunks_mut(block)
        .zip(group.chunks_mut(block))
        .zip(unrelated.chunks_mut(block))
    {
        let mut buffers: [&mut [hex_engine::Frame]; 2] = [group, unrelated];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        runtime.render_with_capture(main, &mut capture).unwrap();
    }
    [
        main.split_off(latency),
        group.split_off(latency),
        unrelated.split_off(latency),
    ]
}

#[test]
fn group_bounce_and_mute_replaces_only_the_half_open_output_with_exact_pcm() {
    use hex_project::{GainInterpolation, GainPoint, GroupOutputMuteRange};

    let folder = Folder::new();
    let mut session = Session::new(8_000, 32, 2_000_000).unwrap();
    let source = audio_track(&mut session, &folder, "source", [0.4, -0.28]);
    let group = group(&mut session, &[source], "selected");
    session
        .command(Command::InsertDevice {
            track: group,
            before: None,
            state: overdrive(),
        })
        .unwrap();
    let points = [0, 8_000]
        .into_iter()
        .zip([0.7, 1.1])
        .map(|(frame, gain)| GainPoint {
            beat: session.project().tempo_map().frame_to_beat(frame).unwrap(),
            gain,
            incoming_gain: None,
            outgoing: GainInterpolation::Linear,
        })
        .collect();
    session
        .command(Command::ReplaceTrackGainEnvelope {
            track: group,
            points,
        })
        .unwrap();
    let Created::ReturnTrack(bus) = session
        .command(Command::AddReturnTrackWithGain {
            name: "return".into(),
            gain: 0.5,
        })
        .unwrap()
    else {
        panic!("return");
    };
    session
        .command(Command::SetReturnSend {
            track: group,
            return_track: bus,
            gain: 0.25,
        })
        .unwrap();
    let unrelated = audio_track(&mut session, &folder, "unrelated", [0.16, 0.32]);
    let start = 4_055;
    let end = 4_133;
    let before = aligned_group_capture(&session, group, unrelated, 4_288, 17);
    assert!(
        before[1][start - 1..=end]
            .iter()
            .all(|frame| frame[0].abs() > 0.01)
    );
    let range = TimeRange::new(vec![group], start as u64, end as u64).unwrap();
    let mut original = session.project().snapshot();
    let target = BounceTarget::Range(range);
    let (Created::Clips { first, last }, _) = session
        .bounce_arrangement_adopting(
            target,
            false,
            BounceAdoption::NewTracksMutingSources,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap()
    else {
        panic!("group bounce and mute");
    };
    assert_eq!(first, last);
    let project = session.project();
    let group_track = project.track(group).unwrap();
    assert!(!group_track.muted());
    assert_eq!(
        group_track.group_output_mutes(),
        &[GroupOutputMuteRange::new(start as u64, end as u64).unwrap()]
    );
    let bounced = project.clip(first).unwrap();
    assert_eq!(
        (bounced.start(), bounced.duration()),
        (start as u64, (end - start) as u64)
    );
    for block in [7, 64, 256] {
        let after = aligned_group_capture(&session, group, unrelated, 4_288, block);
        assert_eq!(after[2], before[2], "unrelated track");
        for (frame, sample) in after[1].iter().enumerate() {
            let expected = if (start..end).contains(&frame) {
                [0.0; 2]
            } else {
                before[1][frame]
            };
            assert_eq!(*sample, expected, "source output at frame {frame}");
            let main = after[0][frame];
            let previous = before[0][frame];
            assert!(
                (main[0] - previous[0]).abs() < 1e-7 && (main[1] - previous[1]).abs() < 1e-7,
                "audible group replacement at frame {frame}: {main:?} != {previous:?}"
            );
        }
    }
    let adopted = project.snapshot();
    let group_before = &original.tracks[0];
    let mut restored_group = adopted.tracks[0].clone();
    restored_group.group_output_mutes.clear();
    assert_eq!(&restored_group, group_before);
    assert_eq!(
        adopted.tracks[1], original.tracks[1],
        "source clip crosses both boundaries unchanged"
    );
    assert!(session.undo());
    let undone = session.project().snapshot();
    original.next_id = undone.next_id;
    assert_eq!(undone, original);
    assert!(session.redo());
    assert_eq!(session.project().snapshot(), adopted);
    let bytes = hex_storage::encode(session.project()).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap().snapshot(), adopted);
}
