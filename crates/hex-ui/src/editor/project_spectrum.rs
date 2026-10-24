//! View-only project analyzer. Canonical selection/color stay in the project;
//! audio owns real capture, freshness, worker storage and retirement.
use super::EditorView;
use crate::theme;
use gpui::{
    Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Window, div, prelude::*, px, rgb,
};
use hex_audio::spectrum_worker::PublishedProjectSpectrum;
use hex_widgets::{SpectrumPlotLayer, SpectrumPlotLayers, layered_spectrum_plot};

pub(super) struct ProjectSpectrumState {
    pub(super) expanded: bool,
    height: f32,
    pub(super) detail_height: Option<f32>,
    resize: Option<Resize>,
    result: Option<PublishedProjectSpectrum>,
    output_identity: Option<u64>,
    plot_cache: Option<PlotCache>,
}
impl ProjectSpectrumState {
    pub(super) fn resize_is_active(&self) -> bool {
        self.resize.is_some()
    }
}

impl Default for ProjectSpectrumState {
    fn default() -> Self {
        Self {
            expanded: true,
            height: 120.0,
            detail_height: None,
            resize: None,
            result: None,
            output_identity: None,
            plot_cache: None,
        }
    }
}
struct PlotCache {
    generation: u64,
    revision: u64,
    theme: hex_preferences::ThemePreferences,
    source: (
        std::time::Instant,
        std::time::Instant,
        u64,
        hex_audio::spectrum_worker::ProjectSelection,
    ),
    layers: SpectrumPlotLayers,
}
struct Resize {
    y: f32,
    original: f32,
    generation: u64,
}
fn bounded_height(requested: f32, viewport: f32) -> f32 {
    // Supported minimum editor is540px. Reserve toolbar/status, detail/mixer/
    // routing rows and usable workspace; the graph itself retains46px or more.
    let max = (viewport - 420.0).clamp(72.0, 300.0);
    if requested.is_finite() {
        requested.clamp(72.0, max)
    } else {
        120.0_f32.min(max)
    }
}
impl EditorView {
    fn project_spectrum_chrome(&self) -> f32 {
        let returns = self
            .state
            .as_ref()
            .map_or(0, |s| s.project.return_tracks().len());
        // Title22, transport26, status22, overview24, arrangement header42,
        // main track44, mixer26 and detail tabs22. Routing owns its own height.
        228.0 + super::return_tracks::section_height(returns)
    }
    fn project_spectrum_compact(&self, window: &Window) -> bool {
        self.project_spectrum.expanded
            && self.project_spectrum.detail_height.is_some_and(|detail| {
                f32::from(window.viewport_size().height)
                    - self.project_spectrum_chrome()
                    - detail
                    - 60.0
                    < 72.0
            })
    }
    fn project_spectrum_height(&self, window: &Window) -> f32 {
        if !self.project_spectrum.expanded || self.project_spectrum_compact(window) {
            return 22.0;
        }
        let viewport = f32::from(window.viewport_size().height);
        let desired = bounded_height(self.project_spectrum.height, viewport);
        // Keep complete populated details whenever a useful graph fits. At the
        // minimum window, the detail viewport scrolls instead of crushing tracks.
        self.project_spectrum
            .detail_height
            .map_or(desired, |detail| {
                desired.min((viewport - self.project_spectrum_chrome() - detail - 60.0).max(72.0))
            })
    }
    /// Empty space yields to the timeline; real controls remain scrollable.
    pub(super) fn project_spectrum_detail_height(&self, window: &Window, preferred: f32) -> f32 {
        let viewport = f32::from(window.viewport_size().height);
        let workspace = if self.project_spectrum.detail_height.is_some()
            && self.project_spectrum_height(window) == 22.0
        {
            44.0
        } else {
            60.0
        };
        preferred.min(
            (viewport
                - self.project_spectrum_chrome()
                - self.project_spectrum_height(window)
                - workspace)
                .max(48.0),
        )
    }
    fn project_spectrum_ids(&self) -> Vec<u64> {
        let mut ids: Vec<_> = self
            .selected_track_ids()
            .iter()
            .map(|id| id.value())
            .collect();
        ids.sort_unstable();
        ids
    }
    pub(super) fn tick_project_spectrum(&mut self) -> bool {
        let ids = self.project_spectrum_ids();
        let generation = self.project_generation;
        let floor = self.state.as_ref().and_then(|state| {
            self.audio_admission
                .published_audio_revision(generation, state.revision)
        });
        let expanded = self.project_spectrum.expanded && !self.opening_project;
        let mut result = None;
        let mut output_identity = None;
        if let Some(audio) = self.audio.as_mut() {
            let desired = audio.control.project_spectrum_desired();
            let same = desired.is_some_and(|s| {
                s.generation == generation
                    && s.tracks() == ids
                    && s.sample_rate_hz == audio.sample_rate
            });
            if !expanded {
                let _ = audio.control.project_spectrum_clear();
            } else {
                if !same {
                    // Clear old identity immediately while a new canonical
                    // revision is still waiting for its real audio publication.
                    let _ = audio.control.project_spectrum_clear();
                    if let Some(floor) = floor {
                        let _ = audio.control.project_spectrum_select(
                            &ids,
                            generation,
                            floor,
                            audio.sample_rate,
                        );
                    }
                }
                result = audio.control.project_spectrum_latest().filter(|r| {
                    r.selection.generation == generation
                        && r.selection.tracks() == ids
                        && r.selection.sample_rate_hz == audio.sample_rate
                });
            }
        }
        if result.is_some() {
            output_identity = self
                .audio
                .as_ref()
                .and_then(|a| a.control.output_identity());
        }
        if self.project_spectrum.result != result
            || self.project_spectrum.output_identity != output_identity
        {
            self.project_spectrum.plot_cache = None;
            self.project_spectrum.result = result;
            self.project_spectrum.output_identity = output_identity;
            true
        } else {
            false
        }
    }
    fn toggle_project_spectrum(&mut self, cx: &mut Context<Self>) {
        self.cancel_project_spectrum_resize(cx);
        self.project_spectrum.expanded = !self.project_spectrum.expanded;
        self.project_spectrum.result = None;
        if !self.project_spectrum.expanded
            && let Some(audio) = self.audio.as_ref()
        {
            let _ = audio.control.project_spectrum_clear();
        }
        self.tick_project_spectrum();
        cx.notify();
    }
    pub(super) fn cancel_project_spectrum_resize(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(resize) = self.project_spectrum.resize.take() {
            self.project_spectrum.height = resize.original;
            cx.notify();
            true
        } else {
            false
        }
    }
    fn resize_project_spectrum(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(resize) = self.project_spectrum.resize.as_ref() else {
            return false;
        };
        if resize.generation != self.project_generation || !self.focus.is_focused(window) {
            return self.cancel_project_spectrum_resize(cx);
        }
        self.project_spectrum.height = bounded_height(
            resize.original + f32::from(position.y) - resize.y,
            f32::from(window.viewport_size().height),
        );
        cx.notify();
        true
    }
    pub(super) fn project_spectrum_pointer_events(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let entity = cx.entity();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let handled = moved.update(cx, |view, cx| {
                        if event.pressed_button != Some(MouseButton::Left) {
                            view.cancel_project_spectrum_resize(cx)
                        } else {
                            view.resize_project_spectrum(event.position, window, cx)
                        }
                    });
                    if handled {
                        cx.stop_propagation();
                        window.refresh();
                    }
                });
                let released = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    let handled = released.update(cx, |view, cx| {
                        if view.project_spectrum.resize.is_none() {
                            return false;
                        }
                        view.resize_project_spectrum(event.position, window, cx);
                        view.project_spectrum.resize = None;
                        cx.notify();
                        true
                    });
                    if handled {
                        cx.stop_propagation();
                        window.refresh();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
    fn project_spectrum_layers(&mut self, now: std::time::Instant) -> SpectrumPlotLayers {
        if self.project_spectrum.output_identity
            != self
                .audio
                .as_ref()
                .and_then(|a| a.control.output_identity())
        {
            return SpectrumPlotLayers::default();
        }
        if !self.project_spectrum.expanded {
            return SpectrumPlotLayers::default();
        }
        let ids = self.project_spectrum_ids();
        let Some(result) = self.project_spectrum.result.as_ref().filter(|r| {
            r.selection.generation == self.project_generation
                && r.selection.tracks() == ids
                && r.is_fresh_at(now)
        }) else {
            return SpectrumPlotLayers::default();
        };
        let Some(state) = self.state.as_ref() else {
            return SpectrumPlotLayers::default();
        };
        if result.selection.sample_rate_hz != state.project.sample_rate() {
            return SpectrumPlotLayers::default();
        }
        let source = (
            result.captured_at,
            result.processed_at,
            result.applied_revision,
            result.selection,
        );
        let palette = theme::preferences();
        if let Some(cache) = self.project_spectrum.plot_cache.as_ref()
            && cache.generation == self.project_generation
            && cache.source == source
            && cache.revision == state.revision
            && cache.theme == palette
        {
            return cache.layers.clone();
        }
        // One canonical traversal per revision/theme/result change, with bounded
        // binary lookup. Never a separate full project scan for each layer.
        let mut colors = vec![None; ids.len()];
        for track in state.project.tracks() {
            if let Ok(index) = ids.binary_search(&track.id().value()) {
                colors[index] = Some(theme::element_colors(track.color()).accent);
            }
        }
        let Some(mut colors) = colors.into_iter().collect::<Option<Vec<_>>>() else {
            return SpectrumPlotLayers::default();
        };
        colors.push(theme::device_analyzer_style().output);
        // Unrelated edits update the lookup key without converting points again.
        if let Some(cache) = self.project_spectrum.plot_cache.as_mut()
            && cache.generation == self.project_generation
            && cache.source == source
            && cache
                .layers
                .iter()
                .map(|l| l.color)
                .eq(colors.iter().copied())
        {
            cache.revision = state.revision;
            cache.theme = palette;
            return cache.layers.clone();
        }
        let points = |frame: &hex_audio::spectrum::SpectrumFrame| {
            let upper = 20_000.0_f32.min(frame.sample_rate_hz as f32 / 2.0);
            let span = (upper / 20.0).ln();
            frame
                .bins_dbfs
                .iter()
                .enumerate()
                .filter_map(|(index, db)| {
                    let (low, high) =
                        hex_audio::spectrum::band_bounds_hz(frame.sample_rate_hz, index)?;
                    Some([
                        (((low * high).sqrt() / 20.0).ln() / span).clamp(0.0, 1.0),
                        super::device_panel::spectrum_display::level(*db),
                    ])
                })
                .collect()
        };
        let mut layers = Vec::with_capacity(result.selection.layer_count());
        for (index, color) in colors[..ids.len()].iter().enumerate() {
            layers.push(SpectrumPlotLayer {
                points: points(&result.frames()[index + 1]),
                color: *color,
            });
        }
        // Real master stays contrasting and above every independent track layer.
        layers.push(SpectrumPlotLayer {
            points: points(&result.frames()[0]),
            color: theme::device_analyzer_style().output,
        });
        let layers = SpectrumPlotLayers::new(layers).unwrap_or_default();
        self.project_spectrum.plot_cache = Some(PlotCache {
            generation: self.project_generation,
            revision: state.revision,
            theme: palette,
            source,
            layers: layers.clone(),
        });
        layers
    }
    pub(super) fn project_spectrum_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if self.project_spectrum.resize.as_ref().is_some_and(|r| {
            r.generation != self.project_generation || !self.focus.is_focused(window)
        }) {
            self.cancel_project_spectrum_resize(cx);
        }
        let expanded = self.project_spectrum.expanded;
        let height = self.project_spectrum_height(window);
        let style = theme::device_style(theme::DeviceStyleRole::Display);
        let available = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.control.project_spectrum_available());
        let summary = if available {
            format!("master · {} tracks", self.selected_track_ids().len())
        } else {
            "unavailable".into()
        };
        let compact = self.project_spectrum_compact(window);
        let mut header = div()
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_1()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .child(
                div()
                    .id("project-spectrum-toggle")
                    .debug_selector(|| "project-spectrum-toggle".into())
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(theme::text(if expanded { "−" } else { "+" }))
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::transport::MiniTooltip {
                            description: if expanded {
                                "collapse spectrum".into()
                            } else {
                                "expand spectrum".into()
                            },
                        })
                        .into()
                    })
                    .on_click(cx.listener(|view, _, _, cx| view.toggle_project_spectrum(cx))),
            )
            .child(theme::text("spectrum"))
            .child(
                div()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(summary)),
            );
        if compact {
            let plot = layered_spectrum_plot(
                self.project_spectrum_layers(std::time::Instant::now()),
                style,
            );
            header = header.child(
                div().flex_1().min_w_0().h(px(18.0)).child(
                    plot.id("project-spectrum-plot")
                        .debug_selector(|| "project-spectrum-plot".into()),
                ),
            );
        }
        let mut panel = div()
            .id("project-spectrum")
            .debug_selector(|| "project-spectrum".into())
            .h(px(height))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .text_xs()
            .border_b_1()
            .border_color(rgb(style.border))
            .child(header);
        if expanded && !compact {
            let layers = self.project_spectrum_layers(std::time::Instant::now());
            let plot = layered_spectrum_plot(layers, style);
            panel = panel
                .child(
                    plot.id("project-spectrum-plot")
                        .debug_selector(|| "project-spectrum-plot".into())
                        .flex_1()
                        .min_h_0(),
                )
                .child(
                    div()
                        .id("project-spectrum-resize")
                        .debug_selector(|| "project-spectrum-resize".into())
                        .h(px(4.0))
                        .flex_shrink_0()
                        .bg(rgb(style.border))
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                window.focus(&view.focus);
                                view.project_spectrum.resize = Some(Resize {
                                    y: f32::from(event.position.y),
                                    original: bounded_height(
                                        view.project_spectrum.height,
                                        f32::from(window.viewport_size().height),
                                    ),
                                    generation: view.project_generation,
                                });
                                cx.notify();
                            }),
                        ),
                );
        }
        panel
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
