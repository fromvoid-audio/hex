//! Control-side authored response sampling; no live/audio ownership.
use super::*;

// Same 256-query limit as the registered response API and shared graph. All
// 64 authored bands retain a center and two shoulder samples, even at capacity.
const MAX_RESPONSE_POINTS: usize = 256;
const BASE_RESPONSE_POINTS: usize = 96;
pub(super) type Curves = [Vec<[f32; 2]>; 2];

fn frequencies(state: &EqState, rate: u32) -> Vec<f32> {
    let upper = (rate as f32 * 0.49).min(20000.0);
    let active = state.nodes.iter().filter(|node| node.enabled).count();
    let base = BASE_RESPONSE_POINTS.min(MAX_RESPONSE_POINTS - 3 * active);
    let mut points = Vec::with_capacity(base + 3 * active);
    for i in 0..base {
        points.push((20.0 * ((upper / 20.0).ln() * i as f32 / (base - 1) as f32).exp()).min(upper));
    }
    for node in state.nodes.iter().filter(|node| node.enabled) {
        let center = node.frequency_hz.min(rate as f32 * 0.49);
        // Bounded shoulders make narrow authored features readable. These are
        // display samples, not a second filter/Q law or a uniform-error promise.
        let half_width = 0.5 / node.q;
        for f in [
            center * (-half_width).exp(),
            center,
            center * half_width.exp(),
        ] {
            points.push(f.clamp(20.0, upper));
        }
    }
    points.sort_unstable_by(f32::total_cmp);
    points.dedup();
    points
}

pub(super) fn curves(authored: &NativeDeviceState, state: &EqState, rate: u32) -> Option<Curves> {
    let frequencies = frequencies(state, rate);
    let response =
        hex_engine::native_effect_frequency_response(authored, rate, &frequencies).ok()??;
    Some(std::array::from_fn(|channel| {
        frequencies
            .iter()
            .zip(&response)
            .map(|(frequency, m)| {
                [
                    super::axes::x(*frequency, rate),
                    ((m[channel].max(0.00001).log10() * 20.0 + 24.0) / 48.0).clamp(0.0, 1.0),
                ]
            })
            .collect()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_notch_curve_preserves_actual_center_depth() {
        let mut state = EqState::default();
        let mut node = Node::new(NodeId::new(1).unwrap(), 1000.0, 0.0, 0).unwrap();
        node.kind = eq::NodeKind::Notch;
        node.q = 18.0;
        state.add(node).unwrap();
        let authored = state.encode().unwrap();
        let actual = hex_engine::native_effect_frequency_response(&authored, 48000, &[1000.0])
            .unwrap()
            .unwrap();
        assert!(actual[0][0] < 0.00001, "production notch has a deep center");
        let [primary, secondary] = curves(&authored, &state, 48000).unwrap();
        let model = ResponseGraphModel {
            primary,
            secondary,
            analyzer: None,
            input_analyzer: None,
            nodes: vec![],
        };
        model.validate().unwrap();
        let minimum = model.primary.iter().map(|p| p[1]).fold(1.0_f32, f32::min);
        assert!(
            minimum < 0.001,
            "rendered notch must reach its -24db display floor; normalized minimum={minimum}"
        );
        let x = (1000.0_f32 / 20.0).ln() / (20000.0_f32 / 20.0).ln();
        assert!(
            model
                .primary
                .iter()
                .any(|p| (p[0] - x).abs() < 0.000001 && p[1] < 0.001),
            "the notch belongs at its actual log-frequency coordinate"
        );
    }
    #[test]
    fn all_64_centers_fit_without_dropping_banks_or_exceeding_graph_capacity() {
        let mut state = EqState::default();
        for i in 0..eq::MAX_NODES {
            let mut node = Node::new(
                NodeId::new(i as u64 + 1).unwrap(),
                (20.0 * (1000.0_f32.ln() * i as f32 / 63.0).exp()).min(20000.0),
                if i % 2 == 0 { 12.0 } else { -12.0 },
                (i % 5) as u8,
            )
            .unwrap();
            node.q = 18.0;
            state.add(node).unwrap();
        }
        for rate in [8000, 44100, 48000, 96000, 192000] {
            let points = frequencies(&state, rate);
            assert!(points.len() <= 256);
            assert!(points.windows(2).all(|w| w[0] < w[1]));
            for node in &state.nodes {
                assert!(
                    points.contains(
                        &node
                            .frequency_hz
                            .min(rate as f32 * 0.49)
                            .clamp(20.0, 20000.0)
                    )
                );
            }
            let [primary, secondary] = curves(&state.encode().unwrap(), &state, rate).unwrap();
            ResponseGraphModel {
                primary,
                secondary,
                analyzer: None,
                input_analyzer: None,
                nodes: vec![],
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn flat_disabled_and_rate_clamped_curves_keep_actual_coordinates() {
        for rate in [8000, 192000] {
            let mut state = EqState::default();
            let mut node = Node::new(NodeId::new(1).unwrap(), 20000.0, 0.0, 0).unwrap();
            node.kind = eq::NodeKind::Notch;
            node.q = 18.0;
            node.enabled = false;
            state.add(node).unwrap();
            let [flat, _] = curves(&state.encode().unwrap(), &state, rate).unwrap();
            assert_eq!(flat.len(), 96);
            assert!(flat.iter().all(|p| (p[1] - 0.5).abs() < 1e-6));
            state.nodes[0].enabled = true;
            let [curve, _] = curves(&state.encode().unwrap(), &state, rate).unwrap();
            assert!((curve.last().unwrap()[0] - 1.0).abs() < 1e-6);
            assert!(curve.last().unwrap()[1] < 0.001);
        }
    }
}
