use super::*;
use gpui::{AppContext, Render, TestAppContext, VisualTestContext};

struct Host {
    graph: Entity<ResponseGraph>,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.graph.clone())
    }
}

#[test]
fn circular_markers_fill_for_hover_or_selection_and_keep_square_pixel_bounds() {
    let mut node = ResponseGraphNode {
        id: 7,
        position: [0.2, 0.8],
        enabled: true,
        selected: false,
    };
    for center in [point(px(4.0), px(11.0)), point(px(300.0), px(45.0))] {
        let normal = circular_node_quad(center, px(3.0), node, None, crate::tests::STYLE);
        assert_eq!(normal.bounds.size.width, normal.bounds.size.height);
        assert_eq!(normal.corner_radii.top_left, px(3.0));
        assert_eq!(normal.corner_radii.top_right, px(3.0));
        assert_eq!(normal.corner_radii.bottom_left, px(3.0));
        assert_eq!(normal.corner_radii.bottom_right, px(3.0));
        assert!(normal.background.is_transparent());
        let hovered = circular_node_quad(center, px(3.0), node, Some(7), crate::tests::STYLE);
        assert!(!hovered.background.is_transparent());
        node.selected = true;
        let selected = circular_node_quad(center, px(4.0), node, None, crate::tests::STYLE);
        assert_eq!(selected.bounds.size.width, selected.bounds.size.height);
        assert_eq!(selected.corner_radii.top_left, px(4.0));
        assert!(!selected.background.is_transparent());
        node.selected = false;
    }
}

#[gpui::test]
fn hover_tracks_existing_node_hits_and_selection_survives_pointer_leave(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, cx| Host {
        graph: cx.new(|cx| {
            let mut graph = ResponseGraph::new(cx, "circle-test", crate::tests::STYLE);
            graph.set_circular_nodes(true, cx);
            graph.set_editable(true, cx);
            let mut model = graph.model.clone();
            model.nodes = vec![ResponseGraphNode {
                id: 7,
                position: [0.5, 0.5],
                enabled: true,
                selected: false,
            }];
            graph.set_model(model, crate::tests::STYLE, cx).unwrap();
            graph
        }),
    });
    let graph = handle.read_with(cx, |host, _| host.graph.clone()).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(300.0), px(120.0)));
    visual.run_until_parked();
    let center = graph.read_with(&visual, |g, _| g.plot_bounds().unwrap().center());
    visual.simulate_mouse_move(center, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(graph.read_with(&visual, |g, _| g.hovered_node), Some(7));
    visual.simulate_mouse_move(point(px(400.0), px(140.0)), None, Modifiers::default());
    visual.run_until_parked();
    graph.read_with(&visual, |g, _| {
        assert_eq!(g.hovered_node, None);
        assert!(
            circular_node_quad(center, px(3.0), g.model.nodes[0], g.hovered_node, g.style)
                .background
                .is_transparent()
        );
    });
    visual.simulate_mouse_move(center, None, Modifiers::default());
    visual.run_until_parked();
    graph.update(&mut visual, |g, cx| {
        let mut model = g.model.clone();
        model.nodes[0].selected = true;
        g.set_model(model, crate::tests::STYLE, cx).unwrap();
    });
    visual.simulate_mouse_move(point(px(400.0), px(140.0)), None, Modifiers::default());
    visual.run_until_parked();
    graph.read_with(&visual, |g, _| {
        assert_eq!(g.hovered_node, None);
        assert!(
            !circular_node_quad(center, px(4.0), g.model.nodes[0], g.hovered_node, g.style)
                .background
                .is_transparent()
        );
    });
    graph.update(&mut visual, |g, cx| {
        let mut model = g.model.clone();
        model.nodes[0].selected = false;
        g.set_model(model, crate::tests::STYLE, cx).unwrap();
    });
    assert!(graph.read_with(&visual, |g, _| {
        circular_node_quad(center, px(3.0), g.model.nodes[0], g.hovered_node, g.style)
            .background
            .is_transparent()
    }));
    visual.simulate_resize(size(px(120.0), px(300.0)));
    visual.run_until_parked();
    let resized_center = graph.read_with(&visual, |g, _| g.plot_bounds().unwrap().center());
    visual.simulate_mouse_move(resized_center, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(graph.read_with(&visual, |g, _| g.hovered_node), Some(7));
}
