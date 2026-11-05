//! Bounded semantic control snapshots. Native backends never retain application pointers.
use crate::{App, Bounds, Pixels, SharedString, Window};
use std::{
    rc::Rc,
    sync::{Arc, Weak},
};

/// A control-owned lifetime lease. Native elements hold only a weak lease.
#[derive(Clone, Debug, Default)]
pub struct AccessibilityLifetime(Arc<()>);
impl AccessibilityLifetime {
    /// Associate a descriptor with this actual control owner.
    pub fn weak(&self) -> Weak<()> {
        Arc::downgrade(&self.0)
    }
}

/// Maximum semantic controls published by one window.
pub const MAX_ACCESSIBILITY_NODES: usize = 4096;
/// Maximum UTF-8 bytes in a semantic label or displayed value.
pub const MAX_ACCESSIBILITY_TEXT_BYTES: usize = 4096;
/// Aggregate owned UTF-8 bytes in one semantic snapshot (two MiB).
pub const MAX_ACCESSIBILITY_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;

/// An owner-lifetime identity, scoped to its window. Never reuse it for a new owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AccessibilityId(pub u64);

/// User-facing semantics of a control, independent of its painted shape.
#[derive(Clone, Debug, PartialEq)]
pub enum AccessibilityValue {
    /// A stateless action, with no editable value or option inventory.
    Button,
    /// A finite adjustable canonical value and its actual formatted readout.
    Number {
        /// Canonical current value.
        value: f64,
        /// Inclusive minimum.
        min: f64,
        /// Inclusive maximum.
        max: f64,
        /// Existing control formatter output, including units.
        text: SharedString,
    },
    /// A boolean control.
    Toggle(bool),
    /// A bounded ordered choice control.
    Choice {
        /// Current option.
        selected: usize,
        /// Original visible option order.
        options: Vec<SharedString>,
    },
}

/// An action requested by assistive technology. The control revalidates it at execution.
#[derive(Clone, Debug, PartialEq)]
pub enum AccessibilityAction {
    /// Move keyboard focus without editing the parameter.
    Focus,
    /// Activate a stateless action, boolean, or choice control.
    Press,
    /// Apply the control's existing positive keyboard step.
    Increment,
    /// Apply the control's existing negative keyboard step.
    Decrement,
    /// Set a finite canonical numeric value using the ordinary edit transaction.
    SetNumber(f64),
    /// Select an existing option through the ordinary choice event.
    Choose(usize),
}

/// A semantic descriptor. Geometry is supplied only by actual layout, separately.
#[derive(Clone, Debug)]
pub struct AccessibilityDescriptor {
    /// Stable owner identity (a generational GPUI entity ID is suitable).
    pub id: AccessibilityId,
    /// Acknowledged owner revision; queued requests must match it exactly.
    pub revision: u64,
    /// Weak actual-owner lifetime; native caches must retire expired owners.
    pub lifetime: Weak<()>,
    /// Human-readable parameter name, never a machine identifier.
    pub name: SharedString,
    /// Actual control value and domain.
    pub value: AccessibilityValue,
    /// Whether edits are currently permitted.
    pub enabled: bool,
    /// Whether the actual control currently owns keyboard focus.
    pub focused: bool,
    /// Explicit draft/pending status, never folded into the acknowledged value.
    pub status: Option<SharedString>,
}

/// A fully laid-out semantic node. Offscreen controls must not invent this frame.
#[derive(Clone, Debug)]
pub struct AccessibilityNode {
    /// Current control semantics.
    pub descriptor: AccessibilityDescriptor,
    /// Actual viewport-clipped window content bounds.
    pub bounds: Bounds<Pixels>,
}

impl AccessibilityDescriptor {
    /// Reject invalid external semantics before publication or action dispatch.
    pub fn is_valid(&self) -> bool {
        let text_ok = |text: &SharedString| {
            !text.is_empty() && text.len() <= MAX_ACCESSIBILITY_TEXT_BYTES && !text.contains('\0')
        };
        self.id.0 != 0
            && self.lifetime.strong_count() > 0
            && text_ok(&self.name)
            && self.status.as_ref().is_none_or(text_ok)
            && match &self.value {
                AccessibilityValue::Number {
                    value,
                    min,
                    max,
                    text,
                } => {
                    value.is_finite()
                        && min.is_finite()
                        && max.is_finite()
                        && min < max
                        && (*min..=*max).contains(value)
                        && text_ok(text)
                }
                AccessibilityValue::Button | AccessibilityValue::Toggle(_) => true,
                AccessibilityValue::Choice { selected, options } => {
                    !options.is_empty()
                        && options.len() <= 128
                        && *selected < options.len()
                        && options.iter().all(text_ok)
                }
            }
    }

    fn text_bytes(&self) -> usize {
        self.name.len()
            + self.status.as_ref().map_or(0, |text| text.len())
            + match &self.value {
                AccessibilityValue::Number { text, .. } => text.len(),
                AccessibilityValue::Button | AccessibilityValue::Toggle(_) => 0,
                AccessibilityValue::Choice { options, .. } => {
                    options.iter().map(|text| text.len()).sum()
                }
            }
    }

    /// Check action shape against this exact current descriptor, not a stale native copy.
    pub fn permits(&self, action: &AccessibilityAction) -> bool {
        if !self.is_valid() || !self.enabled {
            return false;
        }
        match (action, &self.value) {
            (AccessibilityAction::Focus, _) => true,
            (
                AccessibilityAction::Press,
                AccessibilityValue::Button
                | AccessibilityValue::Toggle(_)
                | AccessibilityValue::Choice { .. },
            ) => true,
            (
                AccessibilityAction::Increment | AccessibilityAction::Decrement,
                AccessibilityValue::Number { .. },
            ) => true,
            (AccessibilityAction::SetNumber(v), AccessibilityValue::Number { min, max, .. }) => {
                v.is_finite() && (*min..=*max).contains(v)
            }
            (AccessibilityAction::Choose(index), AccessibilityValue::Choice { options, .. }) => {
                *index < options.len()
            }
            _ => false,
        }
    }
}

pub(crate) type AccessibilityCallback =
    Rc<dyn Fn(&AccessibilityRequest, &mut Window, &mut App) -> bool>;

#[derive(Default)]
pub(crate) struct AccessibilityFrame {
    pub nodes: Vec<(AccessibilityNode, AccessibilityCallback)>,
    pub invalid: bool,
    text_bytes: usize,
}

impl AccessibilityFrame {
    pub fn insert(&mut self, node: AccessibilityNode, callback: AccessibilityCallback) {
        let bounds = node.bounds;
        let valid_bounds = [
            bounds.origin.x.0,
            bounds.origin.y.0,
            bounds.size.width.0,
            bounds.size.height.0,
        ]
        .iter()
        .all(|v| v.is_finite())
            && bounds.size.width.0 > 0.0
            && bounds.size.height.0 > 0.0;
        if self.invalid {
            return;
        }
        if !node.descriptor.is_valid()
            || node.descriptor.text_bytes()
                > MAX_ACCESSIBILITY_SNAPSHOT_BYTES.saturating_sub(self.text_bytes)
            || !valid_bounds
            || self.nodes.len() >= MAX_ACCESSIBILITY_NODES
            || self
                .nodes
                .iter()
                .any(|(n, _)| n.descriptor.id == node.descriptor.id)
        {
            // Refuse the whole invalid snapshot; never expose a misleading partial tree.
            self.invalid = true;
            return;
        }
        self.text_bytes += node.descriptor.text_bytes();
        self.nodes.push((node, callback));
    }
}

/// Owner-supplied acknowledged value and current admission guard.
/// The guard runs only on the GPUI UI thread; native snapshots never receive it.
#[derive(Clone)]
pub struct AccessibilityBinding {
    /// Exact acknowledged project/control revision.
    pub revision: u64,
    /// Actual human-readable parameter name supplied by its owner.
    pub name: SharedString,
    /// Acknowledged value, independent of local/pending painted drafts.
    pub value: AccessibilityValue,
    /// Current pending canonical transaction.
    pub pending: bool,
    /// Revalidate stable ownership, revision and current pending/gesture barriers.
    pub guard: Rc<dyn Fn(&App) -> bool>,
}

/// A bounded native request; acceptance means dispatch, not canonical commit.
#[derive(Clone, Debug)]
pub struct AccessibilityRequest {
    /// Captured owner identity.
    pub id: AccessibilityId,
    /// Captured acknowledged revision.
    pub revision: u64,
    /// Typed payload, revalidated against current semantics.
    pub action: AccessibilityAction,
}
