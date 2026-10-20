use hex_media::{Limits, decode_wav_file};
use hex_project::{AssetId, BeatTime, ClipId, Command, Created, Editor, Project, Tempo, TrackId};
use hex_session::{PreparedImport, Session};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hex-session-tests-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wav(sample_rate: u32, frames: &[[i16; 2]]) -> Vec<u8> {
    let data_bytes = (frames.len() * 4) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in frames {
        for sample in frame {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    bytes
}

fn prepare(directory: &TempDir, name: &str, rate: u32, data: &[[i16; 2]]) -> PreparedImport {
    let path = directory.path().join(name);
    fs::write(&path, wav(rate, data)).expect("write audio fixture");
    PreparedImport::new(decode_wav_file(&path, Limits::new(65_536, 8_192)).expect("decode fixture"))
        .expect("prepare fixture")
}

fn imported_ids(result: Created) -> (TrackId, AssetId, ClipId) {
    match result {
        Created::Imported { track, asset, clip } => (track, asset, clip),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn prepared_import_compiles_exact_audio_and_waveform_summary() {
    let directory = TempDir::new();
    let source = [[8_192, -8_192], [16_384, -16_384], [-32_768, 32_767]];
    let mut session = Session::new(48_000, 8, 32).expect("new session");
    let ids = imported_ids(
        session
            .import(prepare(&directory, "Audio-MiXeD.wav", 48_000, &source))
            .expect("import audio"),
    );
    let (track, asset, clip) = ids;
    assert_eq!(
        session.project().track(track).expect("track").name(),
        "imported audio"
    );
    assert_eq!(session.project().clip(clip).expect("clip").asset(), asset);
    assert_eq!(session.resident_frames(), 3);
    assert_eq!(session.resident_waveform_bins(), 6);
    assert_eq!(
        session.resident_waveform_bytes(),
        6 * std::mem::size_of::<[hex_media::waveform::Peak; 2]>()
    );
    let waveform = session.waveform(asset).expect("cached waveform");
    assert_eq!(
        session.audio(asset).expect("resident audio").sample_rate(),
        48_000
    );
    assert_eq!(waveform.source_frames, 3);
    assert_eq!(waveform.bins.len(), 3);
    assert_eq!(waveform.bins[0][0].min, 0.25);
    assert_eq!(waveform.bins[2][0].min, -1.0);
    assert_eq!(waveform.bins[2][1].max, 32_767.0 / 32_768.0);

    let playback = session.compile().expect("compile session");
    let mut renderer = hex_engine::Renderer::new(&playback);
    renderer.play();
    let mut output = [[0.0; 2]; 3];
    renderer.render(&mut output).expect("render imported audio");
    assert_eq!(
        output,
        [[0.25, -0.25], [0.5, -0.5], [-1.0, 32_767.0 / 32_768.0]]
    );
}

#[test]
fn rejected_unlaunched_session_cell_preserves_project_history_ids_and_prepared_state() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "instrument".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        _ => panic!("track result"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "unlaunched".into(),
        })
        .expect("scene")
    {
        Created::Scene(id) => id,
        _ => panic!("scene result"),
    };
    let clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "dense".into(),
            source_length: BeatTime::from_quarters(4).expect("source length"),
        })
        .expect("clip")
    {
        Created::SessionClip(id) => id,
        _ => panic!("clip result"),
    };
    for pitch in 0..64_u8 {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).expect("gate"),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .expect("admitted note");
    }
    let mut session = Session::from_prepared(editor.project().clone(), 32, 0, Vec::new())
        .expect("session with no media");
    let mut prepared = session
        .compile_prepared_revision(None)
        .expect("prepare all Session cells");

    let created_scene = session
        .command_prepared_revision(
            Command::AddScene {
                name: "redo".into(),
            },
            Some(&prepared),
        )
        .expect("prepared scene edit");
    prepared = created_scene.1;
    let undone = session
        .undo_prepared_revision(Some(&prepared))
        .expect("prepared undo")
        .expect("scene undo exists");
    prepared = undone;
    assert!(session.can_redo());

    let before = session.project().snapshot();
    let rejected = session.command_prepared_revision(
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).expect("gate"),
            pitch: 64,
            channel: 0,
            velocity: 100,
        },
        Some(&prepared),
    );
    assert!(rejected.is_err());
    assert_eq!(session.project().snapshot(), before);
    assert!(session.can_redo());
    assert_eq!(
        session
            .project()
            .session_midi_clip(clip)
            .unwrap()
            .source()
            .notes()
            .len(),
        64
    );

    let before_legacy = session.project().snapshot();
    let extra_note = Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(1).expect("gate"),
        pitch: 64,
        channel: 0,
        velocity: 100,
    };
    assert!(session.command(extra_note.clone()).is_err());
    assert!(
        session
            .command_prepared_with_midi_preview(extra_note)
            .is_err()
    );
    assert_eq!(session.project().snapshot(), before_legacy);
    assert!(session.can_redo());

    let mut invalid_editor = Editor::new(session.project().clone(), 8).expect("invalid editor");
    invalid_editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).expect("gate"),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .expect("canonical model permits 65 source notes");
    let mut invalid_session =
        Session::from_prepared(invalid_editor.project().clone(), 8, 0, Vec::new())
            .expect("load canonical but engine-dense source");
    assert!(invalid_session.compile().is_err());
    assert!(invalid_session.compile_with_midi_preview().is_err());
    assert!(
        invalid_session
            .command(Command::Gain { track, gain: 0.75 })
            .is_err()
    );
}

#[test]
fn legacy_preview_history_apis_prepare_session_inventory_before_history_moves() {
    let mut session = Session::new(48_000, 8, 0).expect("session");
    let (_, project, preview) = session
        .command_prepared_with_midi_preview(Command::AddScene {
            name: "scene".into(),
        })
        .expect("prepared scene");
    assert_eq!(project.sample_rate(), 48_000);
    assert!(preview.clips().is_empty());
    let undone = session
        .undo_prepared_with_midi_preview()
        .expect("prepared undo")
        .expect("scene undo");
    assert!(undone.1.clips().is_empty());
    let redone = session
        .redo_prepared_with_midi_preview()
        .expect("prepared redo")
        .expect("scene redo");
    assert!(redone.1.clips().is_empty());
}

#[test]
fn failed_prepared_import_preserves_project_ids_and_resident_media() {
    let directory = TempDir::new();
    let existing_path = directory.path().join("existing.wav");
    let existing_prepared = prepare(&directory, "existing.wav", 8_000, &[[3, -3]]);
    let mut editor = Editor::new(Project::new(8_000).expect("project"), 8).expect("editor");
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "existing audio".into(),
            path: existing_path,
            source_sample_rate: 8_000,
            source_frames: 1,
            playback_frames: 1,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .expect("add existing asset metadata")
    else {
        panic!("import result expected")
    };
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("add instrument track")
    else {
        panic!("track expected")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(1).expect("clip length"),
        })
        .expect("place midi clip")
    else {
        panic!("clip expected")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 4).expect("note gate"),
            pitch: 120,
            channel: 0,
            velocity: 100,
        })
        .expect("author above-nyquist note");
    let mut session = Session::from_prepared(
        editor.project().clone(),
        8,
        16,
        vec![(asset, existing_prepared)],
    )
    .expect("session with valid metadata and pending engine validation");
    let before = session.project().snapshot();
    let allocator = before.next_id;
    let before_frames = session.resident_frames();
    let before_bins = session.resident_waveform_bins();
    let before_waveform = session.waveform(asset).expect("resident waveform");

    let error = session
        .import_with_midi_preview(prepare(&directory, "candidate.wav", 8_000, &[[1, -1]]))
        .expect_err("candidate graph contains an unrenderable note");
    assert!(
        error
            .to_string()
            .contains("midi note cannot be prepared by its instrument")
    );
    assert_eq!(session.project().snapshot(), before);
    assert_eq!(session.project().snapshot().next_id, allocator);
    assert_eq!(session.resident_frames(), before_frames);
    assert_eq!(session.resident_waveform_bins(), before_bins);
    assert!(
        (session.waveform(asset).expect("resident waveform remains"))
            .shares_owner_with(&before_waveform)
    );
    assert_eq!(session.project().track(track).expect("track").gain(), 1.0);
}

#[test]
fn split_preserves_trimmed_one_shot_render_order_across_blocks_and_persistence() {
    let directory = TempDir::new();
    let source: Vec<[i16; 2]> = (0..16)
        .map(|index| {
            let left = (index as i16 + 1) * 1_500;
            let right = -(index as i16 + 1) * 1_000;
            [left, right]
        })
        .collect();
    let mut session = Session::new(48_000, 16, 64).expect("session");
    let (_, asset, original) = imported_ids(
        session
            .import(prepare(&directory, "split.wav", 48_000, &source))
            .expect("import source"),
    );
    let track = session.project().tracks()[0].id();
    session
        .command(Command::TrimClip {
            clip: original,
            start: 2,
            duration: 15,
            source_offset: 1,
            source_length: 9,
        })
        .expect("trim one-shot");
    session
        .command(Command::PlaceClip {
            track,
            asset,
            start: 5,
            duration: 8,
            source_offset: 3,
            source_length: 8,
            looping: false,
        })
        .expect("place overlapping clip");
    let before = session.project().clone();
    let baseline = session.compile().expect("compile baseline");

    let right = match session
        .command(Command::SplitClip {
            clip: original,
            frame: 7,
        })
        .expect("split at cursor")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let split_project = session.project().clone();
    let right_clip = split_project.clip(right).expect("right clip");
    assert_eq!(right_clip.start(), 7);
    assert_eq!(right_clip.duration(), 10);
    assert_eq!(right_clip.source_offset(), 6);
    assert_eq!(right_clip.source_length(), 4);

    for block_size in [1_usize, 3, 7, 16] {
        let mut original_renderer = hex_engine::Renderer::new(&baseline);
        original_renderer.play();
        let split_playback = session.compile().expect("compile split");
        let mut split_renderer = hex_engine::Renderer::new(&split_playback);
        split_renderer.play();
        let original_frames = render_blocks(&mut original_renderer, 24, block_size);
        let split_frames = render_blocks(&mut split_renderer, 24, block_size);
        assert_eq!(split_frames, original_frames, "block size {block_size}");
    }

    assert!(session.undo());
    let mut undone_snapshot = before.snapshot();
    undone_snapshot.next_id = split_project.snapshot().next_id;
    let expected_undo = hex_project::Project::from_snapshot(undone_snapshot)
        .expect("valid undo snapshot with retained ID highwater");
    assert_eq!(session.project(), &expected_undo);
    assert!(session.redo());
    assert_eq!(session.project(), &split_project);

    let saved_path = directory.path().join("Split.hex");
    hex_storage::save_new(&saved_path, session.project()).expect("save split project");
    let loaded = hex_storage::load(&saved_path).expect("load split project");
    assert_eq!(loaded, split_project);
}

#[test]
fn offline_export_renders_repeated_midi_from_the_prepared_session_graph() {
    let directory = TempDir::new();
    let path = directory.path().join("looped-midi.wav");
    let mut session = Session::new(48_000, 12, 0).expect("session");
    let track = match session
        .command(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("instrument track")
    {
        Created::Track(id) => id,
        _ => panic!("track expected"),
    };
    session
        .command(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(4).unwrap(),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .expect("tempo point");
    let clip = match session
        .command(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .expect("midi clip")
    {
        Created::Clip(id) => id,
        _ => panic!("clip expected"),
    };
    session
        .command(Command::SetMidiClipSource {
            clip,
            source_length: BeatTime::from_quarters(8).unwrap(),
            source_start: BeatTime::from_quarters(1).unwrap(),
            loop_start: BeatTime::from_quarters(2).unwrap(),
            loop_end: BeatTime::from_quarters(4).unwrap(),
            looping: true,
            length: BeatTime::from_ratio(13, 2).unwrap(),
        })
        .expect("loop source");
    session
        .command(Command::AddMidiNote {
            clip,
            start: BeatTime::from_ratio(5, 2).unwrap(),
            duration: BeatTime::from_ratio(5, 2).unwrap(),
            pitch: 62,
            channel: 0,
            velocity: 110,
        })
        .expect("looped note");

    let graph = session.compile().expect("prepared graph");
    let frames = graph.audible_end_frame().expect("looped output");
    assert_eq!(frames, 218_400);
    hex_export::render_new(&path, &graph, 0, frames).expect("export looped midi");
    let decoded = decode_wav_file(
        &path,
        Limits::new(2_000_000, usize::try_from(frames).unwrap()),
    )
    .expect("decode export");
    assert_eq!(decoded.frames(), frames as usize);
    let samples = decoded.audio().frames();
    assert!(
        samples[84_000..144_000]
            .iter()
            .any(|frame| frame[0].abs() + frame[1].abs() > 0.01)
    );
    assert!(
        samples[168_000..216_000]
            .iter()
            .any(|frame| frame[0].abs() + frame[1].abs() > 0.01)
    );
}

fn render_blocks(
    renderer: &mut hex_engine::Renderer<'_>,
    frames: usize,
    block_size: usize,
) -> Vec<hex_engine::Frame> {
    let mut output = vec![[0.0; 2]; frames];
    for chunk in output.chunks_mut(block_size) {
        renderer.render(chunk).expect("render block");
    }
    output
}

#[test]
fn edits_undo_and_redo_compile_against_the_same_resident_media() {
    let directory = TempDir::new();
    let samples = [
        [8_192, 8_192],
        [16_384, 16_384],
        [24_576, 24_576],
        [32_767, 32_767],
    ];
    let mut session = Session::new(48_000, 16, 64).expect("new session");
    let (track, asset, clip) = imported_ids(
        session
            .import(prepare(&directory, "edit.wav", 48_000, &samples))
            .expect("import"),
    );
    let second_track = match session
        .command(Command::AddTrack {
            name: "second".into(),
        })
        .expect("add track")
    {
        Created::Track(id) => id,
        result => panic!("unexpected result: {result:?}"),
    };
    session
        .command(Command::MoveClip {
            clip,
            track: second_track,
            start: 2,
        })
        .expect("move clip");
    session
        .command(Command::TrimClip {
            clip,
            start: 2,
            duration: 6,
            source_offset: 1,
            source_length: 2,
        })
        .expect("trim clip");
    session
        .command(Command::Gain {
            track: second_track,
            gain: 0.5,
        })
        .expect("change gain");

    let playback = session.compile().expect("compile edited session");
    let mut renderer = hex_engine::Renderer::new(&playback);
    renderer.play();
    let mut output = [[0.0; 2]; 6];
    renderer.render(&mut output).expect("render edited session");
    assert_eq!(
        output,
        [
            [0.0; 2], [0.0; 2], [0.25; 2], [0.375; 2], [0.0; 2], [0.0; 2]
        ]
    );

    assert!(session.undo()); // gain
    assert!(session.undo()); // trim
    let playback = session.compile().expect("compile undone edit");
    let mut renderer = hex_engine::Renderer::new(&playback);
    renderer.play();
    let mut restored = [[0.0; 2]; 6];
    renderer.render(&mut restored).expect("render undone edit");
    assert_eq!(
        restored,
        [
            [0.0; 2],
            [0.0; 2],
            [0.25; 2],
            [0.5; 2],
            [0.75; 2],
            [32_767.0 / 32_768.0; 2]
        ]
    );
    assert!(session.redo());
    assert!(session.redo());
    assert_eq!(
        session
            .project()
            .track(track)
            .expect("original track")
            .clips()
            .len(),
        0
    );
    assert!(session.audio(asset).is_some());
    assert!(session.waveform(asset).is_some());
    assert!(session.compile().is_ok());
}

#[test]
fn deleting_asset_keeps_resident_media_for_undo_and_recompile() {
    let directory = TempDir::new();
    let mut session = Session::new(48_000, 8, 16).expect("new session");
    let (track, asset, _) = imported_ids(
        session
            .import(prepare(
                &directory,
                "remove.wav",
                48_000,
                &[[4_096, -4_096]],
            ))
            .expect("import"),
    );
    session
        .command(Command::RemoveTrack(track))
        .expect("remove track");
    session
        .command(Command::RemoveAsset(asset))
        .expect("remove unused asset metadata");
    assert!(session.project().asset(asset).is_none());
    assert_eq!(session.resident_frames(), 1);
    assert_eq!(session.resident_waveform_bins(), 1);
    assert!(session.audio(asset).is_some());
    assert!(session.waveform(asset).is_some());

    assert!(session.undo()); // asset
    assert!(session.undo()); // track and clip
    let playback = session.compile().expect("compile restored media reference");
    let mut renderer = hex_engine::Renderer::new(&playback);
    renderer.play();
    let mut output = [[0.0; 2]; 1];
    renderer.render(&mut output).expect("render restored media");
    assert_eq!(output, [[0.125, -0.125]]);
}

#[test]
fn undo_retains_decoded_frames_against_the_session_limit() {
    let directory = TempDir::new();
    let samples = [[1_024, -1_024]; 3];
    let mut session = Session::new(48_000, 8, 3).expect("new session");
    let (_, asset, _) = imported_ids(
        session
            .import(prepare(&directory, "retained.wav", 48_000, &samples))
            .expect("import"),
    );
    assert!(session.undo());
    assert!(session.project().tracks().is_empty());
    assert_eq!(session.resident_frames(), 3);
    assert!(session.audio(asset).is_some());
    assert!(
        session
            .import(prepare(&directory, "would-exceed.wav", 48_000, &[[0, 0]]))
            .is_err()
    );
    assert!(session.can_redo());
    assert!(session.redo());
    assert!(session.compile().is_ok());
}

#[test]
fn waveform_summary_caps_resolution_and_keeps_bucket_extrema() {
    let directory = TempDir::new();
    let mut samples = vec![[0_i16; 2]; 2_048];
    samples[1] = [16_384, -8_192];
    samples[1_025] = [-16_384, 8_192];
    let mut session = Session::new(48_000, 2, 2_048).expect("new session");
    let (_, asset, _) = imported_ids(
        session
            .import(prepare(&directory, "waveform.wav", 48_000, &samples))
            .expect("import"),
    );
    let bins = &session.waveform(asset).expect("waveform").bins;
    assert_eq!(bins.len(), 2_048);
    assert_eq!(bins[1][0].max, 0.5);
    assert_eq!(bins[1][1].min, -0.25);
    assert_eq!(bins[1_025][0].min, -0.5);
    assert_eq!(bins[1_025][1].max, 0.25);
}

#[test]
fn rate_budget_and_raw_asset_edits_reject_without_changing_history() {
    let directory = TempDir::new();
    let mut session = Session::new(48_000, 8, 2).expect("new session");
    let track = match session
        .command(Command::AddTrack {
            name: "existing".into(),
        })
        .expect("add track")
    {
        Created::Track(id) => id,
        result => panic!("unexpected result: {result:?}"),
    };
    session
        .command(Command::Gain { track, gain: 0.5 })
        .expect("change gain");
    assert!(session.undo());
    let before = session.project().clone();

    assert!(
        session
            .command(Command::AddAsset {
                path: "bypass.wav".into(),
                sample_rate: 48_000,
                frames: 1,
            })
            .is_err()
    );
    assert!(
        session
            .command(Command::ImportAudioTrack {
                name: "bypass".into(),
                path: "bypass.wav".into(),
                sample_rate: 48_000,
                frames: 1,
            })
            .is_err()
    );
    assert!(
        session
            .import(prepare(&directory, "wrong-rate.wav", 44_100, &[[0, 0]]))
            .is_err()
    );
    assert!(
        session
            .import(prepare(
                &directory,
                "over-budget.wav",
                48_000,
                &[[0, 0], [1, 1], [2, 2]],
            ))
            .is_err()
    );
    assert_eq!(session.project(), &before);
    assert_eq!(session.resident_frames(), 0);
    assert!(session.can_redo());
    assert!(session.redo());
    assert_eq!(session.project().track(track).expect("track").gain(), 0.5);
}

#[test]
fn converted_import_preserves_source_metadata_and_uses_playback_frames_for_history() {
    let directory = TempDir::new();
    let path = directory.path().join("Source-MiXeD.wav");
    let samples: Vec<[i16; 2]> = (0..441).map(|index| [index * 30, -index * 20]).collect();
    fs::write(&path, wav(44_100, &samples)).expect("source");
    let decoded = decode_wav_file(&path, Limits::new(65_536, 441)).expect("decode");
    let prepared = PreparedImport::at_rate(
        decoded,
        48_000,
        hex_media::ResampleLimits {
            max_source_frames: 441,
            max_output_frames: 480,
        },
    )
    .expect("prepare conversion");
    assert_eq!(prepared.waveform().source_frames, 480);
    let mut session = Session::new(48_000, 8, 441 + 480).expect("original plus playback budget");
    let (_, asset, clip) = imported_ids(session.import(prepared).expect("import"));
    assert_eq!(session.source_audio(asset).unwrap().sample_rate(), 44_100);
    assert_eq!(session.source_audio(asset).unwrap().frames().len(), 441);
    let metadata = session.project().asset(asset).expect("metadata");
    assert_eq!(metadata.path(), path);
    assert_eq!(metadata.source_sample_rate(), 44_100);
    assert_eq!(metadata.source_frames(), 441);
    assert_eq!(metadata.playback_frames(), 480);
    assert_eq!(
        metadata.conversion_version(),
        hex_project::SINC_CONVERSION_V1
    );
    assert_eq!(
        session.project().clip(clip).expect("clip").source_length(),
        480
    );
    assert_eq!(session.resident_frames(), 441 + 480);
    let original = session
        .audio(asset)
        .expect("prepared PCM")
        .frames()
        .to_vec();
    assert!(session.undo());
    assert!(session.project().tracks().is_empty());
    assert_eq!(session.resident_frames(), 441 + 480);
    assert!(session.redo());
    let playback = session.compile().expect("compile");
    let mut renderer = hex_engine::Renderer::new(&playback);
    renderer.play();
    assert_eq!(render_blocks(&mut renderer, 480, 17), original);
    assert!(
        session
            .command(Command::ImportPreparedAudioTrack {
                name: "bypass".into(),
                path,
                source_sample_rate: 44_100,
                source_frames: 441,
                playback_frames: 480,
                conversion_version: hex_project::SINC_CONVERSION_V1,
            })
            .is_err()
    );
}

#[test]
fn ordered_instrument_candidate_prepares_and_roundtrips_history() {
    let mut session = Session::new(48_000, 16, 0).expect("session");
    let (track, _) = match session
        .command(Command::InsertTrackWithDevice {
            name: "chain".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("track")
    {
        Created::Track(track) => (track, ()),
        _ => panic!("track result"),
    };
    let utility = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .expect("utility default");
    let effect = match session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: utility.clone(),
        })
        .expect("first effect")
    {
        Created::Device(id) => id,
        _ => panic!("effect result"),
    };
    let second_effect = match session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: utility,
        })
        .expect("second effect")
    {
        Created::Device(id) => id,
        _ => panic!("effect result"),
    };
    let prepared = session
        .undo_prepared_revision(None)
        .expect("prepared undo")
        .expect("undo exists");
    assert!(session.can_redo());
    assert!(session.project().device(effect).is_some());
    assert!(session.project().device(second_effect).is_none());
    let before = session.project().snapshot();
    let (_, ordered) = session
        .command_prepared_revision(
            Command::MoveTrackDevice {
                device: session
                    .project()
                    .track(track)
                    .expect("track remains")
                    .instrument()
                    .expect("instrument")
                    .id(),
                track,
                before: None,
            },
            Some(&prepared),
        )
        .expect("ordered candidate prepares before history commits");
    assert_eq!(
        session
            .project()
            .track(track)
            .unwrap()
            .instrument_position(),
        Some(1)
    );
    assert!(
        !session.can_redo(),
        "accepted edit retires the old redo branch"
    );
    let moved = session.project().snapshot();
    let restored = session
        .undo_prepared_revision(Some(&ordered))
        .expect("ordered undo prepares")
        .expect("ordered undo exists");
    assert_eq!(session.project().snapshot(), before);
    assert!(session.can_redo());
    session
        .redo_prepared_revision(Some(&restored))
        .expect("ordered redo prepares")
        .expect("ordered redo exists");
    assert_eq!(session.project().snapshot(), moved);
}
