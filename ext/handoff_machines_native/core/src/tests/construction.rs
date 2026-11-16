//! Cases that once sat behind a `debug_assert!` arm and are now impossible
//! by construction: the siblings' types leave no arm to fall into.

use periscope_machines::{Issuer, SEQUENCE_MAX, scope_of, sequence_of};

use super::*;
use crate::transfer::Transfer;

/// Saved values of every kind `restored` can meet, around `scope`.
fn saved_values(scope: u16) -> Vec<Option<u64>> {
    let mut values = Vec::from([None, Some(0), Some(u64::MAX)]);
    for other in [0, 1, scope, scope.wrapping_add(1), u16::MAX] {
        for sequence in [0, 1, 2, SEQUENCE_MAX - 1, SEQUENCE_MAX] {
            values.push(Some(scoped(other, sequence)));
        }
    }
    values
}

#[test]
fn restored_refuses_only_a_value_that_is_no_generation_of_the_scope() {
    // No sequence an id can carry is out of the issuer's reach, so the two
    // documented refusals are the only `None`: the old "past the last" arm
    // has nothing left to catch, even at the very end of a scope.
    for scope in [0, 1, 9, u16::MAX - 1, u16::MAX] {
        for last in saved_values(scope) {
            let generation =
                last.is_none_or(|last| scope_of(last) == scope && sequence_of(last) != 0);
            let restored = Small::restored(LIMITS, scope, last);
            assert_eq!(restored.is_some(), generation, "{scope} {last:?}");
            assert_eq!(
                restored.as_ref().map(|h| (h.scope(), h.generation())),
                Issuer::restored(scope, last).map(|i| (i.scope(), i.last())),
                "{scope} {last:?}"
            );
            // Whatever it restored, restoring its answer again is itself.
            if let Some(h) = restored {
                assert_eq!(Small::restored(LIMITS, scope, h.generation()), Some(h));
            }
        }
    }
}

#[test]
fn a_transfer_is_built_whatever_its_generation_and_base() {
    // `Transfer::new` returns a transfer, not an option: a const proves it
    // cannot refuse, so `start` has no arm for a drain tracker refusing.
    const BUILT: Transfer<1> = Transfer::new(
        Ticket {
            partition: 1,
            generation: u64::MAX,
        },
        1,
        2,
        u64::MAX,
        FencingToken::from_raw(1).unwrap(),
        Duration::ZERO,
    );
    assert_eq!(BUILT.status().base, u64::MAX);

    // Every start the admission checks pass is honoured and drains under any
    // generation of any scope, the first and the last, at any base.
    for (scope, last) in [(0, None), (u16::MAX, Some(SEQUENCE_MAX - 1))] {
        for base in [0, 10, u64::MAX - 2] {
            let last = last.map(|sequence| scoped(scope, sequence));
            let mut h = Small::restored(LIMITS, scope, last).unwrap();
            let t = h.start(9, 1, 2, base, token(5), at(0)).unwrap();
            assert_eq!(h.generation(), Some(t.generation));
            assert_eq!(h.status(9).unwrap().base, base);
            assert_eq!(h.quiesced(t, 1, base + 2, at(1)), recorded(Phase::Drain));
            assert_eq!(h.drained(t, 1, base + 1, at(2)), recorded(Phase::Drain));
            assert_eq!(h.committed(t, 1, base, at(2)), repeat(Phase::Drain));
            assert_eq!(h.drained(t, 1, base, at(3)), recorded(Phase::Checkpoint));
        }
    }
}
