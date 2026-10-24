//! Optional CPU attribution for the ignored navigation probe. Compiled only in
//! UI tests, disabled by default, and never a production scheduling/paint owner.
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};
use std::{cell::Cell, time::Instant};

#[derive(Clone, Copy)]
pub(super) enum Stage {
    Editor,
    Row,
    ClipModel,
    WaveformLookup,
    WaveformBuild,
    // Host lookup/preparation only; shared widget submission is in Paint.
    WaveformProvider,
    PaintObservation,
    RequestLayout,
    Prepaint,
    Paint,
}

#[derive(Clone, Copy)]
pub(super) struct Measurements {
    pub(super) ns: [u64; 10],
    pub(super) calls: [u64; 10],
}

#[derive(Clone, Copy)]
struct State {
    enabled: bool,
    measurements: Measurements,
}

thread_local! {
    static STATE: Cell<State> = const { Cell::new(State {
        enabled: false,
        measurements: Measurements { ns: [0; 10], calls: [0; 10] },
    }) };
}

pub(super) fn reset(enabled: bool) {
    STATE.with(|state| {
        state.set(State {
            enabled,
            measurements: Measurements {
                ns: [0; 10],
                calls: [0; 10],
            },
        })
    });
}

pub(super) fn snapshot() -> Measurements {
    STATE.with(|state| state.get().measurements)
}

pub(super) struct Timer {
    stage: usize,
    started: Instant,
}

impl Timer {
    pub(super) fn start(stage: Stage) -> Option<Self> {
        STATE.with(|state| state.get().enabled).then(|| Self {
            stage: stage as usize,
            started: Instant::now(),
        })
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        STATE.with(|state| {
            let mut value = state.get();
            value.measurements.ns[self.stage] =
                value.measurements.ns[self.stage].saturating_add(elapsed);
            value.measurements.calls[self.stage] =
                value.measurements.calls[self.stage].saturating_add(1);
            state.set(value);
        });
    }
}

/// Forward the same child layout node without an extra style/container node.
/// This separates GPUI tree phases from view construction and waveform work.
pub(super) struct PhaseProbe {
    child: AnyElement,
}
impl PhaseProbe {
    pub(super) fn new(child: AnyElement) -> Self {
        Self { child }
    }
}
impl IntoElement for PhaseProbe {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for PhaseProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let _timer = Timer::start(Stage::RequestLayout);
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let _timer = Timer::start(Stage::Prepaint);
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let _timer = Timer::start(Stage::Paint);
        self.child.paint(window, cx);
    }
}
