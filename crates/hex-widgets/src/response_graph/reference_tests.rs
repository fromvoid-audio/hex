use super::*;
use gpui::{AppContext, TestAppContext};

fn reference() -> ResponseGraphReference {
    ResponseGraphReference {
        points: vec![[0.0, 0.1], [1.0, 0.8]].into(),
        color: 0x7595a2,
        opacity: 0.25,
    }
}

#[gpui::test]
fn passive_references_validate_atomically_and_leave_editable_model_untouched(
    cx: &mut TestAppContext,
) {
    let graph = cx.new(|cx| ResponseGraph::new(cx, "references", crate::tests::STYLE));
    graph.update(cx, |graph, cx| {
        let mut model = graph.model.clone();
        model.nodes.push(ResponseGraphNode {
            id: 7,
            position: [0.5, 0.5],
            enabled: true,
            selected: true,
        });
        graph.set_model(model, crate::tests::STYLE, cx).unwrap();
        let active = graph.model.primary.clone();
        let nodes = graph.model.nodes.clone();
        let valid = vec![reference(); 4];
        graph.set_references(valid.clone(), cx).unwrap();
        assert_eq!(graph.references(), &valid);
        let retained = graph.references.clone();
        graph.set_references(valid.clone(), cx).unwrap();
        assert!(std::sync::Arc::ptr_eq(&retained, &graph.references));
        let mut invalids = vec![vec![reference(); 5]];
        for points in [
            vec![[0.0, 0.0]],
            vec![[0.5, 0.0], [0.0, 1.0]],
            vec![[0.0, f32::NAN], [1.0, 1.0]],
            vec![[0.0, 0.0]; MAX_CURVE_POINTS + 1],
        ] {
            let mut curve = reference();
            curve.points = points.into();
            invalids.push(vec![curve]);
        }
        for opacity in [f32::NAN, -0.01, 1.01] {
            let mut curve = reference();
            curve.opacity = opacity;
            invalids.push(vec![curve]);
        }
        let mut bad_color = reference();
        bad_color.color = 0x1000000;
        invalids.push(vec![bad_color]);
        for invalid in invalids {
            assert!(graph.set_references(invalid, cx).is_err());
            assert_eq!(graph.references(), &valid);
            assert_eq!(graph.model.primary, active);
            assert_eq!(graph.model.nodes, nodes);
            assert!(graph.press.is_none() && graph.drag.is_none());
        }
        graph.set_references(vec![], cx).unwrap();
        assert!(graph.references().is_empty());
        assert_eq!(graph.model.primary, active);
        assert_eq!(graph.model.nodes, nodes);
    });
}
