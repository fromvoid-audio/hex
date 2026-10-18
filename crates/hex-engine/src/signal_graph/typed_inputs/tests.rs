use super::*;

#[test]
fn reserved_typed_keys_preserve_identity_parallel_order_and_latency_plan() {
    let builder = Builder {
        nodes: vec![
            (NodeId::Main, Some(0)),
            (NodeId::Fader(7), Some(3)),
            (NodeId::Audio(7), Some(0)),
            (NodeId::TrackInput(7), Some(0)),
        ],
        edges: vec![(1, 0), (2, 3), (3, 1), (2, 3)],
    };
    let prepared = prepare(&builder).unwrap();
    assert_eq!(
        prepared.typed,
        [
            NodeId::Audio(7),
            NodeId::TrackInput(7),
            NodeId::Fader(7),
            NodeId::Main
        ]
    );
    assert_eq!(
        prepared.nodes.iter().map(|n| n.key).collect::<Vec<_>>(),
        [4, 3, 1, 2]
    );
    assert_eq!(prepared.edge_keys, [4, 1, 3, 2]);
    assert_eq!(
        prepared
            .edges
            .iter()
            .map(|e| (e.key, e.from, e.to))
            .collect::<Vec<_>>(),
        [(1, 1, 2), (2, 1, 2), (3, 2, 3), (4, 3, 4)]
    );
    let plan = LatencyPlan::build(
        &prepared.nodes,
        &prepared.edges,
        CompensationLimits::default(),
    )
    .unwrap();
    assert_eq!(plan.max_latency_frames(), 3);
    let reversed = Builder {
        nodes: builder.nodes.iter().copied().rev().collect(),
        edges: builder
            .edges
            .iter()
            .rev()
            .map(|&(from, to)| (3 - from, 3 - to))
            .collect(),
    };
    let reordered = prepare(&reversed).unwrap();
    assert_eq!(reordered.typed, prepared.typed);
    assert_eq!(
        LatencyPlan::build(
            &reordered.nodes,
            &reordered.edges,
            CompensationLimits::default()
        )
        .unwrap(),
        plan
    );
    let duplicate = Builder {
        nodes: vec![(NodeId::Main, Some(0)); 2],
        edges: Vec::new(),
    };
    assert!(matches!(
        prepare(&duplicate),
        Err(Invalid("signal graph has duplicate typed nodes"))
    ));
    let invalid = Builder {
        nodes: vec![(NodeId::Main, Some(0))],
        edges: vec![(0, 1)],
    };
    assert!(matches!(
        prepare(&invalid),
        Err(Invalid("signal graph edge destination is missing"))
    ));
}

#[test]
fn reserved_typed_keys_match_tree_reference_at_graph_limits_and_empty() {
    let empty = prepare(&Builder::default()).unwrap();
    assert_eq!(
        (
            empty.typed.capacity(),
            empty.nodes.capacity(),
            empty.edges.capacity(),
            empty.edge_keys.capacity()
        ),
        (0, 0, 0, 0)
    );
    let count = MAX_COMPENSATION_NODES;
    let mut builder = Builder {
        nodes: (0..count)
            .rev()
            .map(|i| (NodeId::Device(i as u64 + 1), Some(0)))
            .collect(),
        edges: Vec::new(),
    };
    for i in 0..MAX_COMPENSATION_EDGES {
        builder.edges.push((i % count, (i + 1) % count));
    }
    let prepared = prepare(&builder).unwrap();
    // Independent historical tree-based identity oracle, test-only.
    let keys: std::collections::BTreeMap<_, _> = builder
        .nodes
        .iter()
        .map(|(id, _)| *id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, i as u64 + 1))
        .collect();
    assert!(prepared.typed.iter().eq(keys.keys()));
    let mut order: Vec<_> = (0..builder.edges.len()).collect();
    order.sort_by_key(|&i| {
        let (from, to) = builder.edges[i];
        (keys[&builder.nodes[from].0], keys[&builder.nodes[to].0])
    });
    for (ordinal, &old) in order.iter().enumerate() {
        assert_eq!(prepared.edge_keys[old], ordinal as u64 + 1);
        let edge = &prepared.edges[ordinal];
        let (from, to) = builder.edges[old];
        assert_eq!(
            (edge.key, edge.from, edge.to),
            (
                ordinal as u64 + 1,
                keys[&builder.nodes[from].0],
                keys[&builder.nodes[to].0]
            )
        );
    }
    assert_eq!(
        (prepared.nodes.len(), prepared.edges.len()),
        (MAX_COMPENSATION_NODES, MAX_COMPENSATION_EDGES)
    );
    assert!(prepared.nodes.capacity() >= prepared.nodes.len());
    assert!(prepared.edges.capacity() >= prepared.edges.len());
}

#[test]
fn reserved_graph_shape_matches_compiled_routes_and_refuses_insertion_growth() {
    let mut f = super::super::tests::fixture(&[&[3], &[7], &[]], 32);
    let mut returns = Vec::new();
    for _ in 0..2 {
        let hex_project::Created::ReturnTrack(id) = f
            .editor
            .execute(hex_project::Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(id);
    }
    f.editor
        .execute(hex_project::Command::SetReturnSend {
            track: f.tracks[0],
            return_track: returns[0],
            gain: 0.5,
        })
        .unwrap();
    f.editor
        .execute(hex_project::Command::SetReturnFeed {
            source: returns[0],
            destination: returns[1],
            gain: 0.75,
        })
        .unwrap();
    f.editor
        .execute(hex_project::Command::SetAudioFeed {
            source: RoutingOwner::Return(returns[1]),
            destination: f.tracks[2],
            gain: 0.5,
        })
        .unwrap();
    let revision = super::super::tests::prepare(&f).unwrap();
    let arrangement = revision.arrangement();
    let shape = counts(
        arrangement,
        f.editor.project().authored_routing().len(),
        f.editor.project().audio_feeds().len(),
    )
    .unwrap();
    assert_eq!(
        arrangement
            .tracks
            .iter()
            .filter(|track| track.instrument_slot_enabled())
            .count(),
        3
    );
    assert_eq!(shape, (22, 24)); // Includes all 3 present generators despite no MIDI clips.
    let graph = arrangement.signal_graph.as_ref().unwrap();
    assert_eq!((graph.typed.len(), graph.plan.edges().len()), shape);
    assert_eq!(graph.plan.max_latency_frames(), 7);
    assert!(matches!(
        counts(arrangement, MAX_COMPENSATION_EDGES, 0),
        Err(Invalid(
            "signal compensation graph exceeds its bounded node or edge limit"
        ))
    ));
    assert!(matches!(
        counts(arrangement, usize::MAX, 1),
        Err(Invalid("signal graph edge count overflow"))
    ));
    let mut builder = Builder {
        nodes: reserved(1).unwrap(),
        edges: reserved(1).unwrap(),
    };
    let pointers = (builder.nodes.as_ptr(), builder.edges.as_ptr());
    assert_eq!(builder.node(NodeId::Main, Some(0)).unwrap(), 0);
    assert_eq!(builder.edge(0, 0).unwrap(), 0);
    assert_eq!(
        builder.node(NodeId::Main, Some(0)),
        Err(Invalid("signal graph node reservation exhausted"))
    );
    assert_eq!(
        builder.edge(0, 0),
        Err(Invalid("signal graph edge reservation exhausted"))
    );
    assert_eq!((builder.nodes.len(), builder.edges.len()), (1, 1));
    assert_eq!((builder.nodes.as_ptr(), builder.edges.as_ptr()), pointers);
}
