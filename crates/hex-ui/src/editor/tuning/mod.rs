//! Native project-scale editor. The worker owns tuning edits and grouped history.
//! Pitch-line pointers use the shared Window lease only while physically held;
//! canonical preview/terminal custody remains with TuningGestureClient until ACK.
mod audition;
mod broadcast;
pub(super) mod library;
#[cfg(all(test, feature = "ui-test-support"))]
mod lifecycle_tests;
mod midi_activity;
mod plots;
#[cfg(all(test, feature = "ui-test-support"))]
mod pointer_tests;
#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests;
use super::EditorView;
use crate::theme;
use gpui::{
    Context, Div, Entity, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Stateful, Subscription, Window, canvas, div, prelude::*, px, rgb,
};
use hex_session::{
    service::{Request, SubmitError},
    tuning_gesture::TuningGestureClient,
};
use hex_tuning::{Audition, ProjectTuning, Scale, Waveform, nearest_lattice};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(super) enum Field {
    Count,
    RootHz,
    RootKey,
    RootDegree,
    Period,
    Level,
    Attack,
    Release,
    Reach,
    Hz(usize),
    Cents(usize),
}
struct Control {
    entity: Entity<NumericControl>,
    _subscription: Subscription,
}
#[derive(Clone, Copy)]
struct Drag {
    degree: usize,
    anchor_x: f32,
    anchor_cents: f32,
}

pub(super) struct TuningState {
    pub open: bool,
    pub(super) broadcast: broadcast::Broadcast,
    audition_keyboard: super::computer_keyboard::ComputerKeyboard,
    pub(super) audition_input:
        Option<std::sync::Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    pub(super) audition_preparing: bool,
    held_keys: [u64; 2],
    midi_keys: [u64; 2],
    midi_last_query: Option<Instant>,
    midi_spans: Option<Box<hex_engine::MidiSpanBuffer>>,
    recent_keys: [Option<Instant>; 128],
    pub client: TuningGestureClient,
    // Ignore the remaining widget events after declining its Begin during preview recovery.
    numeric_begin_refused: bool,
    pub(super) value: ProjectTuning,
    generation: u64,
    focus: FocusHandle,
    controls: HashMap<Field, Control>,
    control_epoch: std::rc::Rc<()>,
    selected: usize,
    pair: usize,
    drag: Option<Drag>,
    pitch_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    palette_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    context: Option<usize>,
    reach: f32,
    variation_seed: u64,
    snap: bool,
    pub(super) shift: bool,
    choices: std::sync::Arc<[Scale]>,
    positions: std::sync::Arc<[(f32, f32)]>,
    analysis_pending: bool,
    analysis_serial: u64,
    analysis_queued: Option<(u64, Scale, f32, u64)>,
    analysis: hex_tuning::analysis::Analysis,
    root_shapes: [(usize, usize, f32); 3],
    last_variations: Instant,
    pub(super) error: Option<String>,
    pub(super) slot: usize,
    pub(super) name: Option<Entity<hex_widgets::TextInput>>,
    pub library_pending: Option<u64>,
    library_terminal: Option<library::DeviceTerminalContext>,
    pub library_next: u64,
    deferred_library: Option<(
        hex_session::tuning_gesture::TuningLibraryOperation,
        Option<(u64, u64)>,
    )>,
    deferred_dialog: Option<bool>,
    slots: Vec<SlotView>,
}
// UI-owned read-only projection; no filesystem dependency or I/O.
#[derive(Clone)]
struct SlotView {
    number: usize,
    name: Option<String>,
    error: Option<String>,
}
impl TuningState {
    pub fn new(cx: &mut Context<EditorView>) -> Self {
        let value = ProjectTuning::default();
        Self {
            open: false,
            broadcast: broadcast::Broadcast::default(),
            audition_keyboard: super::computer_keyboard::ComputerKeyboard::default(),
            audition_input: None,
            audition_preparing: false,
            held_keys: [0; 2],
            midi_keys: [0; 2],
            midi_last_query: None,
            midi_spans: None,
            recent_keys: [None; 128],
            client: TuningGestureClient::default(),
            numeric_begin_refused: false,
            value,
            generation: 0,
            focus: cx.focus_handle(),
            controls: HashMap::new(),
            control_epoch: std::rc::Rc::new(()),
            selected: 0,
            pair: 1,
            drag: None,
            pitch_bounds: None,
            palette_bounds: None,
            context: None,
            reach: 80.0,
            variation_seed: 0,
            snap: false,
            shift: false,
            choices: std::sync::Arc::from([value.scale()]),
            positions: std::sync::Arc::from([(0.5, 0.5)]),
            analysis_pending: false,
            analysis_serial: 0,
            analysis_queued: None,
            analysis: hex_tuning::analysis::Analysis::new(value.scale()),
            root_shapes: hex_tuning::analysis::Analysis::new(value.scale())
                .root_shapes(value.scale()),
            last_variations: Instant::now(),
            error: None,
            slot: 1,
            name: None,
            library_pending: None,
            library_terminal: None,
            library_next: 0,
            deferred_library: None,
            deferred_dialog: None,
            slots: Vec::new(),
        }
    }
}

impl Drop for TuningState {
    fn drop(&mut self) {
        if let Some(input) = &self.audition_input {
            input.cancel();
        }
    }
}
impl Field {
    fn spec(
        self,
        t: ProjectTuning,
        reach: f32,
    ) -> Result<(NumericSpec, f64, String), hex_widgets::NumericError> {
        let s = t.scale();
        let a = t.audition();
        let (min, max, value, precision, label, scale) = match self {
            Self::Count => (
                3.0,
                48.0,
                s.count() as f64,
                0,
                "notes".into(),
                NumericScale::Linear,
            ),
            Self::RootHz => (
                10.0,
                20000.0,
                s.root_hz(),
                3,
                "root hz".into(),
                NumericScale::Logarithmic,
            ),
            Self::RootKey => (
                0.0,
                127.0,
                s.root_key() as f64,
                0,
                "root key".into(),
                NumericScale::Linear,
            ),
            Self::RootDegree => (
                1.0,
                48.0,
                (s.root_degree() + 1) as f64,
                0,
                "root degree".into(),
                NumericScale::Linear,
            ),
            Self::Period => (
                600.0,
                2400.0,
                s.period_cents() as f64,
                2,
                "repeat cents".into(),
                NumericScale::Linear,
            ),
            Self::Level => (
                0.0,
                1.0,
                a.level() as f64,
                3,
                "level".into(),
                NumericScale::Linear,
            ),
            Self::Attack => (
                2.0,
                500.0,
                a.attack_ms() as f64,
                1,
                "attack ms".into(),
                NumericScale::Logarithmic,
            ),
            Self::Release => (
                100.0,
                3000.0,
                a.release_ms() as f64,
                1,
                "release ms".into(),
                NumericScale::Logarithmic,
            ),
            Self::Reach => (
                10.0,
                200.0,
                reach as f64,
                1,
                "reach cents".into(),
                NumericScale::Linear,
            ),
            Self::Hz(i) => (
                2.0,
                80001.0,
                s.degree_frequency(i).unwrap_or(s.root_hz()),
                3,
                format!("node {} hz", i + 1),
                NumericScale::Logarithmic,
            ),
            Self::Cents(i) => (
                0.0,
                2400.0,
                s.cents().get(i).copied().unwrap_or(0.0) as f64,
                2,
                format!("node {} cents", i + 1),
                NumericScale::Linear,
            ),
        };
        let steps = if matches!(self, Self::Count | Self::RootKey | Self::RootDegree) {
            (1.0, 1.0)
        } else if self == Self::Level {
            (0.01, 0.001)
        } else {
            (1.0, 0.01)
        };
        Ok((
            NumericSpec::new(
                min,
                max,
                value,
                scale,
                precision,
                None::<std::sync::Arc<str>>,
            )?
            .with_keyboard_steps(steps.0, steps.1)?,
            value,
            label,
        ))
    }
}
impl EditorView {
    pub(super) fn open_tuning(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tuning.open = true;
        self.tuning.midi_last_query = None;
        self.tuning
            .midi_spans
            .get_or_insert_with(|| Box::new(hex_engine::MidiSpanBuffer::default()));
        self.cancel_computer_keyboard();
        self.tuning.audition_keyboard.enabled = true;
        self.tuning.shift = window.modifiers().shift;
        self.refresh_tuning_variations(cx);
        self.tuning.focus.focus(window);
        self.sync_tuning(cx);
        self.request_tuning_library_list();
        cx.notify();
    }
    pub(super) fn close_tuning(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_tuning_audition();
        let _ = self.tuning.client.cancel();
        self.tuning.drag = None;
        self.tuning.context = None;
        self.tuning.open = false;
        self.reset_tuning_controls(cx);
        self.tuning.analysis_queued = None;
        self.tuning.deferred_library = None;
        self.tuning.deferred_dialog = None;
        self.focus.focus(window);
        cx.notify();
    }
    fn reset_tuning_controls(&mut self, cx: &mut Context<Self>) {
        self.tuning.control_epoch = std::rc::Rc::new(());
        for (
            _,
            Control {
                entity,
                _subscription,
            },
        ) in std::mem::take(&mut self.tuning.controls)
        {
            drop(_subscription);
            entity.update(cx, |input, cx| input.cancel(cx));
        }
    }
    pub(super) fn sync_tuning(&mut self, cx: &mut Context<Self>) {
        let Some(canonical) = self.state.as_ref().map(|state| state.project.tuning()) else {
            return;
        };
        if self.tuning.generation != self.project_generation {
            self.cancel_tuning_audition();
            self.reset_tuning_controls(cx);
            self.tuning.client.reset();
            self.tuning.drag = None;
            self.tuning.name = None;
            self.tuning.library_pending = None;
            self.tuning.library_terminal = None;
            self.tuning.deferred_library = None;
            self.tuning.deferred_dialog = None;
            self.tuning.generation = self.project_generation;
        }
        let value = self
            .tuning
            .client
            .snapshot()
            .map(|s| s.tuning)
            .unwrap_or(canonical);
        if self.tuning.value != value {
            if self.tuning.client.snapshot().is_none() {
                self.reset_tuning_controls(cx);
            }
            let scale_changed = self.tuning.value.scale() != value.scale();
            self.tuning.value = value;
            self.tuning.selected = self.tuning.selected.min(value.scale().count() - 1);
            if scale_changed {
                self.refresh_tuning_variations(cx);
            }
        }
        self.tuning.selected = self.tuning.selected.min(value.scale().count() - 1);
        self.tuning.pair = self.tuning.pair.min(value.scale().count() - 1);
        if !self.tuning.open {
            return;
        }
        let mut fields = vec![
            Field::Count,
            Field::RootHz,
            Field::RootKey,
            Field::RootDegree,
            Field::Period,
            Field::Level,
            Field::Attack,
            Field::Release,
            Field::Reach,
        ];
        for i in 0..value.scale().count() {
            fields.push(Field::Hz(i));
            fields.push(Field::Cents(i));
        }
        for field in fields {
            let Ok((spec, value, label)) = field.spec(value, self.tuning.reach) else {
                continue;
            };
            if let Some(control) = self.tuning.controls.get(&field) {
                control.entity.update(cx, |input, cx| {
                    let _ = input.set_external_value(value, cx);
                    let _ = input.set_external_style(theme::control_style(), cx);
                });
                continue;
            }
            let config = NumericControlConfig::new(
                format!("tuning-{field:?}"),
                theme::text(label),
                NumericControlKind::HorizontalSlider,
                spec,
                value,
                theme::control_style(),
            );
            let Ok(config) = config else { continue };
            let appearance = if matches!(
                field,
                Field::Hz(_) | Field::Cents(_) | Field::Count | Field::RootKey | Field::RootDegree
            ) {
                NumericControlPresentation::InlineValue
            } else {
                NumericControlPresentation::HorizontalSlider
            };
            let entity =
                cx.new(|cx| NumericControl::from_config(cx, config.with_presentation(appearance)));
            entity.update(cx, |input, cx| input.set_tab_navigation(true, cx));
            let generation = self.project_generation;
            let epoch = self.tuning.control_epoch.clone();
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &NumericControlEvent, cx| {
                    if view.tuning.open
                        && view.project_generation == generation
                        && std::rc::Rc::ptr_eq(&view.tuning.control_epoch, &epoch)
                        && !matches!(field, Field::Hz(i) | Field::Cents(i) if i >= view.tuning.value.scale().count())
                    {
                        view.tuning_numeric(field, event.clone(), cx);
                    }
                });
            self.tuning.controls.insert(
                field,
                Control {
                    entity,
                    _subscription: subscription,
                },
            );
        }
        self.bind_tuning_accessibility(cx);
        if self.tuning.client.snapshot().is_none() && self.device_gesture.snapshot().is_none() {
            if let Some((operation, owner)) = self.tuning.deferred_library.take() {
                self.request_tuning_library(operation, owner);
            }
            if let Some(save) = self.tuning.deferred_dialog.take() {
                self.tuning_file_dialog(save, cx);
            }
        }
    }
    fn bind_tuning_accessibility(&self, cx: &mut Context<Self>) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let revision = state.revision;
        let generation = self.project_generation;
        let acknowledged = state.project.tuning();
        let owner = cx.weak_entity();
        let guard: std::rc::Rc<dyn Fn(&gpui::App) -> bool> = std::rc::Rc::new(move |cx| {
            owner
                .read_with(cx, |view, _| {
                    view.tuning.open
                        && view.project_generation == generation
                        && view.state.as_ref().is_some_and(|s| s.revision == revision)
                        && !view.edit_pending()
                        && view.tuning.client.snapshot().is_none()
                })
                .unwrap_or(false)
        });
        for (field, control) in &self.tuning.controls {
            let Ok((_, value, label)) = field.spec(acknowledged, self.tuning.reach) else {
                continue;
            };
            let pending = self.tuning.client.snapshot().is_some()
                || self.edit_pending()
                || !self.tuning.open
                || matches!(field,Field::Hz(i)|Field::Cents(i) if *i>=acknowledged.scale().count());
            let guard = guard.clone();
            control.entity.update(cx, |input, cx| {
                if let Ok(value) = input.accessibility_value(value) {
                    input.set_accessibility_binding(
                        gpui::AccessibilityBinding {
                            revision,
                            name: theme::text(label),
                            value,
                            pending,
                            guard,
                        },
                        cx,
                    );
                }
            });
        }
    }
    pub(in crate::editor) fn finalize_tuning_gesture(&mut self) -> Result<(), &'static str> {
        self.tuning.drag = None;
        if self.tuning.client.has_pending_terminal() {
            return Ok(());
        }
        if let Some(snapshot) = self.tuning.client.snapshot() {
            self.tuning.client.finish(snapshot.tuning)?;
        }
        Ok(())
    }

    pub(in crate::editor) fn refuse_tuning_barriers(&mut self) {
        let effects =
            super::device_gesture::retire_failed_finalize_barriers(&mut self.pending_requests);
        if effects.save_refused {
            self.saving_project = false;
            self.cancel_open_after_save();
        }
        if effects.export_refused {
            self.export_pending = false;
            if let Some(control) = self.export_control.take() {
                control.cancel();
            }
            self.export_window = None;
        }
        if effects.tuning_library_refused {
            self.tuning.library_pending = None;
            self.tuning.library_terminal = None;
        }
        self.refuse_deferred_tuning_writes();
    }

    pub(in crate::editor) fn refuse_deferred_tuning_writes(&mut self) {
        use hex_session::tuning_gesture::TuningLibraryOperation as Operation;
        if matches!(
            self.tuning.deferred_library.as_ref(),
            Some((Operation::Save { .. } | Operation::Export { .. }, _))
        ) {
            self.tuning.deferred_library = None;
        }
        if self.tuning.deferred_dialog == Some(true) {
            self.tuning.deferred_dialog = None;
        }
    }

    pub(super) fn pump_tuning_gesture(&mut self) -> bool {
        let Some(request) = self.tuning.client.next_request() else {
            return false;
        };
        match self.service.submit(Request::TuningGesture(request)) {
            Ok(()) => {
                self.tuning.client.mark_submitted();
                self.worker_submitted();
                true
            }
            Err(SubmitError::Full(_)) => false,
            Err(_) => {
                self.refuse_tuning_barriers();
                self.tuning.client.reset();
                self.error = Some("project worker cannot accept tuning edit".into());
                self.tuning.error = Some("project worker cannot accept tuning edit".into());
                true
            }
        }
    }
    fn tuning_change(&mut self, next: ProjectTuning, phase: NumericEventPhase) {
        if phase == NumericEventPhase::Cancel {
            let _ = self.tuning.client.cancel();
            self.tuning.drag = None;
            return;
        }
        if self.tuning.client.snapshot().is_none() {
            if self.edit_pending() {
                self.tuning.error = Some("wait for pending project changes".into());
                return;
            }
            if let Err(reason) = self
                .tuning
                .client
                .begin(self.project_generation, self.tuning.value)
            {
                self.tuning.error = Some(reason.into());
                return;
            }
        }
        let result = match phase {
            NumericEventPhase::Begin => Ok(0),
            NumericEventPhase::Change => self.tuning.client.change(next),
            NumericEventPhase::Commit => self.tuning.client.finish(next),
            NumericEventPhase::Cancel => Ok(0),
        };
        if let Err(reason) = result {
            self.tuning.error = Some(reason.into());
        } else {
            self.tuning.value = next;
            self.tuning.selected = self.tuning.selected.min(next.scale().count() - 1);
            self.tuning.pair = self.tuning.pair.min(next.scale().count() - 1);
            self.tuning.error = None;
        }
    }
    fn tuning_numeric(&mut self, field: Field, event: NumericControlEvent, cx: &mut Context<Self>) {
        if field == Field::Reach {
            self.tuning.reach = event.value as f32;
            self.refresh_tuning_variations(cx);
            cx.notify();
            return;
        }
        if event.phase == NumericEventPhase::Begin {
            self.tuning.numeric_begin_refused = false;
            if self.tuning.client.is_recovery_pending() {
                self.tuning.numeric_begin_refused = true;
                self.tuning.error = self
                    .tuning
                    .client
                    .begin(self.project_generation, self.tuning.value)
                    .err()
                    .map(str::to_owned);
                cx.notify();
                return;
            }
        } else if self.tuning.numeric_begin_refused {
            return;
        }
        if event.phase == NumericEventPhase::Cancel {
            self.tuning_change(self.tuning.value, event.phase);
            cx.notify();
            return;
        }
        let current = self.tuning.value;
        let mut scale = current.scale();
        let a = current.audition();
        let mut audition = a;
        if matches!(field, Field::Count | Field::RootKey | Field::RootDegree)
            && event.value.fract() != 0.0
        {
            self.tuning.error = Some("enter a whole number".into());
            cx.notify();
            return;
        }
        let edited = match field {
            Field::Count => {
                scale.set_count(event.value as usize);
                Ok(())
            }
            Field::RootHz => {
                scale.set_reference(scale.root_key(), event.value, scale.root_degree())
            }
            Field::RootKey => {
                scale.set_reference(event.value as u8, scale.root_hz(), scale.root_degree())
            }
            Field::RootDegree => scale.set_reference(
                scale.root_key(),
                scale.root_hz(),
                (event.value as usize).saturating_sub(1),
            ),
            Field::Period => {
                let cents = if self.tuning.shift {
                    snap_period(event.value as f32)
                } else {
                    event.value as f32
                };
                scale.set_period(cents);
                if scale.period_cents() == cents {
                    Ok(())
                } else {
                    Err(hex_tuning::Invalid(
                        "repeat period cannot preserve valid node spacing",
                    ))
                }
            }
            Field::Hz(i) => {
                if i == scale.root_degree() && !(10.0..=20_000.0).contains(&event.value) {
                    Err(hex_tuning::Invalid("root frequency must be 10 to 20000 hz"))
                } else {
                    scale.set_degree_frequency(i, event.value);
                    if self.tuning.snap {
                        let cents =
                            snap_node_cents(scale.cents()[i], scale.cents()[scale.root_degree()]);
                        scale.move_degree(i, cents);
                    }
                    self.tuning.selected = i;
                    Ok(())
                }
            }
            Field::Cents(i) => {
                let cents = if self.tuning.snap {
                    snap_node_cents(event.value as f32, scale.cents()[scale.root_degree()])
                } else {
                    event.value as f32
                };
                scale.move_degree(i, cents);
                self.tuning.selected = i;
                Ok(())
            }
            Field::Level | Field::Attack | Field::Release => Audition::new(
                a.waveform(),
                if field == Field::Level {
                    event.value as f32
                } else {
                    a.level()
                },
                if field == Field::Attack {
                    event.value as f32
                } else {
                    a.attack_ms()
                },
                if field == Field::Release {
                    event.value as f32
                } else {
                    a.release_ms()
                },
            )
            .map(|v| audition = v),
            Field::Reach => Ok(()),
        };
        match edited.and_then(|()| ProjectTuning::new(current.enabled(), scale, audition)) {
            Ok(next) => {
                self.tuning_change(next, event.phase);
                if event.phase == NumericEventPhase::Commit {
                    self.refresh_tuning_variations(cx);
                }
            }
            Err(reason) => self.tuning.error = Some(reason.to_string()),
        }
        cx.notify();
    }
    fn tuning_scale_action(&mut self, action: usize, cx: &mut Context<Self>) {
        let mut s = self.tuning.value.scale();
        match action {
            0 => s.set_equal(s.count()),
            1 => s.set_just_major(),
            2 => {
                let i = self.tuning.selected;
                let cents = snap_node_cents(s.cents()[i], s.cents()[s.root_degree()]);
                s.move_degree(i, cents);
            }
            3 => {
                if let Some(i) = s.insert_degree_after(self.tuning.selected) {
                    self.tuning.selected = i;
                } else {
                    self.tuning.error = Some("no room for another node".into());
                    return;
                }
            }
            4 => {
                if !s.remove_degree(self.tuning.selected) {
                    self.tuning.error = Some("keep the root and at least three nodes".into());
                    return;
                }
                self.tuning.selected = self.tuning.selected.min(s.count() - 1);
            }
            _ => return,
        }
        if let Ok(next) =
            ProjectTuning::new(self.tuning.value.enabled(), s, self.tuning.value.audition())
        {
            self.tuning_change(next, NumericEventPhase::Commit);
            self.refresh_tuning_variations(cx);
        }
        self.tuning.context = None;
        cx.notify();
    }
    fn regenerate_tuning_variations(&mut self, cx: &mut Context<Self>) {
        self.tuning.variation_seed = self.tuning.variation_seed.wrapping_add(1);
        self.refresh_tuning_variations(cx);
    }
    fn refresh_tuning_variations(&mut self, cx: &mut Context<Self>) {
        self.tuning.analysis_serial = self.tuning.analysis_serial.saturating_add(1);
        self.tuning.analysis_queued = Some((
            self.tuning.analysis_serial,
            self.tuning.value.scale(),
            self.tuning.reach,
            self.tuning.variation_seed,
        ));
        self.tuning.last_variations = Instant::now();
        self.start_tuning_analysis(cx);
    }
    fn start_tuning_analysis(&mut self, cx: &mut Context<Self>) {
        if self.tuning.analysis_pending {
            return;
        }
        let Some((serial, scale, reach, seed)) = self.tuning.analysis_queued.take() else {
            return;
        };
        self.tuning.analysis_pending = true;
        let generation = self.project_generation;
        let task = cx.background_executor().spawn(async move {
            let choices = plots::variations(scale, reach, seed);
            let positions = plots::positions(&choices);
            let analysis = hex_tuning::analysis::Analysis::new(scale);
            let root_shapes = analysis.root_shapes(scale);
            (choices, positions, analysis, root_shapes)
        });
        cx.spawn(async move |this, cx| {
            let (choices, positions, analysis, root_shapes) = task.await;
            let _ = this.update(cx, |view, cx| {
                view.tuning.analysis_pending = false;
                if view.tuning.open
                    && view.project_generation == generation
                    && view.tuning.analysis_serial == serial
                {
                    view.tuning.choices = choices.into();
                    view.tuning.positions = positions.into();
                    view.tuning.analysis = analysis;
                    view.tuning.root_shapes = root_shapes;
                }
                view.start_tuning_analysis(cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn capture_tuning_pointer(&self, window: &mut Window, cx: &Context<Self>) -> bool {
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        let generation = self.project_generation;
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            gpui::CursorStyle::ResizeLeftRight,
            move |event, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    if view.project_generation != generation || view.tuning.drag.is_none() {
                        return;
                    }
                    // Focus may change before the next completed paint/blur.
                    // Do not let that press finish a now-unfocused edit.
                    if !view.tuning.open || !view.tuning.focus.is_focused(window) {
                        view.tuning_change(view.tuning.value, NumericEventPhase::Cancel);
                        cx.notify();
                        return;
                    }
                    if let Some(event) = event.downcast_ref::<MouseMoveEvent>() {
                        view.tuning_pointer_move(event, cx);
                    } else if let Some(event) = event.downcast_ref::<MouseUpEvent>()
                        && event.button == MouseButton::Left
                    {
                        view.tuning_pointer_up(event, cx);
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.project_generation == generation && view.tuning.drag.is_some() {
                        view.tuning_change(view.tuning.value, NumericEventPhase::Cancel);
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    fn tuning_pointer_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.tuning.pitch_bounds else {
            return;
        };
        let s = self.tuning.value.scale();
        let x = f32::from(event.position.x);
        let left = f32::from(bounds.left());
        let width = f32::from(bounds.size.width).max(1.0);
        let cents = ((x - left) / width).clamp(0.0, 1.0) * s.period_cents();
        let closest = (0..s.count()).min_by(|a, b| {
            (s.cents()[*a] - cents)
                .abs()
                .total_cmp(&(s.cents()[*b] - cents).abs())
        });
        let hit =
            closest.filter(|i| (s.cents()[*i] - cents).abs() / s.period_cents() * width < 10.0);
        if event.button == MouseButton::Right {
            if let Some(index) = hit {
                self.tuning.selected = index;
            }
            self.tuning.context = hit;
            cx.notify();
            return;
        }
        if self.tuning.client.is_recovery_pending() {
            self.tuning.error = self
                .tuning
                .client
                .begin(self.project_generation, self.tuning.value)
                .err()
                .map(str::to_owned);
            self.tuning.drag = None;
            cx.notify();
            return;
        }
        if self.tuning.client.snapshot().is_none() && self.edit_pending() {
            self.tuning.error = Some("wait for pending project changes".into());
            cx.notify();
            return;
        }
        if self.tuning.client.has_pending_terminal() {
            // The admitted terminal keeps canonical custody, but no pointer.
            return;
        }
        self.tuning.focus.focus(window);
        if event.click_count >= 2 {
            if let Some(i) = hit {
                self.tuning.selected = i;
                self.tuning_scale_action(4, cx);
                return;
            }
            let mut next = s;
            if let Some(i) = next.insert_degree(cents) {
                self.tuning.selected = i;
                if let Ok(tuning) = ProjectTuning::new(
                    self.tuning.value.enabled(),
                    next,
                    self.tuning.value.audition(),
                ) {
                    if !self.capture_tuning_pointer(window, cx) {
                        return;
                    }
                    self.tuning_change(tuning, NumericEventPhase::Change);
                    self.tuning.drag = Some(Drag {
                        degree: i,
                        anchor_x: x,
                        anchor_cents: cents,
                    });
                }
            }
        } else if let Some(i) = hit {
            if !self.capture_tuning_pointer(window, cx) {
                return;
            }
            self.tuning.selected = i;
            self.tuning.pair = s.root_degree();
            self.tuning_change(self.tuning.value, NumericEventPhase::Begin);
            self.tuning.drag = Some(Drag {
                degree: i,
                anchor_x: x,
                anchor_cents: s.cents()[i],
            });
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn tuning_pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let (Some(drag), Some(bounds)) = (self.tuning.drag, self.tuning.pitch_bounds) else {
            return;
        };
        let mut s = self.tuning.value.scale();
        let speed = if event.modifiers.shift { 0.1 } else { 1.0 };
        let cents = drag.anchor_cents
            + (f32::from(event.position.x) - drag.anchor_x) / f32::from(bounds.size.width).max(1.0)
                * s.period_cents()
                * speed;
        let cents = if self.tuning.snap {
            snap_node_cents(cents, s.cents()[s.root_degree()])
        } else {
            cents
        };
        s.move_degree(drag.degree, cents);
        if let Ok(next) =
            ProjectTuning::new(self.tuning.value.enabled(), s, self.tuning.value.audition())
        {
            self.tuning_change(next, NumericEventPhase::Change);
        }
        if self.tuning.last_variations.elapsed() > Duration::from_millis(80) {
            self.refresh_tuning_variations(cx);
        }
        cx.notify();
    }
    fn tuning_pointer_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.tuning.drag.take().is_some() {
            self.tuning_change(self.tuning.value, NumericEventPhase::Commit);
            self.refresh_tuning_variations(cx);
            cx.notify();
        }
    }
    fn tuning_control(&self, field: Field) -> Div {
        let label = field
            .spec(self.tuning.value, self.tuning.reach)
            .ok()
            .map(|(_, _, label)| label)
            .unwrap_or_default();
        let mut row = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(25.0))
            .child(div().w(px(92.0)).child(theme::text(label)));
        if let Some(control) = self.tuning.controls.get(&field) {
            row = row.child(div().flex_1().min_w_0().child(control.entity.clone()));
        }
        row
    }
    pub(super) fn tuning_overlay(&mut self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if !self.tuning.open {
            return None;
        }
        let s = self.tuning.value.scale();
        let selected = self.tuning.selected;
        let mut tuning_nodes = div()
            .id("tuning-nodes")
            .flex()
            .flex_col()
            .gap_1()
            .overflow_y_scroll()
            .flex_1()
            .min_h_0();
        for i in 0..s.count() {
            let mut row = div()
                .id(("tuning-node", i))
                .flex()
                .items_center()
                .gap_1()
                .px_1()
                .py_1()
                .bg(rgb(theme::color(if i == selected {
                    theme::PaletteRole::Raised
                } else {
                    theme::PaletteRole::Background
                })))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |view, _, _, cx| {
                        view.tuning.selected = i;
                        view.tuning.context = Some(i);
                        cx.notify();
                    }),
                )
                .child(
                    button(format!("{:02}", i + 1), i == selected).on_click(cx.listener(
                        move |view, _, _, cx| {
                            view.tuning.pair = view.tuning.selected;
                            view.tuning.selected = i;
                            cx.notify();
                        },
                    )),
                );
            for field in [Field::Hz(i), Field::Cents(i)] {
                if let Some(control) = self.tuning.controls.get(&field) {
                    row = row.child(div().w(px(82.0)).child(control.entity.clone()));
                }
            }
            tuning_nodes = tuning_nodes.child(row);
        }
        let mut tuning_reference = tuning_section("reference");
        for field in [
            Field::RootDegree,
            Field::RootHz,
            Field::RootKey,
            Field::Period,
        ] {
            tuning_reference = tuning_reference.child(self.tuning_control(field));
        }
        tuning_reference = tuning_reference.child(
            button("snap ratios", self.tuning.snap).on_click(cx.listener(|view, _, _, cx| {
                view.tuning.snap = !view.tuning.snap;
                cx.notify();
            })),
        );
        let mut audition = tuning_section("audition");
        for (label, wave) in [
            ("triangle", Waveform::Triangle),
            ("square", Waveform::Square),
            ("saw", Waveform::Saw),
        ] {
            audition = audition.child(
                button(label, self.tuning.value.audition().waveform() == wave).on_click(
                    cx.listener(move |view, _, _, cx| {
                        let a = view.tuning.value.audition();
                        if let Ok(a) = Audition::new(wave, a.level(), a.attack_ms(), a.release_ms())
                        {
                            if let Ok(next) = ProjectTuning::new(
                                view.tuning.value.enabled(),
                                view.tuning.value.scale(),
                                a,
                            ) {
                                view.tuning_change(next, NumericEventPhase::Commit);
                            }
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        for field in [Field::Level, Field::Attack, Field::Release] {
            audition = audition.child(self.tuning_control(field));
        }
        audition = audition.child(theme::text("a–; play · z/x octave · c/v velocity"));
        let pitch = div()
            .id("tuning-pitch-line")
            .h(px(110.0))
            .w_full()
            .relative()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::tuning_pointer_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::tuning_pointer_down))
            .child(
                canvas(
                    {
                        let owner = cx.entity();
                        move |bounds, _, cx| {
                            owner.update(cx, |view, _| view.tuning.pitch_bounds = Some(bounds));
                        }
                    },
                    {
                        let activity =
                            std::array::from_fn(|degree| self.tuning.degree_activity(degree));
                        let owner = cx.entity();
                        move |bounds, _, window, cx| {
                            let view = owner.read(cx);
                            if view.tuning.drag.is_some()
                                && view.tuning.open
                                && view.tuning.focus.is_focused(window)
                            {
                                window.renew_pointer_capture(
                                    owner.entity_id(),
                                    gpui::CursorStyle::ResizeLeftRight,
                                );
                            }
                            plots::pitch(bounds, s, selected, activity, window, cx)
                        }
                    },
                )
                .size_full(),
            );
        let palette = plots::palette(self, cx);
        let coordinates = self.tuning.analysis.coordinates;
        let root_shapes = if self.tuning.analysis_pending {
            String::new()
        } else {
            self.tuning
                .root_shapes
                .iter()
                .map(|(a, b, score)| {
                    format!(
                        "{} · {} · {}  {:.3}",
                        s.root_degree() + 1,
                        a + 1,
                        b + 1,
                        score
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let main = div()
            .id("tuning-plots-scroll")
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .flex_1()
            .min_w_0()
            .child(tuning_section("pitch line").child(pitch))
            .child(
                tuning_section("pitch space")
                    .child(palette)
                    .child(self.tuning_control(Field::Reach))
                    .child(
                        button("dice", false).on_click(cx.listener(|view, _, _, cx| {
                            view.regenerate_tuning_variations(cx);
                            cx.notify();
                        })),
                    ),
            )
            .child(
                tuning_section("relationships")
                    .child(plots::lattice(self, cx))
                    .child(theme::text(if self.tuning.analysis_pending {
                        "updating relationships…".to_owned()
                    } else {
                        format!(
                            "cohesion {:.3} · root affinity {:.3} · energy {:.3}",
                            coordinates.cohesion, coordinates.root_gravity, coordinates.energy
                        )
                    }))
                    .child(theme::text(format!("root triads\n{root_shapes}"))),
            );
        let mut settings = div().flex().gap_1();
        for (label, action) in [
            ("equal divisions", 0),
            ("just major", 1),
            ("snap selected", 2),
        ] {
            settings =
                settings.child(button(label, false).on_click(
                    cx.listener(move |view, _, _, cx| view.tuning_scale_action(action, cx)),
                ));
        }
        let mut left = tuning_section("nodes · hz / cents")
            .w(px(230.0))
            .h_full()
            .child(tuning_nodes);
        if self.tuning.context.is_some() {
            left = left
                .child(
                    button("add node after", false)
                        .on_click(cx.listener(|view, _, _, cx| view.tuning_scale_action(3, cx))),
                )
                .child(
                    button("remove node", false)
                        .on_click(cx.listener(|view, _, _, cx| view.tuning_scale_action(4, cx))),
                );
        }
        let right = div()
            .id("tuning-reference-scroll")
            .w(px(285.0))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.tuning_mts_controls(cx))
            .child(tuning_reference)
            .child(
                tuning_section("slots & structure")
                    .child(self.tuning_control(Field::Count))
                    .child(settings),
            )
            .child(audition)
            .child(self.tuning_library_panel(cx));
        let mut content = div()
            .id("tuning-editor")
            .debug_selector(|| "tuning-editor".into())
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .text_size(px(11.0))
            .font_family(theme::FONT_FAMILY)
            .track_focus(&self.tuning.focus)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(theme::text("project tuning · freqs"))
                    .child(
                        button(
                            if self.tuning.value.enabled() {
                                "enabled"
                            } else {
                                "enable tuning"
                            },
                            self.tuning.value.enabled(),
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.tuning_change(
                                view.tuning.value.with_enabled(!view.tuning.value.enabled()),
                                NumericEventPhase::Commit,
                            );
                            cx.notify();
                        })),
                    )
                    .child(
                        button("undo", false).on_click(cx.listener(|view, _, _, cx| {
                            view.enqueue(Request::Undo);
                            cx.notify();
                        })),
                    )
                    .child(
                        button("redo", false).on_click(cx.listener(|view, _, _, cx| {
                            view.enqueue(Request::Redo);
                            cx.notify();
                        })),
                    )
                    .child(div().flex_1())
                    .child(button("close", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_tuning(window, cx)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .flex_1()
                    .min_h_0()
                    .child(left)
                    .child(main)
                    .child(right),
            );
        if let Some(error) = self.tuning.error.as_ref() {
            content = content.child(theme::text(error.clone()));
        }
        Some(
            div()
                .id("tuning-overlay")
                .absolute()
                .inset_0()
                .pt(px(super::header::TITLE_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::Background)))
                .child(content),
        )
    }
}
fn tuning_section(label: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .rounded(px(3.0))
        .child(
            div()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text(label)),
        )
}
fn button(label: impl Into<String>, active: bool) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(gpui::ElementId::Name(label.clone().into()))
        .px_2()
        .py_1()
        .cursor_pointer()
        .rounded(px(3.0))
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(if active {
            theme::PaletteRole::Primary
        } else {
            theme::PaletteRole::Raised
        })))
        .child(theme::text(label))
}
fn snap_period(cents: f32) -> f32 {
    [
        1200.0 * (3.0_f32 / 2.0).log2(),
        1200.0,
        1200.0 * (5.0_f32 / 2.0).log2(),
        1200.0 * 3.0_f32.log2(),
        2400.0,
    ]
    .into_iter()
    .min_by(|a, b| (a - cents).abs().total_cmp(&(b - cents).abs()))
    .unwrap_or(cents)
}

fn snap_node_cents(cents: f32, root: f32) -> f32 {
    let (_, error) = nearest_lattice(cents - root);
    cents - error
}

#[cfg(test)]
mod snap_tests {
    use super::*;
    #[test]
    fn node_snap_keeps_root_and_octave_position_and_period_has_its_own_landmarks() {
        let fifth = 1200.0 * (3.0_f32 / 2.0).log2();
        assert!((snap_node_cents(1900.0, 0.0) - (1200.0 + fifth)).abs() < 0.01);
        assert!((snap_node_cents(1000.0, 300.0) - (300.0 + fifth)).abs() < 0.01);
        assert_eq!(snap_node_cents(300.0, 300.0), 300.0);
        assert_eq!(snap_period(1180.0), 1200.0);
        assert_eq!(snap_period(2390.0), 2400.0);
    }
}
