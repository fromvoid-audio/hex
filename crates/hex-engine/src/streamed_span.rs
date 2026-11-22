//! Contiguous streamed source span rendering shared by Arrangement and Session.
use crate::{Frame, MediaPages, PreparedTrackMix, STREAM_PAGE_FRAMES, StreamedSourceMetadata};

/// Add one contiguous streamed source span, split at its page boundary. Both
/// Arrangement clips and Session one-shots use this path so page validation,
/// short-page silence, and bounded miss requests have one implementation.
#[derive(Clone, Copy)]
pub(crate) struct StreamedPageSpan<'a> {
    pub source_frame: u64,
    pub reversed: bool,
    pub timeline_frame: u64,
    pub output_offset: usize,
    pub count: usize,
    pub gain: f32,
    pub fade: Option<(&'a hex_project::ClipFade, u64)>,
}

pub(crate) fn render_streamed_page_span(
    mix: PreparedTrackMix,
    source: &StreamedSourceMetadata,
    span: StreamedPageSpan<'_>,
    output: &mut [Frame],
    pages: &mut (impl MediaPages + ?Sized),
) -> usize {
    render_streamed_page_span_captured(mix, source, span, output, pages, None)
}

fn render_forward_streamed_page_span_captured(
    mix: PreparedTrackMix,
    source: &StreamedSourceMetadata,
    span: StreamedPageSpan<'_>,
    output: &mut [Frame],
    pages: &mut (impl MediaPages + ?Sized),
    mut capture: Option<&mut [Frame]>,
) -> usize {
    let StreamedPageSpan {
        source_frame,
        reversed: _,
        timeline_frame,
        output_offset,
        count: requested_count,
        fade,
        gain: clip_gain,
    } = span;
    if requested_count == 0 {
        return 0;
    }
    let page_index = source_frame / STREAM_PAGE_FRAMES as u64;
    let page_offset = (source_frame % STREAM_PAGE_FRAMES as u64) as usize;
    let count = requested_count.min(STREAM_PAGE_FRAMES - page_offset);
    let Some(output_end) = output_offset.checked_add(count) else {
        return 0;
    };
    let Some(destination) = output.get_mut(output_offset..output_end) else {
        return 0;
    };
    let (page, fades) = pages.page_and_miss_fades(source.key(), page_index);
    let (frames, valid) = match page {
        Some(page) if page.page_index == page_index => (page.frames, true),
        _ => (&[][..], false),
    };
    let available = frames.len().saturating_sub(page_offset).min(count);
    let mut voice = fades.map(|fades| fades.directed_voice(source.key(), source_frame, false));
    for (offset, destination) in destination.iter_mut().take(available).enumerate() {
        let mut sample = frames[page_offset + offset];
        if let Some((fade, fade_start)) = fade
            && let Some(frame) = timeline_frame.checked_add(offset as u64)
        {
            let gain = fade.gain_at(frame.saturating_sub(fade_start));
            sample[0] *= gain;
            sample[1] *= gain;
        }
        if clip_gain != 1.0 {
            sample[0] *= clip_gain;
            sample[1] *= clip_gain;
        }
        if let Some(voice) = voice.as_mut() {
            sample = voice.present(sample);
        }
        mix.add_captured(
            destination,
            sample,
            capture
                .as_deref_mut()
                .and_then(|frames| frames.get_mut(output_offset + offset)),
        );
    }
    let incomplete = !valid || available < count;
    // Bounded lookahead (#4075): once the playhead is in the second half of a
    // present page, ask for the next page so ordinary decode latency is
    // hidden. The provider drops it whenever any request is pending.
    let read_to = page_offset + available;
    let next_page = page_index.checked_add(1);
    let prefetch = valid
        && read_to >= STREAM_PAGE_FRAMES / 2
        && next_page.is_some_and(|next| {
            next.checked_mul(STREAM_PAGE_FRAMES as u64)
                .is_some_and(|start| start < source.playback_frames())
        });
    if let Some(mut voice) = voice {
        if valid && available < count {
            // A valid short page is the end of the source: true silence.
            voice.end();
        } else if !valid {
            // Conceal the miss with the held sample under the falling gain.
            for (offset, destination) in destination.iter_mut().enumerate().skip(available) {
                let sample = voice.missing();
                if sample == [0.0; 2] {
                    continue;
                }
                mix.add_captured(
                    destination,
                    sample,
                    capture
                        .as_deref_mut()
                        .and_then(|frames| frames.get_mut(output_offset + offset)),
                );
            }
        }
    }
    if incomplete {
        pages.request_page(source.key(), page_index);
    } else if prefetch && let Some(next) = next_page {
        pages.prefetch_page(source.key(), next);
    }
    count
}

pub(crate) fn render_streamed_page_span_captured(
    mix: PreparedTrackMix,
    source: &StreamedSourceMetadata,
    span: StreamedPageSpan<'_>,
    output: &mut [Frame],
    pages: &mut (impl MediaPages + ?Sized),
    mut capture: Option<&mut [Frame]>,
) -> usize {
    if !span.reversed {
        return render_forward_streamed_page_span_captured(
            mix, source, span, output, pages, capture,
        );
    }
    let StreamedPageSpan {
        source_frame,
        reversed: _,
        timeline_frame,
        output_offset,
        count: requested_count,
        fade,
        gain: clip_gain,
    } = span;
    if requested_count == 0 {
        return 0;
    }
    let page_index = source_frame / STREAM_PAGE_FRAMES as u64;
    let page_offset = (source_frame % STREAM_PAGE_FRAMES as u64) as usize;
    let count = requested_count.min(page_offset + 1);
    let Some(output_end) = output_offset.checked_add(count) else {
        return 0;
    };
    let Some(destination) = output.get_mut(output_offset..output_end) else {
        return 0;
    };
    let (page, fades) = pages.page_and_miss_fades(source.key(), page_index);
    let (frames, valid) = match page {
        Some(page) if page.page_index == page_index => (page.frames, true),
        _ => (&[][..], false),
    };
    let mut voice = fades.map(|fades| fades.directed_voice(source.key(), source_frame, true));
    let mut incomplete = !valid;
    for (offset, destination) in destination.iter_mut().enumerate() {
        let source_offset = page_offset - offset;
        let sample = if let Some(&sample) = frames.get(source_offset).filter(|_| valid) {
            let mut sample = sample;
            if let Some((fade, fade_start)) = fade
                && let Some(frame) = timeline_frame.checked_add(offset as u64)
            {
                let gain = fade.gain_at(frame.saturating_sub(fade_start));
                sample[0] *= gain;
                sample[1] *= gain;
            }
            if clip_gain != 1.0 {
                sample[0] *= clip_gain;
                sample[1] *= clip_gain;
            }
            voice.as_mut().map_or(sample, |voice| voice.present(sample))
        } else if valid {
            // A short valid page denotes source-end silence, not a missing page.
            incomplete = true;
            if let Some(voice) = voice.take() {
                voice.end();
            }
            continue;
        } else {
            let sample = voice.as_mut().map_or([0.0; 2], |voice| voice.missing());
            if sample == [0.0; 2] {
                continue;
            }
            sample
        };
        mix.add_captured(
            destination,
            sample,
            capture
                .as_deref_mut()
                .and_then(|frames| frames.get_mut(output_offset + offset)),
        );
    }
    let prefetch_page = (page_offset + 1 - count <= STREAM_PAGE_FRAMES / 2)
        .then(|| page_index.checked_sub(1))
        .flatten();
    if incomplete {
        pages.request_page(source.key(), page_index);
    } else if valid && let Some(next) = prefetch_page {
        pages.prefetch_page(source.key(), next);
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BorrowedMediaPage, StreamSourceKey};

    struct StubPages {
        page0: Vec<Frame>,
        requested: Vec<(StreamSourceKey, u64)>,
        prefetched: Vec<(StreamSourceKey, u64)>,
    }

    impl MediaPages for StubPages {
        fn page(
            &mut self,
            _key: StreamSourceKey,
            page_index: u64,
        ) -> Option<BorrowedMediaPage<'_>> {
            (page_index == 0 && !self.page0.is_empty()).then_some(BorrowedMediaPage {
                page_index: 0,
                frames: &self.page0,
            })
        }

        fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
            self.requested.push((key, page_index));
        }

        fn prefetch_page(&mut self, key: StreamSourceKey, page_index: u64) {
            self.prefetched.push((key, page_index));
        }
    }

    #[test]
    fn forward_and_missing_streamed_spans_mix_or_request_page() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::Asset(asset) = editor
            .execute(hex_project::Command::AddAsset {
                path: "span.wav".into(),
                sample_rate: 48_000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("asset");
        };
        let key = StreamSourceKey::new(1, asset);
        let meta = StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            4,
            4,
            hex_project::IDENTITY_CONVERSION,
            [0; 32],
        )
        .unwrap();
        let mut pages = StubPages {
            page0: vec![[0.5, -0.5], [1.0, -1.0], [0.25, -0.25], [0.0, 0.0]],
            requested: Vec::new(),
            prefetched: Vec::new(),
        };
        let mut out = [[0.0; 2]; 4];
        let rendered = render_streamed_page_span(
            PreparedTrackMix::identity(),
            &meta,
            StreamedPageSpan {
                source_frame: 0,
                reversed: false,
                timeline_frame: 0,
                output_offset: 0,
                count: 2,
                gain: 0.5,
                fade: None,
            },
            &mut out,
            &mut pages,
        );
        assert_eq!(rendered, 2);
        assert_eq!(out[0], [0.25, -0.25]);
        assert_eq!(out[1], [0.5, -0.5]);
        assert!(pages.requested.is_empty());

        let mut missing_pages = StubPages {
            page0: Vec::new(),
            requested: Vec::new(),
            prefetched: Vec::new(),
        };
        let mut silent = [[0.0; 2]; 2];
        let count = render_streamed_page_span(
            PreparedTrackMix::identity(),
            &meta,
            StreamedPageSpan {
                source_frame: 0,
                reversed: false,
                timeline_frame: 0,
                output_offset: 0,
                count: 2,
                gain: 1.0,
                fade: None,
            },
            &mut silent,
            &mut missing_pages,
        );
        assert_eq!(count, 2);
        assert_eq!(silent, [[0.0; 2]; 2]);
        assert_eq!(missing_pages.requested, vec![(key, 0)]);
    }
}
