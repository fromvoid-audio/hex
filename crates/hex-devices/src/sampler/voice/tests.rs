use super::*;
use crate::sampler::{MAX_LOOP_CROSSFADE_MS, Mode, PreparedSampler, Sample, Settings};

const RATE: u32 = 8_000;

/// A ramp makes every loop seam a large, known discontinuity.
fn ramp(frames: usize) -> Vec<StereoFrame> {
    (0..frames)
        .map(|n| {
            let value = n as f32 / frames as f32;
            [value, -value]
        })
        .collect()
}

/// Region 10..300, loop 100..200 (offsets 90..190 forward).
fn settings(crossfade_ms: f64, reverse: bool) -> Settings<'static> {
    let mut settings = Settings::new(Region {
        start: 10,
        end: 300,
    });
    settings.mode = Mode::Classic {
        loop_region: Some(Region {
            start: 100,
            end: 200,
        }),
    };
    settings.reverse = reverse;
    settings.loop_crossfade_ms = crossfade_ms;
    settings
}

fn voice<'a>(pcm: &'a [StereoFrame], settings: Settings<'a>, note: u8) -> Voice<'a> {
    PreparedSampler::new(Sample::new(pcm, RATE).unwrap(), RATE, settings)
        .unwrap()
        .prepare_voice(note, 127)
        .unwrap()
        .unwrap()
}

fn max_step(voice: &Voice<'_>, frames: usize) -> f32 {
    let mut output = vec![[0.0; 2]; frames];
    voice.render(0, u64::MAX, &mut output);
    // Skip the onset: band-limited taps before the region start read silence.
    output[64..]
        .windows(2)
        .flat_map(|pair| (0..2).map(move |channel| (pair[1][channel] - pair[0][channel]).abs()))
        .fold(0.0, f32::max)
}

/// Forward source frame for a unit-rate age under the hard-seam loop law.
fn hard_source(age: u64) -> usize {
    let offset = age as usize;
    10 + if offset < 190 {
        offset
    } else {
        90 + (offset - 190) % 100
    }
}

#[test]
fn crossfade_bounds_the_loop_seam_step_in_both_directions_and_pitches() {
    let pcm = ramp(320);
    for reverse in [false, true] {
        // 48/60: linear interpolation; 67/72: band-limited (polyphase) path.
        for note in [48, 60, 67, 72] {
            let hard = voice(&pcm, settings(0.0, reverse), note);
            // 5 ms is 40 source frames at 8 kHz.
            let faded = voice(&pcm, settings(5.0, reverse), note);
            assert_eq!(faded.coefficients().loop_crossfade, 40);
            // Several passes through the seam at each pitch.
            let hard_step = max_step(&hard, 2_000);
            let faded_step = max_step(&faded, 2_000);
            // The hard seam jumps by the loop's ramp span (100/320), spread
            // over a few output frames by interpolation or the band limit.
            assert!(hard_step > 0.09, "{reverse} {note}: {hard_step}");
            // The fade spreads that span over 40 source frames, plus the ramp
            // slope itself, at up to 2x speed: about 0.022 per frame.
            assert!(faded_step < 0.025, "{reverse} {note}: {faded_step}");
            assert!(faded_step < hard_step / 3.0, "{reverse} {note}");
        }
    }
}

#[test]
fn crossfade_off_and_outside_the_fade_keeps_the_hard_seam_bit_identical() {
    let pcm = ramp(320);
    // Both reconstruction paths are untouched outside the fade.
    for note in [60, 72] {
        let hard = voice(&pcm, settings(0.0, false), note);
        let faded = voice(&pcm, settings(5.0, false), note);
        let (mut hard_out, mut faded_out) = (vec![[0.0; 2]; 1_000], vec![[0.0; 2]; 1_000]);
        hard.render(0, u64::MAX, &mut hard_out);
        faded.render(0, u64::MAX, &mut faded_out);
        let step = hard.coefficients().step as u128;
        for (age, (h, f)) in hard_out.iter().zip(&faded_out).enumerate() {
            let offset = (age as u128 * step) >> FRACTION_BITS;
            let (phase, wrapped) = if offset < 190 {
                (offset, false)
            } else {
                (90 + (offset - 190) % 100, true)
            };
            // After a wrap, band-limited taps before the loop start read the
            // faded-in pre-roll instead of the loop end for one half-span.
            let history =
                wrapped && phase < 90 + crate::sampler::interpolation::MAX_HALF_TAPS as u128;
            if phase < 150 && !(note == 72 && history) {
                assert_eq!(h.map(f32::to_bits), f.map(f32::to_bits), "{note} {age}");
            }
        }
    }
    let hard = voice(&pcm, settings(0.0, false), 60);
    assert_eq!(hard.coefficients().loop_crossfade, 0);
    let faded = voice(&pcm, settings(5.0, false), 60);
    for age in 0..1_000 {
        let source = hard_source(age);
        // Off matches the independent hard-seam law exactly.
        assert_eq!(hard.sample(age, u64::MAX), pcm[source]);
        // On is untouched outside the final 40 loop frames (source 160..200).
        if source < 160 {
            assert_eq!(
                faded.sample(age, u64::MAX).map(f32::to_bits),
                hard.sample(age, u64::MAX).map(f32::to_bits),
                "age {age}"
            );
        }
    }
}

#[test]
fn crossfade_reaches_the_post_wrap_source_and_is_limited_by_pre_roll_and_loop() {
    let pcm = ramp(320);
    let faded = voice(&pcm, settings(5.0, false), 60);
    // Half-way through the fade (source 180), tail and pre-roll mix equally.
    let middle = faded.sample(170, u64::MAX);
    assert!((middle[0] - (pcm[180][0] + pcm[80][0]) / 2.0).abs() < 1e-6);
    // The last pre-wrap frame is almost the loop start's predecessor ...
    let last = faded.sample(189, u64::MAX)[0];
    assert!((last - pcm[99][0]).abs() < 0.01, "{last}");
    // ... and the wrap lands exactly on the loop start.
    assert_eq!(faded.sample(190, u64::MAX), pcm[100]);
    // A second pass repeats the same fade.
    assert_eq!(faded.sample(270, u64::MAX), middle);

    // Forward pre-roll is the 90 frames before the loop start.
    let long = voice(&pcm, settings(1_000.0, false), 60);
    assert_eq!(long.coefficients().loop_crossfade, 90);
    // Reverse pre-roll is the region after the loop end (300 - 200).
    let long = voice(&pcm, settings(1_000.0, true), 60);
    assert_eq!(long.coefficients().loop_crossfade, 100);
    // A loop starting at the region edge has no pre-roll, so no crossfade.
    let mut edge = settings(5.0, false);
    edge.region = Region {
        start: 100,
        end: 300,
    };
    assert_eq!(voice(&pcm, edge, 60).coefficients().loop_crossfade, 0);
    // Nonlooping modes carry no crossfade.
    let mut one_shot = settings(5.0, false);
    one_shot.mode = Mode::Classic { loop_region: None };
    assert_eq!(voice(&pcm, one_shot, 60).coefficients().loop_crossfade, 0);
    // Out-of-domain authored values refuse.
    for invalid in [-1.0, f64::NAN, f64::INFINITY, MAX_LOOP_CROSSFADE_MS + 1.0] {
        let sample = Sample::new(&pcm, RATE).unwrap();
        assert!(PreparedSampler::new(sample, RATE, settings(invalid, false)).is_err());
    }
}
