//! Translated musical copies at constant tempo share prepared roots exactly.
use super::*;
use hex_project::{AudioViewMode, AudioViewPlacement, BeatsPreserve, BeatsTail, ClipFade};

const MODES: [WarpMode; 7] = [
    WarpMode::RePitch,
    WarpMode::Beats {
        preserve: BeatsPreserve::Division(BeatTime::from_raw(BeatTime::ONE.raw() / 4)),
        tail: BeatsTail::Alternating,
        decay: 75,
    },
    WarpMode::Beats {
        preserve: BeatsPreserve::Transients,
        tail: BeatsTail::Forward,
        decay: 100,
    },
    WarpMode::Tones { grain_ms: 40 },
    WarpMode::Texture {
        grain_ms: 40,
        fluctuation: 60,
        seed: 91,
    },
    WarpMode::Complex,
    WarpMode::ComplexPro {
        formants: 75,
        envelope: 128,
    },
];

/// One beat is 8000 frames at the fixture's 16 kHz and the default 120 bpm.
const BEAT_FRAMES: u64 = 8_000;
const ALIGNED: [u64; 4] = [
    4 * BEAT_FRAMES,
    8 * BEAT_FRAMES,
    12 * BEAT_FRAMES,
    16 * BEAT_FRAMES,
];
/// Not on a Q32 frame boundary once converted to beats.
const UNALIGNED: u64 = 20 * BEAT_FRAMES + 1;

#[test]
fn musical_left_extension_renders_silence_then_retained_audio_through_history() {
    let (mut editor, clip, media) = fixture(false);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 2 * BEAT_FRAMES,
        })
        .unwrap();
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
    let context = isolated(64 * 1024 * 1024, 32 * 1024 * 1024, 64);
    let original_state = editor.project().snapshot();
    let original_media = media.values().next().unwrap().frames().to_vec();
    let original = render(
        &prepare(&editor, &media, &context),
        2 * BEAT_FRAMES,
        8_000,
        127,
    );
    assert!(
        original
            .iter()
            .any(|frame| frame[0] != 0.0 || frame[1] != 0.0)
    );

    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: BEAT_FRAMES,
        })
        .unwrap();
    let extended_state = editor.project().snapshot();
    let edited = editor.project().clip(clip).unwrap();
    assert_eq!((edited.start(), edited.duration()), (BEAT_FRAMES, 16_000));
    assert!(matches!(
        edited.audio_edit().unwrap().placement.unwrap().mode,
        AudioViewMode::OnceWithLead(_)
    ));
    let prepared = prepare(&editor, &media, &context);
    for block in [1, 127, 512] {
        let rendered = render(&prepared, BEAT_FRAMES, 16_000, block);
        assert!(rendered[..8_000].iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(&rendered[8_000..], original);
    }
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), original_state);
    assert_eq!(
        render(
            &prepare(&editor, &media, &context),
            2 * BEAT_FRAMES,
            8_000,
            127
        ),
        original
    );
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), extended_state);
    let restored = render(
        &prepare(&editor, &media, &context),
        BEAT_FRAMES,
        16_000,
        127,
    );
    assert!(restored[..8_000].iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(&restored[8_000..], original);
    assert_eq!(media.values().next().unwrap().frames(), original_media);
}

/// A warped musical clip, optionally a Repeat placement `outer` beats long,
/// pasted at aligned and unaligned onsets.
fn copies(
    mode: WarpMode,
    neutral_fades: bool,
    outer_halves: Option<u64>,
) -> (
    Editor,
    Vec<hex_project::ClipId>,
    BTreeMap<hex_project::AssetId, Audio>,
) {
    let (mut editor, clip, media) = fixture(false);
    if neutral_fades {
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 65_536,
                },
                SourceGainPoint {
                    frame: 5_000,
                    gain_q16: 24_000,
                },
            ],
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
    if let Some(halves) = outer_halves {
        let mut snapshot = editor.project().snapshot();
        let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        let edit = row.edit.as_mut().unwrap();
        let span = edit.domain.clock.length();
        edit.placement = Some(AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Beats(BeatTime::from_raw(span * halves / 2)),
            cycle_phase: 0,
        });
        row.duration = BEAT_FRAMES * halves / 2;
        row.fade = ClipFade::neutral(row.duration).unwrap();
        editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
    }
    let track = editor.project().tracks()[0].id();
    let clipboard = Arc::new(editor.project().capture_clips(&[clip]).unwrap());
    for frame in ALIGNED.into_iter().chain([UNALIGNED]) {
        editor
            .execute(Command::PasteClips {
                clipboard: Arc::clone(&clipboard),
                track,
                frame,
                insert: false,
            })
            .unwrap();
    }
    let ids = editor.project().tracks()[0]
        .clips()
        .iter()
        .map(|clip| clip.id())
        .collect();
    (editor, ids, media)
}

fn views_by_start(editor: &Editor, prepared: &crate::PreparedRevision) -> BTreeMap<u64, Audio> {
    prepared
        .arrangement()
        .audio_clip_views()
        .iter()
        .map(|view| {
            (
                editor.project().clip(view.clip).unwrap().start(),
                view.audio.clone(),
            )
        })
        .collect()
}

/// An uncached context prepares every clip at its own origin: the oracle.
fn independent(
    editor: &Editor,
    media: &BTreeMap<hex_project::AssetId, Audio>,
) -> Vec<crate::Frame> {
    render(
        &prepare(editor, media, &isolated(64 * 1024 * 1024, 0, 0)),
        0,
        (UNALIGNED + 3 * BEAT_FRAMES) as usize,
        509,
    )
}

fn translated_musical_copies_share_roots_with_exact_output_at_constant_tempo(
    mode: WarpMode,
    outer_halves: Option<u64>,
) {
    let (editor, ids, media) = copies(mode, true, outer_halves);
    assert_eq!(ids.len(), 1 + ALIGNED.len() + 1);
    let context = isolated(64 * 1024 * 1024, 32 * 1024 * 1024, 64);
    let shared = prepare(&editor, &media, &context);
    let label = format!("{mode:?} outer={outer_halves:?}");
    assert_eq!(
        render(&shared, 0, (UNALIGNED + 3 * BEAT_FRAMES) as usize, 127),
        independent(&editor, &media),
        "{label}"
    );
    let views = views_by_start(&editor, &shared);
    let origin = &views[&0];
    for start in ALIGNED {
        assert!(origin.shares_pcm_with(&views[&start]), "{label} at {start}");
    }
    assert!(!origin.shares_pcm_with(&views[&UNALIGNED]), "{label}");
}

macro_rules! translated_case {
    ($name:ident, $mode:expr, $outer:expr) => {
        #[test]
        fn $name() {
            translated_musical_copies_share_roots_with_exact_output_at_constant_tempo(
                MODES[$mode],
                $outer,
            );
        }
    };
}
translated_case!(repitch_original, 0, None);
translated_case!(repitch_repeat_one, 0, Some(2));
translated_case!(repitch_repeat_partial, 0, Some(5));
translated_case!(division_original, 1, None);
translated_case!(division_repeat_one, 1, Some(2));
translated_case!(division_repeat_partial, 1, Some(5));
translated_case!(transient_original, 2, None);
translated_case!(transient_repeat_one, 2, Some(2));
translated_case!(transient_repeat_partial, 2, Some(5));
translated_case!(tones_original, 3, None);
translated_case!(tones_repeat_one, 3, Some(2));
translated_case!(tones_repeat_partial, 3, Some(5));
translated_case!(texture_original, 4, None);
translated_case!(texture_repeat_one, 4, Some(2));
translated_case!(texture_repeat_partial, 4, Some(5));
translated_case!(complex_original, 5, None);
translated_case!(complex_repeat_one, 5, Some(2));
translated_case!(complex_repeat_partial, 5, Some(5));
translated_case!(complex_pro_original, 6, None);
translated_case!(complex_pro_repeat_one, 6, Some(2));
translated_case!(complex_pro_repeat_partial, 6, Some(5));

fn faded_musical_copies_stay_exact(mode: WarpMode) {
    // Authored fades record their absolute projection; still exact either way.
    let (editor, _, media) = copies(mode, false, None);
    let context = isolated(64 * 1024 * 1024, 32 * 1024 * 1024, 64);
    assert_eq!(
        render(
            &prepare(&editor, &media, &context),
            0,
            (UNALIGNED + 3 * BEAT_FRAMES) as usize,
            257
        ),
        independent(&editor, &media),
        "{mode:?} fades"
    );
}

fn tempo_musical_copies_stay_exact_and_unshared(mode: WarpMode) {
    // A second tempo makes every onset's projection distinct.
    let (mut editor, _, media) = copies(mode, true, Some(5));
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(BeatTime::ONE.raw() * 10),
            tempo: Tempo::new(97_000_000).unwrap(),
        })
        .unwrap();
    let context = isolated(64 * 1024 * 1024, 32 * 1024 * 1024, 64);
    let prepared = prepare(&editor, &media, &context);
    assert_eq!(
        render(&prepared, 0, (UNALIGNED + 3 * BEAT_FRAMES) as usize, 63),
        independent(&editor, &media),
        "{mode:?} tempo"
    );
    let views = views_by_start(&editor, &prepared);
    assert!(
        !views[&0].shares_pcm_with(&views[&ALIGNED[0]]),
        "{mode:?} tempo"
    );
}

#[test]
fn repitch_faded_copies_stay_exact() {
    faded_musical_copies_stay_exact(MODES[0]);
}
#[test]
fn transient_faded_copies_stay_exact() {
    faded_musical_copies_stay_exact(MODES[2]);
}
#[test]
fn texture_faded_copies_stay_exact() {
    faded_musical_copies_stay_exact(MODES[4]);
}
#[test]
fn repitch_tempo_copies_stay_exact_and_unshared() {
    tempo_musical_copies_stay_exact_and_unshared(MODES[0]);
}
#[test]
fn transient_tempo_copies_stay_exact_and_unshared() {
    tempo_musical_copies_stay_exact_and_unshared(MODES[2]);
}
#[test]
fn texture_tempo_copies_stay_exact_and_unshared() {
    tempo_musical_copies_stay_exact_and_unshared(MODES[4]);
}
