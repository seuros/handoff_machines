//! Symbol names for the core's phases, refusals and orders.
use handoff_machines::{Action, FencingToken, Order, Phase, Recipient, Refusal};
use magnus::{IntoValue, Ruby, Symbol, Value, prelude::*};
use matryona_machines::ruby::symbol;

matryona_machines::symbols! {
    /// The core's phases as Symbols: `TypeError` for a non-Symbol,
    /// `RangeError` for a Symbol that names no phase.
    pub Phases for Phase as "phase" {
        Quiesce => "quiesce",
        Drain => "drain",
        Checkpoint => "checkpoint",
        Activate => "activate",
        Rollback => "rollback",
        Complete => "complete",
        Aborted => "aborted",
    }
}

matryona_machines::symbols! {
    /// The core's refusals as Symbols: the snake case of each variant.
    pub Refusals for Refusal as "a refusal" {
        Full => "full",
        Busy => "busy",
        ReplicaBusy => "replica_busy",
        SameReplica => "same_replica",
        Exhausted => "exhausted",
        ClockRegressed => "clock_regressed",
        Stale => "stale",
        Unknown => "unknown",
        OutOfPhase => "out_of_phase",
        WrongReplica => "wrong_replica",
        BehindBase => "behind_base",
        BeyondDrain => "beyond_drain",
        OutOfWindow => "out_of_window",
        Unacked => "unacked",
        Mismatch => "mismatch",
        StaleToken => "stale_token",
        FenceRequired => "fence_required",
    }
}

/// Recipient (replica Integer or `:authority`), action name and its field
/// (a token as its raw Integer);
/// the Ruby side wraps them as `Replica` and the action `Data` values.
pub fn order(ruby: &Ruby, order: &Order) -> (Value, Symbol, Option<u64>) {
    let to = match order.to() {
        Recipient::Replica(replica) => replica.into_value_with(ruby),
        Recipient::Authority => symbol(ruby, "authority").as_value(),
    };
    let (name, value) = match order.action() {
        Action::Quiesce => ("quiesce", None),
        Action::Drain { until } => ("drain", Some(until)),
        Action::Capture { at } => ("capture", Some(at)),
        Action::Install { checkpoint } => ("install", Some(checkpoint)),
        Action::Fence { above } => ("fence", Some(above.raw())),
        Action::Activate { token } => ("activate", Some(token.raw())),
        Action::Resume { token } => ("resume", token.map(FencingToken::raw)),
        Action::Discard => ("discard", None),
    };
    (to, symbol(ruby, name), value)
}
