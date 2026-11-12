#![no_std]
#![doc = include_str!("../README.md")]

mod iter;
mod transfer;
mod types;

use core::time::Duration;

pub use iter::{Orders, Outstanding, Overdue};
pub use lease_machines::FencingToken;
use transfer::{Next, Transfer};
pub use types::{Action, Limits, Order, Phase, Recipient, Refusal, Status, Ticket};

/// Coordinator for at most `N` concurrent partition transfers, each with a
/// drain window of `WORDS * 64` offsets.
///
/// The coordinator decides what is owed next and who owes it. It sends
/// nothing, stores nothing and issues no fencing tokens: the caller delivers
/// orders, reports acknowledgements and brings tokens from its authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff<const N: usize, const WORDS: usize> {
    slots: [Option<Transfer<WORDS>>; N],
    generation: u64,
    observed: Duration,
    limits: Limits,
}

impl<const N: usize, const WORDS: usize> Handoff<N, WORDS> {
    /// Concurrent transfers this coordinator can hold.
    pub const CAPACITY: usize = N;
    /// Offsets one transfer's drain window covers before it must slide.
    pub const WINDOW: u64 = WORDS as u64 * 64;

    /// Start with no transfers in a fresh generation namespace.
    #[must_use]
    pub const fn new(limits: Limits) -> Self {
        Self::restored(limits, 0)
    }

    /// Start with no transfers, issuing generations above `last_generation`:
    /// the highest generation a previous coordinator of this namespace issued.
    #[must_use]
    pub const fn restored(limits: Limits, last_generation: u64) -> Self {
        Self {
            slots: [const { None }; N],
            generation: last_generation,
            observed: Duration::ZERO,
            limits,
        }
    }

    /// Highest generation issued so far; persist it before exposing a ticket.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of transfers in progress.
    #[must_use]
    pub fn active(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// The limits this coordinator was built with.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// Begin moving `partition` from `source` to `target`.
    ///
    /// `base` is the lowest offset not yet durably committed under the source,
    /// and `source_token` the fencing token the source currently serves under.
    ///
    /// # Errors
    /// `ClockRegressed`, `SameReplica`, `Busy`, `Full`, `ReplicaBusy` or `Exhausted`.
    pub fn start(
        &mut self,
        partition: u64,
        source: u64,
        target: u64,
        base: u64,
        source_token: FencingToken,
        now: Duration,
    ) -> Result<Ticket, Refusal> {
        self.clock(now)?;
        if source == target {
            return Err(Refusal::SameReplica);
        }
        if self.find(partition).is_some() {
            return Err(Refusal::Busy);
        }
        let free = self.slots.iter().position(Option::is_none);
        let free = free.ok_or(Refusal::Full)?;
        if self.load(source) >= self.limits.per_replica
            || self.load(target) >= self.limits.per_replica
        {
            return Err(Refusal::ReplicaBusy);
        }
        let generation = self.generation.checked_add(1).ok_or(Refusal::Exhausted)?;
        let ticket = Ticket {
            partition,
            generation,
        };
        let deadline = self.deadline(now);
        let transfer = Transfer::new(ticket, source, target, base, source_token, deadline)
            .ok_or(Refusal::Exhausted)?;
        let slot = self.slots.get_mut(free).ok_or(Refusal::Full)?;
        *slot = Some(transfer);
        self.generation = generation;
        self.observed = now;
        Ok(ticket)
    }

    /// The source stopped admitting work; `until` is one past the last
    /// admitted offset. Moves to `Drain`, or straight to `Checkpoint` when
    /// nothing is in flight.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase`, `WrongReplica` or `BehindBase`.
    pub fn quiesced(
        &mut self,
        ticket: Ticket,
        replica: u64,
        until: u64,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| transfer.quiesced(replica, until))
    }

    /// The source finished one in-flight `offset`. Moves to `Checkpoint` once
    /// every offset below the quiesce point is acknowledged.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase`, `WrongReplica`, `BeyondDrain`,
    /// `Duplicate` or `OutOfWindow`.
    pub fn drained(
        &mut self,
        ticket: Ticket,
        replica: u64,
        offset: u64,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| transfer.drained(replica, offset))
    }

    /// The source durably committed every offset below `offset`; the drain
    /// window slides there. Needed only when the drain is wider than `WINDOW`.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase`, `WrongReplica`, `Duplicate` or `Unacked`.
    pub fn committed(
        &mut self,
        ticket: Ticket,
        replica: u64,
        offset: u64,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| transfer.committed(replica, offset))
    }

    /// The source captured `checkpoint` at the quiesce point.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` or `WrongReplica`.
    pub fn captured(
        &mut self,
        ticket: Ticket,
        replica: u64,
        checkpoint: u64,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| {
            transfer.captured(replica, checkpoint)
        })
    }

    /// The target installed `checkpoint`. Moves to `Activate`.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase`, `WrongReplica` or `Mismatch`.
    pub fn installed(
        &mut self,
        ticket: Ticket,
        replica: u64,
        checkpoint: u64,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| {
            transfer.installed(replica, checkpoint)
        })
    }

    /// The authority issued the target `token`, which must exceed the
    /// source's token. From here on, aborting needs a newer token still.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` or `StaleToken`.
    pub fn fenced(
        &mut self,
        ticket: Ticket,
        token: FencingToken,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| transfer.fenced(token))
    }

    /// The target is serving under `token`. Ends the transfer as `Complete`.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase`, `WrongReplica` or `Mismatch`.
    pub fn activated(
        &mut self,
        ticket: Ticket,
        replica: u64,
        token: FencingToken,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| transfer.activated(replica, token))
    }

    /// Give the transfer up from any phase and move to `Rollback`.
    ///
    /// A `token` fences the rollback: it must exceed every token the transfer
    /// knows, and the source resumes under it. It is required once activation
    /// was ordered, because the target may already be serving.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase`, `StaleToken` or `FenceRequired`.
    pub fn abort(
        &mut self,
        ticket: Ticket,
        token: Option<FencingToken>,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| transfer.abort(token))
    }

    /// The source is serving again. Ends the rollback as `Aborted` once no
    /// other rollback acknowledgement is owed.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase`, `WrongReplica` or `Duplicate`.
    pub fn resumed(
        &mut self,
        ticket: Ticket,
        replica: u64,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| transfer.resumed(replica))
    }

    /// The target dropped what it installed. Ends the rollback as `Aborted`
    /// once no other rollback acknowledgement is owed.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase`, `WrongReplica` or `Duplicate`.
    pub fn discarded(
        &mut self,
        ticket: Ticket,
        replica: u64,
        now: Duration,
    ) -> Result<Phase, Refusal> {
        self.apply(ticket, now, |transfer| transfer.discarded(replica))
    }

    /// The transfer currently moving `partition`, if any.
    #[must_use]
    pub fn status(&self, partition: u64) -> Option<Status> {
        self.find(partition)
            .and_then(|index| self.slots.get(index)?.as_ref())
            .map(Transfer::status)
    }

    /// Outstanding orders from position `from` (zero to begin). At most two
    /// per transfer, so a full pass yields at most `2 * N`. Stop any time and
    /// resume from [`Orders::position`].
    #[must_use]
    pub const fn orders(&self, from: usize) -> Orders<'_, WORDS> {
        Orders::new(&self.slots, from)
    }

    /// Offsets the drain still waits for, ascending from `from`. Resume by
    /// passing the last yielded offset plus one.
    ///
    /// # Errors
    /// `Stale` or `Unknown` for the ticket, `OutOfPhase` outside `Drain`.
    pub fn outstanding(
        &self,
        ticket: Ticket,
        from: u64,
    ) -> Result<Outstanding<'_, WORDS>, Refusal> {
        let index = self.locate(ticket)?;
        let transfer = self.slots.get(index).and_then(Option::as_ref);
        let (tracker, until) = transfer
            .and_then(Transfer::draining)
            .ok_or(Refusal::OutOfPhase)?;
        Ok(Outstanding::new(tracker, from, until))
    }

    /// Transfers whose current phase reached its deadline at `now`, from slot
    /// `from` (zero to begin). Resume from [`Overdue::position`]. Advisory:
    /// the coordinator never aborts on its own.
    #[must_use]
    pub const fn overdue(&self, now: Duration, from: usize) -> Overdue<'_, WORDS> {
        Overdue::new(&self.slots, now, from)
    }

    const fn clock(&self, now: Duration) -> Result<(), Refusal> {
        if now.as_nanos() < self.observed.as_nanos() {
            Err(Refusal::ClockRegressed)
        } else {
            Ok(())
        }
    }

    const fn deadline(&self, now: Duration) -> Duration {
        now.saturating_add(self.limits.phase_budget)
    }

    fn find(&self, partition: u64) -> Option<usize> {
        self.slots.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|transfer| transfer.ticket.partition == partition)
        })
    }

    fn load(&self, replica: u64) -> usize {
        let busy = self.slots.iter().flatten();
        busy.filter(|transfer| transfer.involves(replica)).count()
    }

    fn locate(&self, ticket: Ticket) -> Result<usize, Refusal> {
        let index = self.slots.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|transfer| transfer.ticket == ticket)
        });
        index.ok_or(
            if ticket.generation == 0 || ticket.generation > self.generation {
                Refusal::Unknown
            } else {
                Refusal::Stale
            },
        )
    }

    fn apply(
        &mut self,
        ticket: Ticket,
        now: Duration,
        ack: impl FnOnce(&mut Transfer<WORDS>) -> Result<Next, Refusal>,
    ) -> Result<Phase, Refusal> {
        self.clock(now)?;
        let index = self.locate(ticket)?;
        let deadline = self.deadline(now);
        let slot = self.slots.get_mut(index).ok_or(Refusal::Unknown)?;
        let transfer = slot.as_mut().ok_or(Refusal::Unknown)?;
        let phase = match ack(transfer)? {
            Next::Same => transfer.phase(),
            Next::Moved => {
                transfer.deadline = deadline;
                transfer.phase()
            }
            Next::Done(phase) => {
                *slot = None;
                phase
            }
        };
        self.observed = now;
        Ok(phase)
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
