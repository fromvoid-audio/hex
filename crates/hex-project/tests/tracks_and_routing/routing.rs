use hex_project::{
    Command, Created, Editor, Invalid, MAX_AUTHORED_ROUTES, Project, ROUTING_UNSUPPORTED,
    RoutingEdge, RoutingOwner, RoutingSignal,
};

fn fixture() -> (Editor, RoutingOwner, RoutingOwner, RoutingOwner) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::ReturnTrack(a) = editor
        .execute(Command::AddReturnTrack { name: "a".into() })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::ReturnTrack(b) = editor
        .execute(Command::AddReturnTrack { name: "b".into() })
        .unwrap()
    else {
        panic!("return")
    };
    (
        editor,
        RoutingOwner::Track(track),
        RoutingOwner::Return(a),
        RoutingOwner::Return(b),
    )
}
fn edge(source: RoutingOwner, target: RoutingOwner) -> RoutingEdge {
    RoutingEdge {
        source,
        target,
        signal: RoutingSignal::Audio { channels: 2 },
    }
}

#[test]
fn valid_proposal_is_not_admitted_and_refusal_preserves_ids_and_redo() {
    let (mut editor, track, a, _) = fixture();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let proposal = vec![edge(track, a)];
    assert_eq!(before.validate_routing_proposal(&proposal), Ok(()));
    assert_eq!(
        editor.execute(Command::SetRouting(proposal.clone())),
        Err(Invalid(ROUTING_UNSUPPORTED))
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    let snapshot = before.snapshot();
    assert_eq!(
        Project::from_snapshot_with_routing(snapshot, proposal),
        Err(Invalid(ROUTING_UNSUPPORTED))
    );
    editor.execute(Command::SetRouting(Vec::new())).unwrap();
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().master_gain(), 0.5);
}

#[test]
fn cycles_duplicates_capabilities_and_capacity_refuse_before_adoption() {
    let (mut editor, track, a, b) = fixture();
    let before = editor.project().clone();
    for (proposal, reason) in [
        (
            vec![edge(a, b), edge(b, a)],
            "routing cycles are unsupported",
        ),
        (vec![edge(a, a)], "routing cycles are unsupported"),
        (
            vec![edge(track, a), edge(track, a)],
            "duplicate routing edge",
        ),
        (
            vec![edge(track, RoutingOwner::Main)],
            "duplicate routing edge",
        ),
        (vec![edge(a, RoutingOwner::Main)], "duplicate routing edge"),
        (
            vec![edge(a, track)],
            "routing port direction is unsupported",
        ),
        (
            vec![edge(RoutingOwner::Main, a)],
            "routing port direction is unsupported",
        ),
        (
            vec![RoutingEdge {
                signal: RoutingSignal::Midi,
                ..edge(track, a)
            }],
            "routing requires supported stereo audio ports",
        ),
        (
            vec![RoutingEdge {
                signal: RoutingSignal::Audio { channels: 1 },
                ..edge(track, a)
            }],
            "routing requires supported stereo audio ports",
        ),
        (
            vec![edge(track, a); MAX_AUTHORED_ROUTES + 1],
            "authored routing edge limit reached",
        ),
    ] {
        assert_eq!(
            editor.execute(Command::SetRouting(proposal)),
            Err(Invalid(reason))
        );
        assert_eq!(editor.project(), &before);
    }
    // A diamond and long paths are structurally valid, without playback admission.
    assert_eq!(
        before.validate_routing_proposal(&[edge(track, a), edge(track, b), edge(a, b)]),
        Ok(())
    );
}

#[test]
fn canonical_sends_are_single_owned_and_removed_owners_become_stale() {
    let (mut editor, track, a, _) = fixture();
    let RoutingOwner::Track(track_id) = track else {
        panic!("track")
    };
    let RoutingOwner::Return(return_id) = a else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track: track_id,
            return_track: return_id,
            gain: 0.25,
        })
        .unwrap();
    let with_send = editor.project().clone();
    assert_eq!(
        with_send.validate_routing_proposal(&[edge(track, a)]),
        Err(Invalid("duplicate routing edge"))
    );
    assert_eq!(
        Project::from_snapshot(with_send.snapshot()).unwrap(),
        with_send
    );
    editor
        .execute(Command::RemoveReturnTrack(return_id))
        .unwrap();
    assert!(editor.project().tracks()[0].sends().is_empty());
    assert_eq!(
        editor
            .project()
            .validate_routing_proposal(&[edge(track, a)]),
        Err(Invalid("routing owner no longer exists"))
    );
    assert!(editor.undo());
    assert_eq!(editor.project(), &with_send);
    assert!(editor.redo());
    assert!(editor.project().tracks()[0].sends().is_empty());
    assert!(editor.project().authored_routing().is_empty());
}

#[test]
fn exact_structural_edge_budget_is_checked_without_truncation() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let mut tracks = Vec::new();
    let mut returns = Vec::new();
    for _ in 0..hex_project::MAX_TRACKS {
        let Created::Track(id) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        tracks.push(RoutingOwner::Track(id));
    }
    for _ in 0..hex_project::MAX_RETURN_TRACKS {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(RoutingOwner::Return(id));
    }
    let mut proposal = Vec::new();
    for track in tracks {
        for &bus in &returns {
            proposal.push(edge(track, bus));
        }
    }
    assert_eq!(proposal.len(), MAX_AUTHORED_ROUTES);
    assert_eq!(
        editor.project().validate_routing_proposal(&proposal),
        Ok(())
    );
    proposal.push(edge(returns[0], returns[1]));
    assert_eq!(
        editor.project().validate_routing_proposal(&proposal),
        Err(Invalid("authored routing edge limit reached"))
    );
}
