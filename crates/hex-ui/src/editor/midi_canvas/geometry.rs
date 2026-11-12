//! Pure piano-roll beat, pitch and compact-label mapping.
use super::super::{midi_drag, midi_scene};
use hex_project::BeatTime;

pub(super) fn beat_at_x(
    x: f32,
    width: f32,
    view_start: i128,
    beat_span: BeatTime,
    clip_length: BeatTime,
    grid: midi_drag::MidiGrid,
) -> Option<BeatTime> {
    if !x.is_finite()
        || !width.is_finite()
        || width <= 0.0
        || x < 0.0
        || x >= width
        || beat_span.raw() == 0
    {
        return None;
    }
    let offset = (f64::from(x / width) * beat_span.raw() as f64) as u64;
    let raw = u64::try_from(view_start.checked_add(i128::from(offset))?).ok()?;
    if raw >= clip_length.raw() {
        return None;
    }
    let start = if let Some(step) = grid.step_raw() {
        let last_grid = clip_length.raw().saturating_sub(1) / step * step;
        midi_drag::creation_start(raw, grid).min(last_grid)
    } else {
        raw.min(clip_length.raw().saturating_sub(1))
    };
    let beat = BeatTime::from_raw(start);
    (beat.raw() < clip_length.raw()).then_some(beat)
}

pub(super) fn beat_label(raw: impl Into<i128>) -> String {
    let value = raw.into() as f64 / BeatTime::ONE.raw() as f64;
    let mut text = format!("{value:.4}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

pub(super) fn source_beat_label(raw: u64) -> String {
    let label = beat_label(raw);
    if label.len() <= 7 {
        label
    } else {
        format!(
            "{:.2e}",
            raw as f64 / hex_project::BeatTime::ONE.raw() as f64
        )
    }
}

pub(super) fn pitch_at_y(y: f32, height: f32, low: u8, rows: u8) -> Option<u8> {
    if !y.is_finite()
        || !height.is_finite()
        || height <= 0.0
        || y < 0.0
        || y >= height
        || !midi_scene::valid_pitch_range(low, rows)
    {
        return None;
    }
    Some(
        low.saturating_add(rows - 1)
            .saturating_sub(((y / height * f32::from(rows)).floor() as u8).min(rows - 1)),
    )
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn piano_grid_and_pitch_mapping_stay_in_clip_and_midi_bounds() {
        let clip = BeatTime::from_quarters(4).unwrap();
        assert_eq!(
            beat_at_x(
                25.0,
                100.0,
                0,
                BeatTime::from_quarters(4).unwrap(),
                clip,
                midi_drag::MidiGrid::Sixteenth,
            )
            .unwrap(),
            BeatTime::ONE
        );
        assert_eq!(
            beat_at_x(
                100.0,
                100.0,
                0,
                BeatTime::from_quarters(4).unwrap(),
                clip,
                midi_drag::MidiGrid::Sixteenth,
            ),
            None
        );
        assert_eq!(
            beat_at_x(
                f32::NAN,
                100.0,
                0,
                BeatTime::from_quarters(4).unwrap(),
                clip,
                midi_drag::MidiGrid::Sixteenth,
            ),
            None
        );
        assert_eq!(pitch_at_y(0.0, 120.0, 48, 24), Some(71));
        for (pitch, low) in [(48_u8, 48_u8), (71, 48), (104, 104), (127, 104)] {
            let row = low + 23 - pitch;
            let y = (f32::from(row) + 0.5) / 24.0 * 120.0;
            assert_eq!(pitch_at_y(y, 120.0, low, 24), Some(pitch));
        }
        assert_eq!(pitch_at_y(120.0, 120.0, 48, 24), None);
        assert_eq!(pitch_at_y(f32::INFINITY, 120.0, 48, 24), None);
        assert_eq!((i16::from(0_u8) - 1).clamp(0, 127), 0);
        assert_eq!((i16::from(127_u8) + 1).clamp(0, 127), 127);
        assert_eq!(4 * BeatTime::ONE.raw(), 4 * 4 * BeatTime::ONE.raw() / 4);
    }

    #[test]
    fn piano_click_time_uses_clip_local_selected_grid_or_exact_off_time() {
        let clip = BeatTime::from_quarters(4).unwrap();
        let span = BeatTime::from_quarters(4).unwrap();
        let x = 31.25;
        for (grid, expected) in [
            (midi_drag::MidiGrid::Quarter, BeatTime::ONE.raw()),
            (midi_drag::MidiGrid::Eighth, BeatTime::ONE.raw()),
            (
                midi_drag::MidiGrid::Sixteenth,
                BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4,
            ),
            (
                midi_drag::MidiGrid::ThirtySecond,
                BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4,
            ),
            (
                midi_drag::MidiGrid::Off,
                BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4,
            ),
        ] {
            assert_eq!(
                beat_at_x(x, 100.0, 0, span, clip, grid).unwrap().raw(),
                expected
            );
        }

        let x = 31.0;
        let free = midi_drag::effective_grid(midi_drag::MidiGrid::Sixteenth, true);
        let snapped_from_free = midi_drag::effective_grid(midi_drag::MidiGrid::Off, true);
        assert_eq!(free, midi_drag::MidiGrid::Off);
        assert_eq!(snapped_from_free, midi_drag::MidiGrid::Sixteenth);
        let free_start = beat_at_x(x, 100.0, 0, span, clip, free).unwrap().raw();
        assert!(free_start > BeatTime::ONE.raw() + BeatTime::ONE.raw() * 239 / 1000);
        assert!(free_start < BeatTime::ONE.raw() + BeatTime::ONE.raw() * 241 / 1000);
        assert_ne!(free_start, BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4);
        assert_eq!(
            beat_at_x(x, 100.0, 0, span, clip, snapped_from_free)
                .unwrap()
                .raw(),
            BeatTime::ONE.raw()
        );
    }

    #[test]
    fn compact_gate_label_retains_precision_for_subquarter_notes() {
        assert_eq!(beat_label(BeatTime::ONE.raw() / 512), "0.002");
        assert_eq!(beat_label(BeatTime::ONE.raw()), "1");
    }

    #[test]
    fn source_control_values_stay_bounded_at_the_beat_domain_limit() {
        assert_eq!(source_beat_label(4 * BeatTime::ONE.raw()), "4");
        assert!(source_beat_label(u64::MAX).len() <= 7);
    }
}
