from pathlib import Path
import hashlib,json,subprocess
r=Path('/workspace/hex');d=r/'artifacts/407-wayland-direction';client=r/'third_party/gpui/src/platform/linux/wayland/client.rs';interactive=r/'third_party/gpui/src/interactive.rs';s=client.read_text();i=s.index('            wl_pointer::Event::AxisSource',s.index('// Axis Events'));j=s.index('            _ => {}',i)+len('            _ => {}');arms=s[i:j];u=interactive.read_text();a=u.index('pub struct ScrollWheelEvent {');b=u.index('impl Sealed for ScrollWheelEvent',a);carrier=u[a:b]
pre=r'''
#![allow(dead_code, dropping_references)]
use std::{cell::RefCell,rc::Rc};
#[derive(Clone,Copy,Debug,PartialEq)] struct Point<T>{x:T,y:T}
impl<T> Point<T>{ fn map<U>(self,mut f:impl FnMut(T)->U)->Point<U>{Point{x:f(self.x),y:f(self.y)}} }
fn point<T>(x:T,y:T)->Point<T>{Point{x,y}}
type Pixels=f32; fn px(x:f32)->f32{x}
const SCROLL_LINES:f32=3.0;
#[derive(Clone,Copy,Debug,Default)] struct Modifiers{shift:bool}
#[derive(Clone,Copy,Debug)] enum TouchPhase{Moved}
#[derive(Clone,Copy,Debug)] enum ScrollDelta{Pixels(Point<f32>),Lines(Point<f32>)}
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
'''
post=r'''
#[derive(Clone)] struct Window{out:Rc<RefCell<Vec<ScrollWheelEvent>>>}
enum PlatformInput{ScrollWheel(ScrollWheelEvent)}
impl Window{fn handle_input(&self,input:PlatformInput){let PlatformInput::ScrollWheel(e)=input;self.out.borrow_mut().push(e);}}
struct State{axis_source:AxisSource,modifiers:Modifiers,vertical_modifier:f32,horizontal_modifier:f32,
 scroll_event_received:bool,continuous_scroll_delta:Option<Point<f32>>,discrete_scroll_delta:Option<Point<f32>>,
 mouse_focused_window:Option<Window>,mouse_location:Option<Point<f32>>}
fn state()->State{State{axis_source:AxisSource::Wheel,modifiers:Modifiers::default(),vertical_modifier:-1.0,horizontal_modifier:-1.0,scroll_event_received:false,continuous_scroll_delta:None,discrete_scroll_delta:None,mouse_focused_window:Some(Window{out:Rc::default()}),mouse_location:Some(point(0.0,0.0))}}
fn feed(state:&mut State,event:wl_pointer::Event){match event{
'''
tests=r'''
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
'''
# This is literal production axis match arms and carrier/getter source, NOT a
# handwritten behavior replica or full Wayland connection/window integration.
text=pre+carrier.replace('pub struct ScrollWheelEvent','struct ScrollWheelEvent',1)+post+arms+tests
text=text.replace('#[derive(Clone,Copy,Debug)] enum ScrollDelta','#[derive(Clone,Copy,Debug,PartialEq)] enum ScrollDelta')
p=d/'source-extracted-axis-baseline.rs';p.write_text(text);(d/'axis-arms-exact.txt').write_text(arms);(d/'provenance.json').write_text(json.dumps({'source':'0134de5a06f2bc80baaa24475b1fd221b7ed22f5','client_sha256':hashlib.sha256(client.read_bytes()).hexdigest(),'interactive_sha256':hashlib.sha256(interactive.read_bytes()).hexdigest(),'literal_axis_arms_sha256':hashlib.sha256(arms.encode()).hexdigest(),'harness_sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'limits':'production event arms/getter bytes compiled with minimal protocol/point/window stubs; not full Dispatch or native compositor'},indent=2)+'\n')
compile=subprocess.run(['rustc','--edition=2024','--test',str(p),'-o',str(d/'axis-baseline-tests')],capture_output=True,text=True);(d/'compile.log').write_text(compile.stdout+compile.stderr);print('COMPILE_EXIT',compile.returncode)
if compile.returncode:raise SystemExit(compile.returncode)
run=subprocess.run([str(d/'axis-baseline-tests'),'--nocapture'],capture_output=True,text=True);(d/'baseline.log').write_text(run.stdout+run.stderr);print(run.stdout+run.stderr);print('TEST_EXIT',run.returncode);raise SystemExit(run.returncode)
