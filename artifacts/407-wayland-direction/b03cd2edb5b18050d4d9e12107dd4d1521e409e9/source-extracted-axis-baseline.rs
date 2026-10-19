
#![allow(dead_code, dropping_references)]
use std::{cell::RefCell,rc::Rc};
#[derive(Clone,Copy,Debug,PartialEq)] struct Point<T>{x:T,y:T}
impl<T> Point<T>{ fn map<U>(self,mut f:impl FnMut(T)->U)->Point<U>{Point{x:f(self.x),y:f(self.y)}} }
fn point<T>(x:T,y:T)->Point<T>{Point{x,y}}
type Pixels=f32; fn px(x:f32)->f32{x}
const SCROLL_LINES:f32=3.0;
#[derive(Clone,Copy,Debug,Default)] struct Modifiers{shift:bool}
#[derive(Clone,Copy,Debug)] enum TouchPhase{Moved}
#[derive(Clone,Copy,Debug,PartialEq)] enum ScrollDelta{Pixels(Point<f32>),Lines(Point<f32>)}
#[derive(Clone,Copy,PartialEq)] enum AxisSource{Wheel,Finger}
#[derive(Clone,Copy)] enum WEnum<T>{Value(T),Unknown(u32)}
mod wl_pointer {
 use super::{AxisSource,WEnum};
 #[derive(Clone,Copy)] pub enum Axis{VerticalScroll,HorizontalScroll,Unknown}
 #[derive(Clone,Copy)] pub enum AxisRelativeDirection{Identical,Inverted}
 pub enum Event{
  AxisSource{axis_source:WEnum<AxisSource>}, Axis{axis:WEnum<Axis>,value:f64,time:u32},
  AxisDiscrete{axis:WEnum<Axis>,discrete:i32}, AxisValue120{axis:WEnum<Axis>,value120:i32},
  AxisRelativeDirection{axis:WEnum<Axis>,direction:WEnum<AxisRelativeDirection>},Frame
 }
}
#[derive(Clone,Debug)]
struct ScrollWheelEvent {
    /// True when the platform inverted device scroll direction for user preferences.
    /// Raw `delta` remains suitable for ordinary content scrolling.
    pub direction_inverted_from_device: bool,

    /// True for platform-reported inertial continuation after physical input.
    /// Neutral by default. Ordinary content scrolling keeps using raw `delta`.
    pub is_momentum: bool,

    /// Physical wheel increments, when the platform knows them independently
    /// of content-line preferences. Fractional increments are retained. Uses
    /// the same preference-direction convention as `delta`; content scrolling
    /// must continue using `delta`, not this optional parameter-input metadata.
    pub wheel_ticks: Option<Point<f32>>,

    /// The position of the mouse on the window.
    pub position: Point<Pixels>,

    /// The change in scroll wheel position for this event.
    pub delta: ScrollDelta,

    /// The modifiers that were held down when the mouse was moved.
    pub modifiers: Modifiers,

    /// The phase of the touch event.
    pub touch_phase: TouchPhase,
}

impl ScrollWheelEvent {
    /// Opt-in physical detents, with user-preference inversion undone once.
    pub fn physical_wheel_ticks(&self) -> Option<Point<f32>> {
        self.wheel_ticks.map(|ticks| {
            if self.direction_inverted_from_device {
                point(-ticks.x, -ticks.y)
            } else {
                ticks
            }
        })
    }

    /// Device-direction delta before the platform's user-preference inversion.
    /// This is opt-in: ordinary scrolling should continue using `delta`.
    pub fn physical_delta(&self) -> ScrollDelta {
        if !self.direction_inverted_from_device {
            return self.delta;
        }
        match self.delta {
            ScrollDelta::Pixels(delta) => ScrollDelta::Pixels(point(-delta.x, -delta.y)),
            ScrollDelta::Lines(delta) => ScrollDelta::Lines(point(-delta.x, -delta.y)),
        }
    }
}


#[derive(Clone)] struct Window{out:Rc<RefCell<Vec<ScrollWheelEvent>>>}
enum PlatformInput{ScrollWheel(ScrollWheelEvent)}
impl Window{fn handle_input(&self,input:PlatformInput){let PlatformInput::ScrollWheel(e)=input;self.out.borrow_mut().push(e);}}
struct State{axis_source:AxisSource,modifiers:Modifiers,vertical_modifier:f32,horizontal_modifier:f32,
 scroll_event_received:bool,continuous_scroll_delta:Option<Point<f32>>,discrete_scroll_delta:Option<Point<f32>>,
 mouse_focused_window:Option<Window>,mouse_location:Option<Point<f32>>}
fn state()->State{State{axis_source:AxisSource::Wheel,modifiers:Modifiers::default(),vertical_modifier:-1.0,horizontal_modifier:-1.0,scroll_event_received:false,continuous_scroll_delta:None,discrete_scroll_delta:None,mouse_focused_window:Some(Window{out:Rc::default()}),mouse_location:Some(point(0.0,0.0))}}
fn feed(state:&mut State,event:wl_pointer::Event){match event{
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
use wl_pointer::{Axis,AxisRelativeDirection as Direction,Event};
fn send(s:&mut State,axis:Axis,n:i32,direction:Option<Direction>){
 if let Some(direction)=direction{feed(s,Event::AxisRelativeDirection{axis:WEnum::Value(axis),direction:WEnum::Value(direction)});}
 feed(s,Event::AxisValue120{axis:WEnum::Value(axis),value120:n});
 feed(s,Event::Axis{axis:WEnum::Value(axis),value:1.0,time:0});
}
fn last(s:&State)->ScrollWheelEvent{s.mouse_focused_window.as_ref().unwrap().out.borrow().last().unwrap().clone()}
#[test]fn v9_inverted_wheel_ticks_follow_device(){let mut s=state();send(&mut s,Axis::VerticalScroll,120,Some(Direction::Inverted));feed(&mut s,Event::Frame);let e=last(&s);assert_eq!(e.delta,ScrollDelta::Lines(point(0.0,-3.0)));assert_eq!(e.physical_wheel_ticks(),Some(point(0.0,1.0)));}
#[test]fn v9_mixed_axis_directions_are_independent(){let mut s=state();send(&mut s,Axis::HorizontalScroll,120,Some(Direction::Identical));send(&mut s,Axis::VerticalScroll,120,Some(Direction::Inverted));feed(&mut s,Event::Frame);let e=last(&s);assert_eq!(e.delta,ScrollDelta::Lines(point(-3.0,-3.0)));assert_eq!(e.physical_wheel_ticks(),Some(point(-1.0,1.0)));}
#[test]fn v9_continuous_plus_fractional_ticks_preserves_content(){let mut s=state();feed(&mut s,Event::AxisSource{axis_source:WEnum::Value(AxisSource::Finger)});send(&mut s,Axis::VerticalScroll,30,Some(Direction::Inverted));feed(&mut s,Event::Frame);let e=last(&s);assert_eq!(e.delta,ScrollDelta::Pixels(point(0.0,-3.0)));assert_eq!(e.physical_wheel_ticks(),Some(point(0.0,0.25)));}
#[test]fn v9_shift_follows_original_axis_direction_after_swap(){let mut s=state();s.modifiers.shift=true;send(&mut s,Axis::VerticalScroll,120,Some(Direction::Inverted));feed(&mut s,Event::Frame);let e=last(&s);assert_eq!(e.delta,ScrollDelta::Lines(point(-3.0,0.0)));assert_eq!(e.physical_wheel_ticks(),Some(point(1.0,0.0)));}
#[test]fn pre_v9_unknown_retains_current_fractional_content_and_empty_frames(){let mut s=state();send(&mut s,Axis::VerticalScroll,30,None);feed(&mut s,Event::Frame);let e=last(&s);assert_eq!(e.delta,ScrollDelta::Lines(point(0.0,-0.75)));assert_eq!(e.physical_wheel_ticks(),Some(point(0.0,-0.25)));feed(&mut s,Event::Frame);assert_eq!(s.mouse_focused_window.as_ref().unwrap().out.borrow().len(),1);assert!(!s.scroll_event_received);assert!(s.discrete_scroll_delta.is_none());}
