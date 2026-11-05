//! Bounded, view-local response-shape fixture. Coordinates illustrate frequency
//! and gain; interpolation is demonstration artwork, never a DSP/filter model.
//! The shared graph owns pointer capture. Its creation stream uses node zero;
//! this owner admits one stable local identity and retains its rollback snapshot.
use hex_widgets::{
    ResponseGraphEvent, ResponseGraphMarqueeEvent, ResponseGraphModel, ResponseGraphNode,
    ResponseGraphPhase,
};

const MAX_POINTS: usize = 16;

pub(super) struct ResponseDemo {
    pub model: ResponseGraphModel,
    before: Option<ResponseGraphModel>,
    created: Option<u32>,
}
impl ResponseDemo {
    pub fn new() -> Self {
        Self {
            model: model(vec![ResponseGraphNode {
                id: 1,
                position: [0.5, 0.7],
                enabled: true,
                selected: true,
            }]),
            before: None,
            created: None,
        }
    }
    pub fn can_create(&self) -> bool {
        self.model.nodes.len() < MAX_POINTS
    }
    pub fn event(&mut self, event: &ResponseGraphEvent) -> Option<u32> {
        if event
            .position
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return None;
        }
        let mut id = if event.node_id == 0 {
            self.created.unwrap_or(0)
        } else {
            event.node_id
        };
        match event.phase {
            ResponseGraphPhase::Create => {
                if !self.can_create() {
                    return None;
                }
                id = (1..=MAX_POINTS as u32)
                    .find(|id| self.model.nodes.iter().all(|n| n.id != *id))?;
                self.before = Some(self.model.clone());
                self.created = Some(id);
                for node in &mut self.model.nodes {
                    node.selected = false;
                }
                self.model.nodes.push(ResponseGraphNode {
                    id,
                    position: event.position,
                    enabled: true,
                    selected: true,
                });
            }
            ResponseGraphPhase::Begin => {
                // Creation's baseline precedes insertion, so Escape removes the
                // admitted point rather than keeping its first position.
                if event.node_id != 0 {
                    self.before = Some(self.model.clone());
                }
            }
            ResponseGraphPhase::Select => {
                for node in &mut self.model.nodes {
                    node.selected = node.id == id || (event.modifiers.shift && node.selected);
                }
            }
            ResponseGraphPhase::Remove => self.model.nodes.retain(|n| n.id != id),
            ResponseGraphPhase::Change | ResponseGraphPhase::Commit => {
                self.model.nodes.iter_mut().find(|n| n.id == id)?.position = event.position;
                if event.phase == ResponseGraphPhase::Commit {
                    self.before = None;
                }
            }
            ResponseGraphPhase::Release | ResponseGraphPhase::ReleaseOutside => {
                self.before = None;
                self.created = None;
            }
            ResponseGraphPhase::Cancel => {
                self.cancel();
            }
            _ => {}
        }
        self.model = model(self.model.nodes.clone());
        Some(id)
    }
    pub fn marquee(&mut self, event: &ResponseGraphMarqueeEvent) {
        for node in &mut self.model.nodes {
            node.selected = event.selected.contains(&node.id);
        }
    }
    pub fn cancel(&mut self) -> bool {
        self.created = None;
        if let Some(before) = self.before.take() {
            self.model = before;
            true
        } else {
            false
        }
    }
}
/// Demo X is logarithmic frequency, 20–20,000 Hz.
pub(super) fn frequency(x: f32) -> f32 {
    20.0 * 1000.0_f32.powf(x)
}
/// Demo Y is gain, -12–+12 dB with zero at the midpoint.
pub(super) fn gain(y: f32) -> f32 {
    (y - 0.5) * 24.0
}
fn model(nodes: Vec<ResponseGraphNode>) -> ResponseGraphModel {
    let mut anchors: Vec<_> = nodes.iter().map(|node| node.position).collect();
    anchors.extend([[0.0, 0.5], [1.0, 0.5]]);
    anchors.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut primary = Vec::new();
    if let Some(first) = anchors.first() {
        primary.push(*first);
    }
    // At most 17 segments * 12 subdivisions + 1 = 205 points, within the
    // existing shared graph's 256-point bound. Endpoints stay exact node values.
    for pair in anchors.windows(2) {
        for step in 1..=12 {
            if step == 12 {
                primary.push(pair[1]);
                continue;
            }
            let t = step as f32 / 12.0;
            let smooth = t * t * (3.0 - 2.0 * t);
            primary.push([
                pair[0][0] + (pair[1][0] - pair[0][0]) * t,
                pair[0][1] + (pair[1][1] - pair[0][1]) * smooth,
            ]);
        }
    }
    let secondary = primary.iter().map(|[x, _]| [*x, 0.5]).collect();
    ResponseGraphModel {
        primary,
        secondary,
        analyzer: None,
        input_analyzer: None,
        analyzer_peak: None,
        nodes,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;
    fn event(phase: ResponseGraphPhase, position: [f32; 2]) -> ResponseGraphEvent {
        ResponseGraphEvent {
            phase,
            position,
            node_id: 0,
            modifiers: Modifiers::default(),
        }
    }
    #[test]
    fn smooth_fixture_keeps_points_on_the_curve_and_labels_have_real_units() {
        let demo = ResponseDemo::new();
        assert!(demo.model.validate().is_ok());
        assert!(demo.model.primary.contains(&demo.model.nodes[0].position));
        let quarter = demo.model.primary.iter().find(|p| p[0] == 0.125).unwrap();
        assert!((quarter[1] - 0.53125).abs() < 1e-6);
        assert!(demo.model.secondary.iter().all(|p| p[1] == 0.5));
        assert!(demo.model.analyzer.is_none() && demo.model.input_analyzer.is_none());
        assert_eq!(frequency(0.0), 20.0);
        assert_eq!(frequency(1.0), 20_000.0);
        assert_eq!(gain(0.0), -12.0);
        assert_eq!(gain(1.0), 12.0);
    }
    #[test]
    fn capacity_refuses_new_points_atomically_and_keeps_geometry_bounded() {
        let mut demo = ResponseDemo::new();
        for _ in 1..MAX_POINTS {
            demo.event(&event(ResponseGraphPhase::Create, [0.25, 0.4]));
            demo.event(&event(ResponseGraphPhase::Release, [0.25, 0.4]));
        }
        assert!(!demo.can_create());
        let nodes = demo.model.nodes.clone();
        assert!(
            demo.event(&event(ResponseGraphPhase::Create, [0.9, 0.2]))
                .is_none()
        );
        assert_eq!(demo.model.nodes, nodes);
        assert!(demo.model.validate().is_ok());
        assert!(demo.model.primary.len() <= 256);
    }
}
