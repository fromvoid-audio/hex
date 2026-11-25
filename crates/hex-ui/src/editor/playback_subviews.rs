//! Isolated GPUI `Entity` subviews for 60 Hz playback-driven surfaces.
//!
//! Transport position readout, CPU meter, status-bar output meter, Arrangement
//! and Piano Roll playhead overlays, and Console/Arrangement track meters live
//! in dedicated GPUI entities hosted by [`PlaybackOverlayView`] as a sibling of
//! cached [`EditorView`] under [`EditorWindowRoot`].
//!
//! On steady playback ticks, [`EditorView::tick`] updates only the affected
//! playback subview entities and skips `cx.notify()` on `EditorView`. GPUI then
//! replays `EditorView`'s cached scene without re-executing `EditorView::render`,
//! repainting only the dirty playback subviews at their measured slot bounds.
//! When `EditorView` is mounted directly (without `EditorWindowRoot`) or while a
//! foreground modal/drag is active, the same isolated subviews render inline so
//! z-order and nested `ClipContents` caching remain intact.
use super::{EditorView, drag::LaneBounds, viewport::Viewport};
use crate::output_meter::{
    HEIGHT as OUTPUT_METER_HEIGHT, MeterInput, MeterReading, WIDTH as OUTPUT_METER_WIDTH,
};
use crate::theme;
use gpui::{
    AnyElement, AnyView, Bounds, ContentMask, Context, Div, Entity, IntoElement, MouseButton,
    Pixels, Render, SharedString, Subscription, WeakEntity, Window, canvas, div, point, prelude::*,
    px, rgb, size,
};
use hex_preferences::ThemePreferences;
use hex_project::{ReturnId, TrackId};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

const CPU_METER_WIDTH: f32 = 68.0;

#[derive(Clone, Copy, Debug)]
pub(super) struct ArrangementPlayheadSlot {
    pub(super) body: LaneBounds,
    pub(super) main_top: f32,
    pub(super) header_width: f32,
    pub(super) viewport: Viewport,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PianoRollPlayheadSlot {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) beat_start: i128,
    pub(super) raw_span: u64,
    pub(super) clip_mask: Bounds<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MeterTarget {
    ArrangementTrack { id: TrackId, height: f32 },
    ConsoleTrack { id: TrackId, height: f32 },
    Return { id: ReturnId, height: f32 },
    Main { height: f32 },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MeterSlot {
    pub(super) target: MeterTarget,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) clip_mask: Bounds<Pixels>,
}

#[derive(Default)]
pub(super) struct PlaybackOverlaySlots {
    pub(super) position_readout: Option<Bounds<Pixels>>,
    pub(super) cpu_meter: Option<Bounds<Pixels>>,
    pub(super) output_meter: Option<Bounds<Pixels>>,
    pub(super) arrangement_body: Option<LaneBounds>,
    pub(super) arrangement_main_top: Option<f32>,
    pub(super) arrangement_header_width: f32,
    pub(super) arrangement_viewport: Option<Viewport>,
    pub(super) piano_roll_playhead: Option<PianoRollPlayheadSlot>,
    pub(super) meters: Vec<MeterSlot>,
}

impl PlaybackOverlaySlots {
    pub(super) fn begin_render(&mut self) {
        self.position_readout = None;
        self.cpu_meter = None;
        self.output_meter = None;
        self.arrangement_body = None;
        self.arrangement_main_top = None;
        self.arrangement_viewport = None;
        self.piano_roll_playhead = None;
        self.meters.clear();
    }

    pub(super) fn arrangement_playhead(&self) -> Option<ArrangementPlayheadSlot> {
        Some(ArrangementPlayheadSlot {
            body: self.arrangement_body?,
            main_top: self.arrangement_main_top?,
            header_width: self.arrangement_header_width,
            viewport: self.arrangement_viewport?,
        })
    }
}

/// Isolated transport position readout (`project position`).
pub(super) struct TransportReadoutView {
    label: SharedString,
    text_color: u32,
    slots: Rc<RefCell<PlaybackOverlaySlots>>,
    use_sibling_overlay: Rc<Cell<bool>>,
    #[cfg(all(test, feature = "ui-test-support"))]
    render_count: usize,
}

impl TransportReadoutView {
    fn new(slots: Rc<RefCell<PlaybackOverlaySlots>>, use_sibling_overlay: Rc<Cell<bool>>) -> Self {
        Self {
            label: theme::text("1.1.1"),
            text_color: theme::color(theme::PaletteRole::Text),
            slots,
            use_sibling_overlay,
            #[cfg(all(test, feature = "ui-test-support"))]
            render_count: 0,
        }
    }

    fn sync(&mut self, label: String, text_color: u32) -> bool {
        let label = theme::text(label);
        if self.label == label && self.text_color == text_color {
            return false;
        }
        self.label = label;
        self.text_color = text_color;
        true
    }
}

impl Render for TransportReadoutView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.render_count = self.render_count.saturating_add(1);
        }
        let content = div()
            .debug_selector(|| "project position".into())
            .px(px(4.0))
            .max_w(px(82.0))
            .flex_shrink_0()
            .line_clamp(1)
            .text_ellipsis()
            .text_xs()
            .text_color(rgb(self.text_color))
            .child(theme::text(&self.label));
        if !self.use_sibling_overlay.get() {
            return content.into_any_element();
        }
        let Some(bounds) = self
            .slots
            .try_borrow()
            .ok()
            .and_then(|slots| slots.position_readout)
        else {
            return div().into_any_element();
        };
        div()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .flex()
            .items_center()
            .child(content)
            .into_any_element()
    }
}

/// Isolated CPU meter readout and overload reset control (`editor-cpu-meter`).
pub(super) struct CpuMeterView {
    editor: WeakEntity<EditorView>,
    label: SharedString,
    description: SharedString,
    fraction: f32,
    warning: bool,
    primary_color: u32,
    secondary_color: u32,
    muted_color: u32,
    slots: Rc<RefCell<PlaybackOverlaySlots>>,
    use_sibling_overlay: Rc<Cell<bool>>,
    #[cfg(all(test, feature = "ui-test-support"))]
    render_count: usize,
}

impl CpuMeterView {
    fn new(
        editor: WeakEntity<EditorView>,
        slots: Rc<RefCell<PlaybackOverlaySlots>>,
        use_sibling_overlay: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            editor,
            label: theme::text("cpu --"),
            description: theme::text("audio cpu meter"),
            fraction: 0.0,
            warning: false,
            primary_color: theme::color(theme::PaletteRole::Primary),
            secondary_color: theme::color(theme::PaletteRole::Secondary),
            muted_color: theme::color(theme::PaletteRole::Muted),
            slots,
            use_sibling_overlay,
            #[cfg(all(test, feature = "ui-test-support"))]
            render_count: 0,
        }
    }

    fn sync(
        &mut self,
        label: String,
        description: String,
        fraction: f32,
        warning: bool,
        colors: (u32, u32, u32),
    ) -> bool {
        let label = theme::text(label);
        let description = theme::text(description);
        if self.label == label
            && self.description == description
            && self.fraction.to_bits() == fraction.to_bits()
            && self.warning == warning
            && (self.primary_color, self.secondary_color, self.muted_color) == colors
        {
            return false;
        }
        self.label = label;
        self.description = description;
        self.fraction = fraction;
        self.warning = warning;
        (self.primary_color, self.secondary_color, self.muted_color) = colors;
        true
    }
}

impl Render for CpuMeterView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.render_count = self.render_count.saturating_add(1);
        }
        let description = self.description.clone();
        let editor = self.editor.clone();
        let cpu_meter = div()
            .id("editor-cpu-meter")
            .debug_selector(|| "editor-cpu-meter".into())
            .w(px(CPU_METER_WIDTH))
            .h_full()
            .relative()
            .flex()
            .items_center()
            .justify_end()
            .flex_shrink_0()
            .text_xs()
            .text_right()
            .cursor_pointer()
            .text_color(rgb(if self.warning {
                self.primary_color
            } else {
                self.muted_color
            }))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: description.clone(),
                })
                .into()
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                let _ = editor.update(cx, |view, cx| {
                    let misses = view.audio_status().map_or(0, |status| {
                        status.cpu_meter.reset_peak();
                        status
                            .deadline_misses
                            .load(std::sync::atomic::Ordering::Relaxed)
                    });
                    view.cpu_meter.reset(misses);
                    view.sync_cpu_meter_subview(cx, true);
                    if !view.uses_sibling_playback_overlay() {
                        cx.notify();
                    }
                });
            })
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .h(px(2.0))
                    .w(px(CPU_METER_WIDTH * self.fraction))
                    .bg(rgb(if self.warning {
                        self.primary_color
                    } else {
                        self.secondary_color
                    })),
            )
            .child(theme::text(&self.label));
        if !self.use_sibling_overlay.get() {
            return cpu_meter.into_any_element();
        }
        let Some(bounds) = self
            .slots
            .try_borrow()
            .ok()
            .and_then(|slots| slots.cpu_meter)
        else {
            return div().into_any_element();
        };
        div()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .children([cpu_meter])
            .into_any_element()
    }
}

/// Isolated status-bar stereo output meter (`output-meter`).
pub(super) struct OutputMeterView {
    meter_input: MeterInput,
    reading: MeterReading,
    palette: ThemePreferences,
    slots: Rc<RefCell<PlaybackOverlaySlots>>,
    use_sibling_overlay: Rc<Cell<bool>>,
    #[cfg(all(test, feature = "ui-test-support"))]
    render_count: usize,
}

impl OutputMeterView {
    fn new(slots: Rc<RefCell<PlaybackOverlaySlots>>, use_sibling_overlay: Rc<Cell<bool>>) -> Self {
        Self {
            meter_input: MeterInput::Offline,
            reading: MeterReading::default(),
            palette: theme::preferences(),
            slots,
            use_sibling_overlay,
            #[cfg(all(test, feature = "ui-test-support"))]
            render_count: 0,
        }
    }

    fn sync(
        &mut self,
        meter_input: MeterInput,
        reading: MeterReading,
        palette: ThemePreferences,
    ) -> bool {
        if self.meter_input == meter_input
            && reading_bits_eq(self.reading, reading)
            && self.palette == palette
        {
            return false;
        }
        self.meter_input = meter_input;
        self.reading = reading;
        self.palette = palette;
        true
    }
}

impl Render for OutputMeterView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.render_count = self.render_count.saturating_add(1);
        }
        let meter_input = self.meter_input;
        let meter =
            crate::output_meter::render_unified(meter_input, self.reading).tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: crate::output_meter::description(meter_input).into(),
                })
                .into()
            });
        if !self.use_sibling_overlay.get() {
            return meter.into_any_element();
        }
        let Some(bounds) = self
            .slots
            .try_borrow()
            .ok()
            .and_then(|slots| slots.output_meter)
        else {
            return div().into_any_element();
        };
        div()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .children([meter])
            .into_any_element()
    }
}

/// Isolated Arrangement playhead overlay (`arrangement-playback-layer`).
pub(super) struct ArrangementPlayheadView {
    position: Option<u64>,
    color: u32,
    slots: Rc<RefCell<PlaybackOverlaySlots>>,
    #[cfg(all(test, feature = "ui-test-support"))]
    render_count: usize,
}

impl ArrangementPlayheadView {
    fn new(slots: Rc<RefCell<PlaybackOverlaySlots>>) -> Self {
        Self {
            position: None,
            color: theme::color(theme::PaletteRole::Text),
            slots,
            #[cfg(all(test, feature = "ui-test-support"))]
            render_count: 0,
        }
    }

    fn sync(&mut self, position: Option<u64>, color: u32) -> bool {
        if self.position == position && self.color == color {
            return false;
        }
        self.position = position;
        self.color = color;
        true
    }
}

impl Render for ArrangementPlayheadView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.render_count = self.render_count.saturating_add(1);
        }
        let slots = Rc::clone(&self.slots);
        let position = self.position;
        let color = self.color;
        div()
            .debug_selector(|| "arrangement-playback-layer".into())
            .absolute()
            .inset_0()
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let Some(frame) = position else {
                            return;
                        };
                        let Some(slot) = slots
                            .try_borrow()
                            .ok()
                            .and_then(|slots| slots.arrangement_playhead())
                        else {
                            return;
                        };
                        let width = slot.body.width - slot.header_width;
                        let height = slot.main_top - slot.body.top;
                        if !width.is_finite()
                            || !height.is_finite()
                            || width <= 0.0
                            || height <= 0.0
                        {
                            return;
                        }
                        let Some(fraction) = slot.viewport.fraction(frame) else {
                            return;
                        };
                        let bounds = Bounds::new(
                            point(
                                px(slot.body.left + width * fraction.clamp(0.0, 1.0)),
                                px(slot.body.top),
                            ),
                            size(px(1.0), px(height)),
                        );
                        window.paint_quad(gpui::fill(bounds, rgb(color)));
                    },
                )
                .absolute()
                .inset_0(),
            )
    }
}

/// Isolated Piano Roll playhead overlay.
pub(super) struct PianoRollPlayheadView {
    playhead_beat: Option<u64>,
    color: u32,
    slots: Rc<RefCell<PlaybackOverlaySlots>>,
    #[cfg(all(test, feature = "ui-test-support"))]
    render_count: usize,
}

impl PianoRollPlayheadView {
    fn new(slots: Rc<RefCell<PlaybackOverlaySlots>>) -> Self {
        Self {
            playhead_beat: None,
            color: theme::color(theme::PaletteRole::Secondary),
            slots,
            #[cfg(all(test, feature = "ui-test-support"))]
            render_count: 0,
        }
    }

    fn sync(&mut self, playhead_beat: Option<u64>, color: u32) -> bool {
        if self.playhead_beat == playhead_beat && self.color == color {
            return false;
        }
        self.playhead_beat = playhead_beat;
        self.color = color;
        true
    }
}

impl Render for PianoRollPlayheadView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.render_count = self.render_count.saturating_add(1);
        }
        let slots = Rc::clone(&self.slots);
        let playhead_beat = self.playhead_beat;
        let color = self.color;
        div()
            .debug_selector(|| "piano-roll-playback-layer".into())
            .absolute()
            .inset_0()
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let Some(playhead) = playhead_beat.map(i128::from) else {
                            return;
                        };
                        let Some(slot) = slots
                            .try_borrow()
                            .ok()
                            .and_then(|slots| slots.piano_roll_playhead)
                        else {
                            return;
                        };
                        let width = f32::from(slot.bounds.size.width);
                        let height = f32::from(slot.bounds.size.height);
                        if !width.is_finite()
                            || !height.is_finite()
                            || width <= 0.0
                            || height <= 0.0
                            || slot.raw_span == 0
                        {
                            return;
                        }
                        if playhead < slot.beat_start
                            || playhead >= slot.beat_start.saturating_add(i128::from(slot.raw_span))
                        {
                            return;
                        }
                        let x = ((playhead - slot.beat_start) as f64 / slot.raw_span as f64) as f32
                            * width;
                        let line = Bounds::new(
                            point(slot.bounds.origin.x + px(x), slot.bounds.origin.y),
                            size(px(1.0), px(height)),
                        );
                        window.with_content_mask(
                            Some(ContentMask {
                                bounds: slot.clip_mask,
                            }),
                            |window| {
                                window.paint_quad(gpui::fill(line, rgb(color)));
                            },
                        );
                    },
                )
                .absolute()
                .inset_0(),
            )
    }
}

/// Isolated Console and Arrangement track/return/main stereo meters.
pub(super) struct LiveMetersView {
    tracks: BTreeMap<TrackId, MeterReading>,
    returns: BTreeMap<ReturnId, MeterReading>,
    main: MeterReading,
    palette: ThemePreferences,
    slots: Rc<RefCell<PlaybackOverlaySlots>>,
    #[cfg(all(test, feature = "ui-test-support"))]
    render_count: usize,
}

impl LiveMetersView {
    fn new(slots: Rc<RefCell<PlaybackOverlaySlots>>) -> Self {
        Self {
            tracks: BTreeMap::new(),
            returns: BTreeMap::new(),
            main: MeterReading::default(),
            palette: theme::preferences(),
            slots,
            #[cfg(all(test, feature = "ui-test-support"))]
            render_count: 0,
        }
    }

    fn sync_from(
        &mut self,
        meters: &super::track_meters::State,
        palette: ThemePreferences,
    ) -> bool {
        let mut changed =
            self.palette != palette || !reading_bits_eq(self.main, meters.main_reading());
        let main = meters.main_reading();
        if !changed {
            changed = self.tracks.len() != meters.track_count()
                || self.returns.len() != meters.return_count()
                || meters.iter_track_readings().any(|(id, reading)| {
                    self.tracks
                        .get(&id)
                        .is_none_or(|prev| !reading_bits_eq(*prev, reading))
                })
                || meters.iter_return_readings().any(|(id, reading)| {
                    self.returns
                        .get(&id)
                        .is_none_or(|prev| !reading_bits_eq(*prev, reading))
                });
        }
        if !changed {
            return false;
        }
        self.palette = palette;
        self.main = main;
        self.tracks.clear();
        self.tracks.extend(meters.iter_track_readings());
        self.returns.clear();
        self.returns.extend(meters.iter_return_readings());
        true
    }
}

impl Render for LiveMetersView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.render_count = self.render_count.saturating_add(1);
        }
        let Ok(slots) = self.slots.try_borrow() else {
            return div().absolute().inset_0();
        };
        let mut container = div()
            .debug_selector(|| "live-meters-layer".into())
            .absolute()
            .inset_0();
        for slot in &slots.meters {
            let meter = match slot.target {
                MeterTarget::ArrangementTrack { id, height } => {
                    super::track_meters::render_arrangement_bars(
                        id,
                        self.tracks.get(&id).copied().unwrap_or_default(),
                        height,
                    )
                }
                MeterTarget::ConsoleTrack { id, height } => div()
                    .debug_selector(move || format!("console-track-meter-{}", id.value()))
                    .children([super::track_meters::render(
                        id,
                        self.tracks.get(&id).copied().unwrap_or_default(),
                        height,
                    )]),
                MeterTarget::Return { id, height } => super::track_meters::render_return(
                    id,
                    self.returns.get(&id).copied().unwrap_or_default(),
                    height,
                ),
                MeterTarget::Main { height } => super::track_meters::render_main(self.main, height),
            };
            let clip = slot.clip_mask;
            let bounds = slot.bounds;
            if bounds.bottom() <= clip.top()
                || bounds.top() >= clip.bottom()
                || bounds.right() <= clip.left()
                || bounds.left() >= clip.right()
            {
                continue;
            }
            let clip_left = clip.left().max(bounds.left());
            let clip_top = clip.top().max(bounds.top());
            let clip_right = clip.right().min(bounds.right());
            let clip_bottom = clip.bottom().min(bounds.bottom());
            let clip_w = (clip_right - clip_left).max(px(0.0));
            let clip_h = (clip_bottom - clip_top).max(px(0.0));
            container = container.child(
                div()
                    .absolute()
                    .left(clip_left)
                    .top(clip_top)
                    .w(clip_w)
                    .h(clip_h)
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left(bounds.left() - clip_left)
                            .top(bounds.top() - clip_top)
                            .w(bounds.size.width)
                            .h(bounds.size.height)
                            .children([meter]),
                    ),
            );
        }
        container
    }
}

/// Sibling playback overlay hosting all isolated 60 Hz playback entities.
pub(super) struct PlaybackOverlayView {
    position_readout: Entity<TransportReadoutView>,
    cpu_meter: Entity<CpuMeterView>,
    output_meter: Entity<OutputMeterView>,
    arrangement_playhead: Entity<ArrangementPlayheadView>,
    piano_roll_playhead: Entity<PianoRollPlayheadView>,
    live_meters: Entity<LiveMetersView>,
}

impl Render for PlaybackOverlayView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().absolute().inset_0().children([
            self.arrangement_playhead.clone().into_any_element(),
            self.piano_roll_playhead.clone().into_any_element(),
            self.live_meters.clone().into_any_element(),
            self.position_readout.clone().into_any_element(),
            self.cpu_meter.clone().into_any_element(),
            self.output_meter.clone().into_any_element(),
        ])
    }
}

/// Thin window root that caches `EditorView` alongside the sibling
/// [`PlaybackOverlayView`] so 60 Hz playback subview updates never re-execute
/// `EditorView::render`.
pub(crate) struct EditorWindowRoot {
    editor: Entity<EditorView>,
    overlay: Entity<PlaybackOverlayView>,
    use_sibling_overlay: Rc<Cell<bool>>,
    overlay_epoch: u64,
    _editor_observer: Subscription,
}

impl EditorWindowRoot {
    pub(crate) fn new(editor: Entity<EditorView>, cx: &mut Context<Self>) -> Self {
        let (overlay, use_sibling_overlay) = editor.update(cx, |view, cx| {
            view.playback_subviews.hosted = true;
            view.refresh_sibling_overlay_flag();
            view.sync_all_playback_subviews(cx, false);
            (
                view.playback_subviews.overlay.clone(),
                Rc::clone(&view.playback_subviews.use_sibling_overlay),
            )
        });
        let _editor_observer = cx.observe(&editor, |root, editor, cx| {
            root.overlay_epoch = root.overlay_epoch.wrapping_add(1);
            editor.update(cx, |view, cx| {
                view.refresh_sibling_overlay_flag();
                view.sync_all_playback_subviews(cx, false);
            });
        });
        Self {
            editor,
            overlay,
            use_sibling_overlay,
            overlay_epoch: 0,
            _editor_observer,
        }
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn editor(&self) -> Entity<EditorView> {
        self.editor.clone()
    }
}

impl Render for EditorWindowRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let use_overlay = self.use_sibling_overlay.get();
        let editor_element: AnyElement = if use_overlay {
            let mut style = div().size_full();
            AnyView::from(self.editor.clone())
                .cached(style.style().clone())
                .into_any_element()
        } else {
            self.editor.clone().into_any_element()
        };
        let mut root = div().size_full().relative().children([editor_element]);
        if use_overlay {
            let mut overlay_style = div().absolute().inset_0();
            root = root.children([AnyView::from(self.overlay.clone())
                .cached_with_epoch(overlay_style.style().clone(), self.overlay_epoch)
                .into_any_element()]);
        }
        root
    }
}

/// Owned by [`EditorView`]; creates and synchronizes all isolated 60 Hz subviews.
pub(super) struct PlaybackSubviews {
    pub(super) slots: Rc<RefCell<PlaybackOverlaySlots>>,
    pub(super) use_sibling_overlay: Rc<Cell<bool>>,
    pub(super) hosted: bool,
    pub(super) position_readout: Entity<TransportReadoutView>,
    pub(super) cpu_meter: Entity<CpuMeterView>,
    pub(super) output_meter: Entity<OutputMeterView>,
    pub(super) arrangement_playhead: Entity<ArrangementPlayheadView>,
    pub(super) piano_roll_playhead: Entity<PianoRollPlayheadView>,
    pub(super) live_meters: Entity<LiveMetersView>,
    pub(super) overlay: Entity<PlaybackOverlayView>,
    tick_activity: Cell<bool>,
}

impl PlaybackSubviews {
    pub(super) fn new(editor: WeakEntity<EditorView>, cx: &mut Context<EditorView>) -> Self {
        let slots = Rc::new(RefCell::new(PlaybackOverlaySlots::default()));
        let use_sibling_overlay = Rc::new(Cell::new(false));
        let position_readout = cx
            .new(|_| TransportReadoutView::new(Rc::clone(&slots), Rc::clone(&use_sibling_overlay)));
        let cpu_meter = cx
            .new(|_| CpuMeterView::new(editor, Rc::clone(&slots), Rc::clone(&use_sibling_overlay)));
        let output_meter =
            cx.new(|_| OutputMeterView::new(Rc::clone(&slots), Rc::clone(&use_sibling_overlay)));
        let arrangement_playhead = cx.new(|_| ArrangementPlayheadView::new(Rc::clone(&slots)));
        let piano_roll_playhead = cx.new(|_| PianoRollPlayheadView::new(Rc::clone(&slots)));
        let live_meters = cx.new(|_| LiveMetersView::new(Rc::clone(&slots)));
        let overlay = cx.new(|_| PlaybackOverlayView {
            position_readout: position_readout.clone(),
            cpu_meter: cpu_meter.clone(),
            output_meter: output_meter.clone(),
            arrangement_playhead: arrangement_playhead.clone(),
            piano_roll_playhead: piano_roll_playhead.clone(),
            live_meters: live_meters.clone(),
        });
        Self {
            slots,
            use_sibling_overlay,
            hosted: false,
            position_readout,
            cpu_meter,
            output_meter,
            arrangement_playhead,
            piano_roll_playhead,
            live_meters,
            overlay,
            tick_activity: Cell::new(false),
        }
    }

    pub(super) fn begin_editor_render(&self) {
        if let Ok(mut slots) = self.slots.try_borrow_mut() {
            slots.begin_render();
        }
    }

    pub(super) fn take_tick_activity(&self) -> bool {
        self.tick_activity.replace(false)
    }

    pub(super) fn mark_tick_activity(&self) {
        self.tick_activity.set(true);
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn subview_render_counts(&self, cx: &gpui::App) -> SubviewRenderCounts {
        SubviewRenderCounts {
            position_readout: self.position_readout.read(cx).render_count,
            cpu_meter: self.cpu_meter.read(cx).render_count,
            output_meter: self.output_meter.read(cx).render_count,
            arrangement_playhead: self.arrangement_playhead.read(cx).render_count,
            piano_roll_playhead: self.piano_roll_playhead.read(cx).render_count,
            live_meters: self.live_meters.read(cx).render_count,
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SubviewRenderCounts {
    pub(super) position_readout: usize,
    pub(super) cpu_meter: usize,
    pub(super) output_meter: usize,
    pub(super) arrangement_playhead: usize,
    pub(super) piano_roll_playhead: usize,
    pub(super) live_meters: usize,
}

fn reading_bits_eq(a: MeterReading, b: MeterReading) -> bool {
    a.level[0].to_bits() == b.level[0].to_bits()
        && a.level[1].to_bits() == b.level[1].to_bits()
        && a.peak_hold[0].to_bits() == b.peak_hold[0].to_bits()
        && a.peak_hold[1].to_bits() == b.peak_hold[1].to_bits()
        && a.clipped == b.clipped
}

fn meter_well_width() -> f32 {
    2.0 * hex_widgets::STEREO_PEAK_BAR_WIDTH + hex_widgets::STEREO_PEAK_BAR_GAP
}

impl EditorView {
    /// True while a nested `ClipContents` / `TrackHeaderCache` gesture is active
    /// so `EditorWindowRoot` renders `EditorView` uncached and GPUI keeps nested
    /// scene caches active without `window.refreshing`.
    pub(super) fn has_active_nested_cache_gesture(&self) -> bool {
        self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.arrangement_pan.is_some()
            || self.active_native_device_drag.is_some()
            || self.time_range_move_gesture.is_some()
            || self.time_selection_gesture.is_some()
            || self.clip_paint_gesture.is_some()
    }

    /// True while a modal, popover, or floating drag ghost paints above the
    /// workspace so live playback subviews yield to inline z-order underneath it.
    pub(super) fn has_foreground_overlay(&self) -> bool {
        self.send_menu_track.is_some()
            || self.context_menu.is_some()
            || self.controller_input_is_open()
            || self.theme_settings.open
            || self.tuning.open
            || self.changelog.is_open()
            || self.missing_media.is_some()
            || super::device_panel::picker_is_open(self)
            || super::device_panel::source_naming(self)
            || self.device_insert_confirmation.is_some()
            || self.update_preparing()
            || self.active_browser_audio_drag.is_some()
            || self.displayed_clip_drag().is_some()
    }

    pub(super) fn uses_sibling_playback_overlay(&self) -> bool {
        self.playback_subviews.hosted
            && !self.has_active_nested_cache_gesture()
            && !self.has_foreground_overlay()
    }

    pub(super) fn refresh_sibling_overlay_flag(&self) {
        self.playback_subviews
            .use_sibling_overlay
            .set(self.uses_sibling_playback_overlay());
    }

    /// Panels that still draw a live playhead inline rather than via the
    /// isolated Arrangement/Piano Roll playhead entities.
    pub(super) fn has_inline_playhead_panel(&self) -> bool {
        if !self.clip_view_open {
            return false;
        }
        if self.device_panel_active {
            // Rattle draws a playhead indicator on its lane when visible.
            return super::device_panel::has_rattle_preview(self);
        }
        // Arrangement/Session audio clip detail editors draw an inline playhead;
        // Piano Roll uses the isolated `PianoRollPlayheadView`.
        self.detail_clip_id().is_some_and(|clip| {
            self.state.as_ref().is_some_and(|state| {
                state.project.clip(clip).is_some()
                    || state.project.session_audio_clip(clip).is_some()
            })
        })
    }

    pub(super) fn sync_cpu_meter_subview(&self, cx: &mut Context<Self>, notify: bool) -> bool {
        let label = self.cpu_meter.label();
        let description = self.cpu_meter.description();
        let fraction = self.cpu_meter.fraction();
        let warning = self.cpu_meter.warning();
        let colors = (
            theme::color(theme::PaletteRole::Primary),
            theme::color(theme::PaletteRole::Secondary),
            theme::color(theme::PaletteRole::Muted),
        );
        let entity = self.playback_subviews.cpu_meter.clone();
        let changed = entity.update(cx, |view, cx| {
            let changed = view.sync(label, description, fraction, warning, colors);
            if changed && notify {
                cx.notify();
            }
            changed
        });
        if changed && notify {
            self.playback_subviews.mark_tick_activity();
        }
        changed
    }

    pub(super) fn sync_output_meter_subview(&self, cx: &mut Context<Self>, notify: bool) -> bool {
        let meter_input = self
            .output_meter_frame
            .map_or(MeterInput::Offline, super::refresh::AudioFrame::meter_input);
        let reading = self.track_meters.main_reading();
        let palette = theme::preferences();
        let entity = self.playback_subviews.output_meter.clone();
        let changed = entity.update(cx, |view, cx| {
            let changed = view.sync(meter_input, reading, palette);
            if changed && notify {
                cx.notify();
            }
            changed
        });
        if changed && notify {
            self.playback_subviews.mark_tick_activity();
        }
        changed
    }

    pub(super) fn sync_playhead_and_position_subviews(
        &self,
        cx: &mut Context<Self>,
        notify: bool,
    ) -> bool {
        let label = self.position_label();
        let text_color = theme::color(theme::PaletteRole::Text);
        let readout_entity = self.playback_subviews.position_readout.clone();
        let readout_changed = readout_entity.update(cx, |view, cx| {
            let changed = view.sync(label, text_color);
            if changed && notify {
                cx.notify();
            }
            changed
        });

        let arrangement_position = self.state.as_ref().map(|_| self.audible_cursor_position());
        let arrangement_entity = self.playback_subviews.arrangement_playhead.clone();
        let arrangement_changed = arrangement_entity.update(cx, |view, cx| {
            let changed = view.sync(arrangement_position, text_color);
            if changed && notify {
                cx.notify();
            }
            changed
        });

        let piano_beat = self.midi_playhead_source_beat();
        let secondary_color = theme::color(theme::PaletteRole::Secondary);
        let piano_entity = self.playback_subviews.piano_roll_playhead.clone();
        let piano_changed = piano_entity.update(cx, |view, cx| {
            let changed = view.sync(piano_beat, secondary_color);
            if changed && notify {
                cx.notify();
            }
            changed
        });

        let any = readout_changed || arrangement_changed || piano_changed;
        if any && notify {
            self.playback_subviews.mark_tick_activity();
        }
        any
    }

    pub(super) fn sync_live_meters_subview(&self, cx: &mut Context<Self>, notify: bool) -> bool {
        let palette = theme::preferences();
        let entity = self.playback_subviews.live_meters.clone();
        let track_meters = &self.track_meters;
        let changed = entity.update(cx, |view, cx| {
            let changed = view.sync_from(track_meters, palette);
            if changed && notify {
                cx.notify();
            }
            changed
        });
        if changed && notify {
            self.playback_subviews.mark_tick_activity();
        }
        changed
    }

    pub(super) fn sync_all_playback_subviews(&self, cx: &mut Context<Self>, notify: bool) -> bool {
        let mut any = false;
        any |= self.sync_playhead_and_position_subviews(cx, notify);
        any |= self.sync_cpu_meter_subview(cx, notify);
        any |= self.sync_output_meter_subview(cx, notify);
        any |= self.sync_live_meters_subview(cx, notify);
        any
    }

    /// Position readout element for the transport center zone.
    pub(super) fn transport_position_element(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.uses_sibling_playback_overlay() {
            let slots = Rc::clone(&self.playback_subviews.slots);
            return div()
                .relative()
                .px(px(4.0))
                .w(px(64.0))
                .max_w(px(82.0))
                .h_full()
                .flex_shrink_0()
                .child(
                    canvas(
                        move |bounds, _, _| {
                            if let Ok(mut slots) = slots.try_borrow_mut() {
                                slots.position_readout = Some(bounds);
                            }
                        },
                        |_, _, _, _| (),
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element();
        }
        self.sync_playhead_and_position_subviews(cx, false);
        self.playback_subviews
            .position_readout
            .clone()
            .into_any_element()
    }

    /// CPU meter element for the transport header diagnostics row.
    pub(super) fn transport_cpu_meter_element(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.uses_sibling_playback_overlay() {
            let slots = Rc::clone(&self.playback_subviews.slots);
            return div()
                .relative()
                .w(px(CPU_METER_WIDTH))
                .h_full()
                .flex_shrink_0()
                .child(
                    canvas(
                        move |bounds, _, _| {
                            if let Ok(mut slots) = slots.try_borrow_mut() {
                                slots.cpu_meter = Some(bounds);
                            }
                        },
                        |_, _, _, _| (),
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element();
        }
        self.sync_cpu_meter_subview(cx, false);
        self.playback_subviews.cpu_meter.clone().into_any_element()
    }

    /// Status-bar stereo output meter element.
    pub(super) fn status_output_meter_element(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.uses_sibling_playback_overlay() {
            let slots = Rc::clone(&self.playback_subviews.slots);
            return div()
                .relative()
                .w(px(OUTPUT_METER_WIDTH))
                .h(px(OUTPUT_METER_HEIGHT))
                .flex_shrink_0()
                .child(
                    canvas(
                        move |bounds, _, _| {
                            if let Ok(mut slots) = slots.try_borrow_mut() {
                                slots.output_meter = Some(bounds);
                            }
                        },
                        |_, _, _, _| (),
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element();
        }
        self.sync_output_meter_subview(cx, false);
        self.playback_subviews
            .output_meter
            .clone()
            .into_any_element()
    }

    /// Arrangement playhead layer (inline entity or no-op when hosted as sibling).
    pub(super) fn arrangement_playhead_element(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.uses_sibling_playback_overlay() {
            return div().into_any_element();
        }
        self.sync_playhead_and_position_subviews(cx, false);
        self.playback_subviews
            .arrangement_playhead
            .clone()
            .into_any_element()
    }

    /// Piano Roll playhead overlay (inline entity or no-op when hosted as sibling).
    pub(super) fn piano_roll_playhead_element(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.uses_sibling_playback_overlay() {
            return div().into_any_element();
        }
        self.sync_playhead_and_position_subviews(cx, false);
        self.playback_subviews
            .piano_roll_playhead
            .clone()
            .into_any_element()
    }

    pub(super) fn meter_slot_or_inline(&self, target: MeterTarget) -> Div {
        if self.uses_sibling_playback_overlay() {
            let slots = Rc::clone(&self.playback_subviews.slots);
            let (height, top_margin) = match target {
                MeterTarget::ArrangementTrack { height, .. } => (height - 6.0, 2.0),
                MeterTarget::ConsoleTrack { height, .. }
                | MeterTarget::Return { height, .. }
                | MeterTarget::Main { height } => ((height - 8.0).clamp(10.0, 42.0), 0.0),
            };
            let mut placeholder = div()
                .relative()
                .w(px(meter_well_width()))
                .h(px(height))
                .flex_shrink_0();
            if top_margin > 0.0 {
                placeholder = placeholder.mt(px(top_margin));
            }
            return placeholder.child(
                canvas(
                    move |bounds, window, _| {
                        if let Ok(mut slots) = slots.try_borrow_mut() {
                            slots.meters.push(MeterSlot {
                                target,
                                bounds,
                                clip_mask: window.content_mask().bounds,
                            });
                        }
                    },
                    |_, _, _, _| (),
                )
                .absolute()
                .inset_0(),
            );
        }
        match target {
            MeterTarget::ArrangementTrack { id, height } => {
                super::track_meters::render_arrangement(id, self.track_meters.reading(id), height)
            }
            MeterTarget::ConsoleTrack { id, height } => div()
                .debug_selector(move || format!("console-track-meter-{}", id.value()))
                .children([super::track_meters::render(
                    id,
                    self.track_meters.reading(id),
                    height,
                )]),
            MeterTarget::Return { id, height } => {
                super::track_meters::render_return(id, self.track_meters.return_reading(id), height)
            }
            MeterTarget::Main { height } => {
                super::track_meters::render_main(self.track_meters.main_reading(), height)
            }
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "playback_subviews_ui_test.rs"]
mod tests;
