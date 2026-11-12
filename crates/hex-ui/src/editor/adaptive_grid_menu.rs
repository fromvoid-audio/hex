//! Arranger density choices use the same bounded popup/focus contract as clip actions.
use super::super::musical_time::AdaptiveGridDensity;
use super::*;

impl EditorView {
    pub(in crate::editor) fn open_arranger_grid_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_track_context_menu(position, None, window, cx);
    }

    pub(super) fn append_adaptive_grid_choices(&mut self) {
        if self.mode != super::super::EditorMode::Arrangement {
            return;
        }
        let generation = self.project_generation;
        let current = self.snap_grid.density();
        let decibels = self.waveform_geometry_cache.amplitude_scale()
            == super::super::waveform::WaveformAmplitudeScale::Decibel;
        if let Some(menu) = &mut self.context_menu {
            menu.items
                .retain(|item| !matches!(item.action, EditorContextAction::AdaptiveGrid { .. }));
            menu.items
                .extend(AdaptiveGridDensity::ALL.map(|density| MenuItem {
                    label: density.label(),
                    swatch: None,
                    checked: current == density,
                    action: EditorContextAction::AdaptiveGrid {
                        generation,
                        density,
                    },
                }));
            menu.items.retain(|item| {
                !matches!(item.action, EditorContextAction::WaveformDecibels { .. })
            });
            menu.items.push(MenuItem {
                label: "waveform db scale",
                swatch: None,
                checked: decibels,
                action: EditorContextAction::WaveformDecibels {
                    generation,
                    enabled: !decibels,
                },
            });
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "adaptive_grid_menu_tests.rs"]
mod tests;
