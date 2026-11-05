use super::*;
use hex_project::{BeatTime, ClipId, TrackId};
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "hex-arrangement-bounce-{}-{}",
            std::process::id(),
            NEXT_JOB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Session, TrackId, ClipId) {
    fixture_at_rate(48_000)
}
fn fixture_at_rate(rate: u32) -> (Session, TrackId, ClipId) {
    let mut session = Session::new(rate, 32, 2_000_000).unwrap();
    let Created::Track(track) = session
        .command(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(clip) = session
        .command(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    (session, track, clip)
}
fn pcm(revision: Arc<hex_engine::PreparedRevision>, frames: usize) -> Vec<hex_engine::Frame> {
    let mut runtime = hex_engine::SessionPlayback::new(revision).unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut samples = vec![[0.0; 2]; frames];
    for chunk in samples.chunks_mut(137) {
        runtime.render(chunk).unwrap();
    }
    samples
}

#[test]
fn isolated_bounce_retains_selected_alias_notes_and_excludes_console_members() {
    let (mut session, track, clip) = fixture();
    session
        .command(Command::DuplicateAliasClips(vec![clip]))
        .unwrap();
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "console".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    session
        .command(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "console notes".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap();
    session
        .command(Command::DuplicateSessionCell {
            track,
            scene,
            alias: true,
        })
        .unwrap();
    let original = session.project().clone();
    for (target, members) in [
        (
            BounceTarget::Range(hex_project::TimeRange::new(vec![track], 30_013, 50_007).unwrap()),
            2,
        ),
        (BounceTarget::Clips(vec![clip]), 1),
    ] {
        let plan = original.plan_arrangement_bounce(&target, false).unwrap();
        let projected = isolated(&original, &target, &plan[0], false).unwrap();
        assert!(projected.session_cells().is_empty());
        assert_eq!(
            projected.midi_clip(clip).unwrap().notes(),
            original.midi_clip(clip).unwrap().notes()
        );
        assert_eq!(projected.clip_alias(clip), original.clip_alias(clip));
        assert_eq!(projected.snapshot().aliases.sources.len(), 1);
        assert_eq!(projected.snapshot().aliases.members.len(), members);
    }
    assert_eq!(
        session.project(),
        &original,
        "projection never edits the document"
    );
}
#[test]
fn latent_track_bounce_is_aligned_and_ignores_other_path_latency() {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture_at_rate(8_000);
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: limiter.clone(),
        })
        .unwrap();
    let reference = session.compile_prepared_revision(None).unwrap();
    let latency = reference.arrangement().main_latency_frames() as usize;
    assert!(latency > 0);
    let target = BounceTarget::Clips(vec![clip]);
    let item = session
        .project()
        .plan_arrangement_bounce(&target, false)
        .unwrap()
        .remove(0);
    let expected = pcm(reference, item.end as usize + latency);
    session
        .command(Command::InsertMainDevice {
            before: None,
            state: limiter.clone(),
        })
        .unwrap();
    let Created::Track(other) = session
        .command(Command::AddTrack {
            name: "other".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    session
        .command(Command::InsertDevice {
            track: other,
            before: None,
            state: limiter,
        })
        .unwrap();
    let original_track = session.project().track(track).unwrap().clone();
    let original_other = session.project().track(other).unwrap().clone();
    let original_main = session.project().main_devices().to_vec();
    session
        .bounce_arrangement(
            target,
            false,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap();
    let bounced = &session.project().tracks()[1].clips()[0];
    let frames = session.audio(bounced.asset()).unwrap().frames();
    let range = &expected[item.start as usize + latency..item.end as usize + latency];
    assert_eq!(&frames[..range.len()], range);
    assert!(session.undo());
    assert_eq!(session.project().tracks().len(), 2);
    assert_eq!(session.project().track(track), Some(&original_track));
    assert_eq!(session.project().track(other), Some(&original_other));
    assert_eq!(session.project().main_devices(), original_main);
    assert!(session.redo());
    let redone = &session.project().tracks()[1].clips()[0];
    assert_eq!(
        &session.audio(redone.asset()).unwrap().frames()[..range.len()],
        range
    );
    let project_path = folder.0.join("latent-bounce.hex");
    hex_storage::save_new(&project_path, session.project()).unwrap();
    let loaded = hex_storage::load(&project_path).unwrap();
    let decoded = loaded
        .assets()
        .map(|asset| {
            (
                asset.id(),
                hex_media::decode_wav_file(
                    asset.path(),
                    hex_media::Limits::new(1_000_000, 100_000),
                )
                .unwrap(),
            )
        })
        .collect();
    let reopened = Session::from_loaded(loaded, 32, 100_000, decoded).unwrap();
    let reopened_clip = &reopened.project().tracks()[1].clips()[0];
    assert_eq!(
        &reopened.audio(reopened_clip.asset()).unwrap().frames()[..range.len()],
        range
    );
}

#[test]
fn selected_and_in_place_bounce_history_preserve_stopped_guard_through_undo_redo() {
    for inplace in [false, true] {
        let folder = Folder::new();
        let (mut session, track, clip) = fixture_at_rate(8_000);
        let revision = session.compile_prepared_revision(None).unwrap();
        let target = if inplace {
            BounceTarget::Track(track)
        } else {
            BounceTarget::Clips(vec![clip])
        };
        session
            .bounce_arrangement(
                target,
                inplace,
                &folder.0,
                &hex_export::ExportControl::default(),
                Some(&revision),
            )
            .unwrap();
        assert_eq!(
            session.editor.next_history_requires_stopped_bounce(false),
            inplace
        );
        assert!(session.undo());
        assert_eq!(
            session.editor.next_history_requires_stopped_bounce(true),
            inplace
        );
        assert!(session.redo());
    }
}
#[test]
fn exact_range_retains_prerange_instrument_state_and_stops_without_tail() {
    let folder = Folder::new();
    let (mut session, track, _) = fixture();
    let revision = session.compile_prepared_revision(None).unwrap();
    let expected = pcm(revision.clone(), 70_000);
    let start = 30_013;
    let end = 50_007;
    session
        .bounce_arrangement(
            BounceTarget::Range(hex_project::TimeRange::new(vec![track], start, end).unwrap()),
            false,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let audio = &session.project().tracks()[1].clips()[0];
    assert_eq!(audio.start(), start);
    assert_eq!(audio.duration(), end - start);
    let actual = session.audio(audio.asset()).unwrap().frames();
    let max = actual
        .iter()
        .flatten()
        .zip(expected[start as usize..end as usize].iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max < 1e-5,
        "range uses causal preroll instead of retriggering: {max}"
    );
    assert!(
        session
            .project()
            .plan_arrangement_bounce(
                &BounceTarget::Range(hex_project::TimeRange::new(vec![track], start, end).unwrap()),
                true
            )
            .is_err(),
        "partial in-place bake must not alter unrelated source"
    );
}
#[test]
fn failures_cancellation_and_second_output_preserve_model_history_media_and_original_files() {
    for whole_track in [false, true] {
        bounce_failure_case(whole_track);
    }
}

fn bounce_failure_case(whole_track: bool) {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    let target = if whole_track {
        BounceTarget::Track(track)
    } else {
        BounceTarget::Clips(vec![clip])
    };
    let original = session.project().clone();
    let resident = session.resident_frames;
    let control = hex_export::ExportControl::default();
    assert!(control.cancel());
    assert!(
        session
            .bounce_arrangement(target.clone(), whole_track, &folder.0, &control, None)
            .is_err()
    );
    assert_eq!(session.project(), &original);
    let occupied = folder.0.join("occupied");
    fs::write(&occupied, b"original file").unwrap();
    assert!(
        session
            .bounce_arrangement(
                target.clone(),
                whole_track,
                &occupied,
                &hex_export::ExportControl::default(),
                None
            )
            .is_err()
    );
    assert_eq!(fs::read(&occupied).unwrap(), b"original file");

    let mut files_seen = Vec::new();
    let result: Result<(), String> = session.bounce_arrangement_using(
        target.clone(),
        whole_track,
        &folder.0,
        &hex_export::ExportControl::default(),
        |_, _, _| Ok(()),
        |path, _, item, _, _| {
            fs::write(path, b"not a wav").unwrap();
            files_seen.push(path.to_path_buf());
            Ok(item.end - item.start)
        },
        |_, _, _| panic!("decode failure must not commit"),
    );
    assert!(result.is_err());
    assert!(files_seen.iter().all(|p| !p.exists()));
    assert_eq!(session.project(), &original);
    assert_eq!(session.resident_frames, resident);
    let item = session
        .project()
        .plan_arrangement_bounce(&target, whole_track)
        .unwrap()
        .remove(0);
    let path = folder.0.join("prepared.wav");
    // Use the durable real renderer for the final-preparation failure boundary.
    let revision = session.compile_prepared_revision(None).unwrap();
    hex_export::render_bounce_new(&path, revision, item.start, item.end - item.start, 1.0).unwrap();
    let prepared = PreparedImport::new(
        hex_media::decode_wav_file(&path, hex_media::Limits::new(1_000_000, 100_000)).unwrap(),
    )
    .unwrap();
    let result = session.adopt_arrangement_bounce_using(
        target.clone(),
        whole_track,
        hex_project::BounceAdoption::NewTracks,
        vec![BounceFile {
            source: hex_project::BounceSource::Arrangement,
            track: item.track,
            start: item.start,
            frames: item.end - item.start,
            path,
        }],
        vec![prepared],
        None,
        |_| Err::<(), _>(SessionError::Invalid("publication refused")),
    );
    assert!(result.is_err());
    assert_eq!(session.project(), &original);
    assert_eq!(session.resident_frames, resident);
}

#[test]
fn multi_track_failure_and_cancellation_after_render_preserve_the_entire_transaction() {
    let folder = Folder::new();
    let (mut session, _, first) = fixture();
    let Created::Track(second_track) = session
        .command(Command::InsertTrackWithDevice {
            name: "second".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(second) = session
        .command(Command::PlaceMidiClip {
            track: second_track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!()
    };
    let original = session.project().clone();
    let resident = session.resident_frames;
    for cancel in [false, true] {
        let control = hex_export::ExportControl::default();
        let mut rendered = Vec::new();
        let result: Result<(), String> = session.bounce_arrangement_using(
            BounceTarget::Clips(vec![first, second]),
            false,
            &folder.0,
            &control,
            |session, project, _| {
                let waveforms = std::cell::RefCell::new(
                    session.audio_clip_waveforms().map_err(|e| e.to_string())?,
                );
                crate::prepare_with_waveforms(
                    project,
                    &session.media_bank(),
                    session.sine_table(),
                    None,
                    &session.audio_transforms,
                    &waveforms,
                )
                .map(Arc::new)
                .map_err(|e| e.to_string())
            },
            |path, revision, item, tails, control| {
                if !rendered.is_empty() {
                    return Err("second output failed".into());
                }
                let report = hex_export::render_arrangement_bounce_new(
                    path,
                    revision,
                    None,
                    item.start,
                    item.end - item.start,
                    tails,
                    control,
                )
                .map_err(|e| e.to_string())?;
                rendered.push(path.to_owned());
                if cancel {
                    assert!(control.cancel());
                }
                Ok(report.frames)
            },
            |_, _, _| panic!("partial render must not commit"),
        );
        assert!(result.is_err());
        assert_eq!(rendered.len(), 1);
        assert!(!rendered[0].exists());
        assert_eq!(session.project(), &original);
        assert_eq!(session.resident_frames, resident);
    }
}

#[test]
fn selected_source_isolation_excludes_other_clips_tracks_main_and_returns() {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    let expected = pcm(session.compile_prepared_revision(None).unwrap(), 80_000);
    let Created::Clip(other) = session
        .command(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(3).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::AddMidiNote {
            clip: other,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 72,
            velocity: 127,
            channel: 0,
        })
        .unwrap();
    let utility = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(12.0)
    .unwrap();
    session
        .command(Command::InsertMainDevice {
            before: None,
            state: utility,
        })
        .unwrap();
    let Created::ReturnTrack(bus) = session
        .command(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    session
        .command(Command::AddTrack {
            name: "unrelated".into(),
        })
        .unwrap();
    let target = BounceTarget::Clips(vec![clip]);
    let item = session
        .project()
        .plan_arrangement_bounce(&target, false)
        .unwrap()
        .remove(0);
    let source = isolated(session.project(), &target, &item, false).unwrap();
    assert_eq!(source.tracks().len(), 1);
    assert!(source.main_devices().is_empty());
    assert!(source.return_tracks().is_empty());
    assert!(source.tracks()[0].sends().is_empty());
    assert_eq!(source.tracks()[0].midi_clips().len(), 1);
    assert!(source.midi_clip(other).is_none());
    let before = session.project().clone();
    session
        .bounce_arrangement(
            target,
            false,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap();
    assert_eq!(
        session.project().track(track).unwrap(),
        before.track(track).unwrap()
    );
    let audio = &session.project().tracks()[1].clips()[0];
    let samples = session.audio(audio.asset()).unwrap().frames();
    let expected = &expected[audio.start() as usize..(audio.start() + audio.duration()) as usize];
    let maximum = samples
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        maximum < 1e-5,
        "unrelated track/bus processing is not baked: {maximum}"
    );
    assert!(
        audio.start() + audio.duration() < 72_000,
        "unselected later note is excluded"
    );
}

#[test]
fn retained_source_roles_follow_isolated_track_and_audio_group_projections() {
    let (session, receiver, _) = fixture_at_rate(8_000);
    let mut editor = hex_project::Editor::new(session.project().clone(), 32).unwrap();
    let source = session
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "fixture.wav".into(),
            sample_rate: 8_000,
            frames: 32,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let mut clips = Vec::new();
    for start in [0, 64] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceClip {
                track: receiver,
                asset,
                start,
                duration: 32,
                source_offset: 0,
                source_length: 32,
                looping: false,
            })
            .unwrap()
        else {
            panic!("audio clip")
        };
        clips.push(clip);
    }
    let Created::Track(peer) = editor
        .execute(Command::AddMidiTrack {
            name: "peer".into(),
        })
        .unwrap()
    else {
        panic!("peer")
    };
    editor
        .execute(Command::BindTrackSource {
            track: peer,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(peer_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: peer,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("peer clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: peer_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            velocity: 100,
            channel: 0,
        })
        .unwrap();
    let ordinary = editor.project().clone();
    let mut snapshot = ordinary.snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let retained = Project::from_snapshot(snapshot).unwrap();
    let original = retained.snapshot();
    let samples = vec![[0.25, -0.5]; 32];
    let media = std::collections::BTreeMap::from([(
        asset,
        hex_engine::Audio::new(8_000, samples.clone()).unwrap(),
    )]);
    let render = |project: &Project, frames| {
        pcm(
            Arc::new(
                hex_engine::PreparedRevision::prepare(project, &media, session.sine_table(), None)
                    .unwrap(),
            ),
            frames,
        )
    };

    let target = BounceTarget::Track(peer);
    let item = ordinary
        .plan_arrangement_bounce(&target, true)
        .unwrap()
        .remove(0);
    let expected = isolated(&ordinary, &target, &item, true).unwrap();
    let projected = isolated(&retained, &target, &item, true).unwrap();
    assert!(projected.track(receiver).is_none());
    assert!(!projected.track(peer).unwrap().retained_source_path());
    assert_eq!(
        projected.native_source(source).unwrap().receiver(),
        Some(peer)
    );
    let expected_audio = render(&expected, 9_000);
    assert!(
        expected_audio
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1e-5)
    );
    assert_eq!(render(&projected, 9_000), expected_audio);

    let target = BounceTarget::Clips(vec![clips[0]]);
    let item = retained
        .plan_arrangement_bounce(&target, false)
        .unwrap()
        .remove(0);
    let projected = isolated(&retained, &target, &item, false).unwrap();
    assert!(projected.track(receiver).unwrap().retained_source_path());
    assert_eq!(
        projected.native_source(source).unwrap().receiver(),
        Some(receiver)
    );
    assert_eq!(render(&projected, 32), samples);

    let target = BounceTarget::AudioGroup { clips, range: None };
    let item = retained
        .plan_arrangement_bounce(&target, true)
        .unwrap()
        .remove(0);
    let projected = isolated(&retained, &target, &item, true).unwrap();
    assert!(!projected.track(receiver).unwrap().retained_source_path());
    assert!(projected.track(receiver).unwrap().instrument().is_none());
    assert_eq!(projected.native_source(source).unwrap().receiver(), None);
    let rendered = render(&projected, 96);
    assert_eq!(&rendered[..32], samples);
    assert_eq!(&rendered[32..64], &[[0.0; 2]; 32]);
    assert_eq!(&rendered[64..], samples);
    assert_eq!(retained.snapshot(), original);
    assert_eq!(editor.project(), &ordinary);
}

#[test]
fn whole_receiver_projection_excludes_peer_midi_before_source_processing() {
    let (mut session, receiver, _) = fixture_at_rate(8_000);
    let source = session
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.saturator"),
    )
    .unwrap();
    session
        .command(Command::InsertDevice {
            track: receiver,
            before: None,
            state: effect,
        })
        .unwrap();
    let target = BounceTarget::Track(receiver);
    let item = session
        .project()
        .plan_arrangement_bounce(&target, true)
        .unwrap()
        .remove(0);
    let reference = session.compile_prepared_revision(None).unwrap();
    for (pitch, start) in [(67, BeatTime::ZERO), (72, BeatTime::ONE)] {
        let Created::Track(peer) = session
            .command(Command::AddMidiTrack {
                name: "peer".into(),
            })
            .unwrap()
        else {
            panic!("peer")
        };
        session
            .command(Command::BindTrackSource {
                track: peer,
                source: Some(source),
            })
            .unwrap();
        let Created::Clip(clip) = session
            .command(Command::PlaceMidiClip {
                track: peer,
                start,
                length: BeatTime::from_quarters(2).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        session
            .command(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch,
                velocity: 110,
                channel: 0,
            })
            .unwrap();
    }
    let original = session.project().snapshot();
    let mixed = session.compile_prepared_revision(None).unwrap();
    let projected = isolated(session.project(), &target, &item, true).unwrap();
    assert_eq!(projected.tracks().len(), 1);
    assert_eq!(projected.track(receiver).unwrap().source_id(), Some(source));
    assert_eq!(
        projected.native_source(source),
        session.project().native_source(source)
    );
    assert_eq!(
        projected.track(receiver).unwrap().devices(),
        session.project().track(receiver).unwrap().devices()
    );
    let waveforms = std::cell::RefCell::new(session.audio_clip_waveforms().unwrap());
    let prepared = Arc::new(
        crate::prepare_with_waveforms(
            &projected,
            &session.media_bank(),
            session.sine_table(),
            None,
            &session.audio_transforms,
            &waveforms,
        )
        .unwrap(),
    );
    let frames = usize::try_from(item.end + 1_000).unwrap();
    let expected = pcm(reference, frames);
    let actual = pcm(prepared, frames);
    let mixed = pcm(mixed, frames);
    assert!(
        expected
            .iter()
            .zip(&mixed)
            .any(|(a, b)| (a[0] - b[0]).abs() > 1e-5),
        "peer input must affect the mixed reference"
    );
    assert!(
        expected
            .iter()
            .zip(&actual)
            .all(|(a, b)| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6)
    );
    assert_eq!(session.project().snapshot(), original);
}

#[test]
fn receiver_clip_bounce_excludes_shared_contributors_and_whole_track_refuses() {
    for whole_track in [false, true] {
        shared_receiver_bounce_case(whole_track);
    }
}

fn shared_receiver_bounce_case(whole_track: bool) {
    let folder = Folder::new();
    let (mut session, receiver, receiver_clip) = fixture();
    let source_id = session
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = session
        .command(Command::AddMidiTrack {
            name: "shared midi".into(),
        })
        .unwrap()
    else {
        panic!("midi track")
    };
    session
        .command(Command::BindTrackSource {
            track: contributor,
            source: Some(source_id),
        })
        .unwrap();
    let Created::Clip(contributor_clip) = session
        .command(Command::PlaceMidiClip {
            track: contributor,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    let receiver_effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    session
        .command(Command::InsertDevice {
            track: receiver,
            before: None,
            state: receiver_effect.clone(),
        })
        .unwrap();
    session
        .command(Command::InsertDevice {
            track: contributor,
            before: None,
            state: receiver_effect,
        })
        .unwrap();

    // Capture a render before the other contributor starts feeding the source.
    let receiver_only = session.compile_prepared_revision(None).unwrap();
    session
        .command(Command::AddMidiNote {
            clip: contributor_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 96,
        })
        .unwrap();

    let target = if whole_track {
        BounceTarget::Track(receiver)
    } else {
        BounceTarget::Clips(vec![receiver_clip])
    };
    if whole_track {
        let before = session.project().clone();
        let error = session
            .bounce_arrangement(
                target,
                true,
                &folder.0,
                &hex_export::ExportControl::default(),
                None,
            )
            .unwrap_err();
        assert!(error.contains("linked tracks"), "{error}");
        assert_eq!(session.project(), &before);
        assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 0);
        return;
    }
    let item = session
        .project()
        .plan_arrangement_bounce(&target, whole_track)
        .unwrap()
        .remove(0);
    let isolated = isolated(session.project(), &target, &item, whole_track).unwrap();
    assert_eq!(isolated.tracks().len(), if whole_track { 2 } else { 1 });
    if !whole_track {
        let existing_in_place = super::isolated(session.project(), &target, &item, true).unwrap();
        assert!(existing_in_place.track(contributor).is_some());
    }
    assert_eq!(
        isolated.native_source(source_id).unwrap().receiver(),
        Some(receiver)
    );
    let isolated_receiver = isolated.track(receiver).unwrap();
    assert_eq!(isolated_receiver.source_id(), Some(source_id));
    assert_eq!(isolated_receiver.devices().len(), 1);

    let original = session.project().clone();
    let revision = session.compile_prepared_revision(None).unwrap();
    session
        .bounce_arrangement(
            target,
            whole_track,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let audio = session
        .project()
        .tracks()
        .iter()
        .find_map(|track| track.clips().first())
        .expect("bounce creates an audio clip");
    let actual = session.audio(audio.asset()).unwrap().frames();
    let expected = pcm(
        if whole_track { revision } else { receiver_only },
        (audio.start() + audio.duration()) as usize,
    );
    let expected = &expected[audio.start() as usize..(audio.start() + audio.duration()) as usize];
    let error = actual
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(error < 1e-5, "shared receiver bounce PCM differs: {error}");
    if !whole_track {
        assert_eq!(
            session.project().native_source(source_id),
            original.native_source(source_id)
        );
        assert_eq!(
            session.project().track(contributor),
            original.track(contributor)
        );
        assert!(session.undo());
        assert_eq!(
            session.project().snapshot().tracks,
            original.snapshot().tracks
        );
        assert_eq!(
            session.project().native_sources(),
            original.native_sources()
        );
    }
}

#[test]
fn shared_source_range_bounces_keep_only_selected_tracks_causal_input() {
    for choose_contributor in [false, true] {
        let folder = Folder::new();
        let (mut session, receiver, receiver_clip) = fixture();
        let source = session
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(contributor) = session
            .command(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        session
            .command(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        let Created::Clip(contributor_clip) = session
            .command(Command::PlaceMidiClip {
                track: contributor,
                start: BeatTime::ONE,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        session
            .command(Command::AddMidiNote {
                clip: contributor_clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 67,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let selected = if choose_contributor {
            contributor
        } else {
            receiver
        };
        let other_clip = if choose_contributor {
            receiver_clip
        } else {
            contributor_clip
        };
        let original = session.project().clone();
        if choose_contributor {
            session
                .command(Command::SetNativeSourceReceiver {
                    source,
                    track: Some(selected),
                })
                .unwrap();
        }
        session
            .command(Command::SetClipActive {
                clip: other_clip,
                active: false,
            })
            .unwrap();
        let reference = session.compile_prepared_revision(None).unwrap();
        assert!(session.undo());
        if choose_contributor {
            assert!(session.undo());
        }
        assert_eq!(session.project(), &original);
        let start = 30_000;
        let end = 40_000;
        let expected = pcm(reference, end as usize);
        let expected = &expected[start as usize..end as usize];
        assert!(
            expected.iter().flatten().any(|sample| sample.abs() > 1e-5),
            "preroll must retain an audible earlier note"
        );
        let revision = session.compile_prepared_revision(None).unwrap();
        let range = hex_project::TimeRange::new(vec![selected], start, end).unwrap();
        session
            .bounce_arrangement(
                BounceTarget::Range(range),
                false,
                &folder.0,
                &hex_export::ExportControl::default(),
                Some(&revision),
            )
            .unwrap();
        let clip = session
            .project()
            .tracks()
            .iter()
            .find_map(|track| track.clips().first())
            .expect("bounced clip");
        let actual = session.audio(clip.asset()).unwrap().frames();
        assert_eq!(actual.len(), expected.len());
        let error = actual
            .iter()
            .flatten()
            .zip(expected.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            error < 1e-5,
            "isolated range differs for contributor={choose_contributor}: {error}"
        );
        assert_eq!(
            session.project().native_source(source),
            original.native_source(source)
        );
        assert_eq!(session.project().track(receiver), original.track(receiver));
        assert_eq!(
            session.project().track(contributor),
            original.track(contributor)
        );
        assert!(session.undo());
        assert_eq!(
            session.project().snapshot().tracks,
            original.snapshot().tracks
        );
        assert_eq!(
            session.project().native_sources(),
            original.native_sources()
        );
    }
}

#[test]
fn shared_contributor_selected_clip_renders_its_own_source_without_other_notes() {
    let folder = Folder::new();
    let (mut session, receiver, receiver_clip) = fixture();
    let source = session
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = session
        .command(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    session
        .command(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(clip) = session
        .command(Command::PlaceMidiClip {
            track: contributor,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    session
        .command(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(-6.0)
    .unwrap();
    session
        .command(Command::InsertDevice {
            track: contributor,
            before: None,
            state: effect,
        })
        .unwrap();
    let original = session.project().clone();
    // Independent reference: temporarily route the live source to this track and
    // disable the other track's clip, then compile before restoring the project.
    session
        .command(Command::SetNativeSourceReceiver {
            source,
            track: Some(contributor),
        })
        .unwrap();
    session
        .command(Command::SetClipActive {
            clip: receiver_clip,
            active: false,
        })
        .unwrap();
    let reference = session.compile_prepared_revision(None).unwrap();
    assert!(session.undo());
    assert!(session.undo());
    assert_eq!(session.project(), &original);
    let revision = session.compile_prepared_revision(None).unwrap();
    session
        .bounce_arrangement(
            BounceTarget::Clips(vec![clip]),
            false,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let audio = session
        .project()
        .tracks()
        .iter()
        .find_map(|track| track.clips().first())
        .expect("bounced audio");
    let actual = session.audio(audio.asset()).unwrap().frames();
    let start = audio.start() as usize;
    let expected = pcm(reference, start + actual.len());
    let expected = &expected[start..];
    assert_eq!(actual.len(), expected.len());
    assert!(
        expected.iter().flatten().any(|sample| sample.abs() > 1e-5),
        "reference must contain audible source output"
    );
    let error = actual
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(error < 1e-5, "isolated contributor PCM differs: {error}");
    assert_eq!(
        session.project().native_source(source),
        original.native_source(source)
    );
    assert_eq!(session.project().track(receiver), original.track(receiver));
    assert_eq!(
        session.project().track(contributor),
        original.track(contributor)
    );
    assert!(session.undo());
    assert_eq!(
        session.project().snapshot().tracks,
        original.snapshot().tracks
    );
    assert_eq!(
        session.project().native_sources(),
        original.native_sources()
    );
}

#[test]
fn whole_contributor_bounce_preserves_shared_source_and_receiver() {
    let folder = Folder::new();
    let (mut session, receiver, receiver_clip) = fixture();
    let source = session
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = session
        .command(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    session
        .command(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(clip) = session
        .command(Command::PlaceMidiClip {
            track: contributor,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    session
        .command(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(-6.0)
    .unwrap();
    session
        .command(Command::InsertDevice {
            track: contributor,
            before: None,
            state: effect,
        })
        .unwrap();
    let original = session.project().clone();
    // Independent reference: temporarily route the live source to this track and
    // disable the other track's clip, then compile before restoring the project.
    session
        .command(Command::SetNativeSourceReceiver {
            source,
            track: Some(contributor),
        })
        .unwrap();
    session
        .command(Command::SetClipActive {
            clip: receiver_clip,
            active: false,
        })
        .unwrap();
    let reference = session.compile_prepared_revision(None).unwrap();
    assert!(session.undo());
    assert!(session.undo());
    assert_eq!(session.project(), &original);
    let revision = session.compile_prepared_revision(None).unwrap();
    session
        .bounce_arrangement(
            BounceTarget::Track(contributor),
            true,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let audio = session
        .project()
        .tracks()
        .iter()
        .find_map(|track| track.clips().first())
        .expect("bounced audio");
    let actual = session.audio(audio.asset()).unwrap().frames();
    let start = audio.start() as usize;
    let expected = pcm(reference, start + actual.len());
    let expected = &expected[start..];
    assert_eq!(actual.len(), expected.len());
    assert!(
        expected.iter().flatten().any(|sample| sample.abs() > 1e-5),
        "reference must contain audible source output"
    );
    let error = actual
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(error < 1e-5, "isolated contributor PCM differs: {error}");
    assert_eq!(
        session.project().native_source(source),
        original.native_source(source)
    );
    assert_eq!(session.project().track(receiver), original.track(receiver));
    let bounced = session.project().clone();
    let bounced_track = session.project().track(contributor).unwrap();
    assert!(bounced_track.instrument().is_none());
    assert!(bounced_track.devices().is_empty());
    assert!(session.undo());
    assert_eq!(
        session.project().snapshot().tracks,
        original.snapshot().tracks
    );
    assert_eq!(
        session.project().native_sources(),
        original.native_sources()
    );
    assert!(session.redo());
    assert_eq!(session.project(), &bounced);

    let asset = session.project().track(contributor).unwrap().clips()[0].asset();
    let audio_path = session.project().asset(asset).unwrap().path().to_path_buf();
    let audio_bytes = fs::read(&audio_path).unwrap();
    let project_path = folder.0.join("shared-bounce.hex");
    hex_storage::save_new(&project_path, session.project()).unwrap();
    let loaded = hex_storage::load(&project_path).unwrap();
    assert_eq!(loaded, bounced);
    assert_eq!(
        loaded.native_source(source).unwrap().receiver(),
        Some(receiver)
    );
    assert_eq!(loaded.track(contributor).unwrap().source_id(), None);
    let decoded = loaded
        .assets()
        .map(|asset| {
            (
                asset.id(),
                hex_media::decode_wav_file(
                    asset.path(),
                    hex_media::Limits::new(64 * 1024 * 1024 + 44, 2_000_000),
                )
                .unwrap(),
            )
        })
        .collect();
    let mut reopened = Session::from_loaded(loaded, 32, 2_000_000, decoded).unwrap();
    assert_eq!(
        reopened.audio(asset).unwrap().frames(),
        session.audio(asset).unwrap().frames()
    );
    reopened.command(Command::RemoveTrack(contributor)).unwrap();
    let Created::Track(replacement) = reopened
        .command(Command::AddMidiTrack {
            name: "replacement".into(),
        })
        .unwrap()
    else {
        panic!("replacement")
    };
    reopened
        .command(Command::BindTrackSource {
            track: replacement,
            source: Some(source),
        })
        .unwrap();
    assert_eq!(
        reopened.project().native_source(source).unwrap().receiver(),
        Some(receiver)
    );
    assert_eq!(
        reopened.project().track(replacement).unwrap().source_id(),
        Some(source)
    );
    assert_eq!(fs::read(audio_path).unwrap(), audio_bytes);
}

#[test]
fn shared_contributor_console_midi_matches_isolated_routed_playback() {
    for looping in [false, true] {
        let folder = Folder::new();
        let (mut session, receiver, receiver_clip) = fixture();
        let source = session
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(contributor) = session
            .command(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        session
            .command(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        let Created::Clip(clip) = session
            .command(Command::PlaceMidiClip {
                track: contributor,
                start: BeatTime::ONE,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        session
            .command(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 67,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let Created::Scene(scene) = session
            .command(Command::AddScene {
                name: "console".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(cell) = session
            .command(Command::CopyArrangementMidiToSession {
                source: clip,
                scene,
            })
            .unwrap()
        else {
            panic!("cell")
        };
        session
            .command(Command::SetSessionMidiClipSource {
                clip: cell,
                source_length: BeatTime::ONE,
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: BeatTime::ONE,
                looping,
            })
            .unwrap();
        if looping {
            session
                .command(Command::SetClipActive {
                    clip: cell,
                    active: false,
                })
                .unwrap();
        }
        let original = session.project().clone();
        let plan = original
            .plan_arrangement_bounce(&BounceTarget::Track(contributor), true)
            .unwrap();
        let item = plan
            .iter()
            .find(|item| matches!(item.source, hex_project::BounceSource::Console { clip, .. } if clip == cell))
            .unwrap();
        let frames = item.end as usize;
        session
            .command(Command::SetNativeSourceReceiver {
                source,
                track: Some(contributor),
            })
            .unwrap();
        session
            .command(Command::SetClipActive {
                clip: receiver_clip,
                active: false,
            })
            .unwrap();
        let expected = console_pcm(&session, cell, frames);
        assert!(
            expected.iter().flatten().any(|sample| sample.abs() > 1e-5),
            "reference must be audible"
        );
        assert!(session.undo());
        assert!(session.undo());
        assert_eq!(session.project(), &original);
        session
            .bounce_arrangement(
                BounceTarget::Track(contributor),
                true,
                &folder.0,
                &hex_export::ExportControl::default(),
                None,
            )
            .unwrap();
        let bounced = session
            .project()
            .session_audio_clip(cell)
            .expect("bounced cell");
        assert_eq!(bounced.active(), !looping);
        let actual = session.audio(bounced.asset()).unwrap().frames();
        assert!(actual.len() >= frames);
        let error = actual[..frames]
            .iter()
            .flatten()
            .zip(expected.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            error < 1e-5,
            "console bounce differs for looping={looping}: {error}"
        );
        assert_eq!(
            session.project().native_source(source),
            original.native_source(source)
        );
        assert_eq!(session.project().track(receiver), original.track(receiver));
        let after = session.project().clone();
        assert!(session.undo());
        assert_eq!(
            session.project().snapshot().tracks,
            original.snapshot().tracks
        );
        assert_eq!(
            session.project().native_sources(),
            original.native_sources()
        );
        assert!(session.redo());
        assert_eq!(session.project(), &after);
    }
}

#[test]
fn contributor_console_midi_bounce_cancellation_preserves_project() {
    let (mut session, receiver, _) = fixture();
    let source_id = session
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = session
        .command(Command::AddMidiTrack {
            name: "shared midi".into(),
        })
        .unwrap()
    else {
        panic!("midi track")
    };
    session
        .command(Command::BindTrackSource {
            track: contributor,
            source: Some(source_id),
        })
        .unwrap();
    let Created::Clip(clip) = session
        .command(Command::PlaceMidiClip {
            track: contributor,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    assert!(
        session
            .project()
            .plan_arrangement_bounce(&BounceTarget::Clips(vec![clip]), true)
            .is_err()
    );
    assert!(
        session
            .project()
            .plan_arrangement_bounce(
                &BounceTarget::Range(
                    hex_project::TimeRange::new(vec![contributor], 0, 48_000).unwrap()
                ),
                true,
            )
            .is_err()
    );
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "console".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    session
        .command(Command::CreateSessionMidiClip {
            track: contributor,
            scene,
            name: "console".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap();
    let original = session.project().clone();
    let folder = Folder::new();
    let control = hex_export::ExportControl::default();
    assert!(control.cancel());
    let error = session
        .bounce_arrangement(
            BounceTarget::Track(contributor),
            true,
            &folder.0,
            &control,
            None,
        )
        .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert_eq!(session.project(), &original);
    assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 0);
}

#[test]
fn nonreceiver_audio_bounce_isolates_its_audio_and_unassigns_retained_source() {
    for shared in [false, true] {
        nonreceiver_audio_bounce_case(shared);
    }
}

fn nonreceiver_audio_bounce_case(shared: bool) {
    let folder = Folder::new();
    let (mut session, receiver, _) = fixture();
    let source_id = session
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let source_audio = Audio::new(
        48_000,
        (0..4096)
            .map(|frame| {
                let sample = (frame as f32 * 0.071).sin() * 0.2;
                [sample, -sample]
            })
            .collect(),
    )
    .unwrap();
    let prepared = PreparedImport {
        path: folder.0.join("local-audio.wav"),
        source_sample_rate: 48_000,
        source_frames: source_audio.frames().len() as u64,
        conversion_version: hex_project::IDENTITY_CONVERSION,
        waveform: waveform::summarize(&source_audio, 1024).unwrap(),
        source_audio: source_audio.clone(),
        audio: source_audio.clone(),
        file_identity: None,
    };
    let Created::Imported {
        track: contributor,
        clip,
        ..
    } = session.import(prepared).unwrap()
    else {
        panic!("audio import")
    };
    if shared {
        session
            .command(Command::BindTrackSource {
                track: contributor,
                source: Some(source_id),
            })
            .unwrap();
    }
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(-6.0)
    .unwrap();
    let gain = effect.output_gain_linear();
    session
        .command(Command::InsertDevice {
            track: contributor,
            before: None,
            state: effect,
        })
        .unwrap();
    let target = BounceTarget::Clips(vec![clip]);
    let item = session
        .project()
        .plan_arrangement_bounce(&target, false)
        .unwrap()
        .remove(0);
    let isolated = isolated(session.project(), &target, &item, false).unwrap();
    assert_eq!(isolated.tracks().len(), 1);
    assert!(isolated.track(contributor).unwrap().source_id().is_none());
    assert_eq!(isolated.track(contributor).unwrap().clips().len(), 1);
    assert_eq!(isolated.track(contributor).unwrap().devices().len(), 1);
    assert_eq!(isolated.native_source(source_id).unwrap().receiver(), None);

    let (created, _) = session
        .bounce_arrangement(
            target,
            false,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap();
    let Created::Clips { first, .. } = created else {
        panic!("audio bounce")
    };
    let bounced = session
        .project()
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .find(|clip| clip.id() == first)
        .unwrap();
    let actual = session.audio(bounced.asset()).unwrap().frames();
    assert_eq!(actual.len(), source_audio.frames().len());
    let error = actual
        .iter()
        .flatten()
        .zip(source_audio.frames().iter().flatten())
        .map(|(actual, dry)| (actual - dry * gain).abs())
        .fold(0.0_f32, f32::max);
    assert!(error < 1e-6, "local effects must be baked once: {error}");
}

#[test]
fn directed_audio_and_midi_bounces_preserve_accepted_content_pcm() {
    for midi in [false, true] {
        for range in [false, true] {
            let folder = Folder::new();
            let (mut session, track, clip) = if midi {
                let (mut session, track, clip) = fixture();
                session
                    .command(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::from_ratio(1, 4).unwrap(),
                        pitch: 67,
                        channel: 0,
                        velocity: 83,
                    })
                    .unwrap();
                (session, track, clip)
            } else {
                let mut session = Session::new(48_000, 32, 2_000_000).unwrap();
                let audio = Audio::new(
                    48_000,
                    (0..24_000)
                        .map(|i| {
                            let x = (i as f32 * 0.173).sin() * (i as f32 / 48_000.0);
                            [x, -x]
                        })
                        .collect(),
                )
                .unwrap();
                let prepared = PreparedImport {
                    path: folder.0.join("source.wav"),
                    source_sample_rate: 48_000,
                    source_frames: 24_000,
                    conversion_version: hex_project::IDENTITY_CONVERSION,
                    waveform: waveform::summarize(&audio, 1_024).unwrap(),
                    source_audio: audio.clone(),
                    audio,
                    file_identity: None,
                };
                let Created::Imported { track, clip, .. } = session.import(prepared).unwrap()
                else {
                    panic!("import")
                };
                (session, track, clip)
            };
            let forward = pcm(session.compile_prepared_revision(None).unwrap(), 80_000);
            session.command(Command::ReverseClips(vec![clip])).unwrap();
            let revision = session.compile_prepared_revision(None).unwrap();
            let expected = pcm(revision.clone(), 80_000);
            assert_ne!(expected, forward, "asymmetric fixture exercises reflection");
            let target = if range {
                let start = if midi { 30_013 } else { 1_013 };
                BounceTarget::Range(
                    hex_project::TimeRange::new(vec![track], start, start + 10_007).unwrap(),
                )
            } else {
                BounceTarget::Track(track)
            };
            let (_, bounced) = session
                .bounce_arrangement(
                    target,
                    !range,
                    &folder.0,
                    &hex_export::ExportControl::default(),
                    Some(&revision),
                )
                .unwrap();
            let (actual, expected) = if range {
                let audio = &session.project().tracks()[1].clips()[0];
                (
                    session.audio(audio.asset()).unwrap().frames().to_vec(),
                    expected[audio.start() as usize..(audio.start() + audio.duration()) as usize]
                        .to_vec(),
                )
            } else {
                (pcm(bounced, 80_000), expected)
            };
            let error = actual
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(error < 1e-5, "directed source survives bounce: {error}");
        }
    }
}

#[path = "tests/grouping.rs"]
mod grouping;
#[path = "tests/streamed.rs"]
mod streamed;

#[test]
fn clipped_thirty_second_delay_tail_adopts_media_and_undo_redo_as_one_edit() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    let folder = Folder::new();
    let rate = 8_000;
    let (mut session, track, _clip) = fixture_at_rate(rate);
    let class = hex_engine::NativeDeviceClass::Effect;
    let mut state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.delay"),
    )
    .unwrap();
    for (id, value) in [
        (1, ParameterValue::Float(1000.0)),
        (2, ParameterValue::Float(0.95)),
        (3, ParameterValue::Float(0.5)),
        (5, ParameterValue::Bool(false)),
        (6, ParameterValue::Bool(false)),
        (15, ParameterValue::Bool(false)),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            ParameterId::from_static(id),
            value,
        )
        .unwrap();
    }
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let original = session.project().snapshot();
    let revision = session.compile_prepared_revision(None).unwrap();
    session
        .bounce_arrangement(
            BounceTarget::Track(track),
            true,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let bounced = session.project().snapshot();
    let audio = &session.project().track(track).unwrap().clips()[0];
    let expected_frames = u64::from(rate) / 2 + u64::from(rate) * 30;
    assert_eq!(audio.duration(), expected_frames);
    let samples = session.audio(audio.asset()).unwrap().frames();
    assert_eq!(samples.len() as u64, expected_frames);
    assert!(
        samples[samples.len() - 256..]
            .iter()
            .flatten()
            .any(|s| s.abs() > 1e-6),
        "delay is still sounding at the cutoff"
    );
    let project_path = folder.0.join("capped.hex");
    hex_storage::save_new(&project_path, session.project()).unwrap();
    assert_eq!(
        hex_storage::load(&project_path).unwrap().snapshot(),
        bounced
    );
    assert!(session.undo());
    assert_eq!(session.project().snapshot().tracks, original.tracks);
    assert!(session.redo());
    assert_eq!(session.project().snapshot().tracks, bounced.tracks);
}

fn console_fixture(folder: &Folder, arrangement: bool) -> (Session, TrackId, Vec<ClipId>) {
    let (mut session, track, midi) = fixture_at_rate(8_000);
    session.command(Command::Gain { track, gain: 0.7 }).unwrap();
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(6.0)
    .unwrap();
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: effect.clone(),
        })
        .unwrap();
    session
        .command(Command::InsertMainDevice {
            before: None,
            state: effect.with_output_gain_db(-6.0).unwrap(),
        })
        .unwrap();
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "midi scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(notes) = session
        .command(Command::CopyArrangementMidiToSession {
            source: midi,
            scene,
        })
        .unwrap()
    else {
        panic!("cell")
    };
    session
        .command(Command::SetSessionMidiClipSource {
            clip: notes,
            source_length: BeatTime::ONE,
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::ONE,
            looping: arrangement,
        })
        .unwrap();
    session
        .command(Command::RenameClip {
            clip: notes,
            name: "CaseSensitive MIDI".into(),
        })
        .unwrap();
    if !arrangement {
        session
            .command(Command::SetClipActive {
                clip: notes,
                active: false,
            })
            .unwrap();
    }
    let source = folder.0.join("original-console.wav");
    let samples: Vec<_> = (0..64)
        .map(|i| [i as f32 / 256.0, -(i as f32) / 128.0])
        .collect();
    grouping::write_fixture(&source, 8_000, &samples);
    let decoded = hex_media::decode_wav_file(&source, hex_media::Limits::new(10000, 1000)).unwrap();
    let (created, _) = session
        .import_prepared_audio_clip_revision(
            PreparedImport::new(decoded).unwrap(),
            track,
            12_000,
            None,
        )
        .unwrap();
    let Created::Imported { clip: audio, .. } = created else {
        panic!("audio")
    };
    let mut cells = vec![notes];
    for looping in [false, true] {
        let Created::Scene(scene) = session
            .command(Command::AddScene {
                name: format!("audio {looping}"),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(id) = session
            .command(Command::CopyArrangementAudioClipToSession {
                source: audio,
                scene,
            })
            .unwrap()
        else {
            panic!("cell")
        };
        session
            .command(Command::SetAudioClipGain {
                clip: id,
                millidb: -3000,
            })
            .unwrap();
        if looping {
            session
                .command(Command::SetSessionAudioClipLoop {
                    clip: id,
                    loop_range: Some(hex_project::SessionAudioLoop::new(10, 30).unwrap()),
                })
                .unwrap();
            session
                .command(Command::SetClipActive {
                    clip: id,
                    active: false,
                })
                .unwrap();
        }
        cells.push(id);
    }
    let Created::Scene(stop) = session
        .command(Command::AddScene {
            name: "stop".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    session
        .command(Command::SetSessionStopCell { track, scene: stop })
        .unwrap();
    if !arrangement {
        session.command(Command::RemoveMidiClip(midi)).unwrap();
        session.command(Command::RemoveClip(audio)).unwrap();
    }
    (session, track, cells)
}
fn console_pcm(session: &Session, clip: ClipId, frames: usize) -> Vec<hex_engine::Frame> {
    let mut snapshot = session.project().snapshot();
    for cell in &mut snapshot.session.cells {
        match &mut cell.content {
            hex_project::SessionCellContentSnapshot::Midi(c) if c.id == clip.value() => {
                c.active = true
            }
            hex_project::SessionCellContentSnapshot::Audio(c) if c.id == clip.value() => {
                c.active = true
            }
            _ => {}
        }
    }
    let project = Project::from_snapshot(snapshot).unwrap();
    let waveforms = std::cell::RefCell::new(session.audio_clip_waveforms().unwrap());
    let revision = Arc::new(
        crate::prepare_with_waveforms(
            &project,
            &session.media_bank(),
            session.sine_table(),
            None,
            &session.audio_transforms,
            &waveforms,
        )
        .unwrap(),
    );
    let mut runtime = hex_engine::SessionPlayback::new(revision).unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut samples = vec![[0.0; 2]; frames];
    for chunk in samples.chunks_mut(137) {
        runtime.render(chunk).unwrap();
    }
    samples
}
#[test]
fn whole_track_bounce_replaces_every_console_source_keeps_loops_cells_and_atomic_history() {
    for (arrangement, shared) in [(false, false), (true, false), (false, true), (true, true)] {
        let folder = Folder::new();
        let (mut session, track, cells) = console_fixture(&folder, arrangement);
        let source = session.project().track(track).unwrap().source_id().unwrap();
        let contributor = shared.then(|| {
            let Created::Track(id) = session
                .command(Command::AddMidiTrack {
                    name: "retained contributor".into(),
                })
                .unwrap()
            else {
                panic!("contributor")
            };
            session
                .command(Command::BindTrackSource {
                    track: id,
                    source: Some(source),
                })
                .unwrap();
            id
        });
        let original = session.project().clone();
        let original_bytes = fs::read(folder.0.join("original-console.wav")).unwrap();
        if shared {
            let error = session
                .bounce_arrangement(
                    BounceTarget::Track(track),
                    true,
                    &folder.0,
                    &hex_export::ExportControl::default(),
                    None,
                )
                .unwrap_err();
            assert!(error.contains("linked tracks"), "{error}");
            assert_eq!(session.project(), &original);
            assert_eq!(
                fs::read(folder.0.join("original-console.wav")).unwrap(),
                original_bytes
            );
            assert_eq!(
                session
                    .project()
                    .track(contributor.unwrap())
                    .unwrap()
                    .source_id(),
                Some(source)
            );
            continue;
        }
        let plan = original
            .plan_arrangement_bounce(&BounceTarget::Track(track), true)
            .unwrap();
        assert_eq!(plan.len(), 3 + usize::from(arrangement));
        let expected:Vec<_>=cells.iter().map(|id| {
            let item=plan.iter().find(|item|matches!(item.source,hex_project::BounceSource::Console{clip,..} if clip==*id)).unwrap();
            (*id,item.end as usize,console_pcm(&session,*id,item.end as usize))
        }).collect();
        let (_, revision) = session
            .bounce_arrangement(
                BounceTarget::Track(track),
                true,
                &folder.0,
                &hex_export::ExportControl::default(),
                None,
            )
            .unwrap();
        let owner = session.project().track(track).unwrap();
        assert!(owner.instrument().is_none() && owner.devices().is_empty());
        let retained = session.project().native_source(source).unwrap();
        assert_eq!(retained.receiver(), None);
        assert_eq!(
            retained.instrument(),
            original.native_source(source).unwrap().instrument()
        );
        if let Some(contributor) = contributor {
            assert_eq!(
                session.project().track(contributor).unwrap().source_id(),
                Some(source)
            );
        }
        assert!(owner.midi_clips().is_empty());
        assert_eq!(owner.clips().len(), usize::from(arrangement));
        assert_eq!(owner.gain(), original.track(track).unwrap().gain());
        assert_eq!(session.project().scenes(), original.scenes());
        assert_eq!(session.project().main_devices(), original.main_devices());
        for (clip, frames, expected) in expected {
            let c = session.project().session_audio_clip(clip).unwrap();
            let before = original
                .session_cells()
                .iter()
                .find(|cell| match cell.content() {
                    hex_project::SessionCellContent::Midi(c) => c.id() == clip,
                    hex_project::SessionCellContent::Audio(c) => c.id() == clip,
                    _ => false,
                })
                .unwrap();
            match before.content() {
                hex_project::SessionCellContent::Midi(old) => {
                    assert_eq!(
                        (c.id(), c.name(), c.active(), c.flavor()),
                        (old.id(), old.name(), old.active(), old.flavor())
                    );
                    assert_eq!(
                        c.loop_range(),
                        if old.source().looping() {
                            Some(hex_project::SessionAudioLoop::new(4000, 8000).unwrap())
                        } else {
                            None
                        }
                    );
                }
                hex_project::SessionCellContent::Audio(old) => {
                    assert_eq!(
                        (c.id(), c.name(), c.active(), c.flavor()),
                        (old.id(), old.name(), old.active(), old.flavor())
                    );
                    if old.loop_range().is_some() {
                        assert_eq!(
                            c.loop_range(),
                            Some(hex_project::SessionAudioLoop::new(30, 50).unwrap())
                        );
                    } else {
                        assert!(c.loop_range().is_none());
                    }
                }
                _ => panic!("source"),
            }
            assert_eq!(c.gain(), hex_project::ClipGain::UNITY);
            assert!(c.source_end() >= frames as u64);
            let actual = console_pcm(&session, clip, frames);
            let maximum = actual
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(maximum < 1e-5, "cell {clip:?} differs by {maximum}");
            if let Some(range) = c.loop_range() {
                let actual = console_pcm(
                    &session,
                    clip,
                    (range.end() + 2 * (range.end() - range.start())) as usize,
                );
                let period = (range.end() - range.start()) as usize;
                assert_eq!(
                    &actual[range.start() as usize..range.end() as usize],
                    &actual[range.end() as usize..range.end() as usize + period]
                );
            }
        }
        assert_eq!(
            fs::read(folder.0.join("original-console.wav")).unwrap(),
            original_bytes
        );
        let after = session.project().clone();
        assert!(session.editor.next_history_requires_stopped_bounce(false));
        assert!(session.undo());
        let mut restored = session.project().snapshot();
        let initial = original.snapshot();
        assert!(
            restored.next_id >= initial.next_id,
            "history retains the allocator high-water mark"
        );
        restored.next_id = initial.next_id;
        assert_eq!(restored, initial);
        assert!(session.redo());
        assert_eq!(session.project(), &after);
        let saved = folder.0.join("console-bounce.hex");
        hex_storage::save_new(&saved, session.project()).unwrap();
        let loaded = hex_storage::load(&saved).unwrap();
        assert_eq!(loaded, after);
        let decoded = loaded
            .assets()
            .map(|a| {
                (
                    a.id(),
                    hex_media::decode_wav_file(
                        a.path(),
                        hex_media::Limits::new(64 * 1024 * 1024 + 44, 8_000_000),
                    )
                    .unwrap(),
                )
            })
            .collect();
        let reopened = Session::from_loaded(loaded, 32, 2_000_000, decoded).unwrap();
        for clip in cells {
            assert_eq!(
                console_pcm(&reopened, clip, 10_000),
                console_pcm(&session, clip, 10_000)
            );
        }
        assert_eq!(
            pcm(revision, 20_000),
            pcm(session.compile_prepared_revision(None).unwrap(), 20_000)
        );
    }
}

#[test]
fn whole_track_console_second_output_cancel_and_final_publication_fail_without_partial_changes() {
    for (failure, shared) in
        (0..3).flat_map(|failure| [false, true].map(|shared| (failure, shared)))
    {
        let folder = Folder::new();
        let (mut session, track, _) = console_fixture(&folder, true);
        if shared {
            let source = session.project().track(track).unwrap().source_id().unwrap();
            let Created::Track(receiver) = session
                .command(Command::AddMidiTrack {
                    name: "receiver".into(),
                })
                .unwrap()
            else {
                panic!("receiver")
            };
            session
                .command(Command::BindTrackSource {
                    track: receiver,
                    source: Some(source),
                })
                .unwrap();
            session
                .command(Command::SetNativeSourceReceiver {
                    source,
                    track: Some(receiver),
                })
                .unwrap();
        }
        let original = session.project().clone();
        let resident = session.resident_frames;
        let media = session.media.len();
        let bytes = fs::read(folder.0.join("original-console.wav")).unwrap();
        let control = hex_export::ExportControl::default();
        let target = BounceTarget::Track(track);
        let mut paths = Vec::new();
        let result: Result<(Created, ()), String> = session.bounce_arrangement_using(
            target.clone(),
            true,
            &folder.0,
            &control,
            |session, project, _| {
                let waveforms = std::cell::RefCell::new(
                    session.audio_clip_waveforms().map_err(|e| e.to_string())?,
                );
                crate::prepare_with_waveforms(
                    project,
                    &session.media_bank(),
                    session.sine_table(),
                    None,
                    &session.audio_transforms,
                    &waveforms,
                )
                .map(Arc::new)
                .map_err(|e| e.to_string())
            },
            |path, revision, item, tails, control| {
                if failure == 0 && paths.len() == 1 {
                    return Err("second console output failed".into());
                }
                let frames = hex_export::render_arrangement_bounce_new(
                    path,
                    revision,
                    None,
                    item.start,
                    item.end - item.start,
                    tails,
                    control,
                )
                .map_err(|e| e.to_string())?
                .frames;
                paths.push(path.to_owned());
                if failure == 1 && paths.len() == 2 {
                    assert!(control.cancel());
                }
                Ok(frames)
            },
            |session, prepared, files| {
                session
                    .adopt_arrangement_bounce_using(
                        target,
                        true,
                        hex_project::BounceAdoption::NewTracks,
                        files,
                        prepared,
                        None,
                        |_| Err::<(), _>(SessionError::Invalid("final publication refused")),
                    )
                    .map_err(|e| e.to_string())
            },
        );
        assert!(result.is_err());
        assert_eq!(
            paths.len(),
            if failure == 0 {
                1
            } else if failure == 1 {
                2
            } else {
                4
            }
        );
        assert!(paths.iter().all(|p| !p.exists()));
        assert_eq!(session.project(), &original);
        assert_eq!(session.resident_frames, resident);
        assert_eq!(session.media.len(), media);
        assert_eq!(
            fs::read(folder.0.join("original-console.wav")).unwrap(),
            bytes
        );
    }
}

#[test]
fn shared_console_bounce_refuses_other_authored_midi_without_changing_project_or_files() {
    for console_midi in [false, true] {
        let folder = Folder::new();
        let (mut session, receiver, _) = console_fixture(&folder, true);
        let source = session
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(peer) = session
            .command(Command::AddMidiTrack {
                name: "peer".into(),
            })
            .unwrap()
        else {
            panic!("peer")
        };
        session
            .command(Command::BindTrackSource {
                track: peer,
                source: Some(source),
            })
            .unwrap();
        if console_midi {
            // Preserve the exact Console source and identifiers on a second track.
            let mut snapshot = session.project().snapshot();
            let cell = snapshot
                .session
                .cells
                .iter_mut()
                .find(|cell| {
                    matches!(
                        cell.content,
                        hex_project::SessionCellContentSnapshot::Midi(_)
                    )
                })
                .unwrap();
            cell.track = peer.value();
            session.editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
        } else {
            let Created::Clip(_) = session
                .command(Command::PlaceMidiClip {
                    track: peer,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("midi")
            };
        }
        let before = session.project().clone();
        let entries = fs::read_dir(&folder.0).unwrap().count();
        let error = session
            .bounce_arrangement(
                BounceTarget::Track(receiver),
                true,
                &folder.0,
                &hex_export::ExportControl::default(),
                None,
            )
            .unwrap_err();
        assert!(
            error.contains("another shared-source contributor has midi"),
            "{error}"
        );
        assert_eq!(session.project(), &before);
        assert_eq!(fs::read_dir(&folder.0).unwrap().count(), entries);
    }
}

#[test]
fn retired_clip_and_range_in_place_intents_refuse_before_render_or_media_work() {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    let before = session.project().clone();
    let history = (session.can_undo(), session.can_redo());
    for target in [
        BounceTarget::Clips(vec![clip]),
        BounceTarget::Range(hex_project::TimeRange::new(vec![track], 12_013, 36_007).unwrap()),
    ] {
        let result: Result<(), String> = session.bounce_arrangement_using(
            target,
            true,
            &folder.0,
            &hex_export::ExportControl::default(),
            |_, _, _| -> Result<(), String> { panic!("retired intent must not prepare") },
            |_, _, _, _, _| panic!("retired intent must not render"),
            |_, _, _| panic!("retired intent must not commit"),
        );
        assert!(result.unwrap_err().contains("complete track"));
        assert_eq!(session.project(), &before);
        assert_eq!((session.can_undo(), session.can_redo()), history);
        assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 0);
    }
}

#[test]
fn isolated_bounce_admits_projects_with_automatic_names_on_dropped_tracks() {
    let (mut session, track, clip) = fixture();
    let Created::Track(other) = session
        .command(Command::InsertDefaultTrack { before_track: None })
        .unwrap()
    else {
        panic!("default track")
    };
    let original = session.project().clone();
    assert!(original.track(other).unwrap().name_is_automatic());
    for target in [
        BounceTarget::Clips(vec![clip]),
        BounceTarget::Range(hex_project::TimeRange::new(vec![track], 30_013, 50_007).unwrap()),
    ] {
        let plan = original.plan_arrangement_bounce(&target, false).unwrap();
        let projected = isolated(&original, &target, &plan[0], false).unwrap();
        assert!(projected.track(other).is_none());
        assert!(projected.snapshot().automatic_track_names.is_empty());
    }
    assert_eq!(session.project(), &original);
}

#[path = "tests/group_ranges.rs"]
mod group_ranges;
