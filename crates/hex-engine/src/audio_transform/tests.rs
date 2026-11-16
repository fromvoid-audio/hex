use super::*;
use hex_project::{
    AudioDomainClock, BeatTime, Command, Created, Editor, Project, SourceGainPoint, Tempo,
    WarpMode, default_audio_warp,
};
use std::collections::BTreeMap;

#[path = "tests/group.rs"]
mod group;

#[path = "tests/split_recombination.rs"]
mod split_recombination;

#[path = "tests/aliases.rs"]
mod aliases;
#[path = "tests/content_resampler.rs"]
mod content_resampler;
#[path = "tests/content_reverse.rs"]
mod content_reverse;
#[path = "tests/musical_sharing.rs"]
mod musical_sharing;
#[path = "tests/view_placement.rs"]
mod view_placement;

pub(super) fn fixture(
    looping: bool,
) -> (
    Editor,
    hex_project::ClipId,
    BTreeMap<hex_project::AssetId, Audio>,
) {
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "original.wav".into(),
            sample_rate: 16_000,
            frames: 8_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetClipLooping { clip, looping })
        .unwrap();
    if looping {
        editor
            .execute(Command::SetClipSourcePhase { clip, phase: 137 })
            .unwrap();
    }
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 997,
            fade_out: 1_601,
        })
        .unwrap();
    let audio = Audio::new(
        16_000,
        (0..8_000)
            .map(|i| {
                let x = (i as f32 * 0.173).sin() * 0.3;
                let y = (i as f32 * 0.251).cos() * 0.2;
                [x, y]
            })
            .collect(),
    )
    .unwrap();
    (editor, clip, BTreeMap::from([(asset, audio)]))
}
pub(super) fn prepare(
    editor: &Editor,
    media: &BTreeMap<hex_project::AssetId, Audio>,
    context: &AudioTransformPreparation,
) -> crate::PreparedRevision {
    crate::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        media,
        Arc::new(crate::SineTable::new()),
        None,
        context,
    )
    .unwrap()
}
pub(super) fn render(
    revision: &crate::PreparedRevision,
    start: u64,
    length: usize,
    block: usize,
) -> Vec<crate::Frame> {
    let mut renderer = crate::Renderer::new(revision.arrangement());
    renderer.seek(start);
    renderer.play();
    let mut result = vec![[0.0; 2]; length];
    for chunk in result.chunks_mut(block) {
        renderer.render(chunk).unwrap();
    }
    result
}

#[test]
fn pitched_stream_fade_matches_resident_render_at_start_and_seek() {
    struct Pages {
        key: crate::StreamSourceKey,
        frames: Vec<crate::Frame>,
    }
    impl crate::MediaPages for Pages {
        fn page(
            &mut self,
            key: crate::StreamSourceKey,
            index: u64,
        ) -> Option<crate::BorrowedMediaPage<'_>> {
            let start = usize::try_from(index)
                .ok()?
                .checked_mul(crate::STREAM_PAGE_FRAMES)?;
            if key != self.key || start >= self.frames.len() {
                return None;
            }
            Some(crate::BorrowedMediaPage {
                page_index: index,
                frames: &self.frames
                    [start..(start + crate::STREAM_PAGE_FRAMES).min(self.frames.len())],
            })
        }
        fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {}
        fn prefetch_page(&mut self, _: crate::StreamSourceKey, _: u64) {}
    }
    let (mut editor, id, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch {
            clip: id,
            cents: 700,
        })
        .unwrap();
    let expected = render(
        &prepare(&editor, &media, &AudioTransformPreparation::default()),
        0,
        8000,
        37,
    );
    let (mut neutral, neutral_id, neutral_media) = fixture(false);
    neutral
        .execute(Command::SetClipFades {
            clip: neutral_id,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    neutral
        .execute(Command::SetAudioClipPitch {
            clip: neutral_id,
            cents: 700,
        })
        .unwrap();
    let frames = render(
        &prepare(
            &neutral,
            &neutral_media,
            &AudioTransformPreparation::default(),
        ),
        0,
        8000,
        53,
    );
    let project = editor.project();
    let clip = project.clip(id).unwrap();
    let asset = project.asset(clip.asset()).unwrap();
    let raw = Arc::new(
        crate::StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(1, asset.id()),
            16_000,
            16_000,
            8_000,
            8_000,
            hex_project::IDENTITY_CONVERSION,
            [3; 32],
        )
        .unwrap(),
    );
    let pitched = Arc::new(
        raw.with_plain_pitch(700, hex_project::WarpResampler::V2)
            .unwrap(),
    );
    let derived = BTreeMap::from([(pitched.key(), Arc::clone(&pitched))]);
    let compiled = AudioTransformPreparation::default()
        .compile_clip(
            project,
            clip,
            asset,
            PreparedAudioSourceRef::StreamedWithPitch(&raw, &derived),
        )
        .unwrap()
        .unwrap();
    let track = crate::Track::new(vec![compiled], 1.0, false).unwrap();
    let mut pages = Pages {
        key: pitched.key(),
        frames,
    };
    for start in [0, 400, 7000] {
        let mut actual = [[0.0; 2]; 128];
        track
            .render_additive_with_pages(start, &mut actual, &mut pages, None)
            .unwrap();
        for (a, b) in actual
            .iter()
            .zip(&expected[start as usize..start as usize + 128])
        {
            for channel in 0..2 {
                assert!(
                    (a[channel] - b[channel]).abs() < 1.0e-6,
                    "{start}: {a:?} != {b:?}"
                );
            }
        }
    }
    assert!(expected[400..528].iter().any(|frame| frame[0].abs() > 0.01));
}
fn isolated(limit: usize, cache: usize, entries: usize) -> AudioTransformPreparation {
    AudioTransformPreparation {
        budget: Arc::new(budget::Budget::new(limit)),
        cache_limit: cache,
        entry_limit: entries,
        ..AudioTransformPreparation::default()
    }
}
#[test]
fn reservation_covers_audible_owners_after_cache_eviction_and_releases_on_failure() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let context = isolated(128_000, 64_000, 1);
    let first = prepare(&editor, &media, &context);
    assert_eq!(context.retained_pcm_bytes(), 64_000);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 211 })
        .unwrap();
    let second = prepare(&editor, &media, &context);
    assert_eq!(context.retained_pcm_bytes(), 128_000);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 300 })
        .unwrap();
    assert!(
        crate::PreparedRevision::prepare_with_audio_transforms(
            editor.project(),
            &media,
            Arc::new(crate::SineTable::new()),
            None,
            &context
        )
        .is_err()
    );
    assert_eq!(context.retained_pcm_bytes(), 128_000);
    assert_eq!(context.cached_pcm_bytes().unwrap(), 0);
    drop(first);
    assert_eq!(context.retained_pcm_bytes(), 64_000);
    drop(second);
    assert_eq!(context.retained_pcm_bytes(), 0);
    // Cancellation after reservation must release both output and pending ownership.
    let mut polls = 0;
    let map = hex_warp::TimeMap::new(
        vec![hex_warp::MapSegment {
            output_start: 0,
            output_end: 8_000,
            source_start_q32: 0,
            source_end_q32: 8_000_i128 << 32,
        }],
        8_000,
    )
    .unwrap();
    let reservation = context.budget.reserve(64_000).unwrap();
    let result = hex_warp::render(
        &hex_warp::Request {
            attacks: &[],
            input: hex_warp::Input {
                frames: media.values().next().unwrap().frames(),
                sample_rate: 16_000,
                region_start: 0,
                region_length: 8_000,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map: &map,
            output_frames: 8_000,
            pitch_cents: 137,
            mode: hex_warp::Mode::Resample,
        },
        &mut || {
            polls += 1;
            polls > 2
        },
    );
    assert!(matches!(result, Err(hex_warp::Error::Cancelled)));
    drop(reservation);
    assert_eq!(context.retained_pcm_bytes(), 0);
}
#[test]
fn failed_preparation_transaction_does_not_cache_candidate_and_gain_reuses_pcm() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let context = isolated(256_000, 256_000, 2);
    let asset = editor
        .project()
        .asset(editor.project().clip(clip).unwrap().asset())
        .unwrap();
    let source = &media[&asset.id()];
    let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
    let failure: Result<(), Invalid> = context.transaction(|| {
        let _candidate = context.prepare_audio(editor.project(), edit, asset, source)?;
        Err(Invalid("later inventory failure"))
    });
    assert!(failure.is_err());
    assert_eq!(context.retained_pcm_bytes(), 0);
    assert_eq!(context.cached_pcm_bytes().unwrap(), 0);
    let first = prepare(&editor, &media, &context);
    let before = context.cache_hits();
    editor
        .execute(Command::SetAudioClipGain {
            clip,
            millidb: -6_000,
        })
        .unwrap();
    let second = prepare(&editor, &media, &context);
    assert!(context.cache_hits() > before);
    assert_eq!(context.retained_pcm_bytes(), 64_000);
    let a = render(&first, 0, 8_000, 97);
    let b = render(&second, 0, 8_000, 53);
    let gain = hex_project::ClipGain::new(-6_000).unwrap().linear();
    for (a, b) in a.iter().zip(b) {
        assert_eq!(b, a.map(|x| x * gain));
    }
    context.shutdown.store(true, Ordering::Release);
    assert!(
        crate::PreparedRevision::prepare_with_audio_transforms(
            editor.project(),
            &media,
            Arc::new(crate::SineTable::new()),
            None,
            &context
        )
        .is_err()
    );
}

#[test]
fn accepted_audio_sidecar_survives_tiny_cache_and_preserves_split_crop_identity() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let Created::Clip(other) = editor.execute(Command::DuplicateClip(clip)).unwrap() else {
        panic!("duplicate")
    };
    editor
        .execute(Command::SetAudioClipPitch {
            clip: other,
            cents: 211,
        })
        .unwrap();
    let context = isolated(512_000, 64_000, 1);
    let prepared = prepare(&editor, &media, &context);
    let views = prepared.arrangement().audio_clip_views();
    assert_eq!(views.len(), 2);
    assert!(!views[0].audio.shares_pcm_with(&views[1].audio));
    assert_eq!(context.cached_pcm_bytes().unwrap(), 64_000);
    editor
        .execute(Command::SplitClip { clip, frame: 3_137 })
        .unwrap();
    let split = prepare(&editor, &media, &context);
    let views = split.arrangement().audio_clip_views();
    assert_eq!(views.len(), 3);
    assert!(views[0].audio.shares_pcm_with(&views[1].audio));
    assert_eq!(views[1].offset, 3_137);
    assert_eq!(views[1].length, 4_863);
}

/// The content's saved warp resampler selects the root render law, and one
/// shared cache never serves PCM rendered under the other law.
#[test]
fn content_warp_resampler_selects_the_render_law_and_its_cache_entry() {
    fn with_law(editor: &Editor, law: hex_project::WarpResampler) -> Editor {
        let mut snapshot = editor.project().snapshot();
        let hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } =
            &mut snapshot.tracks[0].content
        else {
            panic!("hybrid track")
        };
        Arc::make_mut(&mut audio_clips[0].edit.as_mut().unwrap().domain).warp_resampler = law;
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap()
    }
    let (mut editor, clip, media) = fixture(false);
    // Unpitched content reads integer positions: both laws are exact.
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![SourceGainPoint {
                frame: 0,
                gain_q16: 40_000,
            }],
        })
        .unwrap();
    let context = isolated(1_024_000, 512_000, 8);
    let unpitched = |law| {
        render(
            &prepare(&with_law(&editor, law), &media, &context),
            0,
            4_000,
            64,
        )
    };
    assert_eq!(
        unpitched(hex_project::WarpResampler::V1),
        unpitched(hex_project::WarpResampler::V2)
    );
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 700 })
        .unwrap();
    let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
    assert_eq!(edit.domain.warp_resampler, hex_project::WarpResampler::V2);
    let pitched = |law, context: &AudioTransformPreparation| {
        render(
            &prepare(&with_law(&editor, law), &media, context),
            0,
            4_000,
            64,
        )
    };
    let v2 = pitched(hex_project::WarpResampler::V2, &context);
    let v1 = pitched(hex_project::WarpResampler::V1, &context);
    assert_ne!(v1, v2);
    let fresh = isolated(1_024_000, 512_000, 8);
    assert_eq!(pitched(hex_project::WarpResampler::V1, &fresh), v1);
    assert_eq!(pitched(hex_project::WarpResampler::V2, &fresh), v2);
}

#[test]
fn injected_mid_render_cancellation_releases_candidate_reservation() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let context = isolated(256_000, 128_000, 2);
    context.cancel_after_polls.set(Some(3));
    let result = crate::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        &media,
        Arc::new(crate::SineTable::new()),
        None,
        &context,
    );
    assert!(matches!(
        result,
        Err(Invalid("audio preparation cancelled"))
    ));
    assert_eq!(context.retained_pcm_bytes(), 0);
    assert_eq!(context.cached_pcm_bytes().unwrap(), 0);
    context.cancel_after_polls.set(None);
    let accepted = prepare(&editor, &media, &context);
    assert!(render(&accepted, 0, 128, 31).iter().any(|f| f[0] != 0.0));
}

#[test]
fn cropped_tail_moved_to_beat_zero_keeps_source_phase_and_bounded_virtual_prefix() {
    let (mut editor, clip, media) = fixture(true);
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let context = isolated(512_000, 256_000, 8);
    let original = prepare(&editor, &media, &context);
    let reference = render(&original, 3_137, 400, 31);
    let Created::Clip(tail) = editor
        .execute(Command::SplitClip { clip, frame: 3_137 })
        .unwrap()
    else {
        panic!("split")
    };
    editor.execute(Command::RemoveClip(clip)).unwrap();
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip: tail,
            track,
            start: 0,
        })
        .unwrap();
    let moved = prepare(&editor, &media, &context);
    // The re-anchored map may round source positions differently in Q32 units.
    // v1 snapped them to the same phase row; the v2 warp resampler interpolates
    // phases continuously, so such sub-picosample shifts stay below -140 dB.
    let moved = render(&moved, 0, 400, 127);
    assert_eq!(moved.len(), reference.len());
    for (a, b) in moved.iter().zip(&reference) {
        assert!((a[0] - b[0]).abs() <= 1e-7 && (a[1] - b[1]).abs() <= 1e-7);
    }
    assert!(
        editor
            .project()
            .clip(tail)
            .unwrap()
            .audio_edit()
            .unwrap()
            .projection_origin()
            .unwrap()
            < 0
    );
}

#[test]
fn neutral_faded_streamed_commands_move_and_trim_match_resident_and_transform_failure_rolls_back() {
    let (mut editor, clip, media) = fixture(true);
    for command in [
        Command::SetAudioClipPitch { clip, cents: 0 },
        Command::SetAudioClipWarp {
            clip,
            settings: None,
        },
        Command::SetAudioClipMusical {
            clip,
            enabled: false,
        },
    ] {
        editor.execute(command).unwrap();
    }
    assert!(editor.project().clip(clip).unwrap().audio_edit().is_none());
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 300,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 400,
        })
        .unwrap();
    let asset = editor.project().clip(clip).unwrap().asset();
    let key = crate::StreamSourceKey::new(1, asset);
    let metadata = Arc::new(
        crate::StreamedSourceMetadata::new(
            key,
            16_000,
            16_000,
            8_000,
            8_000,
            hex_project::IDENTITY_CONVERSION,
            [7; 32],
        )
        .unwrap(),
    );
    let sources = BTreeMap::from([(
        asset,
        crate::compile::PreparedAudioSource::Streamed(metadata),
    )]);
    let streamed =
        crate::compile::project_with_prepared_audio_sources(editor.project(), &sources, 1).unwrap();
    let resident = crate::compile::project(editor.project(), &media).unwrap();
    struct Pages<'a> {
        key: crate::StreamSourceKey,
        frames: &'a [crate::Frame],
    }
    impl crate::MediaPages for Pages<'_> {
        fn page(
            &mut self,
            key: crate::StreamSourceKey,
            index: u64,
        ) -> Option<crate::BorrowedMediaPage<'_>> {
            if key != self.key {
                return None;
            }
            let start = index as usize * crate::STREAM_PAGE_FRAMES;
            let end = (start + crate::STREAM_PAGE_FRAMES).min(self.frames.len());
            Some(crate::BorrowedMediaPage {
                page_index: index,
                frames: self.frames.get(start..end)?,
            })
        }
        fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {
            panic!("resident fixture has every requested page")
        }
    }
    let mut pages = Pages {
        key,
        frames: media[&asset].frames(),
    };
    let mut a = crate::Renderer::new(&streamed);
    let mut b = crate::Renderer::new(&resident);
    a.play();
    b.play();
    let mut actual = vec![[0.0; 2]; 8_400];
    let mut expected = actual.clone();
    for (chunk_a, chunk_b) in actual.chunks_mut(127).zip(expected.chunks_mut(127)) {
        a.render_with_pages(chunk_a, &mut pages).unwrap();
        b.render(chunk_b).unwrap();
    }
    assert_eq!(actual, expected);
    let before = editor.project().clone();
    let failure = editor.execute_prepared(
        Command::SetAudioClipPitch { clip, cents: 137 },
        |candidate| crate::compile::project_with_prepared_audio_sources(candidate, &sources, 1),
    );
    assert!(failure.is_err());
    assert_eq!(editor.project(), &before);
    let failure = editor.execute_prepared(
        Command::EditAudioEvents {
            clip,
            operation: hex_project::AudioEventEdit::Split { at: 2000 },
        },
        |candidate| crate::compile::project_with_prepared_audio_sources(candidate, &sources, 1),
    );
    assert!(failure.is_err());
    assert_eq!(editor.project(), &before);
}
#[test]
fn resident_beat_anchor_without_warp_has_exact_audible_waveform_root() {
    let (mut editor, clip, media) = fixture(true);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let direct = crate::compile::project(editor.project(), &media).unwrap();
    let mut renderer = crate::Renderer::new(&direct);
    renderer.play();
    let mut expected = vec![[0.0; 2]; 8_000];
    renderer.render(&mut expected).unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let context = isolated(128_000, 128_000, 2);
    let prepared = prepare(&editor, &media, &context);
    assert_eq!(prepared.arrangement().audio_clip_views().len(), 1);
    assert_eq!(render(&prepared, 0, 8_000, 127), expected);
}

#[test]
fn tiny_derived_roots_cannot_accumulate_large_original_source_cache_owners() {
    let (mut editor, clip, original) = fixture(false);
    drop(original);
    editor
        .execute(Command::TrimClip {
            clip,
            start: 0,
            duration: 64,
            source_offset: 0,
            source_length: 64,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let asset = editor.project().clip(clip).unwrap().asset();
    let context = AudioTransformPreparation {
        source_limit: 64_000,
        ..isolated(128_000, 16_384, 16)
    };
    let mut old_sources: Vec<std::sync::Weak<crate::AudioData>> = Vec::new();
    let mut audible = Vec::new();
    for generation in 0..4 {
        let audio = Audio::new(16_000, vec![[0.1 + generation as f32 * 0.1; 2]; 8_000]).unwrap();
        let weak = Arc::downgrade(&audio.frames);
        let media = BTreeMap::from([(asset, audio)]);
        audible.push(prepare(&editor, &media, &context));
        drop(media);
        assert!(old_sources.iter().all(|source| source.strong_count() == 0));
        assert_eq!(
            weak.strong_count(),
            1,
            "only bounded cache retains the full original"
        );
        old_sources.push(weak);
        let cache = context.cache.borrow();
        assert_eq!(source_charge(cache.entries.iter(), None).unwrap(), 64_000);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, 512);
    }
    // Pending entries share the source cap with committed cache, even before publication.
    context
        .transaction(|| {
            for generation in 0..4 {
                let source =
                    Audio::new(16_000, vec![[0.5 + generation as f32 * 0.05; 2]; 8_000]).unwrap();
                let weak = Arc::downgrade(&source.frames);
                let canonical = editor.project().clip(clip).unwrap();
                let asset = editor.project().asset(asset).unwrap();
                let _ = context.prepare_audio(
                    editor.project(),
                    canonical.audio_edit().unwrap(),
                    asset,
                    &source,
                )?;
                drop(source);
                assert!(old_sources.iter().all(|source| source.strong_count() == 0));
                old_sources.push(weak);
                let pending = context.pending.borrow();
                let entries = pending.as_ref().unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(source_charge(entries.iter(), None).unwrap(), 64_000);
                assert_eq!(
                    source_charge(context.cache.borrow().entries.iter(), None).unwrap(),
                    0
                );
            }
            Ok(())
        })
        .unwrap();
    drop(context);
    assert!(old_sources.iter().all(|source| source.strong_count() == 0));
    assert!(
        audible
            .iter()
            .all(|revision| !render(revision, 0, 64, 31).is_empty())
    );
}

#[test]
fn section_splices_are_exact_pcm_insertions_removals_and_copies_at_unequal_blocks() {
    const CUT: usize = 16_000;
    const SECTION_END: usize = 32_000;
    const SOURCE_END: usize = 48_000;
    const INSERTED: usize = SECTION_END - CUT;

    for processing in 0..3 {
        let (mut seed, clip, media) = fixture(true);
        seed.execute(Command::TrimClip {
            clip,
            start: 0,
            duration: SOURCE_END as u64,
            source_offset: 0,
            source_length: 8_000,
        })
        .unwrap();
        let extended = seed.project().clip(clip).unwrap();
        assert_eq!(extended.duration(), SOURCE_END as u64);
        assert_eq!(extended.source_phase(), 137);
        assert_eq!(extended.fade().fade_in(), 997);
        assert_eq!(extended.fade().fade_out(), 1_601);
        assert_eq!(
            Project::from_snapshot(seed.project().snapshot()).unwrap(),
            seed.project().clone()
        );

        if processing > 0 {
            seed.execute(Command::SetAudioClipPitch { clip, cents: 137 })
                .unwrap();
            seed.execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        }
        if processing == 2 {
            let settings = default_audio_warp(
                seed.project().clip(clip).unwrap(),
                seed.project().tempo_map(),
                WarpMode::Beats {
                    preserve: hex_project::BeatsPreserve::Transients,
                    tail: hex_project::BeatsTail::Forward,
                    decay: 100,
                },
            )
            .unwrap();
            seed.execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        }
        let context = isolated(8 * 1024 * 1024, 4 * 1024 * 1024, 8);
        let baseline = prepare(&seed, &media, &context);
        let source_pcm = render(&baseline, 0, SOURCE_END, 113);
        assert!(
            source_pcm[CUT..SECTION_END]
                .iter()
                .any(|frame| *frame != [0.0; 2]),
            "the copied/cut interval must contain audible source PCM"
        );
        for name in ["grow", "shrink", "delete", "duplicate"] {
            let mut editor = Editor::new(seed.project().clone(), 32).unwrap();
            let Created::ArrangementSection(section) = editor
                .execute(Command::AddArrangementSection {
                    name: name.into(),
                    color: hex_project::ElementColor::Cyan,
                    start: BeatTime::from_quarters(2).unwrap(),
                    end: BeatTime::from_quarters(if name == "shrink" { 6 } else { 4 }).unwrap(),
                })
                .unwrap()
            else {
                panic!("section")
            };
            let (command, expected) = match name {
                "grow" => {
                    let mut pcm = source_pcm[..SECTION_END].to_vec();
                    pcm.extend(vec![[0.0; 2]; INSERTED]);
                    pcm.extend_from_slice(&source_pcm[SECTION_END..]);
                    (
                        Command::ResizeArrangementSection {
                            section,
                            edge: hex_project::TrimEdge::Right,
                            length: BeatTime::from_quarters(4).unwrap(),
                        },
                        pcm,
                    )
                }
                "shrink" => (
                    Command::ResizeArrangementSection {
                        section,
                        edge: hex_project::TrimEdge::Right,
                        length: BeatTime::from_quarters(2).unwrap(),
                    },
                    source_pcm[..SECTION_END].to_vec(),
                ),
                "delete" => {
                    let mut pcm = source_pcm[..CUT].to_vec();
                    pcm.extend_from_slice(&source_pcm[SECTION_END..]);
                    (Command::DeleteArrangementSection(section), pcm)
                }
                _ => {
                    let mut pcm = source_pcm[..SECTION_END].to_vec();
                    pcm.extend_from_slice(&source_pcm[CUT..SECTION_END]);
                    pcm.extend_from_slice(&source_pcm[SECTION_END..]);
                    (Command::DuplicateArrangementSection(section), pcm)
                }
            };
            editor.execute(command).unwrap();
            let prepared = prepare(&editor, &media, &context);
            assert_eq!(
                render(&prepared, 0, expected.len(), 127),
                expected,
                "{name} with 127-frame blocks"
            );
            assert_eq!(
                render(&prepared, 0, expected.len(), 257),
                expected,
                "{name} with 257-frame blocks"
            );
        }
    }
}

#[test]
fn pending_and_committed_derived_cache_share_one_ceiling_without_freeing_audible_pcm() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let context = isolated(512_000, 128_000, 8);
    let audible = prepare(&editor, &media, &context);
    let expected = render(&audible, 0, 128, 31);
    context
        .transaction(|| {
            for cents in [211, 300] {
                let mut candidate = Editor::new(editor.project().clone(), 8).unwrap();
                candidate
                    .execute(Command::SetAudioClipPitch { clip, cents })
                    .unwrap();
                let clip = candidate.project().clip(clip).unwrap();
                let asset = candidate.project().asset(clip.asset()).unwrap();
                let _ = context.prepare_audio(
                    candidate.project(),
                    clip.audio_edit().unwrap(),
                    asset,
                    &media[&asset.id()],
                )?;
                let cache = context.cache.borrow();
                let pending = context.pending.borrow();
                let pending_bytes = pending
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|entry| entry.bytes)
                    .sum::<usize>();
                assert!(cache.bytes + pending_bytes <= 128_000);
            }
            assert_eq!(context.cache.borrow().bytes, 0);
            assert_eq!(context.retained_pcm_bytes(), 192_000);
            Ok(())
        })
        .unwrap();
    assert_eq!(context.cached_pcm_bytes().unwrap(), 128_000);
    assert_eq!(render(&audible, 0, 128, 53), expected);
    drop(audible);
    assert_eq!(context.retained_pcm_bytes(), 128_000);
}

#[test]
fn global_reservation_reclaims_unused_cache_before_refusing_a_fitting_candidate() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let context = isolated(96_000, 256_000, 8);
    let old = prepare(&editor, &media, &context);
    drop(old);
    assert_eq!(context.retained_pcm_bytes(), 64_000);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 211 })
        .unwrap();
    let accepted = prepare(&editor, &media, &context);
    assert_eq!(context.retained_pcm_bytes(), 64_000);
    assert_eq!(context.cached_pcm_bytes().unwrap(), 64_000);
    assert!(
        render(&accepted, 0, 64, 31)
            .iter()
            .any(|frame| frame[0] != 0.0)
    );
}

#[test]
fn projected_fade_samples_match_prepared_pcm_after_crop_and_tempo_change() {
    let (mut editor, clip, mut media) = fixture(true);
    let asset = editor.project().clip(clip).unwrap().asset();
    media.insert(asset, Audio::new(16_000, vec![[1.0, -1.0]; 8_000]).unwrap());
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let Created::Clip(tail) = editor
        .execute(Command::SplitClip { clip, frame: 3_137 })
        .unwrap()
    else {
        panic!("split")
    };
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let context = isolated(256_000, 128_000, 4);
    let prepared = prepare(&editor, &media, &context);
    let clip = editor.project().clip(tail).unwrap();
    let edit = clip.audio_edit().unwrap();
    let projection = edit.projected_fade(editor.project().tempo_map()).unwrap();
    let view = prepared
        .arrangement()
        .audio_clip_views()
        .iter()
        .find(|v| v.clip == tail)
        .unwrap();
    for index in (0..view.length).step_by(31) {
        let actual = view.audio.frames()[(view.offset + index) as usize];
        let expected = projection.gain_at(view.offset + index).unwrap();
        assert_eq!(actual, [expected, -expected]);
    }
    assert_eq!(
        projection.gain_at(8_267).unwrap(),
        edit.domain.fade.gain_at(7_200)
    );
}

#[test]
fn derived_and_raw_audio_never_gain_private_constructor_provenance() {
    let budget = Arc::new(budget::Budget::new(1024));
    let reservation = budget.reserve(64).unwrap();
    let mut audio = Audio::from_derived(48_000, vec![[0.0; 2]; 8], reservation).unwrap();
    // Even a deliberately forged test receipt cannot relabel a derived owner.
    audio.pcm_arc_receipt = crate::pcm_arc::PcmArcReceipt::current(
        audio.frames.values.len(),
        audio.frames.values.capacity(),
    );
    let source = crate::OwnedAudioSource {
        kind: crate::audio_only_inputs::SourceKind::Resident(audio),
        handle: crate::audio_only_inputs::handle().unwrap(),
    };
    assert_eq!(
        crate::audio_input_storage::parts(&source),
        Err(crate::AudioStorageError::UnknownStorage)
    );
    assert_eq!(budget.used(), 64);
    drop(source);
    assert_eq!(budget.used(), 0);
}

#[test]
fn event_gain_scales_only_its_audio_and_undo_restores_render() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: hex_project::AudioEventEdit::Split { at: 4000 },
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 257);
    let gain = hex_project::ClipGain::new(-6000).unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: hex_project::AudioEventEdit::SetGain { event: 1, gain },
        })
        .unwrap();
    let scaled = render(&prepare(&editor, &media, &context), 0, 8000, 127);
    for (actual, original) in scaled[..4000].iter().zip(&baseline[..4000]) {
        for channel in 0..2 {
            assert!((actual[channel] - original[channel] * gain.linear()).abs() < 1e-6);
        }
    }
    assert_eq!(&scaled[4000..], &baseline[4000..]);
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8000, 64),
        baseline
    );
    assert!(editor.redo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8000, 17),
        scaled
    );
}

#[test]
fn event_gain_preserves_fades_and_inactive_state_in_frame_and_musical_clocks() {
    for musical in [false, true] {
        let (mut editor, clip, media) = fixture(false);
        if musical {
            editor
                .execute(Command::SetAudioClipMusical {
                    clip,
                    enabled: true,
                })
                .unwrap();
        }
        let context = AudioTransformPreparation::default();
        let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 257);
        let gain = hex_project::ClipGain::new(-6000).unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: hex_project::AudioEventEdit::SetGain { event: 1, gain },
            })
            .unwrap();
        let gained = render(&prepare(&editor, &media, &context), 0, 8000, 127);
        for (actual, original) in gained.iter().zip(&baseline) {
            for channel in 0..2 {
                assert!((actual[channel] - original[channel] * gain.linear()).abs() < 1e-6);
            }
        }
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: hex_project::AudioEventEdit::SetActive {
                    events: vec![1],
                    active: false,
                },
            })
            .unwrap();
        assert!(
            render(&prepare(&editor, &media, &context), 0, 8000, 64)
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert!(editor.undo());
        assert_eq!(
            render(&prepare(&editor, &media, &context), 0, 8000, 17),
            gained
        );
    }
}

#[test]
fn event_gain_survives_reverse_split_crop_and_move() {
    let (mut gained, clip, media) = fixture(false);
    let mut baseline = Editor::new(gained.project().clone(), 32).unwrap();
    let gain = hex_project::ClipGain::new(-6000).unwrap();
    gained
        .execute(Command::EditAudioEvents {
            clip,
            operation: hex_project::AudioEventEdit::SetGain { event: 1, gain },
        })
        .unwrap();
    for editor in [&mut baseline, &mut gained] {
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let Created::Clip(second) = editor
            .execute(Command::SplitClip { clip, frame: 4000 })
            .unwrap()
        else {
            panic!("split")
        };
        let track = editor.project().tracks()[0].id();
        editor
            .execute(Command::MoveClip {
                clip: second,
                track,
                start: 9000,
            })
            .unwrap();
    }
    let context = AudioTransformPreparation::default();
    let expected = render(&prepare(&baseline, &media, &context), 0, 13000, 257);
    let actual = render(&prepare(&gained, &media, &context), 0, 13000, 127);
    for (actual, original) in actual.iter().zip(&expected) {
        for channel in 0..2 {
            assert!((actual[channel] - original[channel] * gain.linear()).abs() < 1e-6);
        }
    }
}

#[test]
fn nested_audio_events_are_exact_through_splits_overwrite_silence_seek_and_blocks() {
    use hex_project::AudioEventEdit;
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 257);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 2000 },
        })
        .unwrap();
    let split = prepare(&editor, &media, &context);
    assert_eq!(render(&split, 0, 8000, 1), baseline);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: 4000,
            },
        })
        .unwrap();
    let revision = prepare(&editor, &media, &context);
    let actual = render(&revision, 0, 8000, 257);
    let mut expected = vec![[0.0; 2]; 8000];
    expected[2000..4000].copy_from_slice(&baseline[2000..4000]);
    expected[4000..6000].copy_from_slice(&baseline[0..2000]);
    expected[6000..8000].copy_from_slice(&baseline[6000..8000]);
    assert_eq!(actual, expected);
    assert_eq!(render(&revision, 3993, 2100, 17), expected[3993..6093]);
    editor
        .execute(Command::SplitClip { clip, frame: 5100 })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8000, 127),
        expected
    );
}

#[test]
fn partial_range_clipboard_keeps_gaps_source_offsets_and_one_history_edge() {
    use hex_project::AudioEventEdit;
    for mode in [
        None,
        Some(WarpMode::RePitch),
        Some(WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Division(BeatTime::from_raw(
                BeatTime::ONE.raw() / 4,
            )),
            tail: BeatsTail::Forward,
            decay: 75,
        }),
        Some(WarpMode::Tones { grain_ms: 32 }),
        Some(WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 50,
            seed: 17,
        }),
        Some(WarpMode::Complex),
        Some(WarpMode::ComplexPro {
            formants: 100,
            envelope: 128,
        }),
    ] {
        let (mut editor, clip, media) = fixture(true);
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        if let Some(mode) = mode {
            editor
                .execute(Command::SetAudioClipMusical {
                    clip,
                    enabled: true,
                })
                .unwrap();
            let settings = default_audio_warp(
                editor.project().clip(clip).unwrap(),
                editor.project().tempo_map(),
                mode,
            )
            .unwrap();
            editor
                .execute(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                })
                .unwrap();
        }
        let context = AudioTransformPreparation::default();
        let original = editor.project().clone();
        let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 257);
        let root = editor.project().clip(clip).unwrap().audio_event_root();
        let eighth = root.domain.clock.length() / 8;
        let at = |position| root.root_frame_at(position, original.tempo_map()).unwrap() as usize;
        let (a, b, c, d, e) = (
            at(eighth),
            at(2 * eighth),
            at(3 * eighth),
            at(4 * eighth),
            at(5 * eighth),
        );
        assert_eq!((a, b, c, d, e), (1000, 2000, 3000, 4000, 5000));
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Delete {
                    start: 2 * eighth,
                    end: 4 * eighth,
                },
            })
            .unwrap();
        let cut = editor.project().clone();
        let copied = cut
            .clip(clip)
            .unwrap()
            .editable_audio_events()
            .unwrap()
            .copy_range(eighth, 3 * eighth);
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Paste {
                    at: 3 * eighth,
                    length: 2 * eighth,
                    events: copied,
                },
            })
            .unwrap();
        let pasted = editor.project().clone();
        let mut expected = vec![[0.0; 2]; 8000];
        expected[..b].copy_from_slice(&baseline[..b]);
        expected[c..d].copy_from_slice(&baseline[a..b]);
        expected[e..].copy_from_slice(&baseline[e..]);
        let prepared = prepare(&editor, &media, &context);
        assert_eq!(render(&prepared, 0, 8000, 127), expected, "{mode:?}");
        assert_eq!(
            render(&prepared, 2993, 2014, 17),
            expected[2993..5007],
            "seek and partial blocks"
        );
        assert!(editor.undo());
        assert_eq!(*editor.project(), cut);
        assert!(editor.redo());
        assert_eq!(*editor.project(), pasted);
    }
}

#[test]
fn musical_event_split_preserves_every_sample_in_all_existing_warp_modes() {
    for mode in [
        None,
        Some(WarpMode::RePitch),
        Some(WarpMode::Tones { grain_ms: 32 }),
        Some(WarpMode::Complex),
    ] {
        let (mut editor, clip, media) = fixture(true);
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        if let Some(mode) = mode {
            let settings = default_audio_warp(
                editor.project().clip(clip).unwrap(),
                editor.project().tempo_map(),
                mode,
            )
            .unwrap();
            editor
                .execute(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                })
                .unwrap();
        }
        let context = AudioTransformPreparation::default();
        let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 173);
        let root = editor.project().clip(clip).unwrap().audio_event_root();
        let at = (root.view_start + root.view_end) / 2;
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: hex_project::AudioEventEdit::Split { at },
            })
            .unwrap();
        assert_eq!(
            render(&prepare(&editor, &media, &context), 0, 8000, 257),
            baseline
        );
    }
}

#[test]
fn editing_the_visible_event_after_an_outer_cut_preserves_its_sibling_and_hidden_source() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 257);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: hex_project::AudioEventEdit::Split { at: 2000 },
        })
        .unwrap();
    let Created::Clip(right) = editor
        .execute(Command::SplitClip { clip, frame: 4000 })
        .unwrap()
    else {
        panic!("split")
    };
    let events = editor
        .project()
        .clip(right)
        .unwrap()
        .editable_audio_events()
        .unwrap();
    let visible = events.events.iter().find(|e| e.start == 4000).unwrap();
    assert_eq!(visible.end, 8000);
    editor
        .execute(Command::EditAudioEvents {
            clip: right,
            operation: hex_project::AudioEventEdit::Trim {
                event: visible.id,
                start: 4500,
                end: 7500,
            },
        })
        .unwrap();
    let mut expected = baseline.clone();
    expected[4000..4500].fill([0.0; 2]);
    expected[7500..8000].fill([0.0; 2]);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8000, 193),
        expected
    );
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_events()
            .events
            .len(),
        2
    );
}

#[test]
fn event_scratch_reclaims_unused_committed_and_pending_cache() {
    for pending in [false, true] {
        let (mut editor, clip, media) = fixture(false);
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 137 })
            .unwrap();
        // Each root is 64000 bytes: old cached + output fits, scratch needs eviction.
        let context = isolated(128_000, 256_000, 8);
        let check = || {
            drop(prepare(&editor, &media, &context));
            let mut next = Editor::new(editor.project().clone(), 8).unwrap();
            next.execute(Command::EditAudioEvents {
                clip,
                operation: hex_project::AudioEventEdit::Delete {
                    start: 2000,
                    end: 4000,
                },
            })
            .unwrap();
            let accepted = crate::PreparedRevision::prepare_with_audio_transforms(
                next.project(),
                &media,
                Arc::new(crate::SineTable::new()),
                None,
                &context,
            )
            .expect("scratch can reclaim an inaudible old transform");
            assert_eq!(context.retained_pcm_bytes(), 64_000);
            assert!(
                render(&accepted, 2000, 2000, 127)
                    .iter()
                    .all(|x| *x == [0.0, 0.0])
            );
            Ok(())
        };
        if pending {
            context.transaction(check).unwrap();
        } else {
            check().unwrap();
        }
    }
}

#[test]
fn shaped_outer_fades_follow_moved_event_output_coordinates_and_recursive_splits() {
    use hex_project::{AudioEventEdit, ClipFade};
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 257);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 2000 },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: 4000,
            },
        })
        .unwrap();
    let fade = ClipFade::new(8000, 0, 2300, 2900)
        .unwrap()
        .with_shape(100, 200, 600, -500)
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    let mut expected = vec![[0.0; 2]; 8000];
    expected[2000..4000].copy_from_slice(&baseline[2000..4000]);
    expected[4000..6000].copy_from_slice(&baseline[0..2000]);
    expected[6000..8000].copy_from_slice(&baseline[6000..8000]);
    for (index, frame) in expected.iter_mut().enumerate() {
        let gain = fade.gain_at(index as u64);
        frame[0] *= gain;
        frame[1] *= gain;
    }
    let prepared = prepare(&editor, &media, &context);
    assert_eq!(render(&prepared, 0, 8000, 257), expected);
    assert_eq!(render(&prepared, 3993, 2100, 17), expected[3993..6093]);
    let Created::Clip(right) = editor
        .execute(Command::SplitClip { clip, frame: 5100 })
        .unwrap()
    else {
        panic!("split")
    };
    editor
        .execute(Command::SplitClip {
            clip: right,
            frame: 6500,
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8000, 127),
        expected
    );
    assert!(editor.undo());
    assert!(editor.undo());
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8000, 1),
        expected
    );
}

#[test]
fn endpoint_only_fades_prepare_and_play_in_output_coordinates() {
    for (start, end) in [(100, 0), (0, 200), (100, 200)] {
        let (mut editor, clip, media) = fixture(false);
        let asset = editor.project().clip(clip).unwrap().asset();
        let fade = hex_project::ClipFade::new(8000, 0, 0, 0)
            .unwrap()
            .with_shape(start, end, 500, -500)
            .unwrap();
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        assert!(editor.project().clip(clip).unwrap().audio_edit().is_none());
        let mut expected = media[&asset].frames().to_vec();
        expected[..start as usize].fill([0.0; 2]);
        expected[8000 - end as usize..].fill([0.0; 2]);
        let prepared = prepare(&editor, &media, &AudioTransformPreparation::default());
        assert_eq!(render(&prepared, 0, 8000, 127), expected);
        assert_eq!(render(&prepared, 91, 500, 1), expected[91..591]);
    }
}

#[test]
fn endpoint_only_fades_on_edited_streams_refuse_before_commit_and_preserve_history() {
    for (start, end) in [(100, 0), (0, 200), (100, 200)] {
        let (mut editor, clip, _) = fixture(false);
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 1 })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 0 })
            .unwrap();
        assert!(editor.project().clip(clip).unwrap().audio_edit().is_some());
        let asset = editor.project().clip(clip).unwrap().asset();
        let metadata = Arc::new(
            crate::StreamedSourceMetadata::new(
                crate::StreamSourceKey::new(1, asset),
                16_000,
                16_000,
                8000,
                8000,
                hex_project::IDENTITY_CONVERSION,
                [7; 32],
            )
            .unwrap(),
        );
        let sources = BTreeMap::from([(
            asset,
            crate::compile::PreparedAudioSource::Streamed(metadata),
        )]);
        crate::compile::project_with_prepared_audio_sources(editor.project(), &sources, 1).unwrap();
        let before = editor.project().clone();
        // Leave a real redo branch; refusal must preserve both history stacks.
        editor
            .execute(Command::MoveClip {
                clip,
                track: before.tracks()[0].id(),
                start: 300,
            })
            .unwrap();
        let moved = editor.project().clone();
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        let fade = hex_project::ClipFade::new(8000, 0, 0, 0)
            .unwrap()
            .with_shape(start, end, 600, -500)
            .unwrap();
        let failure = editor
            .execute_prepared(Command::SetClipFadeEnvelope { clip, fade }, |candidate| {
                crate::compile::project_with_prepared_audio_sources(candidate, &sources, 1)
            });
        let Err(error) = failure else {
            panic!("edited streamed fade must refuse before commit")
        };
        assert!(error.0.contains("resident audio"), "{error}");
        assert!(error.0.contains("fade"), "{error}");
        assert_eq!(editor.project(), &before);
        assert!(editor.redo());
        assert_eq!(editor.project(), &moved);
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        assert!(editor.undo());
        assert_eq!(
            editor
                .project()
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .pitch_cents,
            1
        );
    }
}

#[test]
fn endpoint_only_fades_on_unedited_streams_preserve_silence_splits_and_seeks() {
    struct Pages<'a> {
        key: crate::StreamSourceKey,
        frames: &'a [crate::Frame],
    }
    impl crate::MediaPages for Pages<'_> {
        fn page(
            &mut self,
            key: crate::StreamSourceKey,
            index: u64,
        ) -> Option<crate::BorrowedMediaPage<'_>> {
            if key != self.key {
                return None;
            }
            let start = usize::try_from(index)
                .ok()?
                .checked_mul(crate::STREAM_PAGE_FRAMES)?;
            let end = start
                .checked_add(crate::STREAM_PAGE_FRAMES)?
                .min(self.frames.len());
            Some(crate::BorrowedMediaPage {
                page_index: index,
                frames: self.frames.get(start..end)?,
            })
        }
        fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {
            panic!("every fixture page is resident")
        }
    }
    for (start, end) in [(1000, 0), (0, 1200), (1000, 1200)] {
        let (mut editor, clip, mut media) = fixture(false);
        let asset = editor.project().clip(clip).unwrap().asset();
        media.insert(asset, Audio::new(16_000, vec![[0.25, -0.5]; 8000]).unwrap());
        let fade = hex_project::ClipFade::new(8000, 0, 0, 0)
            .unwrap()
            .with_shape(start, end, 600, -500)
            .unwrap();
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        assert!(editor.project().clip(clip).unwrap().audio_edit().is_none());
        let key = crate::StreamSourceKey::new(1, asset);
        let metadata = Arc::new(
            crate::StreamedSourceMetadata::new(
                key,
                16_000,
                16_000,
                8000,
                8000,
                hex_project::IDENTITY_CONVERSION,
                [7; 32],
            )
            .unwrap(),
        );
        let sources = BTreeMap::from([(
            asset,
            crate::compile::PreparedAudioSource::Streamed(metadata),
        )]);
        let mut expected = vec![[0.25, -0.5]; 8000];
        expected[..start as usize].fill([0.0; 2]);
        expected[8000 - end as usize..].fill([0.0; 2]);
        let mut pages = Pages {
            key,
            frames: media[&asset].frames(),
        };
        for split in [false, true] {
            if split {
                editor
                    .execute(Command::SplitClip { clip, frame: 2000 })
                    .unwrap();
            }
            let streamed =
                crate::compile::project_with_prepared_audio_sources(editor.project(), &sources, 1)
                    .unwrap();
            for block in [1, 127] {
                let mut renderer = crate::Renderer::new(&streamed);
                renderer.play();
                let mut actual = vec![[0.0; 2]; 8000];
                for chunk in actual.chunks_mut(block) {
                    renderer.render_with_pages(chunk, &mut pages).unwrap();
                }
                assert_eq!(
                    actual, expected,
                    "endpoints={start}/{end}, split={split}, block={block}"
                );
                renderer.seek(800);
                let mut seek = vec![[0.0; 2]; 3000];
                renderer.render_with_pages(&mut seek, &mut pages).unwrap();
                assert_eq!(seek, expected[800..3800]);
            }
        }
    }
}

#[test]
fn sample_bpm_and_edge_stretch_keep_pcm_pitch_and_partition_seek_consistency() {
    let (mut editor, clip, mut media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let asset = editor.project().clip(clip).unwrap().asset();
    media.insert(
        asset,
        Audio::new(
            16_000,
            (0..8_000)
                .map(|i| {
                    let phase = i as f64 / 16_000.0 * std::f64::consts::TAU;
                    [
                        (phase * 400.0).sin() as f32 * 0.4,
                        (phase * 800.0).sin() as f32 * 0.3,
                    ]
                })
                .collect(),
        )
        .unwrap(),
    );
    let warp = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSampleBpm {
            clip,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 4_000);
    for length in [4_000, 6_000] {
        if length == 6_000 {
            editor
                .execute(Command::StretchAudioClipEdge {
                    clip,
                    edge: hex_project::TrimEdge::Right,
                    frame: length,
                })
                .unwrap();
        }
        let context = isolated(8 * 1024 * 1024, 4 * 1024 * 1024, 8);
        let revision = prepare(&editor, &media, &context);
        let pcm = render(&revision, 0, length as usize, 127);
        assert_eq!(pcm, render(&revision, 0, length as usize, 1));
        assert_eq!(&pcm[719..], render(&revision, 719, pcm.len() - 719, 257));
        let signal = &pcm[500..pcm.len() - 500];
        for (channel, hz) in [(0, 400.0), (1, 800.0)] {
            let crossings = signal
                .windows(2)
                .filter(|pair| pair[0][channel] <= 0.0 && pair[1][channel] > 0.0)
                .count();
            let measured = crossings as f64 * 16_000.0 / signal.len() as f64;
            assert!(
                (measured - hz).abs() < hz * 0.04,
                "duration{length} channel{channel} measured{measured}"
            );
        }
    }
}

#[test]
fn beats_actual_preparation_joins_cache_parameters_version_and_undo_match_rendered_playback() {
    let (mut editor, clip, mut media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let asset = editor.project().clip(clip).unwrap().asset();
    media.insert(
        asset,
        Audio::new(16_000, vec![[0.5, -0.25]; 8_000]).unwrap(),
    );
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Division(BeatTime::from_ratio(1, 4).unwrap()),
            tail: BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let length = editor.project().clip(clip).unwrap().duration() as usize;
    assert_eq!(length, 16_000);
    let context = isolated(8 * 1024 * 1024, 4 * 1024 * 1024, 8);
    let first = prepare(&editor, &media, &context);
    let pcm = render(&first, 0, length, 127);
    for at in [4_000, 8_000, 12_000] {
        assert_eq!(pcm[at], [0.5, -0.25]);
        assert_eq!(pcm[at - 1], pcm[at], "actual prepared neighbor join");
    }
    for block in [1, 31, 257, 513] {
        assert_eq!(render(&first, 0, length, block), pcm);
        assert_eq!(render(&first, 3_987, 511, block), pcm[3_987..4_498]);
    }
    let hits = context.cache_hits();
    let repeated = prepare(&editor, &media, &context);
    assert!(context.cache_hits() > hits);
    assert_eq!(render(&repeated, 0, length, 63), pcm);
    let mut gated = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .warp
        .as_ref()
        .unwrap()
        .clone();
    gated.mode = WarpMode::Beats {
        preserve: hex_project::BeatsPreserve::Division(BeatTime::from_ratio(1, 4).unwrap()),
        tail: BeatsTail::Forward,
        decay: 25,
    };
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(gated),
        })
        .unwrap();
    let low_hits = context.cache_hits();
    let decayed = prepare(&editor, &media, &context);
    assert_eq!(
        context.cache_hits(),
        low_hits,
        "envelope edit must invalidate prepared PCM"
    );
    let low_pcm = render(&decayed, 0, length, 63);
    for (at, frame) in low_pcm.iter().enumerate() {
        let gain = (-9.0 * (at % 4_000) as f64 / 4_000.0).exp();
        assert!((f64::from(frame[0]) - 0.5 * gain).abs() < 1e-7);
        assert!((f64::from(frame[1]) + 0.25 * gain).abs() < 1e-7);
    }
    assert!(editor.undo());
    let full_again = prepare(&editor, &media, &context);
    assert_eq!(render(&full_again, 0, length, 257), pcm);
    assert_eq!(
        render(&decayed, 0, length, 513),
        low_pcm,
        "retained lower-envelope revision is immutable"
    );
    let mut off = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .warp
        .as_ref()
        .unwrap()
        .clone();
    off.mode = WarpMode::Beats {
        preserve: hex_project::BeatsPreserve::Division(BeatTime::from_ratio(1, 4).unwrap()),
        tail: BeatsTail::Off,
        decay: 100,
    };
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(off),
        })
        .unwrap();
    let hits = context.cache_hits();
    let stopped = prepare(&editor, &media, &context);
    assert_eq!(
        context.cache_hits(),
        hits,
        "tail edit must not reuse Forward PCM"
    );
    let off_pcm = render(&stopped, 0, length, 127);
    assert!(off_pcm[2_000..4_000].iter().all(|s| *s == [0.0; 2]));
    assert_eq!(
        off_pcm[4_000], [0.0; 2],
        "Off restart removes background, not source attacks"
    );
    assert_ne!(off_pcm, pcm);
    assert!(editor.undo());
    let restored = prepare(&editor, &media, &context);
    assert!(context.cache_hits() > hits);
    assert_eq!(render(&restored, 0, length, 257), pcm);
    assert!(editor.redo());
    let hits = context.cache_hits();
    for entry in &mut context.cache.borrow_mut().entries {
        entry.algorithm_version = hex_warp::ALGORITHM_VERSION - 1;
    }
    let rebuilt = prepare(&editor, &media, &context);
    assert_eq!(
        context.cache_hits(),
        hits,
        "old algorithm PCM must not survive cache lookup"
    );
    assert_eq!(render(&rebuilt, 0, length, 513), off_pcm);
    assert_eq!(
        render(&first, 0, length, 127),
        pcm,
        "retained old revision remains immutable"
    );
    eprintln!("executed actual Beats canonical/preparation/cache/version/Undo/partition/seek path");
}

#[test]
fn beats_wsola_default_preparation_locks_drums_and_keeps_bass_continuous() {
    let (mut editor, clip, mut media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let asset = editor.project().clip(clip).unwrap().asset();
    let original: Vec<_> = (0..8_000)
        .map(|i| {
            let bass = (std::f32::consts::TAU * 55.0 * i as f32 / 16_000.0).sin() * 0.15;
            if i % 2_000 == 0 {
                [0.9, -0.8]
            } else {
                [bass, -bass * 0.7]
            }
        })
        .collect();
    media.insert(asset, Audio::new(16_000, original.clone()).unwrap());
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let context = isolated(8 * 1024 * 1024, 4 * 1024 * 1024, 8);
    for (tempo, ratio) in [(80_000_000, 1.5), (60_000_000, 2.0)] {
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(tempo).unwrap(),
            })
            .unwrap();
        let length = editor.project().clip(clip).unwrap().duration() as usize;
        assert_eq!(length, (8_000.0 * ratio) as usize);
        let revision = prepare(&editor, &media, &context);
        let pcm = render(&revision, 0, length, 127);
        let expected: Vec<_> = [0, 2_000, 4_000, 6_000]
            .map(|i| (i as f64 * ratio) as usize)
            .to_vec();
        let actual: Vec<_> = pcm
            .iter()
            .enumerate()
            .filter_map(|(i, x)| (x[0] > 0.7).then_some(i))
            .collect();
        assert_eq!(
            actual, expected,
            "mapped attacks must neither move nor repeat"
        );
        for at in &expected {
            assert_eq!(pcm[*at], [0.9, -0.8]);
        }
        for interval in expected.windows(2) {
            let from = interval[0] + 500;
            let to = interval[1] - 500;
            let rms = (pcm[from..to]
                .iter()
                .map(|x| f64::from(x[0]).powi(2))
                .sum::<f64>()
                / (to - from) as f64)
                .sqrt();
            assert!((0.09..0.12).contains(&rms), "bass bed RMS {rms}");
            assert!(
                pcm[from..to]
                    .iter()
                    .all(|x| (x[1] + x[0] * 0.7).abs() < 1e-6)
            );
            let largest_step = pcm[from..to]
                .windows(2)
                .map(|w| (w[1][0] - w[0][0]).abs())
                .fold(0.0_f32, f32::max);
            assert!(largest_step < 0.01, "generated bass edge {largest_step}");
        }
        for block in [1, 31, 257, 513] {
            assert_eq!(render(&revision, 0, length, block), pcm);
            assert_eq!(
                render(&revision, expected[1] as u64 - 19, 511, block),
                pcm[expected[1] - 19..expected[1] + 492]
            );
        }
        let hits = context.cache_hits();
        let repeated = prepare(&editor, &media, &context);
        assert!(context.cache_hits() > hits);
        assert_eq!(render(&repeated, 0, length, 63), pcm);
        eprintln!(
            "executed actual default Transients Forward100 WSOLA drum+bass ratio={ratio} frames={length}, locked attacks/continuous bass/partition/seek/cache"
        );
    }
}

#[test]
fn beats_alternating_preparation_reverses_safe_chirp_tail_and_undo_preserves_revisions() {
    let (mut editor, clip, mut media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let asset = editor.project().clip(clip).unwrap().asset();
    media.insert(
        asset,
        Audio::new(
            16_000,
            (0..8_000)
                .map(|i| {
                    let t = i as f64 / 16_000.0;
                    let x = (std::f64::consts::TAU * (60.0 * t + 500.0 * t * t) + 0.4).sin() as f32
                        * 0.5;
                    [x, -x * 0.7]
                })
                .collect(),
        )
        .unwrap(),
    );
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let context = isolated(8 * 1024 * 1024, 4 * 1024 * 1024, 8);
    let forward = prepare(&editor, &media, &context);
    let forward_pcm = render(&forward, 0, 16_000, 127);
    let mut settings = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .warp
        .as_ref()
        .unwrap()
        .clone();
    settings.mode = WarpMode::Beats {
        preserve: hex_project::BeatsPreserve::Transients,
        tail: BeatsTail::Alternating,
        decay: 100,
    };
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let hits = context.cache_hits();
    let alternating = prepare(&editor, &media, &context);
    assert_eq!(context.cache_hits(), hits);
    let pcm = render(&alternating, 0, 16_000, 63);
    assert_ne!(
        pcm, forward_pcm,
        "Alternating must not silently use Forward traversal"
    );
    let frequency = |frames: &[crate::Frame]| {
        frames
            .windows(2)
            .filter(|w| w[0][0] <= 0.0 && w[1][0] > 0.0)
            .count() as f64
            * 16_000.0
            / frames.len() as f64
    };
    let reversed = frequency(&pcm[11_500..12_500]);
    let normal = frequency(&forward_pcm[11_500..12_500]);
    assert!(
        reversed > 200.0 && reversed + 60.0 < normal,
        "safe chirp tail not reversed: Alt={reversed} Forward={normal}"
    );
    assert!(pcm.iter().all(|x| (x[1] + x[0] * 0.7).abs() < 1e-6));
    for block in [1, 31, 257, 513] {
        assert_eq!(render(&alternating, 0, 16_000, block), pcm);
        assert_eq!(
            render(&alternating, 11_937, 511, block),
            pcm[11_937..12_448]
        );
    }
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 257),
        forward_pcm
    );
    drop(editor);
    drop(media);
    assert_eq!(render(&alternating, 0, 16_000, 513), pcm);
    assert_eq!(render(&forward, 0, 16_000, 127), forward_pcm);
    eprintln!(
        "executed actual Alternating protected WSOLA chirp native preparation/cache/Undo/retained revisions/seek/partition: AltHz={reversed} ForwardHz={normal}"
    );
}

fn preview_matches_preparation(
    editor: &Editor,
    clip: hex_project::ClipId,
    media: &BTreeMap<hex_project::AssetId, Audio>,
) -> bool {
    let metadata = validate_clip_warp_preview(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
    );
    let actual = crate::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        media,
        Arc::new(crate::SineTable::new()),
        None,
        &AudioTransformPreparation::default(),
    );
    assert_eq!(
        metadata.is_ok(),
        actual.is_ok(),
        "metadata={metadata:?} actual_error={:?}",
        actual.as_ref().err()
    );
    if let (Err(expected), Err(actual)) = (&metadata, &actual) {
        assert_eq!(expected, actual);
    }
    metadata.is_ok()
}
#[test]
fn warp_preview_admission_matches_actual_preparation_pitch_modes_tempo_and_rounding() {
    for pitch in [-1_200, 0, 1_200] {
        for mode in [
            None,
            Some(WarpMode::RePitch),
            Some(WarpMode::Beats {
                preserve: hex_project::BeatsPreserve::Transients,
                tail: BeatsTail::Forward,
                decay: 100,
            }),
        ] {
            for end in [499, 500, 1_000, 8_000, 64_000, 64_001] {
                let (mut editor, clip, media) = fixture(false);
                let settings = default_audio_warp(
                    editor.project().clip(clip).unwrap(),
                    editor.project().tempo_map(),
                    mode.unwrap_or(WarpMode::RePitch),
                )
                .unwrap();
                editor
                    .execute(Command::SetAudioClipWarp {
                        clip,
                        settings: Some(settings),
                    })
                    .unwrap();
                editor
                    .execute(Command::SetAudioClipPitch { clip, cents: pitch })
                    .unwrap();
                editor
                    .execute(Command::SetTempoPoint {
                        beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                        tempo: Tempo::new(90_000_000).unwrap(),
                    })
                    .unwrap();
                editor
                    .execute(Command::StretchAudioClipEdge {
                        clip,
                        edge: hex_project::TrimEdge::Right,
                        frame: end,
                    })
                    .unwrap();
                if mode.is_none() {
                    editor
                        .execute(Command::SetAudioClipWarp {
                            clip,
                            settings: None,
                        })
                        .unwrap();
                }
                preview_matches_preparation(&editor, clip, &media);
            }
        }
    }
}
#[test]
fn duplicated_warped_audio_keeps_preparable_timing() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(2 * BeatTime::ONE.raw() - 1),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let compile = |project: &Project| {
        crate::PreparedRevision::prepare_with_audio_transforms(
            project,
            &media,
            Arc::new(crate::SineTable::new()),
            None,
            &context,
        )
    };
    let before = compile(editor.project()).unwrap();
    let original_pcm = render(&before, 0, 8_000, 97);
    let original = editor.project().clip(clip).unwrap().clone();
    let mut selected = clip;
    for iteration in 0..3 {
        let (
            Created::Clips {
                first: duplicate, ..
            },
            prepared,
        ) = editor
            .execute_prepared(Command::DuplicateClips(vec![selected]), compile)
            .unwrap()
        else {
            panic!("duplicate selection")
        };
        let copy = editor.project().clip(duplicate).unwrap();
        assert_eq!(copy.source_offset(), original.source_offset());
        assert_eq!(copy.source_length(), original.source_length());
        assert_eq!(
            copy.audio_edit().unwrap().domain,
            original.audio_edit().unwrap().domain
        );
        assert_eq!(editor.project().clip(clip), Some(&original));
        assert!(preview_matches_preparation(&editor, duplicate, &media));
        if iteration == 0 {
            assert_eq!(render(&prepared, copy.start(), 8_000, 53), original_pcm);
        }
        assert!(editor.undo_prepared(compile).unwrap().is_some());
        assert!(editor.project().clip(duplicate).is_none());
        assert!(editor.redo_prepared(compile).unwrap().is_some());
        assert_eq!(
            editor
                .project()
                .clip(duplicate)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain,
            original.audio_edit().unwrap().domain
        );
        selected = duplicate;
    }
}

#[test]
fn streamed_repitch_projects_canonical_map_and_refuses_other_processing() {
    let (mut editor, clip, _) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let edit = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .clone();
    let tempo = editor.project().tempo_map();
    let cancel = AtomicBool::new(false);
    let (resident, _) = mapping::prepare_map(&edit, tempo, &cancel).unwrap();
    let (streamed, law) = streamed_repitch_map(&edit, tempo, 8_000, &cancel).unwrap();
    assert_eq!(streamed, resident);
    assert_eq!(law, render_resampler(&edit.domain));
    let asset = editor.project().clip(clip).unwrap().asset();
    let raw = crate::StreamedSourceMetadata::new(
        crate::StreamSourceKey::new(1, asset),
        16_000,
        16_000,
        8_000,
        8_000,
        hex_project::IDENTITY_CONVERSION,
        [3; 32],
    )
    .unwrap();
    let requirement = StreamedRepitchRequirement::new(&raw, &edit, tempo, &cancel).unwrap();
    assert_eq!(requirement.map(), &streamed);
    let worker_map = requirement.try_owned_map().unwrap();
    assert_eq!(worker_map, streamed);
    assert_ne!(
        worker_map.segments().as_ptr(),
        requirement.map().segments().as_ptr()
    );
    assert_eq!(requirement.law(), law);
    assert_eq!(
        requirement.metadata_for(&raw).unwrap(),
        raw.with_repitch(&streamed, law).unwrap()
    );
    assert!(requirement.retained_bytes().unwrap() >= streamed.backing_bytes().unwrap());
    assert_eq!(
        requirement.clone().metadata_for(&raw).unwrap(),
        requirement.metadata_for(&raw).unwrap()
    );
    let foreign = crate::StreamedSourceMetadata::new(
        crate::StreamSourceKey::new(1, asset),
        16_000,
        16_000,
        8_000,
        8_000,
        hex_project::IDENTITY_CONVERSION,
        [4; 32],
    )
    .unwrap();
    assert!(requirement.metadata_for(&foreign).is_err());
    let next_generation = crate::StreamedSourceMetadata::new(
        crate::StreamSourceKey::new(2, asset),
        16_000,
        16_000,
        8_000,
        8_000,
        hex_project::IDENTITY_CONVERSION,
        [3; 32],
    )
    .unwrap();
    assert_eq!(
        requirement.metadata_for(&next_generation).unwrap(),
        next_generation.with_repitch(&streamed, law).unwrap()
    );
    assert!(streamed_repitch_map(&edit, tempo, 7_999, &cancel).is_err());
    let mut invalid = edit.clone();
    Arc::make_mut(&mut invalid.domain).pitch_cents = 100;
    assert!(streamed_repitch_map(&invalid, tempo, 8_000, &cancel).is_err());
    assert!(StreamedRepitchRequirement::new(&raw, &invalid, tempo, &cancel).is_err());
    assert!(
        StreamedRepitchRequirement::new(
            &raw.with_repitch(&streamed, law).unwrap(),
            &edit,
            tempo,
            &cancel
        )
        .is_err()
    );
    Arc::make_mut(&mut invalid.domain).pitch_cents = 0;
    Arc::make_mut(&mut invalid.domain).looping = true;
    assert!(streamed_repitch_map(&invalid, tempo, 8_000, &cancel).is_err());
    let mut long = edit;
    let beat_length = BeatTime::from_raw(BeatTime::ONE.raw() * 2_500);
    let domain = Arc::make_mut(&mut long.domain);
    domain.source_length = 20_000_000;
    domain.clock = AudioDomainClock::Beats(beat_length);
    let last = domain.warp.as_mut().unwrap().markers.last_mut().unwrap();
    last.source_frame = 20_000_000;
    last.beat = beat_length;
    long.view_end = beat_length.raw();
    assert!(mapping::prepare_map(&long, tempo, &cancel).is_err());
    let (map, _) = streamed_repitch_map(&long, tempo, 20_000_000, &cancel).unwrap();
    assert_eq!(map.segments().len(), 1);
    assert_eq!(map.segments()[0].output_end, 20_000_000);
    assert_eq!(map.segments()[0].source_end_q32, 20_000_000_i128 << 32);
    assert!(map.backing_bytes().unwrap() < 1024);
    assert!(streamed_repitch_map(&long, tempo, 20_000_000, &AtomicBool::new(true)).is_err());
}

#[test]
fn warp_map_coalesces_partial_cycles_and_tempo_crossings_but_preserves_authored_markers() {
    let (mut editor, clip, _) = fixture(false);
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let tempo = editor.project().tempo_map();
    let mut edit = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .clone();
    let domain = Arc::make_mut(&mut edit.domain);
    domain.looping = true;
    domain.clock = AudioDomainClock::Beats(BeatTime::from_raw(BeatTime::ONE.raw() + 1));
    let (map, _) = mapping::prepare_map(&edit, tempo, &AtomicBool::new(false)).unwrap();
    assert_eq!(map.segments().len(), 1);
    assert_eq!(map.segments()[0].output_end, 8_000);
    assert_eq!(map.segments()[0].source_start_q32, 0);
    assert_eq!(
        map.segments()[0].source_end_q32,
        edit.domain
            .warp_source_position_q32(u128::from(BeatTime::ONE.raw() + 1))
            .unwrap()
    );

    // An interior tempo point that rounds onto a real marker must not move
    // that marker's source coordinate or leave a gap between map segments.
    let domain = Arc::make_mut(&mut edit.domain);
    domain.clock = AudioDomainClock::Beats(BeatTime::ONE);
    domain.warp.as_mut().unwrap().markers.insert(
        1,
        hex_project::WarpMarker {
            source_frame: 3_000,
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
        },
    );
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2 + 1),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let (map, _) =
        mapping::prepare_map(&edit, editor.project().tempo_map(), &AtomicBool::new(false)).unwrap();
    assert_eq!(map.segments().len(), 2);
    assert_eq!(map.segments()[0].output_end, 4_000);
    assert_eq!(map.segments()[0].source_end_q32, 3_000_i128 << 32);
    assert_eq!(map.segments()[1].source_start_q32, 3_000_i128 << 32);

    Arc::make_mut(&mut edit.domain)
        .warp
        .as_mut()
        .unwrap()
        .markers
        .insert(
            1,
            hex_project::WarpMarker {
                source_frame: 1_000,
                beat: BeatTime::from_raw(1),
            },
        );
    assert!(
        mapping::prepare_map(&edit, editor.project().tempo_map(), &AtomicBool::new(false)).is_err()
    );
}
#[test]
fn faster_tempo_coalesces_supported_authored_markers_with_exact_root_endpoints() {
    let (mut editor, clip, _) = fixture(false);
    let mut settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    let tick = BeatTime::ONE.raw() / 8_000;
    for (source_frame, beat) in [
        (1, tick),
        (4_000, BeatTime::ONE.raw() / 2),
        (4_001, BeatTime::ONE.raw() / 2 + tick),
        (7_999, BeatTime::ONE.raw() - tick),
    ] {
        settings.markers.insert(
            settings.markers.len() - 1,
            hex_project::WarpMarker {
                source_frame,
                beat: BeatTime::from_raw(beat),
            },
        );
    }
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(480_000_000).unwrap(),
        })
        .unwrap();
    let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
    let (map, _) =
        mapping::prepare_map(edit, editor.project().tempo_map(), &AtomicBool::new(false)).unwrap();
    assert_eq!(map.segments().len(), 2);
    assert_eq!(
        map.segments()[0],
        hex_warp::MapSegment {
            output_start: 0,
            output_end: 1_000,
            source_start_q32: 0,
            source_end_q32: 4_001_i128 << 32,
        }
    );
    assert_eq!(
        map.segments()[1],
        hex_warp::MapSegment {
            output_start: 1_000,
            output_end: 2_000,
            source_start_q32: 4_001_i128 << 32,
            source_end_q32: 8_000_i128 << 32,
        }
    );
    assert_eq!(
        edit.domain.warp.as_ref().unwrap().markers.len(),
        6,
        "quantization must not rewrite authored markers"
    );
}

#[test]
fn warp_preview_admission_nonuniform_markers_and_subframe_errors_match_actual_preparation() {
    for (middle, pitch) in [
        (BeatTime::ONE.raw() / 2, 0),
        (BeatTime::ONE.raw() / 2, 2_400),
        (1, 0),
    ] {
        let (mut editor, clip, media) = fixture(false);
        let mut settings = default_audio_warp(
            editor.project().clip(clip).unwrap(),
            editor.project().tempo_map(),
            WarpMode::Beats {
                preserve: hex_project::BeatsPreserve::Transients,
                tail: BeatsTail::Forward,
                decay: 100,
            },
        )
        .unwrap();
        settings.markers.insert(
            1,
            hex_project::WarpMarker {
                source_frame: 1_600,
                beat: BeatTime::from_raw(middle),
            },
        );
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: pitch })
            .unwrap();
        let valid = preview_matches_preparation(&editor, clip, &media);
        assert_eq!(
            valid,
            middle > 1 && pitch == 0,
            "nonuniform first segment or subframe projection must control admission"
        );
    }
    let (editor, clip, media) = fixture(false);
    assert!(
        preview_matches_preparation(&editor, clip, &media),
        "untouched resident audio needs no warp map"
    );
}
#[test]
fn warp_preview_valid_endpoints_can_be_disconnected_by_marker_tempo_crossings() {
    // A slow-source second marker interval first traverses 30bpm, then moves
    // entirely beyond the 480bpm point. Its maximum stretch drops, so validity
    // is NOT globally monotonic in the requested right-edge frame.
    let (mut seed, clip, media) = fixture(false);
    let mut settings = default_audio_warp(
        seed.project().clip(clip).unwrap(),
        seed.project().tempo_map(),
        WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    settings.markers.insert(
        1,
        hex_project::WarpMarker {
            source_frame: 7_600,
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
        },
    );
    seed.execute(Command::SetAudioClipWarp {
        clip,
        settings: Some(settings),
    })
    .unwrap();
    seed.execute(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(30_000_000).unwrap(),
    })
    .unwrap();
    seed.execute(Command::SetTempoPoint {
        beat: BeatTime::ONE,
        tempo: Tempo::new(480_000_000).unwrap(),
    })
    .unwrap();
    for (end, expected) in [(3_200, true), (32_000, false), (34_200, true)] {
        let mut editor = Editor::new(seed.project().clone(), 32).unwrap();
        editor
            .execute(Command::StretchAudioClipEdge {
                clip,
                edge: hex_project::TrimEdge::Right,
                frame: end,
            })
            .unwrap();
        assert_eq!(
            preview_matches_preparation(&editor, clip, &media),
            expected,
            "endpoint {end}"
        );
    }
}

#[test]
fn derived_pcm_ingress_refusal_keeps_its_actual_reservation_and_buffer() {
    let budget = Arc::new(budget::Budget::new(1024));
    let reservation = budget.reserve(32).unwrap();
    let audio = Audio::from_derived(8_000, vec![[0.25; 2]; 4], reservation).unwrap();
    let pointer = audio.frames().as_ptr();
    let refused = crate::OwnedAudioSource::try_from_decoded_audio_on_control(
        audio,
        4,
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert_eq!(
        refused.reason,
        crate::native_device_resources::ResourcePreparationError::Storage(
            crate::AudioStorageError::SourceMismatch
        )
    );
    assert_eq!(refused.audio.frames().as_ptr(), pointer);
    assert!(refused.audio.frames._derived.is_some());
    assert_eq!(budget.used(), 32);
    drop(refused);
    assert_eq!(budget.used(), 0);
}

#[path = "tests/project_tempo.rs"]
mod project_tempo;

#[test]
fn section_reorder_preserves_complete_processed_roots_pcm_across_fractional_tempo_prefixes_and_repeated_moves()
 {
    section_pcm_oracle(true, false);
}
#[test]
fn section_reorder_preserves_cropped_event_roots_and_raw_loop_fades_sample_exactly() {
    section_pcm_oracle(true, true);
    section_pcm_oracle(false, false);
}
fn section_pcm_oracle(transformed: bool, cropped: bool) {
    use hex_project::{ElementColor, TempoPoint};
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    let (mut editor, clip, media) = fixture(true);
    let mut snapshot = editor.project().snapshot();
    let audio = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    audio.duration = 96_000;
    audio.fade = hex_project::ClipFade::new(96_000, 0, 997, 1601).unwrap();
    editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
    if transformed {
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 137 })
            .unwrap();
        let warp = default_audio_warp(
            editor.project().clip(clip).unwrap(),
            editor.project().tempo_map(),
            WarpMode::Texture {
                grain_ms: 40,
                fluctuation: 60,
                seed: 91,
            },
        )
        .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            })
            .unwrap();
    }
    for point in [
        TempoPoint::new(beat(0), Tempo::new(123_456_789).unwrap()),
        TempoPoint::new(beat(3), Tempo::new(97_333_331).unwrap()),
        TempoPoint::new(beat(6), Tempo::new(177_777_779).unwrap()),
        TempoPoint::new(beat(9), Tempo::new(83_123_457).unwrap()),
    ] {
        editor
            .execute(Command::SetTempoPoint {
                beat: point.beat(),
                tempo: point.tempo(),
            })
            .unwrap();
    }
    if cropped {
        // Edits refer to the same complete root; a moved visible crop must not
        // restart analysis or reinterpret events using the new tempo outside it.
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: hex_project::AudioEventEdit::MoveRange {
                    start: beat(1).raw(),
                    end: beat(2).raw(),
                    destination: beat(4).raw(),
                },
            })
            .unwrap();
        for (edge, at) in [
            (hex_project::TrimEdge::Right, beat(6)),
            (hex_project::TrimEdge::Left, beat(1)),
        ] {
            let frame = editor.project().tempo_map().beat_to_frame(at).unwrap();
            editor
                .execute(Command::TrimClipEdge { clip, edge, frame })
                .unwrap();
        }
        assert!(
            editor
                .project()
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .events
                .is_some()
        );
    }
    let mut add = |name: &str, start, end| {
        let Created::ArrangementSection(id) = editor
            .execute(Command::AddArrangementSection {
                name: name.into(),
                color: ElementColor::Cyan,
                start: beat(start),
                end: beat(end),
            })
            .unwrap()
        else {
            panic!("section")
        };
        id
    };
    let a = add("intro", 2, 4);
    let c = add("chorus", 7, 10);
    let context = isolated(32 * 1024 * 1024, 16 * 1024 * 1024, 32);
    let length = editor
        .project()
        .tempo_map()
        .beat_to_frame(beat(12))
        .unwrap()
        .max(
            editor.project().clip(clip).unwrap().start()
                + editor.project().clip(clip).unwrap().duration(),
        ) as usize;
    let mut expected = render(&prepare(&editor, &media, &context), 0, length, 127);
    // Repeated permutations also split already frozen roots. Source samples
    // remain exact; no crop is rendered against destination-only tempo context.
    for (source, target) in [(c, a), (a, c), (c, a), (a, c)] {
        let preview = editor
            .project()
            .preview_section_reorder(source, target)
            .unwrap();
        let map = editor.project().tempo_map();
        let s = map.beat_to_frame(preview.source_start()).unwrap() as usize;
        let e = map.beat_to_frame(preview.source_end()).unwrap() as usize;
        let t = map.beat_to_frame(preview.insertion_beat()).unwrap() as usize;
        let mut permuted = Vec::with_capacity(length);
        if t < s {
            permuted.extend_from_slice(&expected[..t]);
            permuted.extend_from_slice(&expected[s..e]);
            permuted.extend_from_slice(&expected[t..s]);
            permuted.extend_from_slice(&expected[e..]);
        } else {
            permuted.extend_from_slice(&expected[..s]);
            permuted.extend_from_slice(&expected[e..t]);
            permuted.extend_from_slice(&expected[s..e]);
            permuted.extend_from_slice(&expected[t..]);
        }
        editor
            .execute(Command::ReorderSectionBefore {
                section: source,
                before: target,
            })
            .unwrap();
        let actual = render(&prepare(&editor, &media, &context), 0, length, 251);
        assert_eq!(
            actual, permuted,
            "complete frozen source PCM must survive section permutation"
        );
        expected = permuted;
        let snapshot = editor.project().snapshot();
        assert_eq!(
            Project::from_snapshot(snapshot.clone()).unwrap().snapshot(),
            snapshot
        );
    }
    if transformed {
        let before = editor.project().snapshot();
        let undo = editor.can_undo();
        assert!(
            editor
                .execute(Command::SetTempoPoint {
                    beat: beat(0),
                    tempo: Tempo::new(120_000_000).unwrap()
                })
                .unwrap_err()
                .0
                .contains("processing rebase")
        );
        assert_eq!(editor.project().snapshot(), before);
        assert_eq!(editor.can_undo(), undo);
    }
}

#[test]
fn section_reorder_prepares_sounding_cross_track_audio_and_loop_midi_with_boundary_retriggers() {
    use hex_project::ElementColor;
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    let (mut editor, clip, media) = fixture(true);
    let mut snapshot = editor.project().snapshot();
    let audio = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    audio.duration = 96_000;
    audio.fade = hex_project::ClipFade::neutral(96_000).unwrap();
    editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "melody".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(0),
            length: beat(12),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip: midi,
            source_length: beat(3),
            source_start: beat(0),
            loop_start: beat(0),
            loop_end: beat(3),
            looping: true,
            length: beat(12),
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::from_ratio(1, 2).unwrap(),
            duration: beat(2),
            pitch: 69,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut add = |start, end| {
        let Created::ArrangementSection(id) = editor
            .execute(Command::AddArrangementSection {
                name: "part".into(),
                color: ElementColor::Blue,
                start: beat(start),
                end: beat(end),
            })
            .unwrap()
        else {
            panic!("section")
        };
        id
    };
    let a = add(2, 4);
    let c = add(7, 10);
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let original = prepare(&editor, &media, &context);
    assert_eq!(
        original.midi_preview().get(midi).unwrap().occurrence_count,
        4
    );
    editor
        .execute(Command::ReorderSectionBefore {
            section: c,
            before: a,
        })
        .unwrap();
    let prepared = prepare(&editor, &media, &context);
    let mut intervals = prepared
        .midi_preview()
        .clips()
        .iter()
        .flat_map(|clip| clip.samples.iter())
        .map(|sample| (sample.frame_start, sample.frame_gate_end, sample.pitch))
        .collect::<Vec<_>>();
    intervals.sort();
    let expected = [
        (1, 4),
        (4, 7),
        (9, 10),
        (10, 11),
        (13, 17),
        (19, 20),
        (20, 23),
    ]
    .into_iter()
    .map(|(s, e)| (s * 4_000, e * 4_000, 69))
    .collect::<Vec<_>>();
    assert_eq!(
        intervals, expected,
        "actual prepared MIDI must include each cut/retrigger, not just moved clip markers"
    );
    let mixed = render(&prepared, 0, 96_000, 127);
    editor
        .execute(Command::Mute { track, muted: true })
        .unwrap();
    let audio_only = render(&prepare(&editor, &media, &context), 0, 96_000, 251);
    assert!(
        mixed.iter().zip(&audio_only).any(|(a, b)| a != b),
        "the MIDI track must actually sound alongside original source audio"
    );
    assert_eq!(
        editor.project().tracks()[0]
            .clips()
            .iter()
            .map(|c| c.duration())
            .sum::<u64>(),
        96_000
    );
    assert!(editor.project().clip(clip).is_some());
    // The live publication law reconciles the edited program at the actual
    // boundary, including an already sounding MIDI note and its release owner.
    let mut previous =
        crate::playback::Playback::try_from_prepared_with_session(Arc::new(original)).unwrap();
    previous.apply(crate::PlaybackCommand::Seek(9_000));
    previous.apply(crate::PlaybackCommand::Play);
    previous.render(&mut [[0.0; 2]; 63]).unwrap();
    let position = previous.position();
    let mut next =
        crate::playback::Playback::try_from_prepared_with_session(Arc::new(prepared)).unwrap();
    next.try_inherit_runtime_from(&mut previous).unwrap();
    assert!(next.playing());
    assert_eq!(next.position(), position);
    let mut sounding = [[0.0; 2]; 127];
    next.render(&mut sounding).unwrap();
    assert!(sounding.iter().any(|frame| frame[0] != 0.0));
    next.apply(crate::PlaybackCommand::Stop);
    next.render(&mut sounding).unwrap();
    assert_eq!(sounding, [[0.0; 2]; 127]);
}

#[test]
fn edited_frame_events_can_enable_warp_and_keep_sound_through_timing_toggles() {
    use hex_project::{AudioEventEdit, TrimEdge};
    let compare = |actual: Vec<crate::Frame>, expected: &[crate::Frame], label: &str| {
        let first = actual.iter().zip(expected).position(|(a, b)| a != b);
        assert!(
            actual.len() == expected.len() && first.is_none(),
            "{label}: first different frame {first:?}"
        );
    };
    for looping in [false, true] {
        let (mut editor, clip, media) = fixture(looping);
        let mut reference = Editor::new(editor.project().clone(), 32).unwrap();
        let reference_settings = default_audio_warp(
            reference.project().clip(clip).unwrap(),
            reference.project().tempo_map(),
            WarpMode::RePitch,
        )
        .unwrap();
        reference
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(reference_settings),
            })
            .unwrap();
        let operations = [
            AudioEventEdit::Split { at: 2000 },
            AudioEventEdit::Move {
                event: 1,
                start: 4000,
            },
            AudioEventEdit::Trim {
                event: 1,
                start: 4250,
                end: 5750,
            },
            AudioEventEdit::Delete {
                start: 6000,
                end: 6500,
            },
        ];
        for operation in operations {
            editor
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        let root = reference
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .clone();
        let at = |frame| {
            root.root_position_at(frame, reference.project().tempo_map())
                .unwrap()
        };
        let operations = [
            AudioEventEdit::Split { at: at(2000) },
            AudioEventEdit::Move {
                event: 1,
                start: at(4000),
            },
            AudioEventEdit::Trim {
                event: 1,
                start: at(4250),
                end: at(5750),
            },
            AudioEventEdit::Delete {
                start: at(6000),
                end: at(6500),
            },
        ];
        for operation in operations {
            reference
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        let track = editor.project().tracks()[0].id();
        for e in [&mut editor, &mut reference] {
            e.execute(Command::MoveClip {
                clip,
                track,
                start: 10_000,
            })
            .unwrap();
            e.execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 11_000,
            })
            .unwrap();
        }
        let original = editor.project().clip(clip).unwrap().clone();
        let context = AudioTransformPreparation::default();
        let baseline = render(&prepare(&editor, &media, &context), 10_000, 8000, 257);
        // Independent command order: enable warp on the unedited root, then
        // perform the same cuts/move/trim in its musical coordinates. This also
        // retains existing warp-specific loop seam processing in the oracle.
        let expected = render(&prepare(&reference, &media, &context), 10_000, 8000, 173);
        if !looping {
            compare(expected.clone(), &baseline, "unity warp");
        }
        let settings =
            default_audio_warp(&original, editor.project().tempo_map(), WarpMode::RePitch).unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings.clone()),
            })
            .unwrap();
        let musical = editor.project().clip(clip).unwrap().clone();
        let edit = musical.audio_edit().unwrap();
        let events = musical.audio_events();
        let old = original.audio_events();
        assert_eq!(events.next_id, old.next_id);
        assert_eq!(events.events.len(), old.events.len());
        for (new, old) in events.events.iter().zip(&old.events) {
            assert_eq!(new.id, old.id);
            for (new, old) in [
                (new.start, old.start),
                (new.end, old.end),
                (new.source_start, old.source_start),
            ] {
                assert_eq!(
                    edit.root_frame_at(new, editor.project().tempo_map())
                        .unwrap(),
                    old
                );
            }
        }
        for block in [1, 257] {
            compare(
                render(&prepare(&editor, &media, &context), 10_000, 8000, block),
                &expected,
                "event/warp command order",
            );
        }
        assert!(editor.undo());
        assert_eq!(editor.project().clip(clip).unwrap(), &original);
        assert!(editor.redo());
        assert_eq!(editor.project().clip(clip).unwrap(), &musical);
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: None,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: false,
            })
            .unwrap();
        assert!(
            editor
                .project()
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .live_musical_start()
                .is_none()
        );
        compare(
            render(&prepare(&editor, &media, &context), 10_000, 8000, 17),
            &baseline,
            "unwarped frozen events",
        );
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        compare(
            render(&prepare(&editor, &media, &context), 10_000, 8000, 127),
            &expected,
            "re-enabled warp",
        );
        editor
            .execute(Command::SetProjectTempo {
                tempo: Tempo::new(180_000_000).unwrap(),
            })
            .unwrap();
        let changed = prepare(&editor, &media, &context);
        let pcm = render(&changed, 0, 18_000, 173);
        assert!(pcm.iter().flatten().any(|x| x.abs() > 0.01));
        compare(
            render(&changed, 7000, 4000, 31),
            &pcm[7000..11_000],
            "tempo seek partition",
        );
    }
}

#[test]
fn per_event_source_fades_are_audible_and_survive_split_move_seek_and_blocks() {
    use hex_project::{AudioEventEdit, AudioEventFade, ClipFade};
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 257);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetFade {
                event: 1,
                fade: Some(AudioEventFade::new(0, ClipFade::new(8000, 0, 8, 8).unwrap()).unwrap()),
            },
        })
        .unwrap();
    let prepared = prepare(&editor, &media, &context);
    let actual = render(&prepared, 0, 8000, 257);
    let mut expected = baseline.clone();
    for (index, frame) in expected.iter_mut().enumerate() {
        let gain = if index < 8 {
            index as f32 / 7.0
        } else if index >= 7992 {
            (7999 - index) as f32 / 7.0
        } else {
            1.0
        };
        for sample in frame {
            *sample *= gain;
        }
    }
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            (a[0] - b[0]).abs() <= 1e-7 && (a[1] - b[1]).abs() <= 1e-7,
            "event fade PCM at {i}: {a:?} != {b:?}"
        );
    }
    assert_eq!(render(&prepared, 0, 8000, 1), actual);
    assert_eq!(render(&prepared, 3, 7995, 17), actual[3..7998]);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 4000 },
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8000, 127),
        actual
    );
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: 4000,
            },
        })
        .unwrap();
    let mut moved = vec![[0.0; 2]; 8000];
    moved[4000..].copy_from_slice(&actual[..4000]);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8000, 17),
        moved
    );
}

#[test]
fn per_event_short_overlapping_fades_keep_literal_endpoint_law() {
    use hex_project::{AudioEventEdit, AudioEventFade, ClipFade};
    for (length, ramp, gains) in [
        (1, 1, vec![0.0]),
        (2, 2, vec![0.0, 0.0]),
        (4, 3, vec![0.0, 0.5, 0.5, 0.0]),
    ] {
        let (mut editor, clip, media) = fixture(false);
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let context = AudioTransformPreparation::default();
        let baseline = render(&prepare(&editor, &media, &context), 0, 8000, 257);
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: length },
            })
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetFade {
                    event: 1,
                    fade: Some(
                        AudioEventFade::new(0, ClipFade::new(length, 0, ramp, ramp).unwrap())
                            .unwrap(),
                    ),
                },
            })
            .unwrap();
        let actual = render(&prepare(&editor, &media, &context), 0, 8000, 17);
        for (i, gain) in gains.into_iter().enumerate() {
            assert_eq!(actual[i], [baseline[i][0] * gain, baseline[i][1] * gain]);
        }
        assert_eq!(actual[length as usize..], baseline[length as usize..]);
    }
}

#[test]
fn per_event_frame_fades_reproject_through_warp_and_a_tempo_seam() {
    use hex_project::{AudioEventEdit, AudioEventFade, ClipFade};
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetFade {
                event: 1,
                fade: Some(
                    AudioEventFade::new(0, ClipFade::new(8000, 0, 2000, 2000).unwrap()).unwrap(),
                ),
            },
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let event = editor.project().clip(clip).unwrap().audio_events().events[0];
    assert_eq!(event.fade.unwrap().shape().span(), BeatTime::ONE.raw());
    assert_eq!(
        event.fade.unwrap().shape().fade_in(),
        BeatTime::ONE.raw() / 4
    );
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
            tempo: Tempo::new(240_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 6000);
    let context = AudioTransformPreparation::default();
    let with_fade = prepare(&editor, &media, &context);
    let actual = render(&with_fade, 0, 6000, 127);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetFade {
                event: 1,
                fade: None,
            },
        })
        .unwrap();
    let baseline = render(&prepare(&editor, &media, &context), 0, 6000, 17);
    for (i, (a, b)) in actual.iter().zip(&baseline).enumerate() {
        let gain = if i < 2000 {
            i as f32 / 1999.0
        } else if i >= 5000 {
            (5999 - i) as f32 / 999.0
        } else {
            1.0
        };
        assert!(
            (a[0] - b[0] * gain).abs() < 1e-7 && (a[1] - b[1] * gain).abs() < 1e-7,
            "tempo fade sample {i}"
        );
    }
    assert_eq!(render(&with_fade, 3993, 2007, 31), actual[3993..]);
}

#[test]
fn selection_crossfade_pcm_is_the_two_source_mix_with_unchanged_outside_audio() {
    let (mut editor, left, media) = fixture(true);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::TrimClipEdge {
            clip: left,
            edge: hex_project::TrimEdge::Right,
            frame: 4_000,
        })
        .unwrap();
    let Created::Clip(right) = editor
        .execute(Command::CopyClipTo {
            clip: left,
            track,
            start: 4_000,
        })
        .unwrap()
    else {
        panic!("fixture copy")
    };
    editor
        .execute(Command::SetClipSourcePhase {
            clip: right,
            phase: 997,
        })
        .unwrap();
    for (clip, incoming, outgoing) in [(left, 33, 0), (right, 0, 47)] {
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: incoming,
                fade_out: outgoing,
            })
            .unwrap();
    }
    let context = AudioTransformPreparation::default();
    let original = render(&prepare(&editor, &media, &context), 0, 8_000, 257);
    let before = editor.project().snapshot();
    editor
        .execute(Command::CreateArrangementFades {
            range: hex_project::TimeRange::new(vec![track], 3_500, 4_500).unwrap(),
        })
        .unwrap();
    let prepared = prepare(&editor, &media, &context);
    let actual = render(&prepared, 0, 8_000, 257);
    assert_eq!(&actual[..3_500], &original[..3_500]);
    assert_eq!(&actual[4_500..], &original[4_500..]);
    let pcm = media.values().next().unwrap().frames();
    for frame in 3_500..4_500 {
        let progress = (frame - 3_500) as f64 / 999.0;
        let incoming_gain = progress * progress * (3.0 - 2.0 * progress);
        let outgoing = pcm[(137 + frame) % 8_000];
        let incoming = pcm[(997_i64 + frame as i64 - 4_000).rem_euclid(8_000) as usize];
        for channel in 0..2 {
            let expected = outgoing[channel] * (1.0 - incoming_gain) as f32
                + incoming[channel] * incoming_gain as f32;
            assert!(
                (actual[frame][channel] - expected).abs() < 1e-7,
                "frame={frame} channel={channel}"
            );
        }
    }
    assert_eq!(render(&prepared, 0, 8_000, 1), actual);
    assert_eq!(render(&prepared, 3_701, 701, 31), actual[3_701..4_402]);
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8_000, 17),
        original
    );
    assert!(editor.redo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 8_000, 17),
        actual
    );
}

#[test]
fn distinct_transforms_share_one_bounded_source_charge_across_edits() {
    let (mut editor, clip, media) = fixture(false);
    let context = AudioTransformPreparation {
        source_limit: 64_000,
        ..isolated(512_000, 512_000, 16)
    };
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let first = prepare(&editor, &media, &context);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 250 })
        .unwrap();
    let second = prepare(&editor, &media, &context);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let again = prepare(&editor, &media, &context);
    let source = |revision: &crate::PreparedRevision| {
        revision.arrangement().audio_clip_views()[0].audio.clone()
    };
    assert!(
        source(&first).shares_pcm_with(&source(&again)),
        "unchanged derived audio should be reused"
    );
    assert!(!source(&first).shares_pcm_with(&source(&second)));
    assert_eq!(
        source_charge(context.cache.borrow().entries.iter(), None).unwrap(),
        64_000
    );
}
