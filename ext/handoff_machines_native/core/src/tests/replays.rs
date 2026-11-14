//! Replayed acknowledgements: honoured from the record, or refused precisely.
use super::{Small, advanced, at, begun, recorded, repeat, token};
use crate::{Acked, Phase, Refusal, Ticket};

type Answer = Result<Acked, Refusal>;
type Call<'a> = &'a dyn Fn(&mut Small) -> Answer;

/// Every call must leave the coordinator exactly as it was, clock included.
fn unchanged(h: &mut Small, calls: &[(Call, Answer)]) {
    for (index, (call, expected)) in calls.iter().enumerate() {
        let answer = matryona_machines::testing::unchanged(h, |h| call(h));
        assert_eq!(answer, *expected, "call {index}");
    }
}

/// The forward acknowledgements of the happy path, replayed late at `at(9)`.
/// `None` skips the one the transfer is waiting for.
fn forward(h: &mut Small, t: Ticket, expected: [Option<Answer>; 6]) {
    let calls: [Call; 6] = [
        &|h| h.quiesced(t, 1, 12, at(9)),
        &|h| h.drained(t, 1, 10, at(9)),
        &|h| h.committed(t, 1, 10, at(9)),
        &|h| h.captured(t, 1, 77, at(9)),
        &|h| h.installed(t, 2, 77, at(9)),
        &|h| h.fenced(t, token(6), at(9)),
    ];
    for (call, expected) in calls.into_iter().zip(expected) {
        if let Some(expected) = expected {
            unchanged(h, &[(call, expected)]);
        }
    }
}

#[test]
fn recorded_acknowledgements_repeat_in_every_later_phase() {
    let early = Err(Refusal::OutOfPhase);
    for steps in 0..=5 {
        let (mut h, t) = advanced(steps);
        let phase = h.status(9).unwrap().phase;
        // Recorded: a repeat. Awaited right now: skipped. Otherwise too early.
        let known = |needed: usize| match steps + 1 {
            next if next > needed => Some(repeat(phase)),
            next if next == needed => None,
            _ => Some(early),
        };
        // Offset 10 is the base: committed from the moment the drain opens.
        let committed = if steps == 0 { early } else { repeat(phase) };
        let expected = [
            known(1),
            known(2),
            Some(committed),
            known(3),
            known(4),
            known(5),
        ];
        forward(&mut h, t, expected);
    }
    let (mut h, t) = begun();
    unchanged(
        &mut h,
        &[
            (&|h| h.drained(t, 1, 9, at(9)), Err(Refusal::OutOfPhase)),
            (&|h| h.committed(t, 1, 10, at(9)), Err(Refusal::OutOfPhase)),
        ],
    );
}

#[test]
fn a_rollback_still_answers_for_what_the_transfer_recorded() {
    let early = Err(Refusal::OutOfPhase);
    let again = repeat(Phase::Rollback);
    for steps in 0..=5 {
        let (mut h, t) = advanced(steps);
        let fence = (steps == 5).then(|| token(7));
        assert_eq!(h.abort(t, fence, at(2)), recorded(Phase::Rollback));
        let known = |needed: usize| Some(if steps >= needed { again } else { early });
        // Offset 10 is at the base: committed there from the quiesce on.
        let expected = [known(1), known(2), known(1), known(3), known(4), known(5)];
        forward(&mut h, t, expected);
        let other = Some(token(8));
        let (same, differs) = (again, Err(Refusal::Mismatch));
        unchanged(
            &mut h,
            &[
                (&|h| h.abort(t, fence, at(9)), same),
                (&|h| h.abort(t, other, at(9)), differs),
                (&|h| h.abort(t, fence.xor(other), at(9)), differs),
                (&|h| h.activated(t, 2, token(6), at(9)), early),
            ],
        );
    }
}

#[test]
fn a_drain_given_up_repeats_only_the_offsets_it_recorded() {
    let (mut h, t) = advanced(1);
    assert_eq!(h.drained(t, 1, 11, at(1)), recorded(Phase::Drain));
    assert_eq!(h.abort(t, None, at(2)), recorded(Phase::Rollback));
    let again = repeat(Phase::Rollback);
    unchanged(
        &mut h,
        &[
            (&|h| h.drained(t, 1, 11, at(9)), again),
            (&|h| h.drained(t, 1, 9, at(9)), again),
            (&|h| h.drained(t, 1, 10, at(9)), Err(Refusal::OutOfPhase)),
            (&|h| h.drained(t, 1, 12, at(9)), Err(Refusal::BeyondDrain)),
            (&|h| h.drained(t, 2, 11, at(9)), Err(Refusal::WrongReplica)),
            (&|h| h.committed(t, 1, 10, at(9)), again),
            (&|h| h.committed(t, 1, 11, at(9)), Err(Refusal::OutOfPhase)),
            (
                &|h| h.committed(t, 2, 10, at(9)),
                Err(Refusal::WrongReplica),
            ),
        ],
    );
}

#[test]
fn a_replay_that_differs_from_the_record_is_refused_precisely() {
    let (mut h, t) = advanced(5);
    let mismatch = Err(Refusal::Mismatch);
    let wrong = Err(Refusal::WrongReplica);
    unchanged(
        &mut h,
        &[
            (&|h| h.quiesced(t, 1, 13, at(9)), mismatch),
            (&|h| h.quiesced(t, 1, 9, at(9)), mismatch),
            (&|h| h.quiesced(t, 2, 12, at(9)), wrong),
            (&|h| h.captured(t, 1, 78, at(9)), mismatch),
            (&|h| h.captured(t, 2, 77, at(9)), wrong),
            (&|h| h.installed(t, 2, 78, at(9)), mismatch),
            (&|h| h.installed(t, 1, 77, at(9)), wrong),
            (&|h| h.fenced(t, token(7), at(9)), mismatch),
            (&|h| h.fenced(t, token(5), at(9)), mismatch),
            (&|h| h.drained(t, 1, 12, at(9)), Err(Refusal::BeyondDrain)),
            (&|h| h.drained(t, 2, 10, at(9)), wrong),
            (&|h| h.committed(t, 1, 11, at(9)), Err(Refusal::OutOfPhase)),
            (&|h| h.committed(t, 2, 10, at(9)), wrong),
            (&|h| h.resumed(t, 1, at(9)), Err(Refusal::OutOfPhase)),
            (&|h| h.discarded(t, 2, at(9)), Err(Refusal::OutOfPhase)),
        ],
    );
}

#[test]
fn a_repeat_is_still_checked_against_the_clock_and_the_ticket() {
    let (mut h, t) = advanced(5);
    assert_eq!(
        h.activated(t, 2, token(6), at(4)),
        recorded(Phase::Complete)
    );
    let (mut live, u) = advanced(5);
    unchanged(
        &mut live,
        &[
            (
                &|h| h.quiesced(u, 1, 12, at(0)),
                Err(Refusal::ClockRegressed),
            ),
            (
                &|h| h.fenced(u, token(6), at(0)),
                Err(Refusal::ClockRegressed),
            ),
        ],
    );
    // The slot is free: nothing left to prove a repeat with.
    unchanged(
        &mut h,
        &[
            (&|h| h.quiesced(t, 1, 12, at(9)), Err(Refusal::Stale)),
            (&|h| h.captured(t, 1, 77, at(9)), Err(Refusal::Stale)),
            (&|h| h.installed(t, 2, 77, at(9)), Err(Refusal::Stale)),
            (&|h| h.fenced(t, token(6), at(9)), Err(Refusal::Stale)),
            (&|h| h.activated(t, 2, token(6), at(9)), Err(Refusal::Stale)),
            (&|h| h.abort(t, None, at(9)), Err(Refusal::Stale)),
        ],
    );
}
