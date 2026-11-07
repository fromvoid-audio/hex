//! Static, explicitly labeled spectrum data for the audio-free EQ fixture.
//! The graph consumes the same normalized output, input and peak layers as the
//! production response graph; these values do not represent an audio signal.

type Curve = Vec<[f32; 2]>;

pub(super) fn layers(spectrum: bool, input: bool) -> (Option<Curve>, Option<Curve>, Option<Curve>) {
    if !spectrum {
        return (None, None, None);
    }
    let output = vec![
        [0.0, 0.16],
        [0.1, 0.35],
        [0.2, 0.51],
        [0.3, 0.43],
        [0.4, 0.58],
        [0.5, 0.38],
        [0.6, 0.46],
        [0.7, 0.31],
        [0.8, 0.24],
        [0.9, 0.13],
        [1.0, 0.04],
    ];
    let peak = output
        .iter()
        .map(|[x, y]| [*x, (*y + 0.1_f32).min(1.0_f32)])
        .collect();
    let input = input.then(|| {
        vec![
            [0.0, 0.22],
            [0.1, 0.42],
            [0.2, 0.47],
            [0.3, 0.49],
            [0.4, 0.41],
            [0.5, 0.45],
            [0.6, 0.34],
            [0.7, 0.37],
            [0.8, 0.2],
            [0.9, 0.17],
            [1.0, 0.07],
        ]
    });
    (Some(output), input, Some(peak))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_layers_follow_both_visibility_controls() {
        assert_eq!(layers(false, false), (None, None, None));
        assert_eq!(layers(false, true), (None, None, None));
        let (output, input, peak) = layers(true, false);
        assert!(output.is_some() && peak.is_some() && input.is_none());
        let (output, input, peak) = layers(true, true);
        let (output, input, peak) = (output.unwrap(), input.unwrap(), peak.unwrap());
        for curve in [&output, &input, &peak] {
            assert!(curve.windows(2).all(|pair| pair[0][0] < pair[1][0]));
            assert!(
                curve
                    .iter()
                    .flatten()
                    .all(|value| (0.0..=1.0).contains(value))
            );
        }
        assert!(
            output
                .iter()
                .zip(&peak)
                .all(|(level, held)| level[0] == held[0] && level[1] <= held[1])
        );
    }
}
