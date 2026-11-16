use crate::{
    BorrowedMediaPage, Clip, Frame, MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey,
    StreamedSourceMetadata, Track,
};
use hex_project::{Command, Created, Editor, Project};
use std::sync::Arc;

struct Pages {
    key: StreamSourceKey,
    samples: Vec<Frame>,
    missing: Option<u64>,
    requests: Vec<u64>,
    prefetch: Vec<u64>,
}
impl MediaPages for Pages {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        let start = usize::try_from(index)
            .ok()?
            .checked_mul(STREAM_PAGE_FRAMES)?;
        (key == self.key && self.missing != Some(index) && start < self.samples.len()).then(|| {
            BorrowedMediaPage {
                page_index: index,
                frames: &self.samples[start..(start + STREAM_PAGE_FRAMES).min(self.samples.len())],
            }
        })
    }
    fn request_page(&mut self, _: StreamSourceKey, page: u64) {
        self.requests.push(page);
    }
    fn prefetch_page(&mut self, _: StreamSourceKey, page: u64) {
        self.prefetch.push(page);
    }
}

#[test]
fn reversed_stream_matches_resident_pcm_and_page_demands_across_boundaries() {
    let length = STREAM_PAGE_FRAMES * 2 + 17;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: length as u64,
        })
        .unwrap()
    else {
        panic!("import");
    };
    let key = StreamSourceKey::new(1, asset);
    let source = Arc::new(
        StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            length as u64,
            length as u64,
            hex_project::IDENTITY_CONVERSION,
            [1; 32],
        )
        .unwrap(),
    );
    assert!(Clip::streamed_region_reversed(Arc::clone(&source), 0, 0, 0, 1, false).is_err());
    assert!(Clip::streamed_region_reversed(Arc::clone(&source), 0, 1, u64::MAX, 2, false).is_err());
    assert!(
        Clip::streamed_region_reversed(Arc::clone(&source), 0, 1, length as u64, 1, false).is_err()
    );
    let clip =
        Clip::streamed_region_reversed(source, 0, length as u64, 0, length as u64, false).unwrap();
    let track = Track::new(vec![clip], 1.0, false).unwrap();
    let samples: Vec<Frame> = (0..length).map(|i| [i as f32, -(i as f32)]).collect();
    let mut pages = Pages {
        key,
        samples,
        missing: None,
        requests: Vec::new(),
        prefetch: Vec::new(),
    };
    let expected: Vec<Frame> = pages.samples.iter().rev().copied().collect();
    let resident = Track::new(
        vec![
            Clip::new(
                crate::Audio::new(48_000, expected.clone()).unwrap(),
                0,
                length as u64,
                false,
            )
            .unwrap(),
        ],
        1.0,
        false,
    )
    .unwrap();
    for block_size in [1, 7, 64, 127, 512] {
        let mut output = vec![[0.0; 2]; length];
        let mut resident_output = vec![[0.0; 2]; length];
        for (block, (chunk, resident_chunk)) in output
            .chunks_mut(block_size)
            .zip(resident_output.chunks_mut(block_size))
            .enumerate()
        {
            let start = (block * block_size) as u64;
            track
                .render_additive_with_pages(start, chunk, &mut pages, None)
                .unwrap();
            resident.render_additive(start, resident_chunk).unwrap();
        }
        assert_eq!(output, expected);
        assert_eq!(output, resident_output);
    }
    let mut output = [[0.0; 2]; 64];
    let mut captured = [[0.0; 2]; 64];
    track
        .render_additive_captured(
            0,
            0,
            &mut output,
            &[],
            Some(&mut pages),
            &mut captured,
            &[],
            None,
            None,
        )
        .unwrap();
    assert_eq!(output, expected[..64]);
    assert_eq!(captured, output);
    let mut extents = Vec::new();
    track
        .visit_stream_demand(0, 64, &mut |extent| {
            extents.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(extents.len(), 2);
    assert_eq!(
        (extents[0].page_index, extents[0].start, extents[0].end),
        (2, 0, 17)
    );
    assert_eq!(
        (extents[1].page_index, extents[1].start, extents[1].end),
        (1, STREAM_PAGE_FRAMES - 47, STREAM_PAGE_FRAMES)
    );
    assert!(pages.prefetch.contains(&1));
    assert!(pages.prefetch.contains(&0));
}

#[test]
fn reversed_loop_fade_and_seek_keep_exact_source_positions() {
    let length = STREAM_PAGE_FRAMES + 200;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: length as u64,
        })
        .unwrap()
    else {
        panic!("import");
    };
    let key = StreamSourceKey::new(2, asset);
    let source = Arc::new(
        StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            length as u64,
            length as u64,
            hex_project::IDENTITY_CONVERSION,
            [2; 32],
        )
        .unwrap(),
    );
    let fade = hex_project::ClipFade::new(311, 0, 20, 20).unwrap();
    let clip = Clip::streamed_region_reversed_with_fade(source, 9, 311, 4070, 100, true, fade)
        .unwrap()
        .with_source_phase(21)
        .unwrap();
    let track = Track::new(vec![clip], 1.0, false).unwrap();
    let samples: Vec<Frame> = (0..length).map(|i| [i as f32 / 8192.0; 2]).collect();
    let mut pages = Pages {
        key,
        samples,
        missing: None,
        requests: Vec::new(),
        prefetch: Vec::new(),
    };
    for (start, size) in [(9, 311), (23, 19), (87, 131), (190, 100)] {
        let mut output = vec![[0.0; 2]; size];
        track
            .render_additive_with_pages(start, &mut output, &mut pages, None)
            .unwrap();
        for (index, frame) in output.iter().enumerate() {
            let elapsed = start as usize - 9 + index;
            let phase = (elapsed + 21) % 100;
            let sample = pages.samples[4070 + 99 - phase][0] * fade.gain_at(elapsed as u64);
            assert_eq!(*frame, [sample; 2], "elapsed {elapsed}");
        }
    }
}

#[test]
fn reversed_missing_page_fades_out_and_recovers_in_playback_order() {
    let length = STREAM_PAGE_FRAMES * 2 + 17;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: length as u64,
        })
        .unwrap()
    else {
        panic!("import");
    };
    let key = StreamSourceKey::new(3, asset);
    let source = Arc::new(
        StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            length as u64,
            length as u64,
            hex_project::IDENTITY_CONVERSION,
            [3; 32],
        )
        .unwrap(),
    );
    let track = Track::new(
        vec![
            Clip::streamed_region_reversed(source, 0, length as u64, 0, length as u64, false)
                .unwrap(),
        ],
        1.0,
        false,
    )
    .unwrap();
    let mut pages = Pages {
        key,
        samples: vec![[1.0; 2]; length],
        missing: Some(1),
        requests: Vec::new(),
        prefetch: Vec::new(),
    };
    let mut fades = crate::stream_miss_fade::StreamMissFades::new(48_000);
    let mut output = [[0.0; 2]; 145];
    {
        let mut view = crate::stream_miss_fade::MissFadedPages {
            pages: &mut pages,
            fades: &mut fades,
        };
        track
            .render_additive_with_pages(0, &mut output[..81], &mut view, None)
            .unwrap();
    }
    assert!(pages.requests.contains(&1));
    pages.missing = None;
    {
        let mut view = crate::stream_miss_fade::MissFadedPages {
            pages: &mut pages,
            fades: &mut fades,
        };
        track
            .render_additive_with_pages(81, &mut output[81..], &mut view, None)
            .unwrap();
    }
    assert!(output[80][0] < 0.6 && output[80][0] > 0.4);
    assert!(output[144][0] > 0.99);
    for pair in output.windows(2) {
        assert!((pair[1][0] - pair[0][0]).abs() <= 1.0 / 128.0 + 1e-6);
    }
}

#[test]
fn reversed_short_page_is_silent_only_for_unavailable_tail() {
    let length = STREAM_PAGE_FRAMES + 100;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: length as u64,
        })
        .unwrap()
    else {
        panic!("import");
    };
    let key = StreamSourceKey::new(4, asset);
    let source = Arc::new(
        StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            length as u64,
            length as u64,
            hex_project::IDENTITY_CONVERSION,
            [4; 32],
        )
        .unwrap(),
    );
    let track = Track::new(
        vec![
            Clip::streamed_region_reversed(source, 0, length as u64, 0, length as u64, false)
                .unwrap(),
        ],
        1.0,
        false,
    )
    .unwrap();
    let mut pages = Pages {
        key,
        samples: vec![[0.5; 2]; length - 7],
        missing: None,
        requests: Vec::new(),
        prefetch: Vec::new(),
    };
    let mut output = [[0.0; 2]; 32];
    track
        .render_additive_with_pages(0, &mut output, &mut pages, None)
        .unwrap();
    assert_eq!(&output[..7], &[[0.0; 2]; 7]);
    assert_eq!(&output[7..], &[[0.5; 2]; 25]);
    assert_eq!(pages.requests, vec![1]);
}

#[test]
fn reversed_twenty_million_frame_region_uses_bounded_pages() {
    const LENGTH: u64 = 20_000_000;
    struct VirtualPages {
        key: StreamSourceKey,
        frames: [Frame; STREAM_PAGE_FRAMES],
    }
    impl MediaPages for VirtualPages {
        fn page(&mut self, key: StreamSourceKey, page: u64) -> Option<BorrowedMediaPage<'_>> {
            if key != self.key {
                return None;
            }
            let start = page.checked_mul(STREAM_PAGE_FRAMES as u64)?;
            if start >= LENGTH {
                return None;
            }
            for (index, frame) in self.frames.iter_mut().enumerate() {
                let value = (start + index as u64) as f32;
                *frame = [value, -value];
            }
            let count = (LENGTH - start).min(STREAM_PAGE_FRAMES as u64) as usize;
            Some(BorrowedMediaPage {
                page_index: page,
                frames: &self.frames[..count],
            })
        }
        fn request_page(&mut self, _: StreamSourceKey, _: u64) {}
    }
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: LENGTH,
        })
        .unwrap()
    else {
        panic!("import");
    };
    let key = StreamSourceKey::new(5, asset);
    let source = Arc::new(
        StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            LENGTH,
            LENGTH,
            hex_project::IDENTITY_CONVERSION,
            [5; 32],
        )
        .unwrap(),
    );
    let track = Track::new(
        vec![Clip::streamed_region_reversed(source, 0, LENGTH, 0, LENGTH, false).unwrap()],
        1.0,
        false,
    )
    .unwrap();
    let mut pages = VirtualPages {
        key,
        frames: [[0.0; 2]; STREAM_PAGE_FRAMES],
    };
    let mut output = [[0.0; 2]; 64];
    track
        .render_additive_with_pages(0, &mut output, &mut pages, None)
        .unwrap();
    for (index, frame) in output.iter().enumerate() {
        let value = (LENGTH - 1 - index as u64) as f32;
        assert_eq!(*frame, [value, -value]);
    }
}
