# handoff-machines

> **AMS DeCandia**, NCC-4234, *DeCandia*-class transfer tender.

Engine-room flight manual: move a partition between replicas one acknowledged phase at a time.

Initial, unstable. Allocation-free `no_std` coordinator policy for partition
ownership transfer: quiesce the source, drain what is in flight, checkpoint,
then activate the target under a newer fencing token. `Handoff<N, WORDS>` holds
at most `N` concurrent transfers, each with a drain window of `WORDS * 64`
offsets, and refuses rather than grows.

It coordinates ownership migration. It does not rank placement, replicate data,
issue fencing tokens, send messages or persist anything.

- `start` issues a `Ticket { partition, generation }`. Generations are strictly
  increasing and never reused. Every acknowledgement carries its ticket: an
  issued generation that is no longer the partition's transfer is `Stale`, one
  never issued is `Unknown`.
- **Scopes.** A ticket means something only to the coordinator that issued
  it. A generation is a `u64` in the
  [`periscope-machines`](https://crates.io/crates/periscope-machines) layout:
  the coordinator's scope in the top 16 bits, a sequence counted from 1 in
  the low 48. Two coordinators count the same sequence, so without scopes a
  ticket for the same partition given to the wrong one is acknowledged there.
  `Handoff::new(limits)` is scope zero and issues 1, 2, 3;
  `Handoff::with_scope(limits, scope)` issues the same sequence under `scope`;
  `Handoff::scope()` and `Ticket::scope()` read it back. A ticket of another
  scope was never issued here: every acknowledgement and `outstanding` answer
  `Unknown` before any transfer is looked up, and nothing changes. The
  coordinator reads no clock and no random source, so it cannot pick a scope;
  two coordinators left in the same scope still alias. A scope holds 2^48 - 1
  generations, then `start` answers `Exhausted`.
- **Restarts.** A fresh coordinator counts from 1 again, so a ticket that
  outlived a restart could name a new transfer. Either resume the sequence,
  or give each incarnation a scope no earlier one used.
  `Handoff::restored(limits, scope, last_generation)` resumes: it takes the
  scope and what `generation()` answered, and is `None` when that value is
  not of `scope`. The scope is never read out of the saved value, so one
  that lost its top bits, or the `0` a store answers for a missing row,
  cannot land a coordinator in scope zero. "Nothing issued yet" is spelled
  one way, `scope` above a zero sequence, which is what `generation()`
  answers then; with no saved value, call `with_scope`.
- An idempotent repeat (`Acked::Duplicate`) is answered only from a live
  transfer whose ticket matches, scope included. A ticket whose transfer
  ended is `Stale` even though the coordinator can tell it issued it: it does
  not keep how the transfer ended.
- Each phase advances only on an explicit acknowledgement from the replica that
  holds the role: `quiesced` (source, with the quiesce point), `drained` (source,
  one in-flight offset at a time), `captured` (source, checkpoint identity),
  `installed` (target, the same identity), `fenced` (the authority's token,
  strictly above the source's), `activated` (target, the same token).
- Fencing tokens are `lease-machines` `FencingToken`s, re-exported here, and
  they carry their authority's scope. Tokens of two scopes have no order, so a
  transfer reads tokens only inside its source token's scope. `fenced`,
  `activated` and `abort` answer `Unknown` to a token of any other scope,
  whatever its number, and they answer it first: after the clock and the
  ticket, before the transfer's phase, the replica or any recorded token is
  looked at. `Unknown` is the name every crate gives an id that is not of
  the namespace the call reads it in; a foreign token is never folded into
  "older" (`StaleToken`), "not the one recorded" (`Mismatch`) or "not now"
  (`OutOfPhase`). Those three are only ever said of a token of the
  transfer's scope: `StaleToken` when it does not exceed the ones the
  transfer knows, `Mismatch` when a different one is recorded. `start` has
  nothing to compare its source token with: that token names the scope.
  One transfer therefore lives under one authority scope: if
  the authority came back under a new scope before `fenced`, give the
  transfer up with an unfenced `abort` and start it again under a source
  token of the new scope.
- Drain readiness is an `offset-machines` `Tracker` per transfer, assigned at the
  transfer's generation: the phase ends when the contiguous acknowledged prefix
  reaches the quiesce point. `committed` slides the window for drains wider
  than it; `outstanding` lists what is still missing, resumably.
- `abort` works from every phase and moves to `Rollback`: the source owes
  `resumed`, and the target owes `discarded` once it was asked to install. After
  `fenced`, activation has been ordered and the target may be serving, so `abort`
  demands a token above the activation token and the source resumes under it.
- `orders(from)` restates what is owed, at most two orders per transfer. Orders
  are derived from state, not queued: nothing overflows, and a pass can stop
  anywhere and resume from `Orders::position()`.
- Every mutating call takes `now`. An earlier `now` than the last accepted one is
  `ClockRegressed`. Each phase gets `Limits::phase_budget`; `overdue` lists
  transfers past it. That is advice: the coordinator never aborts by itself.
- Every call that can refuse returns `Result<T, Refusal>`. `Refusal` is the one
  enum for "not honoured", and a refused call changes nothing. No public enum
  is `#[non_exhaustive]`: a new variant is a breaking change every caller
  matches on.
- A network may deliver any acknowledgement twice, so every acknowledgement
  method (`quiesced`, `drained`, `committed`, `captured`, `installed`, `fenced`,
  `activated`, `abort`, `resumed`, `discarded`) returns `Result<Acked, Refusal>`:
  `Acked::Recorded(phase)` the first time, `Acked::Duplicate(phase)` for an
  idempotent repeat. A repeat is honoured, not refused, and changes nothing,
  not even the observed clock.
- A repeat is answered from what the transfer recorded, for as long as the
  transfer lives, rollback included: `quiesced` with the recorded quiesce
  point, `drained` of an acknowledged or committed offset, `committed` at or
  below the window base, `captured` and `installed` with the recorded
  checkpoint, `fenced` with the recorded token, `abort` during the rollback
  with the same token (or the same absence of one), and a `resumed` or
  `discarded` already recorded or never owed.
- A replay whose payload differs from the record is not a repeat: a different
  quiesce point, checkpoint, activation token or rollback token is `Mismatch`.
  From the wrong replica it is `WrongReplica`.
- `OutOfPhase` is what is left: the transfer is not waiting for the
  acknowledgement and never recorded it, because it came too early or because
  the transfer was given up first. `activated` and the acknowledgement that
  ends a rollback free the slot, so their repeats are `Stale` like any other
  finished ticket's.
- Checks run in one order, and the first that fails is the answer. Every
  acknowledgement: `ClockRegressed`, then the ticket (`Unknown`, `Stale`).
  Then, for the three that take a fencing token:

  | Call | After the clock and the ticket |
  | --- | --- |
  | `fenced` | `Unknown` (token scope), then `Duplicate` or `Mismatch` against a recorded token, `OutOfPhase`, `StaleToken` |
  | `activated` | `Unknown` (token scope), `OutOfPhase`, `WrongReplica`, `Mismatch` |
  | `abort` | `Unknown` (token scope, when a token is given), then `Duplicate` or `Mismatch` during a rollback, `StaleToken`, `FenceRequired` |

```rust
use core::time::Duration;
use handoff_machines::{Acked, Action, FencingToken, Handoff, Limits, Phase, Recipient, Refusal};

# fn example() -> Option<()> {
let limits = Limits { phase_budget: Duration::from_secs(30), per_replica: 2 };
let mut handoff = Handoff::<8, 4>::new(limits); // 8 transfers, 256-offset drains
let now = Duration::ZERO;
let held = FencingToken::from_raw(7)?; // scope zero, sequence 7
// Partition 42 from replica 1 to replica 2; offsets below 100 are committed.
let ticket = handoff.start(42, 1, 2, 100, held, now).ok()?;
let order = handoff.orders(0).next()?;
assert_eq!((order.to, order.action), (Recipient::Replica(1), Action::Quiesce));

let acked = |phase| Ok(Acked::Recorded(phase));
assert_eq!(handoff.quiesced(ticket, 1, 102, now), acked(Phase::Drain));
assert_eq!(handoff.quiesced(ticket, 1, 102, now), Ok(Acked::Duplicate(Phase::Drain)));
assert_eq!(handoff.drained(ticket, 1, 101, now), acked(Phase::Drain));
assert_eq!(handoff.drained(ticket, 1, 101, now), Ok(Acked::Duplicate(Phase::Drain)));
assert_eq!(handoff.outstanding(ticket, 0).ok()?.next(), Some(100));
assert_eq!(handoff.drained(ticket, 1, 100, now), acked(Phase::Checkpoint));
assert_eq!(handoff.captured(ticket, 1, 9, now), acked(Phase::Checkpoint));
assert_eq!(handoff.installed(ticket, 2, 9, now), acked(Phase::Activate));
let granted = FencingToken::from_raw(8)?; // from your lease authority
// A token of another authority's scope has no order against the source's.
let foreign = FencingToken::from_raw((3 << 48) | 8)?;
assert_eq!(handoff.fenced(ticket, foreign, now), Err(Refusal::Unknown));
assert_eq!(handoff.fenced(ticket, held, now), Err(Refusal::StaleToken));
assert_eq!(handoff.fenced(ticket, granted, now), acked(Phase::Activate));
assert_eq!(handoff.activated(ticket, 2, granted, now), acked(Phase::Complete));
assert_eq!(handoff.active(), 0);
# Some(())
# }
# assert_eq!(example(), Some(()));
```

## What stays outside

1. **Durable replica identities.** Replicas are `u64`s the caller vouches for.
   The coordinator compares them; it cannot tell a restarted replica from an
   impostor, and an acknowledgement is believed if its ticket and role match.
2. **Serialized fenced activation.** The token passed to `fenced` must come from
   one serialized authority (`lease-machines` models its decisions), and the
   resource must reject lower tokens atomically with each write. The coordinator
   only checks that the token is of the source token's scope and above it. Without resource-side
   fencing, a source that missed `Quiesce` keeps writing.
3. **Transfer replay protection.** Generations refuse stale acknowledgements
   inside one coordinator lifetime. Across restarts, persist `generation()` before
   exposing a ticket and rebuild with `Handoff::restored`, or build every
   incarnation with a scope of its own. In-flight transfers
   are not restored: start them again under a new generation, and let the
   authority's highest token say who owns a partition that was mid-activation.
4. **Delivery and timing.** Orders are restated until acknowledged; delivery,
   retries and deduplication at the replicas are the adapter's. Checkpoint bytes,
   their integrity and the source's retirement after `Complete` are too.
5. **An abort without a token before `fenced` assumes no grant exists.** If the
   authority may already have issued the target a token, report it with `fenced`
   first, then abort with a newer one.
