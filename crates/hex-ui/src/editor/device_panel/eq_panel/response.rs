//! Control-side authored response sampling; no live/audio ownership.
pub(super) use super::sampling::Curves;
#[cfg(all(test, feature = "ui-unit-tier"))]
use super::sampling::frequencies;
use super::*;

pub(super) fn curves(authored: &NativeDeviceState, state: &EqState, rate: u32) -> Option<Curves> {
    super::sampling::curves(state, rate, |frequencies| {
        hex_engine::native_effect_frequency_response(authored, rate, frequencies).ok()?
    })
}

#[cfg(all(test, feature = "ui-unit-tier"))]
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
            analyzer_peak: None,
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
                analyzer_peak: None,
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
