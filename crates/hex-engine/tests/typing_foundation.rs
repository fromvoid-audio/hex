//! Reference protocol model; the production bridge has separate interaction tests.
#[path = "../src/typing_foundation.rs"]
mod typing_foundation;
use typing_foundation::*;

const OWNER: Owner = Owner {
    generation: 1,
    track: 2,
    device: 3,
};
const FOCUS: Focus = Focus {
    active_window: true,
    text_editing: false,
    modal: false,
    modified: false,
};
const BUDGET: Budget = Budget {
    voice_limit: 16,
    other_voices: 0,
    events_remaining: 256,
};

fn on(token: Token) -> Action {
    Action::On {
        owner: OWNER,
        token,
        key: 0,
        pitch: 60,
        velocity: 100,
    }
}
fn offer(keyboard: &mut Keyboard, key: &str) -> Action {
    keyboard
        .key_down(key, false, FOCUS, Transport::Playing, true)
        .unwrap()
        .unwrap()
}

#[test]
fn mapping_focus_autorepeat_and_owned_key_up_during_text_edit() {
    let mut keyboard = Keyboard::new(OWNER, 1).unwrap();
    assert_eq!(keyboard.owner(), OWNER);
    for (index, key) in KEYS.into_iter().enumerate() {
        let action = offer(&mut keyboard, key);
        assert!(
            matches!(action, Action::On { key, pitch, velocity: 100, .. } if usize::from(key) == index && usize::from(pitch) == 60 + index)
        );
        assert_eq!(keyboard.press_state(key), Some(PressState::Offered));
        keyboard.result(AppliedResult {
            action,
            kind: ResultKind::OnApplied,
        });
        assert_eq!(
            keyboard.press_state(key),
            Some(PressState::Offered),
            "unqueued result cannot fabricate application"
        );
        assert_eq!(
            keyboard.key_down(key, true, FOCUS, Transport::Playing, true),
            Ok(None)
        );
        assert_eq!(
            keyboard.key_down(key, false, FOCUS, Transport::Playing, true),
            Ok(None)
        );
        keyboard.queue_result(action, Some(32));
        assert_eq!(
            keyboard.press_state(key),
            Some(PressState::Queued(32)),
            "32 is a receipt, not proof of application"
        );
        keyboard.result(AppliedResult {
            action,
            kind: ResultKind::OnApplied,
        });
        assert_eq!(keyboard.press_state(key), Some(PressState::Applied));
        assert_eq!(
            keyboard.key_down(
                key,
                false,
                Focus {
                    text_editing: true,
                    ..FOCUS
                },
                Transport::Playing,
                true
            ),
            Ok(None)
        );
        assert_eq!(
            keyboard.key_up(key),
            Some(Action::Off {
                owner: OWNER,
                token: action.token()
            })
        );
        assert_eq!(keyboard.key_up(key), None);
    }
    assert_eq!(
        keyboard.key_down("x", false, FOCUS, Transport::Playing, true),
        Ok(None)
    );
    for focus in [
        Focus {
            text_editing: true,
            ..FOCUS
        },
        Focus {
            active_window: false,
            ..FOCUS
        },
        Focus {
            modal: true,
            ..FOCUS
        },
        Focus {
            modified: true,
            ..FOCUS
        },
    ] {
        assert_eq!(
            keyboard.key_down("a", false, focus, Transport::Playing, true),
            Ok(None)
        );
        assert_eq!(keyboard.key_up("a"), None);
    }
    for transport in [Transport::Paused, Transport::Stopped] {
        assert_eq!(
            keyboard.key_down("a", false, FOCUS, transport, true),
            Err(Refusal::NotPlaying)
        );
        keyboard.key_up("a");
    }
    assert_eq!(
        keyboard.key_down("a", false, FOCUS, Transport::Playing, false),
        Err(Refusal::NotAdmitted)
    );
}

#[test]
fn overlap_original_owner_and_delayed_ack_do_not_resurrect_retired_presses() {
    let mut keyboard = Keyboard::new(OWNER, 1).unwrap();
    let first = offer(&mut keyboard, "a");
    keyboard.queue_result(first, Some(1));
    let release = keyboard.key_up("a").unwrap();
    let second = offer(&mut keyboard, "a");
    assert_ne!(first.token(), second.token());
    keyboard.queue_result(second, Some(3));
    keyboard.result(AppliedResult {
        action: first,
        kind: ResultKind::OnApplied,
    });
    assert_eq!(keyboard.press_state("a"), Some(PressState::Queued(3)));
    let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
    assert_eq!(
        ledger
            .apply(first, 1, Transport::Playing, true, BUDGET)
            .kind,
        ResultKind::OnApplied
    );
    assert_eq!(
        ledger
            .apply(release, 2, Transport::Paused, false, BUDGET)
            .kind,
        ResultKind::OffPending
    );
    assert_eq!(
        ledger
            .apply(second, 3, Transport::Playing, true, BUDGET)
            .kind,
        ResultKind::OnApplied
    );
    assert_eq!(ledger.occupied(), 2, "released reservations remain owned");
    assert_eq!(ledger.pending_releases(Transport::Paused).len, 0);
    let before_first_sample = ledger.pending_releases(Transport::Playing);
    assert_eq!(before_first_sample.len, 1);
    assert_eq!(before_first_sample.identities[0], Some(first.identity()));
    assert!(
        !ledger.retired(first.identity()),
        "returning a release batch is not application"
    );
    assert!(ledger.release_applied(first.identity()));
    assert!(ledger.retired(first.identity()));
    assert_eq!(ledger.occupied(), 1);
    let authored = Identity::Authored {
        track: 2,
        clip: 1,
        note: first.token().serial,
        pass: first.token().epoch,
    };
    assert_ne!(
        authored,
        first.identity(),
        "no shared u64 namespace/high-bit reservation"
    );
    assert!(!ledger.retired(authored));
    let changed_selection = Owner {
        track: 99,
        device: 98,
        ..OWNER
    };
    assert_ne!(changed_selection, OWNER);
    assert_eq!(
        keyboard.key_up("a"),
        Some(Action::Off {
            owner: OWNER,
            token: second.token()
        })
    );
}

#[test]
fn queue_refusal_release_recovery_and_external_cut_orderings() {
    for reason in [
        CutReason::Stop,
        CutReason::Seek,
        CutReason::Reset,
        CutReason::Disarm,
        CutReason::WindowInactive,
        CutReason::AudioDisconnected,
        CutReason::ReleaseQueueFull,
    ] {
        for cut_first in [false, true] {
            let mut keyboard = Keyboard::new(OWNER, 1).unwrap();
            let action = offer(&mut keyboard, "a");
            keyboard.queue_result(action, Some(1));
            let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
            let cut = keyboard.cut(reason);
            if cut_first {
                ledger.cancel(cut);
            }
            let result = ledger.apply(action, 1, Transport::Playing, true, BUDGET);
            if cut_first {
                assert_eq!(result.kind, ResultKind::Refused(Refusal::Stale));
            } else {
                assert_eq!(result.kind, ResultKind::OnApplied);
                ledger.cancel(cut);
            }
            assert_eq!(
                ledger
                    .apply(action, 2, Transport::Playing, true, BUDGET)
                    .kind,
                ResultKind::Refused(Refusal::Stale)
            );
            keyboard.result(result);
            assert_eq!(keyboard.press_state("a"), None);
            assert_eq!(keyboard.rearm(OWNER, 0), Err(Refusal::Stale));
            keyboard.rearm(OWNER, cut.through_epoch).unwrap();
            ledger.rearm(OWNER, keyboard.epoch()).unwrap();
            assert_eq!(
                keyboard.key_down("a", false, FOCUS, Transport::Playing, true),
                Ok(None),
                "held key cannot synthesize a fresh onset after reset"
            );
            keyboard.key_up("a");
            let next = offer(&mut keyboard, "a");
            assert_eq!(
                ledger.apply(next, 3, Transport::Playing, true, BUDGET).kind,
                ResultKind::OnApplied
            );
            ledger.cancel(cut); // Delayed old cut must leave newer epoch alone.
            assert_eq!(
                ledger
                    .apply(
                        Action::Off {
                            owner: OWNER,
                            token: next.token()
                        },
                        4,
                        Transport::Paused,
                        false,
                        BUDGET
                    )
                    .kind,
                ResultKind::OffPending
            );
        }
    }
    let mut keyboard = Keyboard::new(OWNER, 1).unwrap();
    let refused = offer(&mut keyboard, "a");
    keyboard.queue_result(refused, None);
    assert_eq!(keyboard.press_state("a"), None);
    assert_eq!(keyboard.key_up("a"), None);
    let a = offer(&mut keyboard, "a");
    let b = offer(&mut keyboard, "s");
    keyboard.queue_result(a, Some(1));
    keyboard.queue_result(b, Some(2));
    let off = keyboard.key_up("a").unwrap();
    let cut = keyboard.queue_result(off, None).unwrap();
    assert_eq!(cut.reason, CutReason::ReleaseQueueFull);
    assert_eq!(keyboard.press_state("s"), None);
    let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
    ledger.apply(a, 1, Transport::Playing, true, BUDGET);
    ledger.apply(b, 2, Transport::Playing, true, BUDGET);
    ledger.cancel(cut);
    assert_eq!(ledger.pending_releases(Transport::Playing).len, 2);
}

#[test]
fn admission_capacity_invalid_actions_and_checked_identity_exhaustion() {
    let mut keyboard = Keyboard::new(OWNER, u64::MAX).unwrap();
    keyboard.seed_serial_for_test(u64::MAX - 1);
    let last = offer(&mut keyboard, "a");
    assert_eq!(last.token().serial, u64::MAX);
    assert_eq!(
        keyboard.key_down("s", false, FOCUS, Transport::Playing, true),
        Err(Refusal::IdentityExhausted)
    );
    assert!(
        keyboard.key_up("a").is_some(),
        "counter exhaustion must not lose an existing release"
    );
    let cut = keyboard.cut(CutReason::Disarm);
    assert_eq!(cut.through_epoch, u64::MAX);
    assert_eq!(
        keyboard.rearm(OWNER, u64::MAX),
        Err(Refusal::IdentityExhausted)
    );
    assert!(Keyboard::new(Owner { device: 0, ..OWNER }, 1).is_err());
    assert!(Keyboard::new(OWNER, 0).is_err());
    assert!(Ledger::new(OWNER, 1, 1).is_err());
    for limit in [16, 64] {
        let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
        assert!(ledger.rearm(OWNER, 2).is_err());
        let budget = Budget {
            voice_limit: limit,
            ..BUDGET
        };
        for serial in 1..=u64::from(limit) {
            assert_eq!(
                ledger
                    .apply(
                        on(Token { epoch: 1, serial }),
                        serial,
                        Transport::Playing,
                        true,
                        budget
                    )
                    .kind,
                ResultKind::OnApplied
            );
        }
        let extra = on(Token {
            epoch: 1,
            serial: 100,
        });
        assert_eq!(
            ledger
                .apply(extra, 100, Transport::Playing, true, budget)
                .kind,
            ResultKind::Refused(Refusal::Capacity)
        );
        assert_eq!(ledger.occupied(), usize::from(limit));
    }
    for (transport, admitted, budget, reason) in [
        (Transport::Paused, true, BUDGET, Refusal::NotPlaying),
        (Transport::Playing, false, BUDGET, Refusal::NotAdmitted),
        (
            Transport::Playing,
            true,
            Budget {
                other_voices: 16,
                ..BUDGET
            },
            Refusal::Capacity,
        ),
        (
            Transport::Playing,
            true,
            Budget {
                events_remaining: 0,
                ..BUDGET
            },
            Refusal::Capacity,
        ),
        (
            Transport::Playing,
            true,
            Budget {
                events_remaining: 257,
                ..BUDGET
            },
            Refusal::Capacity,
        ),
    ] {
        let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
        assert_eq!(
            ledger
                .apply(
                    on(Token {
                        epoch: 1,
                        serial: 1
                    }),
                    1,
                    transport,
                    admitted,
                    budget
                )
                .kind,
            ResultKind::Refused(reason)
        );
        assert_eq!(ledger.occupied(), 0);
    }
    let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
    let invalid = Action::On {
        owner: OWNER,
        token: Token {
            epoch: 1,
            serial: 1,
        },
        key: 255,
        pitch: 60,
        velocity: 0,
    };
    assert_eq!(
        ledger
            .apply(invalid, 1, Transport::Playing, true, BUDGET)
            .kind,
        ResultKind::Refused(Refusal::Invalid)
    );
    let action = on(Token {
        epoch: 1,
        serial: 1,
    });
    assert_eq!(
        ledger
            .apply(action, 0, Transport::Playing, true, BUDGET)
            .kind,
        ResultKind::Refused(Refusal::Invalid)
    );
    assert_eq!(
        ledger
            .apply(
                Action::Off {
                    owner: OWNER,
                    token: action.token()
                },
                2,
                Transport::Paused,
                true,
                BUDGET
            )
            .kind,
        ResultKind::Refused(Refusal::Stale)
    );
    assert_eq!(
        ledger
            .apply(action, 1, Transport::Playing, true, BUDGET)
            .kind,
        ResultKind::Refused(Refusal::Stale),
        "reordered earlier onset cannot follow its release"
    );
    assert_eq!(
        ledger
            .apply(action, u64::MAX - 1, Transport::Playing, true, BUDGET)
            .kind,
        ResultKind::OnApplied
    );
    assert_eq!(
        ledger
            .apply(action, u64::MAX, Transport::Playing, true, BUDGET)
            .kind,
        ResultKind::Refused(Refusal::Invalid)
    );
}

pub(crate) fn exercise_without_allocations() {
    let mut keyboard = Keyboard::new(OWNER, 1).unwrap();
    let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
    for (index, key) in KEYS.into_iter().enumerate() {
        let action = offer(&mut keyboard, key);
        let ticket = 1 + index as u64 * 2;
        keyboard.queue_result(action, Some(ticket));
        keyboard.result(ledger.apply(action, ticket, Transport::Playing, true, BUDGET));
        let off = keyboard.key_up(key).unwrap();
        ledger.apply(off, ticket + 1, Transport::Paused, false, BUDGET);
    }
    assert_eq!(ledger.pending_releases(Transport::Paused).len, 0);
    let releases = ledger.pending_releases(Transport::Playing);
    assert_eq!(releases.len, 12);
    for identity in releases.identities.into_iter().flatten() {
        assert!(ledger.release_applied(identity));
        assert!(ledger.retired(identity));
    }
    assert_eq!(ledger.occupied(), 0);
    let refused = offer(&mut keyboard, "a");
    keyboard.queue_result(refused, None);
    assert_eq!(keyboard.key_up("a"), None);
    let action = offer(&mut keyboard, "a");
    keyboard.queue_result(action, Some(25));
    keyboard.result(ledger.apply(action, 25, Transport::Playing, true, BUDGET));
    let off = keyboard.key_up("a").unwrap();
    let cut = keyboard.queue_result(off, None).unwrap();
    ledger.cancel(cut);
    assert_eq!(ledger.pending_releases(Transport::Paused).len, 0);
    let release = ledger.pending_releases(Transport::Playing).identities[0].unwrap();
    assert!(ledger.release_applied(release));
    assert!(ledger.retired(release));
    keyboard.rearm(OWNER, cut.through_epoch).unwrap();
    ledger.rearm(OWNER, keyboard.epoch()).unwrap();
    let action = offer(&mut keyboard, "a");
    keyboard.queue_result(action, Some(26));
    keyboard.result(ledger.apply(
        action,
        26,
        Transport::Playing,
        true,
        Budget {
            other_voices: 16,
            ..BUDGET
        },
    ));
    assert_eq!(keyboard.key_up("a"), None);
    ledger.cancel(keyboard.cut(CutReason::Stop));
    assert_eq!(ledger.occupied(), 0);
}

#[test]
fn rearm_preserves_old_release_owner_and_refusals_cannot_change_new_presses() {
    let mut keyboard = Keyboard::new(OWNER, 1).unwrap();
    let old = offer(&mut keyboard, "a");
    keyboard.queue_result(old, Some(1));
    let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
    ledger.apply(old, 1, Transport::Playing, true, BUDGET);
    let cut = keyboard.cut(CutReason::Disarm);
    ledger.cancel(cut);
    let new_owner = Owner {
        device: 40,
        track: 50,
        ..OWNER
    };
    keyboard.rearm(new_owner, cut.through_epoch).unwrap();
    ledger.rearm(new_owner, keyboard.epoch()).unwrap();
    keyboard.key_up("a");
    let new = offer(&mut keyboard, "a");
    keyboard.queue_result(new, Some(2));
    keyboard.result(AppliedResult {
        action: old,
        kind: ResultKind::Refused(Refusal::Stale),
    });
    assert_eq!(keyboard.press_state("a"), Some(PressState::Queued(2)));
    assert_eq!(
        ledger.apply(new, 2, Transport::Playing, true, BUDGET).kind,
        ResultKind::OnApplied
    );
    let releases = ledger.pending_releases(Transport::Playing);
    assert_eq!(releases.identities[0], Some(old.identity()));
    assert_eq!(releases.len, 1);
    assert_eq!(
        ledger
            .apply(
                Action::Off {
                    owner: OWNER,
                    token: old.token()
                },
                3,
                Transport::Playing,
                false,
                BUDGET
            )
            .kind,
        ResultKind::OffPending
    );
    assert_eq!(ledger.occupied(), 2);
    assert!(ledger.release_applied(old.identity()));
    assert!(ledger.retired(old.identity()));
    assert_eq!(ledger.occupied(), 1);
    keyboard.result(AppliedResult {
        action: new,
        kind: ResultKind::Refused(Refusal::NotAdmitted),
    });
    assert_eq!(keyboard.press_state("a"), None);
    assert_eq!(
        keyboard.key_down("a", false, FOCUS, Transport::Playing, true),
        Ok(None)
    );
    assert_eq!(keyboard.key_up("a"), None);
    for velocity in [0, 99, 101, 127] {
        let mut ledger = Ledger::new(OWNER, 1, 0).unwrap();
        let invalid = Action::On {
            owner: OWNER,
            token: Token {
                epoch: 1,
                serial: 1,
            },
            key: 0,
            pitch: 60,
            velocity,
        };
        assert_eq!(
            ledger
                .apply(invalid, 1, Transport::Playing, true, BUDGET)
                .kind,
            ResultKind::Refused(Refusal::Invalid)
        );
        assert_eq!(ledger.occupied(), 0);
    }
    let mut keyboard = Keyboard::new(OWNER, 1).unwrap();
    let offered = offer(&mut keyboard, "a");
    keyboard.queue_result(offered, Some(u64::MAX));
    assert_eq!(
        keyboard.press_state("a"),
        None,
        "reserved ticket cannot fabricate admission"
    );
}

#[test]
fn bounded_copy_storage_and_release_cycle() {
    fn copy<T: Copy>() {}
    copy::<Keyboard>();
    copy::<Ledger>();
    copy::<ReleaseBatch>();
    assert!(!std::mem::needs_drop::<Keyboard>() && !std::mem::needs_drop::<Ledger>());
    assert!(std::mem::size_of::<Keyboard>() <= 1024);
    assert!(std::mem::size_of::<Ledger>() <= 4096);
    assert!(std::mem::size_of::<ReleaseBatch>() <= 4096);
    println!(
        "private typing layout: keyboard={} ledger={} release_batch={} action={} result={}",
        std::mem::size_of::<Keyboard>(),
        std::mem::size_of::<Ledger>(),
        std::mem::size_of::<ReleaseBatch>(),
        std::mem::size_of::<Action>(),
        std::mem::size_of::<AppliedResult>()
    );
    exercise_without_allocations();
}
