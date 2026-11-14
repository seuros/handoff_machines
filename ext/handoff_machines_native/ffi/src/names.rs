//! Symbol names for the core's phases, refusals and orders.
use handoff_machines::{Action, FencingToken, Order, Phase, Recipient, Refusal};
use magnus::{IntoValue, Ruby, Symbol, Value, prelude::*};
use matryona_machines::ruby::SymbolNames;

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

matryona_machines::symbols! {
    /// The core's recipients as Symbols: `:authority` reads back; a
    /// replica is its `Replica`.
    pub Recipients for Recipient as "a recipient" => names_only {
        Replica(..) => "replica",
        Authority => "authority",
    }
}

matryona_machines::symbols! {
    /// The core's actions as Symbols: `:quiesce` and `:discard` read back;
    /// every other action is its `Data`.
    pub Actions for Action as "an action" => names_only {
        Quiesce => "quiesce",
        Drain { .. } => "drain",
        Capture { .. } => "capture",
        Install { .. } => "install",
        Fence { .. } => "fence",
        Activate { .. } => "activate",
        Resume { .. } => "resume",
        Discard => "discard",
    }
}

/// Recipient (replica Integer or `:authority`), action name and its field
/// (a token as its raw Integer);
/// the Ruby side wraps them as `Replica` and the action `Data` values.
pub fn order(ruby: &Ruby, order: &Order) -> (Value, Symbol, Option<u64>) {
    let to = match order.to() {
        Recipient::Replica(replica) => replica.into_value_with(ruby),
        Recipient::Authority => Recipients::symbol(ruby, &Recipient::Authority).as_value(),
    };
    let action = order.action();
    let value = match action {
        Action::Quiesce | Action::Discard => None,
        Action::Drain { until } => Some(until),
        Action::Capture { at } => Some(at),
        Action::Install { checkpoint } => Some(checkpoint),
        Action::Fence { above } => Some(above.raw()),
        Action::Activate { token } => Some(token.raw()),
        Action::Resume { token } => token.map(FencingToken::raw),
    };
    (to, Actions::symbol(ruby, &action), value)
}
