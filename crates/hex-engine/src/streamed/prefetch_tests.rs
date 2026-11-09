//! The shared streamed span renderer asks for the next page from the second
//! half of each present page, and never past the source end (#4075).
use crate::playback::{Playback, PlaybackCommand};
use crate::{
    BorrowedMediaPage, Frame, MediaPages, PreparedRevision, STREAM_PAGE_FRAMES, SineTable,
    StreamSourceKey, StreamedSourceMetadata, compile::PreparedAudioSource,
};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
/// Two and a half pages.
const CLIP: usize = 5 * STREAM_PAGE_FRAMES / 2;

struct Pages {
    key: StreamSourceKey,
    samples: Vec<Frame>,
    /// (playhead frame, requested page) for every prefetch call.
    prefetches: Vec<(u64, u64)>,
    position: u64,
}

impl MediaPages for Pages {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        let start = usize::try_from(index)
            .ok()?
            .checked_mul(STREAM_PAGE_FRAMES)?;
        (key == self.key && start < self.samples.len()).then(|| BorrowedMediaPage {
            page_index: index,
            frames: &self.samples[start..(start + STREAM_PAGE_FRAMES).min(self.samples.len())],
        })
    }

    fn request_page(&mut self, _: StreamSourceKey, _: u64) {}

    fn prefetch_page(&mut self, key: StreamSourceKey, index: u64) {
        assert_eq!(key, self.key);
        self.prefetches.push((self.position, index));
    }
}

#[test]
fn present_pages_prefetch_the_next_page_from_their_second_half_only() {
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
    let revision = PreparedRevision::prepare_with_prepared_audio_sources(
        editor.project(),
        &sources,
        31,
        Arc::new(SineTable::new()),
        None,
    )
    .unwrap();
    let mut playback = Playback::from_prepared(Arc::new(revision));
    playback.apply(PlaybackCommand::Play);
    let mut pages = Pages {
        key,
        samples: vec![[0.5; 2]; CLIP],
        prefetches: Vec::new(),
        position: 0,
    };
    let mut block = [[0.0; 2]; 256];
    while pages.position < CLIP as u64 + 1_024 {
        playback.render_with_pages(&mut block, &mut pages).unwrap();
        pages.position += block.len() as u64;
    }
    let half = STREAM_PAGE_FRAMES as u64 / 2;
    let page = STREAM_PAGE_FRAMES as u64;
    assert!(!pages.prefetches.is_empty());
    for (position, next) in &pages.prefetches {
        // Requested from the second half of the page before `next`.
        let reading = next - 1;
        assert!(
            *position + 256 >= reading * page + half && *position < next * page,
            "{position} -> {next}"
        );
        // The final half page has no successor to ask for.
        assert!(*next < 3, "{next}");
    }
    assert!(pages.prefetches.iter().any(|(_, next)| *next == 1));
    assert!(pages.prefetches.iter().any(|(_, next)| *next == 2));
}
