//! Thin binding to the bounded partition handoff coordinator.
mod classes;
mod handoff;
mod init;
mod names;
mod values;

use handoff_machines::Ticket;
use magnus::{Error, Ruby, Value};
use matryona_machines::ruby::integer;

use handoff::Handoff;

/// Concurrent transfers compiled into the binding.
const CAPACITY: usize = 16;
/// Drain window words: 64 offsets each.
const WORDS: usize = 4;

type Core = handoff_machines::Handoff<CAPACITY, WORDS>;

/// The scope a ticket's generation carries: `Ticket#scope`.
fn scope_of(ruby: &Ruby, ticket: Value) -> Result<u16, Error> {
    values::ticket(ruby, ticket).map(|ticket| ticket.scope())
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
