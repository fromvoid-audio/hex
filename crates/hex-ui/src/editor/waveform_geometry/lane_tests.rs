use super::*;
use gpui::size;

fn geometry(source: &CachedWaveform, height: f32, binned: bool) -> WaveformGeometry {
    let bounds = Bounds::new(point(px(7.0), px(9.0)), size(px(100.0), px(height)));
    prepare_waveform_geometry(
        source,
        &WaveformMapping::Source { timeline_origin: 0 },
        0,
        source.source_frames() as u64,
        bounds,
        2,
        Some(WaveformGrid {
            start: 0,
            end: source.source_frames() as u64,
            left: 7.0,
            width: 100.0,
            columns: 2,
            binned,
        }),
    )
    .unwrap()
}

#[test]
fn short_rows_union_independent_channels_and_tall_rows_keep_stereo() {
    let source = CachedWaveform::from_peaks(
        2,
        vec![
            [
                CachedPeak {
                    min: -0.2,
                    max: 0.75
                },
                CachedPeak {
                    min: -0.9,
                    max: 0.15
                },
            ];
            2
        ],
    )
    .unwrap();
    assert!(!source.has_identical_channels());
    for binned in [false, true] {
        for (height, channels) in [(31.0, 1), (32.0, 2), (80.0, 2)] {
            let geometry = geometry(&source, height, binned);
            assert_eq!(geometry.display.channels().len(), channels);
            assert_eq!(geometry.channels().len(), channels);
            if channels == 1 {
                let outline = &geometry.channels()[0].outline;
                let upper = outline
                    .iter()
                    .map(|p| f32::from(p.y))
                    .reduce(f32::min)
                    .unwrap();
                let lower = outline
                    .iter()
                    .map(|p| f32::from(p.y))
                    .reduce(f32::max)
                    .unwrap();
                assert!((upper - (9.0 + height * 0.5 - 0.75 * height * 0.44)).abs() < 0.0001);
                assert!((lower - (9.0 + height * 0.5 + 0.9 * height * 0.44)).abs() < 0.0001);
            }
        }
    }
}

#[test]
fn known_duplicated_samples_use_one_full_height_band_at_every_height() {
    let audio = hex_engine::Audio::new(48_000, vec![[0.5; 2], [-0.25; 2]]).unwrap();
    let source = hex_media::waveform::summarize(&audio, 2).unwrap();
    assert!(source.has_identical_channels());
    for height in [12.0, 31.0, 32.0, 80.0] {
        let geometry = geometry(&source, height, false);
        assert_eq!(geometry.display.channels().len(), 1);
        assert_eq!(geometry.channels().len(), 1);
        let outline = &geometry.channels()[0].outline;
        let top = outline
            .iter()
            .map(|p| f32::from(p.y))
            .reduce(f32::min)
            .unwrap();
        let bottom = outline
            .iter()
            .map(|p| f32::from(p.y))
            .reduce(f32::max)
            .unwrap();
        assert!((top - (9.0 + height * 0.5 - 0.5 * height * 0.44)).abs() < 0.0001);
        assert!((bottom - (9.0 + height * 0.5 + 0.25 * height * 0.44)).abs() < 0.0001);
    }
}

#[test]
fn short_rows_preserve_opposite_phase_peaks_instead_of_summing_to_silence() {
    let audio = hex_engine::Audio::new(48_000, vec![[-0.75, 0.75], [0.75, -0.75]]).unwrap();
    let source = hex_media::waveform::summarize(&audio, 1).unwrap();
    assert!(!source.has_identical_channels());
    for binned in [false, true] {
        let height = 28.0;
        let geometry = geometry(&source, height, binned);
        assert_eq!(geometry.display.channels().len(), 1);
        let outline = &geometry.channels()[0].outline;
        let top = outline
            .iter()
            .map(|p| f32::from(p.y))
            .reduce(f32::min)
            .unwrap();
        let bottom = outline
            .iter()
            .map(|p| f32::from(p.y))
            .reduce(f32::max)
            .unwrap();
        assert!((bottom - top - 1.5 * height * 0.44).abs() < 0.0001);
    }
}
