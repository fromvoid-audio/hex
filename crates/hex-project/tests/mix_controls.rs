use hex_project::{Command, Created, Editor, Project};

#[test]
fn balance_and_master_adjustments_are_canonical_bounded_and_undoable() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "stereo".into(),
        })
        .expect("track")
    else {
        panic!("track result")
    };

    editor
        .execute(Command::BalanceBy { track, amount: 0.4 })
        .expect("relative balance");
    assert_eq!(editor.project().track(track).expect("track").balance(), 0.4);
    editor
        .execute(Command::BalanceBy {
            track,
            amount: f32::MAX,
        })
        .expect("finite adjustment clamps to endpoint");
    assert_eq!(editor.project().track(track).expect("track").balance(), 1.0);
    assert!(editor.undo());
    assert_eq!(editor.project().track(track).expect("track").balance(), 0.4);
    assert!(editor.undo());
    assert_eq!(editor.project().track(track).expect("track").balance(), 0.0);
    assert!(editor.redo());

    editor
        .execute(Command::MasterGainBy { amount: 0.5 })
        .expect("relative master gain");
    assert_eq!(editor.project().master_gain(), 1.5);
    editor
        .execute(Command::MasterGainBy { amount: f32::MAX })
        .expect("finite master adjustment clamps");
    assert_eq!(editor.project().master_gain(), 2.0);
    assert!(editor.undo());
    assert_eq!(editor.project().master_gain(), 1.5);

    let before = editor.project().clone();
    for command in [
        Command::Balance {
            track,
            balance: f32::NAN,
        },
        Command::Balance {
            track,
            balance: f32::INFINITY,
        },
        Command::Balance {
            track,
            balance: 1.01,
        },
        Command::BalanceBy {
            track,
            amount: f32::NEG_INFINITY,
        },
        Command::MasterGain { gain: f32::NAN },
        Command::MasterGain { gain: -0.01 },
        Command::MasterGain { gain: 2.01 },
        Command::MasterGainBy { amount: f32::NAN },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }
}

#[test]
fn snapshots_validate_and_roundtrip_mix_controls() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::AddTrack { name: "one".into() })
        .expect("track")
    else {
        panic!("track result")
    };
    editor
        .execute(Command::Balance {
            track,
            balance: -0.25,
        })
        .expect("balance");
    editor
        .execute(Command::MasterGain { gain: 1.75 })
        .expect("master gain");
    let snapshot = editor.project().snapshot();
    let restored = Project::from_snapshot(snapshot.clone()).expect("roundtrip");
    assert_eq!(restored.snapshot(), snapshot);
    assert_eq!(restored.tracks()[0].balance(), -0.25);
    assert_eq!(restored.master_gain(), 1.75);

    let mut invalid_balance = snapshot.clone();
    invalid_balance.tracks[0].balance = f32::INFINITY;
    assert!(Project::from_snapshot(invalid_balance).is_err());
    let mut invalid_master = snapshot.clone();
    invalid_master.master_gain = f32::NAN;
    assert!(Project::from_snapshot(invalid_master).is_err());
    let mut excess_master = snapshot;
    excess_master.master_gain = 2.01;
    assert!(Project::from_snapshot(excess_master).is_err());
}
