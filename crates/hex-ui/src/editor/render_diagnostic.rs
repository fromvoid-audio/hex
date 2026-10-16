//! Temporary opt-in UI-thread CPU diagnostics; removed before production candidate.
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug)]
pub(super) enum Kind {
    RenderBuild,
    OverviewLookup,
    OverviewBuild,
    OverviewPaint,
    MidiBuild,
    Normalize,
    WaveLookup,
    WavePrepare,
    WavePaint,
    CacheHit,
    CacheMiss,
    CacheEvict,
    FrameCpu,
    RootToLeadingPrepaint,
    LeadingToTrailingPrepaint,
    LeadingToTrailingPaint,
    RowBuild,
    AudioBuild,
}
const KINDS: [Kind; 18] = [
    Kind::RenderBuild,
    Kind::OverviewLookup,
    Kind::OverviewBuild,
    Kind::OverviewPaint,
    Kind::MidiBuild,
    Kind::Normalize,
    Kind::WaveLookup,
    Kind::WavePrepare,
    Kind::WavePaint,
    Kind::CacheHit,
    Kind::CacheMiss,
    Kind::CacheEvict,
    Kind::FrameCpu,
    Kind::RootToLeadingPrepaint,
    Kind::LeadingToTrailingPrepaint,
    Kind::LeadingToTrailingPaint,
    Kind::RowBuild,
    Kind::AudioBuild,
];
#[derive(Clone, Copy, Default)]
struct Metric {
    count: u64,
    nanos: u128,
}
#[derive(Clone, Copy, Default)]
pub(super) struct Stats {
    metrics: [Metric; 18],
}
thread_local! { static ACTIVE: RefCell<Option<Stats>> = const { RefCell::new(None) }; }
pub(super) fn begin() {
    ACTIVE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            *state = Some(Stats::default());
        }
    });
}
pub(super) fn snapshot(stop: bool) -> Option<Stats> {
    ACTIVE.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        if stop { state.take() } else { *state }
    })
}
pub(super) fn record(kind: Kind, duration: Duration) {
    ACTIVE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut()
            && let Some(stats) = state.as_mut()
        {
            let metric = &mut stats.metrics[kind as usize];
            metric.count += 1;
            metric.nanos += duration.as_nanos();
        }
    });
}
pub(super) struct Scope {
    kind: Kind,
    started: Option<Instant>,
}
pub(super) fn scope(kind: Kind) -> Scope {
    let active = ACTIVE.with(|state| state.try_borrow().ok().is_some_and(|state| state.is_some()));
    Scope {
        kind,
        started: active.then(Instant::now),
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            record(self.kind, started.elapsed());
        }
    }
}
impl Stats {
    pub(super) fn report(self) -> String {
        KINDS
            .into_iter()
            .map(|kind| {
                let metric = self.metrics[kind as usize];
                format!(
                    "{kind:?}:count={},cpu_us={:.3}",
                    metric.count,
                    metric.nanos as f64 / 1000.0
                )
            })
            .collect::<Vec<_>>()
            .join(";")
    }
}
