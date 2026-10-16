use super::*;
use hex_project::{
    BeatTime, Command, Created, Editor, Project, SourceGainPoint, Tempo, WarpMode,
    default_audio_warp,
};
use std::collections::BTreeMap;

fn fixture(
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
fn prepare(
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
fn render(
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
fn isolated(limit: usize, cache: usize, entries: usize) -> AudioTransformPreparation {
    AudioTransformPreparation {
        budget: Arc::new(budget::Budget::new(limit)),
        cache_limit: cache,
        entry_limit: entries,
        ..AudioTransformPreparation::default()
    }
}
#[test]
fn every_mode_split_recombines_exactly_with_root_pitch_phase_envelope_fades_and_tempo() {
    for mode in [
        None,
        Some(WarpMode::RePitch),
        Some(WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Division(BeatTime::from_raw(
                BeatTime::ONE.raw() / 4,
            )),
            tail: BeatsTail::Alternating,
            decay: 75,
        }),
        Some(WarpMode::Tones { grain_ms: 40 }),
        Some(WarpMode::Texture {
            grain_ms: 40,
            fluctuation: 60,
            seed: 91,
        }),
        Some(WarpMode::Complex),
        Some(WarpMode::ComplexPro {
            formants: 75,
            envelope: 128,
        }),
    ] {
        let (mut editor, clip, media) = fixture(true);
        let context = isolated(8 * 1024 * 1024, 4 * 1024 * 1024, 8);
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 137 })
            .unwrap();
        editor
            .execute(Command::SetAudioClipSourceGain {
                clip,
                points: vec![
                    SourceGainPoint {
                        frame: 0,
                        gain_q16: 65_536,
                    },
                    SourceGainPoint {
                        frame: 4_000,
                        gain_q16: 16_384,
                    },
                    SourceGainPoint {
                        frame: 8_000,
                        gain_q16: 65_536,
                    },
                ],
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
            editor
                .execute(Command::SetTempoPoint {
                    beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                    tempo: Tempo::new(90_000_000).unwrap(),
                })
                .unwrap();
        }
        let length = editor.project().clip(clip).unwrap().duration() as usize;
        let unsplit = prepare(&editor, &media, &context);
        let expected = render(&unsplit, 0, length, 127);
        editor
            .execute(Command::SplitClip { clip, frame: 3_137 })
            .unwrap();
        let split = prepare(&editor, &media, &context);
        assert_eq!(render(&split, 0, length, 257), expected, "{mode:?}");
        assert_eq!(
            render(&split, 3_117, 509, 31),
            expected[3_117..3_626],
            "random seek {mode:?}"
        );
        assert!(context.cache_hits() >= 2);
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
    assert_eq!(render(&moved, 0, 400, 127), reference);
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
        assert_eq!(cache.source_bytes, 64_000);
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
                assert_eq!(
                    entries
                        .iter()
                        .map(|entry| entry.source_bytes)
                        .sum::<usize>(),
                    64_000
                );
                assert_eq!(context.cache.borrow().source_bytes, 0);
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
