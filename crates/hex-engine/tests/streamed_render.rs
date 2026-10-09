use hex_engine::{
    Audio, BorrowedMediaPage, Clip, MediaPages, Project as EngineProject, Renderer,
    STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata, Track,
};
use hex_project::{AssetId, Command, Created, Editor, Project};

fn asset() -> AssetId {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    match editor
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    {
        Created::Asset(asset) => asset,
        other => panic!("unexpected command result: {other:?}"),
    }
}

fn source(generation: u64) -> StreamedSourceMetadata {
    StreamedSourceMetadata::new(
        StreamSourceKey::new(generation, asset()),
        48_000,
        48_000,
        (STREAM_PAGE_FRAMES + 16) as u64,
        (STREAM_PAGE_FRAMES + 16) as u64,
        0,
        [0x5a; 32],
    )
    .unwrap()
}

struct Pages {
    key: StreamSourceKey,
    data: Vec<[f32; 2]>,
    requested: Vec<(StreamSourceKey, u64)>,
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
        let end = start.checked_add(STREAM_PAGE_FRAMES)?.min(self.data.len());
        (start < end).then_some(BorrowedMediaPage {
            page_index,
            frames: &self.data[start..end],
        })
    }

    fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.requested.push((key, page_index));
    }
}

fn project(streamed: bool, looping: bool) -> (EngineProject, StreamSourceKey, Vec<[f32; 2]>) {
    let metadata = source(7);
    let key = metadata.key();
    let mut pcm = vec![[0.0, 0.0]; STREAM_PAGE_FRAMES + 16];
    for (index, frame) in pcm.iter_mut().enumerate() {
        *frame = [index as f32 / 32.0, -(index as f32) / 64.0];
    }
    let clip = if streamed {
        Clip::streamed_region(std::sync::Arc::new(metadata), 3, 12, 4093, 9, looping).unwrap()
    } else {
        Clip::region(
            Audio::new(48_000, pcm.clone()).unwrap(),
            3,
            12,
            4093,
            9,
            looping,
        )
        .unwrap()
    };
    (
        EngineProject::new(48_000, vec![Track::new(vec![clip], 0.5, false).unwrap()]).unwrap(),
        key,
        pcm,
    )
}

#[test]
fn borrowed_pages_match_resident_render_across_page_and_loop_boundaries() {
    for looping in [false, true] {
        let (resident, _, pcm) = project(false, looping);
        let (streamed, key, _) = project(true, looping);
        let mut resident_renderer = Renderer::new(&resident);
        let mut streamed_renderer = Renderer::new(&streamed);
        resident_renderer.play();
        streamed_renderer.play();
        let mut pages = Pages {
            key,
            data: pcm,
            requested: Vec::new(),
            missing: false,
        };
        let mut expected = [[0.0; 2]; 18];
        let mut actual = [[0.0; 2]; 18];
        resident_renderer.render(&mut expected).unwrap();
        for (offset, chunk) in [2, 5, 1, 7, 3].into_iter().enumerate() {
            let start = [0, 2, 7, 8, 15][offset];
            streamed_renderer
                .render_with_pages(&mut actual[start..start + chunk], &mut pages)
                .unwrap();
        }
        assert_eq!(actual, expected);
        assert!(pages.requested.is_empty());
    }
}

#[test]
fn missing_page_is_silent_without_stopping_clock_or_other_tracks() {
    let (streamed, key, pcm) = project(true, false);
    let resident_clip = Clip::new(
        Audio::new(48_000, vec![[1.0, -1.0]; 32]).unwrap(),
        0,
        32,
        false,
    )
    .unwrap();
    let mixed = EngineProject::new(
        48_000,
        vec![
            streamed.tracks()[0].clone(),
            Track::new(vec![resident_clip], 1.0, false).unwrap(),
        ],
    )
    .unwrap();
    let mut renderer = Renderer::new(&mixed);
    renderer.play();
    let mut pages = Pages {
        key,
        data: pcm,
        requested: Vec::new(),
        missing: true,
    };
    let mut output = [[0.0; 2]; 8];
    renderer.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output, [[1.0, -1.0]; 8]);
    assert_eq!(renderer.position(), 8);
    assert_eq!(pages.requested, [(key, 0), (key, 1)]);
}

#[test]
fn providerless_render_rejects_streamed_project_before_advancing() {
    let (streamed, _, _) = project(true, false);
    let mut renderer = Renderer::new(&streamed);
    renderer.play();
    let mut output = [[7.0, 8.0]; 2];
    assert!(renderer.render(&mut output).is_err());
    assert_eq!(renderer.position(), 0);
}

#[test]
fn streamed_source_metadata_rejects_wrong_conversion_duration() {
    assert!(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(3, asset()),
            44_100,
            48_000,
            44_100,
            47_999,
            1,
            [0; 32],
        )
        .is_err()
    );
}
