//! Private hooks the Ruby `Data` values validate themselves with, each
//! through the core constructor of the type it mirrors, and the readers for
//! every value the binding takes from Ruby. `TypeError` for a value of the
//! wrong type (a subclass of one of this gem's classes included), `RangeError`
//! for one the core would not build.
use handoff_machines::{
    Acked, Action, FencingToken, Limits, Order, Phase, Recipient, Status, Ticket,
};
use magnus::{Error, Ruby, Value, prelude::*};
use matryona_machines::ruby::{
    Choice, Class, Symbols, duration, exact_fields, instant, integer, optional,
    own_or_sibling_with, symbol_or_fields,
};

use crate::classes::classes;
use crate::names::{Actions, Phases, Recipients};
use crate::raw_token;

fn range(ruby: &Ruby, message: &'static str) -> Error {
    Error::new(ruby.exception_range_error(), message)
}

/// A phase from its symbol: `TypeError` for a non-Symbol, `RangeError` for a
/// Symbol that names no phase.
pub fn phase(ruby: &Ruby, value: Value) -> Result<Phase, Error> {
    Phases::parse(ruby, value)
}

/// The core's `Limits::new`, its two fields converted in order.
fn limits_of(ruby: &Ruby, phase_budget: Value, per_replica: Value) -> Result<Limits, Error> {
    let phase_budget = duration(phase_budget, "phase_budget")?;
    Limits::new(phase_budget, integer(per_replica, "per_replica")?)
        .ok_or_else(|| range(ruby, "per_replica must be at least 1"))
}

/// The fields of exactly this gem's `class`, or of a sibling gem's value
/// through its public readers; `TypeError` naming both for anything else, a
/// subclass of `class` included. Runs Ruby on the sibling path, so it is
/// called while converting arguments, before any core call.
fn own_or_sibling<const N: usize>(
    ruby: &Ruby,
    value: Value,
    class: Class,
    readers: [&str; N],
) -> Result<[Value; N], Error> {
    let own = class.get(ruby);
    own_or_sibling_with(ruby, value, own, readers, || {
        format!("expected a {own} or a value with #{}", readers.join(", #"))
    })
}

/// A token as the core's: this gem's `FencingToken` or a sibling's.
pub fn token(ruby: &Ruby, value: Value) -> Result<FencingToken, Error> {
    let [raw] = own_or_sibling(ruby, value, classes(ruby)?.token, ["raw"])?;
    raw_token(raw)
}

/// A ticket: only this gem's `Ticket`, through `Ticket::new`.
pub fn ticket(ruby: &Ruby, value: Value) -> Result<Ticket, Error> {
    let class = classes(ruby)?.ticket.get(ruby);
    let message = "expected a HandoffMachines::Ticket";
    let [partition, generation] = exact_fields::<2>(ruby, value, class, message)?;
    crate::ticket(ruby, partition, generation)
}

/// Limits: only this gem's `Limits`, through `Limits::new`.
pub fn limits(ruby: &Ruby, value: Value) -> Result<Limits, Error> {
    let class = classes(ruby)?.limits.get(ruby);
    let message = "expected a HandoffMachines::Limits";
    let [phase_budget, per_replica] = exact_fields::<2>(ruby, value, class, message)?;
    limits_of(ruby, phase_budget, per_replica)
}

/// A `HandoffMachines::FencingToken` of `token`, built by its `new`.
fn token_object(ruby: &Ruby, token: FencingToken) -> Result<Value, Error> {
    let class = classes(ruby)?.token.get(ruby);
    class.funcall("new", (token.raw(),))
}

/// `Fence.new`, `Activate.new`, `Resume.new`: a token read as [`token`]
/// reads it, answered as this gem's own to keep in place of the one given.
pub fn check_token(ruby: &Ruby, held: Value) -> Result<Value, Error> {
    token_object(ruby, token(ruby, held)?)
}

/// `Limits.new`.
pub fn check_limits(ruby: &Ruby, phase_budget: Value, per_replica: Value) -> Result<(), Error> {
    limits_of(ruby, phase_budget, per_replica).map(drop)
}

/// `Ticket.new`: the core's `Ticket::new`.
pub fn check_ticket(ruby: &Ruby, partition: Value, generation: Value) -> Result<(), Error> {
    crate::ticket(ruby, partition, generation).map(drop)
}

/// The acknowledgement `build` makes of `value`'s phase, or the `RangeError`
/// its `None` stands for.
fn acked(ruby: &Ruby, value: Value, build: fn(Phase) -> Option<Acked>) -> Result<(), Error> {
    build(phase(ruby, value)?)
        .map(drop)
        .ok_or_else(|| range(ruby, "no acknowledgement answers this phase"))
}

/// `Recorded.new`: the core's `Acked::recorded`.
pub fn check_recorded(ruby: &Ruby, value: Value) -> Result<(), Error> {
    acked(ruby, value, Acked::recorded)
}

/// `Duplicate.new`: the core's `Acked::duplicate`.
pub fn check_duplicate(ruby: &Ruby, value: Value) -> Result<(), Error> {
    acked(ruby, value, Acked::duplicate)
}

/// `Replica.new`: any unsigned 64-bit Integer, as the core's field is.
pub fn check_replica(id: Value) -> Result<(), Error> {
    integer::<u64>(id, "id").map(drop)
}

/// `Drain.new`: any unsigned 64-bit Integer, as the core's field is.
pub fn check_drain(until: Value) -> Result<(), Error> {
    integer::<u64>(until, "until").map(drop)
}

/// `Capture.new`: any unsigned 64-bit Integer, as the core's field is.
pub fn check_capture(at: Value) -> Result<(), Error> {
    integer::<u64>(at, "at").map(drop)
}

/// `Install.new`: any unsigned 64-bit Integer, as the core's field is.
pub fn check_install(checkpoint: Value) -> Result<(), Error> {
    integer::<u64>(checkpoint, "checkpoint").map(drop)
}

/// The core's `Recipient`: `:authority`, or exactly this gem's `Replica`.
fn recipient(ruby: &Ruby, value: Value) -> Result<Recipient, Error> {
    let message = "a recipient is :authority or a HandoffMachines::Replica";
    let replica = [classes(ruby)?.replica.get(ruby)];
    Ok(
        match symbol_or_fields(ruby, value, Recipients, &replica, message)? {
            Choice::Symbol(recipient) => recipient,
            Choice::Struct(_, [id]) => Recipient::Replica(integer(id, "id")?),
        },
    )
}

/// The core's `Action` from a Symbol (`:quiesce` or `:discard`, the two that
/// carry no field) or exactly one of this gem's action `Data` values.
fn action(ruby: &Ruby, value: Value) -> Result<Action, Error> {
    let message = "an action is :quiesce, :discard or an action Data (exactly its class)";
    let actions = classes(ruby)?.actions.map(|class| class.get(ruby));
    let (index, field) = match symbol_or_fields(ruby, value, Actions, &actions, message)? {
        Choice::Symbol(action) => return Ok(action),
        Choice::Struct(index, [field]) => (index, field),
    };
    Ok(match index {
        0 => Action::Drain {
            until: integer(field, "until")?,
        },
        1 => Action::Capture {
            at: integer(field, "at")?,
        },
        2 => Action::Install {
            checkpoint: integer(field, "checkpoint")?,
        },
        3 => Action::Fence {
            above: token(ruby, field)?,
        },
        4 => Action::Activate {
            token: token(ruby, field)?,
        },
        _resume => Action::Resume {
            token: if field.is_nil() {
                None
            } else {
                Some(token(ruby, field)?)
            },
        },
    })
}

/// `Order.new`: the core's `Order::new`.
pub fn check_order(ruby: &Ruby, held: Value, to: Value, action: Value) -> Result<(), Error> {
    let held = ticket(ruby, held)?;
    let (to, action) = (recipient(ruby, to)?, self::action(ruby, action)?);
    Order::new(held, to, action)
        .map(drop)
        .ok_or_else(|| range(ruby, "only the authority fences, and it does nothing else"))
}

/// `Status.new`: the core's `Status::new`. Answers the token as this gem's
/// own (or nil), to keep in place of the one given.
#[allow(clippy::too_many_arguments)] // One per field of the core's `Status`.
pub fn check_status(
    ruby: &Ruby,
    held: Value,
    source: Value,
    target: Value,
    phase: Value,
    base: Value,
    until: Value,
    checkpoint: Value,
    fence: Option<Value>,
    deadline: Value,
) -> Result<Option<Value>, Error> {
    let held = ticket(ruby, held)?;
    let (source, target) = (integer(source, "source")?, integer(target, "target")?);
    let base = integer(base, "base")?;
    let (until, checkpoint) = (
        optional(until, "until")?,
        optional(checkpoint, "checkpoint")?,
    );
    let fence = fence.map(|fence| token(ruby, fence)).transpose()?;
    let status = Status::new(
        held,
        source,
        target,
        self::phase(ruby, phase)?,
        base,
        until,
        checkpoint,
        fence,
        // Up to `Duration::MAX`: the core saturates `now + phase_budget`
        // past what a `now` can be.
        instant(deadline, "deadline")?,
    );
    status.ok_or_else(|| range(ruby, "no live transfer reports this status"))?;
    fence.map(|fence| token_object(ruby, fence)).transpose()
}
