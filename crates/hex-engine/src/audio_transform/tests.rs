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
        Some(WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
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
