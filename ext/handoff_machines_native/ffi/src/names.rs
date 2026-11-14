//! Symbol names for the core's phases, refusals and orders.
use handoff_machines::{Action, FencingToken, Order, Phase, Recipient, Refusal};
use magnus::{IntoValue, Ruby, Symbol, Value, prelude::*};

pub const PHASES: [&str; 7] = [
    "quiesce",
    "drain",
    "checkpoint",
    "activate",
    "rollback",
    "complete",
    "aborted",
];

pub const REFUSALS: [&str; 17] = [
    "full",
    "busy",
    "replica_busy",
    "same_replica",
    "exhausted",
    "clock_regressed",
    "stale",
    "unknown",
    "out_of_phase",
    "wrong_replica",
    "behind_base",
    "beyond_drain",
    "out_of_window",
    "unacked",
    "mismatch",
    "stale_token",
    "fence_required",
];

/// Every phase, in the order of [`PHASES`].
const ALL: [Phase; 7] = [
    Phase::Quiesce,
    Phase::Drain,
    Phase::Checkpoint,
    Phase::Activate,
    Phase::Rollback,
    Phase::Complete,
    Phase::Aborted,
];

/// The phase a symbol name names, if any.
pub fn phase_named(name: &str) -> Option<Phase> {
    ALL.into_iter().find(|candidate| phase(*candidate) == name)
}

pub const fn phase(phase: Phase) -> &'static str {
    match phase {
        Phase::Quiesce => "quiesce",
        Phase::Drain => "drain",
        Phase::Checkpoint => "checkpoint",
        Phase::Activate => "activate",
        Phase::Rollback => "rollback",
        Phase::Complete => "complete",
        Phase::Aborted => "aborted",
    }
}

pub const fn refusal(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::Full => "full",
        Refusal::Busy => "busy",
        Refusal::ReplicaBusy => "replica_busy",
        Refusal::SameReplica => "same_replica",
        Refusal::Exhausted => "exhausted",
        Refusal::ClockRegressed => "clock_regressed",
        Refusal::Stale => "stale",
        Refusal::Unknown => "unknown",
        Refusal::OutOfPhase => "out_of_phase",
        Refusal::WrongReplica => "wrong_replica",
        Refusal::BehindBase => "behind_base",
        Refusal::BeyondDrain => "beyond_drain",
        Refusal::OutOfWindow => "out_of_window",
        Refusal::Unacked => "unacked",
        Refusal::Mismatch => "mismatch",
        Refusal::StaleToken => "stale_token",
        Refusal::FenceRequired => "fence_required",
    }
}

/// Recipient (replica Integer or `:authority`), action name and its field
/// (a token as its raw Integer);
/// the Ruby side wraps them as `Replica` and the action `Data` values.
pub fn order(ruby: &Ruby, order: &Order) -> (Value, Symbol, Option<u64>) {
    let to = match order.to() {
        Recipient::Replica(replica) => replica.into_value_with(ruby),
        Recipient::Authority => ruby.to_symbol("authority").as_value(),
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
    (to, ruby.to_symbol(name), value)
}
