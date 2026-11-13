//! Fencing tokens of two scopes have no order: a transfer ranks tokens only
//! inside its source token's scope.
use super::*;

const HOME: u16 = 3;

/// Partition 9 from replica 1 to replica 2 under source token 5 of `HOME`,
/// waiting for the authority's token.
fn awaiting_fence() -> (Small, Ticket) {
    let mut h = Small::new(LIMITS);
    let held = token_in(HOME, 5);
    let t = h.start(9, 1, 2, 10, held, at(0)).unwrap();
    assert_eq!(h.quiesced(t, 1, 10, at(1)), recorded(Phase::Checkpoint));
    assert_eq!(h.captured(t, 1, 77, at(1)), recorded(Phase::Checkpoint));
    assert_eq!(h.installed(t, 2, 77, at(1)), recorded(Phase::Activate));
    let above = Action::Fence { above: held };
    assert_eq!(orders(&h), [(Recipient::Authority, above)]);
    (h, t)
}

fn refused(h: &mut Small, refusal: Refusal, call: impl FnOnce(&mut Small) -> Answer) {
    let before = h.clone();
    assert_eq!(call(h), Err(refusal));
    assert_eq!(*h, before, "{refusal:?} changed the coordinator");
}

/// Tokens of other scopes: raw values below, above and far above `HOME`'s.
fn foreign() -> [FencingToken; 4] {
    [
        token_in(0, 99),
        token_in(HOME - 1, 6),
        token_in(HOME + 1, 1),
        token_in(u16::MAX, 6),
    ]
}

#[test]
fn fenced_refuses_a_token_of_another_scope_whatever_its_number() {
    let (mut h, t) = awaiting_fence();
    for token in foreign() {
        refused(&mut h, Refusal::Unknown, |h| h.fenced(t, token, at(2)));
    }
    // In the transfer's scope the answer is about rank, and only there.
    for sequence in [1, 5] {
        refused(&mut h, Refusal::StaleToken, |h| {
            h.fenced(t, token_in(HOME, sequence), at(2))
        });
    }
    let granted = token_in(HOME, 6);
    assert_eq!(h.fenced(t, granted, at(2)), recorded(Phase::Activate));
    assert_eq!(h.status(9).unwrap().token, Some(granted));
}

#[test]
fn abort_refuses_a_token_of_another_scope_before_and_after_the_fence() {
    let (mut h, t) = awaiting_fence();
    for token in foreign() {
        refused(&mut h, Refusal::Unknown, |h| h.abort(t, Some(token), at(2)));
    }
    refused(&mut h, Refusal::StaleToken, |h| {
        h.abort(t, Some(token_in(HOME, 5)), at(2))
    });
    assert_eq!(
        h.fenced(t, token_in(HOME, 6), at(2)),
        recorded(Phase::Activate)
    );
    // The floor is now the activation token, still of the same scope.
    for token in foreign() {
        refused(&mut h, Refusal::Unknown, |h| h.abort(t, Some(token), at(3)));
    }
    refused(&mut h, Refusal::StaleToken, |h| {
        h.abort(t, Some(token_in(HOME, 6)), at(3))
    });
    refused(&mut h, Refusal::FenceRequired, |h| h.abort(t, None, at(3)));
    let rollback = token_in(HOME, 7);
    assert_eq!(h.abort(t, Some(rollback), at(3)), recorded(Phase::Rollback));
}

#[test]
fn a_transfer_stuck_behind_a_rescoped_authority_can_still_be_given_up() {
    let (mut h, t) = awaiting_fence();
    refused(&mut h, Refusal::Unknown, |h| {
        h.fenced(t, token_in(HOME + 1, 1), at(2))
    });
    // No token of the old scope will come: an unfenced abort is still open.
    assert_eq!(h.abort(t, None, at(2)), recorded(Phase::Rollback));
}

/// The defect: scope was looked at last. Once a token was recorded, a token
/// of another scope was compared with it and answered `Mismatch`, and a call
/// the transfer did not wait for answered `OutOfPhase` without reading the
/// token's scope at all. A token of another scope names nothing here, and
/// says so before the transfer's phase or its record is consulted.
#[test]
fn a_foreign_token_is_unknown_before_the_record_is_read() {
    let (mut h, t) = awaiting_fence();
    let granted = token_in(HOME, 6);
    assert_eq!(h.fenced(t, granted, at(2)), recorded(Phase::Activate));
    assert_eq!(h.fenced(t, granted, at(2)), repeat(Phase::Activate));
    // Same sequence number, other scope: not a contradicting report.
    let twin = token_in(HOME + 1, 6);
    refused(&mut h, Refusal::Unknown, |h| h.fenced(t, twin, at(2)));
    refused(&mut h, Refusal::Unknown, |h| h.activated(t, 2, twin, at(2)));
    // The transfer's own scope is still told what is wrong with the token.
    refused(&mut h, Refusal::Mismatch, |h| {
        h.fenced(t, token_in(HOME, 7), at(2))
    });
    refused(&mut h, Refusal::Mismatch, |h| {
        h.activated(t, 2, token_in(HOME, 7), at(2))
    });
    let rollback = token_in(HOME, 7);
    assert_eq!(h.abort(t, Some(rollback), at(3)), recorded(Phase::Rollback));
    for token in foreign() {
        refused(&mut h, Refusal::Unknown, |h| h.abort(t, Some(token), at(3)));
        refused(&mut h, Refusal::Unknown, |h| h.fenced(t, token, at(3)));
        refused(&mut h, Refusal::Unknown, |h| {
            h.activated(t, 2, token, at(3))
        });
    }
    refused(&mut h, Refusal::Mismatch, |h| {
        h.abort(t, Some(token_in(HOME, 8)), at(3))
    });
    refused(&mut h, Refusal::Mismatch, |h| h.abort(t, None, at(3)));
    assert_eq!(h.abort(t, Some(rollback), at(3)), repeat(Phase::Rollback));
    assert_eq!(h.fenced(t, granted, at(3)), repeat(Phase::Rollback));
}

#[test]
fn a_foreign_token_is_unknown_before_the_phase_is_read() {
    let mut h = Small::new(LIMITS);
    let held = token_in(HOME, 5);
    let t = h.start(9, 1, 2, 10, held, at(0)).unwrap();
    // Nobody waits for a token yet, and nobody will under this one.
    for token in foreign() {
        refused(&mut h, Refusal::Unknown, |h| h.fenced(t, token, at(1)));
        for replica in [1, 2, 3] {
            refused(&mut h, Refusal::Unknown, |h| {
                h.activated(t, replica, token, at(1))
            });
        }
    }
    let ours = token_in(HOME, 6);
    refused(&mut h, Refusal::OutOfPhase, |h| h.fenced(t, ours, at(1)));
    refused(&mut h, Refusal::OutOfPhase, |h| {
        h.activated(t, 2, ours, at(1))
    });
    // Given up before any token arrived: still scope first.
    assert_eq!(h.abort(t, None, at(1)), recorded(Phase::Rollback));
    for token in foreign() {
        refused(&mut h, Refusal::Unknown, |h| h.fenced(t, token, at(2)));
        refused(&mut h, Refusal::Unknown, |h| h.abort(t, Some(token), at(2)));
    }
    refused(&mut h, Refusal::OutOfPhase, |h| h.fenced(t, ours, at(2)));
    refused(&mut h, Refusal::Mismatch, |h| h.abort(t, Some(ours), at(2)));
}

/// The ticket and the clock are still read before the token.
#[test]
fn the_ticket_and_the_clock_come_before_the_token() {
    let (mut h, t) = awaiting_fence();
    let stray = token_in(HOME + 1, 9);
    let never = Ticket {
        generation: t.generation + 1,
        ..t
    };
    refused(&mut h, Refusal::ClockRegressed, |h| {
        h.fenced(t, stray, at(0))
    });
    refused(&mut h, Refusal::ClockRegressed, |h| {
        h.fenced(never, stray, at(0))
    });
    refused(&mut h, Refusal::Unknown, |h| h.fenced(never, stray, at(2)));
    assert_eq!(h.abort(t, None, at(2)), recorded(Phase::Rollback));
    assert_eq!(h.resumed(t, 1, at(2)), recorded(Phase::Rollback));
    assert_eq!(h.discarded(t, 2, at(2)), recorded(Phase::Aborted));
    // A finished ticket is stale, whatever token comes with it.
    refused(&mut h, Refusal::Stale, |h| h.fenced(t, stray, at(3)));
    refused(&mut h, Refusal::Stale, |h| h.activated(t, 2, stray, at(3)));
    refused(&mut h, Refusal::Stale, |h| h.abort(t, Some(stray), at(3)));
}

#[test]
fn an_early_abort_ranks_against_the_source_token_alone() {
    let mut h = Small::new(LIMITS);
    let t = h.start(9, 1, 2, 10, token_in(HOME, 5), at(0)).unwrap();
    refused(&mut h, Refusal::Unknown, |h| {
        h.abort(t, Some(token_in(0, 9)), at(1))
    });
    refused(&mut h, Refusal::Unknown, |h| {
        h.abort(t, Some(token_in(HOME + 1, 9)), at(1))
    });
    let rollback = token_in(HOME, 9);
    assert_eq!(h.abort(t, Some(rollback), at(1)), recorded(Phase::Rollback));
    let resume = Action::Resume {
        token: Some(rollback),
    };
    assert_eq!(orders(&h), [(SOURCE, resume)]);
}
