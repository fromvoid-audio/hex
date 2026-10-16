//! Temporary opt-in UI-thread elapsed diagnostics; removed before production candidate.
use gpui::{IntoElement, canvas, prelude::*, px};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::OnceLock,
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
    ClipContentBuild,
    ClipContentReuse,
    CachedClipPrepaintWall,
    CachedClipPaintWall,
}
const KINDS: [Kind; 22] = [
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
    Kind::ClipContentBuild,
    Kind::ClipContentReuse,
    Kind::CachedClipPrepaintWall,
    Kind::CachedClipPaintWall,
];
#[derive(Clone, Copy, Default)]
struct Metric {
    count: u64,
    nanos: u128,
}
#[derive(Clone, Copy, Default)]
pub(super) struct Stats {
    metrics: [Metric; 22],
}
pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("HEX_TIME_SELECTION_DIAGNOSTIC").is_some_and(|value| value == "1")
    })
}

/// Live lane siblings bracket the cached child without changing its bounds.
pub(super) fn clip_brackets() -> (impl IntoElement, impl IntoElement) {
    let prepaint = Rc::new(Cell::new(None::<Instant>));
    let paint = Rc::new(Cell::new(None::<Instant>));
    let leading_prepaint = Rc::clone(&prepaint);
    let leading_paint = Rc::clone(&paint);
    let leading = canvas(
        move |_, _, _| leading_prepaint.set(Some(Instant::now())),
        move |_, _, _, _| leading_paint.set(Some(Instant::now())),
    )
    .absolute()
    .top_0()
    .left_0()
    .w(px(1.0))
    .h(px(1.0));
    let trailing = canvas(
        move |_, _, _| {
            if let Some(started) = prepaint.take() {
                record(Kind::CachedClipPrepaintWall, started.elapsed());
            }
        },
        move |_, _, _, _| {
            if let Some(started) = paint.take() {
                record(Kind::CachedClipPaintWall, started.elapsed());
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .w(px(1.0))
    .h(px(1.0));
    (leading, trailing)
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
