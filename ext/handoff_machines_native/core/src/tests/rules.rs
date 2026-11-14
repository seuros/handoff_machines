//! Values valid by construction, and which honoured answers record the clock.
use matryona_machines::testing::refuses;

use super::{LIMITS, Small, advanced, at, begun, recorded, repeat, token};
use crate::{Acked, Action, Duration, Limits, Order, Phase, Recipient, Refusal, Status, Ticket};
use periscope_machines::scoped;

/// Run `ack` on two copies at two instants; the copies end equal exactly
/// when the answer recorded nothing that depends on the instant.
fn same_at_both(
    handoff: &Small,
    early: u64,
    late: u64,
    ack: impl Fn(&mut Small, Duration) -> Result<Acked, Refusal>,
) -> bool {
    let (mut a, mut b) = (handoff.clone(), handoff.clone());
    assert_eq!(ack(&mut a, at(early)), ack(&mut b, at(late)));
    a == b
}

#[test]
fn only_a_new_wait_records_the_clock() {
    // Drained 11 of 10..12 at the quiesce instant: the drain goes on.
    let (h, t) = advanced(1);
    assert!(same_at_both(&h, 1, 50, |h, now| h.drained(t, 1, 11, now)));
    let mut later = h;
    assert_eq!(later.drained(t, 1, 11, at(50)), recorded(Phase::Drain));
    // Nothing recorded 50: a start at 2 is not a regression.
    assert!(later.start(3, 3, 4, 0, token(1), at(2)).is_ok());

    // Completing holds whatever the time and frees the slot.
    let (h, t) = advanced(5);
    assert!(same_at_both(&h, 1, 90, |h, now| h.activated(
        t,
        2,
        token(6),
        now
    )));
    let mut done = h;
    assert_eq!(
        done.activated(t, 2, token(6), at(90)),
        recorded(Phase::Complete)
    );
    assert!(done.start(9, 1, 2, 0, token(1), at(1)).is_ok());

    // A step within the rollback, and the end of it.
    let (mut h, t) = advanced(4);
    assert_eq!(h.abort(t, Some(token(7)), at(1)), recorded(Phase::Rollback));
    assert!(same_at_both(&h, 1, 40, |h, now| h.resumed(t, 1, now)));
    assert_eq!(h.resumed(t, 1, at(40)), recorded(Phase::Rollback));
    assert!(same_at_both(&h, 1, 40, |h, now| h.discarded(t, 2, now)));
    assert!(same_at_both(&h, 1, 40, |h, now| h.resumed(t, 1, now)));
    assert_eq!(h.resumed(t, 1, at(2)), repeat(Phase::Rollback));

    // A move to a new wait counts its deadline from `now` and records it.
    let (h, t) = begun();
    assert!(!same_at_both(&h, 1, 2, |h, now| h.quiesced(t, 1, 12, now)));
    let mut moved = h;
    assert_eq!(moved.quiesced(t, 1, 12, at(30)), recorded(Phase::Drain));
    assert_eq!(moved.status(9).map(|s| s.deadline), Some(at(35)));
    refuses(&mut moved, Refusal::ClockRegressed, |s| {
        s.drained(t, 1, 11, at(29))
    });
}

#[test]
fn a_ticket_is_one_some_coordinator_issues() {
    assert_eq!(
        Ticket::new(4, 1).map(|t| (t.partition, t.generation)),
        Some((4, 1))
    );
    assert_eq!(Ticket::new(4, scoped(3, 9)).map(|t| t.scope()), Some(3));
    assert_eq!(Ticket::new(4, 0), None);
    assert_eq!(Ticket::new(4, scoped(3, 0)), None);
    let (_, ticket) = begun();
    assert_eq!(
        Ticket::new(ticket.partition, ticket.generation),
        Some(ticket)
    );
}

#[test]
fn limits_admit_at_least_one_transfer_per_replica() {
    assert_eq!(
        Limits::new(LIMITS.phase_budget, LIMITS.per_replica),
        Some(LIMITS)
    );
    assert_eq!((LIMITS.phase_budget(), LIMITS.per_replica()), (at(5), 2));
    assert_eq!(Small::new(LIMITS).limits(), LIMITS);
    assert!(Limits::new(Duration::ZERO, 1).is_some());
    assert_eq!(Limits::new(at(5), 0), None);
}

#[test]
fn only_the_authority_fences_and_it_does_nothing_else() {
    let (_, ticket) = begun();
    let replica = Recipient::Replica(1);
    let fence = Action::Fence { above: token(5) };
    let order = |to, action| Order::new(ticket, to, action);
    assert!(order(Recipient::Authority, fence).is_some());
    assert_eq!(order(replica, fence), None);
    for action in [
        Action::Quiesce,
        Action::Drain { until: 3 },
        Action::Capture { at: 3 },
        Action::Install { checkpoint: 3 },
        Action::Activate { token: token(6) },
        Action::Resume { token: None },
        Action::Discard,
    ] {
        assert_eq!(
            order(replica, action),
            Some(Order {
                ticket,
                to: replica,
                action
            })
        );
        assert_eq!(order(Recipient::Authority, action), None);
    }
    let unissued = Ticket {
        generation: 0,
        ..ticket
    };
    assert_eq!(Order::new(unissued, replica, Action::Quiesce), None);
}

#[test]
fn a_status_is_one_a_live_transfer_reports() {
    for steps in 0..5 {
        let status = advanced(steps).0.status(9).unwrap();
        let rebuilt = Status::new(
            status.ticket(),
            status.source(),
            status.target(),
            status.phase(),
            status.base(),
            status.until(),
            status.checkpoint(),
            status.token(),
            status.deadline(),
        );
        assert_eq!(rebuilt, Some(status));
        assert_eq!(status.checked(), Some(status));
        let broken = [
            Status {
                phase: Phase::Complete,
                ..status
            },
            Status {
                phase: Phase::Aborted,
                ..status
            },
            Status {
                target: status.source,
                ..status
            },
            Status {
                ticket: Ticket {
                    generation: 0,
                    ..status.ticket
                },
                ..status
            },
            Status {
                token: Some(token(9)),
                checkpoint: None,
                ..status
            },
        ];
        for wrong in broken {
            assert_eq!(wrong.checked(), None, "{wrong:?}");
        }
    }
    let drain = advanced(1).0.status(9).unwrap();
    assert_eq!(Status { base: 13, ..drain }.checked(), None);
    assert_eq!(
        Status {
            until: None,
            ..drain
        }
        .checked(),
        None
    );
    assert_eq!(
        Status {
            checkpoint: Some(1),
            ..drain
        }
        .checked(),
        None
    );
    let fresh = begun().0.status(9).unwrap();
    assert_eq!(
        Status {
            until: Some(12),
            ..fresh
        }
        .checked(),
        None
    );
    let (mut rolled, t) = advanced(4);
    assert_eq!(
        rolled.abort(t, Some(token(7)), at(1)),
        recorded(Phase::Rollback)
    );
    let rollback = rolled.status(9).unwrap();
    assert_eq!(rollback.checked(), Some(rollback));
    assert_eq!(
        Status {
            until: None,
            ..rollback
        }
        .checked(),
        None
    );
}

/// A drain that reached its quiesce point has moved on to the capture, on
/// either path (`quiesced` at the base, `drained` up to it): no live transfer
/// reports a drain with `base == until`.
#[test]
fn a_drain_status_is_below_its_quiesce_point() {
    let drain = advanced(1).0.status(9).unwrap();
    let reached = Status {
        base: drain.until.unwrap(),
        ..drain
    };
    assert_eq!(reached.checked(), None);
    let capture = Status {
        phase: Phase::Checkpoint,
        ..reached
    };
    assert_eq!(capture.checked(), Some(capture));
}

#[test]
fn acknowledgement_answers_are_ones_a_transfer_gives() {
    let phases = [
        Phase::Quiesce,
        Phase::Drain,
        Phase::Checkpoint,
        Phase::Activate,
        Phase::Rollback,
        Phase::Complete,
        Phase::Aborted,
    ];
    for phase in phases {
        let ends = matches!(phase, Phase::Complete | Phase::Aborted);
        assert_eq!(phase.is_final(), ends);
        let first = phase != Phase::Quiesce;
        assert_eq!(
            Acked::recorded(phase),
            first.then_some(Acked::Recorded(phase))
        );
        let repeated = first && !ends;
        assert_eq!(
            Acked::duplicate(phase),
            repeated.then_some(Acked::Duplicate(phase))
        );
    }
}
