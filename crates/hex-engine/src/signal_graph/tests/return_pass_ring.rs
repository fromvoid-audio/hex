//! Candidate value snapshot for the sole dry edge of a routed Return graph.
use super::*;

#[test]
fn sole_routed_return_alignment_supplies_bounded_committed_dry_pcm() {
    const DELAY: usize = 512;
    let mut fixture = fixture(&[&[]], 1024);
    let track = fixture.tracks[0];
    let Created::ReturnTrack(bus) = fixture
        .editor
        .execute(Command::AddReturnTrack { name: "wet".into() })
        .unwrap()
    else {
        panic!("Return");
    };
    fixture
        .editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    fixture
        .editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: state(DELAY as u32).unwrap(),
        })
        .unwrap();
    let revision = prepare(&fixture).unwrap();
    let graph = revision.arrangement().signal_graph.as_ref().unwrap();
    let (dry, arrival) = graph.resident_main_alignment(0).unwrap();
    assert_eq!(arrival, 0);
    assert_eq!(graph.main_latency(), DELAY as u32);
    assert_eq!(
        graph.pre_main_endpoints(dry),
        Some((NodeId::Fader(track.value()), NodeId::Main))
    );
    assert_eq!(
        graph.return_latency(0).unwrap().default_route_delay_frames,
        0
    );
    let planned = graph.pre_main_edges[dry];
    assert_eq!(
        graph
            .plan
            .edges()
            .iter()
            .enumerate()
            .filter_map(|(index, edge)| (edge.delay_frames != 0).then_some(index))
            .collect::<Vec<_>>(),
        [planned]
    );

    let mut provider = SignalRuntime::new(Arc::clone(graph)).unwrap();
    let signal = |frame: usize| {
        let phase = (frame % 127) as f32 / 256.0;
        [phase, -phase]
    };
    provider.delays.begin_transaction().unwrap();
    let mut prefix = std::array::from_fn::<_, { DELAY - 1 }, _>(signal);
    provider.edge_block(dry, &mut prefix).unwrap();
    provider.delays.commit_transaction().unwrap();
    assert!(provider.delays.test_committed_dry_pass(planned).is_none());

    provider.delays.begin_transaction().unwrap();
    let mut wrap = std::array::from_fn::<_, 263, _>(|offset| signal(DELAY - 1 + offset));
    provider.edge_block(dry, &mut wrap).unwrap();
    assert!(provider.delays.test_committed_dry_pass(planned).is_none());
    provider.delays.commit_transaction().unwrap();

    let mut pass = provider.delays.test_committed_dry_pass(planned).unwrap();
    assert_eq!(pass.latency_frames(), DELAY as u32);
    // Reject a speculative write without publishing it into the value snapshot.
    provider.delays.begin_transaction().unwrap();
    let mut poison = [[-1.0, 1.0]; 17];
    provider.edge_block(dry, &mut poison).unwrap();
    assert!(provider.delays.test_committed_dry_pass(planned).is_none());
    provider.delays.rollback_transaction().unwrap();

    let cut = DELAY - 1 + 263;
    let mut old_edge = std::array::from_fn::<_, 769, _>(|offset| signal(cut + offset));
    let mut passive = old_edge;
    pass.process(&mut passive);
    provider.delays.begin_transaction().unwrap();
    provider.edge_block(dry, &mut old_edge).unwrap();
    provider.delays.commit_transaction().unwrap();
    for (offset, frame) in passive.iter().enumerate() {
        assert_eq!(*frame, signal(cut - DELAY + offset));
    }
    assert_eq!(passive, old_edge);
}
