//! Directed content views read fractional base positions with their own
//! versioned law: migrated musical views keep frozen v1 PCM, new views use v2.
use super::*;
use hex_project::WarpResampler;

const FRAMES: usize = 30_000;
/// Reversed output beats 0..1/2 (6000 frames at 240 bpm) read base beats
/// 1/2..1 (24000 frames at 60 bpm): a rate-4 compression.
const COMPRESSED: std::ops::Range<usize> = 500..5_500;

#[derive(Clone, Copy)]
enum Signal {
    Nyquist,
    Tone(f32),
}

fn rate4_reversed(signal: Signal) -> (Editor, BTreeMap<hex_project::AssetId, Audio>) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "rate witness".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    for (beat, bpm) in [
        (BeatTime::ZERO, 240),
        (BeatTime::from_ratio(1, 2).unwrap(), 60),
    ] {
        editor
            .execute(Command::SetTempoPoint {
                beat,
                tempo: Tempo::new(bpm * 1_000_000).unwrap(),
            })
            .unwrap();
    }
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    assert_eq!(
        editor.project().clip(clip).unwrap().duration(),
        FRAMES as u64
    );
    let frames = (0..FRAMES)
        .map(|i| {
            let x = match signal {
                Signal::Nyquist if i % 2 == 0 => 1.0,
                Signal::Nyquist => -1.0,
                Signal::Tone(hz) => 0.25 * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin(),
            };
            [x, x]
        })
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(48_000, frames).unwrap())]);
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    (editor, media)
}

/// The same project with its directed view read by `law`.
fn with_read_law(editor: &Editor, law: WarpResampler) -> Editor {
    let mut snapshot = editor.project().snapshot();
    let hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } =
        &mut snapshot.tracks[0].content
    else {
        panic!("hybrid track")
    };
    let edit = audio_clips[0].edit.as_mut().unwrap();
    edit.content_view.as_mut().unwrap().read_resampler = law;
    Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap()
}

fn rms(frames: &[crate::Frame]) -> f64 {
    (frames.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>() / frames.len() as f64).sqrt()
}

/// FNV-1a over every rendered sample's bits.
fn pcm_hash(frames: &[crate::Frame]) -> u64 {
    frames
        .iter()
        .flatten()
        .flat_map(|s| s.to_bits().to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
}

#[test]
fn migrated_musical_view_keeps_frozen_v1_pcm_and_new_views_read_with_v2() {
    // Hashes of the same renders recorded before views were versioned
    // (4ce0ecfb0, frozen v1 for every directed view).
    for (signal, recorded) in [
        (Signal::Nyquist, RECORDED_V1_NYQUIST),
        (Signal::Tone(9_000.0), RECORDED_V1_TONE_9K),
    ] {
        let (editor, media) = rate4_reversed(signal);
        let view = editor.project().tracks()[0].clips()[0]
            .audio_edit()
            .unwrap()
            .content_view
            .unwrap();
        assert_eq!(view.read_resampler, WarpResampler::V2, "new views use v2");
        let context = AudioTransformPreparation::default();
        let v1 = render(
            &prepare(&with_read_law(&editor, WarpResampler::V1), &media, &context),
            0,
            FRAMES,
            257,
        );
        assert_eq!(pcm_hash(&v1), recorded, "migrated v1 view is bit-identical");
    }
}

#[test]
fn rate4_directed_compression_aliasing_is_lower_with_v2() {
    // A Nyquist square and a 9 kHz tone compressed 4x both lie entirely above
    // the output Nyquist; any residual is aliasing. A 1 kHz tone is passband.
    for signal in [
        Signal::Nyquist,
        Signal::Tone(9_000.0),
        Signal::Tone(6_600.0),
        Signal::Tone(1_000.0),
    ] {
        let (editor, media) = rate4_reversed(signal);
        let measure = |law| {
            let pcm = render(
                &prepare(
                    &with_read_law(&editor, law),
                    &media,
                    &AudioTransformPreparation::default(),
                ),
                0,
                FRAMES,
                257,
            );
            rms(&pcm[COMPRESSED])
        };
        let (v1, v2) = (measure(WarpResampler::V1), measure(WarpResampler::V2));
        eprintln!("rate-4 directed residual rms: v1 {v1:.6}, v2 {v2:.6}");
        match signal {
            Signal::Tone(hz) if hz < 6_000.0 => {
                assert!((0.15..0.20).contains(&v2), "v2 passband rms {v2}");
                assert!((0.15..0.20).contains(&v1), "v1 passband rms {v1}");
            }
            _ => {
                assert!(v2 < v1, "v2 aliases less: v1 {v1}, v2 {v2}");
                assert!(v2 < 0.001, "v2 stopband residual {v2}");
            }
        }
    }
}

#[test]
fn directed_read_law_selects_a_separate_cache_entry() {
    let (editor, media) = rate4_reversed(Signal::Tone(9_000.0));
    let shared = isolated(64_000_000, 32_000_000, 16);
    let pcm = |law, context: &AudioTransformPreparation| {
        render(
            &prepare(&with_read_law(&editor, law), &media, context),
            0,
            FRAMES,
            257,
        )
    };
    let v1 = pcm(WarpResampler::V1, &shared);
    let v2 = pcm(WarpResampler::V2, &shared);
    assert_ne!(v1, v2, "a cached v1 view never serves the v2 law");
    assert_eq!(pcm(WarpResampler::V1, &shared), v1);
    let fresh = isolated(64_000_000, 32_000_000, 16);
    assert_eq!(pcm(WarpResampler::V2, &fresh), v2);
    assert_eq!(pcm(WarpResampler::V1, &fresh), v1);
}

const RECORDED_V1_NYQUIST: u64 = 0x0efe_9482_dae6_d1a1;
const RECORDED_V1_TONE_9K: u64 = 0x8e74_9296_1d3b_38bd;
