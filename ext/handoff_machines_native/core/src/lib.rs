#![no_std]
#![doc = include_str!("../README.md")]

mod iter;
mod refusal;
mod transfer;
mod types;

use core::time::Duration;

pub use iter::{Orders, Outstanding, Overdue};
pub use lease_machines::FencingToken;
use periscope_machines::{Issuer, scope_of, scoped, sequence_of};
pub use refusal::Refusal;
use transfer::{Next, Transfer};
pub use types::{Acked, Action, Limits, Order, Phase, Recipient, Status, Ticket};

/// Coordinator for at most `N` concurrent partition transfers, each with a
/// drain window of `WORDS * 64` offsets.
///
/// The coordinator decides what is owed next and who owes it. It sends
/// nothing, stores nothing and issues no fencing tokens: the caller delivers
/// orders, reports acknowledgements and brings tokens from its authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff<const N: usize, const WORDS: usize> {
    slots: [Option<Transfer<WORDS>>; N],
    /// Issues the ticket generations; it also holds the coordinator's scope.
    generations: Issuer,
    observed: Duration,
    limits: Limits,
}

impl<const N: usize, const WORDS: usize> Handoff<N, WORDS> {
    /// Concurrent transfers this coordinator can hold.
    pub const CAPACITY: usize = N;
    /// Offsets one transfer's drain window covers before it must slide.
    pub const WINDOW: u64 = WORDS as u64 * 64;

    /// Start with no transfers in a fresh generation namespace, in scope zero.
    #[must_use]
    pub const fn new(limits: Limits) -> Self {
        Self::with_scope(limits, 0)
    }

    /// Start with no transfers in a fresh generation namespace whose
    /// generations carry `scope`.
    ///
    /// A generation is a plain integer counted from one, so two coordinators
    /// in the same scope issue the same tickets for the same partition, and
    /// an acknowledgement sent to the wrong one is recorded there. Give every
    /// coordinator whose tickets could meet its own scope: a ticket of
    /// another scope is then always [`Refusal::Unknown`]. The coordinator
    /// cannot pick a scope itself; keeping them distinct is the caller's job.
    #[must_use]
    pub const fn with_scope(limits: Limits, scope: u16) -> Self {
        Self::resuming(limits, Issuer::new(scope))
    }

    /// Start with no transfers in `scope`, issuing generations above
    /// `last_generation`: what [`Handoff::generation`] answered on the
    /// previous coordinator of this namespace.
    ///
    /// The scope is the caller's, never read out of the saved value, and the
    /// saved value must carry it: `None` when `last_generation` is of another
    /// scope. So a value that lost its top bits, or the `0` a store answers
    /// for a row that is not there, cannot put a coordinator of a non-zero
    /// scope into scope zero. The one value that means "nothing issued yet"
    /// is `scope` above a zero sequence, which is what `generation()` answers
    /// then; it is `0` only in scope zero. With no saved value at all, call
    /// [`Handoff::with_scope`].
    #[must_use]
    pub const fn restored(limits: Limits, scope: u16, last_generation: u64) -> Option<Self> {
        if scope_of(last_generation) != scope {
            return None;
        }
        match Issuer::resumed(scope, sequence_of(last_generation)) {
            Some(generations) => Some(Self::resuming(limits, generations)),
            // Unreachable: a sequence taken out of an id is never past the last.
            None => None,
        }
    }

    const fn resuming(limits: Limits, generations: Issuer) -> Self {
        Self {
            slots: [const { None }; N],
            generations,
            observed: Duration::ZERO,
            limits,
        }
    }

    /// The scope carried by every generation this coordinator issues.
    #[must_use]
    pub const fn scope(&self) -> u16 {
        self.generations.scope()
    }

    /// Highest generation issued so far; persist it before exposing a ticket.
    ///
    /// Before the first, it is the scope alone above a zero sequence:
    /// `scope << 48`, which is zero only in scope zero. That value is not a
    /// generation (no ticket carries it, and a ticket made from it is
    /// [`Refusal::Unknown`]), so read "nothing issued yet" from its sequence,
    /// not by comparing it with zero. Persist the whole `u64` beside
    /// [`Handoff::scope`]: [`Handoff::restored`] takes both and refuses a
    /// value that is not of that scope.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        scoped(self.generations.scope(), self.generations.issued())
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
        let generation = match self.generations.peek() {
            Ok(generation) => generation,
            Err(periscope_machines::Refusal::Exhausted) => return Err(Refusal::Exhausted),
        };
        let ticket = Ticket {
            partition,
            generation,
        };
        let deadline = self.deadline(now);
        let transfer = Transfer::new(ticket, source, target, base, source_token, deadline)
            .ok_or(Refusal::Exhausted)?;
        let slot = self.slots.get_mut(free).ok_or(Refusal::Full)?;
        match self.generations.issue() {
            Ok(_) => *slot = Some(transfer),
            Err(periscope_machines::Refusal::Exhausted) => return Err(Refusal::Exhausted),
        }
        self.observed = now;
        Ok(ticket)
    }

    /// The source stopped admitting work; `until` is one past the last
    /// admitted offset. Moves to `Drain`, or straight to `Checkpoint` when
    /// nothing is in flight.
    ///
    /// Repeated with the recorded `until`, for as long as the transfer lives,
    /// it is an idempotent repeat: [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (the transfer was given up before the
    /// source quiesced), `WrongReplica`, `BehindBase`, or `Mismatch` when a
    /// different quiesce point is already recorded.
    pub fn quiesced(
        &mut self,
        ticket: Ticket,
        replica: u64,
        until: u64,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| transfer.quiesced(replica, until))
    }

    /// The source finished one in-flight `offset`. Moves to `Checkpoint` once
    /// every offset below the quiesce point is acknowledged.
    ///
    /// An offset already acknowledged or already committed is an idempotent
    /// repeat, during the drain and after it: [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (before the quiesce point is known, or
    /// an unacknowledged offset once the drain was given up), `WrongReplica`,
    /// `BeyondDrain` or `OutOfWindow`.
    pub fn drained(
        &mut self,
        ticket: Ticket,
        replica: u64,
        offset: u64,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| transfer.drained(replica, offset))
    }

    /// The source durably committed every offset below `offset`; the drain
    /// window slides there. Needed only when the drain is wider than `WINDOW`.
    ///
    /// An offset not beyond the window base is already committed, an
    /// idempotent repeat during the drain and after it: [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (before the quiesce point is known, or
    /// a new offset once the drain is over), `WrongReplica` or `Unacked`.
    pub fn committed(
        &mut self,
        ticket: Ticket,
        replica: u64,
        offset: u64,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| transfer.committed(replica, offset))
    }

    /// The source captured `checkpoint` at the quiesce point.
    ///
    /// Repeated with the recorded checkpoint it is an idempotent repeat:
    /// [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (the drain is not over), `WrongReplica`,
    /// or `Mismatch` when a different checkpoint is already recorded.
    pub fn captured(
        &mut self,
        ticket: Ticket,
        replica: u64,
        checkpoint: u64,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| {
            transfer.captured(replica, checkpoint)
        })
    }

    /// The target installed `checkpoint`. Moves to `Activate`.
    ///
    /// Repeated once recorded it is an idempotent repeat: [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (no checkpoint was captured, or the
    /// transfer was given up before the install), `WrongReplica`, or
    /// `Mismatch` when `checkpoint` is not the captured one.
    pub fn installed(
        &mut self,
        ticket: Ticket,
        replica: u64,
        checkpoint: u64,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| {
            transfer.installed(replica, checkpoint)
        })
    }

    /// The authority issued the target `token`, which must exceed the
    /// source's token. From here on, aborting needs a newer token still.
    ///
    /// Repeated with the recorded token it is an idempotent repeat:
    /// [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (no token is awaited yet, or the
    /// transfer was given up before one arrived), `StaleToken`, or `Mismatch`
    /// when a different token is already recorded.
    pub fn fenced(
        &mut self,
        ticket: Ticket,
        token: FencingToken,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| transfer.fenced(token))
    }

    /// The target is serving under `token`. Ends the transfer as `Complete`.
    ///
    /// It frees the slot, so a repeat cannot be told from any other finished
    /// ticket and is refused as `Stale`; this call never answers a duplicate.
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (activation is not ordered, or the
    /// transfer was given up), `WrongReplica` or `Mismatch`.
    pub fn activated(
        &mut self,
        ticket: Ticket,
        replica: u64,
        token: FencingToken,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| transfer.activated(replica, token))
    }

    /// Give the transfer up from any phase and move to `Rollback`.
    ///
    /// A `token` fences the rollback: it must exceed every token the transfer
    /// knows, and the source resumes under it. It is required once activation
    /// was ordered, because the target may already be serving.
    ///
    /// During the rollback, repeated with the same `token` (or the same
    /// absence of one), it is an idempotent repeat: [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `StaleToken`, `FenceRequired`, or `Mismatch` when the
    /// rollback is already under way with a different token.
    pub fn abort(
        &mut self,
        ticket: Ticket,
        token: Option<FencingToken>,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| transfer.abort(token))
    }

    /// The source is serving again. Ends the rollback as `Aborted` once no
    /// other rollback acknowledgement is owed.
    ///
    /// While the rollback still waits for the target, a second `resumed` is
    /// an idempotent repeat: [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (no rollback was ordered) or
    /// `WrongReplica`.
    pub fn resumed(
        &mut self,
        ticket: Ticket,
        replica: u64,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| transfer.resumed(replica))
    }

    /// The target dropped what it installed. Ends the rollback as `Aborted`
    /// once no other rollback acknowledgement is owed.
    ///
    /// A `discarded` the rollback does not wait for (already recorded, or
    /// never owed because the target was never asked to install) is an
    /// idempotent repeat: [`Acked::Duplicate`].
    ///
    /// # Errors
    /// A ticket refusal, `OutOfPhase` (no rollback was ordered) or
    /// `WrongReplica`.
    pub fn discarded(
        &mut self,
        ticket: Ticket,
        replica: u64,
        now: Duration,
    ) -> Result<Acked, Refusal> {
        self.acked(ticket, now, |transfer| transfer.discarded(replica))
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

    /// A generation this coordinator did not issue (another scope, or a
    /// sequence it has not reached) is unknown: no slot is read. One it did
    /// issue and holds in no slot is stale.
    fn locate(&self, ticket: Ticket) -> Result<usize, Refusal> {
        if !self.generations.owns(ticket.generation) {
            return Err(Refusal::Unknown);
        }
        let index = self.slots.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|transfer| transfer.ticket == ticket)
        });
        index.ok_or(Refusal::Stale)
    }

    /// A repeat answers before `observed` moves: it changes nothing at all.
    fn acked(
        &mut self,
        ticket: Ticket,
        now: Duration,
        ack: impl FnOnce(&mut Transfer<WORDS>) -> Result<Next, Refusal>,
    ) -> Result<Acked, Refusal> {
        self.clock(now)?;
        let index = self.locate(ticket)?;
        let deadline = self.deadline(now);
        let slot = self.slots.get_mut(index).ok_or(Refusal::Unknown)?;
        let transfer = slot.as_mut().ok_or(Refusal::Unknown)?;
        let phase = match ack(transfer)? {
            Next::Repeat => return Ok(Acked::Duplicate(transfer.phase())),
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
        Ok(Acked::Recorded(phase))
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
