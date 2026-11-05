//! Audio-free EQ catalog using the production response graph and its shared
//! axes, sampling, selection, node geometry, group movement and slope rules.
//! The fixture owns disposable EQ state and local history; it never writes a
//! project or acquires audio resources. Spectrum layers are labeled static
//! fixtures, and audition is unavailable. The fixture does not model live
//! audio or production worker acknowledgments.
use super::*;
use hex_devices::eq::{self, EqState, Node, NodeId, NodeKind};
use hex_widgets::{
    ResponseGraphEvent, ResponseGraphMarqueeEvent, ResponseGraphMarqueePhase, ResponseGraphPhase,
    ResponseGraphScrollEvent, ResponseGraphScrollPhase,
};
#[path = "../../src/editor/device_panel/eq_panel/axes.rs"]
mod axes;
#[path = "../../src/editor/device_panel/eq_panel/group_move.rs"]
mod group_move;
#[path = "../../src/editor/device_panel/eq_panel/node_geometry.rs"]
mod node_geometry;
#[path = "../../src/editor/device_panel/eq_panel/sampling.rs"]
mod sampling;
#[path = "../../src/editor/device_panel/eq_panel/selection.rs"]
mod selection;
#[path = "../../src/editor/device_panel/eq_panel/slope.rs"]
mod slope;
use hex_widgets::{
    NumericControlEvent, NumericEventPhase, NumericGestureId, SelectorChanged, ToggleChanged,
};
use std::collections::VecDeque;

const HISTORY_CAPACITY: usize = 100;
const PENDING_ACTION_CAPACITY: usize = 256;
const SPECTRUM_TOGGLE: usize = 6;
const INPUT_TOGGLE: usize = 7;
pub(super) const AUDITION_TOGGLE: usize = 8;

/// Ephemeral catalog state; it never owns a project or audio resource.
pub(super) struct Local {
    state: EqState,
    selected: Option<NodeId>,
    bank: u8,
    group: Vec<NodeId>,
    collapse: Option<NodeId>,
    before: Option<Snapshot>,
    group_drag: Option<GroupDrag>,
    option_drag: Option<(Node, f32)>,
    scroll: Option<LocalScroll>,
    created: Option<NodeId>,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    numeric_before: Option<(usize, Option<NumericGestureId>, HistorySnapshot)>,
    finalized_numeric: Vec<NumericGestureId>,
    pending_numeric: Vec<(usize, f64, f64)>,
    pending_actions: Vec<PendingAction>,
    pending_overflow: bool,
    flush_scheduled: bool,
}

enum PendingAction {
    Graph(ResponseGraphEvent),
    Marquee(ResponseGraphMarqueeEvent),
    Scroll(ResponseGraphScrollEvent),
    Bank(usize, bool),
    History(bool),
}

#[derive(Clone)]
struct HistorySnapshot {
    state: EqState,
    selected: Option<NodeId>,
    group: Vec<NodeId>,
    bank: u8,
}

struct HistoryEntry {
    before: HistorySnapshot,
    after: HistorySnapshot,
}

struct Snapshot {
    state: EqState,
    selected: Option<NodeId>,
    group: Vec<NodeId>,
}

struct GroupDrag {
    origin: [f32; 2],
    nodes: Vec<(Node, [f32; 2])>,
}

struct LocalScroll {
    node: NodeId,
    stream: u64,
    value: u32,
    last_steps: i32,
    before: Snapshot,
}

impl Local {
    fn new() -> Result<Self, String> {
        let id = NodeId::new(1).map_err(|e| e.to_string())?;
        let mut state = EqState::default();
        state
            .add(Node::new(id, 1_000.0, 6.0, 0).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            state,
            selected: Some(id),
            bank: 0,
            group: Vec::new(),
            collapse: None,
            before: None,
            group_drag: None,
            option_drag: None,
            scroll: None,
            created: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
            numeric_before: None,
            finalized_numeric: Vec::new(),
            pending_numeric: Vec::new(),
            pending_actions: Vec::new(),
            pending_overflow: false,
            flush_scheduled: false,
        })
    }

    fn history_snapshot(&self) -> HistorySnapshot {
        HistorySnapshot {
            state: self.state.clone(),
            selected: self.selected,
            group: self.group.clone(),
            bank: self.bank,
        }
    }

    fn record(&mut self, before: HistorySnapshot) {
        let after = self.history_snapshot();
        if before.state == after.state {
            return;
        }
        if self.undo.len() == HISTORY_CAPACITY {
            self.undo.pop_front();
        }
        self.undo.push_back(HistoryEntry { before, after });
        self.redo.clear();
    }
}

pub(super) fn has_selected(fixture: &Fixture) -> bool {
    fixture
        .eq_local
        .as_ref()
        .is_some_and(|local| local.selected.is_some())
}

pub(super) fn fixture(cx: &mut Context<Gallery>) -> Result<Fixture, String> {
    let mut subscriptions = Vec::new();
    let mut numbers = Vec::new();
    for (index, (label, min, max, value, scale, unit)) in [
        (
            "out",
            0.0,
            4.0,
            1.0,
            NumericScale::LinearAmplitudeDecibels,
            "db",
        ),
        (
            "freq",
            20.0,
            20000.0,
            1000.0,
            NumericScale::Logarithmic,
            "hz",
        ),
        ("gain", -24.0, 24.0, 6.0, NumericScale::Bipolar, "db"),
        ("q", 0.1, 18.0, 0.707, NumericScale::Logarithmic, ""),
    ]
    .into_iter()
    .enumerate()
    {
        let width = if index == 0 { 58.0 } else { 104.0 };
        let config = NumericControlConfig::new(
            format!("fixture-eq-numeric-{index}"),
            theme::text(label),
            NumericControlPresentation::InlineValue,
            NumericSpec::new(min, max, value, scale, 2, Some(unit)).map_err(|e| e.to_string())?,
            value,
            theme::device_style(theme::DeviceStyleRole::Readout),
        )
        .map_err(|e| e.to_string())?;
        let entity = cx.new(|cx| {
            let mut control = NumericControl::from_config(cx, config);
            let _ = control.set_inline_value_width(width, cx);
            control.set_tab_navigation(true, cx);
            control
        });
        subscriptions.push(cx.subscribe(
            &entity,
            move |view, _, event: &NumericControlEvent, cx| {
                if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq) {
                    numeric_event(fixture, index, event.clone());
                }
                cx.notify();
            },
        ));
        numbers.push(Number {
            entity,
            role: theme::DeviceStyleRole::Readout,
            width,
            height: 14.0,
        });
    }
    let mut toggles = Vec::new();
    for (index, label) in [
        "on", "st", "l", "r", "m", "s", "spectrum", "input", "listen",
    ]
    .into_iter()
    .enumerate()
    {
        let config = ToggleConfig::new(
            format!("fixture-eq-toggle-{index}"),
            theme::text(label),
            matches!(index, 0 | 1 | 6 | 7),
            theme::device_style(theme::DeviceStyleRole::Control),
        )
        .map_err(str::to_owned)?
        .with_appearance(ToggleAppearance::CompactButton);
        let entity = cx.new(|cx| Toggle::from_config(cx, config));
        subscriptions.push(
            cx.subscribe(&entity, move |view, _, event: &ToggleChanged, cx| {
                if index == 0
                    && let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                {
                    discrete_event(fixture, DiscreteEdit::Enabled(event.checked));
                }
                if (1..=5).contains(&index) && event.checked && !view.finish_drafts(cx) {
                    if let Some(fixture) = view.fixtures.get(&Entry::Eq) {
                        let bank = fixture
                            .eq_local
                            .as_ref()
                            .map_or(0, |local| usize::from(local.bank));
                        for other in 1..=5 {
                            fixture.toggles[other]
                                .update(cx, |toggle, cx| toggle.set_checked(other == bank + 1, cx));
                        }
                    }
                    cx.notify();
                    return;
                }
                if (1..=5).contains(&index)
                    && let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                    && defer_action(fixture, PendingAction::Bank(index, event.checked))
                {
                    cx.notify();
                    return;
                }
                if (1..=5).contains(&index)
                    && let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                {
                    if event.checked {
                        if let Err(error) = select_bank(fixture, (index - 1) as u8, cx) {
                            view.error = Some(error);
                            let bank = fixture
                                .eq_local
                                .as_ref()
                                .map_or(0, |local| usize::from(local.bank));
                            for other in 1..=5 {
                                fixture.toggles[other].update(cx, |toggle, cx| {
                                    toggle.set_checked(other == bank + 1, cx)
                                });
                            }
                            cx.notify();
                            return;
                        }
                        for other in 1..=5 {
                            if other != index {
                                fixture.toggles[other]
                                    .update(cx, |toggle, cx| toggle.set_checked(false, cx));
                            }
                        }
                    } else if fixture
                        .eq_local
                        .as_ref()
                        .is_some_and(|local| usize::from(local.bank) + 1 == index)
                    {
                        fixture.toggles[index]
                            .update(cx, |toggle, cx| toggle.set_checked(true, cx));
                    }
                }
                cx.notify();
            }),
        );
        toggles.push(entity);
    }
    let mut selectors = Vec::new();
    for (index, labels) in [
        vec!["stereo", "left/right", "mid/side"],
        vec![
            "bell",
            "low shelf",
            "high shelf",
            "low cut",
            "high cut",
            "notch",
        ],
        vec!["12 db/oct", "24 db/oct", "36 db/oct", "48 db/oct"],
    ]
    .into_iter()
    .enumerate()
    {
        let config = SelectorConfig::new(
            format!("fixture-eq-selector-{index}"),
            labels.into_iter().map(theme::text),
            0,
            theme::device_style(theme::DeviceStyleRole::Control),
        )
        .map_err(str::to_owned)?
        .with_appearance(SelectorAppearance::CompactSelector);
        let entity = cx.new(|cx| Selector::from_config(cx, config));
        subscriptions.push(
            cx.subscribe(&entity, move |view, _, event: &SelectorChanged, cx| {
                if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq) {
                    discrete_event(fixture, DiscreteEdit::Selector(index, event.index));
                }
                cx.notify();
            }),
        );
        selectors.push(entity);
    }
    let graph = cx.new(|cx| {
        let mut graph = ResponseGraph::new(
            cx,
            "fixture-eq-graph",
            theme::device_style(theme::DeviceStyleRole::Display),
        );
        graph.set_editable(true, cx);
        graph.set_creation_enabled(true);
        graph.set_creation_drag_enabled(true);
        graph.set_backspace_removal(true);
        graph.set_marquee_enabled(true, cx);
        graph
    });
    subscriptions.push(
        cx.subscribe(&graph, |view, _, event: &ResponseGraphEvent, cx| {
            let cancel = event.phase == ResponseGraphPhase::Cancel;
            if !cancel && (view.disabled || view.entry != Entry::Eq) {
                return;
            }
            if matches!(
                event.phase,
                ResponseGraphPhase::Create
                    | ResponseGraphPhase::Select
                    | ResponseGraphPhase::Remove
            ) && !view.finish_drafts(cx)
            {
                if let Some(fixture) = view.fixtures.get(&Entry::Eq) {
                    fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
                }
                cx.notify();
                return;
            }
            if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq) {
                if defer_action(fixture, PendingAction::Graph(*event)) {
                    return;
                }
                if let Err(error) = event_graph(fixture, *event, cx) {
                    fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
                    view.error = Some(error);
                }
            }
            cx.notify();
        }),
    );
    subscriptions.push(
        cx.subscribe(&graph, |view, _, event: &ResponseGraphMarqueeEvent, cx| {
            let cancel = event.phase == ResponseGraphMarqueePhase::Cancel;
            if !cancel && (view.disabled || view.entry != Entry::Eq) {
                return;
            }
            if !cancel && !view.finish_drafts(cx) {
                if let Some(fixture) = view.fixtures.get(&Entry::Eq) {
                    fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
                }
                cx.notify();
                return;
            }
            if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq) {
                if defer_action(fixture, PendingAction::Marquee(event.clone())) {
                    return;
                }
                if let Err(error) = marquee(fixture, event, cx) {
                    view.error = Some(error);
                }
            }
            cx.notify();
        }),
    );
    subscriptions.push(
        cx.subscribe(&graph, |view, _, event: &ResponseGraphScrollEvent, cx| {
            let cancel = event.phase == ResponseGraphScrollPhase::Cancel;
            if !cancel && (view.disabled || view.entry != Entry::Eq) {
                return;
            }
            if event.phase == ResponseGraphScrollPhase::Begin && !view.finish_drafts(cx) {
                if let Some(fixture) = view.fixtures.get(&Entry::Eq) {
                    fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
                }
                cx.notify();
                return;
            }
            if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq) {
                if defer_action(fixture, PendingAction::Scroll(*event)) {
                    return;
                }
                if let Err(error) = scroll(fixture, *event, cx) {
                    fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
                    view.error = Some(error);
                }
            }
            cx.notify();
        }),
    );
    let mut fixture = Fixture {
        entry: Entry::Eq,
        _subscriptions: subscriptions,
        numbers,
        toggles,
        selectors,
        graph,
        eq_local: Some(Local::new()?),
        discrete_height: 14.0,
    };
    if let Some(state) = current(&fixture, cx)
        && let Some(local) = fixture.eq_local.as_mut()
    {
        local.state = state;
    }
    Ok(fixture)
}

fn current(fixture: &Fixture, cx: &gpui::App) -> Option<EqState> {
    let local = fixture.eq_local.as_ref()?;
    let mut state = local.state.clone();
    let value = |i: usize| fixture.numbers[i].entity.read(cx).value() as f32;
    state.mode = fixture.selectors[0].read(cx).selected() as u8;
    state.output_gain = value(0);
    if let Some(node) = local
        .selected
        .and_then(|id| state.nodes.iter_mut().find(|n| n.id == id))
    {
        node.frequency_hz = value(1);
        node.gain_db = value(2);
        node.q = value(3);
        node.slope = fixture.selectors[2].read(cx).selected() as u8;
        node.enabled = fixture.toggles[0].read(cx).checked();
        node.kind = [
            NodeKind::Bell,
            NodeKind::LowShelf,
            NodeKind::HighShelf,
            NodeKind::LowCut,
            NodeKind::HighCut,
            NodeKind::Notch,
        ][fixture.selectors[1].read(cx).selected()];
    }
    Some(state)
}

fn set_numeric(state: &mut EqState, selected: Option<NodeId>, index: usize, value: f64) {
    let value = value as f32;
    if index == 0 {
        state.output_gain = value;
    } else if let Some(node) = selected.and_then(|id| state.nodes.iter_mut().find(|n| n.id == id)) {
        match index {
            1 => node.frequency_hz = value,
            2 => node.gain_db = value,
            3 => node.q = value,
            _ => {}
        }
    }
}

fn numeric_event(fixture: &mut Fixture, index: usize, event: NumericControlEvent) {
    let Some(local) = fixture.eq_local.as_mut() else {
        return;
    };
    if event
        .gesture
        .as_ref()
        .is_some_and(|gesture| local.finalized_numeric.contains(gesture))
    {
        return;
    }
    match event.phase {
        NumericEventPhase::Begin => {
            // A keyboard step delivered during this control's captured drag
            // belongs to that still-active edit; keep its original snapshot.
            if event.gesture.is_none()
                && local
                    .numeric_before
                    .as_ref()
                    .is_some_and(|(owner, gesture, _)| *owner == index && gesture.is_some())
            {
                return;
            }
            let mut before = local.history_snapshot();
            set_numeric(&mut before.state, before.selected, index, event.value);
            local.numeric_before = Some((index, event.gesture, before));
        }
        NumericEventPhase::Change => {
            set_numeric(&mut local.state, local.selected, index, event.value);
        }
        NumericEventPhase::Commit => {
            set_numeric(&mut local.state, local.selected, index, event.value);
            if let Some((owner, gesture, before)) = local.numeric_before.take() {
                if owner == index && gesture == event.gesture {
                    local.record(before);
                } else {
                    local.numeric_before = Some((owner, gesture, before));
                }
            }
        }
        NumericEventPhase::Cancel => {
            set_numeric(&mut local.state, local.selected, index, event.value);
            if local
                .numeric_before
                .as_ref()
                .is_some_and(|(owner, gesture, _)| *owner == index && *gesture == event.gesture)
            {
                local.numeric_before = None;
            }
        }
    }
}

enum DiscreteEdit {
    Enabled(bool),
    Selector(usize, usize),
}

fn discrete_event(fixture: &mut Fixture, edit: DiscreteEdit) {
    let Some(local) = fixture.eq_local.as_mut() else {
        return;
    };
    let before = local.history_snapshot();
    match edit {
        DiscreteEdit::Selector(0, value) if value <= 2 => local.state.mode = value as u8,
        edit => {
            let Some(node) = local
                .selected
                .and_then(|id| local.state.nodes.iter_mut().find(|n| n.id == id))
            else {
                return;
            };
            match edit {
                DiscreteEdit::Enabled(value) => node.enabled = value,
                DiscreteEdit::Selector(1, value) => {
                    let kinds = [
                        NodeKind::Bell,
                        NodeKind::LowShelf,
                        NodeKind::HighShelf,
                        NodeKind::LowCut,
                        NodeKind::HighCut,
                        NodeKind::Notch,
                    ];
                    let Some(kind) = kinds.get(value) else { return };
                    node.kind = *kind;
                }
                DiscreteEdit::Selector(2, value) if value <= 3 => node.slope = value as u8,
                _ => return,
            }
        }
    }
    local.record(before);
}

pub(super) fn finish_numeric(
    fixture: &mut Fixture,
    index: usize,
    previous: f64,
    value: f64,
    gesture: Option<NumericGestureId>,
    cx: &mut Context<Gallery>,
) {
    let Some(local) = fixture.eq_local.as_mut() else {
        return;
    };
    if let Some(gesture) = gesture {
        local.finalized_numeric.push(gesture);
    }
    local.pending_numeric.push((index, previous, value));
    if local.flush_scheduled {
        return;
    }
    local.flush_scheduled = true;
    let view = cx.weak_entity();
    cx.defer(move |cx| {
        let _ = view.update(cx, flush_pending);
    });
}

pub(super) fn can_finish_numeric(fixture: &Fixture, additional: usize) -> bool {
    fixture.eq_local.as_ref().is_some_and(|local| {
        local
            .pending_numeric
            .len()
            .checked_add(additional)
            .is_some_and(|count| count <= HISTORY_CAPACITY)
    })
}

fn flush_pending(view: &mut Gallery, cx: &mut Context<Gallery>) {
    let Some(fixture) = view.fixtures.get_mut(&Entry::Eq) else {
        return;
    };
    let Some(local) = fixture.eq_local.as_mut() else {
        return;
    };
    let pending = std::mem::take(&mut local.pending_numeric);
    for (index, previous, value) in pending {
        let mut before = local.history_snapshot();
        set_numeric(&mut before.state, before.selected, index, previous);
        set_numeric(&mut local.state, local.selected, index, value);
        if local
            .numeric_before
            .as_ref()
            .is_some_and(|(owner, _, _)| *owner == index)
        {
            local.numeric_before = None;
        }
        local.record(before);
    }
    let actions = std::mem::take(&mut local.pending_actions);
    let overflow = std::mem::take(&mut local.pending_overflow);
    local.finalized_numeric.clear();
    local.flush_scheduled = false;
    if overflow {
        fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
        let bank = fixture
            .eq_local
            .as_ref()
            .map_or(0, |local| usize::from(local.bank));
        for index in 1..=5 {
            fixture.toggles[index]
                .update(cx, |toggle, cx| toggle.set_checked(index == bank + 1, cx));
        }
        view.error = Some("too many pending eq actions; edit cancelled".into());
        return;
    }
    for action in actions {
        if view.entry != Entry::Eq || view.disabled {
            let cancellation = match &action {
                PendingAction::Graph(event) => event.phase == ResponseGraphPhase::Cancel,
                PendingAction::Marquee(event) => event.phase == ResponseGraphMarqueePhase::Cancel,
                PendingAction::Scroll(event) => event.phase == ResponseGraphScrollPhase::Cancel,
                PendingAction::Bank(..) | PendingAction::History(..) => false,
            };
            if !cancellation {
                continue;
            }
        }
        let result = match action {
            PendingAction::Graph(event) => event_graph(fixture, event, cx),
            PendingAction::Marquee(event) => marquee(fixture, &event, cx),
            PendingAction::Scroll(event) => scroll(fixture, event, cx),
            PendingAction::Bank(index, checked) => {
                let result = if checked {
                    select_bank(fixture, (index - 1) as u8, cx)
                } else {
                    Ok(())
                };
                let bank = fixture
                    .eq_local
                    .as_ref()
                    .map_or(0, |local| usize::from(local.bank));
                for other in 1..=5 {
                    fixture.toggles[other]
                        .update(cx, |toggle, cx| toggle.set_checked(other == bank + 1, cx));
                }
                result
            }
            PendingAction::History(redo) => history(fixture, redo, cx),
        };
        if let Err(error) = result {
            fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
            view.error = Some(error);
            break;
        }
    }
    cx.notify();
}

fn defer_action(fixture: &mut Fixture, action: PendingAction) -> bool {
    let Some(local) = fixture.eq_local.as_mut() else {
        return false;
    };
    if !local.flush_scheduled {
        return false;
    }
    if local.pending_actions.len() == PENDING_ACTION_CAPACITY {
        local.pending_actions.clear();
        local.pending_overflow = true;
    }
    if !local.pending_overflow {
        local.pending_actions.push(action);
    }
    true
}

pub(super) fn defer_history(fixture: &mut Fixture, redo: bool) -> bool {
    defer_action(fixture, PendingAction::History(redo))
}

pub(super) fn history(
    fixture: &mut Fixture,
    redo: bool,
    cx: &mut Context<Gallery>,
) -> Result<(), String> {
    let local = fixture.eq_local.as_ref().ok_or("eq fixture unavailable")?;
    if local.before.is_some() || local.scroll.is_some() || local.numeric_before.is_some() {
        return Err("finish the current eq edit first".into());
    }
    if fixture
        .numbers
        .iter()
        .any(|number| number.entity.read(cx).is_interacting())
    {
        return Err("finish the current eq edit first".into());
    }
    let target = if redo {
        local.redo.last().map(|entry| &entry.after)
    } else {
        local.undo.back().map(|entry| &entry.before)
    };
    let Some(target) = target else { return Ok(()) };
    if target.state.mode > 2 || target.bank > 4 {
        return Err("eq history contains an invalid selection".into());
    }
    fixture.numbers[0]
        .entity
        .read(cx)
        .accessibility_value(f64::from(target.state.output_gain))
        .map_err(|error| error.to_string())?;
    if let Some(id) = target.selected {
        let node = target.state.node(id).map_err(|error| error.to_string())?;
        if node.bank != target.bank || node.slope > 3 {
            return Err("eq history contains an invalid selection".into());
        }
        for (index, value) in [(1, node.frequency_hz), (2, node.gain_db), (3, node.q)] {
            fixture.numbers[index]
                .entity
                .read(cx)
                .accessibility_value(f64::from(value))
                .map_err(|error| error.to_string())?;
        }
    }
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    let entry = if redo {
        local.redo.pop()
    } else {
        local.undo.pop_back()
    };
    let Some(entry) = entry else { return Ok(()) };
    let target = if redo { &entry.after } else { &entry.before };
    local.state = target.state.clone();
    local.selected = target.selected;
    local.group = target.group.clone();
    local.bank = target.bank;
    local.collapse = None;
    local.created = None;
    local.group_drag = None;
    local.option_drag = None;
    let node = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied());
    let output = local.state.output_gain;
    let mode = local.state.mode;
    let bank = local.bank;
    if redo {
        local.undo.push_back(entry);
    } else {
        local.redo.push(entry);
    }
    fixture.numbers[0]
        .entity
        .update(cx, |control, cx| {
            control.set_external_value(f64::from(output), cx)
        })
        .map_err(|error| error.to_string())?;
    fixture.selectors[0]
        .update(cx, |selector, cx| {
            selector.set_selected(usize::from(mode), cx)
        })
        .map_err(str::to_owned)?;
    for index in 1..=5 {
        fixture.toggles[index].update(cx, |toggle, cx| {
            toggle.set_checked(index == usize::from(bank) + 1, cx)
        });
    }
    if let Some(node) = node {
        set_controls(fixture, node, cx)?;
    }
    Ok(())
}

fn select_bank(fixture: &mut Fixture, bank: u8, cx: &mut Context<Gallery>) -> Result<(), String> {
    if fixture
        .eq_local
        .as_ref()
        .is_some_and(|local| local.before.is_some() || local.scroll.is_some())
    {
        return Err("finish the eq graph edit before changing banks".into());
    }
    let state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    local.state = state;
    local.bank = bank;
    local.group.clear();
    local.collapse = None;
    local.group_drag = None;
    local.selected = local
        .state
        .nodes
        .iter()
        .find(|node| node.bank == bank)
        .map(|node| node.id);
    let selected = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied());
    if let Some(node) = selected {
        set_controls(fixture, node, cx)?;
    }
    Ok(())
}

fn marquee(
    fixture: &mut Fixture,
    event: &ResponseGraphMarqueeEvent,
    cx: &mut Context<Gallery>,
) -> Result<(), String> {
    let state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    local.state = state;
    let members: Vec<_> = event
        .selected
        .iter()
        .copied()
        .filter(|raw| {
            NodeId::new(u64::from(*raw))
                .ok()
                .and_then(|id| local.state.node(id).ok())
                .is_some_and(|node| node.bank == local.bank)
        })
        .collect();
    selection::apply_marquee(&mut local.selected, &mut local.group, &members);
    local.collapse = None;
    let selected = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied());
    if let Some(node) = selected {
        set_controls(fixture, node, cx)?;
    }
    Ok(())
}

fn scroll(
    fixture: &mut Fixture,
    event: ResponseGraphScrollEvent,
    cx: &mut Context<Gallery>,
) -> Result<(), String> {
    if event.gesture_id == 0 {
        return Ok(());
    }
    let mut state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    let mut completed = None;
    if event.phase == ResponseGraphScrollPhase::Begin {
        if local.before.is_some() || local.scroll.is_some() {
            return Err("finish the current eq edit first".into());
        }
        let id = NodeId::new(u64::from(event.node_id)).map_err(|e| e.to_string())?;
        let node = *state.node(id).map_err(|_| "eq scroll node unavailable")?;
        if node.bank != local.bank || !matches!(node.kind, NodeKind::LowCut | NodeKind::HighCut) {
            return Err("eq scroll node unavailable".into());
        }
        let before = Snapshot {
            state: state.clone(),
            selected: local.selected,
            group: local.group.clone(),
        };
        local.selected = Some(id);
        local.scroll = Some(LocalScroll {
            node: id,
            stream: event.gesture_id,
            value: u32::from(node.slope),
            last_steps: 0,
            before,
        });
    } else {
        let Some(mut scroll) = local.scroll.take() else {
            return Ok(());
        };
        if scroll.node.value() != event.node_id || scroll.stream != event.gesture_id {
            local.scroll = Some(scroll);
            return Ok(());
        }
        if event.phase == ResponseGraphScrollPhase::Cancel {
            state = scroll.before.state;
            local.selected = scroll.before.selected;
            local.group = scroll.before.group;
        } else {
            let Some(node) = state.nodes.iter_mut().find(|node| node.id == scroll.node) else {
                local.scroll = Some(scroll);
                return Err("eq scroll node unavailable".into());
            };
            if node.bank != local.bank || !matches!(node.kind, NodeKind::LowCut | NodeKind::HighCut)
            {
                local.scroll = Some(scroll);
                return Err("eq scroll node changed during edit".into());
            }
            let delta = event.steps.saturating_sub(scroll.last_steps);
            scroll.value = slope::advance(scroll.value, delta);
            scroll.last_steps = event.steps;
            node.slope = scroll.value as u8;
            if event.phase == ResponseGraphScrollPhase::Commit {
                completed = Some(HistorySnapshot {
                    state: scroll.before.state,
                    selected: scroll.before.selected,
                    group: scroll.before.group,
                    bank: local.bank,
                });
            } else {
                local.scroll = Some(scroll);
            }
        }
    }
    local.state = state;
    if let Some(before) = completed {
        local.record(before);
    }
    let selected = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied());
    if let Some(node) = selected {
        set_controls(fixture, node, cx)?;
    }
    Ok(())
}

fn set_controls(fixture: &Fixture, node: Node, cx: &mut Context<Gallery>) -> Result<(), String> {
    for (index, value) in [(1, node.frequency_hz), (2, node.gain_db), (3, node.q)] {
        fixture.numbers[index]
            .entity
            .update(cx, |control, cx| {
                control.set_external_value(f64::from(value), cx)
            })
            .map_err(|e| e.to_string())?;
    }
    fixture.selectors[1]
        .update(cx, |selector, cx| {
            selector.set_selected(node.kind as usize, cx)
        })
        .map_err(str::to_owned)?;
    fixture.selectors[2]
        .update(cx, |selector, cx| {
            selector.set_selected(node.slope as usize, cx)
        })
        .map_err(str::to_owned)?;
    fixture.toggles[0].update(cx, |toggle, cx| toggle.set_checked(node.enabled, cx));
    for index in 1..=5 {
        fixture.toggles[index].update(cx, |toggle, cx| {
            toggle.set_checked(index == usize::from(node.bank) + 1, cx)
        });
    }
    Ok(())
}

fn set_position(node: &mut Node, position: [f32; 2]) {
    let (frequency, vertical) = group_move::values(node.kind, position, [0.0, 0.0], 48_000);
    node.frequency_hz = frequency;
    match vertical {
        group_move::Vertical::Gain(value) => node.gain_db = value,
        group_move::Vertical::Q(value) => node.q = value,
    }
}

fn event_graph(
    fixture: &mut Fixture,
    event: ResponseGraphEvent,
    cx: &mut Context<Gallery>,
) -> Result<(), String> {
    if event
        .position
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("invalid eq graph position".into());
    }
    let mut state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    let mut completed = None;
    let id = if event.node_id == 0 {
        local.created
    } else {
        NodeId::new(u64::from(event.node_id)).ok()
    };
    if event.node_id != 0
        && matches!(
            event.phase,
            ResponseGraphPhase::Begin | ResponseGraphPhase::Change | ResponseGraphPhase::Commit
        )
        && id
            .and_then(|id| state.node(id).ok())
            .is_none_or(|node| node.bank != local.bank)
    {
        return Err("eq node unavailable in this bank".into());
    }
    match event.phase {
        ResponseGraphPhase::Remove => {
            if local.before.is_some() || local.scroll.is_some() {
                return Err("finish the current eq edit first".into());
            }
            let Some(id) = id else { return Ok(()) };
            let removed = if local.group.len() > 1 && local.group.contains(&id) {
                local.group.clone()
            } else if local.selected == Some(id) {
                vec![id]
            } else {
                return Ok(());
            };
            if removed
                .iter()
                .any(|id| !state.node(*id).is_ok_and(|node| node.bank == local.bank))
            {
                return Err("eq removal node unavailable in this bank".into());
            }
            let mut before = local.history_snapshot();
            before.state = state.clone();
            completed = Some(before);
            for id in removed {
                state.remove(id).map_err(|e| e.to_string())?;
            }
            local.selected = None;
            local.group.clear();
            local.collapse = None;
        }
        ResponseGraphPhase::Create => {
            let id = NodeId::new(u64::from(state.next_node_id)).map_err(|e| e.to_string())?;
            let mut node = Node::new(id, 1_000.0, 0.0, 0).map_err(|e| e.to_string())?;
            node.kind = node_geometry::creation_kind(event.position);
            node.bank = local.bank;
            set_position(&mut node, event.position);
            let before = Snapshot {
                state: state.clone(),
                selected: local.selected,
                group: local.group.clone(),
            };
            state.add(node).map_err(|e| e.to_string())?;
            local.before = Some(before);
            local.selected = Some(id);
            local.group.clear();
            local.collapse = None;
            local.created = Some(id);
        }
        ResponseGraphPhase::Select => {
            let Some(id) = id else { return Ok(()) };
            if !state.node(id).is_ok_and(|node| node.bank == local.bank) {
                return Ok(());
            }
            selection::select_node(
                &mut local.selected,
                &mut local.group,
                &mut local.collapse,
                id,
                event.modifiers.shift,
            );
        }
        ResponseGraphPhase::Begin => {
            if event.node_id != 0 {
                if id.is_some_and(|id| {
                    local.group.len() > group_move::MAX_GROUP_NODES && local.group.contains(&id)
                }) {
                    return Err("eq group move is limited to 32 nodes".into());
                }
                local.before = Some(Snapshot {
                    state: state.clone(),
                    selected: local.selected,
                    group: local.group.clone(),
                });
                local.collapse = None;
                if id.is_some_and(|id| local.group.len() > 1 && local.group.contains(&id)) {
                    let nodes = local
                        .group
                        .iter()
                        .map(|id| {
                            state
                                .node(*id)
                                .copied()
                                .map(|node| (node, node_geometry::node_position(&node, 48_000)))
                                .map_err(|_| "eq group node unavailable")
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    local.group_drag = Some(GroupDrag {
                        origin: event.position,
                        nodes,
                    });
                } else if event.modifiers.alt
                    && !event.modifiers.platform
                    && !event.modifiers.control
                    && let Some(node) = id.and_then(|id| state.node(id).ok()).copied()
                    && node.kind == NodeKind::Bell
                {
                    local.option_drag = Some((node, event.position[1]));
                }
            }
        }
        ResponseGraphPhase::Change | ResponseGraphPhase::Commit => {
            if let Some(drag) = &local.group_drag {
                let delta = [
                    event.position[0] - drag.origin[0],
                    event.position[1] - drag.origin[1],
                ];
                for (original, origin) in &drag.nodes {
                    let node = state
                        .nodes
                        .iter_mut()
                        .find(|node| node.id == original.id)
                        .ok_or("eq group node unavailable")?;
                    if node.kind != original.kind || node.bank != original.bank {
                        return Err("eq group node changed during drag".into());
                    }
                    let (frequency, vertical) =
                        group_move::values(original.kind, *origin, delta, 48_000);
                    node.frequency_hz = frequency;
                    match vertical {
                        group_move::Vertical::Gain(value) => node.gain_db = value,
                        group_move::Vertical::Q(value) => node.q = value,
                    }
                }
            } else {
                let Some(id) = id else { return Ok(()) };
                let Some(node) = state.nodes.iter_mut().find(|node| node.id == id) else {
                    return Ok(());
                };
                if let Some((original, origin_y)) = local.option_drag {
                    if node.id != original.id
                        || node.kind != original.kind
                        || node.bank != original.bank
                    {
                        return Err("eq option-drag node changed during edit".into());
                    }
                    node.frequency_hz = original.frequency_hz;
                    node.gain_db = original.gain_db;
                    node.q = group_move::option_q(original.q, event.position[1] - origin_y);
                } else {
                    set_position(node, event.position);
                }
            }
            if event.phase == ResponseGraphPhase::Commit {
                completed = local.before.take().map(|before| HistorySnapshot {
                    state: before.state,
                    selected: before.selected,
                    group: before.group,
                    bank: local.bank,
                });
                local.group_drag = None;
                local.option_drag = None;
            }
        }
        ResponseGraphPhase::Cancel => {
            if let Some(before) = local.before.take() {
                state = before.state;
                local.selected = before.selected;
                local.group = before.group;
            }
            local.created = None;
            local.group_drag = None;
            local.option_drag = None;
            local.collapse = None;
        }
        ResponseGraphPhase::Release | ResponseGraphPhase::ReleaseOutside => {
            if event.phase == ResponseGraphPhase::Release
                && id.is_some()
                && local.collapse.take() == id
            {
                local.group.clear();
            }
            local.collapse = None;
            completed = local.before.take().map(|before| HistorySnapshot {
                state: before.state,
                selected: before.selected,
                group: before.group,
                bank: local.bank,
            });
            local.group_drag = None;
            local.option_drag = None;
            local.created = None;
        }
        _ => {}
    }
    local.state = state;
    if let Some(before) = completed {
        local.record(before);
    }
    let selected = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied());
    if let Some(node) = selected {
        set_controls(fixture, node, cx)?;
    }
    Ok(())
}

pub(super) fn model(fixture: &Fixture, cx: &gpui::App) -> Option<ResponseGraphModel> {
    let state = current(fixture, cx)?;
    let bank = usize::from(fixture.eq_local.as_ref()?.bank);
    let Ok(authored) = state.encode() else {
        return None;
    };
    let mut curves = sampling::curves(&state, 48_000, |frequencies| {
        hex_devices::native_effect_frequency_response(&authored, 48_000, frequencies).ok()?
    })?;
    if bank == 2 || bank == 4 {
        curves.swap(0, 1);
    }
    let [primary, secondary] = curves;
    let (analyzer, input_analyzer, analyzer_peak) = super::eq_spectrum::layers(
        fixture.toggles[SPECTRUM_TOGGLE].read(cx).checked(),
        fixture.toggles[INPUT_TOGGLE].read(cx).checked(),
    );
    let model = ResponseGraphModel {
        primary,
        secondary,
        analyzer,
        input_analyzer,
        analyzer_peak,
        nodes: state
            .nodes
            .iter()
            .filter(|node| usize::from(node.bank) == bank)
            .map(|node| hex_widgets::ResponseGraphNode {
                id: node.id.value(),
                position: node_geometry::node_position(node, 48_000),
                enabled: node.enabled,
                selected: fixture.eq_local.as_ref().is_some_and(|local| {
                    local.selected == Some(node.id) || local.group.contains(&node.id)
                }),
            })
            .collect(),
    };
    Some(model)
}

pub(super) fn sync(fixture: &Fixture, disabled: bool, cx: &mut Context<Gallery>) {
    let Some(model) = model(fixture, cx) else {
        return;
    };
    let eligible = if disabled
        || fixture
            .eq_local
            .as_ref()
            .is_none_or(|local| local.before.is_some())
        || fixture.numbers.iter().any(|number| {
            number.entity.read(cx).is_editing() || number.entity.read(cx).is_interacting()
        }) {
        Vec::new()
    } else {
        let bank = fixture.eq_local.as_ref().map_or(0, |local| local.bank);
        current(fixture, cx).map_or_else(Vec::new, |state| {
            state
                .nodes
                .iter()
                .filter(|node| {
                    node.bank == bank && matches!(node.kind, NodeKind::LowCut | NodeKind::HighCut)
                })
                .map(|node| node.id.value())
                .collect()
        })
    };
    let show_hint = !disabled
        && fixture.eq_local.as_ref().is_some_and(|local| {
            local.before.is_none()
                && local.scroll.is_none()
                && local.state.nodes.len() < eq::MAX_NODES
        })
        && !fixture.numbers.iter().any(|number| {
            number.entity.read(cx).is_editing() || number.entity.read(cx).is_interacting()
        });
    fixture.graph.update(cx, |graph, cx| {
        if disabled {
            graph.cancel(cx);
        }
        graph.set_editable(!disabled, cx);
        graph.set_creation_enabled(!disabled);
        graph.set_creation_hint_resolver(show_hint.then_some(node_geometry::creation_hint), cx);
        graph.set_marquee_enabled(!disabled, cx);
        let _ = graph.set_scroll_nodes(eligible, cx);
        let _ = graph.set_axes(Some(axes::frequency_axes(48000)), cx);
        let _ = graph.set_model(
            model,
            theme::device_style(theme::DeviceStyleRole::Display),
            cx,
        );
    });
}

pub(super) fn body(fixture: &Fixture) -> Div {
    let row = || {
        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .w_full()
            .flex_shrink_0()
    };
    let number = |i, label: &str| {
        row()
            .h(px(16.0))
            .child(
                div()
                    .w(px(if i == 0 { 18.0 } else { 32.0 }))
                    .child(theme::text(label)),
            )
            .child(fixture.number(i))
    };
    let mut banks = row();
    for i in 1..=5 {
        banks = banks.child(fixture.toggle(i, 14.0));
    }
    let controls = div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            row()
                .child(fixture.selector(0, 72.0))
                .child(number(0, "out").w(px(78.0))),
        )
        .child(banks)
        .child(
            row()
                .child(fixture.toggle(6, 44.0))
                .child(fixture.toggle(7, 32.0))
                .child(fixture.toggle(8, 38.0)),
        )
        .child(
            row()
                .child(fixture.toggle(0, 14.0))
                .child(fixture.selector(1, 120.0)),
        )
        .child(number(1, "freq"))
        .child(number(2, "gain"))
        .child(number(3, "q"))
        .child(
            row()
                .child(div().w(px(32.0)).child(theme::text("slope")))
                .child(fixture.selector(2, 104.0)),
        )
        .child(theme::text("fixture · no audio"));
    device_composition::eq_graph_body(
        fixture.graph.clone(),
        device_composition::eq_sidebar(
            "fixture-eq-sidebar",
            controls,
            div().child(theme::text("no audio")),
        ),
    )
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn event(phase: ResponseGraphPhase, node_id: u32, position: [f32; 2]) -> ResponseGraphEvent {
        ResponseGraphEvent {
            phase,
            node_id,
            position,
            modifiers: gpui::Modifiers::default(),
        }
    }

    #[gpui::test]
    fn fixture_spectrum_controls_update_layers_and_audition_is_unavailable(
        cx: &mut TestAppContext,
    ) {
        use gpui::{Modifiers, VisualTestContext, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let root = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1200.0), px(700.0)));
        visual.run_until_parked();
        let check = |spectrum: bool, input: bool, cx: &mut TestAppContext| {
            root.read_with(cx, |view, cx| {
                let fixture = &view.fixtures[&Entry::Eq];
                let graph = model(fixture, cx).unwrap();
                assert_eq!(graph.analyzer.is_some(), spectrum);
                assert_eq!(graph.analyzer_peak.is_some(), spectrum);
                assert_eq!(graph.input_analyzer.is_some(), spectrum && input);
                assert!(!fixture.toggles[8].read(cx).checked());
            });
        };
        check(true, true, cx);
        for (id, spectrum, input) in [
            ("fixture-eq-toggle-7", true, false),
            ("fixture-eq-toggle-6", false, false),
            ("fixture-eq-toggle-7", false, true),
            ("fixture-eq-toggle-6", true, true),
            ("fixture-eq-toggle-8", true, true),
        ] {
            let bounds = visual.debug_bounds(id).unwrap();
            visual.simulate_click(bounds.center(), Modifiers::default());
            visual.run_until_parked();
            check(spectrum, input, cx);
        }
        visual.update(|_, cx| {
            root.update(cx, |view, cx| {
                view.disabled = true;
                cx.notify();
            })
        });
        visual.run_until_parked();
        let spectrum = visual.debug_bounds("fixture-eq-toggle-6").unwrap();
        visual.simulate_click(spectrum.center(), Modifiers::default());
        visual.run_until_parked();
        check(true, true, cx);
        visual.update(|_, cx| {
            root.update(cx, |view, cx| {
                view.disabled = false;
                cx.notify();
            })
        });
        visual.run_until_parked();
        let audition = visual.debug_bounds("fixture-eq-toggle-8").unwrap();
        visual.simulate_click(audition.center(), Modifiers::default());
        visual.run_until_parked();
        check(true, true, cx);
    }

    #[gpui::test]
    fn history_restores_created_and_removed_nodes_and_clears_redo_on_new_edit(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let initial = current(fixture, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            let created = current(fixture, cx).unwrap();
            assert_eq!(created.nodes.len(), 2);
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), initial);
            assert_eq!(
                fixture.eq_local.as_ref().unwrap().selected.unwrap().value(),
                1
            );
            history(fixture, true, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), created);
            assert_eq!(
                fixture.eq_local.as_ref().unwrap().selected.unwrap().value(),
                2
            );
            event_graph(
                fixture,
                event(ResponseGraphPhase::Remove, 2, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap().nodes.len(), 1);
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), created);
            history(fixture, false, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.6, 0.4]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.6, 0.4]),
                cx,
            )
            .unwrap();
            let replacement = current(fixture, cx).unwrap();
            history(fixture, true, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), replacement);
        });
    }

    #[gpui::test]
    fn numeric_drag_is_one_undoable_edit_and_cancel_does_not_add_history(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        let end = point(field.x, field.y - px(20.0));
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let edited = view.read_with(cx, |view, cx| {
            current(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        assert_ne!(edited.nodes[0].gain_db, 6.0);
        visual.simulate_keystrokes("cmd-z");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                current(&view.fixtures[&Entry::Eq], cx).unwrap().nodes[0].gain_db,
                6.0
            )
        });
        visual.simulate_keystrokes("cmd-shift-z");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(current(&view.fixtures[&Entry::Eq], cx).unwrap(), edited)
        });
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.fixtures[&Entry::Eq].numbers[2]
                    .entity
                    .update(cx, |number, cx| number.cancel(cx));
            })
        });
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let fixture = &view.fixtures[&Entry::Eq];
            assert_eq!(current(fixture, cx).unwrap(), edited);
            assert_eq!(fixture.eq_local.as_ref().unwrap().undo.len(), 1);
        });
    }

    #[gpui::test]
    fn keyboard_step_during_pointer_drag_does_not_lose_the_drag_history(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_numeric_control_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        let end = point(field.x, field.y - px(20.0));
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("up");
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-z");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                current(&view.fixtures[&Entry::Eq], cx).unwrap().nodes[0].gain_db,
                6.0
            );
        });
    }

    #[gpui::test]
    fn synchronous_barrier_does_not_readmit_queued_numeric_gesture(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(field.x, field.y - px(20.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                let entity = view.fixtures[&Entry::Eq].numbers[2].entity.clone();
                let finalized = entity.update(cx, |number, cx| {
                    number.prepare_finish_edit_for_owner(cx).unwrap().unwrap()
                });
                entity.update(cx, |_, cx| {
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Begin,
                        value: finalized.previous,
                        gesture: finalized.gesture.clone(),
                    });
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Change,
                        value: finalized.value,
                        gesture: finalized.gesture.clone(),
                    });
                });
                assert!(view.finish_drafts(cx));
            })
        });
        visual.run_until_parked();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            assert_ne!(current(fixture, cx).unwrap().nodes[0].gain_db, 6.0);
            assert_eq!(fixture.eq_local.as_ref().unwrap().undo.len(), 1);
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap().nodes[0].gain_db, 6.0);
            assert!(fixture.eq_local.as_ref().unwrap().undo.is_empty());
        });
    }

    #[gpui::test]
    fn queued_earlier_control_edit_precedes_synchronously_finalized_drag(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.fixtures[&Entry::Eq].numbers[0]
                    .entity
                    .update(cx, |control, cx| {
                        control.set_external_value(2.0, cx).unwrap()
                    });
            })
        });
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(field.x, field.y - px(20.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                let entity = view.fixtures[&Entry::Eq].numbers[0].entity.clone();
                entity.update(cx, |_, cx| {
                    for (phase, value) in [
                        (NumericEventPhase::Begin, 1.0),
                        (NumericEventPhase::Change, 2.0),
                        (NumericEventPhase::Commit, 2.0),
                    ] {
                        cx.emit(NumericControlEvent {
                            phase,
                            value,
                            gesture: None,
                        });
                    }
                });
                assert!(view.finish_drafts(cx));
            })
        });
        visual.run_until_parked();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            history(fixture, false, cx).unwrap();
            let state = current(fixture, cx).unwrap();
            assert_eq!(state.output_gain, 2.0);
            assert_eq!(state.nodes[0].gain_db, 6.0);
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap().output_gain, 1.0);
        });
    }

    #[gpui::test]
    fn discrete_control_edits_are_independent_and_undoable(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let initial = current(fixture, cx).unwrap();
            discrete_event(fixture, DiscreteEdit::Enabled(false));
            fixture.toggles[0].update(cx, |toggle, cx| toggle.set_checked(false, cx));
            discrete_event(fixture, DiscreteEdit::Selector(1, 3));
            fixture.selectors[1].update(cx, |control, cx| control.set_selected(3, cx).unwrap());
            assert_eq!(
                current(fixture, cx).unwrap().nodes[0].kind,
                NodeKind::LowCut
            );
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap().nodes[0].kind, NodeKind::Bell);
            assert!(!current(fixture, cx).unwrap().nodes[0].enabled);
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), initial);
        });
    }

    #[gpui::test]
    fn actual_toggle_and_selector_changes_undo_in_order(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let toggle = visual.debug_bounds("fixture-eq-toggle-0").unwrap().center();
        visual.simulate_click(toggle, Modifiers::default());
        let selector = visual
            .debug_bounds("fixture-eq-selector-1")
            .unwrap()
            .center();
        visual.simulate_click(selector, Modifiers::default());
        visual.run_until_parked();
        let option = visual
            .debug_bounds("fixture-eq-selector-1-option-3")
            .unwrap()
            .center();
        visual.simulate_click(option, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let state = current(&view.fixtures[&Entry::Eq], cx).unwrap();
            assert!(!state.nodes[0].enabled);
            assert_eq!(state.nodes[0].kind, NodeKind::LowCut);
        });
        visual.simulate_keystrokes("cmd-z");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let state = current(&view.fixtures[&Entry::Eq], cx).unwrap();
            assert!(!state.nodes[0].enabled);
            assert_eq!(state.nodes[0].kind, NodeKind::Bell);
        });
        visual.simulate_keystrokes("cmd-z");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(current(&view.fixtures[&Entry::Eq], cx).unwrap().nodes[0].enabled)
        });
    }

    #[gpui::test]
    fn accepted_text_draft_and_following_graph_creation_undo_separately(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_numeric_control_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("-12");
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.3,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_click(position, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let state = current(&view.fixtures[&Entry::Eq], cx).unwrap();
            assert_eq!(state.nodes.len(), 2);
            assert_eq!(state.nodes[0].gain_db, -12.0);
        });
        visual.simulate_keystrokes("cmd-z");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let state = current(&view.fixtures[&Entry::Eq], cx).unwrap();
            assert_eq!(state.nodes.len(), 1);
            assert_eq!(state.nodes[0].gain_db, -12.0);
        });
        visual.simulate_keystrokes("cmd-z");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let state = current(&view.fixtures[&Entry::Eq], cx).unwrap();
            assert_eq!(state.nodes.len(), 1);
            assert_eq!(state.nodes[0].gain_db, 6.0);
        });
    }

    #[gpui::test]
    fn grouped_move_and_slope_stream_each_record_one_history_entry(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            marquee(
                fixture,
                &ResponseGraphMarqueeEvent {
                    phase: ResponseGraphMarqueePhase::Commit,
                    selected: vec![1, 2],
                    modifiers: gpui::Modifiers::default(),
                },
                cx,
            )
            .unwrap();
            let before = current(fixture, cx).unwrap();
            let start = node_geometry::node_position(&before.nodes[1], 48_000);
            let end = [start[0] + 0.05, start[1] + 0.1];
            event_graph(fixture, event(ResponseGraphPhase::Begin, 2, start), cx).unwrap();
            event_graph(fixture, event(ResponseGraphPhase::Change, 2, end), cx).unwrap();
            event_graph(fixture, event(ResponseGraphPhase::Commit, 2, end), cx).unwrap();
            let moved = current(fixture, cx).unwrap();
            assert_ne!(before, moved);
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert_eq!(fixture.eq_local.as_ref().unwrap().group.len(), 2);
            history(fixture, true, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), moved);
            event_graph(fixture, event(ResponseGraphPhase::Remove, 2, end), cx).unwrap();
            assert!(current(fixture, cx).unwrap().nodes.is_empty());
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), moved);
            assert_eq!(fixture.eq_local.as_ref().unwrap().group.len(), 2);

            discrete_event(fixture, DiscreteEdit::Selector(1, 3));
            fixture.selectors[1].update(cx, |control, cx| control.set_selected(3, cx).unwrap());
            let before_scroll = current(fixture, cx).unwrap();
            let ev = |phase, steps| ResponseGraphScrollEvent {
                phase,
                node_id: 2,
                gesture_id: 7,
                steps,
            };
            scroll(fixture, ev(ResponseGraphScrollPhase::Begin, 0), cx).unwrap();
            scroll(fixture, ev(ResponseGraphScrollPhase::Change, 1), cx).unwrap();
            scroll(fixture, ev(ResponseGraphScrollPhase::Commit, 2), cx).unwrap();
            let after_scroll = current(fixture, cx).unwrap();
            assert_ne!(before_scroll.nodes[1].slope, after_scroll.nodes[1].slope);
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before_scroll);
            history(fixture, true, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), after_scroll);
        });
    }

    #[test]
    fn history_capacity_is_bounded_and_noop_does_not_clear_redo() {
        let mut local = Local::new().unwrap();
        for index in 0..(HISTORY_CAPACITY + 1) {
            let before = local.history_snapshot();
            local.state.output_gain = if index % 2 == 0 { 0.5 } else { 1.0 };
            local.record(before);
        }
        assert_eq!(local.undo.len(), HISTORY_CAPACITY);
        let entry = local.undo.pop_back().unwrap();
        local.state = entry.before.state.clone();
        local.redo.push(entry);
        let before = local.history_snapshot();
        local.record(before);
        assert_eq!(local.redo.len(), 1);
        let before = local.history_snapshot();
        local.state.output_gain = 0.25;
        local.record(before);
        assert!(local.redo.is_empty());
    }

    #[gpui::test]
    fn single_node_move_is_one_undoable_gesture(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = current(fixture, cx).unwrap();
            let start = node_geometry::node_position(&before.nodes[0], 48_000);
            let end = [start[0] + 0.05, start[1] + 0.05];
            event_graph(fixture, event(ResponseGraphPhase::Begin, 1, start), cx).unwrap();
            event_graph(fixture, event(ResponseGraphPhase::Change, 1, end), cx).unwrap();
            event_graph(fixture, event(ResponseGraphPhase::Commit, 1, end), cx).unwrap();
            let after = current(fixture, cx).unwrap();
            assert_ne!(after, before);
            history(fixture, false, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            history(fixture, true, cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), after);
        });
    }

    #[gpui::test]
    fn invalid_history_target_is_refused_before_state_or_stacks_change(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let local = fixture.eq_local.as_mut().unwrap();
            let before = local.history_snapshot();
            local.state.output_gain = 0.5;
            local.record(before);
            local.undo.back_mut().unwrap().before.state.nodes[0].q = f32::NAN;
            let prior = local.state.clone();
            assert!(history(fixture, false, cx).is_err());
            let local = fixture.eq_local.as_ref().unwrap();
            assert_eq!(local.state, prior);
            assert_eq!(local.undo.len(), 1);
            assert!(local.redo.is_empty());
        });
    }

    #[gpui::test]
    fn pending_action_overflow_refuses_graph_edits_without_losing_prior_state(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = current(fixture, cx).unwrap();
            fixture.eq_local.as_mut().unwrap().flush_scheduled = true;
            for _ in 0..=PENDING_ACTION_CAPACITY {
                assert!(defer_action(
                    fixture,
                    PendingAction::Graph(event(ResponseGraphPhase::Create, 0, [0.3, 0.7]))
                ));
            }
            flush_pending(view, cx);
            let fixture = &view.fixtures[&Entry::Eq];
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert!(fixture.eq_local.as_ref().unwrap().undo.is_empty());
            assert!(view.error.is_some());
        });
    }

    #[gpui::test]
    fn remove_group_preserves_other_banks_and_refuses_active_or_stale_edits(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let id2 = NodeId::new(2).unwrap();
            let id3 = NodeId::new(3).unwrap();
            let local = fixture.eq_local.as_mut().unwrap();
            local
                .state
                .nodes
                .push(Node::new(id2, 2_000.0, 0.0, 0).unwrap());
            let mut foreign = Node::new(id3, 3_000.0, 0.0, 0).unwrap();
            foreign.bank = 1;
            local.state.nodes.push(foreign);
            local.group = vec![local.selected.unwrap(), id3];
            let before = current(fixture, cx).unwrap();
            let remove = event(ResponseGraphPhase::Remove, 1, [0.5, 0.625]);
            assert!(event_graph(fixture, remove, cx).is_err());
            assert_eq!(current(fixture, cx).unwrap(), before);
            let local = fixture.eq_local.as_mut().unwrap();
            local.group = vec![local.selected.unwrap(), id2];
            local.before = Some(Snapshot {
                state: before.clone(),
                selected: local.selected,
                group: local.group.clone(),
            });
            assert!(event_graph(fixture, remove, cx).is_err());
            assert_eq!(current(fixture, cx).unwrap(), before);
            fixture.eq_local.as_mut().unwrap().before = None;
            event_graph(fixture, remove, cx).unwrap();
            let after = current(fixture, cx).unwrap();
            assert_eq!(after.nodes, vec![foreign]);
            let local = fixture.eq_local.as_ref().unwrap();
            assert!(local.selected.is_none() && local.group.is_empty());
        });
    }

    #[gpui::test]
    fn backspace_removes_selected_node(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes[0].position
        });
        let screen = point(
            plot.left() + plot.size.width * position[0],
            plot.top() + plot.size.height * (1.0 - position[1]),
        );
        visual.simulate_click(screen, Modifiers::default());
        visual.simulate_keystrokes("backspace");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(
                current(&view.fixtures[&Entry::Eq], cx)
                    .unwrap()
                    .nodes
                    .is_empty()
            )
        });
    }

    #[gpui::test]
    fn blank_graph_hover_shows_creation_shape(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        for (x, y, name) in [
            (0.02, 0.8, "fixture-eq-graph-creation-hint-low-cut"),
            (0.98, 0.8, "fixture-eq-graph-creation-hint-high-cut"),
            (0.3, 0.02, "fixture-eq-graph-creation-hint-notch"),
            (0.3, 0.8, "fixture-eq-graph-creation-hint-bell"),
        ] {
            let p = point(
                plot.left() + plot.size.width * x,
                plot.top() + plot.size.height * (1.0 - y),
            );
            visual.simulate_mouse_move(p, None, Modifiers::default());
            visual.run_until_parked();
            assert!(visual.debug_bounds(name).is_some(), "missing {name} hint");
        }
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                let local = view
                    .fixtures
                    .get_mut(&Entry::Eq)
                    .unwrap()
                    .eq_local
                    .as_mut()
                    .unwrap();
                local.before = Some(Snapshot {
                    state: local.state.clone(),
                    selected: local.selected,
                    group: local.group.clone(),
                });
                cx.notify();
            });
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("fixture-eq-graph-creation-hint-bell")
                .is_none()
        );
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.fixtures
                    .get_mut(&Entry::Eq)
                    .unwrap()
                    .eq_local
                    .as_mut()
                    .unwrap()
                    .before = None;
                view.disabled = true;
                cx.notify();
            })
        });
        visual.run_until_parked();
        let disabled_blank = point(
            plot.left() + plot.size.width * 0.35,
            plot.top() + plot.size.height * 0.2,
        );
        visual.simulate_mouse_move(disabled_blank, None, Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("fixture-eq-graph-creation-hint-bell")
                .is_none()
        );
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.disabled = false;
                let local = view
                    .fixtures
                    .get_mut(&Entry::Eq)
                    .unwrap()
                    .eq_local
                    .as_mut()
                    .unwrap();
                for raw_id in 2..=eq::MAX_NODES as u64 {
                    let id = NodeId::new(raw_id).unwrap();
                    local
                        .state
                        .nodes
                        .push(Node::new(id, 1_000.0, 0.0, 0).unwrap());
                }
                cx.notify();
            })
        });
        visual.run_until_parked();
        let blank = point(
            plot.left() + plot.size.width * 0.3,
            plot.top() + plot.size.height * 0.2,
        );
        visual.simulate_mouse_move(blank, None, Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("fixture-eq-graph-creation-hint-bell")
                .is_none()
        );
    }

    #[gpui::test]
    fn option_bell_drag_changes_only_q_and_cancel_restores(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = current(fixture, cx).unwrap();
            let mut begin = event(ResponseGraphPhase::Begin, 1, [0.5, 0.625]);
            begin.modifiers.alt = true;
            event_graph(fixture, begin, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Change, 1, [0.7, 0.725]),
                cx,
            )
            .unwrap();
            let changed = current(fixture, cx).unwrap();
            assert_eq!(changed.nodes[0].frequency_hz, before.nodes[0].frequency_hz);
            assert_eq!(changed.nodes[0].gain_db, before.nodes[0].gain_db);
            assert!(changed.nodes[0].q > before.nodes[0].q);
            event_graph(
                fixture,
                event(ResponseGraphPhase::Cancel, 1, [0.7, 0.725]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            event_graph(fixture, begin, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Commit, 1, [0.7, 0.725]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap(), changed);
        });
    }

    #[gpui::test]
    fn node_drag_cancel_restores_controls_and_authored_state(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = current(fixture, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Begin, 1, [0.5, 0.625]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Change, 1, [0.7, 0.8]),
                cx,
            )
            .unwrap();
            assert_ne!(current(fixture, cx).unwrap(), before);
            event_graph(
                fixture,
                event(ResponseGraphPhase::Cancel, 1, [0.7, 0.8]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert_eq!(fixture.numbers[2].entity.read(cx).value(), 6.0);
        });
    }

    #[gpui::test]
    fn creation_cancel_restores_original_node_and_selection(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = current(fixture, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap().nodes.len(), 2);
            event_graph(
                fixture,
                event(ResponseGraphPhase::Change, 0, [0.4, 0.8]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Cancel, 0, [0.4, 0.8]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert!(fixture.eq_local.as_ref().unwrap().undo.is_empty());
            assert_eq!(
                fixture.eq_local.as_ref().unwrap().selected.unwrap().value(),
                1
            );
        });
    }

    #[gpui::test]
    fn selecting_created_nodes_restores_their_controls_and_response(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = model(fixture, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            fixture.numbers[2].entity.update(cx, |control, cx| {
                control.set_external_value(-12.0, cx).unwrap()
            });
            event_graph(
                fixture,
                event(ResponseGraphPhase::Select, 1, [0.5, 0.6]),
                cx,
            )
            .unwrap();
            assert_eq!(fixture.numbers[2].entity.read(cx).value(), 6.0);
            event_graph(
                fixture,
                event(ResponseGraphPhase::Select, 2, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            assert_eq!(fixture.numbers[2].entity.read(cx).value(), -12.0);
            let after = model(fixture, cx).unwrap();
            assert_eq!(after.nodes.len(), 2);
            assert!(after.nodes[1].selected);
            assert_ne!(after.primary, before.primary);
        });
    }

    #[gpui::test]
    fn switching_banks_keeps_nodes_in_their_authored_bank(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            select_bank(fixture, 1, cx).unwrap();
            assert!(model(fixture, cx).unwrap().nodes.is_empty());
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            let state = current(fixture, cx).unwrap();
            assert_eq!(
                state.nodes.iter().map(|node| node.bank).collect::<Vec<_>>(),
                [0, 1]
            );
            select_bank(fixture, 0, cx).unwrap();
            let model = model(fixture, cx).unwrap();
            assert_eq!(model.nodes.len(), 1);
            assert_eq!(model.nodes[0].id, 1);
        });
    }

    #[gpui::test]
    fn numeric_begin_event_can_arrive_after_the_control_has_changed(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        use std::{cell::RefCell, rc::Rc};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let capture = seen.clone();
        let _subscription = view.update(cx, |view, cx| {
            let number = view.fixtures[&Entry::Eq].numbers[2].entity.clone();
            cx.subscribe(
                &number,
                move |_, control, event: &NumericControlEvent, cx| {
                    capture
                        .borrow_mut()
                        .push((event.phase, event.value, control.read(cx).value()));
                },
            )
        });
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(field.x, field.y - px(20.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let observed = seen.borrow();
        let begin = observed
            .iter()
            .find(|(phase, _, _)| *phase == hex_widgets::NumericEventPhase::Begin)
            .unwrap();
        assert_eq!(begin.1, 6.0);
        assert_ne!(
            begin.2, begin.1,
            "begin must use the event's before-value, not the live control"
        );
    }

    #[gpui::test]
    fn draft_barrier_finishes_captured_numeric_drag_once(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        let moved = point(field.x, field.y - px(20.0));
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let number = view.read_with(cx, |view, _| {
            view.fixtures[&Entry::Eq].numbers[2].entity.clone()
        });
        assert!(number.read_with(cx, |number, _| number.is_interacting()));
        visual.update(|_, cx| view.update(cx, |view, cx| assert!(view.finish_drafts(cx))));
        let accepted = number.read_with(cx, |number, _| number.value());
        assert_ne!(accepted, 6.0);
        assert!(!number.read_with(cx, |number, _| number.is_interacting()));
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(number.read_with(cx, |number, _| number.value()), accepted);
    }

    #[gpui::test]
    fn draft_barrier_validates_all_controls_before_accepting_any(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, px, size};
        cx.update(hex_widgets::bind_numeric_control_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let q = visual
            .debug_bounds("fixture-eq-numeric-3-value")
            .unwrap()
            .center();
        let gain = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_click(q, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid q");
        visual.simulate_click(gain, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("-12");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let fixture = &view.fixtures[&Entry::Eq];
            assert!(fixture.numbers[2].entity.read(cx).is_editing());
            assert!(fixture.numbers[3].entity.read(cx).is_editing());
            assert_eq!(fixture.numbers[2].entity.read(cx).value(), 6.0);
        });
        visual.update(|_, cx| view.update(cx, |view, cx| assert!(!view.finish_drafts(cx))));
        view.read_with(cx, |view, cx| {
            let fixture = &view.fixtures[&Entry::Eq];
            assert!(fixture.numbers[2].entity.read(cx).is_editing());
            assert_eq!(fixture.numbers[2].entity.read(cx).value(), 6.0);
            assert!(view.error.is_some());
        });
    }

    #[gpui::test]
    fn draft_barrier_accepts_without_deferred_numeric_events(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, px, size};
        use std::{cell::RefCell, rc::Rc};
        cx.update(hex_widgets::bind_numeric_control_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let capture = seen.clone();
        let _subscription = view.update(cx, |view, cx| {
            let number = view.fixtures[&Entry::Eq].numbers[2].entity.clone();
            cx.subscribe(&number, move |_, _, event: &NumericControlEvent, _| {
                capture.borrow_mut().push(event.phase)
            })
        });
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("-12");
        visual.run_until_parked();
        seen.borrow_mut().clear();
        visual.update(|_, cx| view.update(cx, |view, cx| assert!(view.finish_drafts(cx))));
        visual.run_until_parked();
        assert!(
            seen.borrow().is_empty(),
            "barrier must not leave deferred numeric events"
        );
        view.read_with(cx, |view, cx| {
            assert_eq!(
                current(&view.fixtures[&Entry::Eq], cx).unwrap().nodes[0].gain_db,
                -12.0
            )
        });
    }

    #[gpui::test]
    fn invalid_control_draft_refuses_graph_removal(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, px, size};
        cx.update(hex_widgets::bind_numeric_control_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid eq draft");
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        visual.update(|_, cx| {
            graph.update(cx, |_, cx| {
                cx.emit(event(ResponseGraphPhase::Remove, 1, [0.5, 0.625]));
            })
        });
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(view.error.is_some());
            assert_eq!(
                current(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            );
        });
    }

    #[gpui::test]
    fn invalid_control_draft_refuses_graph_creation(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_numeric_control_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid eq draft");
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.25,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_click(position, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(view.error.is_some());
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            );
        });
    }

    #[gpui::test]
    fn bank_buttons_preserve_node_bank_and_keep_one_choice_selected(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let left = visual.debug_bounds("fixture-eq-toggle-2").unwrap().center();
        visual.simulate_click(left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let fixture = &view.fixtures[&Entry::Eq];
            assert!(fixture.toggles[2].read(cx).checked());
            assert_eq!(current(fixture, cx).unwrap().nodes[0].bank, 0);
            assert!(model(fixture, cx).unwrap().nodes.is_empty());
        });
        let frequency = visual
            .debug_bounds("fixture-eq-numeric-1-value")
            .unwrap()
            .center();
        visual.simulate_click(frequency, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(
                !view.fixtures[&Entry::Eq].numbers[1]
                    .entity
                    .read(cx)
                    .is_editing()
            )
        });
        visual.simulate_click(left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(view.fixtures[&Entry::Eq].toggles[2].read(cx).checked())
        });
    }

    #[gpui::test]
    fn graph_click_creates_a_node_and_existing_node_remains_selectable(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_response_graph_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let at = |position: [f32; 2]| {
            point(
                plot.left() + plot.size.width * position[0],
                plot.top() + plot.size.height * (1.0 - position[1]),
            )
        };
        visual.simulate_click(at([0.25, 0.7]), Modifiers::default());
        visual.run_until_parked();
        let model_after = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        assert_eq!(model_after.nodes.len(), 2);
        assert!(model_after.nodes[1].selected);
        visual.simulate_click(at(model_after.nodes[0].position), Modifiers::default());
        visual.run_until_parked();
        let selected = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        assert!(selected.nodes[0].selected);
        assert!(!selected.nodes[1].selected);
    }

    #[gpui::test]
    fn graph_creates_and_disabling_during_capture_cancels_preview(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_response_graph_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.25,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                2
            )
        });
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.disabled = true;
                cx.notify();
            })
        });
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            )
        });
    }

    #[gpui::test]
    fn escape_during_graph_creation_restores_prior_nodes(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_response_graph_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.25,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                2
            )
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            )
        });
    }

    #[gpui::test]
    fn leaving_eq_during_capture_cancels_only_the_preview(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.25,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                2
            )
        });
        visual.update(|window, cx| {
            view.update(cx, |view, cx| view.choose_entry(Entry::Utility, window, cx))
        });
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            )
        });
    }

    #[gpui::test]
    fn marquee_group_drag_and_escape_preserve_complete_node_state(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_response_graph_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let at = |p: [f32; 2]| {
            point(
                plot.left() + plot.size.width * p[0],
                plot.top() + plot.size.height * (1.0 - p[1]),
            )
        };
        visual.simulate_click(at([0.25, 0.7]), Modifiers::default());
        visual.simulate_click(at([0.75, 0.7]), Modifiers::default());
        visual.run_until_parked();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_click(at([0.25, 0.7]), Modifiers::default());
        visual.simulate_click(at([0.75, 0.7]), shift);
        visual.simulate_click(at([0.75, 0.7]), shift);
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx)
                    .unwrap()
                    .nodes
                    .iter()
                    .filter(|node| node.selected)
                    .count(),
                2
            );
        });
        visual.simulate_mouse_down(at([0.25, 0.7]), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx)
                    .unwrap()
                    .nodes
                    .iter()
                    .filter(|node| node.selected)
                    .count(),
                2
            );
        });
        visual.simulate_mouse_up(at([0.25, 0.7]), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx)
                    .unwrap()
                    .nodes
                    .iter()
                    .filter(|node| node.selected)
                    .count(),
                1
            );
        });
        visual.simulate_mouse_down(at([0.15, 0.9]), MouseButton::Left, shift);
        visual.simulate_mouse_move(at([0.85, 0.4]), Some(MouseButton::Left), shift);
        visual.simulate_mouse_up(at([0.85, 0.4]), MouseButton::Left, shift);
        visual.run_until_parked();
        let before = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        assert_eq!(before.nodes.iter().filter(|node| node.selected).count(), 3);
        let toggle = Modifiers {
            shift: true,
            platform: true,
            ..Modifiers::default()
        };
        for expected in [2, 3] {
            visual.simulate_mouse_down(at([0.47, 0.82]), MouseButton::Left, toggle);
            visual.simulate_mouse_move(at([0.66, 0.44]), Some(MouseButton::Left), toggle);
            visual.simulate_mouse_up(at([0.66, 0.44]), MouseButton::Left, toggle);
            visual.run_until_parked();
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    model(&view.fixtures[&Entry::Eq], cx)
                        .unwrap()
                        .nodes
                        .iter()
                        .filter(|node| node.selected)
                        .count(),
                    expected
                )
            });
        }
        let origin = before.nodes[0].position;
        let target = [origin[0] + 0.05, origin[1] + 0.1];
        visual.simulate_mouse_down(at(origin), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(target), Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(at(target), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let after = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        for (old, new) in before.nodes.iter().zip(&after.nodes) {
            assert!((new.position[0] - old.position[0] - 0.05).abs() < 0.002);
            assert!((new.position[1] - old.position[1] - 0.1).abs() < 0.002);
        }
        assert_ne!(after.primary, before.primary);
        let start = after.nodes[0].position;
        let cancel_to = [start[0] + 0.05, start[1] - 0.1];
        visual.simulate_mouse_down(at(start), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(cancel_to), Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        let restored = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        assert_eq!(restored.nodes, after.nodes);
        visual.simulate_click(at(restored.nodes[0].position), Modifiers::default());
        visual.run_until_parked();
        let collapsed = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        assert_eq!(
            collapsed.nodes.iter().filter(|node| node.selected).count(),
            1
        );
    }

    #[gpui::test]
    fn escape_after_marquee_repaint_restores_prior_selection(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_response_graph_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let at = |p: [f32; 2]| {
            point(
                plot.left() + plot.size.width * p[0],
                plot.top() + plot.size.height * (1.0 - p[1]),
            )
        };
        visual.simulate_click(at([0.25, 0.7]), Modifiers::default());
        visual.simulate_click(at([0.75, 0.7]), Modifiers::default());
        visual.run_until_parked();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(at([0.15, 0.9]), MouseButton::Left, shift);
        visual.simulate_mouse_move(at([0.85, 0.4]), Some(MouseButton::Left), shift);
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx)
                    .unwrap()
                    .nodes
                    .iter()
                    .filter(|node| node.selected)
                    .count(),
                3
            )
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let nodes = model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes;
            assert_eq!(
                nodes
                    .iter()
                    .filter(|node| node.selected)
                    .map(|node| node.id)
                    .collect::<Vec<_>>(),
                [3]
            );
        });
    }

    #[gpui::test]
    fn stale_selection_events_cannot_select_nodes_from_another_bank(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            select_bank(fixture, 1, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            select_bank(fixture, 0, cx).unwrap();
            let stale = ResponseGraphMarqueeEvent {
                phase: ResponseGraphMarqueePhase::Commit,
                selected: vec![1, 2],
                modifiers: gpui::Modifiers::default(),
            };
            marquee(fixture, &stale, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Select, 2, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            let before = current(fixture, cx).unwrap();
            assert!(
                event_graph(fixture, event(ResponseGraphPhase::Begin, 2, [0.3, 0.7]), cx).is_err()
            );
            assert!(
                event_graph(
                    fixture,
                    event(ResponseGraphPhase::Change, 2, [0.4, 0.8]),
                    cx
                )
                .is_err()
            );
            assert_eq!(current(fixture, cx).unwrap(), before);
            let local = fixture.eq_local.as_ref().unwrap();
            assert_eq!(local.selected.unwrap().value(), 1);
            assert!(local.group.is_empty());
        });
    }

    #[gpui::test]
    fn group_drag_refuses_a_changed_node_kind_and_cancel_restores_state(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            let selection = ResponseGraphMarqueeEvent {
                phase: ResponseGraphMarqueePhase::Commit,
                selected: vec![1, 2],
                modifiers: gpui::Modifiers::default(),
            };
            marquee(fixture, &selection, cx).unwrap();
            let before = current(fixture, cx).unwrap();
            let start = node_geometry::node_position(&before.nodes[1], 48_000);
            event_graph(fixture, event(ResponseGraphPhase::Begin, 2, start), cx).unwrap();
            fixture.selectors[1].update(cx, |selector, cx| selector.set_selected(3, cx).unwrap());
            let error = event_graph(
                fixture,
                event(
                    ResponseGraphPhase::Change,
                    2,
                    [start[0] + 0.05, start[1] + 0.1],
                ),
                cx,
            )
            .unwrap_err();
            assert_eq!(error, "eq group node changed during drag");
            event_graph(fixture, event(ResponseGraphPhase::Cancel, 2, start), cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
        });
    }

    #[gpui::test]
    fn oversized_group_drag_is_refused_without_changing_nodes(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let local = fixture.eq_local.as_mut().unwrap();
            for raw in 2..=33 {
                let id = NodeId::new(raw).unwrap();
                local
                    .state
                    .add(Node::new(id, 1_000.0, 0.0, 0).unwrap())
                    .unwrap();
            }
            local.group = local.state.nodes.iter().map(|node| node.id).collect();
            let before = current(fixture, cx).unwrap();
            let error = event_graph(
                fixture,
                event(ResponseGraphPhase::Begin, 1, [0.5, 0.625]),
                cx,
            )
            .unwrap_err();
            assert_eq!(error, "eq group move is limited to 32 nodes");
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert!(fixture.eq_local.as_ref().unwrap().before.is_none());
        });
    }

    #[gpui::test]
    fn active_drag_refuses_bank_change_and_cancel_restores_state(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = current(fixture, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Begin, 1, [0.5, 0.625]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Change, 1, [0.7, 0.8]),
                cx,
            )
            .unwrap();
            assert!(select_bank(fixture, 1, cx).is_err());
            assert_eq!(fixture.eq_local.as_ref().unwrap().bank, 0);
            event_graph(
                fixture,
                event(ResponseGraphPhase::Cancel, 1, [0.7, 0.8]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
        });
    }

    #[gpui::test]
    fn mixed_group_drag_uses_gain_and_q_and_cancel_restores_both(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            fixture.selectors[1].update(cx, |selector, cx| selector.set_selected(3, cx).unwrap());
            let selection = ResponseGraphMarqueeEvent {
                phase: ResponseGraphMarqueePhase::Commit,
                selected: vec![1, 2],
                modifiers: gpui::Modifiers::default(),
            };
            marquee(fixture, &selection, cx).unwrap();
            let before = current(fixture, cx).unwrap();
            let start = node_geometry::node_position(&before.nodes[1], 48_000);
            event_graph(fixture, event(ResponseGraphPhase::Begin, 2, start), cx).unwrap();
            event_graph(
                fixture,
                event(
                    ResponseGraphPhase::Change,
                    2,
                    [start[0] + 0.05, start[1] + 0.1],
                ),
                cx,
            )
            .unwrap();
            let moved = current(fixture, cx).unwrap();
            assert!(moved.nodes[0].gain_db > before.nodes[0].gain_db);
            assert_eq!(moved.nodes[1].gain_db, before.nodes[1].gain_db);
            assert!(moved.nodes[1].q > before.nodes[1].q);
            event_graph(fixture, event(ResponseGraphPhase::Cancel, 2, start), cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert_eq!(fixture.eq_local.as_ref().unwrap().group.len(), 2);
        });
    }

    #[gpui::test]
    fn scroll_stream_clamps_and_cancel_restores_state(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            fixture.selectors[1].update(cx, |selector, cx| selector.set_selected(3, cx).unwrap());
            let before = current(fixture, cx).unwrap();
            let event = |phase, gesture_id, steps| ResponseGraphScrollEvent {
                phase,
                node_id: 1,
                gesture_id,
                steps,
            };
            scroll(fixture, event(ResponseGraphScrollPhase::Begin, 7, 0), cx).unwrap();
            scroll(fixture, event(ResponseGraphScrollPhase::Change, 7, 10), cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap().nodes[0].slope, 3);
            scroll(fixture, event(ResponseGraphScrollPhase::Change, 8, -10), cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap().nodes[0].slope, 3);
            assert!(select_bank(fixture, 1, cx).is_err());
            scroll(fixture, event(ResponseGraphScrollPhase::Change, 7, 9), cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap().nodes[0].slope, 2);
            scroll(fixture, event(ResponseGraphScrollPhase::Cancel, 7, 9), cx).unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert!(fixture.eq_local.as_ref().unwrap().scroll.is_none());
        });
    }

    #[gpui::test]
    fn scroll_rejects_non_cut_and_foreign_bank_nodes(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let event = ResponseGraphScrollEvent {
                phase: ResponseGraphScrollPhase::Begin,
                node_id: 1,
                gesture_id: 1,
                steps: 0,
            };
            assert!(scroll(fixture, event, cx).is_err());
            let local = fixture.eq_local.as_mut().unwrap();
            local.bank = 1;
            fixture.selectors[1].update(cx, |selector, cx| selector.set_selected(4, cx).unwrap());
            assert!(scroll(fixture, event, cx).is_err());
            assert!(fixture.eq_local.as_ref().unwrap().scroll.is_none());
        });
    }

    #[gpui::test]
    fn wheel_on_cut_node_changes_slope(cx: &mut TestAppContext) {
        use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.fixtures[&Entry::Eq].selectors[1]
                    .update(cx, |kind, cx| kind.set_selected(3, cx).unwrap());
                cx.notify();
            })
        });
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes[0].position
        });
        let screen = point(
            plot.left() + plot.size.width * position[0],
            plot.top() + plot.size.height * (1.0 - position[1]),
        );
        visual.simulate_event(ScrollWheelEvent {
            position: screen,
            delta: ScrollDelta::Lines(point(0.0, 1.0)),
            touch_phase: TouchPhase::Started,
            ..Default::default()
        });
        visual.simulate_event(ScrollWheelEvent {
            position: screen,
            delta: ScrollDelta::default(),
            touch_phase: TouchPhase::Ended,
            ..Default::default()
        });
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.fixtures[&Entry::Eq].selectors[2].read(cx).selected(),
                1
            )
        });
    }

    #[gpui::test]
    fn cut_and_notch_positions_follow_q(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let (kind, q) = view.read_with(cx, |view, _| {
            let fixture = &view.fixtures[&Entry::Eq];
            (
                fixture.selectors[1].clone(),
                fixture.numbers[3].entity.clone(),
            )
        });
        q.update(cx, |q, cx| q.set_external_value(18.0, cx).unwrap());
        for index in [3, 4, 5] {
            kind.update(cx, |kind, cx| kind.set_selected(index, cx).unwrap());
            let model = view.read_with(cx, |view, cx| {
                model(&view.fixtures[&Entry::Eq], cx).unwrap()
            });
            assert!(
                (model.nodes[0].position[1] - 1.0).abs() < 0.0001,
                "kind {index}: {:?}",
                model.nodes[0].position
            );
        }
    }
}
