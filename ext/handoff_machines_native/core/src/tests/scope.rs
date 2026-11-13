//! Every ticket carries the scope of the coordinator that issued it.
use periscope_machines::{SEQUENCE_MAX, scoped};

use super::{LIMITS, Small, advanced_in, at, begun_in, recorded, repeat, token};
use crate::{Phase, Refusal, Ticket};

/// Every call that takes a ticket answers `refusal` and changes nothing.
fn refuses_everywhere(h: &mut Small, ticket: Ticket, refusal: Refusal) {
    let before = h.clone();
    let now = at(9);
    assert_eq!(h.quiesced(ticket, 1, 12, now), Err(refusal));
    assert_eq!(h.drained(ticket, 1, 10, now), Err(refusal));
    assert_eq!(h.committed(ticket, 1, 10, now), Err(refusal));
    assert_eq!(h.captured(ticket, 1, 77, now), Err(refusal));
    assert_eq!(h.installed(ticket, 2, 77, now), Err(refusal));
    assert_eq!(h.fenced(ticket, token(6), now), Err(refusal));
    assert_eq!(h.activated(ticket, 2, token(6), now), Err(refusal));
    assert_eq!(h.abort(ticket, None, now), Err(refusal));
    assert_eq!(h.abort(ticket, Some(token(9)), now), Err(refusal));
    assert_eq!(h.resumed(ticket, 1, now), Err(refusal));
    assert_eq!(h.discarded(ticket, 2, now), Err(refusal));
    assert_eq!(h.outstanding(ticket, 0).err(), Some(refusal));
    assert_eq!(*h, before);
}

#[test]
fn a_ticket_from_another_coordinator_is_unknown() {
    // The review's sequence: two coordinators, the same calls, the same
    // partition. Both wait for activation, so every earlier acknowledgement
    // would repeat and `activated` would complete the wrong transfer.
    let (mut near, near_ticket) = advanced_in(1, 5);
    let (mut far, far_ticket) = advanced_in(2, 5);
    assert_eq!(near_ticket.partition, far_ticket.partition);
    assert_ne!(near_ticket, far_ticket);
    assert_eq!((near_ticket.scope(), far_ticket.scope()), (1, 2));
    refuses_everywhere(&mut near, far_ticket, Refusal::Unknown);
    refuses_everywhere(&mut far, near_ticket, Refusal::Unknown);

    // The same in every other phase a repeat is answered from.
    for steps in 0..5 {
        let (mut own, _) = advanced_in(1, steps);
        let (_, other) = advanced_in(2, steps);
        refuses_everywhere(&mut own, other, Refusal::Unknown);
        let (mut undone, mine) = advanced_in(1, steps);
        assert_eq!(undone.abort(mine, None, at(1)), recorded(Phase::Rollback));
        refuses_everywhere(&mut undone, other, Refusal::Unknown);
    }

    // Each coordinator still honours its own, repeats included.
    assert_eq!(
        near.quiesced(near_ticket, 1, 12, at(9)),
        repeat(Phase::Activate)
    );
    assert_eq!(
        near.fenced(near_ticket, token(6), at(9)),
        repeat(Phase::Activate)
    );
    assert_eq!(
        far.activated(far_ticket, 2, token(6), at(9)),
        recorded(Phase::Complete)
    );
    // Finished here is stale here, and still unknown over there.
    refuses_everywhere(&mut far, far_ticket, Refusal::Stale);
    refuses_everywhere(&mut near, far_ticket, Refusal::Unknown);
}

#[test]
fn two_coordinators_left_in_one_scope_still_alias() {
    // Nothing tells them apart: keeping scopes distinct is the caller's job.
    let (mut one, mine) = advanced_in(5, 5);
    let (_, theirs) = advanced_in(5, 5);
    assert_eq!(mine, theirs);
    assert_eq!(one.quiesced(theirs, 1, 12, at(9)), repeat(Phase::Activate));
    assert_eq!(
        one.activated(theirs, 2, token(6), at(9)),
        recorded(Phase::Complete)
    );
}

#[test]
fn scope_zero_issues_the_plain_counter() {
    let mut h = Small::new(LIMITS);
    assert_eq!(h, Small::with_scope(LIMITS, 0));
    assert_eq!(Some(&h), Small::restored(LIMITS, 0, 0).as_ref());
    assert_eq!((h.scope(), h.generation()), (0, 0));
    let first = h.start(1, 1, 2, 0, token(1), at(0)).unwrap();
    let second = h.start(2, 3, 4, 0, token(1), at(0)).unwrap();
    assert_eq!((first.generation, second.generation), (1, 2));
    assert_eq!((first.scope(), h.generation()), (0, 2));
}

#[test]
fn a_restored_coordinator_keeps_its_scope_and_its_sequence() {
    let (mut h, old) = begun_in(9);
    assert_eq!((h.scope(), old.scope()), (9, 9));
    assert_eq!(
        (old.generation, h.generation()),
        (scoped(9, 1), scoped(9, 1))
    );
    assert_eq!(Small::with_scope(LIMITS, 9).generation(), scoped(9, 0));
    assert_eq!(
        Some(Small::with_scope(LIMITS, 9)),
        Small::restored(LIMITS, 9, scoped(9, 0))
    );
    assert_eq!(h.abort(old, None, at(0)), recorded(Phase::Rollback));

    let mut again = Small::restored(LIMITS, 9, h.generation()).unwrap();
    assert_eq!((again.scope(), again.generation()), (9, scoped(9, 1)));
    // Issued before the restart, not live now: stale, not unknown.
    refuses_everywhere(&mut again, old, Refusal::Stale);
    let next = again.start(9, 1, 2, 10, token(5), at(0)).unwrap();
    assert_eq!(next.generation, scoped(9, 2));
    let unissued = Ticket {
        partition: 9,
        generation: scoped(9, 3),
    };
    let plain = Ticket {
        partition: 9,
        generation: 2,
    };
    refuses_everywhere(&mut again, unissued, Refusal::Unknown);
    refuses_everywhere(&mut again, plain, Refusal::Unknown);
    assert_eq!(again.quiesced(next, 1, 12, at(9)), recorded(Phase::Drain));
}

#[test]
fn the_last_generation_is_issued_then_the_scope_is_spent() {
    let mut h = Small::restored(LIMITS, 0, SEQUENCE_MAX - 1).unwrap();
    assert_eq!(h.scope(), 0);
    let last = h.start(1, 1, 2, 0, token(1), at(3)).unwrap();
    assert_eq!((last.generation, last.scope()), (SEQUENCE_MAX, 0));
    let before = h.clone();
    // Not even the instant is recorded, and scope one is not borrowed from.
    assert_eq!(
        h.start(2, 3, 4, 0, token(1), at(7)),
        Err(Refusal::Exhausted)
    );
    assert_eq!(h, before);
    assert_eq!((h.scope(), h.generation()), (0, SEQUENCE_MAX));
    // Another refusal comes first and still spends nothing.
    assert_eq!(h.start(1, 3, 4, 0, token(1), at(7)), Err(Refusal::Busy));
    assert_eq!(h, before);
    // The ticket already issued still works.
    assert_eq!(h.quiesced(last, 1, 0, at(3)), recorded(Phase::Checkpoint));

    let mut spent = Small::restored(LIMITS, 0, SEQUENCE_MAX).unwrap();
    let before = spent.clone();
    assert_eq!(
        spent.start(1, 1, 2, 0, token(1), at(0)),
        Err(Refusal::Exhausted)
    );
    assert_eq!(spent, before);
}

#[test]
fn restoring_from_generation_continues_where_the_coordinator_stopped() {
    for scope in [0, 9, u16::MAX] {
        // Fresh: nothing issued, so the answer is the scope above sequence
        // zero. It is not a generation, and it restores an equal coordinator.
        let fresh = Small::with_scope(LIMITS, scope);
        assert_eq!(fresh.generation(), scoped(scope, 0));
        assert_eq!(fresh.generation() == 0, scope == 0);
        let mut again = Small::restored(LIMITS, scope, fresh.generation()).unwrap();
        assert_eq!(again, fresh);
        let never = Ticket {
            partition: 9,
            generation: fresh.generation(),
        };
        refuses_everywhere(&mut again, never, Refusal::Unknown);
        let first = again.start(9, 1, 2, 10, token(5), at(0)).unwrap();
        assert_eq!(first.generation, scoped(scope, 1));

        // Used: the restored coordinator issues what the old one would have.
        let (mut used, old) = begun_in(scope);
        assert_eq!(used.generation(), scoped(scope, 1));
        let mut again = Small::restored(LIMITS, scope, used.generation()).unwrap();
        assert_eq!(
            (again.scope(), again.generation(), again.active()),
            (scope, used.generation(), 0)
        );
        // Issued before the restart and not carried over: stale there.
        refuses_everywhere(&mut again, old, Refusal::Stale);
        let next = [&mut used, &mut again].map(|h| h.start(5, 5, 6, 0, token(1), at(0)));
        assert_eq!(next[0], next[1]);
        assert_eq!(next[0].map(|t| t.generation), Ok(scoped(scope, 2)));
        assert_eq!(again.generation(), used.generation());
    }
}

#[test]
fn the_last_generation_of_a_scope_restores_a_spent_coordinator() {
    let mut h = Small::restored(LIMITS, u16::MAX, scoped(u16::MAX, SEQUENCE_MAX - 1)).unwrap();
    let last = h.start(1, 1, 2, 0, token(1), at(3)).unwrap();
    assert_eq!((last.generation, h.generation()), (u64::MAX, u64::MAX));
    let mut spent = Small::restored(LIMITS, u16::MAX, h.generation()).unwrap();
    assert_eq!((spent.scope(), spent.generation()), (u16::MAX, u64::MAX));
    let before = spent.clone();
    assert_eq!(
        spent.start(2, 3, 4, 0, token(1), at(7)),
        Err(Refusal::Exhausted)
    );
    assert_eq!(spent, before);
    refuses_everywhere(&mut spent, last, Refusal::Stale);
}

#[test]
fn restoring_refuses_a_saved_value_of_another_scope() {
    // The review's three inputs. A store that answers 0 for a missing row:
    assert_eq!(Small::restored(LIMITS, 9, 0), None);
    assert_eq!(Small::restored(LIMITS, u16::MAX, 0), None);
    // A saved value cut to 32 bits, which would alias a scope-zero coordinator:
    let (h, _) = begun_in(9);
    let cut = h.generation() & 0xFFFF_FFFF;
    assert_eq!(begun_in(0).0.generation(), cut);
    assert_eq!(Small::restored(LIMITS, 9, cut), None);
    // And the scope is never the saved value's: each side refuses the other's.
    assert_eq!(Small::restored(LIMITS, 0, h.generation()), None);
    assert_eq!(Small::restored(LIMITS, 8, h.generation()), None);
    assert_eq!(Small::restored(LIMITS, 0, scoped(9, 0)), None);
    for scope in [0, 9, u16::MAX] {
        // "Nothing issued yet" is spelled one way: the scope above sequence zero.
        let fresh = Small::restored(LIMITS, scope, scoped(scope, 0));
        assert_eq!(fresh, Some(Small::with_scope(LIMITS, scope)));
        for other in [1, 10, u16::MAX - 1] {
            assert_eq!(Small::restored(LIMITS, scope, scoped(other, 1)), None);
        }
    }
}
