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

pub const REFUSALS: [&str; 18] = [
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
    "duplicate",
    "out_of_window",
    "unacked",
    "mismatch",
    "stale_token",
    "fence_required",
];

pub const fn phase(phase: Phase) -> &'static str {
    match phase {
        Phase::Quiesce => "quiesce",
        Phase::Drain => "drain",
        Phase::Checkpoint => "checkpoint",
        Phase::Activate => "activate",
        Phase::Rollback => "rollback",
        Phase::Complete => "complete",
        _ => "aborted",
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
        Refusal::Duplicate => "duplicate",
        Refusal::OutOfWindow => "out_of_window",
        Refusal::Unacked => "unacked",
        Refusal::Mismatch => "mismatch",
        Refusal::StaleToken => "stale_token",
        _ => "fence_required",
    }
}

pub const fn outcome(outcome: Result<Phase, Refusal>) -> &'static str {
    match outcome {
        Ok(next) => phase(next),
        Err(refused) => refusal(refused),
    }
}

/// Recipient (replica Integer or `:authority`), action name and its operand.
pub fn order(ruby: &Ruby, order: &Order) -> (Value, Symbol, Option<u64>) {
    let to = match order.to {
        Recipient::Replica(replica) => replica.into_value_with(ruby),
        _ => ruby.to_symbol("authority").as_value(),
    };
    let (name, value) = match order.action {
        Action::Quiesce => ("quiesce", None),
        Action::Drain { until } => ("drain", Some(until)),
        Action::Capture { at } => ("capture", Some(at)),
        Action::Install { checkpoint } => ("install", Some(checkpoint)),
        Action::Fence { above } => ("fence", Some(above.value())),
        Action::Activate { token } => ("activate", Some(token.value())),
        Action::Resume { token } => ("resume", token.map(FencingToken::value)),
        _ => ("discard", None),
    };
    (to, ruby.to_symbol(name), value)
}
