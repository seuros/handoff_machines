//! The one answer for every call that was not honoured.
use core::fmt;

/// Why a call was not honoured. Every refusal leaves the coordinator exactly
/// as it was. An idempotent repeat is not a refusal: see [`Acked::Duplicate`](crate::Acked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Every transfer slot is in use.
    Full,
    /// The partition already has a transfer in progress.
    Busy,
    /// The source or the target is already in its per-replica limit of transfers.
    ReplicaBusy,
    /// Source and target are the same replica.
    SameReplica,
    /// The scope's generation space (2^48 - 1) is exhausted; generations
    /// never wrap.
    Exhausted,
    /// `now` is earlier than the latest accepted observation.
    ClockRegressed,
    /// The ticket's generation was issued but is not the partition's current transfer.
    Stale,
    /// An id that is not of the namespace the call ranks it in. The ticket
    /// was never issued by this coordinator: its generation is zero, beyond
    /// the last one issued, or of another scope. Or the fencing token offered
    /// to `fenced`, `activated` or `abort` is of another scope than the
    /// transfer's source token: it names nothing in this transfer, and is
    /// answered before the transfer's phase or recorded token is read.
    Unknown,
    /// The transfer is not waiting for this acknowledgement and never
    /// recorded it: it came too early, or the transfer left the path that
    /// wanted it. Never a repeat of something recorded.
    OutOfPhase,
    /// The acknowledgement came from a replica that does not hold that role.
    WrongReplica,
    /// The quiesce point is below the transfer's base offset.
    BehindBase,
    /// The offset is at or beyond the quiesce point.
    BeyondDrain,
    /// The offset is beyond the drain window; confirm commits to slide it.
    OutOfWindow,
    /// The confirmed offset is beyond the contiguous acknowledged prefix.
    Unacked,
    /// The quiesce point, checkpoint or token is not the one this transfer
    /// recorded: a first report that contradicts it, or a repeat that differs.
    Mismatch,
    /// The token is of the transfer's token scope and does not exceed every
    /// token this transfer already knows. A token of another scope is
    /// `Unknown`, never this.
    StaleToken,
    /// Activation was ordered; aborting needs a newer fencing token.
    FenceRequired,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Full => "every transfer slot is in use",
            Self::Busy => "the partition already has a transfer in progress",
            Self::ReplicaBusy => "a replica is at its limit of concurrent transfers",
            Self::SameReplica => "source and target are the same replica",
            Self::Exhausted => "generation space exhausted",
            Self::ClockRegressed => "time is earlier than the latest accepted observation",
            Self::Stale => "the ticket is not the partition's current transfer",
            Self::Unknown => {
                "the ticket was never issued by this coordinator, or the token is of another scope"
            }
            Self::OutOfPhase => {
                "the transfer neither waits for this acknowledgement nor recorded it"
            }
            Self::WrongReplica => "the replica does not hold that role",
            Self::BehindBase => "the quiesce point is below the base offset",
            Self::BeyondDrain => "the offset is at or beyond the quiesce point",
            Self::OutOfWindow => "the offset is beyond the drain window",
            Self::Unacked => "the offset is beyond the acknowledged prefix",
            Self::Mismatch => "the quiesce point, checkpoint or token is not the one recorded",
            Self::StaleToken => "the token does not exceed every known token",
            Self::FenceRequired => "aborting after activation was ordered needs a newer token",
        })
    }
}

impl core::error::Error for Refusal {}
