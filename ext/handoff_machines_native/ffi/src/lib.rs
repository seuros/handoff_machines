//! Thin binding to the bounded partition handoff coordinator.
mod classes;
mod handoff;
mod init;
mod names;
mod values;

use std::cmp::Ordering;

use handoff_machines::{FencingToken, Ticket};
use magnus::{Error, Ruby, Value};
use matryona_machines::ruby::integer;

use handoff::Handoff;

/// Concurrent transfers compiled into the binding.
const CAPACITY: usize = 16;
/// Drain window words: 64 offsets each.
const WORDS: usize = 4;

type Core = handoff_machines::Handoff<CAPACITY, WORDS>;

/// A fencing token from its raw Integer: `FencingToken::from_raw`, or the
/// `RangeError` its `None` stands for.
fn raw_token(value: Value) -> Result<FencingToken, Error> {
    FencingToken::from_raw(integer(value, "raw")?).ok_or_else(|| {
        Error::new(
            Ruby::get_with(value).exception_range_error(),
            "a fencing token has a positive sequence number",
        )
    })
}

/// The scope a ticket's generation carries: `Ticket#scope`.
fn scope_of(ruby: &Ruby, ticket: Value) -> Result<u16, Error> {
    values::ticket(ruby, ticket).map(|ticket| ticket.scope())
}

/// `HandoffMachines.native_raw`: what `FencingToken.new` validates.
fn validate_raw(raw: Value) -> Result<(), Error> {
    raw_token(raw).map(drop)
}

/// `FencingToken#scope`; reading the token is what validates it.
fn token_scope(ruby: &Ruby, held: Value) -> Result<u16, Error> {
    values::token(ruby, held).map(FencingToken::scope)
}

/// `FencingToken#order`: -1, 0, 1, or `nil` across scopes.
fn token_order(ruby: &Ruby, held: Value, other: Value) -> Result<Option<i8>, Error> {
    let (held, other) = (values::token(ruby, held)?, values::token(ruby, other)?);
    Ok(held.order(other).map(|order| match order {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }))
}

/// `FencingToken#newer_than?`.
fn token_newer(ruby: &Ruby, held: Value, other: Value) -> Result<bool, Error> {
    let (held, other) = (values::token(ruby, held)?, values::token(ruby, other)?);
    Ok(held.is_newer_than(other))
}

/// The core's `Ticket::new` of two Integers, or the `RangeError` its `None`
/// stands for.
fn ticket(ruby: &Ruby, partition: Value, generation: Value) -> Result<Ticket, Error> {
    let partition = integer(partition, "partition")?;
    Ticket::new(partition, integer(generation, "generation")?).ok_or_else(|| {
        Error::new(
            ruby.exception_range_error(),
            "no coordinator issues a generation with a zero sequence",
        )
    })
}
