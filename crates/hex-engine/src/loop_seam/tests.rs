use super::LoopSeam;
use crate::playback::{Playback, PlaybackAction, PlaybackCommand};
use crate::{
    Audio, AudioSourceContext, BorrowedMediaPage, Frame, MediaPages, PreparedRevision,
    STREAM_PAGE_FRAMES, SineTable, StreamSourceKey, StreamedSourceMetadata,
    compile::PreparedAudioSource,
};
use hex_project::{AssetId, Command, Created, Editor, LoopRegion, Project};
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

/// One audio track with a clip at frame zero, looped over `region` if any.
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

/// DC that stops (`dc_first`) or starts at `EDGE`, inside the loop, so every
/// wrap jumps from silence to full scale (`dc_first`) or back.
fn stepped_dc(dc_first: bool) -> Vec<Frame> {
    (0..CLIP)
        .map(|frame| {
            if (frame < EDGE) == dc_first {
                [1.0, -1.0]
            } else {
                [0.0, 0.0]
            }
        })
        .collect()
}

fn resident(project: &Project, asset: AssetId, samples: &[Frame]) -> Playback {
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples.to_vec()).unwrap())]);
    let revision =
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None).unwrap();
    Playback::from_prepared(Arc::new(revision))
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

/// Play and render `frames` in blocks of `block`.
fn play(
    playback: &mut Playback,
    block: usize,
    frames: usize,
    mut pages: Option<&mut Pages<'_>>,
) -> Vec<Frame> {
    playback
        .try_apply_declicked_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut output = vec![[0.0; 2]; frames];
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

fn stepped_dc_seam_case(dc_first: bool, use_stream: bool) {
    let (looped, asset) = project(Some((LOOP_START, LOOP_END)));
    let samples = stepped_dc(dc_first);
    let reference = play(&mut resident(&looped, asset, &samples), 1, RENDERED, None);
    let (at_end, at_start) = if dc_first {
        ([0.0, 0.0], [1.0, -1.0])
    } else {
        ([1.0, -1.0], [0.0, 0.0])
    };
    let mut checked = 0;
    for wrap in wraps() {
        let step = largest_step(&reference[wrap - 2 * F..wrap + 2 * F]);
        assert!(
            step <= 1.0 / F as f32 + 1e-6,
            "dc_first {dc_first}: step {step} at wrap {wrap}"
        );
        // The outgoing side runs on past the loop end at (F-1)/F while
        // the loop start ramps in at 1/F; the pre-seam frame is untouched.
        assert_eq!(reference[wrap - 1], at_end, "dc_first {dc_first}");
        let first = 1.0 / F as f32;
        let expected = [
            at_start[0] * first + at_end[0] * (1.0 - first),
            at_start[1] * first + at_end[1] * (1.0 - first),
        ];
        assert!(
            (reference[wrap][0] - expected[0]).abs() < 1e-6
                && (reference[wrap][1] - expected[1]).abs() < 1e-6,
            "dc_first {dc_first}: {:?} != {expected:?}",
            reference[wrap]
        );
        assert_eq!(reference[wrap + F - 1], at_start, "dc_first {dc_first}");
        checked += 1;
    }
    assert_eq!(checked, 3);
    if !use_stream {
        for block in [64, 512] {
            let output = play(
                &mut resident(&looped, asset, &samples),
                block,
                RENDERED,
                None,
            );
            assert_eq!(output, reference, "dc_first {dc_first}: block {block}");
        }
    } else {
        for block in [1, 64, 512] {
            let mut pages = Pages {
                key: StreamSourceKey::new(31, asset),
                samples: &samples,
            };
            let output = play(
                &mut streamed(&looped, asset),
                block,
                RENDERED,
                Some(&mut pages),
            );
            assert_eq!(
                output, reference,
                "dc_first {dc_first}: streamed block {block}"
            );
        }
    }
}

macro_rules! stepped_dc_seam_tests {
    ($($name:ident: $dc_first:expr, $streamed:expr;)*) => {
        $(
            #[test]
            fn $name() {
                stepped_dc_seam_case($dc_first, $streamed);
            }
        )*
    };
}

stepped_dc_seam_tests! {
    stepped_dc_seam_first_resident: true, false;
    stepped_dc_seam_first_streamed: true, true;
    stepped_dc_seam_last_resident: false, false;
    stepped_dc_seam_last_streamed: false, true;
}

#[test]
fn content_continuous_across_the_seam_plays_without_a_dip() {
    // A period that divides the loop length: the material past the loop end
    // is exactly the material at its start.
    const PERIOD: usize = 600;
    let (looped, asset) = project(Some((2_400, 8_400)));
    let period: Vec<Frame> = (0..PERIOD)
        .map(|i| {
            let value = (i as f32 * std::f32::consts::TAU / PERIOD as f32).sin() * 0.5;
            [value, -value]
        })
        .collect();
    let samples: Vec<Frame> = (0..CLIP).map(|i| period[i % PERIOD]).collect();
    let frames = 8_400 + 2 * 6_000;
    for block in [1, 64, 512] {
        let output = play(&mut resident(&looped, asset, &samples), block, frames, None);
        // The un-looped continuation repeats the same period.
        let deviation = output
            .iter()
            .enumerate()
            .map(|(i, frame)| {
                let expected = period[i % PERIOD];
                (frame[0] - expected[0])
                    .abs()
                    .max((frame[1] - expected[1]).abs())
            })
            .fold(0.0, f32::max);
        assert_eq!(deviation, 0.0, "block {block}");
    }
}

#[test]
fn loop_off_output_is_the_unchanged_clip() {
    let (linear, asset) = project(None);
    for dc_first in [true, false] {
        let samples = stepped_dc(dc_first);
        for block in [1, 64, 512] {
            let output = play(
                &mut resident(&linear, asset, &samples),
                block,
                RENDERED,
                None,
            );
            assert_eq!(output[..CLIP], samples[..], "dc_first {dc_first}: {block}");
            assert!(output[CLIP..].iter().all(|frame| *frame == [0.0, 0.0]));
        }
    }
}

#[test]
fn the_first_pass_keeps_its_transient() {
    let (looped, asset) = project(Some((LOOP_START, LOOP_END)));
    let samples = stepped_dc(true);
    let mut playback = resident(&looped, asset, &samples);
    // Immediate Play (no transport resume fade) from the loop start.
    playback.apply(PlaybackCommand::PlayFrom(LOOP_START));
    let mut output = [[0.0; 2]; F];
    playback.render(&mut output).unwrap();
    assert!(output.iter().all(|frame| *frame == [1.0, -1.0]));
}

#[test]
fn a_seam_needs_a_wrapped_pass_and_a_loop_of_two_ramps() {
    let short = LoopRegion::new(10, 10 + 2 * F as u64 - 1, true).unwrap();
    assert_eq!(LoopSeam::for_pass(short, 1, RATE), None);
    let region = LoopRegion::new(10, 10 + 2 * F as u64, true).unwrap();
    assert_eq!(LoopSeam::for_pass(region, 0, RATE), None);
    let seam = LoopSeam::for_pass(region, 1, RATE).unwrap();
    // Only chunks starting inside the first F frames read an overrun, clipped
    // to the end of the crossfade.
    assert_eq!(seam.overrun(9, 64), None);
    assert_eq!(seam.overrun(10, 64), Some((10 + 2 * F as u64, 64)));
    assert_eq!(
        seam.overrun(10 + 100, 64),
        Some((10 + 2 * F as u64 + 100, 28))
    );
    assert_eq!(seam.overrun(10 + F as u64, 64), None);
}

#[test]
fn streamed_demand_pins_the_overrun_with_the_wrapped_chunks_that_read_it() {
    let (looped, asset) = project(Some((LOOP_START, LOOP_END)));
    let samples = stepped_dc(true);
    let key = StreamSourceKey::new(31, asset);
    let mut pages = Pages {
        key,
        samples: &samples,
    };
    let context = AudioSourceContext {
        project_generation: 1,
        editor_revision: 1,
    };
    let next_pages = |playback: &mut Playback| {
        let mut indices = Vec::new();
        playback
            .stream_playback(context)
            .visit_next_stream_chunk(256, Some(true), &mut |extent| {
                indices.push(extent.page_index);
                Ok(())
            })
            .unwrap();
        indices
    };
    // Frame 0 and loop start 2345 are in page 0; the overrun from 8901 is in
    // page 2, read only by a wrapped pass's first chunks.
    let overrun_page = LOOP_END / STREAM_PAGE_FRAMES as u64;
    let mut playback = streamed(&looped, asset);
    assert_eq!(next_pages(&mut playback), vec![0]);
    play(&mut playback, 512, LOOP_END as usize, Some(&mut pages));
    assert_eq!(playback.position(), LOOP_START);
    let wrapped = next_pages(&mut playback);
    assert!(wrapped.contains(&0), "{wrapped:?}");
    assert!(wrapped.contains(&overrun_page), "{wrapped:?}");
    // Past the crossfade the chunk demands only what it reads itself.
    play(&mut playback, 512, F, Some(&mut pages));
    assert_eq!(next_pages(&mut playback), vec![0]);
    // Loop off demands only what the chunk itself reads.
    let (linear, asset) = project(None);
    let mut playback = streamed(&linear, asset);
    assert_eq!(next_pages(&mut playback), vec![0]);
}

#[test]
fn a_latent_graph_crossfades_the_seam_it_actually_outputs() {
    const LATENCY: usize = 7;
    let mut f = crate::signal_graph::tests::fixture(&[&[LATENCY as u32]], CLIP);
    f.editor
        .execute(Command::SetLoopRegion {
            start: LOOP_START,
            end: LOOP_END,
            enabled: true,
        })
        .unwrap();
    let samples = stepped_dc(true);
    for audio in f.media.values_mut() {
        *audio = Audio::new(RATE, samples.clone()).unwrap();
    }
    let latent = || Playback::from_prepared(crate::signal_graph::tests::prepare(&f).unwrap());
    let mut playback = latent();
    assert_eq!(playback.main_output_latency_frames(), LATENCY as u32);
    let reference = play(&mut playback, 1, RENDERED, None);
    for wrap in wraps() {
        let seam = wrap + LATENCY;
        let step = largest_step(&reference[seam - 2 * F..seam + 2 * F]);
        assert!(step <= 1.0 / F as f32 + 1e-6, "step {step} at seam {seam}");
        assert_eq!(reference[seam - 1], [0.0, 0.0]);
        assert_eq!(reference[seam + F - 1], [1.0, -1.0]);
    }
    for block in [64, 512] {
        assert_eq!(
            play(&mut latent(), block, RENDERED, None),
            reference,
            "block {block}"
        );
    }
}
