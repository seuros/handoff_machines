#![allow(clippy::unwrap_used)] // Failed test assumptions should panic.
use std::vec::Vec;

use super::*;

mod draws;
mod edges;
mod model;
mod random;
mod replays;
mod rules;
mod scope;
mod tokens;

pub const LIMITS: Limits = Limits {
    phase_budget: Duration::from_secs(5),
    per_replica: 2,
};

pub fn at(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// The token whose raw value is `raw`: in scope zero, its sequence number.
pub fn token(raw: u64) -> FencingToken {
    FencingToken::from_raw(raw).unwrap()
}

/// Token `sequence` of `scope`.
pub fn token_in(scope: u16, sequence: u64) -> FencingToken {
    token(scoped(scope, sequence))
}

pub type Small = Handoff<2, 1>;
type Answer = Result<Acked, Refusal>;
type Ack<'a> = dyn Fn(&mut Small) -> Answer + 'a;

/// The acknowledgement was recorded and the transfer stands at `phase`.
#[allow(clippy::unnecessary_wraps)] // Compared against whole answers.
pub const fn recorded(phase: Phase) -> Answer {
    Ok(Acked::Recorded(phase))
}

/// An idempotent repeat: honoured, and nothing changed.
#[allow(clippy::unnecessary_wraps)] // Compared against whole answers.
pub const fn repeat(phase: Phase) -> Answer {
    Ok(Acked::Duplicate(phase))
}

fn orders<const N: usize, const W: usize>(handoff: &Handoff<N, W>) -> Vec<(Recipient, Action)> {
    handoff.orders(0).map(|o| (o.to, o.action)).collect()
}

const SOURCE: Recipient = Recipient::Replica(1);
const TARGET: Recipient = Recipient::Replica(2);

/// Partition 9 from replica 1 to replica 2, base 10, source token 5.
pub fn begun() -> (Small, Ticket) {
    begun_in(0)
}

/// The same transfer on a coordinator of `scope`.
pub fn begun_in(scope: u16) -> (Small, Ticket) {
    let mut handoff = Small::with_scope(LIMITS, scope);
    let ticket = handoff.start(9, 1, 2, 10, token(5), at(0)).unwrap();
    (handoff, ticket)
}

/// Advance a fresh transfer through `steps` acknowledgements of the happy path.
pub fn advanced(steps: usize) -> (Small, Ticket) {
    advanced_in(0, steps)
}

/// The same walk on a coordinator of `scope`.
pub fn advanced_in(scope: u16, steps: usize) -> (Small, Ticket) {
    let (mut h, t) = begun_in(scope);
    let acks: [&Ack; 6] = [
        &|h| h.quiesced(t, 1, 12, at(1)),
        &|h| {
            h.drained(t, 1, 11, at(1))
                .and_then(|_| h.drained(t, 1, 10, at(1)))
        },
        &|h| h.captured(t, 1, 77, at(1)),
        &|h| h.installed(t, 2, 77, at(1)),
        &|h| h.fenced(t, token(6), at(1)),
        &|h| h.activated(t, 2, token(6), at(1)),
    ];
    for ack in acks.iter().take(steps) {
        ack(&mut h).unwrap();
    }
    (h, t)
}

#[test]
fn happy_path_walks_every_phase_and_restates_orders() {
    let (mut h, t) = begun();
    assert_eq!((t.partition, t.generation), (9, 1));
    assert_eq!(orders(&h), [(SOURCE, Action::Quiesce)]);
    assert_eq!(orders(&h), [(SOURCE, Action::Quiesce)]);
    assert_eq!(h.quiesced(t, 1, 12, at(1)), recorded(Phase::Drain));
    assert_eq!(orders(&h), [(SOURCE, Action::Drain { until: 12 })]);
    assert_eq!(h.drained(t, 1, 11, at(1)), recorded(Phase::Drain));
    assert_eq!(h.outstanding(t, 0).unwrap().collect::<Vec<_>>(), [10]);
    assert_eq!(h.drained(t, 1, 10, at(2)), recorded(Phase::Checkpoint));
    assert_eq!(orders(&h), [(SOURCE, Action::Capture { at: 12 })]);
    assert_eq!(h.captured(t, 1, 77, at(2)), recorded(Phase::Checkpoint));
    assert_eq!(orders(&h), [(TARGET, Action::Install { checkpoint: 77 })]);
    assert_eq!(h.installed(t, 2, 77, at(3)), recorded(Phase::Activate));
    let fence = Action::Fence { above: token(5) };
    assert_eq!(orders(&h), [(Recipient::Authority, fence)]);
    assert_eq!(h.fenced(t, token(6), at(3)), recorded(Phase::Activate));
    assert_eq!(orders(&h), [(TARGET, Action::Activate { token: token(6) })]);
    let status = h.status(9).unwrap();
    assert_eq!((status.ticket, status.source, status.target), (t, 1, 2));
    assert_eq!((status.phase, status.base), (Phase::Activate, 10));
    assert_eq!((status.until, status.checkpoint), (Some(12), Some(77)));
    assert_eq!((status.token, status.deadline), (Some(token(6)), at(8)));
    assert_eq!(
        h.activated(t, 2, token(6), at(4)),
        recorded(Phase::Complete)
    );
    assert_eq!((h.active(), h.status(9), orders(&h).len()), (0, None, 0));
    assert_eq!(h.activated(t, 2, token(6), at(4)), Err(Refusal::Stale));
}

#[test]
fn empty_drain_skips_to_checkpoint() {
    let (mut h, t) = begun();
    assert_eq!(h.quiesced(t, 1, 9, at(0)), Err(Refusal::BehindBase));
    assert_eq!(h.quiesced(t, 1, 10, at(0)), recorded(Phase::Checkpoint));
    assert_eq!(h.status(9).unwrap().until, Some(10));
    assert_eq!(h.outstanding(t, 0).err(), Some(Refusal::OutOfPhase));
}

#[test]
fn abort_rolls_back_from_each_phase() {
    // (acks taken, phase, discard owed)
    let cases = [
        (0, Phase::Quiesce, false),
        (1, Phase::Drain, false),
        (2, Phase::Checkpoint, false),
        (3, Phase::Checkpoint, true),
        (4, Phase::Activate, true),
    ];
    for (steps, phase, discard) in cases {
        let (mut h, t) = advanced(steps);
        assert_eq!(h.status(9).unwrap().phase, phase);
        assert_eq!(h.abort(t, None, at(2)), recorded(Phase::Rollback));
        let mut owed = Vec::from([(SOURCE, Action::Resume { token: None })]);
        if discard {
            owed.push((TARGET, Action::Discard));
        }
        assert_eq!(orders(&h), owed);
        assert_eq!(h.resumed(t, 2, at(2)), Err(Refusal::WrongReplica));
        assert_eq!(h.discarded(t, 1, at(2)), Err(Refusal::WrongReplica));
        if discard {
            assert_eq!(h.resumed(t, 1, at(2)), recorded(Phase::Rollback));
            let before = h.clone();
            let again = h.resumed(t, 1, at(9));
            assert_eq!(again, repeat(Phase::Rollback));
            assert_eq!(h, before, "a repeat changes nothing, not even the clock");
            assert_eq!(orders(&h), [(TARGET, Action::Discard)]);
            assert_eq!(h.discarded(t, 2, at(3)), recorded(Phase::Aborted));
        } else {
            let before = h.clone();
            let unowed = h.discarded(t, 2, at(9));
            assert_eq!(unowed, repeat(Phase::Rollback));
            assert_eq!(h, before, "a discard never owed changes nothing");
            assert_eq!(h.resumed(t, 1, at(3)), recorded(Phase::Aborted));
        }
        assert_eq!((h.active(), h.status(9)), (0, None));
        assert_eq!(h.resumed(t, 1, at(3)), Err(Refusal::Stale));
    }
}

#[test]
fn abort_after_activation_was_ordered_needs_a_newer_token() {
    let (mut h, t) = advanced(5);
    let before = h.clone();
    assert_eq!(h.abort(t, None, at(2)), Err(Refusal::FenceRequired));
    assert_eq!(h.abort(t, Some(token(6)), at(2)), Err(Refusal::StaleToken));
    assert_eq!(h, before);
    assert_eq!(h.abort(t, Some(token(7)), at(2)), recorded(Phase::Rollback));
    let resume = Action::Resume {
        token: Some(token(7)),
    };
    assert_eq!(orders(&h), [(SOURCE, resume), (TARGET, Action::Discard)]);
    assert_eq!(h.activated(t, 2, token(6), at(2)), Err(Refusal::OutOfPhase));
    assert_eq!(h.discarded(t, 2, at(2)), recorded(Phase::Rollback));
    assert_eq!(orders(&h), [(SOURCE, resume)]);
    assert_eq!(h.resumed(t, 1, at(2)), recorded(Phase::Aborted));
    assert_eq!(h.active(), 0);
}

#[test]
fn fenced_abort_is_allowed_early_and_checks_the_source_token() {
    let (mut h, t) = advanced(1);
    assert_eq!(h.abort(t, Some(token(5)), at(2)), Err(Refusal::StaleToken));
    assert_eq!(h.abort(t, Some(token(9)), at(2)), recorded(Phase::Rollback));
    let resume = Action::Resume {
        token: Some(token(9)),
    };
    assert_eq!(orders(&h), [(SOURCE, resume)]);
}

#[test]
fn stale_generation_acknowledgements_are_refused() {
    let (mut h, old) = begun();
    h.abort(old, None, at(0)).unwrap();
    h.resumed(old, 1, at(0)).unwrap();
    let new = h.start(9, 1, 2, 10, token(5), at(1)).unwrap();
    assert_eq!(new.generation, 2);
    let before = h.clone();
    let forged = Ticket {
        partition: 9,
        generation: 3,
    };
    let zero = Ticket {
        partition: 9,
        generation: 0,
    };
    for (ticket, refusal) in [
        (old, Refusal::Stale),
        (forged, Refusal::Unknown),
        (zero, Refusal::Unknown),
    ] {
        assert_eq!(h.quiesced(ticket, 1, 12, at(1)), Err(refusal));
        assert_eq!(h.drained(ticket, 1, 10, at(1)), Err(refusal));
        assert_eq!(h.committed(ticket, 1, 11, at(1)), Err(refusal));
        assert_eq!(h.captured(ticket, 1, 7, at(1)), Err(refusal));
        assert_eq!(h.installed(ticket, 2, 7, at(1)), Err(refusal));
        assert_eq!(h.fenced(ticket, token(6), at(1)), Err(refusal));
        assert_eq!(h.activated(ticket, 2, token(6), at(1)), Err(refusal));
        assert_eq!(h.abort(ticket, None, at(1)), Err(refusal));
        assert_eq!(h.resumed(ticket, 1, at(1)), Err(refusal));
        assert_eq!(h.discarded(ticket, 2, at(1)), Err(refusal));
        assert_eq!(h.outstanding(ticket, 0).err(), Some(refusal));
    }
    assert_eq!(h, before);
    assert_eq!(h.quiesced(new, 1, 12, at(1)), recorded(Phase::Drain));
}

#[test]
fn acknowledgements_must_match_phase_replica_and_evidence() {
    let (mut h, t) = begun();
    let before = h.clone();
    assert_eq!(h.quiesced(t, 2, 12, at(0)), Err(Refusal::WrongReplica));
    assert_eq!(h.drained(t, 1, 10, at(0)), Err(Refusal::OutOfPhase));
    assert_eq!(h.committed(t, 1, 11, at(0)), Err(Refusal::OutOfPhase));
    assert_eq!(h.captured(t, 1, 7, at(0)), Err(Refusal::OutOfPhase));
    assert_eq!(h.installed(t, 2, 7, at(0)), Err(Refusal::OutOfPhase));
    assert_eq!(h.fenced(t, token(6), at(0)), Err(Refusal::OutOfPhase));
    assert_eq!(h.activated(t, 2, token(6), at(0)), Err(Refusal::OutOfPhase));
    assert_eq!(h.resumed(t, 1, at(0)), Err(Refusal::OutOfPhase));
    assert_eq!(h.discarded(t, 2, at(0)), Err(Refusal::OutOfPhase));
    assert_eq!(h, before);

    let (mut h, t) = advanced(1);
    let before = h.clone();
    assert_eq!(h.drained(t, 2, 10, at(1)), Err(Refusal::WrongReplica));
    assert_eq!(h.drained(t, 1, 12, at(1)), Err(Refusal::BeyondDrain));
    let below = h.drained(t, 1, 9, at(7));
    assert_eq!(below, repeat(Phase::Drain), "below the base");
    assert_eq!(h.committed(t, 2, 11, at(1)), Err(Refusal::WrongReplica));
    assert_eq!(h.committed(t, 1, 11, at(1)), Err(Refusal::Unacked));
    let settled = h.committed(t, 1, 10, at(7));
    assert_eq!(settled, repeat(Phase::Drain));
    assert_eq!(h, before, "refusals and repeats change nothing");
    assert_eq!(h.drained(t, 1, 10, at(1)), recorded(Phase::Drain));
    let before = h.clone();
    let again = h.drained(t, 1, 10, at(7));
    assert_eq!(again, repeat(Phase::Drain));
    assert!(again.is_ok_and(Acked::is_duplicate));
    assert_eq!(again.map(Acked::phase), Ok(Phase::Drain));
    assert_eq!(h, before, "a repeat changes nothing, not even the clock");

    let (mut h, t) = advanced(2);
    assert_eq!(h.captured(t, 2, 77, at(1)), Err(Refusal::WrongReplica));
    let (mut h, t) = advanced(3);
    let before = h.clone();
    assert_eq!(h.installed(t, 1, 77, at(1)), Err(Refusal::WrongReplica));
    assert_eq!(h.installed(t, 2, 78, at(1)), Err(Refusal::Mismatch));
    assert_eq!(h, before);
    let (mut h, t) = advanced(4);
    let before = h.clone();
    assert_eq!(h.fenced(t, token(5), at(1)), Err(Refusal::StaleToken));
    assert_eq!(h.fenced(t, token(4), at(1)), Err(Refusal::StaleToken));
    assert_eq!(h, before);
    let (mut h, t) = advanced(5);
    let before = h.clone();
    assert_eq!(
        h.activated(t, 1, token(6), at(1)),
        Err(Refusal::WrongReplica)
    );
    assert_eq!(h.activated(t, 2, token(7), at(1)), Err(Refusal::Mismatch));
    assert_eq!(h.fenced(t, token(8), at(1)), Err(Refusal::Mismatch));
    assert_eq!(h, before);
}

#[test]
fn concurrent_transfers_are_bounded_by_slots_and_per_replica() {
    let mut h = Handoff::<2, 1>::new(LIMITS);
    let a = h.start(1, 1, 2, 0, token(1), at(0)).unwrap();
    assert_eq!(h.start(1, 3, 4, 0, token(1), at(0)), Err(Refusal::Busy));
    assert_eq!(
        h.start(2, 5, 5, 0, token(1), at(0)),
        Err(Refusal::SameReplica)
    );
    let b = h.start(2, 2, 1, 0, token(1), at(0)).unwrap();
    assert_eq!((a.generation, b.generation, h.active()), (1, 2, 2));
    let before = h.clone();
    assert_eq!(h.start(3, 7, 8, 0, token(1), at(0)), Err(Refusal::Full));
    assert_eq!(h, before);
    h.abort(a, None, at(0)).unwrap();
    assert_eq!(h.start(3, 7, 8, 0, token(1), at(0)), Err(Refusal::Full));
    h.resumed(a, 1, at(0)).unwrap();
    let c = h.start(3, 7, 8, 0, token(1), at(0)).unwrap();
    assert_eq!((c.generation, h.generation()), (3, 3));

    let mut wide = Handoff::<4, 1>::new(LIMITS);
    wide.start(1, 1, 2, 0, token(1), at(0)).unwrap();
    wide.start(2, 3, 1, 0, token(1), at(0)).unwrap();
    let before = wide.clone();
    assert_eq!(
        wide.start(3, 1, 4, 0, token(1), at(0)),
        Err(Refusal::ReplicaBusy)
    );
    assert_eq!(
        wide.start(3, 4, 1, 0, token(1), at(0)),
        Err(Refusal::ReplicaBusy)
    );
    assert_eq!(wide, before);
    assert!(wide.start(3, 2, 3, 0, token(1), at(0)).is_ok());

    let mut none = Handoff::<0, 1>::new(LIMITS);
    assert_eq!(none.start(1, 1, 2, 0, token(1), at(0)), Err(Refusal::Full));
    assert_eq!(
        (Handoff::<0, 1>::CAPACITY, Handoff::<3, 2>::WINDOW),
        (0, 128)
    );
}

#[test]
fn orders_resume_from_any_position_with_a_bounded_buffer() {
    let mut h = Handoff::<3, 1>::new(Limits {
        per_replica: 3,
        ..LIMITS
    });
    let a = h.start(1, 1, 2, 0, token(1), at(0)).unwrap();
    let b = h.start(2, 1, 2, 0, token(1), at(0)).unwrap();
    let c = h.start(3, 1, 2, 0, token(1), at(0)).unwrap();
    h.quiesced(b, 1, 0, at(0)).unwrap();
    h.captured(b, 1, 4, at(0)).unwrap();
    h.abort(b, None, at(0)).unwrap();
    let all: Vec<Order> = h.orders(0).collect();
    assert_eq!(all.len(), 4);
    assert_eq!(
        all.iter().map(|o| o.ticket).collect::<Vec<_>>(),
        [a, b, b, c]
    );
    for size in 1..=4 {
        let (mut seen, mut position) = (Vec::new(), 0);
        loop {
            let mut pass = h.orders(position);
            let chunk: Vec<Order> = pass.by_ref().take(size).collect();
            position = pass.position();
            if chunk.is_empty() {
                break;
            }
            assert!(chunk.len() <= size);
            seen.extend(chunk);
        }
        assert_eq!(seen, all);
    }
    assert_eq!(h.orders(6).next(), None);
    assert_eq!(h.orders(usize::MAX).next(), None);
    assert_eq!(h.orders(3).next().map(|o| o.action), Some(Action::Discard));
}
