use crate::playback::{Playback, PlaybackAction, PlaybackCommand};
use crate::{
    Audio, BorrowedMediaPage, Frame, MediaPages, PreparedRevision, STREAM_PAGE_FRAMES, SineTable,
    StreamSourceKey, StreamedSourceMetadata, compile::PreparedAudioSource,
};
use hex_project::{AssetId, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
const F: usize = 128;
const CLIP: usize = 12_000;
/// An arbitrary, block-unaligned loop inside the clip.
const LOOP_START: u64 = 2_345;
const LOOP_END: u64 = 8_901;
/// Where the DC content starts or stops inside the loop.
const EDGE: usize = 5_000;
/// Three wraps when playing from frame zero.
const RENDERED: usize = LOOP_END as usize + 3 * (LOOP_END - LOOP_START) as usize - 100;

/// DC that stops (`dc_first`) or starts at `EDGE`, inside the loop, so every
/// wrap jumps from silence to full scale (`dc_first`) or back. The loop is set when `looped`.
fn project(dc_first: bool, looped: bool) -> (Project, AssetId, Vec<Frame>) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "dc".into(),
            path: "dc.wav".into(),
            sample_rate: RATE,
            frames: CLIP as u64,
        })
        .unwrap()
    else {
        panic!("import result");
    };
    if looped {
        editor
            .execute(Command::SetLoopRegion {
                start: LOOP_START,
                end: LOOP_END,
                enabled: true,
            })
            .unwrap();
    }
    let samples = (0..CLIP)
        .map(|frame| {
            if (frame < EDGE) == dc_first {
                [1.0, -1.0]
            } else {
                [0.0, 0.0]
            }
        })
        .collect();
    (editor.project().clone(), asset, samples)
}

fn resident(project: &Project, asset: AssetId, samples: &[Frame]) -> Playback {
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples.to_vec()).unwrap())]);
    let revision =
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None).unwrap();
    let playback = Playback::from_prepared(Arc::new(revision));
    assert!(playback.session_runtime.is_some());
    playback
}

fn streamed(project: &Project, asset: AssetId) -> Playback {
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
        project,
        &sources,
        31,
        Arc::new(SineTable::new()),
        None,
    )
    .unwrap();
    Playback::from_prepared(Arc::new(revision))
}

/// Every page of one streamed source, always installed.
struct Pages<'a> {
    key: StreamSourceKey,
    samples: &'a [Frame],
}

impl MediaPages for Pages<'_> {
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
}

/// Play from frame zero and render `RENDERED` frames in blocks of `block`.
fn play(playback: &mut Playback, block: usize, mut pages: Option<&mut Pages<'_>>) -> Vec<Frame> {
    playback
        .try_apply_declicked_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut output = vec![[0.0; 2]; RENDERED];
    for chunk in output.chunks_mut(block) {
        match pages.as_deref_mut() {
            Some(pages) => playback.render_with_pages(chunk, pages).unwrap(),
            None => playback.render(chunk).unwrap(),
        }
    }
    output
}

/// Output frames at which the transport wraps, playing from zero.
fn wraps() -> impl Iterator<Item = usize> {
    let length = (LOOP_END - LOOP_START) as usize;
    (0..)
        .map(move |pass| LOOP_END as usize + pass * length)
        .take_while(|wrap| *wrap < RENDERED)
}

fn largest_step(output: &[Frame]) -> f32 {
    output
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
fn loop_wrap_ramps_dc_out_and_in_identically_for_every_block_size() {
    for dc_first in [true, false] {
        let (looped, asset, samples) = project(dc_first, true);
        let reference = play(&mut resident(&looped, asset, &samples), 1, None);
        let mut checked = 0;
        for wrap in wraps() {
            let around = &reference[wrap - 2 * F..wrap + 2 * F];
            let step = largest_step(around);
            assert!(
                step <= 1.0 / F as f32 + 1e-6,
                "dc_first {dc_first}: step {step} at wrap {wrap}"
            );
            // Out over F frames to silence at the seam, back in over F.
            let (before, after) = if dc_first {
                ([0.0, 0.0], [1.0, -1.0])
            } else {
                ([1.0, -1.0], [0.0, 0.0])
            };
            assert_eq!(reference[wrap - F - 1], before, "dc_first {dc_first}");
            assert_eq!(reference[wrap - 1], [0.0, 0.0], "dc_first {dc_first}");
            assert_eq!(reference[wrap + F - 1], after, "dc_first {dc_first}");
            checked += 1;
        }
        assert_eq!(checked, 3);
        for block in [64, 512] {
            let output = play(&mut resident(&looped, asset, &samples), block, None);
            assert_eq!(output, reference, "dc_first {dc_first}: block {block}");
        }
        for block in [1, 64, 512] {
            let mut pages = Pages {
                key: StreamSourceKey::new(31, asset),
                samples: &samples,
            };
            let output = play(&mut streamed(&looped, asset), block, Some(&mut pages));
            assert_eq!(
                output, reference,
                "dc_first {dc_first}: streamed block {block}"
            );
        }
    }
}

#[test]
fn loop_off_output_is_the_unramped_clip() {
    for dc_first in [true, false] {
        let (linear, asset, samples) = project(dc_first, false);
        for block in [1, 64, 512] {
            let output = play(&mut resident(&linear, asset, &samples), block, None);
            assert_eq!(output[..CLIP], samples[..], "dc_first {dc_first}: {block}");
            assert!(output[CLIP..].iter().all(|frame| *frame == [0.0, 0.0]));
        }
    }
}

#[test]
fn ramp_follows_the_heard_seam_behind_main_latency() {
    let mut ramp = super::LoopWrapRamp::default();
    let region = hex_project::LoopRegion::new(10, 20, true).unwrap();
    let latency = 3;
    let frames = 4;
    // Cursor 14..20 then wraps to 10; the seam is heard 3 frames after the wrap.
    let mut output = vec![[1.0, 1.0]; 13];
    ramp.apply(&mut output, true, 14, Some(region), latency, frames);
    let gains: Vec<f32> = output.iter().map(|frame| frame[0]).collect();
    // 6 frames to the wrap plus 3 of latency: seam after output frame 8.
    assert_eq!(
        gains,
        [
            1.0, 1.0, 1.0, 1.0, 1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75, 1.0
        ]
    );
    // The ramp-in finished; a stopped transport is untouched.
    let mut stopped = vec![[1.0, 1.0]; 4];
    ramp.apply(&mut stopped, false, 18, Some(region), latency, frames);
    assert!(stopped.iter().all(|frame| *frame == [1.0, 1.0]));
}

#[test]
fn a_loop_shorter_than_two_ramps_is_left_unramped() {
    let mut ramp = super::LoopWrapRamp::default();
    let region = hex_project::LoopRegion::new(10, 10 + 2 * F as u64 - 1, true).unwrap();
    let mut output = vec![[1.0, 1.0]; 4 * F];
    ramp.apply(&mut output, true, 10, Some(region), 0, F as u32);
    assert!(output.iter().all(|frame| *frame == [1.0, 1.0]));
    // From two ramps long, every pass dips at its seam.
    let region = hex_project::LoopRegion::new(10, 10 + 2 * F as u64, true).unwrap();
    ramp.apply(&mut output, true, 10, Some(region), 0, F as u32);
    assert_eq!(output[2 * F - 1], [0.0, 0.0]);
    assert_eq!(output[2 * F + F - 1], [1.0, 1.0]);
    assert_eq!(output[4 * F - 1], [0.0, 0.0]);
}

#[test]
fn a_latent_graph_ramps_around_the_seam_it_actually_outputs() {
    const LATENCY: usize = 7;
    let mut f = crate::signal_graph::tests::fixture(&[&[LATENCY as u32]], CLIP);
    f.editor
        .execute(Command::SetLoopRegion {
            start: LOOP_START,
            end: LOOP_END,
            enabled: true,
        })
        .unwrap();
    let (_, _, samples) = project(true, false);
    for audio in f.media.values_mut() {
        *audio = Audio::new(RATE, samples.clone()).unwrap();
    }
    let latent = || Playback::from_prepared(crate::signal_graph::tests::prepare(&f).unwrap());
    let mut playback = latent();
    assert_eq!(playback.main_output_latency_frames(), LATENCY as u32);
    let reference = play(&mut playback, 1, None);
    for wrap in wraps() {
        let seam = wrap + LATENCY;
        let step = largest_step(&reference[seam - 2 * F..seam + 2 * F]);
        assert!(step <= 1.0 / F as f32 + 1e-6, "step {step} at seam {seam}");
        assert_eq!(reference[seam - 1], [0.0, 0.0]);
        assert_eq!(reference[seam + F - 1], [1.0, -1.0]);
    }
    for block in [64, 512] {
        assert_eq!(play(&mut latent(), block, None), reference, "block {block}");
    }
}
