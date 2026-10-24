//! Optional CPU attribution for the ignored navigation probe. Compiled only in
//! UI tests, disabled by default, and never a production scheduling/paint owner.
use std::{cell::Cell, time::Instant};

#[derive(Clone, Copy)]
pub(super) enum Stage {
    Editor,
    Row,
    ClipModel,
    WaveformLookup,
    WaveformBuild,
    WaveformSubmit,
    PaintObservation,
}

#[derive(Clone, Copy)]
pub(super) struct Measurements {
    pub(super) ns: [u64; 7],
    pub(super) calls: [u64; 7],
}

#[derive(Clone, Copy)]
struct State {
    enabled: bool,
    measurements: Measurements,
}

thread_local! {
    static STATE: Cell<State> = const { Cell::new(State {
        enabled: false,
        measurements: Measurements { ns: [0; 7], calls: [0; 7] },
    }) };
}

pub(super) fn reset(enabled: bool) {
    STATE.with(|state| {
        state.set(State {
            enabled,
            measurements: Measurements {
                ns: [0; 7],
                calls: [0; 7],
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
