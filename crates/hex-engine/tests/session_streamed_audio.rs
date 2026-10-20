use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
use hex_engine::{
    Audio, BorrowedMediaPage, Frame, PlaybackAction, PlaybackCommand, PreparedRevision, SineTable,
    StreamSourceKey, StreamedSourceMetadata, compile::PreparedAudioSource, playback::Playback,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    AssetId, AssetSnapshot, BeatTime, ClipSnapshot, Command, Created, Editor, LoopRegion, Meter,
    MeterPoint, Project, SceneSnapshot, SessionAudioClipSnapshot, SessionCellContentSnapshot,
    SessionCellSnapshot, SessionSnapshot, Snapshot, Tempo, TempoPoint, TrackContentSnapshot,
    TrackSnapshot,
};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
const STREAM_ASSET: u64 = 1;
const SESSION_TRACK: u64 = 2;
const SCENE: u64 = 3;
const SESSION_CLIP: u64 = 4;
const BED_TRACK: u64 = 5;
const BED_ASSET: u64 = 7;

fn project(source_start: u64, source_end: u64) -> Project {
    Project::from_snapshot(Snapshot {
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        import_provenance: None,
        sample_rate: RATE,
        next_id: 10,
        assets: vec![
            AssetSnapshot {
                id: STREAM_ASSET,
                path: "session-source.wav".into(),
                source_sample_rate: RATE,
                source_frames: 8_192,
                playback_frames: 8_192,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
            AssetSnapshot {
                id: BED_ASSET,
                path: "resident-bed.wav".into(),
                source_sample_rate: RATE,
                source_frames: 32,
                playback_frames: 32,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
            AssetSnapshot {
                id: 9,
                path: "unused.wav".into(),
                source_sample_rate: RATE,
                source_frames: 32,
                playback_frames: 32,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
        ],
        tracks: vec![
            TrackSnapshot {
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: SESSION_TRACK,
                name: "session audio".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(Vec::new()),
                sends: Vec::new(),
            },
            TrackSnapshot {
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: BED_TRACK,
                name: "resident bed".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                    name: "clip".into(),
                    flavor: hex_project::ClipFlavor::default(),
                    edit: None,
                    gain: hex_project::ClipGain::UNITY,
                    id: 8,
                    asset: BED_ASSET,
                    start: 0,
                    duration: 12,
                    source_offset: 0,
                    source_length: 32,
                    looping: false,
                    fade: hex_project::ClipFade::neutral(12).expect("neutral fade"),
                    source_phase: 0,
                }]),
                sends: Vec::new(),
            },
        ],
        session: SessionSnapshot {
            scenes: vec![SceneSnapshot {
                id: SCENE,
                name: "stream scene".into(),
            }],
            cells: vec![SessionCellSnapshot {
                track: SESSION_TRACK,
                scene: SCENE,
                content: SessionCellContentSnapshot::Audio(SessionAudioClipSnapshot {
                    flavor: hex_project::ClipFlavor::default(),
                    gain: hex_project::ClipGain::UNITY,
                    id: SESSION_CLIP,
                    name: "one shot".into(),
                    asset: STREAM_ASSET,
                    source_start,
                    source_end,
                }),
            }],
        },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })
    .expect("valid Session streamed project")
}

fn project_with_arrangement_midi(source_start: u64, source_end: u64) -> Project {
    let mut editor = Editor::new(project(source_start, source_end), 8).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected instrument track"),
    };
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected Arrangement MIDI clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    Project::from_snapshot(editor.project().snapshot()).unwrap()
}

fn sample(frame: u64) -> Frame {
    [(frame % 19) as f32 / 32.0, -((frame % 13) as f32) / 16.0]
}

fn metadata(project: &Project, generation: u64, fingerprint: [u8; 32]) -> StreamedSourceMetadata {
    let asset = project
        .assets()
        .find(|asset| asset.id().value() == STREAM_ASSET)
        .expect("stream asset");
    StreamedSourceMetadata::new(
        StreamSourceKey::new(generation, asset.id()),
        asset.source_sample_rate(),
        project.sample_rate(),
        asset.source_frames(),
        asset.playback_frames(),
        asset.conversion_version(),
        fingerprint,
    )
    .expect("valid streamed metadata")
}

fn sources(
    project: &Project,
    source: StreamedSourceMetadata,
) -> BTreeMap<AssetId, PreparedAudioSource> {
    let resident = Audio::new(RATE, (0..32).map(|index| sample(index + 2)).collect())
        .expect("resident bed PCM");
    project
        .assets()
        .map(|asset| {
            let prepared = if asset.id().value() == STREAM_ASSET {
                PreparedAudioSource::Streamed(Arc::new(source.clone()))
            } else {
                PreparedAudioSource::Resident(resident.clone())
            };
            (asset.id(), prepared)
        })
        .collect()
}

#[derive(Default)]
struct MemoryPages {
    pages: BTreeMap<(StreamSourceKey, u64), Vec<Frame>>,
    requests: Vec<(StreamSourceKey, u64)>,
}

impl MemoryPages {
    fn with_source(source: StreamSourceKey) -> Self {
        let mut pages = BTreeMap::new();
        for page_index in 0..2_u64 {
            let first = page_index * hex_engine::STREAM_PAGE_FRAMES as u64;
            pages.insert(
                (source, page_index),
                (0..hex_engine::STREAM_PAGE_FRAMES)
                    .map(|offset| sample(first + offset as u64))
                    .collect(),
            );
        }
        Self {
            pages,
            requests: Vec::new(),
        }
    }
}

impl hex_engine::MediaPages for MemoryPages {
    fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        self.pages
            .get(&(key, page_index))
            .map(|frames| BorrowedMediaPage { page_index, frames })
    }

    fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.requests.push((key, page_index));
    }
}

#[derive(Clone, Copy)]
enum Malformation {
    WrongIndex,
    Short,
}

struct MalformedPages {
    key: StreamSourceKey,
    frames: Vec<Frame>,
    kind: Malformation,
    requests: Vec<(StreamSourceKey, u64)>,
}

impl MalformedPages {
    fn new(key: StreamSourceKey, kind: Malformation) -> Self {
        let frame_count = match kind {
            Malformation::WrongIndex => hex_engine::STREAM_PAGE_FRAMES,
            Malformation::Short => hex_engine::STREAM_PAGE_FRAMES - 1,
        };
        Self {
            key,
            frames: (0..frame_count).map(|index| sample(index as u64)).collect(),
            kind,
            requests: Vec::new(),
        }
    }
}

impl hex_engine::MediaPages for MalformedPages {
    fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        if key != self.key || page_index != 0 {
            return None;
        }
        let returned_index = match self.kind {
            Malformation::WrongIndex => page_index.checked_add(1)?,
            Malformation::Short => page_index,
        };
        Some(BorrowedMediaPage {
            page_index: returned_index,
            frames: &self.frames,
        })
    }

    fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.requests.push((key, page_index));
    }
}

fn prepared(project: &Project, generation: u64, fingerprint: [u8; 32]) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            project,
            &sources(project, metadata(project, generation, fingerprint)),
            generation,
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepared mixed revision"),
    )
}

fn launch_and_play(playback: &mut Playback) {
    playback
        .try_apply_action(PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip: playback
                    .prepared_revision()
                    .and_then(|revision| revision.session().audio_clips().first())
                    .expect("prepared Session audio clip")
                    .id,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
        ))
        .expect("launch Session clip");
    playback.apply(PlaybackCommand::Play);
}

#[test]
fn streamed_session_one_shot_mixes_pages_and_keeps_cold_misses_local() {
    let project = project(4_094, 4_102);
    let prepared = prepared(&project, 71, [3; 32]);
    assert!(prepared.contains_streamed_sources());
    let prepared_clip = &prepared.session().audio_clips()[0];
    assert!(matches!(
        prepared_clip.source(),
        PreparedAudioSource::Streamed(_)
    ));
    assert!(prepared_clip.resident_audio().is_none());

    let source_key = StreamSourceKey::new(71, project.assets().next().unwrap().id());
    let mut cold = Playback::try_from_prepared_with_session(Arc::clone(&prepared)).unwrap();
    launch_and_play(&mut cold);

    let mut untouched = [[9.0; 2]; 12];
    assert_eq!(
        cold.render(&mut untouched)
            .expect_err("provider is required")
            .to_string(),
        "streamed session playback requires a media page provider"
    );
    assert_eq!(untouched, [[9.0; 2]; 12]);
    assert_eq!(cold.performance_frame(), 0);
    let mut empty = [];
    cold.render(&mut empty)
        .expect("empty providerless block is a no-op");
    assert_eq!(cold.performance_frame(), 0);

    let mut missing = MemoryPages::default();
    let mut output = [[0.0; 2]; 12];
    cold.render_with_pages(&mut output, &mut missing)
        .expect("cold page misses do not block other tracks");
    let bed: Vec<_> = (0..12).map(|index| sample(index + 2)).collect();
    assert_eq!(output.as_slice(), bed.as_slice());
    assert_eq!(cold.performance_frame(), 12);
    assert_eq!(missing.requests, vec![(source_key, 0), (source_key, 1)]);

    let mut warm = Playback::try_from_prepared_with_session(prepared).unwrap();
    launch_and_play(&mut warm);
    let mut pages = MemoryPages::with_source(source_key);
    let mut actual = [[0.0; 2]; 12];
    warm.render_with_pages(&mut actual, &mut pages)
        .expect("warmed Session streamed render");
    let mut expected: Vec<_> = (0..12).map(|index| sample(index + 2)).collect();
    for (offset, frame) in expected.iter_mut().enumerate().take(8) {
        let streamed = sample(4_094 + offset as u64);
        frame[0] += streamed[0];
        frame[1] += streamed[1];
    }
    assert_eq!(actual.as_slice(), expected.as_slice());
    assert!(pages.requests.is_empty());
}

#[test]
fn streamed_arrangement_audio_mixes_with_builtin_fm4_midi() {
    let project = project_with_arrangement_midi(0, 8_192);
    let stream_asset = project
        .assets()
        .find(|asset| asset.id().value() == STREAM_ASSET)
        .expect("stream asset id")
        .id();
    let mut editor = Editor::new(project, 8).unwrap();
    let midi_track = editor
        .project()
        .tracks()
        .iter()
        .find(|track| track.instrument().is_some())
        .expect("Arrangement MIDI track")
        .id();
    let instrument = editor
        .project()
        .track(midi_track)
        .and_then(|track| track.instrument())
        .expect("instrument slot")
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.fm4"),
            )
            .unwrap(),
        })
        .unwrap();
    let stream_track = editor
        .project()
        .tracks()
        .iter()
        .find(|track| track.id().value() == SESSION_TRACK)
        .expect("streamed audio source track")
        .id();
    editor
        .execute(Command::PlaceClip {
            track: stream_track,
            asset: stream_asset,
            start: 0,
            duration: 512,
            source_offset: 0,
            source_length: 512,
            looping: false,
        })
        .unwrap();
    let project = editor.project().clone();
    let generation = 85;
    let source_metadata = metadata(&project, generation, [9; 32]);
    let revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &sources(&project, source_metadata),
            generation,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let source_key = StreamSourceKey::new(generation, stream_asset);
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(PlaybackCommand::Play);
    let mut pages = MemoryPages::with_source(source_key);
    let mut actual = [[0.0; 2]; 256];
    playback.render_with_pages(&mut actual, &mut pages).unwrap();

    for (frame_index, actual_frame) in actual.iter().enumerate() {
        let age = frame_index as u64;
        let frequency = 440.0 * 2.0_f64.powf((60.0 - 69.0) / 12.0);
        let step = (frequency / f64::from(RATE) * 18_446_744_073_709_551_616.0).round() as u64;
        let turns = step.wrapping_mul(age) as f64 / 18_446_744_073_709_551_616.0;
        let envelope = if age < 240 {
            age as f64 / 240.0
        } else {
            1.0 - (age - 240) as f64 / 4_800.0 * 0.3
        };
        let fm4 = (((turns * std::f64::consts::TAU).sin() as f32) as f64
            * envelope
            * (100.0 / 127.0)) as f32;
        let mut expected = sample(age);
        expected[0] += fm4;
        expected[1] += fm4;
        if age < 12 {
            let resident_bed = sample(age + 2);
            expected[0] += resident_bed[0];
            expected[1] += resident_bed[1];
        }
        assert!(
            (actual_frame[0] - expected[0]).abs() < 0.000_02
                && (actual_frame[1] - expected[1]).abs() < 0.000_02,
            "streamed hybrid frame {age}: {actual_frame:?} != {expected:?}"
        );
    }
}

#[test]
fn streamed_session_audio_uses_registered_native_device_matrix() {
    let original = project(4_094, 4_102);
    let mut editor = Editor::new(original, 16).unwrap();
    let track = editor
        .project()
        .tracks()
        .iter()
        .find(|track| track.name() == "session audio")
        .unwrap()
        .id();
    let state = UTILITY_DEFINITION.default_state().unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            utility_parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let project = editor.project().clone();
    let revision = prepared(&project, 73, [5; 32]);
    let source_key = StreamSourceKey::new(
        73,
        project
            .assets()
            .find(|asset| asset.id().value() == STREAM_ASSET)
            .unwrap()
            .id(),
    );
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    launch_and_play(&mut playback);
    let mut pages = MemoryPages::with_source(source_key);
    let mut output = [[0.0; 2]; 12];
    playback.render_with_pages(&mut output, &mut pages).unwrap();
    for (index, frame) in output.iter().enumerate() {
        let bed = sample(index as u64 + 2);
        let expected = if index < 8 {
            let source = sample(4_094 + index as u64);
            [source[0] + bed[0], -source[1] + bed[1]]
        } else {
            bed
        };
        assert_eq!(*frame, expected);
    }
}

#[test]
fn streamed_session_silences_and_requests_short_or_wrong_index_pages() {
    let project = project(4_094, 4_102);
    let revision = prepared(&project, 71, [3; 32]);
    let key = StreamSourceKey::new(71, project.assets().next().unwrap().id());

    for (kind, source_prefix, expected_requests) in [
        (Malformation::WrongIndex, 0, vec![(key, 0), (key, 1)]),
        (Malformation::Short, 1, vec![(key, 0), (key, 1)]),
    ] {
        let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        launch_and_play(&mut playback);
        let mut pages = MalformedPages::new(key, kind);
        let mut actual = [[0.0; 2]; 8];
        playback.render_with_pages(&mut actual, &mut pages).unwrap();
        let mut expected: Vec<_> = (0..8).map(|index| sample(index + 2)).collect();
        for (offset, frame) in expected.iter_mut().take(source_prefix).enumerate() {
            let streamed = sample(4_094 + offset as u64);
            frame[0] += streamed[0];
            frame[1] += streamed[1];
        }
        assert_eq!(actual.as_slice(), expected.as_slice());
        assert_eq!(pages.requests, expected_requests);
        assert_eq!(playback.performance_frame(), 8);
    }
}

#[test]
fn streamed_session_cold_miss_keeps_arrangement_midi_and_audio_exact() {
    let project = project_with_arrangement_midi(4_094, 4_102);
    let revision = prepared(&project, 71, [3; 32]);
    let key = StreamSourceKey::new(71, project.assets().next().unwrap().id());
    let mut streamed = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
    launch_and_play(&mut streamed);
    let mut cold_pages = MemoryPages::default();
    let mut cold_output = [[0.0; 2]; 12];
    streamed
        .render_with_pages(&mut cold_output, &mut cold_pages)
        .unwrap();
    assert_eq!(cold_pages.requests, vec![(key, 0), (key, 1)]);
    assert!(cold_output.iter().any(|frame| frame[0] != 0.0));

    let mut stop_snapshot = project.snapshot();
    let audio_cell = stop_snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == SESSION_TRACK)
        .unwrap();
    audio_cell.content = SessionCellContentSnapshot::Stop;
    let stopped = Project::from_snapshot(stop_snapshot).unwrap();
    let mut resident_media = BTreeMap::new();
    for asset in stopped.assets() {
        let frames = if asset.id().value() == STREAM_ASSET {
            (0..8_192).map(sample).collect()
        } else {
            (0..32).map(|index| sample(index + 2)).collect()
        };
        resident_media.insert(asset.id(), Audio::new(RATE, frames).unwrap());
    }
    let baseline_revision = Arc::new(
        PreparedRevision::prepare(&stopped, &resident_media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let mut baseline = Playback::try_from_prepared_with_session(baseline_revision).unwrap();
    baseline.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 12];
    baseline.render(&mut expected).unwrap();
    assert_eq!(
        cold_output, expected,
        "only the missing Session source is silent"
    );
    assert!(
        cold_output
            .iter()
            .enumerate()
            .any(|(index, frame)| *frame != sample(index as u64 + 2))
    );
    assert_eq!(streamed.performance_frame(), 12);
}

#[test]
fn streamed_session_bindings_validate_referenced_facts_and_ignore_unused_extras() {
    let project = project(4_094, 4_102);
    let canonical = metadata(&project, 71, [3; 32]);
    let good = sources(&project, canonical.clone());
    assert!(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &good,
            71,
            Arc::new(SineTable::new()),
            None
        )
        .is_ok()
    );

    let referenced = project.assets().next().unwrap().id();
    let mut missing = good.clone();
    missing.remove(&referenced);
    assert!(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &missing,
            71,
            Arc::new(SineTable::new()),
            None
        )
        .is_err()
    );

    let rejected = |candidate: StreamedSourceMetadata| {
        let mut bank = good.clone();
        bank.insert(
            referenced,
            PreparedAudioSource::Streamed(Arc::new(candidate)),
        );
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &bank,
            71,
            Arc::new(SineTable::new()),
            None,
        )
        .is_err()
    };
    assert!(rejected(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(72, referenced),
            RATE,
            RATE,
            8_192,
            8_192,
            hex_project::IDENTITY_CONVERSION,
            [3; 32],
        )
        .unwrap()
    ));
    let unused = project.assets().nth(2).unwrap().id();
    assert!(rejected(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(71, unused),
            RATE,
            RATE,
            8_192,
            8_192,
            hex_project::IDENTITY_CONVERSION,
            [3; 32],
        )
        .unwrap()
    ));
    assert!(rejected(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(71, referenced),
            RATE,
            44_100,
            8_192,
            7_526,
            hex_project::SINC_CONVERSION_V1,
            [3; 32],
        )
        .unwrap()
    ));
    assert!(rejected(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(71, referenced),
            RATE,
            RATE,
            8_193,
            8_193,
            hex_project::IDENTITY_CONVERSION,
            [3; 32],
        )
        .unwrap()
    ));
    assert!(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(71, referenced),
            RATE,
            RATE,
            8_192,
            8_193,
            hex_project::IDENTITY_CONVERSION,
            [3; 32],
        )
        .is_err()
    );
    assert!(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(71, referenced),
            RATE,
            RATE,
            8_192,
            8_192,
            hex_project::SINC_CONVERSION_V1,
            [3; 32],
        )
        .is_err()
    );

    let unused_metadata = StreamedSourceMetadata::new(
        StreamSourceKey::new(999, unused),
        RATE,
        RATE,
        32,
        32,
        hex_project::IDENTITY_CONVERSION,
        [99; 32],
    )
    .unwrap();
    let mut with_unused = good;
    with_unused.insert(
        unused,
        PreparedAudioSource::Streamed(Arc::new(unused_metadata)),
    );
    assert!(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &with_unused,
            71,
            Arc::new(SineTable::new()),
            None
        )
        .is_ok()
    );
}

#[test]
fn streamed_session_seek_stop_and_source_handoff_keep_frame_anchors() {
    let old_project = project(4_094, 4_102);
    let old_revision = prepared(&old_project, 71, [3; 32]);
    let old_key = StreamSourceKey::new(71, old_project.assets().next().unwrap().id());
    let clip = old_revision.session().audio_clips()[0].id;
    let mut previous = Playback::try_from_prepared_with_session(Arc::clone(&old_revision)).unwrap();
    launch_and_play(&mut previous);
    let mut old_pages = MemoryPages::with_source(old_key);
    let mut prefix = [[0.0; 2]; 3];
    previous
        .render_with_pages(&mut prefix, &mut old_pages)
        .unwrap();
    for (offset, frame) in prefix.iter().enumerate() {
        let mut expected = sample(4_094 + offset as u64);
        let bed = sample(offset as u64 + 2);
        expected[0] += bed[0];
        expected[1] += bed[1];
        assert_eq!(*frame, expected);
    }

    let replacement_project = project(4_095, 4_104);
    let replacement = prepared(&replacement_project, 72, [8; 32]);
    let new_key = StreamSourceKey::new(72, replacement_project.assets().next().unwrap().id());
    let mut next = Playback::try_from_prepared_with_session(replacement).unwrap();
    next.try_inherit_runtime_from(&mut previous).unwrap();
    assert_eq!(next.performance_frame(), 3);
    assert_eq!(
        next.session_route_status(0).unwrap().owner,
        hex_engine::SessionPlaybackOwner::Session { clip }
    );
    let mut new_pages = MemoryPages::with_source(new_key);
    let mut suffix = [[0.0; 2]; 3];
    next.render_with_pages(&mut suffix, &mut new_pages).unwrap();
    for (offset, frame) in suffix.iter().enumerate() {
        let mut expected = sample(4_098 + offset as u64);
        let bed = sample(offset as u64 + 5);
        expected[0] += bed[0];
        expected[1] += bed[1];
        assert_eq!(*frame, expected, "source elapsed frame {offset}");
    }

    next.apply(PlaybackCommand::Seek(100));
    let mut seeked = [[0.0; 2]; 1];
    next.render_with_pages(&mut seeked, &mut new_pages).unwrap();
    assert_eq!(seeked[0], sample(4_095));
    next.apply(PlaybackCommand::Stop);
    next.apply(PlaybackCommand::Play);
    let mut restarted = [[0.0; 2]; 1];
    next.render_with_pages(&mut restarted, &mut new_pages)
        .unwrap();
    let mut expected = sample(4_095);
    let bed = sample(2);
    expected[0] += bed[0];
    expected[1] += bed[1];
    assert_eq!(restarted[0], expected);
}

#[test]
fn streamed_session_quantized_audio_launch_begins_on_the_due_performance_frame() {
    let base = project(4_094, 4_104);
    let revision = prepared(&base, 71, [3; 32]);
    let key = StreamSourceKey::new(71, base.assets().next().unwrap().id());
    let clip = revision.session().audio_clips()[0].id;
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(PlaybackCommand::Seek(1));
    playback.apply(PlaybackCommand::Play);
    playback
        .try_apply_action(PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::FourQuarter,
            },
        ))
        .unwrap();
    let mut pages = MemoryPages::with_source(key);
    let mut before_due = vec![[0.0; 2]; 95_999];
    playback
        .render_with_pages(&mut before_due, &mut pages)
        .unwrap();
    assert_eq!(playback.performance_frame(), 96_000);
    let mut at_due = [[0.0; 2]; 10];
    playback.render_with_pages(&mut at_due, &mut pages).unwrap();
    assert!(pages.requests.is_empty());
    for (offset, frame) in at_due.iter().enumerate() {
        assert_eq!(
            *frame,
            sample(4_094 + offset as u64),
            "due frame offset {offset}"
        );
    }
    assert_eq!(playback.position(), 96_010);
    assert_eq!(playback.performance_frame(), 96_010);
}

#[test]
fn streamed_session_audio_remains_frame_anchored_across_arrangement_loop_wrap() {
    let base = project(4_094, 4_104);
    let mut snapshot = base.snapshot();
    snapshot.loop_region =
        Some(hex_project::LoopRegion::new(0, 96_000, true).expect("one-bar loop"));
    let loop_project = Project::from_snapshot(snapshot).expect("looped Session project");
    let revision = prepared(&loop_project, 71, [3; 32]);
    let key = StreamSourceKey::new(71, loop_project.assets().next().unwrap().id());
    let clip = revision.session().audio_clips()[0].id;
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(PlaybackCommand::Seek(95_998));
    playback
        .try_apply_action(PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    playback.apply(PlaybackCommand::Play);
    let mut pages = MemoryPages::with_source(key);
    let mut across_wrap = [[0.0; 2]; 10];
    playback
        .render_with_pages(&mut across_wrap, &mut pages)
        .unwrap();
    for (offset, frame) in across_wrap.iter().enumerate() {
        let mut expected = sample(4_094 + offset as u64);
        if offset >= 2 {
            let bed = sample((offset - 2) as u64 + 2);
            expected[0] += bed[0];
            expected[1] += bed[1];
        }
        assert_eq!(*frame, expected, "loop-crossing frame {offset}");
    }
    assert_eq!(playback.position(), 8);
    assert_eq!(playback.performance_frame(), 96_008);
}

#[test]
fn finite_playback_keeps_streamed_arrangement_loop_segments_and_wraps() {
    let mut snapshot = project(4_093, 4_100).snapshot();
    let TrackContentSnapshot::Hybrid {
        audio_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    else {
        panic!("audio track expected")
    };
    clips.push(ClipSnapshot {
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 10,
        asset: STREAM_ASSET,
        start: 0,
        duration: 6,
        source_offset: 4_093,
        source_length: 4,
        looping: true,
        fade: hex_project::ClipFade::neutral(6).unwrap(),
        source_phase: 0,
    });
    snapshot.next_id = 11;
    snapshot.loop_region = Some(LoopRegion::new(1, 4, true).unwrap());
    let project = Project::from_snapshot(snapshot).unwrap();
    let revision = prepared(&project, 71, [3; 32]);
    let source_key = StreamSourceKey::new(71, project.assets().next().unwrap().id());
    let mut playback = Playback::from_prepared(revision);
    playback.apply(PlaybackCommand::Seek(1));
    playback.apply(PlaybackCommand::Play);
    let mut pages = MemoryPages::with_source(source_key);
    let mut actual = [[0.0; 2]; 8];
    playback.render_with_pages(&mut actual, &mut pages).unwrap();

    for (index, frame) in actual.iter().enumerate() {
        let timeline_frame = 1 + (index as u64 % 3);
        let mut expected = sample(4_093 + timeline_frame);
        let bed = sample(timeline_frame + 2);
        expected[0] += bed[0];
        expected[1] += bed[1];
        assert_eq!(*frame, expected, "looped finite Playback frame {index}");
    }
    assert!(pages.requests.is_empty());
    assert_eq!(playback.position(), 3);
    assert_eq!(playback.performance_frame(), 9);
}

#[test]
fn streamed_session_source_reuse_compares_metadata_values_not_arc_identity() {
    let project = project(4_094, 4_102);
    let table = Arc::new(SineTable::new());
    let first_source = metadata(&project, 71, [3; 32]);
    let first_bank = sources(&project, first_source.clone());
    let first = PreparedRevision::prepare_with_prepared_audio_sources(
        &project,
        &first_bank,
        71,
        Arc::clone(&table),
        None,
    )
    .unwrap();
    let clip_id = first.session().audio_clips()[0].id;
    let first_binding = match first.session().audio_clip(clip_id).unwrap().source() {
        PreparedAudioSource::Streamed(source) => Arc::clone(source),
        PreparedAudioSource::Resident(_) => panic!("streamed source expected"),
    };

    let equivalent_source = StreamedSourceMetadata::new(
        first_source.key(),
        first_source.source_rate(),
        first_source.project_rate(),
        first_source.source_frames(),
        first_source.playback_frames(),
        first_source.conversion_version(),
        first_source.fingerprint(),
    )
    .unwrap();
    let equivalent = PreparedRevision::prepare_with_prepared_audio_sources(
        &project,
        &sources(&project, equivalent_source),
        71,
        Arc::clone(&table),
        Some(first.session()),
    )
    .unwrap();
    let equivalent_binding = match equivalent.session().audio_clip(clip_id).unwrap().source() {
        PreparedAudioSource::Streamed(source) => source,
        PreparedAudioSource::Resident(_) => panic!("streamed source expected"),
    };
    assert!(Arc::ptr_eq(&first_binding, equivalent_binding));

    let changed_source = metadata(&project, 72, [4; 32]);
    let changed = PreparedRevision::prepare_with_prepared_audio_sources(
        &project,
        &sources(&project, changed_source),
        72,
        table,
        Some(equivalent.session()),
    )
    .unwrap();
    let changed_binding = match changed.session().audio_clip(clip_id).unwrap().source() {
        PreparedAudioSource::Streamed(source) => source,
        PreparedAudioSource::Resident(_) => panic!("streamed source expected"),
    };
    assert!(!Arc::ptr_eq(equivalent_binding, changed_binding));
}

#[test]
fn canonical_capture_streamed_empty_chain_offsets_partitions_and_provider_refusal() {
    use hex_engine::{OutputCapture, OutputCaptureId, SessionPlayback, SessionPlaybackCommand};
    let mut snapshot = project(0, 8192).snapshot();
    snapshot.session = SessionSnapshot::default();
    snapshot.next_id = 11;
    snapshot.master_gain = 0.25;
    snapshot.tracks[0].gain = 0.5;
    snapshot.tracks[0].balance = 0.5;
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 10,
        asset: STREAM_ASSET,
        start: 3,
        duration: 520,
        source_offset: 4090,
        source_length: 520,
        looping: false,
        source_phase: 0,
        fade: hex_project::ClipFade::neutral(520).unwrap(),
    }]);
    let project = Project::from_snapshot(snapshot).unwrap();
    let source = metadata(&project, 13, [4; 32]);
    let revision = prepared(&project, 13, [4; 32]);
    let mut runtime =
        SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let ids = project
        .tracks()
        .iter()
        .map(|track| OutputCaptureId::Track(track.id()))
        .collect::<Vec<_>>();
    let plan = runtime.output_capture_plan(&ids, 1024).unwrap();
    let mut main = [[9.0; 2]; 523];
    let mut streamed = main;
    let mut bed = main;
    let mut buffers: [&mut [Frame]; 2] = [&mut streamed, &mut bed];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    assert!(
        runtime
            .render_with_capture(&mut main, &mut capture)
            .is_err()
    );
    assert_eq!(runtime.performance_frame(), 0);
    assert_eq!(capture.valid_frames(), 0);
    assert!(main.iter().flatten().all(|sample| *sample == 0.0));
    let mut pages = MemoryPages::with_source(source.key());
    let mut offset = 0;
    for frames in [1, 2, 255, 257, 8] {
        let mut buffers: [&mut [Frame]; 2] = [
            &mut streamed[offset..offset + frames],
            &mut bed[offset..offset + frames],
        ];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        runtime
            .render_with_pages_and_capture(
                &mut main[offset..offset + frames],
                &mut pages,
                &mut capture,
            )
            .unwrap();
        assert_eq!(capture.valid_frames(), frames);
        offset += frames;
    }
    let mut reference = SessionPlayback::new_linear_arrangement_export(revision).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 523];
    reference
        .render_with_pages(&mut expected, &mut MemoryPages::with_source(source.key()))
        .unwrap();
    assert_eq!(main, expected);
    for (index, frame) in main.iter().enumerate() {
        let source = if index < 3 {
            [0.0; 2]
        } else {
            let raw = sample(4090 + index as u64 - 3);
            [raw[0] * 0.25, raw[1] * 0.5]
        };
        let resident = if index < 12 {
            sample(index as u64 + 2)
        } else {
            [0.0; 2]
        };
        assert_eq!(streamed[index], source);
        assert_eq!(bed[index], resident);
        assert_eq!(
            *frame,
            [
                (source[0] + resident[0]) * 0.25,
                (source[1] + resident[1]) * 0.25
            ]
        );
    }
}

#[test]
fn session_gain_resident_streamed_and_replacement_keep_one_shot_anchor() {
    for streamed in [false, true] {
        let project = project(4_094, 4_102);
        let clip = project.session_cells()[0].content();
        let hex_project::SessionCellContent::Audio(clip) = clip else {
            panic!("audio")
        };
        let clip = clip.id();
        let media: BTreeMap<_, _> = project
            .assets()
            .map(|asset| {
                let frames = if asset.id().value() == STREAM_ASSET {
                    (0..8_192).map(sample).collect()
                } else {
                    (0..32).map(|index| sample(index + 2)).collect()
                };
                (asset.id(), Audio::new(RATE, frames).unwrap())
            })
            .collect();
        let prepare = |project: &Project| {
            if streamed {
                prepared(project, 71, [3; 32])
            } else {
                Arc::new(
                    PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
                        .unwrap(),
                )
            }
        };
        let mut before = Playback::try_from_prepared_with_session(prepare(&project)).unwrap();
        launch_and_play(&mut before);
        let key = StreamSourceKey::new(71, project.assets().next().unwrap().id());
        let mut prefix = [[0.0; 2]; 3];
        before
            .render_with_pages(&mut prefix, &mut MemoryPages::with_source(key))
            .unwrap();
        let mut editor = Editor::new(project, 8).unwrap();
        editor
            .execute(Command::SetAudioClipGain {
                clip,
                millidb: -6_000,
            })
            .unwrap();
        let gain = hex_project::ClipGain::new(-6_000).unwrap().linear();
        let mut next = Playback::try_from_prepared_with_session(prepare(editor.project())).unwrap();
        next.try_inherit_runtime_from(&mut before).unwrap();
        assert_eq!(next.performance_frame(), 3);
        let mut actual = [[0.0; 2]; 9];
        let mut pages = MemoryPages::with_source(key);
        for chunk in actual.chunks_mut(2) {
            next.render_with_pages(chunk, &mut pages).unwrap();
        }
        for (index, actual) in actual.iter().enumerate() {
            let age = index as u64 + 3;
            let bed = sample(age + 2);
            let expected = if age < 8 {
                let source = sample(4_094 + age);
                [source[0] * gain + bed[0], source[1] * gain + bed[1]]
            } else {
                bed
            };
            assert_eq!(*actual, expected);
        }
    }
}

#[test]
fn exact_engine_stream_demand_is_pure_across_pending_play_page_edges_and_handoff_retry() {
    use hex_engine::{SessionPlayback, SessionPlaybackCommand, StreamPageExtent};
    let project = project(4094, 4105);
    let source = metadata(&project, 51, [0x71; 32]);
    let key = source.key();
    let revision = prepared(&project, 51, [0x71; 32]);
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: revision.session().audio_clips()[0].id,
        quantization: hex_engine::LaunchQuantization::Immediate,
    })
    .unwrap();
    let before = (
        old.performance_frame(),
        old.arrangement_frame(),
        old.route_state_generation(),
    );
    let route = old.route_status(0);
    let mut extents = Vec::new();
    assert_eq!(
        old.visit_next_stream_chunk(8, None, &mut |e| {
            extents.push(e);
            Ok(())
        })
        .unwrap()
        .frames,
        0
    );
    let proposed = old
        .visit_next_stream_chunk(8, Some(true), &mut |e| {
            extents.push(e);
            Ok(())
        })
        .unwrap();
    assert_eq!(proposed.frames, 8);
    assert_eq!(
        extents,
        vec![
            StreamPageExtent {
                source: key,
                page_index: 0,
                start: 4094,
                end: 4096
            },
            StreamPageExtent {
                source: key,
                page_index: 1,
                start: 0,
                end: 6
            },
        ]
    );
    assert_eq!(
        (
            old.performance_frame(),
            old.arrangement_frame(),
            old.route_state_generation()
        ),
        before
    );
    assert_eq!(old.route_status(0), route);
    assert!(
        old.visit_next_stream_chunk(8, Some(true), &mut |_| Err(hex_engine::Invalid(
            "reserved extent capacity"
        )))
        .is_err()
    );
    assert_eq!(
        (
            old.performance_frame(),
            old.arrangement_frame(),
            old.route_state_generation()
        ),
        before
    );
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render_with_pages(&mut [[0.0; 2]; 4], &mut MemoryPages::with_source(key))
        .unwrap();
    let mut next = SessionPlayback::new(revision).unwrap();
    let old_before = (
        old.performance_frame(),
        old.arrangement_frame(),
        old.route_status(0),
    );
    assert!(
        next.preview_stream_handoff(&mut old, 4, None, &mut |_| Err(hex_engine::Invalid(
            "reserved extent capacity"
        )))
        .is_err()
    );
    assert_eq!(
        (
            old.performance_frame(),
            old.arrangement_frame(),
            old.route_status(0)
        ),
        old_before
    );
    let mut planned = Vec::new();
    let planned_window = next
        .preview_stream_handoff(&mut old, 4, None, &mut |e| {
            planned.push(e);
            Ok(())
        })
        .unwrap();
    assert_eq!(next.performance_frame(), 0); // preview did not consume candidate admission
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = Vec::new();
    assert_eq!(
        next.visit_next_stream_chunk(4, None, &mut |e| {
            actual.push(e);
            Ok(())
        })
        .unwrap(),
        planned_window
    );
    assert_eq!(actual, planned);
    assert_eq!(
        actual,
        vec![StreamPageExtent {
            source: key,
            page_index: 1,
            start: 2,
            end: 6
        }]
    );
}
