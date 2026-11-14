//! Thin binding to the bounded partition handoff coordinator.
mod classes;
mod handoff;
mod init;
mod names;
mod values;

use std::{cmp::Ordering, time::Duration};

use handoff_machines::{FencingToken, Ticket};
use magnus::{Error, Integer, Ruby, TryConvert, Value};

use handoff::Handoff;
use values::{TicketValue, TokenValue};

/// Concurrent transfers compiled into the binding.
const CAPACITY: usize = 16;
/// Drain window words: 64 offsets each.
const WORDS: usize = 4;

type Core = handoff_machines::Handoff<CAPACITY, WORDS>;

/// `TypeError` unless `value` is an Integer.
fn integer(value: Value, message: &'static str) -> Result<(), Error> {
    match Integer::from_value(value) {
        Some(_) => Ok(()),
        None => Err(Error::new(
            Ruby::get_with(value).exception_type_error(),
            message,
        )),
    }
}

#[derive(Clone, Copy)]
struct Number(u64);

impl TryConvert for Number {
    fn try_convert(value: Value) -> Result<Self, Error> {
        integer(value, "expected an Integer (unsigned 64-bit)")?;
        u64::try_convert(value).map(Self)
    }
}

impl Number {
    const fn at(self) -> Duration {
        Duration::from_nanos(self.0)
    }

    /// A fencing token from its raw value: `FencingToken::from_raw`, or the
    /// `RangeError` its `None` stands for.
    fn token(self, ruby: &Ruby) -> Result<FencingToken, Error> {
        FencingToken::from_raw(self.0).ok_or_else(|| {
            Error::new(
                ruby.exception_range_error(),
                "a fencing token has a positive sequence number",
            )
        })
    }

    fn index(self, ruby: &Ruby) -> Result<usize, Error> {
        usize::try_from(self.0)
            .map_err(|_| Error::new(ruby.exception_range_error(), "integer too large"))
    }
}

/// A scope: an Integer in `0..=65535`.
#[derive(Clone, Copy)]
struct Scope(u16);

impl TryConvert for Scope {
    fn try_convert(value: Value) -> Result<Self, Error> {
        integer(value, "scope must be an Integer")?;
        u16::try_convert(value).map(Self).map_err(|_| {
            Error::new(
                Ruby::get_with(value).exception_range_error(),
                "scope must be in 0..=65535",
            )
        })
    }
}

/// The scope a ticket's generation carries: `Ticket#scope`.
const fn scope_of(ticket: TicketValue) -> u16 {
    ticket.0.scope()
}

/// `HandoffMachines.native_raw`: what `FencingToken.new` validates.
fn validate_raw(ruby: &Ruby, raw: Number) -> Result<(), Error> {
    raw.token(ruby).map(drop)
}

/// `FencingToken#scope`; reading the token is what validates it.
const fn token_scope(token: TokenValue) -> u16 {
    token.0.scope()
}

/// `FencingToken#order`: -1, 0, 1, or `nil` across scopes.
fn token_order(token: TokenValue, other: TokenValue) -> Option<i8> {
    token.0.order(other.0).map(|order| match order {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    })
}

/// `FencingToken#newer_than?`.
const fn token_newer(token: TokenValue, other: TokenValue) -> bool {
    token.0.is_newer_than(other.0)
}

/// The core's `Ticket::new`, or the `RangeError` its `None` stands for.
fn ticket(ruby: &Ruby, partition: Number, generation: Number) -> Result<Ticket, Error> {
    Ticket::new(partition.0, generation.0).ok_or_else(|| {
        Error::new(
            ruby.exception_range_error(),
            "no coordinator issues a generation with a zero sequence",
        )
    })
}
