use super::StreamMissFades;
use crate::playback::{Playback, PlaybackAction, PlaybackCommand};
use crate::{
    BorrowedMediaPage, Frame, MediaPages, PreparedRevision, STREAM_PAGE_FRAMES, SineTable,
    StreamSourceKey, StreamedSourceMetadata, compile::PreparedAudioSource,
};
use hex_project::{AssetId, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
const F: usize = 128;
const CLIP: usize = 12_000;
const LEVEL: f32 = 0.5;
/// Page 1 (frames 4096..8192) is withheld until the transport reaches this
/// frame, so playback resumes in the middle of the page.
const RESUME: u64 = 6_000;
const MISS_START: usize = STREAM_PAGE_FRAMES;
/// The largest step a miss may add to a DC signal of `LEVEL`.
const BOUND: f32 = LEVEL / F as f32 + 1e-6;

fn project(region: Option<(u64, u64)>) -> (Project, AssetId) {
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
    if let Some((start, end)) = region {
        editor
            .execute(Command::SetLoopRegion {
                start,
                end,
                enabled: true,
            })
            .unwrap();
    }
    (editor.project().clone(), asset)
}

fn key(asset: AssetId) -> StreamSourceKey {
    StreamSourceKey::new(31, asset)
}

fn streamed(project: &Project, asset: AssetId) -> Playback {
    let metadata = StreamedSourceMetadata::new(
        key(asset),
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
        project,
        &sources,
        31,
        Arc::new(SineTable::new()),
        None,
    )
    .unwrap();
    Playback::from_prepared(Arc::new(revision))
}

/// DC pages of one source; `missing` withholds one page index.
struct Pages {
    key: StreamSourceKey,
    samples: Vec<Frame>,
    missing: Option<u64>,
}

impl Pages {
    fn new(asset: AssetId) -> Self {
        Self {
            key: key(asset),
            samples: vec![[LEVEL, -LEVEL]; CLIP],
            missing: Some(1),
        }
    }
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

fn start(playback: &mut Playback) {
    playback
        .try_apply_declicked_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
}

/// Render `frames` in blocks of `block`, installing the missing page once the
/// transport has reached `RESUME`.
fn render(playback: &mut Playback, pages: &mut Pages, block: usize, frames: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; frames];
    for chunk in output.chunks_mut(block) {
        if playback.position() >= RESUME {
            pages.missing = None;
        }
        playback.render_with_pages(chunk, pages).unwrap();
    }
    output
}

fn largest_step(frames: &[Frame]) -> f32 {
    frames
        .windows(2)
        .flat_map(|pair| {
            [
                (pair[1][0] - pair[0][0]).abs(),
                (pair[1][1] - pair[0][1]).abs(),
            ]
        })
        .fold(0.0, f32::max)
}

#[test]
fn present_missing_present_fades_both_edges_at_every_block_size() {
    let (project, asset) = project(None);
    for block in [1, 7, 64, 512] {
        let mut playback = streamed(&project, asset);
        let mut pages = Pages::new(asset);
        start(&mut playback);
        let output = render(&mut playback, &mut pages, block, CLIP - 1_000);
        let step = largest_step(&output);
        assert!(step <= BOUND, "block {block}: step {step}");
        // Untouched before the miss, silent once faded, full after recovery.
        assert!(output[..MISS_START].iter().all(|f| *f == [LEVEL, -LEVEL]));
        assert_eq!(
            output[MISS_START],
            [LEVEL, -LEVEL].map(|v| v * 127.0 / 128.0)
        );
        assert!(
            output[MISS_START + F..RESUME as usize]
                .iter()
                .all(|f| *f == [0.0; 2])
        );
        let resumed = output[RESUME as usize..]
            .iter()
            .position(|f| *f != [0.0; 2])
            .unwrap()
            + RESUME as usize;
        assert!(
            resumed < RESUME as usize + block,
            "block {block}: {resumed}"
        );
        assert!(
            output[resumed][0] <= BOUND,
            "recovery ramps in from silence"
        );
        assert!(
            output[resumed + F..].iter().all(|f| *f == [LEVEL, -LEVEL]),
            "block {block}: full level F frames after the page returns"
        );
    }
}

#[test]
fn a_voice_that_never_misses_is_bit_identical() {
    let (project, asset) = project(None);
    let mut playback = streamed(&project, asset);
    let mut pages = Pages::new(asset);
    pages.missing = None;
    start(&mut playback);
    let output = render(&mut playback, &mut pages, 64, CLIP);
    assert!(output.iter().all(|f| *f == [LEVEL, -LEVEL]));
}

#[test]
fn a_seek_starts_a_fresh_voice_at_full_level() {
    let (project, asset) = project(None);
    let mut playback = streamed(&project, asset);
    let mut pages = Pages::new(asset);
    start(&mut playback);
    let faded = render(&mut playback, &mut pages, 64, MISS_START + 2 * F);
    assert_eq!(faded[MISS_START + 2 * F - 1], [0.0; 2]);
    // Seek into present page 0 while the old voice is silent: the new
    // position does not inherit its zero gain.
    playback
        .try_apply_declicked_action(PlaybackAction::Transport(PlaybackCommand::Seek(100)))
        .unwrap();
    let after = render(&mut playback, &mut pages, 64, 4 * F);
    assert_eq!(after.last(), Some(&[LEVEL, -LEVEL]));
    assert!(
        after[..F].iter().all(|f| f[0] <= LEVEL),
        "the transport seek declick, not the miss gain, shapes the edge"
    );
    let fresh = after.iter().position(|f| *f == [LEVEL, -LEVEL]).unwrap();
    assert!(fresh <= 2 * F, "{fresh}");
}

#[test]
fn a_loop_wrap_out_of_a_miss_starts_a_fresh_voice() {
    // The loop ends inside the missing page; each wrap returns to page 0.
    let (project, asset) = project(Some((1_000, 5_000)));
    for block in [1, 64, 512] {
        let mut playback = streamed(&project, asset);
        let mut pages = Pages::new(asset);
        start(&mut playback);
        // RESUME is never reached inside the loop: page 1 stays missing.
        let output = render(&mut playback, &mut pages, block, 5_000 + 3 * 4_000);
        let step = largest_step(&output);
        assert!(step <= BOUND, "block {block}: step {step}");
        for wrap in [5_000, 9_000, 13_000] {
            // Each pass reaches full level again in page 0 and fades out
            // again at the page 1 miss.
            assert_eq!(output[wrap + 2 * F], [LEVEL, -LEVEL], "block {block}");
            assert_eq!(output[wrap + 3_096 + F], [0.0; 2], "block {block}");
        }
    }
}

#[test]
fn graph_replacement_keeps_a_continuing_voice_fading() {
    let (project, asset) = project(None);
    for block in [1, 64] {
        let mut old = streamed(&project, asset);
        let mut pages = Pages::new(asset);
        start(&mut old);
        let mut output = render(&mut old, &mut pages, block, MISS_START + F / 2);
        let mut next = streamed(&project, asset);
        next.try_inherit_runtime_from(&mut old).unwrap();
        output.extend(render(&mut next, &mut pages, block, 3 * F));
        let step = largest_step(&output);
        assert!(step <= BOUND, "block {block}: step {step}");
        assert_eq!(output.last(), Some(&[0.0; 2]));
    }
}

#[test]
fn table_continues_contiguous_reads_and_resets_on_any_discontinuity() {
    let asset = project(None).1;
    let mut fades = StreamMissFades::new(RATE);
    let held = [LEVEL, -LEVEL];
    let mut voice = fades.voice(key(asset), 0);
    assert_eq!(voice.present(held), held);
    assert_eq!(voice.missing(), held.map(|v| v * 127.0 / 128.0));
    // The next contiguous frame continues the faded voice.
    let mut voice = fades.voice(key(asset), 2);
    assert_eq!(voice.missing(), held.map(|v| v * 126.0 / 128.0));
    assert_eq!(voice.present(held), held.map(|v| v * 127.0 / 128.0));
    // A jump (seek, loop wrap, scene launch) or another source starts fresh.
    let mut jumped = fades.voice(key(asset), 100);
    assert_eq!(jumped.present(held), held);
    let other = StreamSourceKey::new(32, asset);
    let mut other = fades.voice(other, 4);
    assert_eq!(
        other.missing(),
        [0.0; 2],
        "a fresh voice has nothing to hold"
    );
    // The source end clears the voice without a concealment tail.
    let voice = fades.voice(key(asset), 4);
    voice.end();
    assert_eq!(fades.voice(key(asset), 5).missing(), [0.0; 2]);
}

#[test]
fn eviction_reuses_the_least_recently_touched_voice() {
    let asset = project(None).1;
    let mut fades = StreamMissFades::new(RATE);
    for voice in 0..super::STREAM_MISS_FADE_VOICES as u64 {
        fades.voice(key(asset), voice * 10).present([1.0; 2]);
    }
    // Touch voice 0 again, then start one more: voice 1 is evicted.
    fades.voice(key(asset), 1).present([1.0; 2]);
    fades.voice(key(asset), 99_999).present([1.0; 2]);
    assert_eq!(
        fades.voice(key(asset), 2).missing(),
        [127.0 / 128.0; 2],
        "voice 0 kept its held sample"
    );
    assert_eq!(
        fades.voice(key(asset), 11).missing(),
        [0.0; 2],
        "voice 1 was evicted"
    );
}
