use core::time::Duration;

use lease_machines::FencingToken;

/// Identity of one transfer: the partition and the generation it was issued under.
///
/// Every acknowledgement carries the ticket. A ticket whose generation is not
/// the partition's current transfer generation is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ticket {
    /// Caller-owned partition identity.
    pub partition: u64,
    /// Coordinator-issued transfer generation; never zero, never reused.
    pub generation: u64,
}

/// Where a transfer stands. `Complete` and `Aborted` are only ever returned
/// from the call that ended the transfer; the slot is free afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

/// Why a call changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// Every transfer slot is in use.
    Full,
    /// The partition already has a transfer in progress.
    Busy,
    /// The source or the target is already in its per-replica limit of transfers.
    ReplicaBusy,
    /// Source and target are the same replica.
    SameReplica,
    /// The generation space is exhausted; generations never wrap.
    Exhausted,
    /// `now` is earlier than the latest accepted observation.
    ClockRegressed,
    /// The ticket's generation was issued but is not the partition's current transfer.
    Stale,
    /// The ticket was never issued by this coordinator.
    Unknown,
    /// The transfer is not waiting for this acknowledgement.
    OutOfPhase,
    /// The acknowledgement came from a replica that does not hold that role.
    WrongReplica,
    /// The quiesce point is below the transfer's base offset.
    BehindBase,
    /// The offset is at or beyond the quiesce point.
    BeyondDrain,
    /// Already acknowledged, already confirmed, or never required.
    Duplicate,
    /// The offset is beyond the drain window; confirm commits to slide it.
    OutOfWindow,
    /// The confirmed offset is beyond the contiguous acknowledged prefix.
    Unacked,
    /// The checkpoint or token is not the one this transfer recorded.
    Mismatch,
    /// The token does not exceed every token this transfer already knows.
    StaleToken,
    /// Activation was ordered; aborting needs a newer fencing token.
    FenceRequired,
}

/// Who an order is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Recipient {
    /// A replica, by its caller-owned durable identity.
    Replica(u64),
    /// The serialized lease authority that issues fencing tokens.
    Authority,
}

/// What the recipient is asked to do. Orders are restated until acknowledged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
    /// The transfer this order belongs to; echo it in the acknowledgement.
    pub ticket: Ticket,
    /// Who must act.
    pub to: Recipient,
    /// What they must do.
    pub action: Action,
}

/// A read-only view of one active transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// The transfer's identity.
    pub ticket: Ticket,
    /// Replica giving the partition up.
    pub source: u64,
    /// Replica taking the partition over.
    pub target: u64,
    /// Current phase; never `Complete` or `Aborted`.
    pub phase: Phase,
    /// Lowest offset not yet confirmed committed.
    pub base: u64,
    /// The quiesce point, once the source reported it.
    pub until: Option<u64>,
    /// The checkpoint identity, once the source reported it.
    pub checkpoint: Option<u64>,
    /// The activation token, once the authority issued it.
    pub token: Option<FencingToken>,
    /// When the current phase becomes overdue.
    pub deadline: Duration,
}

/// Admission and timing limits for a coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How long one phase may last before `overdue` reports the transfer.
    pub phase_budget: Duration,
    /// How many concurrent transfers one replica may take part in.
    pub per_replica: usize,
}
