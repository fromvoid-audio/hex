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
