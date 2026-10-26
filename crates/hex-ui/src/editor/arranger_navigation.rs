//! Transient arranger navigation extent, independent of canonical content/export
//! bounds. A canonical install retains the prior view; only explicit navigation
//! retires obsolete offscreen tail. Clip editors keep their own source clocks.
use super::{EditorView, ViewState, viewport::Viewport};

const EXTRA_TRAILING_SECONDS: u64 = 6; // Service already supplies two seconds.
const TRAILING_SECONDS: u64 = 8;

#[derive(Default)]
pub(super) struct Extent {
    owner: Option<(u64, u64)>,
    normal: u64,
    retained: u64,
}

impl EditorView {
    pub(super) fn normal_navigation_horizon(&self, horizon: u64) -> u64 {
        let Some(state) = self.state.as_ref() else {
            return horizon;
        };
        let base = state.timeline_frames.saturating_add(
            u64::from(state.project.sample_rate()).saturating_mul(EXTRA_TRAILING_SECONDS),
        );
        let authored =
            if self.arranger_navigation.owner == Some((self.project_generation, state.revision)) {
                self.arranger_navigation.normal
            } else {
                0
            };
        horizon.max(base).max(authored)
    }

    pub(super) fn retained_navigation_horizon(&self, horizon: u64) -> u64 {
        let retained = if self
            .arranger_navigation
            .owner
            .is_some_and(|owner| owner.0 == self.project_generation)
        {
            self.arranger_navigation.retained
        } else {
            0
        };
        self.normal_navigation_horizon(horizon).max(retained)
    }

    pub(super) fn reconcile_arranger_navigation(&mut self, state: &ViewState, replaced: bool) {
        let owner = (self.project_generation, state.revision);
        if !replaced && self.arranger_navigation.owner == Some(owner) {
            return;
        }
        let prior = (!replaced)
            .then(|| {
                self.state.as_ref().map(|old| {
                    (
                        self.visible_viewport(old.timeline_frames),
                        self.viewport_horizon(old.timeline_frames),
                    )
                })
            })
            .flatten();
        let rate = u64::from(state.project.sample_rate());
        // Visit authored automation once per installed revision, never per paint.
        let automation_end = state
            .project
            .tracks()
            .iter()
            .flat_map(|track| {
                track
                    .gain_envelope()
                    .last()
                    .map(|p| p.beat)
                    .into_iter()
                    .chain(track.gain_lane().spans().iter().map(|span| span.end))
            })
            .filter_map(|beat| state.project.tempo_map().beat_to_frame(beat).ok())
            .max();
        let normal = state
            .timeline_frames
            .saturating_add(rate.saturating_mul(EXTRA_TRAILING_SECONDS))
            .max(automation_end.map_or(0, |end| {
                end.saturating_add(rate.saturating_mul(TRAILING_SECONDS))
            }));
        self.arranger_navigation = Extent {
            owner: Some(owner),
            normal,
            retained: 0,
        };
        if let Some((visible, old_horizon)) = prior {
            if old_horizon != normal {
                self.viewport = Some(visible);
            }
            self.arranger_navigation.retained =
                old_horizon.max(visible.end().unwrap_or(old_horizon));
        }
    }

    pub(super) fn install_arranger_navigation(&mut self, viewport: Viewport) {
        let normal = self.state.as_ref().map_or(0, |state| {
            self.normal_navigation_horizon(state.timeline_frames)
        });
        self.arranger_navigation.retained = viewport.end().unwrap_or(normal);
        self.viewport = (viewport != Viewport::fit(normal)).then_some(viewport);
    }

    pub(super) fn clear_retained_navigation(&mut self) {
        self.arranger_navigation.retained = 0;
    }
}
