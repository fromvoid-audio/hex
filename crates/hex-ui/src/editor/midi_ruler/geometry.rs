//! Pure piano-roll ruler handle, loop-grid clamp, and gesture geometry.
use super::super::{
    midi_drag::{self, MidiGrid},
    midi_source_controls::SourceValues,
    midi_stroke::CanvasBounds,
};
use hex_project::ClipId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Context {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) clip: ClipId,
    pub(super) values: SourceValues,
    // A forward one-shot Arrangement region begins at its start marker, so
    // the marker stays inside song time and before the region end.
    pub(super) play_start_region: bool,
    pub(super) start: i128,
    pub(super) span: u64,
    pub(super) grid: MidiGrid,
    pub(super) bounds: CanvasBounds,
}
impl Context {
    pub(super) fn exclusive_end(self) -> u64 {
        if self.values.looping {
            self.values.loop_end
        } else {
            self.values.source_length
        }
    }
    pub(super) fn valid(self) -> bool {
        self.bounds.is_valid()
            && self.span > 0
            && self.start == super::super::midi_scene::clamp_start(self.start, self.span)
            && self.start.checked_add(i128::from(self.span)).is_some()
            && self.values.source_length > 0
            && self.values.loop_start < self.values.loop_end
            && self.values.loop_end <= self.values.source_length
            && self.values.source_start < self.exclusive_end()
    }
    pub(super) fn x(self, beat: u64) -> Option<f32> {
        let offset = i128::from(beat).checked_sub(self.start)?;
        (offset >= 0 && offset <= i128::from(self.span))
            .then(|| self.bounds.x + (offset as f64 / self.span as f64) as f32 * self.bounds.width)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handle {
    Start,
    LoopStart,
    LoopEnd,
    LoopBody,
}
impl Handle {
    pub(super) fn held_cursor(self) -> gpui::CursorStyle {
        match self {
            Self::Start => gpui::CursorStyle::ResizeLeftRight,
            Self::LoopStart => gpui::CursorStyle::ResizeLeft,
            Self::LoopEnd => gpui::CursorStyle::ResizeRight,
            Self::LoopBody => gpui::CursorStyle::ClosedHand,
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct Gesture {
    pub(super) context: Context,
    pub(super) down_x: f32,
    pub(super) handle: Handle,
    pub(super) preview: SourceValues,
}
// Bounds are inclusive and ordered by the validated source context. Restrict
// snapping to legal grid points, rather than snapping then landing off-grid at a
// raw boundary. A sub-grid interval with no grid point keeps the legal raw clamp.
pub(super) fn clamp_loop_grid(
    raw: i128,
    first: u64,
    last: u64,
    grid: MidiGrid,
    origin: u64,
) -> u64 {
    if let Some(step) = grid.step_raw()
        && let (Some(snapped_first), Some(snapped_last)) = (
            midi_drag::first_grid_at_or_after(first, step, origin),
            midi_drag::last_grid_at_or_before(last, step, origin),
        )
        && snapped_first <= snapped_last
    {
        return raw.clamp(i128::from(snapped_first), i128::from(snapped_last)) as u64;
    }
    raw.clamp(i128::from(first), i128::from(last)) as u64
}
impl Gesture {
    pub(super) fn update(&mut self, x: f32, invert_snap: bool) -> bool {
        if !x.is_finite() {
            return false;
        }
        let dx = f64::from(x) - f64::from(self.down_x);
        let original = self.context.values;
        let mut next = original;
        // A stationary click on an off-grid marker remains a true no-op.
        if dx.abs() >= 3.0 {
            let ticks = (dx / f64::from(self.context.bounds.width) * self.context.span as f64)
                .round()
                .clamp(-(u64::MAX as f64), u64::MAX as f64) as i128;
            let grid = midi_drag::effective_grid(self.context.grid, invert_snap);
            let origin = original.source_start;
            let moved = |start: u64| {
                let raw = i128::from(start) + ticks;
                midi_drag::snap_signed_aligned(raw, grid, origin)
            };
            match self.handle {
                Handle::Start => {
                    let mut first = 0;
                    let mut last = self.context.exclusive_end() - 1;
                    if self.context.play_start_region {
                        first = original
                            .source_start
                            .saturating_sub(original.arrangement_start);
                        last = last.min(
                            original
                                .source_start
                                .saturating_add(original.arrangement_length)
                                .saturating_sub(1),
                        );
                    }
                    let last = grid
                        .step_raw()
                        .and_then(|step| midi_drag::last_grid_at_or_before(last, step, origin))
                        .filter(|&grid_last| grid_last >= first)
                        .unwrap_or(last);
                    next.source_start = moved(original.source_start)
                        .clamp(i128::from(first), i128::from(last))
                        as u64;
                }
                Handle::LoopStart => {
                    next.loop_start = clamp_loop_grid(
                        moved(original.loop_start),
                        0,
                        original.loop_end - 1,
                        grid,
                        origin,
                    )
                }
                Handle::LoopEnd => {
                    let first = if original.looping {
                        original.loop_start.max(original.source_start) + 1
                    } else {
                        original.loop_start + 1
                    };
                    next.loop_end = clamp_loop_grid(
                        moved(original.loop_end),
                        first,
                        original.source_length,
                        grid,
                        origin,
                    );
                }
                Handle::LoopBody => {
                    let length = original.loop_end - original.loop_start;
                    let first = if original.looping {
                        original.source_start.saturating_sub(length - 1)
                    } else {
                        0
                    };
                    next.loop_start = clamp_loop_grid(
                        moved(original.loop_start),
                        first,
                        original.source_length - length,
                        grid,
                        origin,
                    );
                    next.loop_end = next.loop_start + length;
                }
            }
        }
        let changed = next != self.preview;
        self.preview = next;
        changed
    }
}

#[cfg(test)]
#[allow(dead_code)] // Used by midi_loop lib tests and ui-unit-tier geometry tests.
pub(super) fn test_context() -> Context {
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "marker".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    Context {
        generation: 0,
        revision: 0,
        clip,
        values: SourceValues::from_source(
            editor.project().midi_source(clip).unwrap(),
            0,
            4 * BeatTime::ONE.raw(),
        ),
        play_start_region: false,
        start: 0,
        span: 4 * BeatTime::ONE.raw(),
        grid: MidiGrid::Sixteenth,
        bounds: CanvasBounds {
            x: 100.0,
            y: 20.0,
            width: 400.0,
            height: 16.0,
        },
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod geometry_tests {
    use super::*;
    use hex_project::BeatTime;
    fn context() -> Context {
        test_context()
    }
    #[test]
    fn marker_snaps_to_half_open_bounds_with_intro_before_or_start_after_loop_start() {
        let beat = BeatTime::ONE.raw();
        let base = context();
        for looping in [false, true] {
            let c = Context {
                values: SourceValues {
                    loop_start: beat,
                    loop_end: 2 * beat,
                    looping,
                    ..base.values
                },
                ..base
            };
            assert!(c.valid()); // Intro begins before loop_start.
            let mut g = Gesture {
                context: c,
                down_x: 100.0,
                handle: Handle::Start,
                preview: c.values,
            };
            assert!(g.update(250.0, false));
            assert_eq!(g.preview.source_start, beat * 3 / 2); // Also after loop_start.
            assert!(g.update(f32::MAX, false));
            assert_eq!(g.preview.source_start, c.exclusive_end() - beat / 4);
            assert!(g.update(f32::MAX, true));
            assert_eq!(g.preview.source_start, c.exclusive_end() - 1);
            assert!(g.update(-f32::MAX, false));
            assert_eq!(g.preview.source_start, 0);
            assert!(!g.update(f32::NAN, false));
            assert_eq!(g.preview.source_start, 0);
        }
        let c = Context {
            values: SourceValues {
                source_start: beat / 3,
                ..base.values
            },
            ..base
        };
        let mut g = Gesture {
            context: c,
            down_x: 100.0,
            handle: Handle::Start,
            preview: c.values,
        };
        assert!(!g.update(100.0, false));
        assert_eq!(g.preview.source_start, beat / 3);
    }
    #[test]
    fn invalid_or_overflowing_ruler_contexts_are_refused_without_source_growth() {
        let c = context();
        for invalid in [
            Context { span: 0, ..c },
            Context {
                start: i128::from(u64::MAX),
                span: 1,
                ..c
            },
            Context {
                bounds: CanvasBounds {
                    width: f32::NAN,
                    ..c.bounds
                },
                ..c
            },
            Context {
                values: SourceValues {
                    source_start: c.values.source_length,
                    ..c.values
                },
                ..c
            },
            Context {
                values: SourceValues {
                    looping: true,
                    source_start: c.values.loop_end,
                    ..c.values
                },
                ..c
            },
            Context {
                values: SourceValues {
                    loop_start: c.values.loop_end,
                    ..c.values
                },
                ..c
            },
        ] {
            assert!(!invalid.valid());
        }
        let mut g = Gesture {
            context: c,
            down_x: 100.0,
            handle: Handle::Start,
            preview: c.values,
        };
        g.update(f32::MAX, false);
        assert_eq!(g.context.values, c.values);
        assert!(g.preview.source_start < c.values.source_length);
    }
    #[test]
    fn loop_edges_preserve_opposite_anchor_and_body_preserves_length_inside_source() {
        let beat = hex_project::BeatTime::ONE.raw();
        let base = context();
        let c = Context {
            values: SourceValues {
                loop_start: beat,
                loop_end: 3 * beat,
                ..base.values
            },
            ..base
        };
        for (handle, down, x, first, last) in [
            (Handle::LoopStart, 200.0, 300.0, 2 * beat, 3 * beat),
            (Handle::LoopEnd, 400.0, 300.0, beat, 2 * beat),
            (Handle::LoopBody, 300.0, 400.0, 2 * beat, 4 * beat),
            (Handle::LoopBody, 300.0, -f32::MAX, 0, 2 * beat),
        ] {
            let mut g = Gesture {
                context: c,
                down_x: down,
                handle,
                preview: c.values,
            };
            assert!(g.update(x, false));
            assert_eq!((g.preview.loop_start, g.preview.loop_end), (first, last));
            assert_eq!(
                g.preview,
                SourceValues {
                    loop_start: first,
                    loop_end: last,
                    ..c.values
                }
            );
            assert!(!g.update(x, false));
        }
    }

    #[test]
    fn loop_grid_limits_remain_snapped_or_use_a_legal_short_interval_fallback() {
        let beat = hex_project::BeatTime::ONE.raw();
        let base = context();
        let mut leading = Gesture {
            context: base,
            down_x: 100.0,
            handle: Handle::LoopStart,
            preview: base.values,
        };
        leading.update(f32::MAX, false);
        assert_eq!(leading.preview.loop_start, 4 * beat - beat / 4);
        leading.update(f32::MAX, true);
        assert_eq!(leading.preview.loop_start, 4 * beat - 1);
        let short = Context {
            values: SourceValues {
                source_start: beat + 3,
                source_length: beat + 5,
                loop_start: beat + 1,
                loop_end: beat + 4,
                looping: true,
                ..base.values
            },
            ..base
        };
        assert!(short.valid());
        for (handle, x, first, last) in [
            (Handle::LoopEnd, -f32::MAX, beat + 1, beat + 4),
            (Handle::LoopEnd, f32::MAX, beat + 1, beat + 5),
            (Handle::LoopBody, -f32::MAX, beat + 1, beat + 4),
            (Handle::LoopBody, f32::MAX, beat + 2, beat + 5),
        ] {
            let mut g = Gesture {
                context: short,
                down_x: 200.0,
                handle,
                preview: short.values,
            };
            g.update(x, false);
            assert_eq!((g.preview.loop_start, g.preview.loop_end), (first, last));
            assert!(g.preview.source_start < g.preview.loop_end);
            assert!(g.preview.loop_end <= g.preview.source_length);
            let preview = g.preview;
            assert!(!g.update(f32::INFINITY, false));
            assert_eq!(g.preview, preview);
        }
        let step = MidiGrid::Sixteenth.step_raw().unwrap();
        let last_grid = u64::MAX / step * step;
        assert_eq!(
            clamp_loop_grid(i128::from(u64::MAX), 1, u64::MAX, MidiGrid::Sixteenth, 0),
            last_grid
        );
        assert_eq!(
            clamp_loop_grid(0, u64::MAX - 1, u64::MAX, MidiGrid::Sixteenth, 0),
            u64::MAX - 1
        );
        assert_eq!(
            clamp_loop_grid(0, beat + 1, beat + 3, MidiGrid::Off, 0),
            beat + 1
        );
        let mut off = Gesture {
            context: Context {
                grid: MidiGrid::Off,
                ..base
            },
            down_x: 100.0,
            handle: Handle::LoopStart,
            preview: base.values,
        };
        off.update(f32::MAX, false);
        assert_eq!(off.preview.loop_start, 4 * beat - 1);
        off.update(f32::MAX, true);
        assert_eq!(off.preview.loop_start, 4 * beat - beat / 4);
        let preview = off.preview;
        off.update(102.0, true);
        assert_eq!(off.preview, base.values); // The original three-pixel no-op threshold.
        assert_ne!(off.preview, preview);
    }
    #[test]
    fn loop_source_start_constraint_depends_on_enable_and_snap_override_is_exact() {
        let beat = hex_project::BeatTime::ONE.raw();
        let base = context();
        for looping in [false, true] {
            let c = Context {
                values: SourceValues {
                    source_start: beat * 5 / 2,
                    loop_start: beat,
                    loop_end: 3 * beat,
                    looping,
                    ..base.values
                },
                ..base
            };
            let mut g = Gesture {
                context: c,
                down_x: 400.0,
                handle: Handle::LoopEnd,
                preview: c.values,
            };
            g.update(-f32::MAX, false);
            assert_eq!(g.preview.loop_start, beat);
            assert_eq!(
                g.preview.loop_end,
                if looping { beat * 11 / 4 } else { beat * 5 / 4 }
            );
            let mut body = Gesture {
                context: c,
                down_x: 300.0,
                handle: Handle::LoopBody,
                preview: c.values,
            };
            body.update(-f32::MAX, false);
            assert_eq!(body.preview.loop_end - body.preview.loop_start, 2 * beat);
            assert_eq!(
                body.preview.loop_start,
                if looping { beat * 3 / 4 } else { 0 }
            );
        }
        let c = Context {
            values: SourceValues {
                loop_start: beat,
                loop_end: 3 * beat,
                ..base.values
            },
            ..base
        };
        let mut g = Gesture {
            context: c,
            down_x: 200.0,
            handle: Handle::LoopStart,
            preview: c.values,
        };
        g.update(235.0, false);
        assert_eq!(g.preview.loop_start, beat * 5 / 4);
        g.update(235.0, true);
        assert!((g.preview.loop_start as i128 - (beat as i128 * 135 / 100)).abs() <= 1);
        assert_eq!(g.preview.loop_end, 3 * beat);
        let preview = g.preview;
        assert!(!g.update(f32::NAN, false));
        assert_eq!(g.preview, preview);
    }
}
