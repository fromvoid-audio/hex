//! UI-only snap selection and bounded musical timeline helpers.

use crate::adaptive_grid_density as density;
#[path = "signed_musical_time.rs"]
pub mod signed;
pub use density::AdaptiveGridDensity;

use crate::viewport::Viewport;
use hex_project::{BeatTime, MeterPosition, Project};

const MAX_RULER_TICKS: usize = 64;
const MAX_METER_MAP_POINTS: usize = 1_024;
const MAX_GRID_LINES: usize = 256;
const MAX_GRID_BANDS: usize = 256;
const RULER_LABEL_SPACING_PX: f32 = 48.0;

#[cfg(test)]
thread_local! {
    // Actual resolver work per test thread: step/base/share/natural demand.
    static GRID_RESOLUTION_WORK: std::cell::Cell<[usize; 4]> = const { std::cell::Cell::new([0; 4]) };
}

#[cfg(test)]
fn record_grid_work(kind: usize) {
    GRID_RESOLUTION_WORK.with(|work| {
        let mut counts = work.get();
        counts[kind] += 1;
        work.set(counts);
    });
}

fn adaptive_grid_spacing(pixel_width: f32, density: AdaptiveGridDensity) -> f32 {
    density
        .minimum_spacing()
        .max(pixel_width / (MAX_GRID_LINES.saturating_sub(1) as f32))
}

mod bands;
mod grid;
mod ruler;
mod snap;

pub use bands::{GridBand, adaptive_grid_bands, adaptive_grid_bands_for_grid};
pub use grid::{GridLine, grid_lines};
pub use ruler::{RulerTick, ruler_ticks};
pub use snap::SnapGrid;

#[cfg(test)]
use bands::bounded_band_indices;
#[cfg(test)]
use grid::GRID_LINE_BUILDS;
use grid::{AdaptiveGridResolution, adaptive_base_step, grid_lines_resolved};
#[cfg(test)]
use ruler::meter_label;
use ruler::{LandmarkDensity, capped_fair_share, ruler_candidates, visible_meter_sections};

#[cfg(test)]
#[path = "adaptive_grid_density_tests.rs"]
mod density_tests;
#[cfg(test)]
mod tests;
