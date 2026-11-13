//! Windows, clocks and integer limits.
use std::vec::Vec;

use super::{LIMITS, Small, at, begun, recorded, token};
use crate::{Duration, Handoff, Phase, Refusal};

#[test]
fn wide_drains_slide_the_window_and_list_what_is_missing() {
    let (mut h, t) = begun();
    assert_eq!(h.quiesced(t, 1, 10 + 100, at(0)), recorded(Phase::Drain));
    assert_eq!(h.drained(t, 1, 74, at(0)), Err(Refusal::OutOfWindow));
    assert_eq!(h.drained(t, 1, 73, at(0)), recorded(Phase::Drain));
    assert_eq!(h.drained(t, 1, 11, at(0)), recorded(Phase::Drain));
    let missing: Vec<u64> = h.outstanding(t, 0).unwrap().collect();
    assert_eq!(missing.len(), 98);
    assert_eq!((missing[0], missing[1], missing[97]), (10, 12, 109));
    assert!(!missing.contains(&73));
    let resumed: Vec<u64> = h.outstanding(t, 13).unwrap().take(2).collect();
    assert_eq!(resumed, [13, 14]);
    assert_eq!(
        h.outstanding(t, 108).unwrap().collect::<Vec<_>>(),
        [108, 109]
    );
    assert_eq!(h.outstanding(t, 110).unwrap().next(), None);
    for offset in (10..74).filter(|o| ![11, 73].contains(o)) {
        assert_eq!(h.drained(t, 1, offset, at(0)), recorded(Phase::Drain));
    }
    assert_eq!(h.committed(t, 1, 75, at(0)), Err(Refusal::Unacked));
    assert_eq!(h.committed(t, 1, 74, at(0)), recorded(Phase::Drain));
    assert_eq!(h.status(9).unwrap().base, 74);
    assert_eq!(h.outstanding(t, 0).unwrap().next(), Some(74));
    for offset in 74..109 {
        assert_eq!(h.drained(t, 1, offset, at(0)), recorded(Phase::Drain));
    }
    assert_eq!(h.drained(t, 1, 109, at(0)), recorded(Phase::Checkpoint));
}

#[test]
fn time_regression_is_refused_and_deadlines_are_advisory() {
    let (mut h, t) = begun();
    assert_eq!(h.quiesced(t, 1, 12, at(3)), recorded(Phase::Drain));
    let before = h.clone();
    assert_eq!(h.drained(t, 1, 10, at(2)), Err(Refusal::ClockRegressed));
    assert_eq!(h.abort(t, None, at(2)), Err(Refusal::ClockRegressed));
    let late = h.start(1, 3, 4, 0, token(1), at(2));
    assert_eq!(late, Err(Refusal::ClockRegressed));
    assert_eq!(h, before);
    assert_eq!(h.status(9).unwrap().deadline, at(8));
    assert_eq!(h.overdue(at(7), 0).next(), None);
    assert_eq!(h.overdue(at(8), 0).next(), Some(t));
    assert_eq!(h.drained(t, 1, 10, at(9)), recorded(Phase::Drain));
    assert_eq!(h.status(9).unwrap().deadline, at(8));
    assert_eq!(h.drained(t, 1, 11, at(9)), recorded(Phase::Checkpoint));
    assert_eq!(h.status(9).unwrap().deadline, at(14));
    let other = h.start(1, 3, 4, 0, token(1), at(9)).unwrap();
    let mut pass = h.overdue(at(20), 0);
    assert_eq!((pass.next(), pass.position()), (Some(t), 1));
    assert_eq!(h.overdue(at(20), 1).collect::<Vec<_>>(), [other]);
    assert_eq!(h.overdue(at(20), 2).next(), None);
}

#[test]
fn u64_limits_fail_closed() {
    let mut h = Small::restored(LIMITS, u64::MAX - 1);
    let top = token(u64::MAX);
    let t = h
        .start(u64::MAX, u64::MAX, 0, u64::MAX - 2, top, at(0))
        .unwrap();
    assert_eq!((t.generation, h.generation()), (u64::MAX, u64::MAX));
    let before = h.clone();
    assert_eq!(
        h.start(1, 1, 2, 0, token(1), at(0)),
        Err(Refusal::Exhausted)
    );
    assert_eq!(h, before);
    let now = Duration::MAX;
    assert_eq!(
        h.quiesced(t, u64::MAX, u64::MAX, now),
        recorded(Phase::Drain)
    );
    assert_eq!(h.status(u64::MAX).unwrap().deadline, Duration::MAX);
    assert_eq!(h.overdue(Duration::MAX, 0).next(), Some(t));
    assert_eq!(
        h.drained(t, u64::MAX, u64::MAX, now),
        Err(Refusal::BeyondDrain)
    );
    let missing: Vec<u64> = h.outstanding(t, 0).unwrap().collect();
    assert_eq!(missing, [u64::MAX - 2, u64::MAX - 1]);
    assert_eq!(
        h.drained(t, u64::MAX, u64::MAX - 1, now),
        recorded(Phase::Drain)
    );
    assert_eq!(
        h.drained(t, u64::MAX, u64::MAX - 2, now),
        recorded(Phase::Checkpoint)
    );
    assert_eq!(
        h.captured(t, u64::MAX, u64::MAX, now),
        recorded(Phase::Checkpoint)
    );
    assert_eq!(h.installed(t, 0, u64::MAX, now), recorded(Phase::Activate));
    // No token exceeds the source's: activation cannot be fenced, only rolled back.
    assert_eq!(h.fenced(t, top, now), Err(Refusal::StaleToken));
    assert_eq!(h.abort(t, Some(top), now), Err(Refusal::StaleToken));
    assert_eq!(h.abort(t, None, now), recorded(Phase::Rollback));
    assert_eq!(h.discarded(t, 0, now), recorded(Phase::Rollback));
    assert_eq!(h.resumed(t, u64::MAX, now), recorded(Phase::Aborted));

    let mut narrow = Handoff::<1, 0>::new(LIMITS);
    let t = narrow.start(1, 1, 2, 5, token(1), at(0)).unwrap();
    assert_eq!(narrow.quiesced(t, 1, 6, at(0)), recorded(Phase::Drain));
    assert_eq!(narrow.drained(t, 1, 5, at(0)), Err(Refusal::OutOfWindow));
    assert_eq!(narrow.outstanding(t, 0).unwrap().collect::<Vec<_>>(), [5]);
}

#[test]
fn refusal_is_an_error_with_a_message_per_variant() {
    use std::string::ToString;
    let all = [
        Refusal::Full,
        Refusal::Busy,
        Refusal::ReplicaBusy,
        Refusal::SameReplica,
        Refusal::Exhausted,
        Refusal::ClockRegressed,
        Refusal::Stale,
        Refusal::Unknown,
        Refusal::OutOfPhase,
        Refusal::WrongReplica,
        Refusal::BehindBase,
        Refusal::BeyondDrain,
        Refusal::OutOfWindow,
        Refusal::Unacked,
        Refusal::Mismatch,
        Refusal::StaleToken,
        Refusal::FenceRequired,
    ];
    let source: &dyn core::error::Error = &all[0];
    assert!(source.source().is_none());
    for (index, refusal) in all.iter().enumerate() {
        let message = refusal.to_string();
        assert_ne!(message, "");
        for other in &all[index + 1..] {
            assert_ne!(message, other.to_string());
        }
    }
}
