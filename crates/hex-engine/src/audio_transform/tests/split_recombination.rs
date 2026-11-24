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
    let cut = 3_137;
    let length = editor.project().clip(clip).unwrap().duration();
    // Pitch, source gain, phase and warp stay on the continuous processing
    // root. The split creates two destination-clock fades above that signal.
    let mut neutral = Editor::new(editor.project().clone(), 1).unwrap();
    neutral
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let prepared_root = prepare(&neutral, &media, &context);
    let mut expected = render(&prepared_root, 0, length as usize, 127);
    let left = hex_project::ClipFade::new(cut, 0, 997, 0).unwrap();
    let right = hex_project::ClipFade::new(length - cut, 0, 0, 1_601).unwrap();
    for (index, frame) in expected.iter_mut().enumerate() {
        let index = index as u64;
        let gain = if index < cut {
            left.gain_at(index)
        } else {
            right.gain_at(index - cut)
        };
        *frame = frame.map(|sample| sample * gain);
    }
    let Created::Clip(tail) = editor
        .execute(Command::SplitClip { clip, frame: cut })
        .unwrap()
    else {
        panic!("split must create a right clip")
    };
    assert_eq!(editor.project().clip(clip).unwrap().retained_fade(), left);
    assert_eq!(editor.project().clip(tail).unwrap().retained_fade(), right);
    let hits_before = context.cache_hits();
    let split = prepare(&editor, &media, &context);
    for (start, count, block) in [(0, length as usize, 257), (3_117, 509, 31)] {
        let actual = render(&split, start, count, block);
        for (offset, frame) in actual.iter().enumerate() {
            let index = start as usize + offset;
            assert_eq!(
                *frame, expected[index],
                "{mode:?} frame {index}, seek {start}"
            );
        }
    }
    assert!(context.cache_hits() >= hits_before + 2);
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
