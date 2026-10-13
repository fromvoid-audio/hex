use super::model::*;
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{Command, Created, DeviceSnapshot, Editor, Project};
use std::collections::BTreeMap;
fn limits(graph: &Graph) -> Limits {
    Limits {
        nodes: graph.nodes.len(),
        edges: graph.edges.len(),
        delay: u64::MAX,
        coexist_bytes: usize::MAX,
        node_capacity: graph.nodes.len(),
        edge_capacity: graph.edges.len(),
        ring_quantum: 1,
    }
}
fn zeros(graph: &Graph) -> BTreeMap<NodeKey, Fixed> {
    graph
        .nodes
        .iter()
        .map(|n| (n.key, Fixed::Known(0)))
        .collect()
}
fn device(next: &mut u64) -> DeviceSnapshot {
    let id = *next;
    *next += 1;
    DeviceSnapshot {
        id,
        bypassed: false,
        state: NativeDeviceState::new(
            DeviceTypeId::new("private.latency-study").unwrap(),
            1,
            vec![1, 2],
        )
        .unwrap(),
    }
}
fn fixture() -> Project {
    let mut e = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let mut tracks = Vec::new();
    let mut buses = Vec::new();
    for _ in 0..2 {
        let Created::Track(id) = e
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        tracks.push(id);
    }
    for _ in 0..2 {
        let Created::ReturnTrack(id) = e
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        buses.push(id);
    }
    for track in tracks {
        e.execute(Command::SetReturnSend {
            track,
            return_track: buses[0],
            gain: 0.5,
        })
        .unwrap();
        e.execute(Command::SetReturnSend {
            track,
            return_track: buses[1],
            gain: 0.25,
        })
        .unwrap();
    }
    e.execute(Command::SetReturnFeed {
        source: buses[0],
        destination: buses[1],
        gain: 0.75,
    })
    .unwrap();
    let mut s = e.project().snapshot();
    s.tracks[0].devices = vec![device(&mut s.next_id), device(&mut s.next_id)];
    s.tracks[0].instrument = Some(device(&mut s.next_id));
    s.tracks[0].instrument_position = Some(1);
    for bus in &mut s.return_tracks {
        bus.devices = vec![device(&mut s.next_id)];
    }
    Project::from_snapshot(s).unwrap()
}
fn declared_graph(p: &Project) -> (Graph, BTreeMap<NodeKey, Fixed>) {
    let graph = Graph::from_project(p, &BTreeMap::new(), &[]).unwrap();
    let declared = zeros(&graph);
    (Graph::from_project(p, &declared, &[]).unwrap(), declared)
}
fn edge(layout: &Layout, key: EdgeKey) -> EdgeLayout {
    *layout.edges.iter().find(|e| e.key == key).unwrap()
}
fn arrival(layout: &Layout, key: NodeKey) -> u64 {
    layout.nodes.iter().find(|n| n.key == key).unwrap().arrival
}
#[test]
fn zero_identity_uses_no_rings_and_keeps_canonical_pinned_order() {
    let p = fixture();
    let (g, _) = declared_graph(&p);
    let plan = prepare(&g, limits(&g), &Context::default()).unwrap();
    assert_eq!(plan.main_latency, 0);
    assert_eq!(plan.demand.ring_frames_per_bank, 0);
    assert_eq!(plan.demand.ring_twin_bytes, 0);
    assert_eq!(plan.demand.ring_metadata_bytes, 0);
    assert!(
        plan.edges
            .iter()
            .all(|e| e.delay == 0 && e.ring_capacity == 0)
    );
    let returns = p.return_tracks();
    assert!(
        plan.order
            .iter()
            .position(|k| *k == NodeKey::ReturnMix(returns[0].id()))
            .unwrap()
            < plan
                .order
                .iter()
                .position(|k| *k == NodeKey::ReturnInput(returns[1].id()))
                .unwrap()
    );
    assert_eq!(plan.order.last(), Some(&NodeKey::Main));
}
#[test]
fn sixteen_frame_serial_parallel_hybrid_sends_feeds_and_main_align_at_each_merge() {
    let p = fixture();
    let (_, mut d) = declared_graph(&p);
    let track = &p.tracks()[0];
    let buses = p.return_tracks();
    d.insert(NodeKey::Device(track.devices()[0].id()), Fixed::Known(16));
    d.insert(
        NodeKey::Instrument(track.instrument().unwrap().id()),
        Fixed::Known(4),
    );
    d.insert(NodeKey::Device(track.devices()[1].id()), Fixed::Known(8));
    for bus in buses {
        d.insert(NodeKey::Device(bus.devices()[0].id()), Fixed::Known(16));
    }
    let g = Graph::from_project(&p, &d, &[]).unwrap();
    let plan = prepare(&g, limits(&g), &Context::default()).unwrap();
    assert_eq!(arrival(&plan, NodeKey::Merge(track.id())), 16);
    assert_eq!(arrival(&plan, NodeKey::TrackMix(track.id())), 24);
    assert_eq!(
        edge(
            &plan,
            EdgeKey::Internal(
                NodeKey::Instrument(track.instrument().unwrap().id()),
                NodeKey::Merge(track.id())
            )
        )
        .delay,
        12
    );
    assert_eq!(arrival(&plan, NodeKey::ReturnMix(buses[0].id())), 40);
    assert_eq!(arrival(&plan, NodeKey::ReturnMix(buses[1].id())), 56);
    assert_eq!(plan.main_latency, 56);
    assert_eq!(edge(&plan, EdgeKey::TrackMain(track.id())).delay, 32);
    assert_eq!(
        edge(&plan, EdgeKey::TrackMain(p.tracks()[1].id())).delay,
        56
    );
    assert_eq!(
        edge(&plan, EdgeKey::Send(track.id(), buses[1].id())).delay,
        16
    );
    assert_eq!(
        edge(&plan, EdgeKey::Feed(p.authored_routing()[0].id())).delay,
        0
    );
    assert_eq!(edge(&plan, EdgeKey::ReturnMain(buses[0].id())).delay, 16);
    // Independent invariant oracle for every fan-in, not an audio rendering claim.
    for e in &g.edges {
        let Fixed::Known(own) = g.nodes[e.target].fixed else {
            panic!("known")
        };
        assert_eq!(
            arrival(&plan, g.nodes[e.source].key) + edge(&plan, e.key).delay,
            arrival(&plan, g.nodes[e.target].key) - own
        );
    }
}
#[test]
fn edge_permutations_leave_keyed_delays_and_pinned_topological_order_unchanged() {
    let p = fixture();
    let (_, mut d) = declared_graph(&p);
    d.insert(
        NodeKey::Device(p.return_tracks()[0].devices()[0].id()),
        Fixed::Known(16),
    );
    let mut g = Graph::from_project(&p, &d, &[]).unwrap();
    let before = prepare(&g, limits(&g), &Context::default()).unwrap();
    g.edges.reverse();
    let after = prepare(&g, limits(&g), &Context::default()).unwrap();
    let keyed = |plan: &Layout| {
        plan.edges
            .iter()
            .map(|e| (e.key, (e.delay, e.ring_capacity)))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(keyed(&before), keyed(&after));
    assert_eq!(before.nodes, after.nodes);
    assert_eq!(before.order, after.order);
}
#[test]
fn bypass_is_zero_in_the_proof_without_enabling_unknown_return_processors() {
    let p = fixture();
    let (_, mut d) = declared_graph(&p);
    let key = NodeKey::Device(p.return_tracks()[0].devices()[0].id());
    d.insert(key, Fixed::Unknown);
    let g = Graph::from_project(&p, &d, &[]).unwrap();
    assert_eq!(
        prepare(&g, limits(&g), &Context::default()).unwrap_err(),
        "unknown latency"
    );
    let mut s = p.snapshot();
    s.return_tracks[0].devices[0].bypassed = true;
    let bypassed = Project::from_snapshot(s).unwrap();
    let g = Graph::from_project(&bypassed, &d, &[]).unwrap();
    assert_eq!(
        prepare(&g, limits(&g), &Context::default())
            .unwrap()
            .main_latency,
        0
    );
    // Only metadata calculation changed; no registry/compiler admission was called.
}
#[test]
fn unknown_cycles_missing_endpoints_duplicate_ids_and_serial_overflow_refuse() {
    let p = fixture();
    let (g, mut d) = declared_graph(&p);
    let mut bad = g.clone();
    let main = bad.nodes.len() - 1;
    bad.edges.push(Edge {
        key: EdgeKey::Internal(NodeKey::Main, bad.nodes[0].key),
        source: main,
        target: 0,
    });
    assert_eq!(
        prepare(&bad, limits(&bad), &Context::default()).unwrap_err(),
        "cycle refused"
    );
    bad = g.clone();
    bad.edges[0].target = bad.nodes.len();
    assert_eq!(
        prepare(&bad, limits(&bad), &Context::default()).unwrap_err(),
        "edge endpoint missing"
    );
    bad = g.clone();
    bad.nodes[1].key = bad.nodes[0].key;
    assert_eq!(
        prepare(&bad, limits(&bad), &Context::default()).unwrap_err(),
        "duplicate node"
    );
    d.insert(
        NodeKey::Device(p.tracks()[0].devices()[0].id()),
        Fixed::Known(u64::MAX),
    );
    d.insert(
        NodeKey::Device(p.tracks()[0].devices()[1].id()),
        Fixed::Known(1),
    );
    let g = Graph::from_project(&p, &d, &[]).unwrap();
    assert_eq!(
        prepare(&g, limits(&g), &Context::default()).unwrap_err(),
        "latency overflow"
    );
}
#[test]
fn rounded_ring_twins_metadata_and_active_retained_coexistence_are_charged_exactly() {
    let p = fixture();
    let (_, mut d) = declared_graph(&p);
    d.insert(
        NodeKey::Device(p.return_tracks()[0].devices()[0].id()),
        Fixed::Known(3),
    );
    let g = Graph::from_project(&p, &d, &[]).unwrap();
    let mut l = limits(&g);
    l.ring_quantum = 8;
    l.node_capacity += 4;
    l.edge_capacity += 7;
    let context = Context {
        active_bytes: 1024,
        retained_bytes: 2048,
        ..Context::default()
    };
    let plan = prepare(&g, l, &context).unwrap();
    println!("synthetic demand report: {:?}", plan.demand);
    let rings = plan.edges.iter().filter(|e| e.delay > 0).count();
    assert_eq!(
        plan.demand.stereo_frame_bytes,
        std::mem::size_of::<[f32; 2]>()
    );
    assert_eq!(plan.demand.ring_frames_per_bank, rings * 8);
    assert_eq!(
        plan.demand.ring_twin_bytes,
        rings * 8 * std::mem::size_of::<[f32; 2]>() * 2
    );
    assert_eq!(
        plan.demand.ring_metadata_bytes,
        rings
            * (std::mem::size_of::<[Vec<[f32; 2]>; 2]>()
                + std::mem::size_of::<[(usize, usize, u64); 2]>())
    );
    assert_eq!(
        plan.demand.candidate_bytes,
        plan.demand.layout_bytes + plan.demand.ring_twin_bytes + plan.demand.ring_metadata_bytes
    );
    assert_eq!(
        plan.demand.coexist_bytes,
        plan.demand.candidate_bytes + 3072
    );
    l.coexist_bytes = plan.demand.coexist_bytes;
    assert!(prepare(&g, l, &context).is_ok());
    l.coexist_bytes -= 1;
    assert_eq!(
        prepare(&g, l, &context).unwrap_err(),
        "coexistence capacity refused"
    );
    l.coexist_bytes = usize::MAX;
    l.delay = 2;
    assert_eq!(
        prepare(&g, l, &context).unwrap_err(),
        "delay capacity refused"
    );
    let overflowing = Context {
        active_bytes: usize::MAX,
        ..Context::default()
    };
    l.delay = u64::MAX;
    assert_eq!(prepare(&g, l, &overflowing).unwrap_err(), "byte overflow");
    l.ring_quantum = usize::MAX;
    assert_eq!(
        prepare(&g, l, &Context::default()).unwrap_err(),
        "ring capacity overflow"
    );
}
#[test]
fn continuous_history_and_old_tail_ceiling_are_preflighted_without_adoption() {
    let p = fixture();
    let (_, d) = declared_graph(&p);
    let g = Graph::from_project(&p, &d, &[(7, 40)]).unwrap();
    let mut c = Context {
        previous_main_floor: 32,
        requested_ceiling: Some(32),
        ..Context::default()
    };
    assert_eq!(
        prepare(&g, limits(&g), &c).unwrap_err(),
        "tail or path exceeds ceiling"
    );
    c.requested_ceiling = Some(48);
    c.continuous = true;
    assert_eq!(
        prepare(&g, limits(&g), &c).unwrap_err(),
        "history unavailable"
    );
    let fresh = prepare(
        &g,
        limits(&g),
        &Context {
            requested_ceiling: Some(48),
            ..Context::default()
        },
    )
    .unwrap();
    c.histories = fresh.edges.iter().map(|e| (e.key, e.delay)).collect();
    let plan = prepare(&g, limits(&g), &c).unwrap();
    assert_eq!(plan.main_latency, 48);
    assert_eq!(edge(&plan, EdgeKey::TailMain(7)).delay, 8);
    let g = Graph::from_project(&p, &d, &[]).unwrap();
    c.requested_ceiling = None;
    c.continuous = false;
    assert_eq!(prepare(&g, limits(&g), &c).unwrap().main_latency, 32);
}
#[test]
fn refused_preparation_preserves_canonical_history_redo_and_ids() {
    let p = fixture();
    let id = p.tracks()[0].id();
    let mut editor = Editor::new(p, 8).unwrap();
    editor
        .execute(Command::Gain {
            track: id,
            gain: 0.5,
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let result: Result<_, hex_project::Invalid> = editor.execute_prepared(
        Command::Gain {
            track: id,
            gain: 0.75,
        },
        |candidate| {
            let g = Graph::from_project(candidate, &BTreeMap::new(), &[])
                .map_err(hex_project::Invalid)?;
            prepare(&g, limits(&g), &Context::default()).map_err(hex_project::Invalid)
        },
    );
    assert_eq!(result.unwrap_err().0, "unknown latency");
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    let result: Result<_, hex_project::Invalid> = editor.execute_prepared(
        Command::Gain {
            track: id,
            gain: 0.75,
        },
        |candidate| {
            let (_, mut declared) = declared_graph(candidate);
            declared.insert(
                NodeKey::Device(candidate.return_tracks()[0].devices()[0].id()),
                Fixed::Known(16),
            );
            let graph =
                Graph::from_project(candidate, &declared, &[]).map_err(hex_project::Invalid)?;
            prepare(
                &graph,
                limits(&graph),
                &Context {
                    continuous: true,
                    ..Context::default()
                },
            )
            .map_err(hex_project::Invalid)
        },
    );
    assert_eq!(result.unwrap_err().0, "history unavailable");
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
}
#[test]
fn limits_are_caller_inputs_not_new_policy_and_malformed_layouts_refuse() {
    let (g, _) = declared_graph(&fixture());
    let mut l = limits(&g);
    l.nodes -= 1;
    assert!(prepare(&g, l, &Context::default()).is_err());
    l = limits(&g);
    l.edges -= 1;
    assert!(prepare(&g, l, &Context::default()).is_err());
    l = limits(&g);
    l.ring_quantum = 0;
    assert!(prepare(&g, l, &Context::default()).is_err());
    let mut bad = g.clone();
    bad.edges.push(bad.edges[0]);
    assert_eq!(
        prepare(&bad, limits(&bad), &Context::default()).unwrap_err(),
        "duplicate edge"
    );
}

#[test]
fn impossible_metadata_capacity_refuses_before_reservation() {
    let (graph, _) = declared_graph(&fixture());
    let mut bound = limits(&graph);
    bound.node_capacity = usize::MAX;
    assert_eq!(
        prepare(&graph, bound, &Context::default()).unwrap_err(),
        "byte overflow"
    );
    bound = limits(&graph);
    bound.edge_capacity = usize::MAX;
    assert_eq!(
        prepare(&graph, bound, &Context::default()).unwrap_err(),
        "byte overflow"
    );
}
