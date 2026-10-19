from pathlib import Path
import subprocess,hashlib,json
r=Path('/workspace/hex');d=r/'artifacts/407-wayland-direction'
s=subprocess.check_output(['git','show','0134de5a06f2bc80baaa24475b1fd221b7ed22f5:third_party/gpui/src/platform/linux/wayland/client.rs'],cwd=r,text=True)
a=s.index('            wl_pointer::Event::AxisSource',s.index('// Axis Events'));b=s.index('            _ => {}',a)+len('            _ => {}');arms=s[a:b]
prefix='''
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
'''
suffix='''
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
'''
(d/'legacy-raw-differential.rs').write_text(prefix+arms+suffix)
(d/'legacy-raw-provenance.json').write_text(json.dumps({'source':'0134de5a06f2bc80baaa24475b1fd221b7ed22f5','literal_old_arms_sha256':hashlib.sha256(arms.encode()).hexdigest(),'limits':'actual old match-arm bytes; actual gpui/Wayland types; minimal output-window carrier, not Dispatch/server/hardware; mixed synthetic ordering includes malformed protocol to check raw preservation'},indent=2)+'\n')
