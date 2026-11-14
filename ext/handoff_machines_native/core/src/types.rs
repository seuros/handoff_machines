use core::time::Duration;

use lease_machines::FencingToken;
use periscope_machines::{scope_of, sequence_of};

/// Identity of one transfer: the partition and the generation it was issued under.
///
/// Every acknowledgement carries the ticket. A ticket whose generation is not
/// the partition's current transfer generation is refused, and so is one
/// whose generation another coordinator's scope issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ticket {
    pub(crate) partition: u64,
    pub(crate) generation: u64,
}

impl Ticket {
    /// A ticket from its two fields, as one that travelled to a replica and
    /// back is rebuilt. `None` when `generation` carries a zero sequence: no
    /// coordinator issues it, whatever its scope.
    #[must_use]
    pub const fn new(partition: u64, generation: u64) -> Option<Self> {
        if sequence_of(generation) == 0 {
            return None;
        }
        Some(Self {
            partition,
            generation,
        })
    }

    /// Caller-owned partition identity.
    #[must_use]
    pub const fn partition(&self) -> u64 {
        self.partition
    }

    /// Coordinator-issued transfer generation; never zero, never reused. It
    /// carries the coordinator's scope above a sequence counted from one, so
    /// scope zero issues 1, 2, 3.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The scope of the coordinator that issued this ticket, see
    /// [`Handoff::with_scope`](crate::Handoff::with_scope).
    #[must_use]
    pub const fn scope(&self) -> u16 {
        scope_of(self.generation)
    }
}

/// Where a transfer stands. `Complete` and `Aborted` are only ever returned
/// from the call that ended the transfer; the slot is free afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the source to stop admitting new work.
    Quiesce,
    /// Waiting for in-flight offsets below the quiesce point.
    Drain,
    /// Waiting for the source to capture, then the target to install.
    Checkpoint,
    /// Waiting for a fencing token, then the target's activation.
    Activate,
    /// Aborted; waiting for the replicas to undo what they were told.
    Rollback,
    /// The target owns the partition under the activation token.
    Complete,
    /// Every rollback acknowledgement arrived; the source owns the partition.
    Aborted,
}

impl Phase {
    /// Whether the transfer is over: `Complete` or `Aborted`. No live transfer
    /// stands at a final phase.
    #[must_use]
    pub const fn is_final(self) -> bool {
        match self {
            Self::Quiesce | Self::Drain | Self::Checkpoint | Self::Activate | Self::Rollback => {
                false
            }
            Self::Complete | Self::Aborted => true,
        }
    }
}

/// Honoured outcome of every acknowledgement. Each may be delivered more than
/// once; the transfer answers a repeat from what it recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acked {
    /// Recorded for the first time; the transfer now stands at this phase.
    Recorded(Phase),
    /// Idempotent repeat: the transfer already recorded this acknowledgement
    /// with the same payload (quiesce point, offset, checkpoint, token), or
    /// the rollback never owed it. Nothing changed, not even the observed
    /// clock; the transfer still stands at this phase, which may be a later
    /// one than the acknowledgement first led to.
    Duplicate(Phase),
}

impl Acked {
    /// `Recorded(phase)`, or `None` for `Quiesce`: a transfer starts there, so
    /// no acknowledgement ever leads to it.
    #[must_use]
    pub const fn recorded(phase: Phase) -> Option<Self> {
        match phase {
            Phase::Quiesce => None,
            Phase::Drain
            | Phase::Checkpoint
            | Phase::Activate
            | Phase::Rollback
            | Phase::Complete
            | Phase::Aborted => Some(Self::Recorded(phase)),
        }
    }

    /// `Duplicate(phase)`, or `None` for `Quiesce` (nothing is recorded yet,
    /// so nothing repeats) and for a final phase (the slot is free, and a
    /// repeat on a finished ticket is `Stale`).
    #[must_use]
    pub const fn duplicate(phase: Phase) -> Option<Self> {
        match phase {
            Phase::Quiesce | Phase::Complete | Phase::Aborted => None,
            Phase::Drain | Phase::Checkpoint | Phase::Activate | Phase::Rollback => {
                Some(Self::Duplicate(phase))
            }
        }
    }

    /// Where the transfer stands after the call.
    #[must_use]
    pub const fn phase(self) -> Phase {
        match self {
            Self::Recorded(phase) | Self::Duplicate(phase) => phase,
        }
    }

    /// Whether the call was an idempotent repeat that changed nothing.
    #[must_use]
    pub const fn is_duplicate(self) -> bool {
        match self {
            Self::Recorded(_) => false,
            Self::Duplicate(_) => true,
        }
    }
}

/// Who an order is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipient {
    /// A replica, by its caller-owned durable identity.
    Replica(u64),
    /// The serialized lease authority that issues fencing tokens.
    Authority,
}

/// What the recipient is asked to do. Orders are restated until acknowledged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Source: stop admitting new work and report the quiesce point.
    Quiesce,
    /// Source: finish in-flight offsets below `until` and acknowledge each.
    Drain {
        /// One past the last admitted offset.
        until: u64,
    },
    /// Source: capture a checkpoint at exactly `at` and report its identity.
    Capture {
        /// The drained quiesce point.
        at: u64,
    },
    /// Target: install the named checkpoint and acknowledge it.
    Install {
        /// Checkpoint identity reported by the source.
        checkpoint: u64,
    },
    /// Authority: issue the target a token strictly greater than `above`.
    Fence {
        /// The source's token; the activation token must exceed it.
        above: FencingToken,
    },
    /// Target: start serving under `token` and acknowledge it.
    Activate {
        /// The activation token.
        token: FencingToken,
    },
    /// Source: resume serving; under `token` when the rollback was fenced.
    Resume {
        /// The rollback token, when one was supplied to `abort`.
        token: Option<FencingToken>,
    },
    /// Target: stop serving if it started, and drop the installed checkpoint.
    Discard,
}

/// One outstanding instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub(crate) ticket: Ticket,
    pub(crate) to: Recipient,
    pub(crate) action: Action,
}

impl Order {
    /// An order from its three fields. `None` when the recipient cannot be
    /// asked for the action: only the authority fences, and it does nothing
    /// else. Also `None` for a ticket no coordinator issues.
    #[must_use]
    pub const fn new(ticket: Ticket, to: Recipient, action: Action) -> Option<Self> {
        if Ticket::new(ticket.partition, ticket.generation).is_none() {
            return None;
        }
        let authority = match to {
            Recipient::Authority => true,
            Recipient::Replica(_) => false,
        };
        let fence = match action {
            Action::Fence { .. } => true,
            Action::Quiesce
            | Action::Drain { .. }
            | Action::Capture { .. }
            | Action::Install { .. }
            | Action::Activate { .. }
            | Action::Resume { .. }
            | Action::Discard => false,
        };
        let fits = authority == fence;
        if fits {
            Some(Self { ticket, to, action })
        } else {
            None
        }
    }

    /// The transfer this order belongs to; echo it in the acknowledgement.
    #[must_use]
    pub const fn ticket(&self) -> Ticket {
        self.ticket
    }

    /// Who must act.
    #[must_use]
    pub const fn to(&self) -> Recipient {
        self.to
    }

    /// What they must do.
    #[must_use]
    pub const fn action(&self) -> Action {
        self.action
    }
}

/// A read-only view of one active transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub(crate) ticket: Ticket,
    pub(crate) source: u64,
    pub(crate) target: u64,
    pub(crate) phase: Phase,
    pub(crate) base: u64,
    pub(crate) until: Option<u64>,
    pub(crate) checkpoint: Option<u64>,
    pub(crate) token: Option<FencingToken>,
    pub(crate) deadline: Duration,
}

impl Status {
    /// A view from its fields, as one that was stored or sent is rebuilt;
    /// `None` when no coordinator could have answered it.
    ///
    /// A live transfer has a ticket some coordinator issues, two different
    /// replicas and a phase that is not final. What it has recorded follows
    /// its phase: no quiesce point while it waits for one, one above `base`
    /// during the drain and one at or above it afterwards; a checkpoint only after a quiesce point, none
    /// before the drain is over and one once activation is under way; an
    /// activation token only with a checkpoint, and only while activating or
    /// rolling back. A rollback keeps whatever was recorded when it began.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // One per field, in field order.
    pub const fn new(
        ticket: Ticket,
        source: u64,
        target: u64,
        phase: Phase,
        base: u64,
        until: Option<u64>,
        checkpoint: Option<u64>,
        token: Option<FencingToken>,
        deadline: Duration,
    ) -> Option<Self> {
        Self {
            ticket,
            source,
            target,
            phase,
            base,
            until,
            checkpoint,
            token,
            deadline,
        }
        .checked()
    }

    /// This view, when a coordinator could have answered it.
    pub(crate) const fn checked(self) -> Option<Self> {
        let (until, checkpoint, token) = (
            self.until.is_some(),
            self.checkpoint.is_some(),
            self.token.is_some(),
        );
        let recorded = match self.phase {
            Phase::Quiesce => !until && !checkpoint && !token,
            Phase::Drain => until && !checkpoint && !token,
            Phase::Checkpoint => until && !token,
            Phase::Activate => until && checkpoint,
            Phase::Rollback => (until || !checkpoint) && (checkpoint || !token),
            Phase::Complete | Phase::Aborted => false,
        };
        // A drain lasts only while the base is below the quiesce point: both
        // `quiesced` at the base and `drained` up to it move to the capture.
        let based = match (self.phase, self.until) {
            (Phase::Drain, Some(until)) => self.base < until,
            (_, Some(until)) => self.base <= until,
            (_, None) => true,
        };
        let issued = Ticket::new(self.ticket.partition, self.ticket.generation).is_some();
        if recorded && based && issued && self.source != self.target {
            Some(self)
        } else {
            None
        }
    }

    /// The transfer's identity.
    #[must_use]
    pub const fn ticket(&self) -> Ticket {
        self.ticket
    }

    /// Replica giving the partition up.
    #[must_use]
    pub const fn source(&self) -> u64 {
        self.source
    }

    /// Replica taking the partition over.
    #[must_use]
    pub const fn target(&self) -> u64 {
        self.target
    }

    /// Current phase; never `Complete` or `Aborted`.
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// Lowest offset not yet confirmed committed.
    #[must_use]
    pub const fn base(&self) -> u64 {
        self.base
    }

    /// The quiesce point, once the source reported it.
    #[must_use]
    pub const fn until(&self) -> Option<u64> {
        self.until
    }

    /// The checkpoint identity, once the source reported it.
    #[must_use]
    pub const fn checkpoint(&self) -> Option<u64> {
        self.checkpoint
    }

    /// The activation token, once the authority issued it.
    #[must_use]
    pub const fn token(&self) -> Option<FencingToken> {
        self.token
    }

    /// When the current phase becomes overdue.
    #[must_use]
    pub const fn deadline(&self) -> Duration {
        self.deadline
    }
}

/// Admission and timing limits for a coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub(crate) phase_budget: Duration,
    pub(crate) per_replica: usize,
}

impl Limits {
    /// Limits a coordinator can admit a transfer under. `None` when
    /// `per_replica` is zero: no replica could take part in any transfer, so
    /// every [`Handoff::start`](crate::Handoff::start) would be refused. Any
    /// `phase_budget` is accepted; zero makes every phase overdue at once.
    #[must_use]
    pub const fn new(phase_budget: Duration, per_replica: usize) -> Option<Self> {
        if per_replica == 0 {
            return None;
        }
        Some(Self {
            phase_budget,
            per_replica,
        })
    }

    /// How long one phase may last before `overdue` reports the transfer.
    #[must_use]
    pub const fn phase_budget(&self) -> Duration {
        self.phase_budget
    }

    /// How many concurrent transfers one replica may take part in.
    #[must_use]
    pub const fn per_replica(&self) -> usize {
        self.per_replica
    }
}
