use super::*;
use crate::audio_transform::tests::{fixture, isolated};
use hex_project::{
    AudioEventEdit, AudioEventFade, BeatTime, ClipGain, Command, Created, Editor, Project, Tempo,
    WarpMode, default_audio_warp,
};
use std::collections::BTreeMap;

fn frame_fixture() -> (
    Editor,
    hex_project::ClipId,
    BTreeMap<hex_project::AssetId, crate::Audio>,
) {
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 16_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let fade = AudioEventFade::new(0, ClipFade::new(128, 0, 9, 17).unwrap()).unwrap();
    for operation in [
        AudioEventEdit::SetFade {
            event: 1,
            fade: Some(fade),
        },
        AudioEventEdit::SetGain {
            event: 1,
            gain: ClipGain::new(3_000).unwrap(),
        },
    ] {
        editor
            .execute(Command::EditAudioEvents { clip, operation })
            .unwrap();
    }
    (
        editor,
        clip,
        BTreeMap::from([(
            asset,
            crate::Audio::new(16_000, vec![[0.25, -0.125]; 128]).unwrap(),
        )]),
    )
}

#[test]
fn outer_fade_only_changes_reuse_event_pcm_even_at_retained_capacity() {
    let (editor, id, media) = frame_fixture();
    let clip = editor.project().clip(id).unwrap();
    let asset = editor.project().asset(clip.asset()).unwrap();
    let edit = clip.audio_edit().unwrap();
    let bytes = 128 * std::mem::size_of::<crate::Frame>();
    let context = isolated(4 * bytes, 4 * bytes, 8);
    let mut outer = edit.clone();
    let domain = Arc::make_mut(&mut outer.domain);
    domain.fade_clock = AudioClipFadeClock::Outer;
    domain.fade = ClipFade::new(128, 0, 21, 41).unwrap();
    let a = context
        .prepare_audio(editor.project(), &outer, asset, &media[&clip.asset()])
        .unwrap();
    let event = edit.domain.events.as_ref().unwrap().events[0];
    let gain = event.gain.linear();
    for (i, sample) in a.frames().iter().enumerate() {
        let fade = event.fade.unwrap().shape().gain_at(i as u64);
        assert_eq!(*sample, [0.25 * fade * gain, -0.125 * fade * gain]);
    }
    // An accidental cache miss would need root+event scratch and exceed this
    // retained reservation. A fade-only edit must keep the same actual PCM.
    let held = context.budget.reserve(2 * bytes).unwrap();
    let used = context.budget.used();
    // Consumed frame offset and a silent prefix are separate legal contexts.
    // The Outer query rejects a compound nonzero offset/prefix pair.
    for (offset, prefix) in [(7, 0), (0, 31)] {
        let hits = context.cache_hits();
        let domain = Arc::make_mut(&mut outer.domain);
        domain.fade = ClipFade::new(512, offset, 101, 97).unwrap();
        domain.fade_start = prefix;
        let b = context
            .prepare_audio(editor.project(), &outer, asset, &media[&clip.asset()])
            .unwrap();
        assert!(a.shares_pcm_with(&b));
        assert_eq!(context.budget.used(), used);
        assert!(context.cache_hits() > hits);
    }
    drop(held);
    let mut legacy = edit.clone();
    Arc::make_mut(&mut legacy.domain).fade = ClipFade::new(128, 0, 21, 41).unwrap();
    let legacy_audio = context
        .prepare_audio(editor.project(), &legacy, asset, &media[&clip.asset()])
        .unwrap();
    assert!(!a.shares_pcm_with(&legacy_audio));
    for (i, (unfaded, actual)) in a.frames().iter().zip(legacy_audio.frames()).enumerate() {
        assert_eq!(
            *actual,
            unfaded.map(|v| v * legacy.domain.fade.gain_at(i as u64))
        );
    }
    assert!(std::ptr::eq(
        unfaded_outer_edit(&legacy, editor.project().tempo_map())
            .unwrap()
            .as_ref(),
        &legacy
    ));
}

#[test]
fn musical_outer_authoring_metadata_does_not_change_processing_identity() {
    let (mut editor, id, media) = fixture(false);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip: id,
            track,
            start: 160_001,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip: id,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(id).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip: id,
            settings: Some(settings),
        })
        .unwrap();
    let clip = editor.project().clip(id).unwrap();
    let asset = editor.project().asset(clip.asset()).unwrap();
    let edit = clip.audio_edit().unwrap();
    assert!(
        !crate::audio_transform::translation_free_origin(
            edit.processing_tempo(editor.project().tempo_map()),
            edit.processing_origin().unwrap()
        )
        .unwrap()
    );
    let context = isolated(8 * 1024 * 1024, 8 * 1024 * 1024, 8);
    let mut outer = edit.clone();
    let domain = Arc::make_mut(&mut outer.domain);
    domain.fade_clock = AudioClipFadeClock::Outer;
    // This authored clock is fixed across the prefix edit below. It may differ
    // from source processing; neutral root identity must not retain that map.
    domain.fade_projection = Some(AudioFadeProjection {
        tempo: TempoMap::constant(16_000, Tempo::new(90_000_000).unwrap()).unwrap(),
        origin_beat_raw: i128::from(BeatTime::ONE.raw()) + 31,
        local_start_raw: 0,
    });
    let first = context
        .prepare_audio(editor.project(), &outer, asset, &media[&clip.asset()])
        .unwrap();
    let domain = Arc::make_mut(&mut outer.domain);
    domain.fade = ClipFade::new(16_000, 3, 1_000, 2_000).unwrap();
    // D uses prefix97/consumed0; keep the immutable original author origin.
    // The independent shape offset3 remains legal in this musical context.
    domain.fade_start = 97;
    assert_eq!(domain.fade_projection.as_ref().unwrap().local_start_raw, 0);
    let processing = unfaded_outer_edit(&outer, editor.project().tempo_map()).unwrap();
    assert_eq!(processing.domain.fade_start, 0);
    assert_eq!(
        processing
            .domain
            .fade_projection
            .as_ref()
            .unwrap()
            .local_start_raw,
        0
    );
    assert_eq!(
        processing
            .domain
            .fade_projection
            .as_ref()
            .unwrap()
            .origin_beat_raw,
        edit.processing_origin().unwrap()
    );
    assert_eq!(
        &processing.domain.fade_projection.as_ref().unwrap().tempo,
        edit.processing_tempo(editor.project().tempo_map())
    );
    let again = context
        .prepare_audio(editor.project(), &outer, asset, &media[&clip.asset()])
        .unwrap();
    assert!(first.shares_pcm_with(&again));
}

#[test]
// Structural preservation only: this does not admit or render an Outer group.
fn normalization_preserves_retained_child_and_source_content_identity() {
    let (editor, id, _) = frame_fixture();
    let child = editor.project().clip(id).unwrap().clone();
    let mut outer = child.audio_edit().unwrap().clone();
    let mut events = outer.domain.events.as_ref().unwrap().as_ref().clone();
    events.sources.push(hex_project::RetainedAudioRoot {
        id: 1,
        clip: child.clone(),
        tempo: Arc::new(editor.project().tempo_map().clone()),
    });
    events.events[0].source = 1;
    let domain = Arc::make_mut(&mut outer.domain);
    domain.events = Some(Arc::new(events));
    domain.fade_clock = AudioClipFadeClock::Outer;
    domain.fade = ClipFade::new(128, 0, 31, 41).unwrap();
    let processing = unfaded_outer_edit(&outer, editor.project().tempo_map()).unwrap();
    assert!(Arc::ptr_eq(
        &processing.domain.content,
        &outer.domain.content
    ));
    assert_eq!(
        processing.domain.events.as_ref().unwrap().sources[0].clip,
        child
    );
    let mut source_change = outer.clone();
    Arc::make_mut(&mut source_change.domain).pitch_cents = 100;
    assert_ne!(
        unfaded_outer_edit(&source_change, editor.project().tempo_map())
            .unwrap()
            .domain,
        processing.domain
    );
    let mut event_change = outer.clone();
    let domain = Arc::make_mut(&mut event_change.domain);
    let events = Arc::make_mut(domain.events.as_mut().unwrap());
    events.events[0].gain = ClipGain::new(-6_000).unwrap();
    assert_ne!(
        unfaded_outer_edit(&event_change, editor.project().tempo_map())
            .unwrap()
            .domain,
        processing.domain
    );
}

#[test]
fn root_normalization_does_not_admit_previously_unsupported_stream_fades() {
    for mode in [WarpMode::RePitch, WarpMode::Tones { grain_ms: 40 }] {
        let (mut editor, id, _) = fixture(false);
        editor
            .execute(Command::SetClipFades {
                clip: id,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let settings = default_audio_warp(
            editor.project().clip(id).unwrap(),
            editor.project().tempo_map(),
            mode,
        )
        .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip: id,
                settings: Some(settings),
            })
            .unwrap();
        let clip = editor.project().clip(id).unwrap();
        let asset = editor.project().asset(clip.asset()).unwrap();
        let raw = crate::StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(11, asset.id()),
            16_000,
            16_000,
            8_000,
            8_000,
            hex_project::IDENTITY_CONVERSION,
            [7; 32],
        )
        .unwrap();
        let mut outer = clip.audio_edit().unwrap().clone();
        Arc::make_mut(&mut outer.domain).fade_clock = AudioClipFadeClock::Outer;
        let supported = |edit: &AudioClipEdit| match mode {
            WarpMode::RePitch => crate::audio_transform::streamed_repitch_map(
                edit,
                editor.project().tempo_map(),
                8_000,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .is_ok(),
            _ => crate::audio_transform::streamed_granular_root(
                &raw,
                edit,
                editor.project().tempo_map(),
                &std::sync::atomic::AtomicBool::new(false),
            )
            .is_ok(),
        };
        assert!(supported(clip.audio_edit().unwrap()));
        assert!(supported(&outer));
        Arc::make_mut(&mut outer.domain).fade = ClipFade::new(8_000, 0, 101, 301).unwrap();
        assert!(!supported(&outer));
        Arc::make_mut(&mut outer.domain).fade_clock = AudioClipFadeClock::LegacyRoot;
        assert!(!supported(&outer));
    }
}

#[test]
fn prepared_frame_placement_applies_one_outer_fade_over_event_pcm() {
    use hex_project::{AudioDomainClock, AudioViewMode, AudioViewPlacement, AudioViewProjection};

    const CYCLE: u64 = 128;
    const DURATION: u64 = CYCLE * 4;
    let (editor, id, media) = frame_fixture();
    let fade = ClipFade::new(DURATION, 0, 31, 41).unwrap();
    for mode in [AudioViewMode::Repeat, AudioViewMode::Once] {
        let mut snapshot = editor.project().snapshot();
        let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        row.duration = DURATION;
        row.fade = ClipFade::neutral(DURATION).unwrap();
        let edit = row.edit.as_mut().unwrap();
        edit.placement = Some(AudioViewPlacement {
            mode,
            outer_length: AudioDomainClock::Frames(DURATION),
            cycle_phase: 0,
        });
        let project = Project::from_snapshot(snapshot).unwrap();
        let clip = project.clip(id).unwrap();
        let asset = project.asset(clip.asset()).unwrap();
        let edit = clip.audio_edit().unwrap();
        let input = &media[&clip.asset()];
        let event = edit.domain.events.as_ref().unwrap().events[0];
        let context = isolated(64 * 1024, 64 * 1024, 8);
        let prepare = |envelope| {
            let mut candidate = edit.clone();
            let domain = Arc::make_mut(&mut candidate.domain);
            domain.fade_clock = AudioClipFadeClock::Outer;
            domain.fade = envelope;
            let query = clip
                .projected_outer_fade_preview(project.tempo_map(), envelope)
                .unwrap();
            context
                .compile_view_with_outer_fade(
                    project.tempo_map(),
                    clip,
                    &candidate,
                    asset,
                    super::super::PreparedAudioSourceRef::Resident(input),
                    AudioViewProjection::new(clip, project.tempo_map()).unwrap(),
                    Some(&query),
                )
                .unwrap()
        };
        let compiled = prepare(fade);
        let changed_fade = ClipFade::new(DURATION, 0, 47, 53).unwrap();
        let changed = prepare(changed_fade);
        let prepared_audio = compiled.audio.as_ref().unwrap();
        assert!(prepared_audio.shares_pcm_with(changed.audio.as_ref().unwrap()));
        let prepared_length = if mode == AudioViewMode::Repeat {
            CYCLE
        } else {
            DURATION
        };
        assert_eq!(prepared_audio.frames().len() as u64, prepared_length);
        assert_eq!(changed.metadata.as_ref().unwrap().fade, Some(changed_fade));
        let mix = crate::Project::new(
            project.sample_rate(),
            vec![crate::Track::new(vec![compiled], 1.0, false).unwrap()],
        )
        .unwrap();
        for block in [1, 37, 256] {
            let mut renderer = crate::Renderer::new(&mix);
            renderer.play();
            let mut pcm = vec![[0.0; 2]; DURATION as usize];
            for output in pcm.chunks_mut(block) {
                renderer.render(output).unwrap();
            }
            for (elapsed, actual) in pcm.into_iter().enumerate() {
                let elapsed = elapsed as u64;
                let root = elapsed % CYCLE;
                let expected = if mode == AudioViewMode::Repeat || elapsed < CYCLE {
                    let event_fade = event.fade.unwrap().shape().gain_at(root);
                    input.frames()[root as usize].map(|sample| {
                        (sample * event_fade * event.gain.linear()) * fade.gain_at(elapsed)
                    })
                } else {
                    [0.0; 2]
                };
                assert_eq!(actual, expected, "{mode:?} frame {elapsed}, block {block}");
            }
        }
    }
}

#[test]
fn forward_frame_crop_applies_the_destination_fade_after_processing() {
    let (mut editor, id, media) = frame_fixture();
    editor
        .execute(Command::TrimClipEdge {
            clip: id,
            edge: hex_project::TrimEdge::Left,
            frame: 16,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    Arc::make_mut(&mut row.edit.as_mut().unwrap().domain).fade =
        ClipFade::new(128, 0, 41, 19).unwrap();
    let project = Project::from_snapshot(snapshot).unwrap();
    let clip = project.clip(id).unwrap();
    let edit = clip.audio_edit().unwrap();
    assert!(edit.content_view.is_none() && edit.placement.is_none());
    let source = &media[&clip.asset()];
    let asset = project.asset(clip.asset()).unwrap();
    let event = edit.domain.events.as_ref().unwrap().events[0];
    let fade = ClipFade::new(clip.duration(), 0, 41, 19).unwrap();
    let context = isolated(64 * 1024, 64 * 1024, 8);
    let prepare = |fade| {
        let mut prospective = edit.clone();
        let domain = Arc::make_mut(&mut prospective.domain);
        domain.fade_clock = AudioClipFadeClock::Outer;
        domain.fade = fade;
        context
            .compile_captured_clip_at_tempo(
                project.tempo_map(),
                clip,
                &prospective,
                asset,
                super::super::PreparedAudioSourceRef::Resident(source),
                Some(super::super::OuterPlaybackFade::Frame(fade)),
            )
            .unwrap()
            .unwrap()
    };
    let compiled = prepare(fade);
    let changed = prepare(ClipFade::new(clip.duration(), 0, 13, 37).unwrap());
    assert!(
        compiled
            .audio
            .as_ref()
            .unwrap()
            .shares_pcm_with(changed.audio.as_ref().unwrap())
    );
    let mix = crate::Project::new(
        project.sample_rate(),
        vec![crate::Track::new(vec![compiled], 1.0, false).unwrap()],
    )
    .unwrap();
    let mut renderer = crate::Renderer::new(&mix);
    renderer.seek(clip.start());
    renderer.play();
    let mut pcm = vec![[0.0; 2]; clip.duration() as usize];
    for block in pcm.chunks_mut(37) {
        renderer.render(block).unwrap();
    }
    for (index, sample) in pcm.into_iter().enumerate() {
        let root_frame = edit.view_start + index as u64;
        let event_fade = event.fade.unwrap().shape().gain_at(root_frame);
        let expected = source.frames()[root_frame as usize]
            .map(|sample| (sample * event_fade * event.gain.linear()) * fade.gain_at(index as u64));
        assert_eq!(sample, expected, "frame {index}");
    }
}

#[test]
fn verified_frame_pitch_and_reverse_share_stream_identity_with_outer_fades() {
    use crate::{StreamSourceKey, StreamedSourceMetadata, compile::PreparedAudioSourceRef};

    for reversed in [false, true] {
        let (mut editor, id, _) = fixture(false);
        editor
            .execute(Command::SetClipFades {
                clip: id,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(if reversed {
                Command::ReverseClips(vec![id])
            } else {
                Command::SetAudioClipPitch {
                    clip: id,
                    cents: 100,
                }
            })
            .unwrap();
        let project = editor.project();
        let clip = project.clip(id).unwrap();
        let asset = project.asset(clip.asset()).unwrap();
        let edit = clip.audio_edit().unwrap();
        let raw = Arc::new(
            StreamedSourceMetadata::new(
                StreamSourceKey::new(11, asset.id()),
                project.sample_rate(),
                asset.source_sample_rate(),
                asset.source_frames(),
                asset.playback_frames(),
                asset.conversion_version(),
                [7; 32],
            )
            .unwrap(),
        );
        let derived = if reversed {
            Arc::clone(&raw)
        } else {
            Arc::new(
                raw.with_plain_pitch(edit.domain.pitch_cents, edit.domain.warp_resampler)
                    .unwrap(),
            )
        };
        let pitched = BTreeMap::from([(derived.key(), Arc::clone(&derived))]);
        let source = || {
            if reversed {
                PreparedAudioSourceRef::Streamed(&raw)
            } else {
                PreparedAudioSourceRef::StreamedWithPitch(&raw, &pitched)
            }
        };
        let context = super::super::AudioTransformPreparation::default();
        for (fade_in, fade_out) in [(151, 513), (334, 712)] {
            let mut prospective = edit.clone();
            let domain = Arc::make_mut(&mut prospective.domain);
            domain.fade_clock = AudioClipFadeClock::Outer;
            domain.fade = ClipFade::new(clip.duration(), 0, fade_in, fade_out).unwrap();
            let fade = domain.fade;
            let compiled = context
                .compile_captured_clip_at_tempo(
                    project.tempo_map(),
                    clip,
                    &prospective,
                    asset,
                    source(),
                    Some(super::super::OuterPlaybackFade::Frame(fade)),
                )
                .unwrap()
                .unwrap();
            let metadata = compiled.metadata.unwrap();
            assert!(compiled.audio.is_none());
            assert_eq!(metadata.reversed, reversed);
            assert_eq!(metadata.fade, Some(fade));
            assert!(Arc::ptr_eq(metadata.streamed.as_ref().unwrap(), &derived));
        }
    }
}
