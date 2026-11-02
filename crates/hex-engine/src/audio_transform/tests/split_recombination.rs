use super::*;

fn split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(mode: Option<WarpMode>) {
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

#[test]
fn split_recombines_unwarped() {
    split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(None);
}
#[test]
fn split_recombines_repitch() {
    split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(Some(WarpMode::RePitch));
}
#[test]
fn split_recombines_beats_division() {
    split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(Some(WarpMode::Beats {
        preserve: hex_project::BeatsPreserve::Division(BeatTime::from_raw(BeatTime::ONE.raw() / 4)),
        tail: BeatsTail::Alternating,
        decay: 75,
    }));
}
#[test]
fn split_recombines_beats_transients() {
    split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(Some(WarpMode::Beats {
        preserve: hex_project::BeatsPreserve::Transients,
        tail: BeatsTail::Forward,
        decay: 100,
    }));
}
#[test]
fn split_recombines_tones() {
    split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(Some(WarpMode::Tones {
        grain_ms: 40,
    }));
}
#[test]
fn split_recombines_texture() {
    split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(Some(WarpMode::Texture {
        grain_ms: 40,
        fluctuation: 60,
        seed: 91,
    }));
}
#[test]
fn split_recombines_complex() {
    split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(Some(WarpMode::Complex));
}
#[test]
fn split_recombines_complex_pro() {
    split_recombines_with_root_pitch_phase_envelope_fades_and_tempo(Some(WarpMode::ComplexPro {
        formants: 75,
        envelope: 128,
    }));
}
