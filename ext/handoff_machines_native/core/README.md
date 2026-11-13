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
- Each phase advances only on an explicit acknowledgement from the replica that
  holds the role: `quiesced` (source, with the quiesce point), `drained` (source,
  one in-flight offset at a time), `captured` (source, checkpoint identity),
  `installed` (target, the same identity), `fenced` (the authority's token,
  strictly above the source's), `activated` (target, the same token).
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

```rust
use core::time::Duration;
use handoff_machines::{Acked, Action, FencingToken, Handoff, Limits, Phase, Recipient};

# fn example() -> Option<()> {
let limits = Limits { phase_budget: Duration::from_secs(30), per_replica: 2 };
let mut handoff = Handoff::<8, 4>::new(limits); // 8 transfers, 256-offset drains
let now = Duration::ZERO;
let held = FencingToken::new(7)?;
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
let granted = FencingToken::new(8)?; // from your lease authority
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
   only checks that the token is above the source's. Without resource-side
   fencing, a source that missed `Quiesce` keeps writing.
3. **Transfer replay protection.** Generations refuse stale acknowledgements
   inside one coordinator lifetime. Across restarts, persist `generation()` before
   exposing a ticket and rebuild with `Handoff::restored`. In-flight transfers
   are not restored: start them again under a new generation, and let the
   authority's highest token say who owns a partition that was mid-activation.
4. **Delivery and timing.** Orders are restated until acknowledged; delivery,
   retries and deduplication at the replicas are the adapter's. Checkpoint bytes,
   their integrity and the source's retirement after `Complete` are too.
5. **An abort without a token before `fenced` assumes no grant exists.** If the
   authority may already have issued the target a token, report it with `fenced`
   first, then abort with a newer one.
