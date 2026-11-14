//! Private hooks the Ruby `Data` values validate themselves with, each
//! through the core constructor of the type it mirrors, and the readers for
//! every value the binding takes from Ruby. `TypeError` for a value of the
//! wrong type (a subclass of one of this gem's classes included), `RangeError`
//! for one the core would not build.
use std::time::Duration;

use handoff_machines::{
    Acked, Action, FencingToken, Limits, Order, Phase, Recipient, Status, Ticket,
};
use magnus::{Error, Integer, Ruby, Symbol, TryConvert, Value, prelude::*};

use crate::classes::{ACTIONS, Origin, classes, fields, is};
use crate::{Number, names};

fn range(ruby: &Ruby, message: &'static str) -> Error {
    Error::new(ruby.exception_range_error(), message)
}

/// The name of a Symbol; `TypeError` for anything else.
fn symbol(ruby: &Ruby, value: Value, what: &'static str) -> Result<String, Error> {
    let symbol =
        Symbol::from_value(value).ok_or_else(|| Error::new(ruby.exception_type_error(), what))?;
    Ok(symbol.name()?.into_owned())
}

/// A phase from its symbol: `TypeError` for a non-Symbol, `RangeError` for a
/// Symbol that names no phase.
pub fn phase(ruby: &Ruby, value: Value) -> Result<Phase, Error> {
    let name = symbol(ruby, value, "a phase is a Symbol")?;
    names::phase_named(&name).ok_or_else(|| range(ruby, "not a phase"))
}

/// The core's `Limits::new`.
fn limits(ruby: &Ruby, phase_budget: Number, per_replica: Number) -> Result<Limits, Error> {
    Limits::new(phase_budget.at(), per_replica.index(ruby)?)
        .ok_or_else(|| range(ruby, "per_replica must be at least 1"))
}

/// A token as the core's: this gem's `FencingToken` or a sibling's.
fn token(ruby: &Ruby, value: Value) -> Result<FencingToken, Error> {
    let [raw] = fields(ruby, value, classes(ruby)?.token, ["raw"], Origin::Sibling)?;
    Number::try_convert(raw)?.token(ruby)
}

/// A token argument, read by [`token`].
#[derive(Clone, Copy)]
pub struct TokenValue(pub FencingToken);

impl TryConvert for TokenValue {
    fn try_convert(value: Value) -> Result<Self, Error> {
        token(&Ruby::get_with(value), value).map(Self)
    }
}

/// A ticket argument: only this gem's `Ticket`, through `Ticket::new`.
#[derive(Clone, Copy)]
pub struct TicketValue(pub Ticket);

impl TryConvert for TicketValue {
    fn try_convert(value: Value) -> Result<Self, Error> {
        let ruby = Ruby::get_with(value);
        let readers = ["partition", "generation"];
        let [partition, generation] =
            fields(&ruby, value, classes(&ruby)?.ticket, readers, Origin::Own)?;
        let (partition, generation) = (
            Number::try_convert(partition)?,
            Number::try_convert(generation)?,
        );
        crate::ticket(&ruby, partition, generation).map(Self)
    }
}

/// A limits argument: only this gem's `Limits`, through `Limits::new`.
#[derive(Clone, Copy)]
pub struct LimitsValue(pub Limits);

impl TryConvert for LimitsValue {
    fn try_convert(value: Value) -> Result<Self, Error> {
        let ruby = Ruby::get_with(value);
        let readers = ["phase_budget", "per_replica"];
        let [phase_budget, per_replica] =
            fields(&ruby, value, classes(&ruby)?.limits, readers, Origin::Own)?;
        let fields = (
            Number::try_convert(phase_budget)?,
            Number::try_convert(per_replica)?,
        );
        limits(&ruby, fields.0, fields.1).map(Self)
    }
}

/// A `HandoffMachines::FencingToken` of `token`, built by its `new`.
fn token_object(ruby: &Ruby, token: FencingToken) -> Result<Value, Error> {
    let class = ruby.get_inner(classes(ruby)?.token);
    class.funcall("new", (token.raw(),))
}

/// `Fence.new`, `Activate.new`, `Resume.new`: a token read as [`token`]
/// reads it, answered as this gem's own to keep in place of the one given.
pub fn check_token(ruby: &Ruby, held: TokenValue) -> Result<Value, Error> {
    token_object(ruby, held.0)
}

/// `Limits.new`.
pub fn check_limits(ruby: &Ruby, phase_budget: Number, per_replica: Number) -> Result<(), Error> {
    limits(ruby, phase_budget, per_replica).map(drop)
}

/// `Ticket.new`: the core's `Ticket::new`.
pub fn check_ticket(ruby: &Ruby, partition: Number, generation: Number) -> Result<(), Error> {
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

/// `Replica.new`, `Drain.new`, `Capture.new`, `Install.new`: any unsigned
/// 64-bit Integer, as the core's field is.
pub const fn check_u64(_value: Number) {}

/// The core's `Recipient`: this gem's `Replica`, or `:authority`.
fn recipient(ruby: &Ruby, value: Value) -> Result<Recipient, Error> {
    if let Some(symbol) = Symbol::from_value(value) {
        return if symbol.name()? == "authority" {
            Ok(Recipient::Authority)
        } else {
            Err(range(ruby, "the only recipient symbol is :authority"))
        };
    }
    let [id] = fields(ruby, value, classes(ruby)?.replica, ["id"], Origin::Own)?;
    Ok(Recipient::Replica(Number::try_convert(id)?.0))
}

/// The core's `Action` from a Symbol (`:quiesce` or `:discard`, the two that
/// carry no field) or one of this gem's action `Data` values.
fn action(ruby: &Ruby, value: Value) -> Result<Action, Error> {
    if let Some(symbol) = Symbol::from_value(value) {
        return match symbol.name()?.as_ref() {
            "quiesce" => Ok(Action::Quiesce),
            "discard" => Ok(Action::Discard),
            _other => Err(range(
                ruby,
                "only :quiesce and :discard are Symbols; every other action is its Data",
            )),
        };
    }
    let classes = classes(ruby)?;
    let index = (0..ACTIONS.len())
        .find(|&index| is(ruby, value, classes.actions[index]))
        .ok_or_else(|| {
            Error::new(
                ruby.exception_type_error(),
                "an action is an action Data (exactly its class) or a Symbol",
            )
        })?;
    let [field] = fields(ruby, value, classes.actions[index], ["field"], Origin::Own)?;
    let number = |field: Value| Number::try_convert(field).map(|number| number.0);
    Ok(match index {
        0 => Action::Drain {
            until: number(field)?,
        },
        1 => Action::Capture { at: number(field)? },
        2 => Action::Install {
            checkpoint: number(field)?,
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
pub fn check_order(
    ruby: &Ruby,
    ticket: TicketValue,
    to: Value,
    action: Value,
) -> Result<(), Error> {
    let (to, action) = (recipient(ruby, to)?, self::action(ruby, action)?);
    Order::new(ticket.0, to, action)
        .map(drop)
        .ok_or_else(|| range(ruby, "only the authority fences, and it does nothing else"))
}

/// A deadline: Integer nanoseconds up to `Duration::MAX`, since the core
/// saturates `now + phase_budget` past what a `now` can be.
fn deadline(ruby: &Ruby, value: Value) -> Result<Duration, Error> {
    if Integer::from_value(value).is_none() {
        return Err(Error::new(
            ruby.exception_type_error(),
            "a deadline is Integer nanoseconds",
        ));
    }
    let nanos = u128::try_convert(value)?;
    if nanos > Duration::MAX.as_nanos() {
        return Err(range(ruby, "a deadline is at most Duration::MAX"));
    }
    Ok(Duration::from_nanos_u128(nanos))
}

/// `Status.new`: the core's `Status::new`. Answers the token as this gem's
/// own (or nil), to keep in place of the one given.
#[allow(clippy::too_many_arguments)] // One per field of the core's `Status`.
pub fn check_status(
    ruby: &Ruby,
    ticket: TicketValue,
    source: Number,
    target: Number,
    phase: Value,
    base: Number,
    until: Option<Number>,
    checkpoint: Option<Number>,
    token: Option<TokenValue>,
    deadline: Value,
) -> Result<Option<Value>, Error> {
    let token = token.map(|token| token.0);
    let status = Status::new(
        ticket.0,
        source.0,
        target.0,
        self::phase(ruby, phase)?,
        base.0,
        until.map(|until| until.0),
        checkpoint.map(|checkpoint| checkpoint.0),
        token,
        self::deadline(ruby, deadline)?,
    );
    status.ok_or_else(|| range(ruby, "no live transfer reports this status"))?;
    token.map(|token| token_object(ruby, token)).transpose()
}
