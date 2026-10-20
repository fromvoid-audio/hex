use hex_engine::{
    Audio, BorrowedMediaPage, MediaPages, Renderer, STREAM_PAGE_FRAMES, StreamSourceKey,
    StreamedSourceMetadata,
    compile::{self, PreparedAudioSource},
};
use hex_project::{AssetId, BeatTime, Command, Created, Editor, Project, SINC_CONVERSION_V1};
use std::{collections::BTreeMap, sync::Arc};

const GENERATION: u64 = 29;
const SOURCE_FRAMES: usize = STREAM_PAGE_FRAMES * 2;

fn add_asset(editor: &mut Editor, path: &str, frames: u64) -> AssetId {
    match editor
        .execute(Command::AddAsset {
            path: path.into(),
            sample_rate: 48_000,
            frames,
        })
        .expect("add asset")
    {
        Created::Asset(id) => id,
        other => panic!("unexpected asset result: {other:?}"),
    }
}

fn stream(asset: AssetId) -> Arc<StreamedSourceMetadata> {
    Arc::new(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(GENERATION, asset),
            48_000,
            48_000,
            SOURCE_FRAMES as u64,
            SOURCE_FRAMES as u64,
            0,
            [0x31; 32],
        )
        .expect("valid catalog metadata"),
    )
}

fn source_pcm() -> Vec<[f32; 2]> {
    (0..SOURCE_FRAMES)
        .map(|index| match index % 4 {
            0 => [0.25, -0.5],
            1 => [-0.125, 0.75],
            2 => [0.5, 0.125],
            _ => [-0.25, -0.25],
        })
        .collect()
}

struct Pages {
    key: StreamSourceKey,
    frames: Vec<[f32; 2]>,
    requests: Vec<(StreamSourceKey, u64)>,
    missing: bool,
}

impl MediaPages for Pages {
    fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        if self.missing || key != self.key {
            return None;
        }
        let start = usize::try_from(page_index)
            .ok()?
            .checked_mul(STREAM_PAGE_FRAMES)?;
        let end = start
            .checked_add(STREAM_PAGE_FRAMES)?
            .min(self.frames.len());
        (start < end).then_some(BorrowedMediaPage {
            page_index,
            frames: &self.frames[start..end],
        })
    }

    fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.requests.push((key, page_index));
    }
}

fn fixture() -> (
    Editor,
    AssetId,
    AssetId,
    hex_project::TrackId,
    Vec<[f32; 2]>,
) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let streamed_asset = add_asset(&mut editor, "stream.wav", SOURCE_FRAMES as u64);
    let resident_asset = add_asset(&mut editor, "resident.wav", 64);
    let streamed_track = match editor
        .execute(Command::AddTrack {
            name: "streamed".into(),
        })
        .expect("streamed track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected track result: {other:?}"),
    };
    let streamed_clip = match editor
        .execute(Command::PlaceClip {
            track: streamed_track,
            asset: streamed_asset,
            start: 2,
            duration: 28,
            source_offset: STREAM_PAGE_FRAMES as u64 - 4,
            source_length: 9,
            looping: true,
        })
        .expect("streamed clip")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected clip result: {other:?}"),
    };
    editor
        .execute(Command::SetClipFades {
            clip: streamed_clip,
            fade_in: 3,
            fade_out: 5,
        })
        .expect("set clip fades");
    editor
        .execute(Command::SplitClip {
            clip: streamed_clip,
            frame: 9,
        })
        .expect("split clip to produce a nonzero source phase");
    editor
        .execute(Command::Gain {
            track: streamed_track,
            gain: 0.5,
        })
        .expect("track gain");
    editor
        .execute(Command::Balance {
            track: streamed_track,
            balance: 0.25,
        })
        .expect("track balance");

    let resident_track = match editor
        .execute(Command::AddTrack {
            name: "resident".into(),
        })
        .expect("resident track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected track result: {other:?}"),
    };
    editor
        .execute(Command::PlaceClip {
            track: resident_track,
            asset: resident_asset,
            start: 4,
            duration: 20,
            source_offset: 3,
            source_length: 8,
            looping: true,
        })
        .expect("resident clip");
    editor
        .execute(Command::Gain {
            track: resident_track,
            gain: 0.25,
        })
        .expect("resident gain");
    editor
        .execute(Command::MasterGain { gain: 0.75 })
        .expect("master gain");
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 16,
            enabled: true,
        })
        .expect("project loop");

    let midi_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("midi track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected midi track result: {other:?}"),
    };
    let midi_clip = match editor
        .execute(Command::PlaceMidiClip {
            track: midi_track,
            start: BeatTime::ZERO,
            length: BeatTime::from_ratio(2, 1).expect("beat length"),
        })
        .expect("midi clip")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected midi clip result: {other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 2).expect("note length"),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("midi note");

    (
        editor,
        streamed_asset,
        resident_asset,
        streamed_track,
        source_pcm(),
    )
}

fn resident_sources(
    streamed_asset: AssetId,
    resident_asset: AssetId,
    streamed_pcm: &[[f32; 2]],
) -> BTreeMap<AssetId, PreparedAudioSource> {
    BTreeMap::from([
        (
            streamed_asset,
            PreparedAudioSource::Resident(
                Audio::new(48_000, streamed_pcm.to_vec()).expect("resident oracle audio"),
            ),
        ),
        (
            resident_asset,
            PreparedAudioSource::Resident(
                Audio::new(48_000, vec![[0.5, 0.25]; 64]).expect("resident audio"),
            ),
        ),
    ])
}

#[test]
fn mixed_compile_matches_resident_oracle_with_preview_and_shared_table() {
    for gain in [0, -6_000, 24_000] {
        mixed_compile_with_gain(gain);
    }
}

fn mixed_compile_with_gain(millidb: i32) {
    let (mut editor, streamed_asset, resident_asset, _, pcm) = fixture();
    let clips: Vec<_> = editor
        .project()
        .tracks()
        .iter()
        .flat_map(|track| track.clips().iter().map(|clip| clip.id()))
        .collect();
    for clip in clips {
        editor
            .execute(Command::SetAudioClipGain { clip, millidb })
            .unwrap();
    }
    let oracle_sources = resident_sources(streamed_asset, resident_asset, &pcm);
    let (resident, resident_preview) =
        compile::project_with_prepared_audio_sources_and_midi_preview_and_table(
            editor.project(),
            &oracle_sources,
            GENERATION,
            Some(hex_engine::SharedSineTable::new_on_control()),
        )
        .expect("resident oracle compile");

    let stream_map = BTreeMap::from([
        (
            streamed_asset,
            PreparedAudioSource::Streamed(stream(streamed_asset)),
        ),
        (
            resident_asset,
            PreparedAudioSource::Resident(
                Audio::new(48_000, vec![[0.5, 0.25]; 64]).expect("resident audio"),
            ),
        ),
    ]);
    let (streamed, streamed_preview) =
        compile::project_with_prepared_audio_sources_and_midi_preview_and_table(
            editor.project(),
            &stream_map,
            GENERATION,
            Some(hex_engine::SharedSineTable::new_on_control()),
        )
        .expect("mixed-source compile");
    assert_eq!(resident_preview.clips(), streamed_preview.clips());
    assert_eq!(resident_preview.clips().len(), 1);
    assert_eq!(resident.loop_region(), streamed.loop_region());

    let mut resident_renderer = Renderer::new(&resident);
    let mut streamed_renderer = Renderer::new(&streamed);
    resident_renderer.play();
    streamed_renderer.play();
    let mut expected = [[0.0; 2]; 40];
    let mut actual = [[0.0; 2]; 40];
    resident_renderer
        .render(&mut expected)
        .expect("resident render");
    let key = stream(streamed_asset).key();
    let mut pages = Pages {
        key,
        frames: pcm,
        requests: Vec::new(),
        missing: false,
    };
    let mut offset = 0;
    for count in [1, 3, 2, 7, 1, 11, 15] {
        streamed_renderer
            .render_with_pages(&mut actual[offset..offset + count], &mut pages)
            .expect("streamed render");
        offset += count;
    }
    assert_eq!(actual, expected);
    assert!(pages.requests.is_empty());
}

#[test]
fn ordered_hybrid_streamed_render_matches_resident_audio_and_midi() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let asset = add_asset(&mut editor, "ordered-stream.wav", SOURCE_FRAMES as u64);
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "ordered hybrid".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        other => panic!("unexpected track: {other:?}"),
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 40,
            source_offset: 0,
            source_length: 40,
            looping: false,
        })
        .unwrap();
    let midi = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip: {other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let prefix_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: prefix_state,
        })
        .unwrap();
    let suffix_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::CHANNEL,
            ParameterValue::Choice(3),
        )
        .unwrap();
    let suffix = match editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: suffix_state,
        })
        .unwrap()
    {
        Created::Device(device) => device,
        other => panic!("unexpected device: {other:?}"),
    };
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::MoveTrackDevice {
            device: instrument,
            track,
            before: Some(suffix),
        })
        .unwrap();

    let pcm = source_pcm();
    let resident_media = BTreeMap::from([(asset, Audio::new(48_000, pcm.clone()).unwrap())]);
    let resident = compile::project(editor.project(), &resident_media).unwrap();
    let streamed_media = BTreeMap::from([(asset, PreparedAudioSource::Streamed(stream(asset)))]);
    let streamed =
        compile::project_with_prepared_audio_sources(editor.project(), &streamed_media, GENERATION)
            .unwrap();

    let mut expected = [[0.0; 2]; 40];
    let mut resident_renderer = Renderer::new(&resident);
    resident_renderer.play();
    resident_renderer.render(&mut expected).unwrap();
    let mut actual = [[0.0; 2]; 40];
    let mut streamed_renderer = Renderer::new(&streamed);
    streamed_renderer.play();
    let mut pages = Pages {
        key: stream(asset).key(),
        frames: pcm,
        requests: Vec::new(),
        missing: false,
    };
    streamed_renderer
        .render_with_pages(&mut actual, &mut pages)
        .unwrap();
    assert_eq!(actual, expected);
    assert!(pages.requests.is_empty());
}

#[test]
fn streamed_compile_rejects_invalid_referenced_bindings_but_ignores_extras() {
    let (mut editor, streamed_asset, resident_asset, _, pcm) = fixture();
    let mut valid = resident_sources(streamed_asset, resident_asset, &pcm);
    valid.insert(
        streamed_asset,
        PreparedAudioSource::Streamed(stream(streamed_asset)),
    );
    assert!(
        compile::project_with_prepared_audio_sources(editor.project(), &valid, GENERATION).is_ok()
    );
    assert!(compile::project_with_prepared_audio_sources(editor.project(), &valid, 0).is_err());

    for invalid in [
        StreamedSourceMetadata::new(
            StreamSourceKey::new(GENERATION + 1, streamed_asset),
            48_000,
            48_000,
            SOURCE_FRAMES as u64,
            SOURCE_FRAMES as u64,
            0,
            [0x31; 32],
        ),
        StreamedSourceMetadata::new(
            StreamSourceKey::new(GENERATION, resident_asset),
            48_000,
            48_000,
            SOURCE_FRAMES as u64,
            SOURCE_FRAMES as u64,
            0,
            [0x31; 32],
        ),
        StreamedSourceMetadata::new(
            StreamSourceKey::new(GENERATION, streamed_asset),
            44_100,
            48_000,
            7_526,
            SOURCE_FRAMES as u64,
            SINC_CONVERSION_V1,
            [0x31; 32],
        ),
        StreamedSourceMetadata::new(
            StreamSourceKey::new(GENERATION, streamed_asset),
            44_100,
            44_100,
            SOURCE_FRAMES as u64,
            SOURCE_FRAMES as u64,
            0,
            [0x31; 32],
        ),
        StreamedSourceMetadata::new(
            StreamSourceKey::new(GENERATION, streamed_asset),
            48_000,
            48_000,
            SOURCE_FRAMES as u64 + 1,
            SOURCE_FRAMES as u64 + 1,
            0,
            [0x31; 32],
        ),
    ] {
        assert!(invalid.is_ok());
        let mut sources = valid.clone();
        sources.insert(
            streamed_asset,
            PreparedAudioSource::Streamed(Arc::new(invalid.expect("well-formed mismatch"))),
        );
        assert!(
            compile::project_with_prepared_audio_sources(editor.project(), &sources, GENERATION)
                .is_err()
        );
    }

    let mut missing = valid.clone();
    missing.remove(&streamed_asset);
    assert!(
        compile::project_with_prepared_audio_sources(editor.project(), &missing, GENERATION)
            .is_err()
    );

    let mut unused_invalid = valid;
    let extra_id = add_asset(&mut editor, "unused.wav", 8);
    unused_invalid.insert(
        extra_id,
        PreparedAudioSource::Resident(Audio::new(44_100, vec![[0.0; 2]; 1]).unwrap()),
    );
    unused_invalid.insert(
        add_asset(&mut editor, "unused-stream.wav", 8),
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                StreamSourceKey::new(GENERATION + 100, extra_id),
                48_000,
                48_000,
                8,
                8,
                0,
                [0; 32],
            )
            .expect("valid but stale unused metadata"),
        )),
    );
    assert!(
        compile::project_with_prepared_audio_sources(editor.project(), &unused_invalid, GENERATION)
            .is_ok()
    );

    assert!(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(GENERATION, streamed_asset),
            48_000,
            48_000,
            SOURCE_FRAMES as u64,
            SOURCE_FRAMES as u64 - 1,
            0,
            [0; 32],
        )
        .is_err()
    );
    assert!(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(GENERATION, streamed_asset),
            48_000,
            48_000,
            SOURCE_FRAMES as u64,
            SOURCE_FRAMES as u64,
            SINC_CONVERSION_V1,
            [0; 32],
        )
        .is_err()
    );
}

#[test]
fn cold_stream_miss_keeps_resident_and_midi_tracks_rendering_and_clock_moving() {
    let (mut editor, streamed_asset, resident_asset, streamed_track, pcm) = fixture();
    let mut sources = resident_sources(streamed_asset, resident_asset, &pcm);
    sources.insert(
        streamed_asset,
        PreparedAudioSource::Streamed(stream(streamed_asset)),
    );
    let (with_stream, _) = compile::project_with_prepared_audio_sources_and_midi_preview_and_table(
        editor.project(),
        &sources,
        GENERATION,
        Some(hex_engine::SharedSineTable::new_on_control()),
    )
    .expect("mixed compile");

    editor
        .execute(Command::Mute {
            track: streamed_track,
            muted: true,
        })
        .expect("mute only streamed track");
    let (resident_and_midi, _) =
        compile::project_with_prepared_audio_sources_and_midi_preview_and_table(
            editor.project(),
            &sources,
            GENERATION,
            Some(hex_engine::SharedSineTable::new_on_control()),
        )
        .expect("resident plus midi oracle");

    let mut actual_renderer = Renderer::new(&with_stream);
    let mut expected_renderer = Renderer::new(&resident_and_midi);
    actual_renderer.play();
    expected_renderer.play();
    let mut expected = vec![[0.0; 2]; 13_000];
    let mut actual = vec![[0.0; 2]; 13_000];
    let key = stream(streamed_asset).key();
    let mut expected_pages = Pages {
        key,
        frames: pcm.clone(),
        requests: Vec::new(),
        missing: false,
    };
    expected_renderer
        .render_with_pages(&mut expected, &mut expected_pages)
        .expect("resident plus midi render");
    let mut pages = Pages {
        key,
        frames: pcm,
        requests: Vec::new(),
        missing: true,
    };
    let mut offset = 0;
    for count in [127, 997, 2048, 31, 4096, 5701] {
        actual_renderer
            .render_with_pages(&mut actual[offset..offset + count], &mut pages)
            .expect("render with cold streamed pages");
        offset += count;
    }
    assert_eq!(actual, expected);
    assert_eq!(actual_renderer.position(), expected_renderer.position());
    assert_eq!(actual_renderer.position(), 13_000);
    assert!(pages.requests.contains(&(key, 0)));
    assert!(pages.requests.contains(&(key, 1)));
}

#[test]
fn streamed_one_shot_exhaustion_matches_resident_source_across_irregular_blocks() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let asset = add_asset(&mut editor, "one-shot.wav", 16);
    let track = match editor
        .execute(Command::AddTrack {
            name: "one shot".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected track result: {other:?}"),
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 14,
            source_offset: 5,
            source_length: 4,
            looping: false,
        })
        .expect("one-shot clip");
    let frames = vec![
        [0.0, 0.0],
        [0.125, -0.125],
        [0.25, -0.25],
        [0.375, -0.375],
        [0.5, -0.5],
        [0.625, -0.625],
        [0.75, -0.75],
        [0.875, -0.875],
        [1.0, -1.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
    ];
    let resident_sources = BTreeMap::from([(
        asset,
        PreparedAudioSource::Resident(Audio::new(48_000, frames.clone()).expect("pcm")),
    )]);
    let streamed_sources = BTreeMap::from([(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                StreamSourceKey::new(GENERATION, asset),
                48_000,
                48_000,
                16,
                16,
                0,
                [0x77; 32],
            )
            .expect("metadata"),
        )),
    )]);
    let resident = compile::project_with_prepared_audio_sources(
        editor.project(),
        &resident_sources,
        GENERATION,
    )
    .expect("resident compile");
    let streamed = compile::project_with_prepared_audio_sources(
        editor.project(),
        &streamed_sources,
        GENERATION,
    )
    .expect("streamed compile");
    let mut resident_renderer = Renderer::new(&resident);
    let mut streamed_renderer = Renderer::new(&streamed);
    resident_renderer.play();
    streamed_renderer.play();
    let mut expected = [[0.0; 2]; 18];
    let mut actual = [[0.0; 2]; 18];
    resident_renderer
        .render(&mut expected)
        .expect("resident one-shot render");
    let key = StreamSourceKey::new(GENERATION, asset);
    let mut pages = Pages {
        key,
        frames,
        requests: Vec::new(),
        missing: false,
    };
    let mut offset = 0;
    for count in [1, 2, 5, 3, 7] {
        streamed_renderer
            .render_with_pages(&mut actual[offset..offset + count], &mut pages)
            .expect("streamed one-shot render");
        offset += count;
    }
    assert_eq!(actual, expected);
    assert_eq!(actual[4..], [[0.0; 2]; 14]);
    assert!(pages.requests.is_empty());
}

#[test]
fn canonical_converted_asset_metadata_compiles_for_streamed_playback() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let asset = match editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "converted".into(),
            path: "converted-source.wav".into(),
            source_sample_rate: 44_100,
            source_frames: 44_100,
            playback_frames: 48_000,
            conversion_version: SINC_CONVERSION_V1,
        })
        .expect("prepared import")
    {
        Created::Imported { asset, .. } => asset,
        other => panic!("unexpected import result: {other:?}"),
    };
    let pcm = (0..48_000)
        .map(|index| match index % 4 {
            0 => [0.25, -0.5],
            1 => [-0.125, 0.75],
            2 => [0.5, 0.125],
            _ => [-0.25, -0.25],
        })
        .collect::<Vec<_>>();
    let resident = BTreeMap::from([(
        asset,
        PreparedAudioSource::Resident(
            Audio::new(48_000, pcm.clone()).expect("converted project-rate PCM"),
        ),
    )]);
    let streamed = BTreeMap::from([(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                StreamSourceKey::new(GENERATION, asset),
                44_100,
                48_000,
                44_100,
                48_000,
                SINC_CONVERSION_V1,
                [0x42; 32],
            )
            .expect("canonical converted metadata"),
        )),
    )]);
    let resident_project =
        compile::project_with_prepared_audio_sources(editor.project(), &resident, GENERATION)
            .expect("resident converted compile");
    let streamed_project =
        compile::project_with_prepared_audio_sources(editor.project(), &streamed, GENERATION)
            .expect("streamed converted compile");
    let mut resident_renderer = Renderer::new(&resident_project);
    let mut streamed_renderer = Renderer::new(&streamed_project);
    resident_renderer.play();
    streamed_renderer.play();
    let mut expected = [[0.0; 2]; 31];
    let mut actual = [[0.0; 2]; 31];
    resident_renderer
        .render(&mut expected)
        .expect("resident render");
    let key = StreamSourceKey::new(GENERATION, asset);
    let mut pages = Pages {
        key,
        frames: pcm,
        requests: Vec::new(),
        missing: false,
    };
    for (chunk_index, chunk) in [3, 1, 10, 2, 15].into_iter().enumerate() {
        let start = [0, 3, 4, 14, 16][chunk_index];
        streamed_renderer
            .render_with_pages(&mut actual[start..start + chunk], &mut pages)
            .expect("streamed render");
    }
    assert_eq!(actual, expected);
    assert!(pages.requests.is_empty());
}
