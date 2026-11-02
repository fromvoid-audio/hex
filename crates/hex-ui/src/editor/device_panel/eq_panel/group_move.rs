//! Pure EQ graph coordinates and grouped-node movement; no gesture custody.
use super::{eq::NodeKind, node_geometry::edits_q};

/// Group drags carry two parameters per node in the host snapshot.
pub(super) const MAX_GROUP_NODES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Vertical {
    Gain(f32),
    Q(f32),
}

pub(super) fn frequency_from_x(x: f32, rate: u32) -> f32 {
    let upper = (rate as f32 * 0.49).clamp(20.0, 20000.0);
    (20.0 * ((upper / 20.0).ln() * x.clamp(0.0, 1.0)).exp()).clamp(20.0, upper)
}

pub(super) fn q_from_y(y: f32) -> f32 {
    (0.1 * ((18.0_f32 / 0.1).ln() * y.clamp(0.0, 1.0)).exp()).clamp(0.1, 18.0)
}

/// Apply a pointer delta to a node's original normalized position.
pub(super) fn values(
    kind: NodeKind,
    origin: [f32; 2],
    delta: [f32; 2],
    rate: u32,
) -> (f32, Vertical) {
    let x = (origin[0] + delta[0]).clamp(0.0, 1.0);
    let y = (origin[1] + delta[1]).clamp(0.0, 1.0);
    let vertical = if edits_q(kind) {
        Vertical::Q(q_from_y(y))
    } else {
        Vertical::Gain(y * 48.0 - 24.0)
    };
    (frequency_from_x(x, rate), vertical)
}

/// Bell option-drag maps pointer displacement relative to the original Q.
pub(super) fn option_q(initial: f32, displacement: f32) -> f32 {
    (initial * ((18.0_f32 / 0.1).ln() * displacement).exp()).clamp(0.1, 18.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_nodes_share_displacement_and_keep_their_vertical_semantics() {
        let (low, gain) = values(NodeKind::Bell, [0.2, 0.5], [0.1, 0.25], 48_000);
        let (high, q) = values(NodeKind::LowCut, [0.4, 0.5], [0.1, 0.25], 48_000);
        assert!(high > low);
        assert_eq!(gain, Vertical::Gain(12.0));
        assert_eq!(q, Vertical::Q(q_from_y(0.75)));
        assert_eq!(
            values(NodeKind::Notch, [0.4, 0.5], [0.1, 0.25], 48_000).1,
            q
        );
    }

    #[test]
    fn grouped_motion_clamps_to_the_rate_and_vertical_bounds() {
        let (low, gain) = values(NodeKind::Bell, [0.2, 0.4], [-1.0, -1.0], 8_000);
        let (high, q) = values(NodeKind::HighCut, [0.8, 0.8], [1.0, 1.0], 8_000);
        assert_eq!(low, 20.0);
        assert_eq!(gain, Vertical::Gain(-24.0));
        assert!((high - 3_920.0).abs() < 0.001);
        assert!((q_from_y(1.0) - 18.0).abs() < 0.0001);
        assert_eq!(q, Vertical::Q(q_from_y(1.0)));
    }
}
