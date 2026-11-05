//! The streamed page-miss fade path renders without callback allocation.
use hex_engine::{
    BorrowedMediaPage, Frame, MediaPages, PreparedRevision, STREAM_PAGE_FRAMES, SineTable,
    StreamSourceKey, StreamedSourceMetadata,
    compile::PreparedAudioSource,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
const CLIP: usize = 3 * STREAM_PAGE_FRAMES;
const LEVEL: f32 = 0.5;

/// DC pages of one source; `missing` withholds one page index.
struct Pages {
    key: StreamSourceKey,
    samples: Vec<Frame>,
    missing: Option<u64>,
}

impl MediaPages for Pages {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        if key != self.key || self.missing == Some(index) {
            return None;
        }
        let start = usize::try_from(index)
            .ok()?
            .checked_mul(STREAM_PAGE_FRAMES)?;
        (start < self.samples.len()).then(|| BorrowedMediaPage {
            page_index: index,
            frames: &self.samples[start..(start + STREAM_PAGE_FRAMES).min(self.samples.len())],
        })
    }

    fn request_page(&mut self, _: StreamSourceKey, _: u64) {}
}

/// Present, missing (fading out to silence), then present again from the
/// middle of the page (fading in), at several block sizes; every render call
/// is measured.
#[test]
fn present_missing_present_fade_is_callback_allocation_free() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: RATE,
            frames: CLIP as u64,
        })
        .unwrap()
    else {
        panic!("import result");
    };
    let key = StreamSourceKey::new(31, asset);
    let metadata = StreamedSourceMetadata::new(
        key,
        RATE,
        RATE,
        CLIP as u64,
        CLIP as u64,
        hex_project::IDENTITY_CONVERSION,
        [31; 32],
    )
    .unwrap();
    let sources = BTreeMap::from([(asset, PreparedAudioSource::Streamed(Arc::new(metadata)))]);
    let revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            editor.project(),
            &sources,
            31,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let resume = (STREAM_PAGE_FRAMES + STREAM_PAGE_FRAMES / 2) as u64;
    for block in [1, 64, 512] {
        let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut pages = Pages {
            key,
            samples: vec![[LEVEL, -LEVEL]; CLIP],
            missing: Some(1),
        };
        let mut output = vec![[0.0; 2]; 2 * STREAM_PAGE_FRAMES];
        for chunk in output.chunks_mut(block) {
            if playback.position() >= resume {
                pages.missing = None;
            }
            let mut result = Ok(());
            let counts = allocation_counter::measure(|| {
                result = playback.render_with_pages(chunk, &mut pages);
            });
            assert!(result.is_ok());
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0),
                "block {block}"
            );
        }
        let faded = STREAM_PAGE_FRAMES + 128;
        assert_eq!(output[faded], [0.0; 2], "block {block}: the miss faded out");
        assert!(
            output[STREAM_PAGE_FRAMES - 1..faded]
                .iter()
                .all(|frame| frame[0] > 0.0 || *frame == [0.0; 2]),
            "block {block}: concealed, not cut"
        );
        assert_eq!(
            output.last(),
            Some(&[LEVEL, -LEVEL]),
            "block {block}: recovered"
        );
    }
}
