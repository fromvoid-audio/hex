use gpui::{Point, Pixels, point, px, ScrollDelta, ScrollWheelEvent};
use wayland_client::{protocol::wl_pointer, protocol::wl_pointer::AxisSource, WEnum};
const SCROLL_LINES: f32 = 3.0;
// Axis metadata stays local to one wl_pointer.frame. Raw content accumulators
// retain their original arithmetic and routing; only optional parameter ticks
// are normalized per original axis before Shift can collapse both onto x.
#[derive(Clone, Copy, Default, PartialEq)]
enum AxisDirection {
    #[default]
    Unknown,
    Identical,
    Inverted,
    Ambiguous,
}

impl AxisDirection {
    fn observe(&mut self, direction: WEnum<wl_pointer::AxisRelativeDirection>) {
        let next = match direction {
            WEnum::Value(wl_pointer::AxisRelativeDirection::Identical) => Self::Identical,
            WEnum::Value(wl_pointer::AxisRelativeDirection::Inverted) => Self::Inverted,
            _ => Self::Ambiguous,
        };
        *self = match (*self, next) {
            (Self::Unknown, next) => next,
            (old, next) if old == next => old,
            _ => Self::Ambiguous,
        };
    }
}

#[derive(Default)]
struct AxisTicks {
    // [original protocol axis][routed output axis], horizontal then vertical.
    lines: [[f32; 2]; 2],
    directions: [AxisDirection; 2],
    pending_direction: [bool; 2],
}

impl AxisTicks {
    fn index(axis: wl_pointer::Axis) -> Option<usize> {
        match axis {
            wl_pointer::Axis::HorizontalScroll => Some(0),
            wl_pointer::Axis::VerticalScroll => Some(1),
            _ => None,
        }
    }

    fn record(&mut self, original: wl_pointer::Axis, routed: wl_pointer::Axis, lines: f32) {
        if let (Some(original), Some(routed)) = (Self::index(original), Self::index(routed)) {
            self.lines[original][routed] += lines;
        }
    }

    fn physical(&self, raw: Point<f32>) -> Point<f32> {
        if !self.directions.contains(&AxisDirection::Inverted) {
            // Preserve the old f32 accumulation/division exactly in all fallback
            // cases. Unknown/conflicting direction never guesses an inversion.
            return raw.map(|lines| lines / SCROLL_LINES);
        }
        let signed = |original: usize, routed: usize| {
            let lines = self.lines[original][routed];
            if self.directions[original] == AxisDirection::Inverted {
                -lines
            } else {
                lines
            }
        };
        point(
            (signed(0, 0) + signed(1, 0)) / SCROLL_LINES,
            (signed(0, 1) + signed(1, 1)) / SCROLL_LINES,
        )
    }
}

struct AxisFrame {
    axis_source: AxisSource,
    continuous_scroll_delta: Option<Point<Pixels>>,
    discrete_scroll_delta: Option<Point<f32>>,
    vertical_modifier: f32,
    horizontal_modifier: f32,
    scroll_event_received: bool,
    ticks: AxisTicks,
}

impl Default for AxisFrame {
    fn default() -> Self {
        Self {
            axis_source: AxisSource::Wheel,
            continuous_scroll_delta: None,
            discrete_scroll_delta: None,
            vertical_modifier: -1.0,
            horizontal_modifier: -1.0,
            scroll_event_received: false,
            ticks: AxisTicks::default(),
        }
    }
}

impl AxisFrame {
    fn fold(
        &mut self,
        event: &wl_pointer::Event,
        shift: bool,
    ) -> Option<(ScrollDelta, Option<Point<f32>>)> {
        match event {
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(axis_source),
            } => {
                self.axis_source = *axis_source;
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                if let Some(original) = AxisTicks::index(*axis) {
                    if !std::mem::take(&mut self.ticks.pending_direction[original]) {
                        // A direction report describes one paired Axis, not all
                        // later motion of that axis in this frame.
                        self.ticks.directions[original] = AxisDirection::Ambiguous;
                    }
                }
                if self.axis_source == AxisSource::Wheel {
                    return None;
                }
                let axis = if shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    *axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => self.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => self.horizontal_modifier,
                    _ => 1.0,
                };
                self.scroll_event_received = true;
                let scroll_delta = self
                    .continuous_scroll_delta
                    .get_or_insert(point(px(0.0), px(0.0)));
                let modifier = 3.0;
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += px(*value as f32 * modifier * axis_modifier);
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += px(*value as f32 * modifier * axis_modifier);
                    }
                    _ => unreachable!(),
                }
            }
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                let original_axis = *axis;
                self.scroll_event_received = true;
                let axis = if shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    *axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => self.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => self.horizontal_modifier,
                    _ => 1.0,
                };

                let scroll_delta = self.discrete_scroll_delta.get_or_insert(point(0.0, 0.0));
                self.ticks.record(
                    original_axis,
                    axis,
                    *discrete as f32 * axis_modifier * SCROLL_LINES,
                );
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += *discrete as f32 * axis_modifier * SCROLL_LINES;
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += *discrete as f32 * axis_modifier * SCROLL_LINES;
                    }
                    _ => unreachable!(),
                }
            }
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(axis),
                value120,
            } => {
                let original_axis = *axis;
                self.scroll_event_received = true;
                let axis = if shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    *axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => self.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => self.horizontal_modifier,
                    _ => unreachable!(),
                };

                let scroll_delta = self.discrete_scroll_delta.get_or_insert(point(0.0, 0.0));
                let wheel_percent = *value120 as f32 / 120.0;
                self.ticks.record(
                    original_axis,
                    axis,
                    wheel_percent * axis_modifier * SCROLL_LINES,
                );
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += wheel_percent * axis_modifier * SCROLL_LINES;
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += wheel_percent * axis_modifier * SCROLL_LINES;
                    }
                    _ => unreachable!(),
                }
            }
            wl_pointer::Event::AxisRelativeDirection {
                axis: WEnum::Value(axis),
                direction,
            } => {
                if let Some(axis) = AxisTicks::index(*axis) {
                    self.ticks.directions[axis].observe(*direction);
                    self.ticks.pending_direction[axis] = true;
                }
            }
            wl_pointer::Event::Frame => {
                // Drain even without motion or a focused window: per-frame
                // direction reports must never affect the next logical group.
                let mut ticks = std::mem::take(&mut self.ticks);
                for axis in 0..2 {
                    if ticks.pending_direction[axis] {
                        ticks.directions[axis] = AxisDirection::Ambiguous;
                    }
                }
                if self.scroll_event_received {
                    self.scroll_event_received = false;
                    let continuous = self.continuous_scroll_delta.take();
                    let discrete = self.discrete_scroll_delta.take();
                    let physical = discrete.map(|raw| ticks.physical(raw));
                    if let Some(continuous) = continuous {
                        return Some((ScrollDelta::Pixels(continuous), physical));
                    } else if let Some(discrete) = discrete {
                        return Some((ScrollDelta::Lines(discrete), physical));
                    }
                }
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod axis_frame_tests {
    use super::*;
    use wl_pointer::{Axis, AxisRelativeDirection as Direction, Event};

    fn direction(axis: Axis, direction: WEnum<Direction>) -> Event {
        Event::AxisRelativeDirection {
            axis: WEnum::Value(axis),
            direction,
        }
    }

    fn tick(axis: Axis, value120: i32) -> Event {
        Event::AxisValue120 {
            axis: WEnum::Value(axis),
            value120,
        }
    }

    fn axis(axis: Axis, value: f64) -> Event {
        Event::Axis {
            time: 0,
            axis: WEnum::Value(axis),
            value,
        }
    }

    fn source(source: AxisSource) -> Event {
        Event::AxisSource {
            axis_source: WEnum::Value(source),
        }
    }

    fn finish(frame: &mut AxisFrame) -> (ScrollDelta, Option<Point<f32>>) {
        frame.fold(&Event::Frame, false).expect("motion frame")
    }

    fn raw_bits(delta: ScrollDelta) -> (bool, [u32; 2]) {
        match delta {
            ScrollDelta::Lines(p) => (false, [p.x.to_bits(), p.y.to_bits()]),
            ScrollDelta::Pixels(p) => (true, [f32::from(p.x).to_bits(), f32::from(p.y).to_bits()]),
        }
    }

    #[test]
    fn axis_frame_v9_direction_order_and_mixed_axes() {
        for inverted_h in [false, true] {
            for inverted_v in [false, true] {
                for late in [false, true] {
                    let mut f = AxisFrame::default();
                    let h = direction(
                        Axis::HorizontalScroll,
                        WEnum::Value(if inverted_h {
                            Direction::Inverted
                        } else {
                            Direction::Identical
                        }),
                    );
                    let v = direction(
                        Axis::VerticalScroll,
                        WEnum::Value(if inverted_v {
                            Direction::Inverted
                        } else {
                            Direction::Identical
                        }),
                    );
                    if !late {
                        f.fold(&h, false);
                        f.fold(&v, false);
                    }
                    f.fold(&tick(Axis::VerticalScroll, 120), false);
                    f.fold(&tick(Axis::HorizontalScroll, -240), false);
                    if late {
                        f.fold(&h, false);
                        f.fold(&v, false);
                    }
                    f.fold(&axis(Axis::HorizontalScroll, -2.0), false);
                    f.fold(&axis(Axis::VerticalScroll, 1.0), false);
                    let (raw, ticks) = finish(&mut f);
                    // Exact baseline content bits: direction metadata cannot change it.
                    assert_eq!(
                        raw_bits(raw),
                        (false, [6.0f32.to_bits(), (-3.0f32).to_bits()])
                    );
                    assert_eq!(
                        ticks,
                        Some(point(
                            if inverted_h { -2.0 } else { 2.0 },
                            if inverted_v { 1.0 } else { -1.0 }
                        ))
                    );
                }
            }
        }
    }

    #[test]
    fn axis_frame_shift_collapses_only_after_original_direction() {
        let mut f = AxisFrame::default();
        f.fold(
            &direction(Axis::VerticalScroll, WEnum::Value(Direction::Inverted)),
            true,
        );
        f.fold(
            &direction(Axis::HorizontalScroll, WEnum::Value(Direction::Identical)),
            true,
        );
        f.fold(&tick(Axis::VerticalScroll, 120), true);
        f.fold(&tick(Axis::HorizontalScroll, 240), true);
        f.fold(&axis(Axis::VerticalScroll, 1.0), true);
        f.fold(&axis(Axis::HorizontalScroll, 2.0), true);
        let (raw, ticks) = finish(&mut f);
        assert_eq!(
            raw_bits(raw),
            (false, [(-9.0f32).to_bits(), 0.0f32.to_bits()])
        );
        assert_eq!(ticks, Some(point(-1.0, 0.0)));
        // Existing event helper must not invert these normalized ticks again.
        let event = ScrollWheelEvent {
            delta: raw,
            wheel_ticks: ticks,
            ..Default::default()
        };
        assert_eq!(event.physical_wheel_ticks(), ticks);
        assert_eq!(raw_bits(event.physical_delta()), raw_bits(raw));
    }

    #[test]
    fn axis_frame_pixels_plus_ticks_and_continuous_only() {
        let mut f = AxisFrame::default();
        f.fold(&source(AxisSource::Finger), false);
        f.fold(
            &direction(Axis::VerticalScroll, WEnum::Value(Direction::Inverted)),
            false,
        );
        f.fold(&tick(Axis::VerticalScroll, 30), false);
        f.fold(&axis(Axis::VerticalScroll, 1.0), false);
        let (raw, ticks) = finish(&mut f);
        assert_eq!(
            raw_bits(raw),
            (true, [0.0f32.to_bits(), (-3.0f32).to_bits()])
        );
        assert_eq!(ticks, Some(point(0.0, 0.25)));
        f.fold(
            &direction(Axis::VerticalScroll, WEnum::Value(Direction::Inverted)),
            false,
        );
        f.fold(&axis(Axis::VerticalScroll, 1.0), false);
        let (raw, ticks) = finish(&mut f);
        assert_eq!(
            raw_bits(raw),
            (true, [0.0f32.to_bits(), (-3.0f32).to_bits()])
        );
        assert_eq!(ticks, None); // Do not invent detents for continuous motion.
    }

    #[test]
    fn axis_frame_conflict_unknown_and_independent_known_axis() {
        for reports in [
            [
                WEnum::Value(Direction::Inverted),
                WEnum::Value(Direction::Identical),
            ],
            [WEnum::Unknown(999), WEnum::Value(Direction::Inverted)],
            [WEnum::Value(Direction::Inverted), WEnum::Unknown(999)],
        ] {
            let mut f = AxisFrame::default();
            for report in reports {
                f.fold(&direction(Axis::VerticalScroll, report), true);
                f.fold(&axis(Axis::VerticalScroll, 1.0), true);
            }
            f.fold(
                &direction(Axis::HorizontalScroll, WEnum::Value(Direction::Inverted)),
                true,
            );
            f.fold(&tick(Axis::VerticalScroll, 120), true);
            f.fold(&tick(Axis::HorizontalScroll, 240), true);
            f.fold(&axis(Axis::HorizontalScroll, 1.0), true);
            let (raw, ticks) = finish(&mut f);
            assert_eq!(
                raw_bits(raw),
                (false, [(-9.0f32).to_bits(), 0.0f32.to_bits()])
            );
            // Only vertical falls back; the independent horizontal axis corrects.
            assert_eq!(ticks, Some(point(1.0, 0.0)));
        }
    }

    #[test]
    fn axis_frame_fractions_large_zero_and_cancellation() {
        for values in [
            [30, 0, 0],
            [-30, 0, 0],
            [240, 0, 0],
            [-240, 0, 0],
            [30, -30, 0],
            [0, 0, 0],
        ] {
            for inverted in [false, true] {
                let mut f = AxisFrame::default();
                if inverted {
                    f.fold(
                        &direction(Axis::VerticalScroll, WEnum::Value(Direction::Inverted)),
                        false,
                    );
                }
                for value in values {
                    f.fold(&tick(Axis::VerticalScroll, value), false);
                }
                f.fold(&axis(Axis::VerticalScroll, 0.0), false);
                let (raw, ticks) = finish(&mut f);
                let expected = -(values.into_iter().sum::<i32>() as f32) / 120.0;
                let ScrollDelta::Lines(raw) = raw else {
                    panic!("lines");
                };
                assert_eq!(raw.y, expected * SCROLL_LINES);
                assert_eq!(
                    ticks.unwrap().y,
                    if inverted { -expected } else { expected }
                );
            }
        }
    }

    #[test]
    fn axis_frame_missing_report_cannot_inherit_a_previous_pair() {
        let mut f = AxisFrame::default();
        f.fold(
            &direction(Axis::VerticalScroll, WEnum::Value(Direction::Inverted)),
            false,
        );
        f.fold(&tick(Axis::VerticalScroll, 30), false);
        f.fold(&axis(Axis::VerticalScroll, 1.0), false);
        f.fold(&tick(Axis::VerticalScroll, 30), false);
        f.fold(&axis(Axis::VerticalScroll, 1.0), false);
        assert_eq!(finish(&mut f).1, Some(point(0.0, -0.5)));
        // A report without its required Axis must not normalize a whole frame.
        f.fold(&tick(Axis::VerticalScroll, 30), false);
        f.fold(
            &direction(Axis::VerticalScroll, WEnum::Value(Direction::Inverted)),
            false,
        );
        assert_eq!(finish(&mut f).1, Some(point(0.0, -0.25)));
    }

    #[test]
    fn axis_frame_pre_v9_discrete_and_frame_retirement() {
        let mut f = AxisFrame::default();
        f.fold(
            &Event::AxisDiscrete {
                axis: WEnum::Value(Axis::VerticalScroll),
                discrete: -2,
            },
            false,
        );
        assert_eq!(finish(&mut f).1, Some(point(0.0, 2.0)));
        assert!(f.fold(&Event::Frame, false).is_none());
        // No motion/window delivery must still discard this direction report.
        f.fold(
            &direction(Axis::VerticalScroll, WEnum::Value(Direction::Inverted)),
            false,
        );
        assert!(f.fold(&Event::Frame, false).is_none());
        f.fold(&tick(Axis::VerticalScroll, 30), false);
        // Discard the fold result exactly as Dispatch does with no focused window.
        let _undelivered = finish(&mut f);
        f.fold(&tick(Axis::VerticalScroll, 30), false);
        assert_eq!(finish(&mut f).1, Some(point(0.0, -0.25)));
        f.fold(
            &Event::AxisRelativeDirection {
                axis: WEnum::Unknown(99),
                direction: WEnum::Value(Direction::Inverted),
            },
            false,
        );
        f.fold(&tick(Axis::VerticalScroll, 30), false);
        assert_eq!(finish(&mut f).1, Some(point(0.0, -0.25)));
    }
}

#[allow(dropping_references)]
mod legacy_raw_differential {
    use super::*;
    use std::{cell::RefCell, rc::Rc};
    #[derive(Clone)]
    struct Window(Rc<RefCell<Vec<ScrollWheelEvent>>>);
    impl Window { fn handle_input(&self, input: gpui::PlatformInput) {
        let gpui::PlatformInput::ScrollWheel(event) = input else { panic!("scroll only"); };
        self.0.borrow_mut().push(event);
    }}
    struct State {
        axis_source: AxisSource, modifiers: gpui::Modifiers,
        vertical_modifier: f32, horizontal_modifier: f32,
        scroll_event_received: bool, continuous_scroll_delta: Option<Point<Pixels>>,
        discrete_scroll_delta: Option<Point<f32>>,
        mouse_focused_window: Option<Window>, mouse_location: Option<Point<Pixels>>,
    }
    use gpui::{PlatformInput, TouchPhase};
    fn feed(state: &mut State, event: wl_pointer::Event) { match event {
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(axis_source),
            } => {
                state.axis_source = axis_source;
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                if state.axis_source == AxisSource::Wheel {
                    return;
                }
                let axis = if state.modifiers.shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => state.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => state.horizontal_modifier,
                    _ => 1.0,
                };
                state.scroll_event_received = true;
                let scroll_delta = state
                    .continuous_scroll_delta
                    .get_or_insert(point(px(0.0), px(0.0)));
                let modifier = 3.0;
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += px(value as f32 * modifier * axis_modifier);
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += px(value as f32 * modifier * axis_modifier);
                    }
                    _ => unreachable!(),
                }
            }
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                state.scroll_event_received = true;
                let axis = if state.modifiers.shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => state.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => state.horizontal_modifier,
                    _ => 1.0,
                };

                let scroll_delta = state.discrete_scroll_delta.get_or_insert(point(0.0, 0.0));
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += discrete as f32 * axis_modifier * SCROLL_LINES;
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += discrete as f32 * axis_modifier * SCROLL_LINES;
                    }
                    _ => unreachable!(),
                }
            }
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(axis),
                value120,
            } => {
                state.scroll_event_received = true;
                let axis = if state.modifiers.shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => state.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => state.horizontal_modifier,
                    _ => unreachable!(),
                };

                let scroll_delta = state.discrete_scroll_delta.get_or_insert(point(0.0, 0.0));
                let wheel_percent = value120 as f32 / 120.0;
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += wheel_percent * axis_modifier * SCROLL_LINES;
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += wheel_percent * axis_modifier * SCROLL_LINES;
                    }
                    _ => unreachable!(),
                }
            }
            wl_pointer::Event::Frame => {
                if state.scroll_event_received {
                    state.scroll_event_received = false;
                    let continuous = state.continuous_scroll_delta.take();
                    let discrete = state.discrete_scroll_delta.take();
                    if let Some(continuous) = continuous {
                        if let Some(window) = state.mouse_focused_window.clone() {
                            let input = PlatformInput::ScrollWheel(ScrollWheelEvent {
                                direction_inverted_from_device: false,
                                is_momentum: false,
                                position: state.mouse_location.unwrap(),
                                delta: ScrollDelta::Pixels(continuous),
                                wheel_ticks: discrete.map(|ticks| ticks.map(|lines| lines / SCROLL_LINES)),
                                modifiers: state.modifiers,
                                touch_phase: TouchPhase::Moved,
                            });
                            drop(state);
                            window.handle_input(input);
                        }
                    } else if let Some(discrete) = discrete
                        && let Some(window) = state.mouse_focused_window.clone()
                    {
                        let input = PlatformInput::ScrollWheel(ScrollWheelEvent {
                            direction_inverted_from_device: false,
                            is_momentum: false,
                            position: state.mouse_location.unwrap(),
                            delta: ScrollDelta::Lines(discrete),
                            wheel_ticks: Some(discrete.map(|lines| lines / SCROLL_LINES)),
                            modifiers: state.modifiers,
                            touch_phase: TouchPhase::Moved,
                        });
                        drop(state);
                        window.handle_input(input);
                    }
                }
            }
            _ => {}
    }}
    fn event(kind: u32, seed: u32) -> wl_pointer::Event {
        use wl_pointer::{Axis, AxisRelativeDirection as D, Event};
        let axis=if seed & 1 == 0 { Axis::HorizontalScroll } else { Axis::VerticalScroll };
        match kind {
            0 => Event::AxisSource { axis_source: WEnum::Value(if seed & 2 == 0 { AxisSource::Finger } else { AxisSource::Wheel }) },
            1 => Event::Axis { time: 0, axis: WEnum::Value(axis), value: f64::from((seed % 1001) as i32 - 500) / 7.0 },
            2 => Event::AxisDiscrete { axis: WEnum::Value(axis), discrete: (seed % 17) as i32 - 8 },
            3 => Event::AxisValue120 { axis: WEnum::Value(axis), value120: (seed % 1023) as i32 - 511 },
            4 => Event::AxisRelativeDirection { axis: WEnum::Value(axis), direction: if seed & 8 != 0 { WEnum::Unknown(99) } else { WEnum::Value(if seed & 4 == 0 { D::Inverted } else { D::Identical }) } },
            _ => Event::Frame,
        }
    }
    fn raw(delta: ScrollDelta) -> (bool, [u32; 2]) {
        match delta {
            ScrollDelta::Lines(p) => (false,[p.x.to_bits(),p.y.to_bits()]),
            ScrollDelta::Pixels(p) => (true,[f32::from(p.x).to_bits(),f32::from(p.y).to_bits()]),
        }
    }
    #[test]
    fn exact_old_arms_raw_bits_and_fallback_match_across_4096_frames() {
        let out=Rc::new(RefCell::new(Vec::new()));
        let mut old=State { axis_source: AxisSource::Wheel, modifiers: Default::default(), vertical_modifier:-1.0, horizontal_modifier:-1.0, scroll_event_received:false, continuous_scroll_delta:None, discrete_scroll_delta:None, mouse_focused_window:Some(Window(out.clone())), mouse_location:Some(point(px(0.0),px(0.0))) };
        let mut new=AxisFrame::default();
        let mut seed=7u32; let mut compared=0;
        for frame in 0..4096 {
            let no_window=frame % 11 == 0;
            old.mouse_focused_window=if no_window { None } else { Some(Window(out.clone())) };
            for step in 0..9 {
                seed=seed.wrapping_mul(1664525).wrapping_add(1013904223);
                old.modifiers.shift=seed & 32 != 0;
                let kind=if step==8 { 5 } else { seed % 5 };
                feed(&mut old,event(kind,seed));
                let got=new.fold(&event(kind,seed),old.modifiers.shift);
                if kind==5 {
                    if !no_window {
                        let expected=out.borrow_mut().pop();
                        assert_eq!(got.is_some(),expected.is_some(),"frame {frame}");
                        if let (Some((delta,ticks)),Some(e))=(got,expected) {
                            assert_eq!(raw(delta),raw(e.delta),"frame {frame}");
                            // With no reported physical correction this path is
                            // bit-identical, including non-associative f32 sums.
                            if ticks==e.wheel_ticks { compared+=1; }
                        }
                    }
                    assert!(out.borrow().is_empty());
                }
            }
        }
        assert!(compared>1000,"exercise legacy metadata too: {compared}");
        println!("4096 frames: exact raw bits; {compared} identical metadata frames; unknown/no-window/Shift/source/order included");
    }
}
